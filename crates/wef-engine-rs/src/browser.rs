//! Declarative browser bridge (WEF 0.0.4).
//!
//! Sources submit data-only [`BrowserTask`] values; the host owns every line
//! of JavaScript that runs in the page. There is no `script` field.
//!
//! # Reference algorithm (port this, not the CDP calls)
//!
//! Any language can reimplement the host with these observable steps:
//!
//! 1. `validate`: require consent, check `url` is HTTP(S) on an allowed
//!    origin, `timeoutMs <= maxTimeoutMs`, then [`BrowserTask::validate`].
//! 2. Open an ephemeral tab (fresh profile, no user data). If `html` is set,
//!    load it with `url` as base and skip network navigation.
//! 3. `snapshot` task: return the parsed JSON text of `selector`
//!    (`null` when absent/unparseable).
//! 4. `capture` task: observe every network response body and parsed JSON
//!    value; keep those where `browser_capture_matches(spec, value)` is true
//!    (see `wef-core`). Dedupe exact-equal values.
//! 5. Wait until the first match or `timeoutMs`, then return it; else try
//!    the `snapshot` fallback; else return `null`. Multi-page lists are the
//!    source's job: it signs plain requests and walks `meta` pagination
//!    natively (WEF 0.0.4 store flow) instead of auto-clicking.
//! 6. Enforce `maxPayloadBytes` on the returned payload, close the tab even
//!    on error, and mint an opaque `session` scoped to the source/profile.
//!
//! The CDP host in `cdp.rs` is one backend for this algorithm; a Playwright,
//! WebView, or headless-DOM backend conforms if the six steps match.
//!
//! # Backend map (same steps, different platform APIs)
//!
//! The step numbers above are the contract; each backend implements them with
//! whatever observation channel the platform provides. Source packages cannot
//! tell the difference.
//!
//! | Step | This CDP backend | Android WebView (Mihon-style) | iOS WKWebView (Aidoku-style) |
//! |------|----------------|-------------------------------|--------------------------------|
//! | 2. Isolated page | `json/new` tab + close on exit | fresh `WebView` instance (clear data) or incognito context; `loadDataWithBaseURL` for `html` | fresh `WKWebView` + non-persistent `WKWebsiteDataStore`; `loadHTMLString(_:baseURL:)` for `html` |
//! | 3. Snapshot read | `DOM.getDocument/querySelector/getOuterHTML`, JSON parsed in Rust | fixed `evaluateJavascript("document.querySelector(…​).textContent")` one-liner, `org.json` parse in Kotlin | fixed `evaluateJavaScript` one-liner, `JSONSerialization` in Swift |
//! | 4a. Network bodies | `Network.enable` + `getResponseBody`, matched in Rust | `WebViewClient.shouldInterceptRequest`: read the bytes (pass-through fetch shares the cookie jar) | no https-body API: fetch the API natively with cookies from `WKHTTPCookieStore`, or rely on 4b |
//! | 4b. Parsed values | fixed `backends/tap_install.js` proxy on `JSON.parse` + `tap_drain.js` poll | same fixed tap via `addJavascriptInterface`-free `evaluateJavascript` (see `backends::android`), or a `WKUserScript` at document start on iOS (see `backends::ios`) | `WKUserScript` (document start, main frame only) + `evaluateJavaScript` drain — on iOS this is the *primary* channel, not the fallback |
//! | 6. Session | `Network.getCookies` → opaque token for `browserSession` HTTP | `CookieManager.getInstance().getCookie(url)` → same token scheme | `WKHTTPCookieStore.getAllCookies` → same token scheme |
//!
//! Notes for porters:
//!
//! - Mobile WebViews have no `Network`/`DOM` domains, so backends there will
//!   evaluate small *fixed* snippets (the prepared-statement pattern: constant
//!   code, task data passed as JSON arguments, never string-built). That keeps
//!   the data-only property — sources send data only; the host owns all page
//!   code. Per-platform sheets: [`crate::backends::android`],
//!   [`crate::backends::ios`]; desktop quirks: [`crate::backends::desktop`].
//! - `backends/tap_install.js` / `backends/tap_drain.js` are deliberately
//!   dependency-free and
//!   interpolation-free so ports can ship them verbatim as `WKUserScript` /
//!   asset strings. The `cdp_tap_snippets_carry_no_task_data` test pins that.
//! - Encrypted envelopes (site decrypts client-side before render) are the
//!   reason step 4 observes *both* channels: either one may be ciphertext
//!   while the other carries the plaintext the predicate matches.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

pub use wef_core::{
    BrowserCaptureSpec, BrowserCaptureTask, BrowserSnapshotSpec, BrowserSnapshotTask, BrowserTask,
};

use crate::{HostError, HttpRequest, HttpResponse, WefHost};

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

/// Maximum source-visible browser payload (5 MiB of JSON).
pub const MAX_PAYLOAD_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunRequest {
    pub url: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Data-only task. Sources describe what to capture; the host owns
    /// every line of JavaScript that runs in the page.
    pub task: BrowserTask,
}

