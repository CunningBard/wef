//! Desktop plain-HTTP backend: one blocking client, cookies, rate limits.
//!
//! This is the `WefHost::request` implementation behind `ctx.http.request`
//! on desktop. Portable behavior (what every backend repeats): HTTP(S) only,
//! manifest-origin allowlist on the initial URL and every redirect hop,
//! query encoding per [`crate::host::append_query`], header names
//! lowercased on the way out, non-2xx statuses returned (never thrown),
//! image candidates retried only after 404/410/transport errors. Desktop
//! quirks (see `super::desktop`): manual redirect following (so each hop
//! is origin-checked), in-memory cookie jar with JSON import/export, 30 s
//! default timeout, 5 MB body cap, and a sliding-window rate limiter fed
//! by `set_rate_limit`.

use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io::{BufRead, Write},
    rc::Rc,
    time::{Duration, Instant},
};

use ureq::AsSendBody;
use url::Url;
use wef_core::{ImageRequest, RateLimit};

use crate::host::{
    BinaryHttpResponse, HostError, HttpRequest, HttpResponse, WefHost, check_allowed_url,
};

/// A production HTTP host backed by a blocking [`ureq`] agent.
///
/// Redirects are followed manually (never by the agent) so every hop is
/// origin-checked: session material and cookies can never ride along
/// off-origin. Keep one host attached to an engine for the duration of a
/// source session if the source relies on cookies.
#[derive(Clone)]
pub struct UreqHost {
    agent: ureq::Agent,
    max_response_body_bytes: u64,
    rate_window: Rc<RefCell<Option<RateWindow>>>,
    allowed_urls: Vec<String>,
}

/// Redirect hops followed for one request before failing.
const MAX_REDIRECT_HOPS: u32 = 10;

/// One fully-fetched response: final status, headers, and body bytes after
/// following redirects. Ports: same struct — method rewrite and hop cap
/// below are contract, not desktop quirks.
struct FetchedResponse {
    status: u16,
    url: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// Returns the next hop for a redirect status with a joinable `Location`,
/// or `None` when this response is final. Ports: same status set.
fn redirect_target(status: u16, headers: &BTreeMap<String, String>, current: &Url) -> Option<Url> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = headers.get("location")?;
    current.join(location).ok()
}

#[derive(Clone)]
struct RateWindow {
    policy: RateLimit,
    requests: VecDeque<Instant>,
}

impl Default for UreqHost {
    fn default() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }
}

