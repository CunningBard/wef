use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use serde_json::{Map, Value};

use crate::wef_core::RateLimit;
use crate::wef_core::{ImageRequest, ImageRequestCandidate};
use crate::{
    BrowserPolicy, CdpBrowserHost, Engine, EngineError, ExtensionOperation, HostError, HttpRequest,
    HttpResponse, ImageTransformInput, InteractiveBrowserHost, InteractiveBrowserSurface,
    MockBrowserHost, MockBrowserReply, Operation, Package, UreqHost, WefHost,
};

static NEXT_PACKAGE: AtomicU64 = AtomicU64::new(0);

fn package(source: &str, requires: &[&str]) -> (Package, PathBuf) {
    package_with_version(source, requires, "0.0.1")
}

fn package_with_version(source: &str, requires: &[&str], wef_version: &str) -> (Package, PathBuf) {
    let id = NEXT_PACKAGE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("wef-engine-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let manifest = serde_json::json!({
        "wef": wef_version,
        "id": "org.example.test",
        "name": "Test source",
        "version": "0.1.0",
        "entry": "source.js",
        "languages": ["en"],
        "baseUrls": ["https://example.com"],
        "requires": requires,
        "listings": [{"id": "latest", "name": "Latest"}]
    });
    fs::write(
        root.join("wef.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(root.join("source.js"), source).unwrap();
    let package = Package::load(&root).unwrap();
    (package, root)
}

fn package_with_capabilities(source: &str, capabilities: Value) -> (Package, PathBuf) {
    let (package, root) = package(source, &[]);
    drop(package);
    let manifest_path = root.join("wef.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["capabilities"] = capabilities;
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    (Package::load(&root).unwrap(), root)
}

fn package_0_0_2(source: &str, capabilities: Value) -> (Package, PathBuf) {
    package_0_0_2_with_requires(source, &[], capabilities)
}

fn package_0_0_2_with_requires(
    source: &str,
    requires: &[&str],
    capabilities: Value,
) -> (Package, PathBuf) {
    let (_package, root) = package_with_version(source, requires, "0.0.2");
    let manifest_path = root.join("wef.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["capabilities"] = capabilities;
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    (Package::load(&root).unwrap(), root)
}

fn manga(key: &str, title: &str) -> Value {
    serde_json::json!({"key": key, "title": title})
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end;
    loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "server received an incomplete HTTP request");
        request.extend_from_slice(&buffer[..read]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }

    let header = String::from_utf8_lossy(&request[..header_end]);
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.eq_ignore_ascii_case("content-length"))
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "server received an incomplete HTTP body");
        request.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(request).unwrap()
}

fn spawn_http_server(responses: Vec<String>) -> (String, Receiver<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            requests.push(read_request(&mut stream));
            stream.write_all(response.as_bytes()).unwrap();
        }
        sender.send(requests).unwrap();
    });
    (format!("http://{address}"), receiver)
}

fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut response = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    response
}

#[test]
fn runs_an_async_listing_operation() {
    let (package, root) = package(
        r#"
            export async function getMangaList(ctx, input) {
                return {
                    items: [{ key: input.listingId, title: "Demo" }],
                    hasNextPage: input.page < 2
                };
            }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
        "#,
        &[],
    );

    let output = Engine::default()
        .run(
            &package,
            Operation::GetMangaList,
            serde_json::json!({"listingId": "latest", "page": 1}),
        )
        .unwrap();
    assert_eq!(output["items"][0], manga("latest", "Demo"));
    assert_eq!(output["hasNextPage"], true);
    fs::remove_dir_all(root).unwrap();
}

struct MockHost;

impl WefHost for MockHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        Ok(HttpResponse {
            status: 200,
            url: request.url,
            headers: BTreeMap::new(),
            body: "mock body".into(),
        })
    }
}

