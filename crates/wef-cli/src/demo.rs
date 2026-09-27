//! Local web demo for WEF sources: browse, search with filters, chapters,
//! pages. Stateless by design — no history, library, updates, downloads,
//! or persistence beyond the engine store file. Serves 127.0.0.1 only.
//!
//! The filter form is the first real consumer of the section 11.1.1 value
//! encoding: controls render from `getFilters` declarations and parse back
//! into the exact shapes readers must produce. All source strings are
//! HTML-escaped (reader rule: source output is untrusted markup).

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    time::Duration,
};

use serde_json::{Map, Value, json};
use wef_engine_rs::{
    BrowserPolicy, CdpBrowserHost, Engine, ExtensionOperation, Operation, Package, UreqHost,
    wef_core,
};

use super::{
    extract_cdp_option, extract_session_option, extract_settings_option, extract_store_option,
};

const DEFAULT_PORT: u16 = 8099;
const PAGE_CACHE_LIMIT: usize = 8;
const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

pub fn demo(args: &[String]) -> Result<String, String> {
    let (session_path, args) = extract_session_option(args)?;
    let (settings, args) = extract_settings_option(&args)?;
    let (cdp_url, args) = extract_cdp_option(&args)?;
    let (store_path, args) = extract_store_option(&args)?;
    let (port, dir, paths) = extract_demo_options(&args)?;
    if paths.is_empty() && dir.is_none() {
        return Err("expected wef demo <path>... [--dir <extensions-dir>] [--port N]".into());
    }
    if cdp_url.is_some() && session_path.is_some() {
        return Err(
            "--cdp uses the browser profile for cookies; do not combine it with --session".into(),
        );
    }
    let cdp_url = cdp_url.or_else(|| {
        if probe_cdp(DEFAULT_CDP_URL) {
            eprintln!("demo: browser found at {DEFAULT_CDP_URL}");
            Some(DEFAULT_CDP_URL.into())
        } else {
            None
        }
    });
    let mut state = Demo::load(
        &paths,
        DemoOptions {
            session_path,
            settings,
            cdp_url,
            store_path,
            extensions_dir: dir,
        },
    )?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("could not bind :{port}: {error}"))?;
    let address = format!("http://127.0.0.1:{port}");
    eprintln!("wef demo: {address} (Ctrl-C to stop)");
    for stream in listener.incoming() {
        let mut stream = stream.map_err(|error| format!("connection failed: {error}"))?;
        state.rescan();
        if let Err(error) = handle_one(&mut state, &mut stream) {
            let _ = respond(&mut stream, 500, "text/plain", error.as_bytes());
        }
    }
    Ok(String::new())
}

fn extract_demo_options(args: &[String]) -> Result<(u16, Option<PathBuf>, Vec<String>), String> {
    let mut port = DEFAULT_PORT;
    let mut dir = None;
    let mut rest = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--port" {
            let value = iter
                .next()
                .ok_or_else(|| "expected a port number after --port".to_owned())?;
            port = value
                .parse::<u16>()
                .map_err(|_| format!("invalid port {value:?}"))?;
        } else if arg == "--dir" {
            let value = iter
                .next()
                .ok_or_else(|| "expected a directory after --dir".to_owned())?;
            dir = Some(PathBuf::from(value));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((port, dir, rest))
}

struct DemoOptions {
    session_path: Option<PathBuf>,
    settings: Map<String, Value>,
    cdp_url: Option<String>,
    store_path: Option<PathBuf>,
    extensions_dir: Option<PathBuf>,
}

const DEFAULT_CDP_URL: &str = "http://127.0.0.1:9222";

/// Uses a local browser when one is already listening. Probing keeps the
/// default safe: without a browser, hosts stay browserless and sources
/// take their no-browser paths instead of failing on refused connections.
fn probe_cdp(url: &str) -> bool {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(1500)))
        .build()
        .new_agent()
        .get(&format!("{url}/json/version"))
        .call()
        .map(|response| response.status().as_u16() == 200)
        .unwrap_or(false)
}

struct DemoSource {
    id: String,
    dynamic: bool,
    package: Package,
    engine: Engine,
    filters: Option<Vec<Value>>,
}

struct CachedPages {
    manga: Value,
    chapter: Value,
    pages: Vec<Value>,
}

struct Demo {
    sources: Vec<DemoSource>,
    updates: HashMap<String, Value>,
    pages: HashMap<String, CachedPages>,
    page_order: Vec<String>,
    extensions_dir: Option<PathBuf>,
    base_settings: Map<String, Value>,
    session_path: Option<PathBuf>,
    cdp_url: Option<String>,
    store_path: Option<PathBuf>,
    cookie_host: Option<UreqHost>,
    cookie_path: Option<PathBuf>,
    agent: ureq::Agent,
}

/// Shared host inputs, stored so settings changes can rebuild engines.
struct HostInputs {
    session_path: Option<PathBuf>,
    cdp_url: Option<String>,
}

fn build_engine(
    package: &Package,
    settings: &Map<String, Value>,
    inputs: &HostInputs,
) -> Result<(Engine, Option<UreqHost>), String> {
    if let Some(cdp_url) = &inputs.cdp_url {
        let mut policy = BrowserPolicy::for_origins(package.manifest().base_urls.clone());
        policy.consent_granted = true;
        let engine = Engine::with_host(
            CdpBrowserHost::new(cdp_url, policy).map_err(|error| error.to_string())?,
        )
        .with_settings(settings.clone());
        Ok((engine, None))
    } else {
        let host = UreqHost::default();
        if let Some(session_path) = &inputs.session_path
            && session_path.exists()
        {
            let file = fs::File::open(session_path).map_err(|error| {
                format!(
                    "could not open cookie session {}: {error}",
                    session_path.display()
                )
            })?;
            host.load_cookie_jar_json(std::io::BufReader::new(file))
                .map_err(|error| error.to_string())?;
        }
        let engine = Engine::with_host(host.clone()).with_settings(settings.clone());
        Ok((engine, Some(host)))
    }
}

impl Demo {
    fn load(paths: &[String], options: DemoOptions) -> Result<Self, String> {
        let mut demo = Self {
            sources: Vec::new(),
            updates: HashMap::new(),
            pages: HashMap::new(),
            page_order: Vec::new(),
            extensions_dir: options.extensions_dir,
            base_settings: options.settings.clone(),
            session_path: options.session_path.clone(),
            cdp_url: options.cdp_url.clone(),
            store_path: options.store_path.clone(),
            cookie_host: None,
            cookie_path: options.session_path,
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .new_agent(),
        };
        for path in paths {
            demo.load_path(path, false)?;
        }
        if demo.sources.is_empty() && demo.extensions_dir.is_none() {
            return Err("expected wef demo <path>... [--dir <extensions-dir>]".into());
        }
        Ok(demo)
    }

    fn load_path(&mut self, path: &str, dynamic: bool) -> Result<(), String> {
        let package = super::load_package(path)?;
        let id = package.manifest().id.clone();
        if self.sources.iter().any(|source| source.id == id) {
            return Ok(());
        }
        let settings = self.base_settings.clone();
        let inputs = HostInputs {
            session_path: self.session_path.clone(),
            cdp_url: self.cdp_url.clone(),
        };
        let (engine, http) = build_engine(&package, &settings, &inputs)?;
        super::restore_store(&engine, &self.store_path)?;
        if http.is_some() && self.cookie_host.is_none() {
            self.cookie_host = http.clone();
        }
        self.sources.push(DemoSource {
            id,
            dynamic,
            package,
            engine,
            filters: None,
        });
        Ok(())
    }