impl UreqHost {
    /// Creates a host with the default 30-second total request timeout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a host with a custom total request timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .max_redirects(0)
            .build();
        Self {
            agent: config.new_agent(),
            max_response_body_bytes: 5 * 1024 * 1024,
            rate_window: Rc::new(RefCell::new(None)),
            allowed_urls: Vec::new(),
        }
    }

    /// Sets the maximum decoded text response body accepted by this host.
    pub fn with_max_response_body_bytes(mut self, bytes: u64) -> Self {
        self.max_response_body_bytes = bytes;
        self
    }

    /// Loads persistent cookies into this host's shared cookie jar.
    pub fn load_cookie_jar_json<R: BufRead>(&self, reader: R) -> Result<(), HostError> {
        self.agent
            .cookie_jar_lock()
            .load_json(reader)
            .map_err(|error| HostError::Message(format!("could not load cookie jar: {error}")))
    }

    /// Saves unexpired persistent cookies from this host's shared cookie jar.
    pub fn save_cookie_jar_json<W: Write>(&self, writer: &mut W) -> Result<(), HostError> {
        self.agent
            .cookie_jar_lock()
            .save_json(writer)
            .map_err(|error| HostError::Message(format!("could not save cookie jar: {error}")))
    }

    /// Sends a built request without treating HTTP statuses as errors.
    /// Callers read the body as text or bytes and map the shared parts.
    /// This is one hop: redirect following lives in [`Self::fetch`], which
    /// origin-checks every hop so session material never rides off-origin.
    fn send_built<B>(
        &self,
        request: ureq::http::Request<B>,
    ) -> Result<ureq::http::Response<ureq::Body>, HostError>
    where
        B: AsSendBody,
    {
        let request = self
            .agent
            .configure_request(request)
            .http_status_as_error(false)
            .build();
        self.agent
            .run(request)
            .map_err(|error| HostError::Message(error.to_string()))
    }

    /// Fetches one request, following redirects manually. Every hop —
    /// initial URL included — MUST be an allowed origin; a hop leaving the
    /// allowlist fails the whole request instead of leaking headers or
    /// cookies. Ports: same loop — check, send, follow `Location` (301,
    /// 302, 303 switch POST to GET; 307/308 preserve), cap hops.
    fn fetch(
        &self,
        method: &str,
        url: Url,
        headers: BTreeMap<String, String>,
        body: Option<String>,
    ) -> Result<FetchedResponse, HostError> {
        let mut current_url = url;
        let mut current_method = method.to_owned();
        let mut current_headers = headers;
        let mut current_body = body;
        let mut hops = 0u32;
        loop {
            check_allowed_url(&self.allowed_urls, current_url.as_str())?;
            let mut builder = ureq::http::Request::builder()
                .method(current_method.as_str())
                .uri(current_url.as_str());
            for (name, value) in &current_headers {
                builder = builder.header(name, value);
            }
            let mut response = match current_body.clone() {
                Some(text) => self.send_built(builder.body(text).map_err(|error| {
                    HostError::Message(format!("invalid HTTP request: {error}"))
                })?)?,
                None => {
                    self.send_built(builder.body(ureq::SendBody::none()).map_err(|error| {
                        HostError::Message(format!("invalid HTTP request: {error}"))
                    })?)?
                }
            };
            let status = response.status().as_u16();
            let response_headers = response_headers(&response)?;
            let body = response
                .body_mut()
                .with_config()
                .limit(self.max_response_body_bytes)
                .read_to_vec()
                .map_err(|error| HostError::Message(error.to_string()))?;
            let Some(next_url) = redirect_target(status, &response_headers, &current_url) else {
                return Ok(FetchedResponse {
                    status,
                    url: current_url.to_string(),
                    headers: response_headers,
                    body,
                });
            };
            hops += 1;
            if hops > MAX_REDIRECT_HOPS {
                return Err(HostError::Message("too many redirects".into()));
            }
            if next_url.origin() != current_url.origin() {
                // The hop stays inside the allowlist (checked next
                // iteration), but credentials MUST NOT cross origins even
                // between two listed ones.
                current_headers.retain(|name, _| {
                    !matches!(
                        name.to_ascii_lowercase().as_str(),
                        "authorization" | "proxy-authorization" | "cookie"
                    )
                });
            }
            if status == 303 && current_method != "HEAD"
                || (status == 301 || status == 302) && current_method == "POST"
            {
                current_method = "GET".to_owned();
                current_body = None;
            }
            current_url = next_url;
        }
    }

    fn run_request(&self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        let mut url = Url::parse(&request.url)
            .map_err(|error| HostError::Message(format!("invalid HTTP URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HostError::Message(format!(
                "unsupported HTTP URL scheme {:?}",
                url.scheme()
            )));
        }
        crate::host::append_query(&mut url, request.query.as_ref())?;
        let fetched = self.fetch(
            request.method.as_deref().unwrap_or("GET"),
            url,
            request.headers.unwrap_or_default(),
            request.body,
        )?;
        Ok(HttpResponse {
            status: fetched.status,
            url: fetched.url,
            headers: fetched.headers,
            body: String::from_utf8(fetched.body)
                .map_err(|error| HostError::Message(error.to_string()))?,
        })
    }

    fn fetch_binary(
        &self,
        url: Url,
        headers: BTreeMap<String, String>,
    ) -> Result<BinaryHttpResponse, HostError> {
        let fetched = self.fetch("GET", url, headers, None)?;
        Ok(BinaryHttpResponse {
            status: fetched.status,
            url: fetched.url,
            headers: fetched.headers,
            body: fetched.body,
        })
    }

    fn image_request(
        &mut self,
        url: String,
        headers: Option<BTreeMap<String, String>>,
    ) -> Result<BinaryHttpResponse, HostError> {
        self.enforce_rate_limit()?;
        let url = Url::parse(&url)
            .map_err(|error| HostError::Message(format!("invalid HTTP URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HostError::Message(format!(
                "unsupported HTTP URL scheme {:?}",
                url.scheme()
            )));
        }
        self.fetch_binary(url, headers.unwrap_or_default())
    }

    /// Fetches an image request, trying ordered candidates only after a 404,
    /// 410, or transport error.
    pub fn fetch_image(&mut self, request: &ImageRequest) -> Result<BinaryHttpResponse, HostError> {
        let mut attempts = Vec::with_capacity(1 + request.candidates.as_ref().map_or(0, Vec::len));
        attempts.push((request.url.clone(), request.headers.clone()));
        if let Some(candidates) = &request.candidates {
            attempts.extend(
                candidates
                    .iter()
                    .map(|candidate| (candidate.url.clone(), candidate.headers.clone())),
            );
        }
        let mut last_error = None;
        for (index, (url, headers)) in attempts.into_iter().enumerate() {
            match self.image_request(url, headers) {
                Ok(response)
                    if !matches!(response.status, 404 | 410)
                        || index + 1 == request.candidates.as_ref().map_or(0, Vec::len) + 1 =>
                {
                    return Ok(response);
                }
                Ok(_) => continue,
                Err(error @ HostError::Message(_))
                    if index + 1 < request.candidates.as_ref().map_or(0, Vec::len) + 1 =>
                {
                    last_error = Some(error)
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| HostError::Message("all image candidates failed".into())))
    }

    fn enforce_rate_limit(&self) -> Result<(), HostError> {
        let mut state = self.rate_window.borrow_mut();
        let Some(window) = state.as_mut() else {
            return Ok(());
        };
        let now = Instant::now();
        let duration = Duration::from_millis(window.policy.window_ms);
        while window
            .requests
            .front()
            .is_some_and(|request| now.duration_since(*request) >= duration)
        {
            window.requests.pop_front();
        }
        if window.requests.len() >= window.policy.max_requests as usize {
            return Err(HostError::RateLimited);
        }
        window.requests.push_back(now);
        Ok(())
    }
}

/// Lowercased response headers shared by text and binary requests.
/// Ports: header names are case-insensitive everywhere; normalizing once
/// keeps source-visible behavior identical across backends.
fn response_headers(
    response: &ureq::http::Response<ureq::Body>,
) -> Result<BTreeMap<String, String>, HostError> {
    response
        .headers()
        .iter()
        .map(|(name, value)| {
            let value = value
                .to_str()
                .map_err(|error| HostError::Message(error.to_string()))?;
            Ok((name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect()
}

impl WefHost for UreqHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        if request.browser_session.is_some() {
            return Err(HostError::Unsupported);
        }
        self.enforce_rate_limit()?;
        self.run_request(request)
    }

    fn set_rate_limit(&mut self, limit: Option<RateLimit>) {
        *self.rate_window.borrow_mut() = limit.map(|policy| RateWindow {
            policy,
            requests: VecDeque::new(),
        });
    }

    /// Installs the manifest `baseUrls` whitelist. Empty means deny-all:
    /// the engine pushes the manifest list before every run, so an empty
    /// set here is always a wiring bug, never "allow everything".
    /// Ports: same fail-closed default.
    fn set_allowed_urls(&mut self, urls: &[String]) {
        self.allowed_urls = urls.to_vec();
    }
}
