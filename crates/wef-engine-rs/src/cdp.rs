use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};
use tungstenite::{Message, connect};
use url::Url;

use crate::{
    BrowserPolicy, BrowserRunRequest, BrowserRunResult, HostError, HttpRequest, HttpResponse,
    UreqHost, WefHost,
};

/// Opt-in Chromium DevTools Protocol host. It connects only to an explicitly
/// supplied local debugging endpoint and never launches a browser process.
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

    fn rpc(
        socket: &mut tungstenite::WebSocket<
            tungstenite::stream::MaybeTlsStream<std::net::TcpStream>,
        >,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, HostError> {
        socket
            .send(Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .map_err(|error| HostError::Message(error.to_string()))?;
        loop {
            let message = socket
                .read()
                .map_err(|error| HostError::Message(error.to_string()))?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| HostError::Message(error.to_string()))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(HostError::Message(format!("CDP {method}: {error}")));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

#[derive(Deserialize)]
struct Target {
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_url: String,
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
        let headers = request.headers.get_or_insert_with(BTreeMap::new);
        headers.insert("Cookie".into(), cookie);
        self.http.request(request)
    }

    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        self.policy.validate(&request)?;
        let endpoint = self.endpoint(&format!(
            "json/new?{}",
            url::form_urlencoded::byte_serialize(request.url.as_bytes()).collect::<String>()
        ))?;
        let target: Target = ureq::put(endpoint.as_str())
            .send_empty()
            .map_err(|error| HostError::Message(error.to_string()))?
            .body_mut()
            .read_json()
            .map_err(|error| HostError::Message(error.to_string()))?;
        let (mut socket, _) = connect(target.websocket_url.as_str())
            .map_err(|error| HostError::Message(error.to_string()))?;
        let _ = Self::rpc(&mut socket, 1, "Page.enable", json!({}))?;
        if let Some(script) = request.initialization_script {
            let _ = Self::rpc(
                &mut socket,
                2,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source":script}),
            )?;
        }
        let _ = Self::rpc(&mut socket, 3, "Page.navigate", json!({"url":request.url}))?;
        let result = Self::rpc(
            &mut socket,
            4,
            "Runtime.evaluate",
            json!({"expression":format!("(async () => {{ {} }})()", request.script),"returnByValue":true,"awaitPromise":true}),
        )?;
        let payload = result.pointer("/result/value").cloned();
        let cookies = Self::rpc(&mut socket, 5, "Network.getCookies", json!({}))?;
        let cookie = cookies
            .get("cookies")
            .and_then(Value::as_array)
            .map(|cookies| {
                cookies
                    .iter()
                    .filter_map(|cookie| {
                        Some(format!(
                            "{}={}",
                            cookie.get("name")?.as_str()?,
                            cookie.get("value")?.as_str()?
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        let session = format!("cdp-session-{}", self.next_session);
        self.next_session += 1;
        self.sessions.insert(session.clone(), cookie);
        let current = Self::rpc(
            &mut socket,
            6,
            "Runtime.evaluate",
            json!({"expression":"location.href","returnByValue":true}),
        )?;
        let url = current
            .pointer("/result/value")
            .and_then(Value::as_str)
            .unwrap_or(&request.url)
            .to_owned();
        self.policy.allow_url(&url)?;
        Ok(BrowserRunResult {
            url,
            payload,
            session,
        })
    }
}
