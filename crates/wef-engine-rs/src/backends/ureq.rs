//! Desktop plain-HTTP backend: one blocking client, cookies, rate limits.
//!
//! This is the `WefHost::request` implementation behind `ctx.http.request`
//! on desktop. Portable behavior (what every backend repeats): HTTP(S) only,
//! query encoding per [`crate::host::append_query`], header names
//! lowercased on the way out, non-2xx statuses returned (never thrown),
//! image candidates retried only after 404/410/transport errors. Desktop
//! quirks (see `super::desktop`): redirect following, in-memory cookie jar
//! with JSON import/export, 30 s default timeout, 5 MB body cap, and a
//! sliding-window rate limiter fed by `set_rate_limit`.

use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io::{BufRead, Write},
    rc::Rc,
    time::{Duration, Instant},
};

use ureq::{AsSendBody, ResponseExt};
use url::Url;
use wef_core::{ImageRequest, RateLimit};

use crate::host::{BinaryHttpResponse, HostError, HttpRequest, HttpResponse, WefHost};

/// A production HTTP host backed by a blocking [`ureq`] agent.
///
/// The agent follows redirects and keeps cookies for the lifetime of this host.
/// Keep one host attached to an engine for the duration of a source session if
/// the source relies on either behavior.
#[derive(Clone)]
pub struct UreqHost {
    agent: ureq::Agent,
    max_response_body_bytes: u64,
    rate_window: Rc<RefCell<Option<RateWindow>>>,
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
            .build();
        Self {
            agent: config.new_agent(),
            max_response_body_bytes: 5 * 1024 * 1024,
            rate_window: Rc::new(RefCell::new(None)),
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

    fn run_request<B>(&self, request: ureq::http::Request<B>) -> Result<HttpResponse, HostError>
    where
        B: AsSendBody,
    {
        let request = self
            .agent
            .configure_request(request)
            .http_status_as_error(false)
            .build();
        let mut response = self
            .agent
            .run(request)
            .map_err(|error| HostError::Message(error.to_string()))?;

        let url = response.get_uri().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let value = value
                    .to_str()
                    .map_err(|error| HostError::Message(error.to_string()))?;
                Ok((name.as_str().to_ascii_lowercase(), value.to_owned()))
            })
            .collect::<Result<BTreeMap<_, _>, HostError>>()?;
        let body = response
            .body_mut()
            .with_config()
            .limit(self.max_response_body_bytes)
            .read_to_string()
            .map_err(|error| HostError::Message(error.to_string()))?;

        Ok(HttpResponse {
            status: response.status().as_u16(),
            url,
            headers,
            body,
        })
    }

    fn run_binary_request<B>(
        &self,
        request: ureq::http::Request<B>,
    ) -> Result<BinaryHttpResponse, HostError>
    where
        B: AsSendBody,
    {
        let request = self
            .agent
            .configure_request(request)
            .http_status_as_error(false)
            .build();
        let mut response = self
            .agent
            .run(request)
            .map_err(|error| HostError::Message(error.to_string()))?;
        let url = response.get_uri().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                Ok((
                    name.as_str().to_ascii_lowercase(),
                    value
                        .to_str()
                        .map_err(|error| HostError::Message(error.to_string()))?
                        .to_owned(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, HostError>>()?;
        let body = response
            .body_mut()
            .with_config()
            .limit(self.max_response_body_bytes)
            .read_to_vec()
            .map_err(|error| HostError::Message(error.to_string()))?;
        Ok(BinaryHttpResponse {
            status: response.status().as_u16(),
            url,
            headers,
            body,
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
        let mut builder = ureq::http::Request::builder()
            .method("GET")
            .uri(url.as_str());
        if let Some(headers) = headers {
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
        }
        self.run_binary_request(
            builder
                .body(ureq::SendBody::none())
                .map_err(|error| HostError::Message(format!("invalid HTTP request: {error}")))?,
        )
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

impl WefHost for UreqHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        if request.browser_session.is_some() {
            return Err(HostError::Unsupported);
        }
        self.enforce_rate_limit()?;
        let mut url = Url::parse(&request.url)
            .map_err(|error| HostError::Message(format!("invalid HTTP URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HostError::Message(format!(
                "unsupported HTTP URL scheme {:?}",
                url.scheme()
            )));
        }
        crate::host::append_query(&mut url, request.query.as_ref())?;

        let method = request
            .method
            .as_deref()
            .unwrap_or("GET")
            .parse::<ureq::http::Method>()
            .map_err(|error| HostError::Message(format!("invalid HTTP method: {error}")))?;
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(url.as_str());
        if let Some(headers) = request.headers {
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
        }

        match request.body {
            Some(body) => {
                self.run_request(builder.body(body).map_err(|error| {
                    HostError::Message(format!("invalid HTTP request: {error}"))
                })?)
            }
            None => {
                self.run_request(builder.body(ureq::SendBody::none()).map_err(|error| {
                    HostError::Message(format!("invalid HTTP request: {error}"))
                })?)
            }
        }
    }

    fn set_rate_limit(&mut self, limit: Option<RateLimit>) {
        *self.rate_window.borrow_mut() = limit.map(|policy| RateWindow {
            policy,
            requests: VecDeque::new(),
        });
    }
}