    /// Picks up newly dropped-in extension directories (and forgets
    /// removed ones). Runs on every request; cheap until something loads.
    fn rescan(&mut self) {
        let Some(dir) = self.extensions_dir.clone() else {
            return;
        };
        let entries = fs::read_dir(&dir).map(|entries| entries.flatten().collect::<Vec<_>>());
        let Ok(entries) = entries else {
            return;
        };
        let mut present = Vec::new();
        for entry in entries {
            let manifest = entry.path().join("wef.json");
            if !manifest.is_file() {
                continue;
            }
            let path = entry.path().to_string_lossy().into_owned();
            if let Err(error) = self.load_path(&path, true) {
                eprintln!("demo: skipping {path}: {error}");
                continue;
            }
            if let Ok(package) = super::load_package(&path) {
                present.push(package.manifest().id.clone());
            }
        }
        self.sources.retain(|source| {
            !source.dynamic || present.iter().any(|id| id == &source.id) || {
                eprintln!("demo: unloaded {}", source.id);
                false
            }
        });
        self.updates.retain(|key, _| {
            self.sources
                .iter()
                .any(|source| key.starts_with(&format!("{}\0", source.id)))
        });
        self.pages.retain(|key, _| {
            self.sources
                .iter()
                .any(|source| key.starts_with(&format!("{}\0", source.id)))
        });
    }

    /// Rebuilds every engine (used when the shared CDP endpoint changes).
    /// Plain-HTTP hosts are cloned, so cookies survive; browser sessions
    /// re-bootstrap on next use.
    fn rebuild_all(&mut self) -> Result<(), String> {
        let inputs = HostInputs {
            session_path: self.session_path.clone(),
            cdp_url: self.cdp_url.clone(),
        };
        let store_path = self.store_path.clone();
        let base = self.base_settings.clone();
        let mut cookie_host = None;
        for source in &mut self.sources {
            let (engine, http) = build_engine(&source.package, &base, &inputs)?;
            super::restore_store(&engine, &store_path)?;
            source.engine = engine;
            if cookie_host.is_none() {
                cookie_host = http;
            }
        }
        self.cookie_host = cookie_host;
        Ok(())
    }

    fn source(&self, id: &str) -> Result<&DemoSource, String> {
        self.sources
            .iter()
            .find(|source| source.id == id)
            .ok_or_else(|| format!("unknown source {id:?}"))
    }

    fn source_mut(&mut self, id: &str) -> Result<&mut DemoSource, String> {
        self.sources
            .iter_mut()
            .find(|source| source.id == id)
            .ok_or_else(|| format!("unknown source {id:?}"))
    }

    fn persist(&self) -> Result<(), String> {
        for source in &self.sources {
            super::save_store(&source.engine, &self.store_path)?;
        }
        if let (Some(host), Some(path)) = (&self.cookie_host, &self.cookie_path) {
            let file = fs::File::create(path).map_err(|error| {
                format!("could not save cookie session {}: {error}", path.display())
            })?;
            host.save_cookie_jar_json(&mut std::io::BufWriter::new(file))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn filters_of(&mut self, id: &str) -> Result<Vec<Value>, String> {
        let cached = self.source(id)?.filters.clone();
        if let Some(filters) = cached {
            return Ok(filters);
        }
        let source = self.source(id)?;
        let output = source
            .engine
            .run_extension(&source.package, ExtensionOperation::GetFilters, Value::Null)
            .map_err(|error| error.to_string())?;
        let filters: Vec<Value> =
            serde_json::from_value(output).map_err(|error| format!("invalid filters: {error}"))?;
        self.source_mut(id)?.filters = Some(filters.clone());
        Ok(filters)
    }
}

fn handle_one(state: &mut Demo, stream: &mut std::net::TcpStream) -> Result<(), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|error| error.to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            break;
        }
    }
    drop(reader);
    if method != "GET" {
        return respond(stream, 405, "text/plain", b"demo serves GET only");
    }
    let (path, query) = match target.find('?') {
        Some(index) => (&target[..index], &target[index + 1..]),
        None => (target, ""),
    };
    let params = parse_query(query);
    let route = match path {
        "/" => route_index(state),
        "/source" => route_source(state, &params),
        "/manga" => route_manga(state, &params),
        "/chapter" => route_chapter(state, &params),
        "/settings" => route_settings(state, &params),
        "/add" => route_add(state, &params),
        "/image" => return route_image(state, &params, stream),
        _ => return respond(stream, 404, "text/plain", b"unknown route"),
    };
    match route {
        Ok(html) => respond(stream, 200, "text/html; charset=utf-8", html.as_bytes()),
        Err(message) => {
            let body = format!(
                "{}<p class=\"error\">{}</p>",
                navbar(state, None),
                esc(&message)
            );
            respond(
                stream,
                500,
                "text/html; charset=utf-8",
                page("Error", &body).as_bytes(),
            )
        }
    }
}

fn respond(
    stream: &mut std::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Error",
        _ => "OK",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| error.to_string())
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.find('=') {
            Some(index) => (pct_decode(&pair[..index]), pct_decode(&pair[index + 1..])),
            None => (pct_decode(pair), String::new()),
        })
        .collect()
}