#[test]
fn exposes_the_http_bridge_to_declared_sources() {
    let (package, root) = package(
        r#"
            export async function search(ctx, input) {
                const response = await ctx.http.request({ url: "https://example.com/data" });
                return {
                    items: [{ key: response.status.toString(), title: response.body }],
                    hasNextPage: false
                };
            }
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
        "#,
        &["http"],
    );

    let output = Engine::with_host(MockHost)
        .run(
            &package,
            Operation::Search,
            serde_json::json!({"query": "demo", "page": 1, "filters": {}}),
        )
        .unwrap();
    assert_eq!(output["items"][0]["key"], "200");
    assert_eq!(output["items"][0]["title"], "mock body");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ureq_host_implements_wef_http_semantics() {
    let (base_url, requests) = spawn_http_server(vec![
        http_response(
            "302 Found",
            &[
                ("Location", "/final"),
                ("Set-Cookie", "session=abc; Path=/"),
            ],
            "",
        ),
        http_response(
            "404 Not Found",
            &[
                ("Content-Type", "text/plain; charset=utf-8"),
                ("X-Test", "yes"),
            ],
            "not found",
        ),
    ]);
    let mut host = UreqHost::with_timeout(Duration::from_secs(5));
    let mut query = Map::new();
    query.insert("ids".into(), serde_json::json!(["a", "b"]));
    query.insert("q".into(), Value::String("hello world".into()));
    let mut headers = BTreeMap::new();
    headers.insert("X-Client".into(), "test".into());

    let response = host
        .request(HttpRequest {
            method: Some("POST".into()),
            url: format!("{base_url}/start?existing=1"),
            headers: Some(headers),
            query: Some(query),
            body: Some("payload".into()),
            browser_session: None,
        })
        .unwrap();

    assert_eq!(response.status, 404);
    assert_eq!(response.url, format!("{base_url}/final"));
    assert_eq!(response.body, "not found");
    assert_eq!(
        response.headers["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.headers["x-test"], "yes");

    let requests = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].starts_with("POST /start?existing=1&ids=a&ids=b&q=hello+world HTTP/1.1\r\n")
    );
    assert!(requests[0].contains("x-client: test\r\n"));
    assert!(requests[0].ends_with("\r\n\r\npayload"));
    assert!(requests[1].starts_with("GET /final HTTP/1.1\r\n"));
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("cookie: session=abc"),
        "{}",
        requests[1]
    );
}

#[test]
fn ureq_host_rejects_non_string_query_values() {
    let mut host = UreqHost::default();
    let mut query = Map::new();
    query.insert("page".into(), serde_json::json!(1));
    let error = host
        .request(HttpRequest {
            method: None,
            url: "https://example.com".into(),
            headers: None,
            query: Some(query),
            body: None,
            browser_session: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("string or string array"));
}

#[test]
fn ureq_host_enforces_the_manifest_rate_limit() {
    let (base_url, requests) = spawn_http_server(vec![http_response("200 OK", &[], "ok")]);
    let mut host = UreqHost::with_timeout(Duration::from_secs(5));
    host.set_rate_limit(Some(RateLimit {
        max_requests: 1,
        window_ms: 60_000,
    }));

    let response = host
        .request(HttpRequest {
            method: None,
            url: format!("{base_url}/first"),
            headers: None,
            query: None,
            body: None,
            browser_session: None,
        })
        .unwrap();
    assert_eq!(response.body, "ok");

    let error = host
        .request(HttpRequest {
            method: None,
            url: format!("{base_url}/second"),
            headers: None,
            query: None,
            body: None,
            browser_session: None,
        })
        .unwrap_err();
    assert!(matches!(error, HostError::RateLimited));
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(2)).unwrap().len(),
        1
    );
}

#[test]
fn image_fetch_retries_candidates_after_not_found() {
    let (base_url, requests) = spawn_http_server(vec![
        http_response("404 Not Found", &[], "missing"),
        http_response("200 OK", &[("X-Image", "candidate")], "image"),
    ]);
    let mut host = UreqHost::with_timeout(Duration::from_secs(5));
    let response = host
        .fetch_image(&ImageRequest {
            url: format!("{base_url}/primary"),
            headers: None,
            candidates: Some(vec![ImageRequestCandidate {
                url: format!("{base_url}/candidate"),
                headers: Some(BTreeMap::from([(
                    "Referer".into(),
                    "https://example.com/".into(),
                )])),
            }]),
        })
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"image");
    let requests = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("referer: https://example.com/")
    );
}

