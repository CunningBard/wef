//! Portable host contract: the only types a port must reimplement.
//!
//! A `WefHost` serves three capabilities — plain HTTP (`request`), browser
//! observation (`run_browser`, defaulting to unavailable), rate limits, and
//! the manifest origin allowlist (`set_allowed_urls`, defaulting to
//! ignore). Everything HTTP-shaped but client-specific (redirect policy,
//! cookie storage, timeouts) lives in a backend (`backends::ureq` on
//! desktop, platform cookie jars on mobile). The query-encoding rule below
//! (`append_query`) is contract: every backend encodes `query` maps the
//! same way so signed URLs match across ports.

use std::collections::BTreeMap;

use crate::browser::{BrowserRunRequest, BrowserRunResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use url::Url;
use wef_core::RateLimit;

/// A shared, mutable reference to the host capability object.
///
/// Ports: this is an ordinary object reference. Kotlin holds the host in a
/// plain property (adding their usual synchronization if JS runs off-thread),
/// Swift holds it in a property (or actor). Rust needs the `Rc<RefCell<…>>`
/// wrapper because the JS context (`runtime.rs`) keeps a second cloneable
/// handle to the same host while `Engine` drives it mutably — there is no
/// `Rc`/`RefCell` concept to translate, only the four methods below.
#[derive(Clone)]
pub(crate) struct HostHandle(std::rc::Rc<std::cell::RefCell<Box<dyn WefHost>>>);

impl HostHandle {
    pub(crate) fn new(host: Box<dyn WefHost>) -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(host)))
    }

    pub(crate) fn request(&self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        self.0.borrow_mut().as_mut().request(request)
    }

    pub(crate) fn set_rate_limit(&self, limit: Option<RateLimit>) {
        self.0.borrow_mut().as_mut().set_rate_limit(limit);
    }

    pub(crate) fn set_allowed_urls(&self, urls: &[String]) {
        self.0.borrow_mut().as_mut().set_allowed_urls(urls);
    }

    pub(crate) fn run_browser(
        &self,
        request: BrowserRunRequest,
    ) -> Result<BrowserRunResult, HostError> {
        self.0.borrow_mut().as_mut().run_browser(request)
    }
}

/// Per-source persistent storage (WEF 0.0.4 `ctx.store`): one source's own
/// key/value map. Same scoping as cookies/sessions; hosts persist at will,
/// memory minimum.
/// Ports: a small class around `MutableMap<String, JsonElement>` with the
/// limit checks below. The numbers come from `wef_core::store_limits` and
/// are contract, not tuning.
#[derive(Clone, Debug)]
pub(crate) struct StoreMap {
    entries: BTreeMap<String, Value>,
}

/// Why a store write fails. Ports: same three rejections with the same
/// messages (see `StoreError::message`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreError {
    InvalidKey,
    ValueTooLarge,
    TooManyKeys,
}

impl StoreError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::InvalidKey => format!(
                "store key must be 1..={} chars",
                wef_core::store_limits::KEY_MAX_LEN
            ),
            Self::ValueTooLarge => format!(
                "store value exceeds {} bytes",
                wef_core::store_limits::VALUE_MAX_BYTES
            ),
            Self::TooManyKeys => format!(
                "store holds at most {} keys",
                wef_core::store_limits::MAX_KEYS
            ),
        }
    }
}

impl StoreMap {
    pub(crate) fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Reads one key; a missing key is `None` (surfaced to JS as `null`).
    /// Ports: plain map lookup after the key-shape check.
    pub(crate) fn get(&self, key: &str) -> Result<Option<Value>, StoreError> {
        if key.is_empty() || key.len() > wef_core::store_limits::KEY_MAX_LEN {
            return Err(StoreError::InvalidKey);
        }
        Ok(self.entries.get(key).cloned())
    }