fn pct_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
            continue;
        }
        if bytes[index] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn param(params: &[(String, String)], name: &str) -> String {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

fn esc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for char in value.chars() {
        match char {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(char),
        }
    }
    out
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>\
        *{{box-sizing:border-box}}\
        body{{background:#05070d;color:#dbe7f3;font:14px/1.45 system-ui,-apple-system,\"Segoe UI\",sans-serif;margin:0}}\
        .wrap{{padding:0 16px 40px}}\
        a{{color:#7dd3fc;text-decoration:none}}a:hover{{text-decoration:underline}}\
        h1{{font-size:22px;margin:14px 0 8px}}h2{{font-size:17px;margin:16px 0 6px}}\
        nav{{display:flex;align-items:center;gap:18px;padding:10px 16px;border-bottom:1px solid #1e3a5f;background:#070c16}}\
        nav .brand{{font-weight:700;font-size:16px}}\
        nav details{{position:relative}}nav summary{{cursor:pointer;list-style:none}}nav summary::-webkit-details-marker{{display:none}}\
        nav details ul{{position:absolute;top:140%;left:0;min-width:220px;max-height:60vh;overflow:auto;background:#0b1220;border:1px solid #1e3a5f;border-radius:8px;padding:6px;margin:0;z-index:10}}\
        nav details li{{list-style:none}}nav details li a{{display:block;padding:6px 10px;border-radius:6px;white-space:nowrap}}\
        nav details li a:hover{{background:#16283f;text-decoration:none}}\
        nav .spacer{{flex:1}}nav .status{{color:#7d93ad;font-size:12px;white-space:nowrap}}\
        .listings a{{margin-right:12px}}\
        .searchrow{{display:flex;flex-wrap:wrap;gap:8px 14px;align-items:flex-end;background:#070c16;border:1px solid #1e3a5f;border-radius:10px;padding:10px 12px;margin:10px 0}}\
        .searchrow form{{display:flex;flex-wrap:wrap;gap:6px 10px;align-items:flex-end;margin:0}}\
        .searchrow label{{display:inline-flex;align-items:center;gap:5px;margin:0;white-space:nowrap}}\
        .searchrow fieldset{{margin:0}}\
        .searchrow p{{margin:0}}\
        fieldset{{border:1px solid #1e3a5f;border-radius:8px;padding:6px 10px}}\
        legend{{padding:0 5px;font-size:12px;color:#7d93ad}}\
        input[type=text],input[type=number],select{{background:#0b1220;border:1px solid #2a4a70;color:#dbe7f3;border-radius:8px;padding:6px 9px;font:inherit}}\
        input[type=text]:focus,input[type=number]:focus,select:focus{{outline:none;border-color:#38bdf8}}\
        .searchrow input[type=text]{{width:13em}}\
        input[type=number]{{width:4.5em}}\
        .searchrow select{{max-width:15em}}\
        input[type=checkbox],input[type=radio]{{accent-color:#38bdf8;width:15px;height:15px;margin:0;flex:none}}\
        button{{width:auto;background:#0369a1;color:#fff;border:0;border-radius:8px;padding:7px 18px;font:inherit;cursor:pointer;white-space:nowrap}}\
        button:hover{{background:#0284c7}}\
        .pill{{display:inline-block;background:#0e2233;border:1px solid #2a4a70;border-radius:20px;padding:1px 6px 1px 10px;margin:1px 4px 1px 0;white-space:nowrap}}\
        .pill.exc{{border-color:#7f2d3f;background:#2a1220}}\
        .pill a{{margin-left:4px;padding:0 4px}}\
        ul.cards{{list-style:none;display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:16px 12px;padding:0;margin:12px 0}}\
        ul.cards li{{list-style:none}}\
        ul.cards a.card{{display:flex;flex-direction:column;gap:6px}}\
        ul.cards a.card:hover{{text-decoration:none}}\
        ul.cards img{{width:100%;height:210px;object-fit:cover;border-radius:8px;background:#0b1220}}\
        ul.cards span{{font-size:13px;text-align:center;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden}}\
        ul.chapters{{list-style:none;padding:0}}ul.chapters li{{list-style:none;padding:5px 0;border-bottom:1px solid #101a2c}}\
        img.page{{max-width:100%;display:block;margin:8px auto}}\
        .error{{color:#fda4af}}.muted{{color:#7d93ad}}\
        </style></head><body><div class=\"wrap\">{}</div></body></html>",
        esc(title),
        body
    )
}

/// Top navigation: brand, sources dropdown (loaded sources plus an
/// add-by-path entry), and global settings. No JavaScript — the dropdown
/// is a plain details element.
fn navbar(state: &Demo, active: Option<&str>) -> String {
    let mut items = String::new();
    for source in &state.sources {
        let marker = if Some(source.id.as_str()) == active {
            " ●"
        } else {
            ""
        };
        items.push_str(&format!(
            "<li><a href=\"/source?id={}\">{}{}</a></li>",
            esc(&source.id),
            esc(&source.package.manifest().name),
            marker
        ));
    }
    items.push_str("<li><a href=\"/add\">＋ add source…</a></li>");
    let status = match &state.cdp_url {
        Some(url) => format!("cdp: {url}"),
        None => "browserless".into(),
    };
    let watching = state
        .extensions_dir
        .as_ref()
        .map(|dir| format!(" · watching {}", dir.to_string_lossy()))
        .unwrap_or_default();
    format!(
        "<nav><a class=\"brand\" href=\"/\">WEF demo</a>\
        <details><summary>Sources ▾</summary><ul>{items}</ul></details>\
        <a href=\"/settings\">Settings</a>\
        <span class=\"spacer\"></span><span class=\"status\">{}{}</span></nav>",
        esc(&status),
        esc(&watching)
    )
}

fn render_filter_form(filters: &[Value], params: &[(String, String)]) -> (String, String) {
    let mut inner = String::new();
    let mut after = String::new();
    for filter in filters {
        let (inside, outside) = render_filter(filter, params);
        inner.push_str(&inside);
        after.push_str(&outside);
    }
    (inner, after)
}

fn filter_field(filter: &Value, field: &str) -> String {
    filter
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn filter_options(filter: &Value) -> Vec<(String, String)> {
    filter
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .map(|option| {
                    (
                        option
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        option
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn render_filter(filter: &Value, params: &[(String, String)]) -> (String, String) {
    let kind = filter_field(filter, "type");
    let id = filter_field(filter, "id");
    let name = filter_field(filter, "name");
    if kind == "group" {
        let mut children = String::new();
        let mut after = String::new();
        if let Some(items) = filter.get("children").and_then(Value::as_array) {
            for child in items {
                let (inside, outside) = render_filter(child, params);
                children.push_str(&inside);
                after.push_str(&outside);
            }
        }
        return (
            format!(
                "<fieldset><legend>{}</legend>{children}</fieldset>",
                esc(&name)
            ),
            after,
        );
    }
    let label = format!("<label>{}: ", esc(&name));
    let control = match kind.as_str() {
        "text" => {
            let current = param(params, &format!("f_{id}"));
            let current = if current.is_empty() {
                filter.get("default").and_then(Value::as_str).unwrap_or("")
            } else {
                current.as_str()
            };
            let placeholder = filter
                .get("placeholder")
                .and_then(Value::as_str)
                .unwrap_or("");
            format!(
                "<input type=\"text\" name=\"f_{id}\" value=\"{}\" placeholder=\"{}\">",
                esc(current),
                esc(placeholder)
            )
        }
        "toggle" => {
            let checked = if params.iter().any(|(key, _)| key == &format!("f_{id}")) {
                param(params, &format!("f_{id}")) == "1"
            } else {
                filter
                    .get("default")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            };
            format!(
                "<input type=\"checkbox\" name=\"f_{id}\" value=\"1\"{}>",
                if checked { " checked" } else { "" }
            )
        }
        "select" => {
            let current = param(params, &format!("f_{id}"));
            let current = if current.is_empty() {
                filter.get("default").and_then(Value::as_str).unwrap_or("")
            } else {
                current.as_str()
            };
            let mut options = String::new();
            for (value, title) in filter_options(filter) {
                options.push_str(&format!(
                    "<option value=\"{}\"{}>{}</option>",
                    esc(&value),
                    if value == current { " selected" } else { "" },
                    esc(&title)
                ));
            }
            format!("<select name=\"f_{id}\">{options}</select>")
        }
        "multi-select" => {
            let selected: Vec<String> = params
                .iter()
                .filter(|(key, _)| key == &format!("f_{id}"))
                .map(|(_, value)| value.clone())
                .collect();
            let defaults: Vec<String> = filter
                .get("default")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let active = if params.iter().any(|(key, _)| key == &format!("f_{id}")) {
                selected
            } else {
                defaults
            };
            let mut boxes = String::new();
            for (value, title) in filter_options(filter) {
                boxes.push_str(&format!(
                    "<label><input type=\"checkbox\" name=\"f_{id}\" value=\"{}\"{}>{}</label> ",
                    esc(&value),
                    if active.contains(&value) {
                        " checked"
                    } else {
                        ""
                    },
                    esc(&title)
                ));
            }
            boxes
        }
        "tri-state" => {
            // Selections persist as hidden inputs; the visible list and
            // adder render after the form (see render_tristate_after).
            let mut hidden = String::new();
            for (state, option) in current_tristate(filter, params) {
                hidden.push_str(&format!(
                    "<input type=\"hidden\" name=\"f_{id}__{option}\" value=\"{state}\">",
                    option = esc(&option),
                    state = esc(&state)
                ));
            }
            return (hidden, render_tristate_after(filter, params));
        }
        "range" => {
            let number = |part: &str| {
                let field = format!("f_{id}_{part}");
                let current = param(params, &field);
                if !current.is_empty() {
                    return current;
                }
                filter
                    .get("default")
                    .and_then(|default| default.get(part))
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            };
            format!(
                "min <input type=\"number\" step=\"any\" name=\"f_{id}_min\" value=\"{}\"> max <input type=\"number\" step=\"any\" name=\"f_{id}_max\" value=\"{}\">",
                esc(&number("min")),
                esc(&number("max"))
            )
        }
        "sort" => {
            let current = param(params, &format!("f_{id}"));
            let current = if current.is_empty() {
                filter
                    .get("default")
                    .and_then(|default| default.get("value"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            } else {
                current.as_str()
            };
            let mut options = String::new();
            for (value, title) in filter_options(filter) {
                options.push_str(&format!(
                    "<option value=\"{}\"{}>{}</option>",
                    esc(&value),
                    if value == current { " selected" } else { "" },
                    esc(&title)
                ));
            }
            let dir = param(params, &format!("f_{id}_dir"));
            let dir = if dir.is_empty() {
                filter
                    .get("default")
                    .and_then(|default| default.get("direction"))
                    .and_then(Value::as_str)
                    .unwrap_or("desc")
            } else {
                dir.as_str()
            };
            format!(
                "<select name=\"f_{id}\">{options}</select> <label><input type=\"radio\" name=\"f_{id}_dir\" value=\"asc\"{}>asc</label> <label><input type=\"radio\" name=\"f_{id}_dir\" value=\"desc\"{}>desc</label>",
                if dir == "asc" { " checked" } else { "" },
                if dir != "asc" { " checked" } else { "" }
            )
        }
        _ => return (String::new(), String::new()),
    };
    (format!("{label}{control}</label> "), String::new())
}

/// Current tri-state selections as (state, option-id) pairs, from params
/// or the filter defaults — plus the adder-form selection, so the
/// re-rendered list always matches what parsing sends to the search.
fn current_tristate(filter: &Value, params: &[(String, String)]) -> Vec<(String, String)> {
    let id = filter_field(filter, "id");
    let options = filter_options(filter);
    let mut selected = Vec::new();
    for (value, _) in &options {
        let field = format!("f_{id}__{value}");
        let current = param(params, &field);
        let current = if current.is_empty() {
            filter
                .get("default")
                .and_then(|default| default.get(value))
                .and_then(Value::as_str)
                .unwrap_or("neutral")
        } else {
            current.as_str()
        };
        if current == "include" || current == "exclude" {
            selected.push((current.to_owned(), value.clone()));
        }
    }
    let add = param(params, &format!("f_{id}_add"));
    if !add.is_empty() && options.iter().any(|(value, _)| value == &add) {
        let mode = param(params, &format!("f_{id}_mode"));
        let mode = if mode == "exclude" {
            "exclude"
        } else {
            "include"
        };
        selected.retain(|(_, option)| option != &add);
        selected.push((mode.into(), add));
    }
    selected
}

/// Tag-list UI for one tri-state filter: current selections with × remove
/// links, plus an adder form (dropdown of remaining options + mode).
/// State travels in the query string; add/remove re-runs the search.
fn render_tristate_after(filter: &Value, params: &[(String, String)]) -> String {
    let id = filter_field(filter, "id");
    let name = filter_field(filter, "name");
    let source_id = param(params, "id");
    let listing = param(params, "listing");
    let query = param(params, "q");
    let mut out = format!("<p>{}: ", esc(&name));
    let selected = current_tristate(filter, params);
    if selected.is_empty() {
        out.push_str("(none) ");
    }
    for (state, option) in &selected {
        let title = filter_options(filter)
            .into_iter()
            .find(|(value, _)| value == option)
            .map(|(_, title)| title)
            .unwrap_or_else(|| option.clone());
        // Removal keeps every other selection (converted to plain
        // keys) plus unrelated params; the page resets.
        let mut href = format!("/source?id={}", esc(&source_id));
        if !listing.is_empty() {
            href.push_str(&format!("&listing={}", esc(&listing)));
        }
        if !query.is_empty() {
            href.push_str(&format!("&q={}", esc(&pct_encode(&query))));
        }
        for (keep_state, keep_option) in &selected {
            if keep_option == option {
                continue;
            }
            href.push_str(&format!(
                "&f_{id}__{keep}={state}",
                keep = esc(keep_option),
                state = esc(keep_state)
            ));
        }
        for (key, value) in params {
            if key == "id" || key == "listing" || key == "q" || key == "page" {
                continue;
            }
            if key.starts_with(&format!("f_{id}__"))
                || key == &format!("f_{id}_add")
                || key == &format!("f_{id}_mode")
            {
                continue;
            }
            href.push_str(&format!("&{}={}", esc(key), esc(&pct_encode(value))));
        }
        out.push_str(&format!(
            "<span class=\"pill{}\">{}<a href=\"{href}\" title=\"remove\">×</a></span> ",
            if state == "exclude" { " exc" } else { "" },
            esc(&format!("{title} ({state})")),
        ));
    }
    out.push_str("</p>");
    // Adder form: hidden context (source, listing, query, every other
    // param, current selections of this filter) + dropdown + mode.
    let remaining: Vec<(String, String)> = filter_options(filter)
        .into_iter()
        .filter(|(value, _)| !selected.iter().any(|(_, chosen)| chosen == value))
        .collect();
    if remaining.is_empty() {
        return out;
    }
    out.push_str("<form method=\"get\" action=\"/source\">");
    out.push_str(&format!(
        "<input type=\"hidden\" name=\"id\" value=\"{}\">",
        esc(&source_id)
    ));
    if !listing.is_empty() {
        out.push_str(&format!(
            "<input type=\"hidden\" name=\"listing\" value=\"{}\">",
            esc(&listing)
        ));
    }
    if !query.is_empty() {
        out.push_str(&format!(
            "<input type=\"hidden\" name=\"q\" value=\"{}\">",
            esc(&query)
        ));
    }
    for (key, value) in params {
        if key == "id" || key == "listing" || key == "q" || key == "page" {
            continue;
        }
        if key.starts_with(&format!("f_{id}__"))
            || key == &format!("f_{id}_add")
            || key == &format!("f_{id}_mode")
        {
            continue;
        }
        out.push_str(&format!(
            "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
            esc(key),
            esc(value)
        ));
    }
    for (state, option) in &selected {
        out.push_str(&format!(
            "<input type=\"hidden\" name=\"f_{id}__{option}\" value=\"{state}\">",
            option = esc(option),
            state = esc(state)
        ));
    }
    out.push_str(&format!("<select name=\"f_{id}_add\">"));
    for (value, title) in &remaining {
        out.push_str(&format!(
            "<option value=\"{}\">{}</option>",
            esc(value),
            esc(title)
        ));
    }
    out.push_str("</select> ");
    out.push_str(&format!(
        "<label><input type=\"radio\" name=\"f_{id}_mode\" value=\"include\" checked>include</label> <label><input type=\"radio\" name=\"f_{id}_mode\" value=\"exclude\">exclude</label> <button type=\"submit\">add</button></form>"
    ));
    out
}

/// Parses submitted form params back into the section 11.1.1 value shapes:
/// text strings, toggle booleans, select ids, id arrays, tri-state maps,
/// `{min,max}` numbers, and `{value,direction}` sorts.
fn parse_filter_form(filters: &[Value], params: &[(String, String)]) -> Map<String, Value> {
    let mut out = Map::new();
    for filter in filters {
        collect_filter_value(filter, params, &mut out);
    }
    out
}

fn collect_filter_value(filter: &Value, params: &[(String, String)], out: &mut Map<String, Value>) {
    let kind = filter_field(filter, "type");
    let id = filter_field(filter, "id");
    if kind == "group" {
        if let Some(children) = filter.get("children").and_then(Value::as_array) {
            for child in children {
                collect_filter_value(child, params, out);
            }
        }
        return;
    }
    if id.is_empty() {
        return;
    }
    match kind.as_str() {
        "text" => {
            let value = param(params, &format!("f_{id}"));
            if !value.trim().is_empty() {
                out.insert(id, Value::String(value));
            }
        }
        "toggle" => {
            let field = format!("f_{id}");
            out.insert(id, Value::Bool(param(params, &field) == "1"));
        }
        "select" => {
            let value = param(params, &format!("f_{id}"));
            let value = if value.is_empty() {
                filter
                    .get("default")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            } else {
                value
            };
            out.insert(id, Value::String(value));
        }
        "multi-select" => {
            let values: Vec<Value> = params
                .iter()
                .filter(|(key, _)| key == &format!("f_{id}"))
                .map(|(_, value)| Value::String(value.clone()))
                .collect();
            out.insert(id, Value::Array(values));
        }
        "tri-state" => {
            let mut map = Map::new();
            for (state, option) in current_tristate(filter, params) {
                map.insert(option, Value::String(state));
            }
            if !map.is_empty() {
                out.insert(id, Value::Object(map));
            }
        }
        "range" => {
            let mut map = Map::new();
            for part in ["min", "max"] {
                let raw = param(params, &format!("f_{id}_{part}"));
                let raw = if raw.is_empty() {
                    filter
                        .get("default")
                        .and_then(|default| default.get(part))
                        .map(|value| value.to_string())
                        .unwrap_or_default()
                } else {
                    raw
                };
                if let Ok(number) = raw.parse::<f64>() {
                    map.insert(
                        part.into(),
                        Value::Number(serde_json::Number::from_f64(number).unwrap_or(0.into())),
                    );
                }
            }
            if !map.is_empty() {
                out.insert(id, Value::Object(map));
            }
        }
        "sort" => {
            let value = param(params, &format!("f_{id}"));
            let value = if value.is_empty() {
                filter
                    .get("default")
                    .and_then(|default| default.get("value"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            } else {
                value
            };
            let direction = param(params, &format!("f_{id}_dir"));
            out.insert(
                id,
                json!({"value": value, "direction": if direction == "asc" { "asc" } else { "desc" }}),
            );
        }
        _ => {}
    }
}

fn route_index(state: &Demo) -> Result<String, String> {
    let mut items = String::new();
    for source in &state.sources {
        items.push_str(&format!(
            "<li><a href=\"/source?id={}\">{}</a></li>",
            esc(&source.id),
            esc(&source.package.manifest().name)
        ));
    }
    Ok(page(
        "WEF demo",
        &format!("{}<h1>Sources</h1><ul>{items}</ul>", navbar(state, None)),
    ))
}

fn route_add(state: &mut Demo, params: &[(String, String)]) -> Result<String, String> {
    let mut body = format!("{}<h1>Add source</h1>", navbar(state, None));
    let path = param(params, "path");
    if !path.is_empty() {
        match super::load_package(&path) {
            Ok(package) => {
                let id = package.manifest().id.clone();
                match state.load_path(&path, false) {
                    Ok(()) => body.push_str(&format!(
                        "<p>loaded <a href=\"/source?id={}\">{}</a></p>",
                        esc(&id),
                        esc(&package.manifest().name)
                    )),
                    Err(error) => body.push_str(&format!("<p class=\"error\">{}</p>", esc(&error))),
                }
            }
            Err(error) => body.push_str(&format!("<p class=\"error\">{}</p>", esc(&error))),
        }
    }
    body.push_str(
        "<form method=\"get\" action=\"/add\">\
        <label>Package path: <input type=\"text\" name=\"path\" placeholder=\"/path/to/source\" size=\"48\"></label> \
        <button type=\"submit\">load</button></form>",
    );
    Ok(page("Add source", &body))
}

fn route_source(state: &mut Demo, params: &[(String, String)]) -> Result<String, String> {
    let id = param(params, "id");
    let listing = param(params, "listing");
    let listing = listing.as_str();
    let query = param(params, "q");
    let page_number: u32 = param(params, "page").parse().unwrap_or(1).max(1);
    let source = state.source(&id)?;
    let manifest_name = source.package.manifest().name.clone();
    let manifest_listings = source.package.manifest().listings.clone();
    let manifest_filters = source.package.manifest().capabilities.filters;
    let mut body = format!(
        "{}<h1>{}</h1>",
        navbar(state, Some(&id)),
        esc(&manifest_name)
    );
    body.push_str("<p class=\"listings\">");
    for listing_entry in &manifest_listings {
        body.push_str(&format!(
            " <a href=\"/source?id={}&listing={}\">{}</a>",
            esc(&id),
            esc(&listing_entry.id),
            esc(&listing_entry.name)
        ));
    }
    body.push_str("</p>");
    let filters = if manifest_filters {
        state.filters_of(&id)?
    } else {
        Vec::new()
    };
    body.push_str("<div class=\"searchrow\">");
    body.push_str(&format!(
        "<form method=\"get\" action=\"/source\"><input type=\"hidden\" name=\"id\" value=\"{}\">",
        esc(&id)
    ));
    if !listing.is_empty() {
        body.push_str(&format!(
            "<input type=\"hidden\" name=\"listing\" value=\"{}\">",
            esc(listing)
        ));
    }
    body.push_str(&format!(
        "<input type=\"text\" name=\"q\" value=\"{}\" placeholder=\"search\"> ",
        esc(&query)
    ));
    let (form_inner, form_after) = render_filter_form(&filters, params);
    body.push_str(&form_inner);
    body.push_str("<button type=\"submit\">go</button></form>");
    body.push_str(&form_after);
    body.push_str("</div>");
    // Empty queries still search when filters were submitted: genres-only
    // browsing is valid (§10.2 allows empty queries).
    let submitted = !query.is_empty() || params.iter().any(|(key, _)| key.starts_with("f_"));
    if listing.is_empty() && !submitted {
        return Ok(page(&manifest_name, &body));
    }
    let filter_values = parse_filter_form(&filters, params);
    let source = state.source(&id)?;
    let fetched = if listing.is_empty() {
        source.engine.run(
            &source.package,
            Operation::Search,
            json!({"query": query, "page": page_number, "filters": filter_values}),
        )
    } else {
        source.engine.run(
            &source.package,
            Operation::GetMangaList,
            json!({"listingId": listing, "page": page_number, "filters": filter_values}),
        )
    };
    match fetched {
        Ok(output) => match render_manga_list(&id, &output, listing, &query, params, page_number) {
            Ok(list) => body.push_str(&list),
            Err(error) => body.push_str(&format!("<p>bad results: {}</p>", esc(&error))),
        },
        Err(error) => body.push_str(&format!(
            "<p>search failed: {}</p>",
            esc(&error.to_string())
        )),
    }
    Ok(page(&manifest_name, &body))
}

fn render_manga_list(
    id: &str,
    output: &Value,
    listing: &str,
    query: &str,
    params: &[(String, String)],
    page_number: u32,
) -> Result<String, String> {
    let items = output
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "listing output has no items".to_owned())?;
    let mut body = String::from("<ul class=\"cards\">");
    for item in items {
        let key = item.get("key").and_then(Value::as_str).unwrap_or("");
        let title = item.get("title").and_then(Value::as_str).unwrap_or(key);
        let mut line = format!(
            "<li><a class=\"card\" href=\"/manga?source={}&key={}\">",
            esc(id),
            esc(&pct_encode(key)),
        );
        if let Some(cover) = item.get("coverUrl").and_then(Value::as_str) {
            line.push_str(&format!(
                "<img src=\"/image?source={}&manga={}&cover={}\" loading=\"lazy\">",
                esc(id),
                esc(&pct_encode(key)),
                esc(&pct_encode(cover))
            ));
        }
        line.push_str(&format!("<span>{}</span></a></li>", esc(title)));
        body.push_str(&line);
    }
    body.push_str("</ul>");
    if output.get("hasNextPage").and_then(Value::as_bool) == Some(true) {
        let mut next = format!("/source?id={}&page={}", esc(id), page_number + 1);
        if !listing.is_empty() {
            next.push_str(&format!("&listing={}", esc(listing)));
        }
        if !query.is_empty() {
            next.push_str(&format!("&q={}", esc(&pct_encode(query))));
        }
        for (key, value) in params {
            if key == "id" || key == "listing" || key == "q" || key == "page" {
                continue;
            }
            next.push_str(&format!("&{}={}", esc(key), esc(&pct_encode(value))));
        }
        body.push_str(&format!("<p><a href=\"{next}\">next page</a></p>"));
    }
    Ok(body)
}

fn pct_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        if matches!(byte, b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn route_manga(state: &mut Demo, params: &[(String, String)]) -> Result<String, String> {
    let id = param(params, "source");
    let key = param(params, "key");
    let source = state.source(&id)?;
    let full = source
        .engine
        .run(
            &source.package,
            Operation::GetMangaUpdate,
            json!({
                "manga": {"key": key, "title": key},
                "chapters": [],
                "fetchDetails": true,
                "fetchChapters": true,
            }),
        )
        .map_err(|error| error.to_string());
    // A flaky details fetch must not hide the chapter list: retry it alone.
    let output = match full {
        Ok(output) => output,
        Err(first) => source
            .engine
            .run(
                &source.package,
                Operation::GetMangaUpdate,
                json!({
                    "manga": {"key": key, "title": key},
                    "chapters": [],
                    "fetchDetails": false,
                    "fetchChapters": true,
                }),
            )
            .map_err(|_| first)?,
    };
    state.updates.insert(format!("{id}\0{key}"), output.clone());
    state.persist()?;
    let manga = output
        .get("manga")
        .cloned()
        .unwrap_or(json!({"key": key, "title": key}));
    let title = manga.get("title").and_then(Value::as_str).unwrap_or(&key);
    let mut body = format!(
        "{}<p><a href=\"/source?id={}\">back</a></p><h1>{}</h1>",
        navbar(state, Some(&id)),
        esc(&id),
        esc(title)
    );
    if let Some(description) = manga.get("description").and_then(Value::as_str) {
        body.push_str(&format!("<p>{}</p>", esc(description)));
    }
    for field in ["status", "authors", "artists", "tags"] {
        if let Some(value) = manga.get(field) {
            body.push_str(&format!("<p>{field}: {}</p>", esc(&render_scalar(value))));
        }
    }
    let chapters = output
        .get("chapters")
        .and_then(Value::as_array)
        .ok_or_else(|| "update returned no chapters".to_owned())?;
    body.push_str(&format!(
        "<h2>Chapters ({})</h2><ul class=\"chapters\">",
        chapters.len()
    ));
    for chapter in chapters {
        let chapter_key = chapter.get("key").and_then(Value::as_str).unwrap_or("");
        let name = chapter
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(chapter_key);
        body.push_str(&format!(
            "<li><a href=\"/chapter?source={}&manga={}&chapter={}\">{}</a></li>",
            esc(&id),
            esc(&pct_encode(&key)),
            esc(&pct_encode(chapter_key)),
            esc(name)
        ));
    }
    body.push_str("</ul>");
    Ok(page(title, &body))
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(render_scalar)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn route_chapter(state: &mut Demo, params: &[(String, String)]) -> Result<String, String> {
    let id = param(params, "source");
    let manga_key = param(params, "manga");
    let chapter_key = param(params, "chapter");
    let update = state
        .updates
        .get(&format!("{id}\0{manga_key}"))
        .cloned()
        .ok_or_else(|| "open the manga page first".to_owned())?;
    let manga = update
        .get("manga")
        .cloned()
        .unwrap_or(json!({"key": manga_key, "title": manga_key}));
    let chapter = update
        .get("chapters")
        .and_then(Value::as_array)
        .and_then(|chapters| {
            chapters
                .iter()
                .find(|chapter| {
                    chapter.get("key").and_then(Value::as_str) == Some(chapter_key.as_str())
                })
                .cloned()
        })
        .ok_or_else(|| format!("chapter {chapter_key:?} not found"))?;
    let name = chapter
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&chapter_key);
    let source = state.source(&id)?;
    let output = source
        .engine
        .run(
            &source.package,
            Operation::GetPages,
            json!({"manga": manga, "chapter": chapter}),
        )
        .map_err(|error| error.to_string())?;
    let pages: Vec<Value> =
        serde_json::from_value(output).map_err(|error| format!("invalid pages: {error}"))?;
    let cache_key = format!("{id}\0{manga_key}\0{chapter_key}");
    state.pages.insert(
        cache_key.clone(),
        CachedPages {
            manga: manga.clone(),
            chapter: chapter.clone(),
            pages: pages.clone(),
        },
    );
    state.page_order.push(cache_key);
    while state.page_order.len() > PAGE_CACHE_LIMIT {
        let old = state.page_order.remove(0);
        state.pages.remove(&old);
    }
    state.persist()?;
    let mut body = format!(
        "{}<p><a href=\"/manga?source={}&key={}\">back</a></p><h1>{}</h1>",
        navbar(state, Some(&id)),
        esc(&id),
        esc(&pct_encode(&manga_key)),
        esc(name)
    );
    for (index, _) in pages.iter().enumerate() {
        body.push_str(&format!(
            "<p><img class=\"page\" src=\"/image?source={}&manga={}&chapter={}&i={}\"></p>",
            esc(&id),
            esc(&pct_encode(&manga_key)),
            esc(&pct_encode(&chapter_key)),
            index
        ));
    }
    Ok(page(name, &body))
}

/// Current settings values as form params, so the settings form renders
/// with the same code as search filters. Explicit `"0"` marks toggles off
/// (an absent key would fall back to the declaration default instead).
/// Global settings: the CDP endpoint hosts use for browser-backed work.
/// Saved into the running demo; engines rebuild (browser sessions
/// re-bootstrap, plain-HTTP state survives).
fn route_settings(state: &mut Demo, params: &[(String, String)]) -> Result<String, String> {
    if param(params, "set") == "1" {
        let endpoint = param(params, "cdp").trim().to_owned();
        state.cdp_url = if endpoint.is_empty() {
            None
        } else {
            Some(endpoint)
        };
        state.rebuild_all()?;
        state.persist()?;
    }
    let current = state.cdp_url.clone().unwrap_or_default();
    let status = match &state.cdp_url {
        Some(url) => format!("browser endpoint: {url}"),
        None => "browserless: sources take their no-browser paths".into(),
    };
    let mut body = format!("{}<h1>Settings</h1>", navbar(state, None));
    if param(params, "set") == "1" {
        body.push_str("<p>saved — engines rebuilt.</p>");
    }
    body.push_str(&format!("<p class=\"muted\">{}</p>", esc(&status)));
    body.push_str(&format!(
        "<form method=\"get\" action=\"/settings\"><input type=\"hidden\" name=\"set\" value=\"1\">\
        <label>CDP endpoint (empty = auto-detect a local browser, then browserless): \
        <input type=\"text\" name=\"cdp\" value=\"{}\" placeholder=\"http://127.0.0.1:9222\"></label> \
        <button type=\"submit\">save</button></form>",
        esc(&current)
    ));
    Ok(page("Settings", &body))
}

fn route_image(
    state: &mut Demo,
    params: &[(String, String)],
    stream: &mut std::net::TcpStream,
) -> Result<(), String> {
    let id = param(params, "source");
    let cover = params
        .iter()
        .find(|(key, _)| key == "cover")
        .map(|(_, value)| value.clone());
    let (url, manga, chapter, page, context) = if let Some(cover) = cover {
        let manga_key = param(params, "manga");
        let manga = state
            .updates
            .get(&format!("{id}\0{manga_key}"))
            .and_then(|update| update.get("manga"))
            .cloned()
            .unwrap_or(json!({"key": manga_key, "title": manga_key}));
        (
            cover.clone(),
            manga,
            Value::Null,
            json!({"imageUrl": cover}),
            "cover",
        )
    } else {
        let manga_key = param(params, "manga");
        let chapter_key = param(params, "chapter");
        let index: usize = param(params, "i")
            .parse()
            .map_err(|_| "bad image index".to_owned())?;
        let cached = state
            .pages
            .get(&format!("{id}\0{manga_key}\0{chapter_key}"))
            .ok_or_else(|| "open the chapter page first".to_owned())?;
        let page = cached
            .pages
            .get(index)
            .cloned()
            .ok_or_else(|| "image out of range".to_owned())?;
        let url = page
            .get("imageUrl")
            .or_else(|| page.get("url"))
            .and_then(Value::as_str)
            .ok_or_else(|| "page has no URL".to_owned())?
            .to_owned();
        (
            url,
            cached.manga.clone(),
            cached.chapter.clone(),
            page,
            "page",
        )
    };
    let source = state.source(&id)?;
    let request: wef_core::ImageRequest = if source.package.manifest().capabilities.image_requests {
        let mut request_input = Map::new();
        request_input.insert("manga".into(), manga);
        if !chapter.is_null() {
            request_input.insert("chapter".into(), chapter);
        }
        if !page.is_null() {
            request_input.insert("page".into(), page.clone());
        }
        request_input.insert("url".into(), Value::String(url.clone()));
        request_input.insert("context".into(), Value::String(context.into()));
        serde_json::from_value(
            source
                .engine
                .run_extension(
                    &source.package,
                    ExtensionOperation::GetImageRequest,
                    Value::Object(request_input),
                )
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("invalid image request: {error}"))?
    } else {
        // No image-request capability: readers fetch the URL directly.
        wef_core::ImageRequest {
            url: url.clone(),
            headers: None,
            candidates: None,
        }
    };
    let (bytes, content_type) = state.fetch_image(&request)?;
    let (bytes, content_type) = if source.package.manifest().capabilities.image_transforms {
        let page_value: wef_core::Page =
            serde_json::from_value(page).map_err(|error| format!("invalid page: {error}"))?;
        let mut headers = BTreeMap::new();
        for (name, value) in &content_type.headers {
            headers.insert(name.clone(), value.clone());
        }
        let output = source
            .engine
            .run_image_transform(
                &source.package,
                wef_engine_rs::ImageTransformInput {
                    request: request.clone(),
                    page: page_value,
                    status: content_type.status,
                    headers,
                    mime_type: Some(content_type.mime.clone()),
                    body: bytes,
                },
            )
            .map_err(|error| error.to_string())?;
        (output.body, output.mime_type)
    } else {
        (bytes, content_type.mime)
    };
    respond(stream, 200, &content_type, &bytes)
}

struct FetchedImage {
    status: u16,
    mime: String,
    headers: Vec<(String, String)>,
}

impl Demo {
    fn fetch_image(
        &self,
        request: &wef_core::ImageRequest,
    ) -> Result<(Vec<u8>, FetchedImage), String> {
        let mut attempts = vec![(request.url.clone(), request.headers.clone())];
        if let Some(candidates) = &request.candidates {
            for candidate in candidates {
                attempts.push((candidate.url.clone(), candidate.headers.clone()));
            }
        }
        let mut last_error = None;
        for (index, (url, headers)) in attempts.iter().enumerate() {
            match self.fetch_once(url, headers.as_ref()) {
                Ok(fetched) if fetched.status == 404 || fetched.status == 410 => {
                    if index + 1 == attempts.len() {
                        return Ok(fetched_bytes(fetched));
                    }
                    last_error = Some(format!("HTTP {}", fetched.status));
                }
                Ok(fetched) => return Ok(fetched_bytes(fetched)),
                Err(error) if index + 1 < attempts.len() => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| "all image candidates failed".into()))
    }

    fn fetch_once(
        &self,
        url: &str,
        headers: Option<&BTreeMap<String, String>>,
    ) -> Result<FetchedFull, String> {
        let mut builder = self.agent.get(url);
        if let Some(headers) = headers {
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
        }
        let mut response = builder.call().map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let mut response_headers = Vec::new();
        for (name, value) in response.headers().iter() {
            if let Ok(text) = value.to_str() {
                response_headers.push((name.as_str().to_ascii_lowercase(), text.to_owned()));
            }
        }
        let mime = response_headers
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| {
                value
                    .split(';')
                    .next()
                    .unwrap_or("application/octet-stream")
                    .trim()
                    .to_owned()
            })
            .unwrap_or_else(|| "application/octet-stream".into());
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_IMAGE_BYTES)
            .read_to_vec()
            .map_err(|error| error.to_string())?;
        Ok(FetchedFull {
            status,
            mime,
            headers: response_headers,
            body,
        })
    }
}

struct FetchedFull {
    status: u16,
    mime: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn fetched_bytes(fetched: FetchedFull) -> (Vec<u8>, FetchedImage) {
    (
        fetched.body,
        FetchedImage {
            status: fetched.status,
            mime: fetched.mime,
            headers: fetched.headers,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter_fixture() -> Vec<Value> {
        vec![
            json!({"type": "text", "id": "author", "name": "Author"}),
            json!({"type": "toggle", "id": "finished", "name": "Finished", "default": false}),
            json!({"type": "select", "id": "status", "name": "Status",
                "options": [{"id": "all", "name": "All"}, {"id": "ongoing", "name": "Ongoing"}],
                "default": "all"}),
            json!({"type": "multi-select", "id": "tags", "name": "Tags",
                "options": [{"id": "a", "name": "A"}, {"id": "b", "name": "B"}],
                "default": ["a"]}),
            json!({"type": "tri-state", "id": "genres", "name": "Genres",
                "options": [{"id": "x", "name": "X"}, {"id": "y", "name": "Y"}],
                "default": {"x": "include"}}),
            json!({"type": "range", "id": "chapters", "name": "Chapters",
                "min": 0, "max": 1000, "default": {"min": 10}}),
            json!({"type": "sort", "id": "order", "name": "Order",
                "options": [{"id": "title", "name": "Title"}],
                "default": {"value": "title", "direction": "desc"}}),
            json!({"type": "group", "id": "g", "name": "G",
                "children": [{"type": "text", "id": "nested", "name": "Nested"}]}),
        ]
    }

    #[test]
    fn filter_form_round_trips_every_kind() {
        let filters = filter_fixture();
        let params = vec![
            ("f_author".into(), "Smith".into()),
            ("f_finished".into(), "1".into()),
            ("f_status".into(), "ongoing".into()),
            ("f_tags".into(), "a".into()),
            ("f_tags".into(), "b".into()),
            ("f_genres__x".into(), "include".into()),
            ("f_genres__y".into(), "exclude".into()),
            ("f_chapters_min".into(), "10".into()),
            ("f_chapters_max".into(), "50".into()),
            ("f_order".into(), "title".into()),
            ("f_order_dir".into(), "asc".into()),
            ("f_nested".into(), "deep".into()),
        ];
        let values = parse_filter_form(&filters, &params);
        assert_eq!(values["author"], json!("Smith"));
        assert_eq!(values["finished"], json!(true));
        assert_eq!(values["status"], json!("ongoing"));
        assert_eq!(values["tags"], json!(["a", "b"]));
        assert_eq!(values["genres"], json!({"x": "include", "y": "exclude"}));
        assert_eq!(values["chapters"], json!({"min": 10.0, "max": 50.0}));
        assert_eq!(
            values["order"],
            json!({"value": "title", "direction": "asc"})
        );
        assert_eq!(values["nested"], json!("deep"));
    }

    #[test]
    fn filter_form_omits_empty_text_but_populates_defaults() {
        let filters = filter_fixture();
        let values = parse_filter_form(&filters, &[]);
        assert!(!values.contains_key("author"));
        assert_eq!(values["finished"], json!(false));
        assert_eq!(values["status"], json!("all"));
        assert_eq!(values["tags"], json!([]));
        assert_eq!(values["genres"], json!({"x": "include"}));
        assert_eq!(values["chapters"], json!({"min": 10.0}));
        assert_eq!(
            values["order"],
            json!({"value": "title", "direction": "desc"})
        );
    }

    #[test]
    fn escapes_markup_in_source_strings() {
        assert_eq!(esc("<b>&\""), "&lt;b&gt;&amp;&quot;");
    }

    #[test]
    fn decodes_query_escapes() {
        assert_eq!(pct_decode("a+b%20c%2F"), "a b c/");
        assert_eq!(pct_decode("solo%20leveling"), "solo leveling");
        let params = parse_query("q=solo+leveling&page=2&f_tags=a&f_tags=b");
        assert_eq!(param(&params, "q"), "solo leveling");
        assert_eq!(param(&params, "page"), "2");
    }

    #[test]
    fn tristate_adder_merges_and_validates_options() {
        let mut filters = filter_fixture();
        // Keep only the genres filter for focus.
        filters.retain(|filter| filter.get("id").and_then(Value::as_str) == Some("genres"));
        // Existing selection plus an added one.
        let params = vec![
            ("f_genres__x".into(), "include".into()),
            ("f_genres_add".into(), "y".into()),
            ("f_genres_mode".into(), "exclude".into()),
        ];
        let values = parse_filter_form(&filters, &params);
        assert_eq!(values["genres"], json!({"x": "include", "y": "exclude"}));
        // Unknown option ids are ignored, never invented (the declared
        // default still applies).
        let params = vec![("f_genres_add".into(), "zzz".into())];
        let values = parse_filter_form(&filters, &params);
        assert_eq!(values["genres"], json!({"x": "include"}));
    }

    #[test]
    fn tristate_renders_list_and_adder() {
        let mut filters = filter_fixture();
        filters.retain(|filter| filter.get("id").and_then(Value::as_str) == Some("genres"));
        let params = vec![
            ("id".into(), "en.wef.demo".into()),
            ("f_genres__x".into(), "include".into()),
        ];
        let (inside, after) = render_filter(&filters[0], &params);
        assert!(inside.contains("name=\"f_genres__x\""), "{inside}");
        assert!(after.contains("X (include)"), "{after}");
        assert!(after.contains("title=\"remove\">×</a>"), "{after}");
        assert!(after.contains("name=\"f_genres_add\""), "{after}");
        // The chosen option leaves the dropdown; the other remains.
        assert!(!after.contains("<option value=\"x\">"), "{after}");
        assert!(after.contains("<option value=\"y\">"), "{after}");
    }

    #[test]
    fn tristate_remove_keeps_other_selections() {
        let mut filters = filter_fixture();
        filters.retain(|filter| filter.get("id").and_then(Value::as_str) == Some("genres"));
        let params = vec![
            ("id".into(), "en.wef.demo".into()),
            ("listing".into(), "popular".into()),
            ("f_genres__x".into(), "include".into()),
            ("f_genres_add".into(), "y".into()),
            ("f_genres_mode".into(), "exclude".into()),
        ];
        let (_, after) = render_filter(&filters[0], &params);
        assert!(after.contains("X (include)"), "{after}");
        assert!(after.contains("Y (exclude)"), "{after}");
        // Removing X keeps Y (converted to a plain key); removing Y keeps X.
        assert!(
            after.contains(
                "<span class=\"pill\">X (include)<a href=\"/source?id=en.wef.demo&listing=popular&f_genres__y=exclude\""
            ),
            "{after}"
        );
        assert!(
            after.contains(
                "<span class=\"pill exc\">Y (exclude)<a href=\"/source?id=en.wef.demo&listing=popular&f_genres__x=include\""
            ),
            "{after}"
        );
    }
}
