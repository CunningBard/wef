use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::{HostError, HttpRequest, HttpResponse, WefHost};

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunRequest {
    pub url: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub initialization_script: Option<String>,
    pub script: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
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
    pub source_id: String,
    pub profile_id: String,
}

impl BrowserPolicy {
    pub fn for_origins(origins: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed_origins: origins.into_iter().collect(),
            consent_granted: false,
            max_timeout_ms: 30_000,
            source_id: "default".into(),
            profile_id: "default".into(),
        }
    }

    pub fn scoped(mut self, source_id: impl Into<String>, profile_id: impl Into<String>) -> Self {
        self.source_id = source_id.into();
        self.profile_id = profile_id.into();
        self
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
        self.sessions.insert(result.session.clone());
        Ok(result)
    }
}

/// A scripted response used by [`MockBrowserHost`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MockBrowserReply {
    pub url: String,
    #[serde(default)]
    pub payload: Option<Value>,
}

/// Offline browser host for conformance tests and reference-host development.
/// It never evaluates browser scripts or accesses storage; it issues opaque
/// session tokens and accepts them only on the same host instance.
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

    fn allow_url(&self, value: &str) -> Result<(), HostError> {
        let url = Url::parse(value)
            .map_err(|error| HostError::Message(format!("invalid browser URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(HostError::Message("browser URL must use HTTP(S)".into()));
        }
        let origin = url.origin().ascii_serialization();
        if !self.policy.allowed_origins.contains(&origin) {
            return Err(HostError::Message(format!(
                "browser origin is not allowed: {origin}"
            )));
        }
        Ok(())
    }
}

impl WefHost for MockBrowserHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        let session = request.browser_session.ok_or(HostError::Unsupported)?;
        if !self.sessions.contains(&session) {
            return Err(HostError::Unsupported);
        }
        self.allow_url(&request.url)?;
        self.session_responses
            .get(&request.url)
            .cloned()
            .ok_or_else(|| {
                HostError::Message("no mock HTTP response for browser session request".into())
            })
    }

    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        if !self.policy.consent_granted {
            return Err(HostError::Message(
                "browser consent has not been granted".into(),
            ));
        }
        self.allow_url(&request.url)?;
        if request.timeout_ms.unwrap_or(self.policy.max_timeout_ms) > self.policy.max_timeout_ms {
            return Err(HostError::Message(
                "browser timeout exceeds host policy".into(),
            ));
        }
        let reply = self
            .replies
            .pop_front()
            .ok_or_else(|| HostError::Message("no mock browser reply configured".into()))?;
        self.allow_url(&reply.url)?;
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