    /// Writes one key. A JSON null value deletes the key; anything else is
    /// stored after the contract limit checks. Violations fail the call,
    /// never truncate. Ports: same null-deletes rule plus the three checks.
    pub(crate) fn set(&mut self, key: &str, value: &Value) -> Result<(), StoreError> {
        if key.is_empty() || key.len() > wef_core::store_limits::KEY_MAX_LEN {
            return Err(StoreError::InvalidKey);
        }
        if value.is_null() {
            self.entries.remove(key);
            return Ok(());
        }
        let size = serde_json::to_string(value)
            .map(|text| text.len())
            .unwrap_or(usize::MAX);
        if size > wef_core::store_limits::VALUE_MAX_BYTES {
            return Err(StoreError::ValueTooLarge);
        }
        if !self.entries.contains_key(key) && self.entries.len() >= wef_core::store_limits::MAX_KEYS
        {
            return Err(StoreError::TooManyKeys);
        }
        self.entries.insert(key.to_owned(), value.clone());
        Ok(())
    }

    /// Copies every entry as plain JSON.
    pub(crate) fn snapshot(&self) -> Map<String, Value> {
        let mut out = Map::new();
        for (key, value) in self.entries.iter() {
            out.insert(key.clone(), value.clone());
        }
        out
    }

    /// Restores entries from a snapshot; malformed entries are skipped, never
    /// fatal. Ports: same skip rule when loading the persisted map.
    pub(crate) fn restore(&mut self, values: &Map<String, Value>) {
        for (key, value) in values {
            if key.is_empty() {
                continue;
            }
            if key.len() > wef_core::store_limits::KEY_MAX_LEN {
                continue;
            }
            if self.entries.len() >= wef_core::store_limits::MAX_KEYS {
                continue;
            }
            self.entries.insert(key.clone(), value.clone());
        }
    }
}

/// Cloneable handle to one source's `StoreMap`; every clone sees the same
/// entries. Ports: an ordinary shared reference to the same map object.
#[derive(Clone)]
pub(crate) struct SharedStore(std::rc::Rc<std::cell::RefCell<StoreMap>>);

impl SharedStore {
    pub(crate) fn new() -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(StoreMap::new())))
    }

    pub(crate) fn get(&self, key: &str) -> Result<Option<Value>, StoreError> {
        self.0.borrow().get(key)
    }

    pub(crate) fn set(&self, key: &str, value: &Value) -> Result<(), StoreError> {
        self.0.borrow_mut().set(key, value)
    }

    pub(crate) fn snapshot(&self) -> Map<String, Value> {
        self.0.borrow().snapshot()
    }

    pub(crate) fn restore(&self, values: &Map<String, Value>) {
        self.0.borrow_mut().restore(values);
    }
}

/// All source stores, keyed by source id.
/// Ports: `stores: MutableMap<String, StoreMap>` with get-or-create.
#[derive(Clone)]
pub(crate) struct StoreRegistry(std::rc::Rc<std::cell::RefCell<BTreeMap<String, SharedStore>>>);

impl StoreRegistry {
    pub(crate) fn new() -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(BTreeMap::new())))
    }

    /// Returns the persistent store for one source, creating it on first use.
    /// Ports: `stores.getOrPut(sourceId) { StoreMap() }`.
    pub(crate) fn store_for(&self, source_id: &str) -> SharedStore {
        let mut stores = self.0.borrow_mut();
        if let Some(handle) = stores.get(source_id) {
            return handle.clone();
        }
        let handle = SharedStore::new();
        stores.insert(source_id.to_owned(), handle.clone());
        handle
    }

    /// Snapshots every source store as plain JSON (for CLI `--store` files).
    /// Ports: serialize the same map the host persists across restarts.
    pub(crate) fn snapshot(&self) -> Value {
        let stores = self.0.borrow();
        let mut root = Map::new();
        for (id, handle) in stores.iter() {
            root.insert(id.clone(), Value::Object(handle.snapshot()));
        }
        Value::Object(root)
    }

    /// Restores a snapshot produced by `snapshot`; malformed entries are
    /// skipped, never fatal. Ports: load the persisted map.
    pub(crate) fn restore(&self, snapshot: &Value) {
        let root = match snapshot.as_object() {
            Some(root) => root,
            None => return,
        };
        for (id, values) in root {
            match values.as_object() {
                Some(values) => self.store_for(id).restore(values),
                None => continue,
            }
        }
    }

    /// Copies one source's store for diagnostics-safe handling.
    pub(crate) fn snapshot_for(&self, source_id: &str) -> Map<String, Value> {
        let stores = self.0.borrow();
        match stores.get(source_id) {
            Some(handle) => handle.snapshot(),
            None => Map::new(),
        }
    }
}

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

    /// Hands the manifest's `baseUrls` to the host before each run. Hosts
    /// that perform HTTP MUST only contact URLs under these entries (every
    /// redirect hop included) and fail closed when the set is empty.
    /// Ports: same hook — push the manifest entries before dispatch.
    fn set_allowed_urls(&mut self, _urls: &[String]) {}

    fn run_browser(&mut self, _request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        Err(HostError::Unsupported)
    }
}

