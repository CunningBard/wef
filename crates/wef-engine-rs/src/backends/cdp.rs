use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use tungstenite::{Message, connect};
use url::Url;

use crate::{
    BrowserPolicy, BrowserRunRequest, BrowserRunResult, HostError, HttpRequest, HttpResponse,
    UreqHost, WefHost, browser::MAX_PAYLOAD_BYTES,
};
use wef_core::{BrowserCaptureTask, browser_capture_matches};

/// Opt-in Chromium DevTools Protocol host. It connects only to an explicitly
/// supplied local debugging endpoint and never launches a browser process.
///
/// Tasks are data-only and stay data-only: the host observes network bodies
/// through the CDP `Network` domain, page-parsed values through a fixed
/// `JSON.parse` tap, and embedded JSON through the `DOM` domain. The two
/// JavaScript snippets below (`TAP_INSTALL`, `TAP_DRAIN`) are immutable constants — they contain
/// no task data, no selectors, no URLs — so there is no code generation and
/// source-supplied code is never evaluated.
pub struct CdpBrowserHost {
    debug_url: Url,
    policy: BrowserPolicy,
    http: UreqHost,
    sessions: BTreeMap<String, String>,
    next_session: u64,
}

impl CdpBrowserHost {
    pub fn new(debug_url: &str, policy: BrowserPolicy) -> Result<Self, HostError> {
        let debug_url = Url::parse(debug_url)
            .map_err(|error| HostError::Message(format!("invalid CDP URL: {error}")))?;
        if !matches!(debug_url.scheme(), "http" | "https")
            || !matches!(
                debug_url.host_str(),
                Some("127.0.0.1") | Some("localhost") | Some("::1")
            )
        {
            return Err(HostError::Message(
                "CDP endpoint must be an explicit local HTTP(S) URL".into(),
            ));
        }
        Ok(Self {
            debug_url,
            policy,
            http: UreqHost::default(),
            sessions: BTreeMap::new(),
            next_session: 0,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, HostError> {
        self.debug_url
            .join(path)
            .map_err(|error| HostError::Message(format!("invalid CDP endpoint: {error}")))
    }

    /// Adds one allowed browser origin to a live host. Long-lived processes
    /// (a REPL, an app) load packages over time; the allowlist is the union
    /// of every loaded package's base URLs, so it only ever grows. Ports:
    /// a set insertion guarded by the same lock as the host itself.
    pub fn add_allowed_origin(&mut self, origin: impl Into<String>) {
        self.policy.allowed_origins.insert(origin.into());
    }

    /// Test-only session injector: the real flow mints sessions inside
    /// `run_browser` (live CDP), which unit tests must not touch.
    #[cfg(test)]
    pub(crate) fn inject_session(&mut self, token: String, cookie: String) {
        self.sessions.insert(token, cookie);
    }

    fn close_target(&self, id: &str) {
        // Best effort: never fail a run because cleanup failed.
        if let Ok(endpoint) = self.endpoint(&format!("json/close/{id}")) {
            let _ = ureq::get(endpoint.as_str()).call();
        }
    }

    fn check_payload(&self, payload: &Option<Value>) -> Result<(), HostError> {
        if let Some(payload) = payload {
            let len = serde_json::to_string(payload)
                .map(|text| text.len())
                .unwrap_or(usize::MAX);
            let max = MAX_PAYLOAD_BYTES.max(self.policy.max_payload_bytes);
            if len > max {
                return Err(HostError::Message(
                    "browser payload exceeds host byte limit".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Target {
    id: Option<String>,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_url: String,
}

type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// Read slice for the CDP event loop. Every socket read returns after at
/// most this long so deadline checks run even on an idle page.
const READ_SLICE_MS: u64 = 250;
/// Upper bound for any single request/response round-trip.
const CALL_TIMEOUT_MS: u64 = 15_000;
/// Bodies larger than this are never parsed (final cap enforced separately).
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Fixed page tap, loaded verbatim from `tap_install.js` via `include_str!`.
/// Using `include_str!` (not `format!`, not concatenation) is the whole point:
/// the compiler guarantees these bytes can never carry task data, so there is
/// no string-building and no injection vector. The tap records values passing
/// through `JSON.parse` (e.g. client-decrypted API envelopes the `Network`
/// domain only ever sees ciphertext for) into a bounded buffer; matching
/// stays in Rust (`browser_capture_matches`).
const TAP_INSTALL: &str = include_str!("tap_install.js");

/// Fixed drain for the tap above, loaded verbatim from `tap_drain.js`.
/// Moves buffered values out and returns them — safe to evaluate on every
/// tick, identical bytes for every task.
const TAP_DRAIN: &str = include_str!("tap_drain.js");

/// Minimal CDP session: monotonically increasing command ids plus an event
/// stash. Ports: this is just a class holding an open websocket, an integer
/// counter, and a list. `Session` owns the socket, so there are no lifetime
/// annotations to translate — Kotlin/Swift hold the connection the same way.
struct Session {
    socket: Socket,
    next_id: u64,
    events: Vec<Value>,
}

impl Session {
    fn new(socket: Socket) -> Self {
        let mut session = Self {
            socket,
            next_id: 1,
            events: Vec::new(),
        };
        session.set_read_timeout(Some(Duration::from_millis(READ_SLICE_MS)));
        session
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        // CDP endpoints are constrained to plain local ws:// by `new`, so the
        // stream is always `Plain`. Timeouts only slice blocking reads; a timed
        // out read surfaces as a retriable tick, never as data loss.
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = self.socket.get_mut() {
            let _ = stream.set_read_timeout(timeout);
        }
    }

    /// Sends a command and waits for its response, stashing any events seen
    /// along the way for the capture loop to process.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, HostError> {
        let id = self.next_id;
        self.next_id += 1;
        self.socket
            .send(Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .map_err(|error| HostError::Message(error.to_string()))?;
        let deadline = Instant::now() + Duration::from_millis(CALL_TIMEOUT_MS);
        loop {
            let message = read_one(&mut self.socket)?;
            match message {
                None => {
                    if Instant::now() >= deadline {
                        return Err(HostError::Message(format!(
                            "CDP {method}: timed out waiting for a response"
                        )));
                    }
                }
                Some(value) => {
                    let response_id = value.get("id").and_then(Value::as_u64);
                    if response_id == Some(id) {
                        if let Some(error) = value.get("error") {
                            return Err(HostError::Message(format!("CDP {method}: {error}")));
                        }
                        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // An event (or a stray response): stash it, keep waiting.
                    self.events.push(value);
                }
            }
        }
    }

    /// Takes stashed events collected while waiting on `call`.
    fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    /// Reads the next live event, waiting up to `timeout`. Returns `None`
    /// on a quiet slice. Responses are stashed; only events are returned.
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<Value>, HostError> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let message = read_one(&mut self.socket)?;
            match message {
                None => return Ok(None),
                Some(value) => {
                    if value.get("id").is_some() {
                        self.events.push(value);
                        continue;
                    }
                    return Ok(Some(value));
                }
            }
        }
    }
}

/// Reads one CDP message. `Ok(None)` is a quiet 250ms slice, not an error.
fn read_one(socket: &mut Socket) -> Result<Option<Value>, HostError> {
    let message = match socket.read() {
        Ok(message) => message,
        Err(tungstenite::Error::Io(error))
            if error.kind() == std::io::ErrorKind::TimedOut
                || error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            return Ok(None);
        }
        Err(error) => return Err(HostError::Message(error.to_string())),
    };
    match message {
        Message::Text(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| HostError::Message(format!("invalid CDP message: {error}"))),
        Message::Close(_) => Err(HostError::Message(
            "browser closed the debugging connection".into(),
        )),
        // Ping/Pong/Binary/Frame: nothing to do, keep waiting.
        _ => Ok(None),
    }
}

impl WefHost for CdpBrowserHost {
    fn request(&mut self, mut request: HttpRequest) -> Result<HttpResponse, HostError> {
        let session = request
            .browser_session
            .take()
            .ok_or(HostError::Unsupported)?;
        let cookie = self
            .sessions
            .get(&session)
            .ok_or(HostError::Unsupported)?
            .clone();
        // The session jar holds the whole browser-profile cookie set, so a
        // session-authenticated request may only target policy origins.
        // Without this, any source could ship the jar to an arbitrary URL
        // just by attaching a valid token.
        self.policy.allow_url(&request.url)?;
        let headers = request.headers.get_or_insert_with(BTreeMap::new);
        headers.insert("Cookie".into(), cookie);
        self.http.request(request)
    }

    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        self.policy.validate(&request)?;
        let wait_ms = request
            .timeout_ms
            .unwrap_or(self.policy.max_timeout_ms)
            .min(self.policy.max_timeout_ms);

        // Open an ephemeral tab. With `html` we start blank and inject below,
        // so no uncontrolled navigation happens.
        let start_url = if request.html.is_some() {
            "about:blank".to_string()
        } else {
            request.url.clone()
        };
        let endpoint = self.endpoint(&format!(
            "json/new?{}",
            url::form_urlencoded::byte_serialize(start_url.as_bytes()).collect::<String>()
        ))?;
        let target: Target = ureq::put(endpoint.as_str())
            .send_empty()
            .map_err(|error| HostError::Message(error.to_string()))?
            .body_mut()
            .read_json()
            .map_err(|error| HostError::Message(error.to_string()))?;
        let target_id = target.id.clone();
        let outcome = self.run_in_target(&target, &request, wait_ms);
        if let Some(id) = target_id {
            self.close_target(&id);
        }
        outcome
    }
}

impl CdpBrowserHost {
    fn run_in_target(
        &mut self,
        target: &Target,
        request: &BrowserRunRequest,
        wait_ms: u64,
    ) -> Result<BrowserRunResult, HostError> {
        let (socket, _) = connect(target.websocket_url.as_str())
            .map_err(|error| HostError::Message(error.to_string()))?;
        let mut session = Session::new(socket);
        let _ = session.call("Page.enable", json!({}))?;
        let _ = session.call(
            "Network.enable",
            json!({"maxTotalBufferSize": 20_000_000u32, "maxResourceBufferSize": 5_000_000u32}),
        )?;
        let outcome = self.run_task(&mut session, request, &request.task, wait_ms);
        // Restore blocking reads on the way out; the socket closes on drop.
        session.set_read_timeout(None);
        outcome
    }

    fn run_task(
        &mut self,
        session: &mut Session,
        request: &BrowserRunRequest,
        task: &wef_core::BrowserTask,
        wait_ms: u64,
    ) -> Result<BrowserRunResult, HostError> {
        match task {
            wef_core::BrowserTask::Snapshot(task) => {
                self.load(session, request)?;
                let payload = snapshot_payload(session, &task.selector);
                self.check_payload(&payload)?;
                let (session_token, url) = self.finish_session(session, &request.url)?;
                Ok(BrowserRunResult {
                    url,
                    payload,
                    session: session_token,
                })
            }
            wef_core::BrowserTask::Capture(task) => {
                self.load(session, request)?;
                // Best effort: enables observation of client-decrypted values
                // (e.g. envelopes the Network domain only sees ciphertext
                // for). Network bodies remain the primary source; a missing
                // tap only loses values no other channel can see.
                let _ = session.call(
                    "Runtime.evaluate",
                    json!({"expression": TAP_INSTALL, "returnByValue": true}),
                );
                let deadline = Instant::now() + Duration::from_millis(wait_ms);
                let payload = self.run_single_capture(session, task, deadline)?;
                self.check_payload(&payload)?;
                let (session_token, url) = self.finish_session(session, &request.url)?;
                Ok(BrowserRunResult {
                    url,
                    payload,
                    session: session_token,
                })
            }
        }
    }

    /// Navigates to `request.url`, or installs `request.html` with `url` as
    /// its base URL via `Page.setDocumentContent` (no script evaluation).
    fn load(
        &mut self,
        session: &mut Session,
        request: &BrowserRunRequest,
    ) -> Result<(), HostError> {
        if let Some(html) = &request.html {
            let tree = session.call("Page.getFrameTree", json!({}))?;
            let frame_id = tree
                .pointer("/frameTree/frame/id")
                .and_then(Value::as_str)
                .ok_or_else(|| HostError::Message("CDP Page.getFrameTree: no frame".into()))?;
            let doc = format!(
                "<base href=\"{}\">{}",
                request.url.replace('"', "%22"),
                html
            );
            let _ = session.call(
                "Page.setDocumentContent",
                json!({"frameId": frame_id, "html": doc}),
            )?;
            return Ok(());
        }
        let _ = session.call("Page.navigate", json!({"url": request.url}))?;
        Ok(())
    }

    /// Collects network bodies until the first spec match or the deadline,
    /// then falls back to the `snapshot` selector when present.
    fn run_single_capture(
        &mut self,
        session: &mut Session,
        task: &BrowserCaptureTask,
        deadline: Instant,
    ) -> Result<Option<Value>, HostError> {
        let mut capture = Capture::new(task.capture.clone());
        drain_until(session, &mut capture, deadline)?;
        let first = std::mem::take(&mut capture.matches).into_iter().next();
        if let Some(first) = first {
            return Ok(wrap_unmatched(Some(first), &capture));
        }
        let fallback = task
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot_payload(session, &snapshot.selector));
        Ok(wrap_unmatched(fallback, &capture))
    }

    fn finish_session(
        &mut self,
        session: &mut Session,
        fallback_url: &str,
    ) -> Result<(String, String), HostError> {
        // Ports: straightforward loops; a cookie entry survives only when
        // both `name` and `value` are present strings.
        let mut cookie_parts: Vec<String> = Vec::new();
        if let Ok(cookies) = session.call("Network.getCookies", json!({}))
            && let Some(list) = cookies.get("cookies").and_then(Value::as_array)
        {
            for cookie in list {
                let name = cookie.get("name").and_then(Value::as_str);
                let value = cookie.get("value").and_then(Value::as_str);
                match (name, value) {
                    (Some(name), Some(value)) => {
                        cookie_parts.push(format!("{name}={value}"));
                    }
                    _ => continue,
                }
            }
        }
        let cookie = cookie_parts.join("; ");
        let token = mint_session_token(self.next_session);
        self.next_session += 1;
        self.sessions.insert(token.clone(), cookie);
        // Current URL without script evaluation: the navigation history's
        // selected entry, falling back to the requested URL.
        let mut url = fallback_url.to_owned();
        if let Ok(history) = session.call("Page.getNavigationHistory", json!({}))
            && let Some(current) = current_history_url(&history)
            && current != "about:blank"
        {
            url = current;
        }
        self.policy.allow_url(&url)?;
        Ok((token, url))
    }
}

/// Mints an opaque session token. Tokens authenticate the whole profile
/// cookie jar, so sequential ids are out: 128 bits of OS randomness, with a
/// counter-derived fallback that never fails the run on platforms without
/// an entropy source. Ports: `UUID.randomUUID()` / `SecureRandom` — the
/// requirement is unpredictability across sources sharing one host, not
/// the exact encoding.
pub(crate) fn mint_session_token(counter: u64) -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        let mut token = String::with_capacity(12 + 32);
        token.push_str("cdp-session-");
        for byte in bytes {
            token.push_str(&format!("{byte:02x}"));
        }
        return token;
    }
    format!("cdp-session-{counter:016x}")
}

/// Ports: `entries[currentIndex].url` with bounds checks; `null` when the
/// history is missing or malformed (the caller falls back to the task URL).
fn current_history_url(history: &Value) -> Option<String> {
    let index = history.get("currentIndex")?.as_u64()? as usize;
    let entries = history.get("entries")?.as_array()?;
    let url = entries.get(index)?.get("url")?.as_str()?;
    Some(url.to_owned())
}

/// Bodies matched against one [`BrowserCaptureTask`] spec, in capture order
/// and deduplicated by exact JSON equality (spec section 4). Owns a copy of
/// the spec so callers never juggle borrows — ports just hold the object.
///
/// With WEF 0.0.4 `capture.includeUnmatched`, rejected observations land in
/// `raw` instead of being dropped; the payload wraps both lists (see
/// `wrap_unmatched`). Ports: two lists plus one boolean.
struct Capture {
    spec: wef_core::BrowserCaptureSpec,
    include_unmatched: bool,
    seen: HashSet<String>,
    matches: Vec<Value>,
    raw: Vec<Value>,
    /// requestIds whose response headers arrived and whose URL passed the
    /// `urlContains` pre-filter, awaiting `loadingFinished`.
    pending: HashMap<String, String>,
}

impl Capture {
    fn new(spec: wef_core::BrowserCaptureSpec) -> Self {
        let include_unmatched = spec.include_unmatched.unwrap_or(false);
        Self {
            spec,
            include_unmatched,
            seen: HashSet::new(),
            matches: Vec::new(),
            raw: Vec::new(),
            pending: HashMap::new(),
        }
    }

    fn push_candidate(&mut self, value: Value) {
        if browser_capture_matches(&self.spec, &value) {
            // Ports: `value.to_string()` is canonical-JSON serialization used
            // only as a dedupe key (spec: "deduplicated by exact JSON equality").
            let key = value.to_string();
            if self.matches.len() >= 200 {
                return;
            }
            if self.seen.insert(key) {
                self.matches.push(value);
            }
            return;
        }
        if self.include_unmatched {
            let key = value.to_string();
            if self.raw.len() >= 200 {
                return;
            }
            if self.seen.insert(key) {
                self.raw.push(value);
            }
        }
    }
}

/// Ports: without `includeUnmatched` the section-4 value passes through
/// untouched (exact 0.0.3 shapes); with it, the payload is the
/// `{"matched": …, "unmatched": […]}` envelope from WEF 0.0.4 section 4.
fn wrap_unmatched(matched: Option<Value>, capture: &Capture) -> Option<Value> {
    if !capture.include_unmatched {
        return matched;
    }
    Some(json!({"matched": matched, "unmatched": capture.raw}))
}

/// Ports: `get_str` is `optString` (Android `JSONObject.optString`) or
/// `dict[key] as? String ?? ""` — missing and non-string values read as "".
fn get_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// MIME types that can never hold the JSON the capture predicate looks for.
/// Everything else (JSON, text, JavaScript, unknown) is fetched and tried.
fn skippable_mime(mime: &str) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime.starts_with("image/")
        || mime.starts_with("font/")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime == "application/wasm"
        || mime == "text/css"
}

/// Processes stashed + live CDP events until the first spec match or
/// `deadline`. Ports: one loop — poll, handle, stash, tap — with an
/// early return once anything matched.
fn drain_until(
    session: &mut Session,
    capture: &mut Capture,
    deadline: Instant,
) -> Result<(), HostError> {
    for event in session.take_events() {
        handle_event(session, capture, &event)?;
        if !capture.matches.is_empty() {
            return Ok(());
        }
    }
    while Instant::now() < deadline {
        if !capture.matches.is_empty() {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let slice = remaining.min(Duration::from_millis(READ_SLICE_MS));
        if let Some(event) = session.poll_event(slice)? {
            handle_event(session, capture, &event)?;
            if !capture.matches.is_empty() {
                return Ok(());
            }
        }
        // Stashed follow-ups from body fetches, plus the fixed parse tap.
        for event in session.take_events() {
            handle_event(session, capture, &event)?;
            if !capture.matches.is_empty() {
                return Ok(());
            }
        }
        if drain_tap(session, capture) && !capture.matches.is_empty() {
            return Ok(());
        }
    }
    Ok(())
}

/// Handles one CDP event. Returns `true` when a new body matched.
/// Ports: switch on the string field `method`; unknown methods are ignored.
fn handle_event(
    session: &mut Session,
    capture: &mut Capture,
    event: &Value,
) -> Result<bool, HostError> {
    let method = get_str(event, "method");
    let params = event.get("params").unwrap_or(&Value::Null);
    if method == "Network.responseReceived" {
        return Ok(handle_response_received(capture, params));
    }
    if method == "Network.loadingFinished" {
        return handle_loading_finished(session, capture, params);
    }
    if method == "Network.loadingFailed" {
        let request_id = get_str(params, "requestId");
        if !request_id.is_empty() {
            capture.pending.remove(request_id.as_str());
        }
        return Ok(false);
    }
    Ok(false)
}

fn handle_response_received(capture: &mut Capture, params: &Value) -> bool {
    let request_id = get_str(params, "requestId");
    if request_id.is_empty() {
        return false;
    }
    let response = params.get("response").unwrap_or(&Value::Null);
    let url = get_str(response, "url");
    if let Some(needle) = &capture.spec.url_contains
        && !url.contains(needle.as_str())
    {
        return false;
    }
    if skippable_mime(get_str(response, "mimeType").as_str()) {
        return false;
    }
    capture
        .pending
        .insert(request_id.to_owned(), url.to_owned());
    false
}

fn handle_loading_finished(
    session: &mut Session,
    capture: &mut Capture,
    params: &Value,
) -> Result<bool, HostError> {
    let request_id = get_str(params, "requestId");
    if capture.pending.remove(request_id.as_str()).is_none() {
        return Ok(false);
    }
    let before = capture.matches.len();
    if let Some(body) = response_body(session, request_id.as_str()) {
        capture.push_candidate(body);
    }
    Ok(capture.matches.len() > before)
}

/// Moves values buffered by the fixed parse tap into the Rust matcher.
/// Never fails the run: a mid-navigation evaluate error just means no new
/// values this tick. Returns `true` when a new body matched.
fn drain_tap(session: &mut Session, capture: &mut Capture) -> bool {
    let before = capture.matches.len();
    let values = session
        .call(
            "Runtime.evaluate",
            json!({"expression": TAP_DRAIN, "returnByValue": true}),
        )
        .ok()
        .and_then(|result| result.pointer("/result/value").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    for value in values {
        // Guard the dedupe serialization against pathological values.
        if value.to_string().len() > MAX_BODY_BYTES {
            continue;
        }
        capture.push_candidate(value);
    }
    capture.matches.len() > before
}

/// Fetches a finished response body and parses it as JSON. Returns `None`
/// for binary bodies (base64), oversized bodies, fetch errors, and non-JSON.
/// Ports: straightforward ifs; `response_body` is nullable in Kotlin/Swift.
fn response_body(session: &mut Session, request_id: &str) -> Option<Value> {
    let result = session
        .call("Network.getResponseBody", json!({"requestId": request_id}))
        .ok()?;
    let is_base64 = result
        .get("base64Encoded")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_base64 {
        return None;
    }
    let body = result.get("body")?.as_str()?;
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    serde_json::from_str(body).ok()
}

/// Reads the parsed JSON text of `selector` through the `DOM` domain only.
/// Returns `None` when the element is absent or unparseable (spec: `null`).
/// Ports: three sequential calls with null checks between them.
fn snapshot_payload(session: &mut Session, selector: &str) -> Option<Value> {
    let root = document_root(session).ok()?;
    let found = session
        .call(
            "DOM.querySelector",
            json!({"nodeId": root, "selector": selector}),
        )
        .ok()?;
    let node = found.get("nodeId")?.as_u64()?;
    if node == 0 {
        return None;
    }
    let html = session
        .call("DOM.getOuterHTML", json!({"nodeId": node}))
        .ok()?;
    let outer = html.get("outerHTML")?.as_str()?;
    // `<script …>JSON</script>`: the payload is the inner text.
    let inner = inner_text(outer)?;
    serde_json::from_str(inner.trim()).ok()
}

fn document_root(session: &mut Session) -> Result<u64, HostError> {
    let doc = session.call("DOM.getDocument", json!({"depth": -1, "pierce": true}))?;
    doc.pointer("/root/nodeId")
        .and_then(Value::as_u64)
        .ok_or_else(|| HostError::Message("CDP DOM.getDocument: no root".into()))
}

/// Inner text of one element's outer HTML (`<tag …>inner</tag>`).
fn inner_text(outer: &str) -> Option<&str> {
    let start = outer.find('>')? + 1;
    let end = outer.rfind('<')?;
    if end <= start {
        return None;
    }
    Some(&outer[start..end])
}