impl BrowserRunRequest {
    pub fn task(url: String, task: BrowserTask) -> Self {
        Self {
            url,
            html: None,
            timeout_ms: None,
            task,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunResult {
    pub url: String,
    #[serde(default)]
    pub payload: Option<Value>,
    pub session: String,
}

/// Fixed policy applied by the deterministic browser reference host.
#[derive(Debug, Clone)]
pub struct BrowserPolicy {
    pub allowed_origins: BTreeSet<String>,
    pub consent_granted: bool,
    pub max_timeout_ms: u64,
    pub max_payload_bytes: usize,
}

impl BrowserPolicy {
    pub fn for_origins(origins: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed_origins: origins.into_iter().collect(),
            consent_granted: false,
            max_timeout_ms: 30_000,
            max_payload_bytes: MAX_PAYLOAD_BYTES,
        }
    }

    pub(crate) fn allow_url(&self, value: &str) -> Result<(), HostError> {
        let url = Url::parse(value)
            .map_err(|error| HostError::Message(format!("invalid browser URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HostError::Message("browser URL must use HTTP(S)".into()));
        }
        let origin = url.origin().ascii_serialization();
        if !self.allowed_origins.contains(&origin) {
            return Err(HostError::Message(format!(
                "browser origin is not allowed: {origin}"
            )));
        }
        Ok(())
    }

    pub(crate) fn validate(&self, request: &BrowserRunRequest) -> Result<(), HostError> {
        if !self.consent_granted {
            return Err(HostError::Message(
                "browser consent has not been granted".into(),
            ));
        }
        self.allow_url(&request.url)?;
        if request.timeout_ms.unwrap_or(self.max_timeout_ms) > self.max_timeout_ms {
            return Err(HostError::Message(
                "browser timeout exceeds host policy".into(),
            ));
        }
        if let Some(html) = &request.html
            && html.len() > self.max_payload_bytes
        {
            return Err(HostError::Message(
                "browser html exceeds host byte limit".into(),
            ));
        }
        request
            .task
            .validate()
            .map_err(|error| HostError::Message(format!("invalid browser task: {error}")))?;
        Ok(())
    }

    pub(crate) fn check_payload(&self, payload: &Option<Value>) -> Result<(), HostError> {
        if let Some(payload) = payload
            && serde_json::to_string(payload)
                .map(|text| text.len())
                .unwrap_or(usize::MAX)
                > self.max_payload_bytes
        {
            return Err(HostError::Message(
                "browser payload exceeds host byte limit".into(),
            ));
        }
        Ok(())
    }
}

pub trait InteractiveBrowserSurface {
    fn run_interactive(
        &mut self,
        request: BrowserRunRequest,
    ) -> Result<BrowserRunResult, HostError>;
    fn request_with_session(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError>;
}

pub struct InteractiveBrowserHost<S> {
    policy: BrowserPolicy,
    surface: S,
    sessions: BTreeSet<String>,
}

impl<S> InteractiveBrowserHost<S> {
    pub fn new(policy: BrowserPolicy, surface: S) -> Self {
        Self {
            policy,
            surface,
            sessions: BTreeSet::new(),
        }
    }
}

impl<S: InteractiveBrowserSurface> WefHost for InteractiveBrowserHost<S> {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        if !request
            .browser_session
            .as_ref()
            .is_some_and(|session| self.sessions.contains(session))
        {
            return Err(HostError::Unsupported);
        }
        self.policy.allow_url(&request.url)?;
        self.surface.request_with_session(request)
    }
    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        self.policy.validate(&request)?;
        let result = self.surface.run_interactive(request)?;
        self.policy.allow_url(&result.url)?;
        self.policy.check_payload(&result.payload)?;
        self.sessions.insert(result.session.clone());
        Ok(result)
    }
}

/// A canned response used by [`MockBrowserHost`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MockBrowserReply {
    pub url: String,
    #[serde(default)]
    pub payload: Option<Value>,
}

/// Offline browser host for conformance tests and reference-host development.
/// It never runs a browser or accesses storage; it issues opaque session
/// tokens and accepts them only on the same host instance.
#[derive(Debug)]
pub struct MockBrowserHost {
    policy: BrowserPolicy,
    replies: VecDeque<MockBrowserReply>,
    sessions: BTreeSet<String>,
    next_session: u64,
    scope_nonce: u64,
    session_responses: BTreeMap<String, HttpResponse>,
}

impl MockBrowserHost {
    pub fn new(policy: BrowserPolicy, replies: impl IntoIterator<Item = MockBrowserReply>) -> Self {
        Self {
            policy,
            replies: replies.into_iter().collect(),
            sessions: BTreeSet::new(),
            next_session: 0,
            scope_nonce: NEXT_SCOPE.fetch_add(1, Ordering::Relaxed),
            session_responses: BTreeMap::new(),
        }
    }

    pub fn grant_consent(&mut self) {
        self.policy.consent_granted = true;
    }

    /// Configures the response returned when an authenticated opaque session is
    /// handed back to `ctx.http.request`.
    pub fn set_session_response(&mut self, url: impl Into<String>, response: HttpResponse) {
        self.session_responses.insert(url.into(), response);
    }
}

impl WefHost for MockBrowserHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        let session = request.browser_session.ok_or(HostError::Unsupported)?;
        if !self.sessions.contains(&session) {
            return Err(HostError::Unsupported);
        }
        self.policy.allow_url(&request.url)?;
        self.session_responses
            .get(&request.url)
            .cloned()
            .ok_or_else(|| {
                HostError::Message("no mock HTTP response for browser session request".into())
            })
    }

    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        self.policy.validate(&request)?;
        let reply = self
            .replies
            .pop_front()
            .ok_or_else(|| HostError::Message("no mock browser reply configured".into()))?;
        self.policy.allow_url(&reply.url)?;
        self.policy.check_payload(&reply.payload)?;
        let session = format!("browser-session-{}-{}", self.scope_nonce, self.next_session);
        self.next_session += 1;
        self.sessions.insert(session.clone());
        Ok(BrowserRunResult {
            url: reply.url,
            payload: reply.payload,
            session,
        })
    }
}
