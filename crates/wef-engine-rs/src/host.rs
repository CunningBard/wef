//! Portable host contract: the only types a port must reimplement.
//!
//! A `WefHost` serves three capabilities — plain HTTP (`request`), browser
//! observation (`run_browser`, defaulting to unavailable), and rate limits.
//! Everything HTTP-shaped but client-specific (redirect policy, cookie
//! storage, timeouts) lives in a backend (`backends::ureq` on desktop,
//! platform cookie jars on mobile). The query-encoding rule below
//! (`append_query`) is contract: every backend encodes `query` maps the
//! same way so signed URLs match across ports.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use crate::browser::{BrowserRunRequest, BrowserRunResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use url::Url;
use wef_core::RateLimit;

/// Ports: `HostHandle` is just "a mutable reference to the host capability
/// object". Rust needs `Rc<RefCell<…>>` because the JS context (`runtime.rs`)
/// holds a second cloneable handle to the same host while `Engine` drives it
/// mutably. Kotlin/Swift ports hold the host in an ordinary mutable
/// property (plus their usual synchronization if JS runs off-thread) —
/// there is no `Rc`/`RefCell` concept to translate.
pub(crate) type HostHandle = Rc<RefCell<dyn WefHost>>;

/// Ports: per-source persistent storage (WEF 0.0.4 `ctx.store`). The outer map
/// is keyed by source id, the inner map holds that source's own keys.
/// Same scoping as cookies/sessions; hosts persist at will, memory minimum.
/// Kotlin: `MutableMap<String, MutableMap<String, JsonElement>>`.
pub(crate) type StoreHandle = Rc<RefCell<BTreeMap<String, Value>>>;
pub(crate) type StoreRegistry = Rc<RefCell<BTreeMap<String, StoreHandle>>>;

/// A text HTTP request exposed to a source through `ctx.http.request`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub method: Option<String>,
    pub url: String,
    pub headers: Option<BTreeMap<String, String>>,
    pub query: Option<Map<String, Value>>,
    pub body: Option<String>,
    #[serde(default)]
    pub browser_session: Option<String>,
}

/// A text HTTP response returned to a source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub status: u16,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryHttpResponse {
    pub status: u16,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

/// Host-side failure while servicing a WEF capability.
#[derive(Debug, Error)]
pub enum HostError {
    #[error("host capability is unavailable")]
    Unsupported,
    #[error("a browser-assisted challenge is required for {url}: {message}")]
    ChallengeRequired { url: String, message: String },
    #[error("source request rate limit exceeded")]
    RateLimited,
    #[error("{0}")]
    Message(String),
}

impl HostError {
    /// Stable machine code for conformance tests. Display prose may change;
    /// these strings are contract — every port reports the same codes.
    /// Ports: a switch/if-chain on the variant, returning the literal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported => "UNSUPPORTED",
            Self::ChallengeRequired { .. } => "CHALLENGE_REQUIRED",
            Self::RateLimited => "RATE_LIMITED",
            Self::Message(_) => "HOST_MESSAGE",
        }
    }
}

/// Host functionality used by the engine.
pub trait WefHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError>;

    fn set_rate_limit(&mut self, _limit: Option<RateLimit>) {}

    fn run_browser(&mut self, _request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        Err(HostError::Unsupported)
    }
}

/// Forwarding impl so `Engine::new` can store any host behind one handle.
/// Ports: unnecessary — passing an interface value already erases the type.
impl WefHost for Box<dyn WefHost> {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        self.as_mut().request(request)
    }

    fn set_rate_limit(&mut self, limit: Option<RateLimit>) {
        self.as_mut().set_rate_limit(limit);
    }

    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        self.as_mut().run_browser(request)
    }
}

/// Encodes a `query` map onto a URL: string values append as-is, arrays
/// repeat the key, anything else fails. Ports: `URLComponents.queryItems`
/// (Swift) / `HttpUrl.Builder.addQueryParameter` in a loop (Kotlin) with
/// the same string-or-string-array rule and the same rejection.
pub fn append_query(url: &mut Url, query: Option<&Map<String, Value>>) -> Result<(), HostError> {
    let Some(query) = query else {
        return Ok(());
    };

    let mut parameters = Vec::new();
    for (key, value) in query {
        match value {
            Value::String(value) => parameters.push((key.as_str(), value.as_str())),
            Value::Array(values) => {
                for value in values {
                    let value = value.as_str().ok_or_else(|| {
                        HostError::Message(format!(
                            "HTTP query parameter {key:?} array values must be strings"
                        ))
                    })?;
                    parameters.push((key.as_str(), value));
                }
            }
            _ => {
                return Err(HostError::Message(format!(
                    "HTTP query parameter {key:?} must be a string or string array"
                )));
            }
        }
    }

    let mut pairs = url.query_pairs_mut();
    for (key, value) in parameters {
        pairs.append_pair(key, value);
    }
    Ok(())
}