#[test]
fn maps_ctx_fail_to_a_structured_source_error() {
    let (package, root) = package(
        r#"
            export async function getPages(ctx) {
                ctx.fail("NOT_FOUND", "missing", { key: "demo" });
            }
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
        "#,
        &[],
    );
    let input = serde_json::json!({
        "manga": manga("demo", "Demo"),
        "chapter": {"key": "chapter", "name": "Chapter 1"}
    });
    let error = Engine::default()
        .run(&package, Operation::GetPages, input)
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::Source { code, .. } if code == "NOT_FOUND"
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn redacts_configured_values_from_source_diagnostics() {
    let (package, root) = package_0_0_2(
        r#"
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages(ctx) { ctx.fail('BAD', `token ${ctx.settings.token}`, { token: ctx.settings.token }); }
            export async function getSettings() { return [{ id: 'token', name: 'Token', type: 'text', secret: true }]; }
        "#,
        serde_json::json!({"settings":true}),
    );
    let error = Engine::default()
        .with_settings(
            serde_json::json!({"token":"super-secret"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .run(
            &package,
            Operation::GetPages,
            serde_json::json!({"manga":manga("m","M"),"chapter":{"key":"c","name":"C"}}),
        )
        .unwrap_err();
    assert!(!error.to_string().contains("super-secret"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn loads_package_local_es_modules() {
    let (_package, root) = package(
        r#"
            import { manga } from "./helpers.js";
            export async function getMangaList() { return { items: [manga], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
        "#,
        &[],
    );
    fs::write(
        root.join("helpers.js"),
        "export const manga = { key: 'module', title: 'Imported' };",
    )
    .unwrap();
    let package = Package::load(&root).unwrap();
    let output = Engine::default()
        .run(
            &package,
            Operation::GetMangaList,
            serde_json::json!({"listingId":"latest","page":1}),
        )
        .unwrap();
    assert_eq!(output["items"][0], manga("module", "Imported"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exposes_html_selection_to_html_sources() {
    let (package, root) = package(
        r#"
            export async function getMangaList(ctx) {
                const card = ctx.html.parse('<article class="card"><a href="/demo"> Demo </a></article>').select('article.card');
                return { items: [{ key: card.select('a').attr('href'), title: card.text().trim() }], hasNextPage: false };
            }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
        "#,
        &["html"],
    );
    let output = Engine::default()
        .run(
            &package,
            Operation::GetMangaList,
            serde_json::json!({"listingId":"latest","page":1}),
        )
        .unwrap();
    assert_eq!(output["items"][0], manga("/demo", "Demo"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runs_enabled_extension_operations() {
    let (package, root) = package_with_capabilities(
        r#"
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
            export async function getFilters() { return [{ id: 'language', name: 'Language', type: 'multi-select', options: [{id:'en',name:'English'}], default: ['en'] }]; }
            export async function resolveUrl(_ctx, input) { return input.url.endsWith('/title/demo') ? { type: 'manga', mangaKey: 'demo' } : null; }
        "#,
        serde_json::json!({"filters":true,"urlResolution":true}),
    );
    let engine = Engine::default();
    let filters = engine
        .run_extension(&package, ExtensionOperation::GetFilters, Value::Null)
        .unwrap();
    assert_eq!(filters[0]["id"], "language");
    let resolved = engine
        .run_extension(
            &package,
            ExtensionOperation::ResolveUrl,
            serde_json::json!({"url":"https://example.com/title/demo"}),
        )
        .unwrap();
    assert_eq!(resolved["mangaKey"], "demo");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runs_0_0_2_settings_and_rich_filters() {
    let (package, root) = package_0_0_2(
        r#"
            export async function getMangaList(ctx) { return { items: [{ key: 'settings', title: ctx.settings.quality }], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
            export async function getSettings() { return [{ id: 'quality', name: 'Quality', type: 'select', options: [{id:'high',name:'High'}], default: 'high' }]; }
            export async function getFilters() { return [{ id: 'genres', name: 'Genres', type: 'group', children: [{ id: 'tag', name: 'Tag', type: 'tri-state', options: [{id:'action',name:'Action'}] }] }]; }
        "#,
        serde_json::json!({"settings":true,"filters":true}),
    );
    let engine = Engine::default();
    let output = engine
        .run(
            &package,
            Operation::GetMangaList,
            serde_json::json!({"listingId":"latest","page":1}),
        )
        .unwrap();
    assert_eq!(output["items"][0]["title"], "high");
    assert!(
        engine
            .run_extension(&package, ExtensionOperation::GetSettings, Value::Null)
            .is_ok()
    );
    assert!(
        engine
            .run_extension(&package, ExtensionOperation::GetFilters, Value::Null)
            .is_ok()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn validates_image_request_candidates() {
    let (package, root) = package_0_0_2(
        r#"
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
            export async function getImageRequest() {
                return { url: 'https://cdn.example/full.jpg', candidates: [
                    { url: 'https://cdn.example/reduced.jpg', headers: { Referer: 'https://example.com/' } }
                ] };
            }
        "#,
        serde_json::json!({"imageRequests":true}),
    );
    let output = Engine::default()
        .run_extension(
            &package,
            ExtensionOperation::GetImageRequest,
            serde_json::json!({
                "manga": {"key":"demo","title":"Demo"},
                "url": "https://cdn.example/full.jpg"
            }),
        )
        .unwrap();
    assert_eq!(
        output["candidates"][0]["url"],
        "https://cdn.example/reduced.jpg"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn browser_mock_enforces_policy_and_keeps_session_handoff_opaque() {
    let (package, root) = package_0_0_2_with_requires(
        r#"
            export async function getMangaList(ctx) {
                const browser = await ctx.browser.run({ url: 'https://example.com/login', script: 'return { ready: true };' });
                const response = await ctx.http.request({ url: 'https://example.com/api/list', browserSession: browser.session });
                return { items: [{ key: browser.payload.ready.toString(), title: response.body }], hasNextPage: false };
            }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
        "#,
        &["http", "browser"],
        serde_json::json!({}),
    );
    let mut host = MockBrowserHost::new(
        BrowserPolicy::for_origins(["https://example.com".into()]),
        [MockBrowserReply {
            url: "https://example.com/login".into(),
            payload: Some(serde_json::json!({"ready": true})),
        }],
    );
    assert!(
        host.run_browser(crate::BrowserRunRequest {
            url: "https://example.com/login".into(),
            html: None,
            initialization_script: None,
            script: "return {};".into(),
            timeout_ms: None,
        })
        .is_err()
    );
    host.grant_consent();
    host.set_session_response(
        "https://example.com/api/list",
        HttpResponse {
            status: 200,
            url: "https://example.com/api/list".into(),
            headers: BTreeMap::new(),
            body: "session-backed".into(),
        },
    );
    let output = Engine::with_host(host)
        .run(
            &package,
            Operation::GetMangaList,
            serde_json::json!({"listingId":"latest","page":1}),
        )
        .unwrap();
    assert_eq!(output["items"][0], manga("true", "session-backed"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn browser_sessions_are_isolated_by_host_scope() {
    let policy = BrowserPolicy::for_origins(["https://example.com".into()])
        .scoped("org.example.one", "profile-a");
    let reply = MockBrowserReply {
        url: "https://example.com/login".into(),
        payload: None,
    };
    let mut first = MockBrowserHost::new(policy.clone(), [reply.clone()]);
    let mut second = MockBrowserHost::new(policy.scoped("org.example.two", "profile-b"), [reply]);
    first.grant_consent();
    second.grant_consent();
    let first_session = first
        .run_browser(crate::BrowserRunRequest {
            url: "https://example.com/login".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: None,
        })
        .unwrap()
        .session;
    let second_session = second
        .run_browser(crate::BrowserRunRequest {
            url: "https://example.com/login".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: None,
        })
        .unwrap()
        .session;
    assert_ne!(first_session, second_session);
    assert!(matches!(
        second.request(HttpRequest {
            method: None,
            url: "https://example.com/data".into(),
            headers: None,
            query: None,
            body: None,
            browser_session: Some(first_session)
        }),
        Err(HostError::Unsupported)
    ));
}

#[test]
fn browser_mock_captures_redirect_payload_and_denies_policy_violations() {
    let policy = BrowserPolicy::for_origins(["https://example.com".into()]);
    let mut host = MockBrowserHost::new(
        policy,
        [MockBrowserReply {
            url: "https://example.com/final".into(),
            payload: Some(serde_json::json!({"captured":true})),
        }],
    );
    host.grant_consent();
    let result = host
        .run_browser(crate::BrowserRunRequest {
            url: "https://example.com/start".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: Some(30_000),
        })
        .unwrap();
    assert_eq!(result.url, "https://example.com/final");
    assert_eq!(result.payload, Some(serde_json::json!({"captured":true})));
    assert!(
        host.run_browser(crate::BrowserRunRequest {
            url: "https://evil.example/".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: None
        })
        .is_err()
    );
    assert!(
        host.run_browser(crate::BrowserRunRequest {
            url: "https://example.com/slow".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: Some(30_001)
        })
        .is_err()
    );
}

struct FakeInteractiveSurface;
impl InteractiveBrowserSurface for FakeInteractiveSurface {
    fn run_interactive(
        &mut self,
        request: crate::BrowserRunRequest,
    ) -> Result<crate::BrowserRunResult, HostError> {
        Ok(crate::BrowserRunResult {
            url: request.url,
            payload: None,
            session: "surface-session".into(),
        })
    }
    fn request_with_session(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        Ok(HttpResponse {
            status: 200,
            url: request.url,
            headers: BTreeMap::new(),
            body: "interactive".into(),
        })
    }
}

#[test]
fn interactive_host_enforces_policy_and_hands_off_opaque_sessions() {
    let mut host = InteractiveBrowserHost::new(
        BrowserPolicy::for_origins(["https://example.com".into()]),
        FakeInteractiveSurface,
    );
    assert!(
        host.run_browser(crate::BrowserRunRequest {
            url: "https://example.com/".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: None
        })
        .is_err()
    );
    let mut host = InteractiveBrowserHost::new(
        {
            let mut policy = BrowserPolicy::for_origins(["https://example.com".into()]);
            policy.consent_granted = true;
            policy
        },
        FakeInteractiveSurface,
    );
    let session = host
        .run_browser(crate::BrowserRunRequest {
            url: "https://example.com/".into(),
            html: None,
            initialization_script: None,
            script: "".into(),
            timeout_ms: None,
        })
        .unwrap()
        .session;
    assert_eq!(
        host.request(HttpRequest {
            method: None,
            url: "https://example.com/data".into(),
            headers: None,
            query: None,
            body: None,
            browser_session: Some(session)
        })
        .unwrap()
        .body,
        "interactive"
    );
}

#[test]
fn cdp_host_rejects_non_local_debugging_endpoints() {
    assert!(
        CdpBrowserHost::new(
            "http://example.com:9222",
            BrowserPolicy::for_origins(["https://example.com".into()])
        )
        .is_err()
    );
    assert!(
        CdpBrowserHost::new(
            "http://127.0.0.1:9222",
            BrowserPolicy::for_origins(["https://example.com".into()])
        )
        .is_ok()
    );
}

#[test]
fn exposes_bounded_image_create_and_encode_api() {
    let (package, root) = package_0_0_2_with_requires(
        r#"
            export async function getMangaList(ctx) {
                const bitmap = ctx.image.create(1, 1);
                const bytes = await ctx.image.encode(bitmap, 'image/png');
                return { items: [{ key: 'image', title: new Uint8Array(bytes).length.toString() }], hasNextPage: false };
            }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
        "#,
        &["image"],
        serde_json::json!({}),
    );
    let output = Engine::default()
        .run(
            &package,
            Operation::GetMangaList,
            serde_json::json!({"listingId":"latest","page":1}),
        )
        .unwrap();
    assert_eq!(output["items"][0]["key"], "image");
    assert!(
        output["items"][0]["title"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap()
            > 0
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dispatches_binary_transform_image() {
    let (package, root) = package_0_0_2_with_requires(
        r#"
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
            export async function transformImage(_ctx, input) {
                const bytes = new Uint8Array(input.body); bytes[0] ^= 0xff;
                return { mimeType: input.mimeType ?? 'image/png', body: input.body };
            }
        "#,
        &["image"],
        serde_json::json!({"imageTransforms":true}),
    );
    let output = Engine::default()
        .run_image_transform(
            &package,
            ImageTransformInput {
                request: ImageRequest {
                    url: "https://cdn.example/image".into(),
                    headers: None,
                    candidates: None,
                },
                page: wef_core::Page {
                    url: None,
                    image_url: Some("https://cdn.example/image".into()),
                    thumbnail_url: None,
                    description: None,
                    headers: None,
                    context: None,
                },
                status: 200,
                headers: BTreeMap::new(),
                mime_type: Some("image/png".into()),
                body: vec![0x0f, 0x10],
            },
        )
        .unwrap();
    assert_eq!(output.mime_type, "image/png");
    assert_eq!(output.body, vec![0xf0, 0x10]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn binary_fixture_is_decoded_and_reencoded_at_the_transform_boundary() {
    let (package, root) = package_0_0_2_with_requires(
        r#"
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
            export async function transformImage(ctx, input) { const image = await ctx.image.decode(input.body); return { mimeType: 'image/png', body: await ctx.image.encode(image, 'image/png') }; }
        "#,
        &["image"],
        serde_json::json!({"imageTransforms":true}),
    );
    let body = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/conformance/binary/black.ppm"
    ))
    .unwrap();
    let output = Engine::default()
        .run_image_transform(
            &package,
            ImageTransformInput {
                request: ImageRequest {
                    url: "https://cdn.example/image".into(),
                    headers: None,
                    candidates: None,
                },
                page: wef_core::Page {
                    url: None,
                    image_url: Some("https://cdn.example/image".into()),
                    thumbnail_url: None,
                    description: None,
                    headers: None,
                    context: None,
                },
                status: 200,
                headers: BTreeMap::new(),
                mime_type: Some("image/x-portable-pixmap".into()),
                body,
            },
        )
        .unwrap();
    assert_eq!(output.mime_type, "image/png");
    assert!(output.body.starts_with(b"\x89PNG"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn transform_loop_limit_stops_unbounded_source_code() {
    let (package, root) = package_0_0_2_with_requires(
        r#"
            export async function getMangaList() { return { items: [], hasNextPage: false }; }
            export async function search() { return { items: [], hasNextPage: false }; }
            export async function getMangaUpdate() { return { chapters: [] }; }
            export async function getPages() { return []; }
            export async function transformImage(_ctx, _input) { while (true) {} }
        "#,
        &["image"],
        serde_json::json!({"imageTransforms":true}),
    );
    let error = Engine::default()
        .run_image_transform(
            &package,
            ImageTransformInput {
                request: ImageRequest {
                    url: "https://cdn.example/image".into(),
                    headers: None,
                    candidates: None,
                },
                page: wef_core::Page {
                    url: None,
                    image_url: Some("https://cdn.example/image".into()),
                    thumbnail_url: None,
                    description: None,
                    headers: None,
                    context: None,
                },
                status: 200,
                headers: BTreeMap::new(),
                mime_type: None,
                body: vec![0],
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("execution limit"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn transform_rejects_oversized_input_before_source_execution() {
    let (package, root) = package_0_0_2_with_requires(
        r#"export async function getMangaList(){return {items:[],hasNextPage:false}} export async function search(){return {items:[],hasNextPage:false}} export async function getMangaUpdate(){return {chapters:[]}} export async function getPages(){return []} export async function transformImage(_ctx,input){return {mimeType:'image/png',body:input.body}}"#,
        &["image"],
        serde_json::json!({"imageTransforms":true}),
    );
    let error = Engine::default()
        .run_image_transform(
            &package,
            ImageTransformInput {
                request: ImageRequest {
                    url: "https://cdn.example/image".into(),
                    headers: None,
                    candidates: None,
                },
                page: wef_core::Page {
                    url: None,
                    image_url: Some("https://cdn.example/image".into()),
                    thumbnail_url: None,
                    description: None,
                    headers: None,
                    context: None,
                },
                status: 200,
                headers: BTreeMap::new(),
                mime_type: None,
                body: vec![0; 20 * 1024 * 1024 + 1],
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("byte limit"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn grid_descramble_fixture_uses_image_blits() {
    let (package, root) = package_0_0_2_with_requires(
        r#"
            export async function getMangaList(){return {items:[],hasNextPage:false}} export async function search(){return {items:[],hasNextPage:false}} export async function getMangaUpdate(){return {chapters:[]}} export async function getPages(){return []}
            export async function transformImage(ctx,input){ const source=await ctx.image.decode(input.body), out=ctx.image.create(source.width,source.height), w=source.width/5,h=source.height/5; for(let i=0;i<25;i++){const s=24-i;ctx.image.blit(out,source,{x:(s%5)*w,y:Math.floor(s/5)*h,width:w,height:h},{x:(i%5)*w,y:Math.floor(i/5)*h,width:w,height:h});} return {mimeType:'image/png',body:await ctx.image.encode(out,'image/png')}; }
        "#,
        &["image"],
        serde_json::json!({"imageTransforms":true}),
    );
    let body = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/conformance/binary/grid-5x5.ppm"
    ))
    .unwrap();
    let output = Engine::default()
        .run_image_transform(
            &package,
            ImageTransformInput {
                request: ImageRequest {
                    url: "https://cdn.example/image".into(),
                    headers: None,
                    candidates: None,
                },
                page: wef_core::Page {
                    url: None,
                    image_url: Some("https://cdn.example/image".into()),
                    thumbnail_url: None,
                    description: None,
                    headers: None,
                    context: None,
                },
                status: 200,
                headers: BTreeMap::new(),
                mime_type: None,
                body,
            },
        )
        .unwrap();
    assert!(output.body.starts_with(b"\x89PNG"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn transform_rejects_malformed_image_fixture() {
    let (package, root) = package_0_0_2_with_requires(
        r#"export async function getMangaList(){return {items:[],hasNextPage:false}} export async function search(){return {items:[],hasNextPage:false}} export async function getMangaUpdate(){return {chapters:[]}} export async function getPages(){return []} export async function transformImage(ctx,input){await ctx.image.decode(input.body);return {mimeType:'image/png',body:input.body}}"#,
        &["image"],
        serde_json::json!({"imageTransforms":true}),
    );
    let error = Engine::default()
        .run_image_transform(
            &package,
            ImageTransformInput {
                request: ImageRequest {
                    url: "https://cdn.example/image".into(),
                    headers: None,
                    candidates: None,
                },
                page: wef_core::Page {
                    url: None,
                    image_url: Some("https://cdn.example/image".into()),
                    thumbnail_url: None,
                    description: None,
                    headers: None,
                    context: None,
                },
                status: 200,
                headers: BTreeMap::new(),
                mime_type: None,
                body: b"not an image".to_vec(),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("image") || error.to_string().contains("format"));
    fs::remove_dir_all(root).unwrap();
}