/// Checks one request URL against the manifest `baseUrls` whitelist: parses,
/// requires HTTP(S), and requires the URL to fall under a listed entry —
/// same scheme, host, and port, plus a path prefix on a segment boundary
/// (a bare origin entry allows every path on that origin). Every redirect
/// hop is checked the same way, so session material can never ride along
/// off-allowlist.
/// Ports: same rule — compare scheme/host/port, then path prefix with the
/// boundary check, never raw string prefix (which `evil.com` defeats).
pub fn check_allowed_url(allowed: &[String], url: &str) -> Result<Url, HostError> {
    let parsed = match Url::parse(url) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Err(HostError::Message(format!("invalid HTTP URL: {error}")));
        }
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(HostError::Message(format!(
            "unsupported HTTP URL scheme {:?}",
            parsed.scheme()
        )));
    }
    for entry in allowed {
        let base = match Url::parse(entry) {
            Ok(base) => base,
            Err(_) => continue,
        };
        if base.scheme() != "http" && base.scheme() != "https" {
            continue;
        }
        if base.scheme() != parsed.scheme() {
            continue;
        }
        if base.port_or_known_default() != parsed.port_or_known_default() {
            continue;
        }
        let base_host = match base.host_str() {
            Some(host) => host,
            None => continue,
        };
        let request_host = match parsed.host_str() {
            Some(host) => host,
            None => continue,
        };
        if !allowlist_host_matches(base_host, request_host) {
            continue;
        }
        let prefix = base.path().trim_end_matches('/');
        if prefix.is_empty()
            || parsed.path() == prefix
            || parsed.path().starts_with(&format!("{prefix}/"))
        {
            return Ok(parsed);
        }
    }
    Err(HostError::Message(format!(
        "URL is not allowed by the baseUrls whitelist: {url}"
    )))
}

/// Matches one allowlist host against one request host. A `*.example.org`
/// entry covers one dynamic label (`a.example.org`, not `a.b.example.org`
/// nor `example.org` itself): image CDNs with server-assigned hosts stay
/// listable. Ports: same single-label rule.
fn allowlist_host_matches(base_host: &str, request_host: &str) -> bool {
    let suffix = match base_host.strip_prefix("*.") {
        Some(suffix) => suffix,
        None => return base_host == request_host,
    };
    let prefix = match request_host.strip_suffix(suffix) {
        Some(prefix) => prefix,
        None => return false,
    };
    let label = match prefix.strip_suffix('.') {
        Some(label) => label,
        None => return false,
    };
    !label.is_empty() && !label.contains('.')
}

/// Encodes a `query` map onto a URL: string values append as-is, arrays
/// repeat the key, anything else fails. Ports: `URLComponents.queryItems`
/// (Swift) / `HttpUrl.Builder.addQueryParameter` in a loop (Kotlin) with
/// the same string-or-string-array rule and the same rejection.
pub fn append_query(url: &mut Url, query: Option<&Map<String, Value>>) -> Result<(), HostError> {
    let query = match query {
        Some(query) => query,
        None => return Ok(()),
    };

    let mut parameters: Vec<(&str, &str)> = Vec::new();
    for (key, value) in query {
        match value {
            Value::String(text) => parameters.push((key.as_str(), text.as_str())),
            Value::Array(values) => {
                for value in values {
                    match value.as_str() {
                        Some(text) => parameters.push((key.as_str(), text)),
                        None => {
                            return Err(HostError::Message(format!(
                                "HTTP query parameter {key:?} array values must be strings"
                            )));
                        }
                    }
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
