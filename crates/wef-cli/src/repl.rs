//! Interactive source explorer.
//!
//! `wef repl` keeps loaded packages, settings, cookies, CDP sessions, and
//! source storage alive across commands so one manga/chapter chain can be
//! walked without re-passing files on every invocation.
//!
//! The browser is strictly pay-as-you-go: `cdp <url>` only stores the
//! endpoint (it is validated, never contacted). The first `ctx.browser.run`
//! in any operation attaches, prints a `[cdp]` notice to stderr, and reuses
//! the connection from then on. Plain HTTP-only flows never touch it —
//! search, listings, and warm-store operations run with no browser at all.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs,
    io::{self, BufReader, BufWriter, Write},
    path::PathBuf,
    rc::Rc,
};

use serde_json::{Map, Value, json};
use wef_engine_rs::{
    BrowserPolicy, BrowserRunRequest, BrowserRunResult, CdpBrowserHost, Engine, ExtensionOperation,
    HostError, HttpRequest, HttpResponse, Operation, Package, UreqHost, WefHost, wef_core,
};

use super::{json_argument, load_package, parse_update_options, pretty_json};

const HELP: &str = r#"commands:
  load <path>          load a source package and select it
  unload [id|index|path]  unload a source (default: selected)
  sources                list loaded sources (* = selected)
  use <id|index>         select the active source
  search <query> [--page N] [--filters <json|@file>]
  listing <id> [--page N] [--filters <json|@file>]
  update <manga> [--details-only|--chapters-only|--existing-chapters <json>]
  pages <manga> <chapter>
  filters                show the source's search filters
  resolve <url>          resolve a URL to manga/chapter keys
  validate [id]          validate a loaded package
  lint [id]              lint a loaded package
  test [id]              run a loaded package's fixtures
  settings [json|@file]  show or replace engine settings
  cdp <url>|off          configure the lazy browser endpoint (default off)
  session <path>|off     cookie-jar file for plain HTTP (default off)
  store <path>|off       source-store file, saved on exit and via `save`
  save                   persist the source store now
  help                   this text
  quit                   save state and exit (Ctrl-D works too)

<manga> and <chapter> are inline JSON, @file, or an index into the last
search/listing (`update 0`) or update (`pages 0 3`) output. Quotes group
arguments: search "solo leveling". `--cdp`/`--session` stay mutually
exclusive, like `wef run`: the browser profile owns cookies when CDP is on.
"#;

/// Entry point for `wef repl`. All interaction happens inside the read loop;
/// the returned string is always empty (main prints nothing for it).
pub fn repl(args: &[String]) -> Result<String, String> {
    let (session_path, args) = super::extract_session_option(args)?;
    let (settings, args) = super::extract_settings_option(&args)?;
    let (cdp_url, args) = super::extract_cdp_option(&args)?;
    let (store_path, args) = super::extract_store_option(&args)?;
    if !args.is_empty() {
        return Err(format!("repl takes no positional arguments\n\n{HELP}"));
    }
    if cdp_url.is_some() && session_path.is_some() {
        return Err(
            "--cdp uses the browser profile for cookies; do not combine it with --session".into(),
        );
    }

    let mut session = Session::new(settings);
    session.session_path = session_path;
    session.store_path = store_path;
    if let Some(url) = cdp_url {
        session.host.set_cdp_url(Some(url))?;
    }
    session.restore_all()?;

    println!("wef repl — type `help` for commands, `quit` to exit.");
    loop {
        print!("wef[{}]> ", session.selected_name());
        io::stdout()
            .flush()
            .map_err(|error| format!("could not write prompt: {error}"))?;
        let mut line = String::new();
        let bytes = io::stdin()
            .read_line(&mut line)
            .map_err(|error| format!("could not read input: {error}"))?;
        if bytes == 0 {
            break; // EOF (Ctrl-D).
        }
        match dispatch(&mut session, line.trim()) {
            Ok(Action::Output(text)) => {
                if !text.is_empty() {
                    print_out(&text);
                }
            }
            Ok(Action::Exit) => break,
            Err(error) => eprintln!("error: {error}"),
        }
    }
    session.persist_all();
    Ok(String::new())
}

/// Prints a result, exiting quietly when stdout is closed (e.g. piped to
/// `head`). The default `println!` panic would kill the session mid-run and
/// skip exit persistence; closed-pipe callers already have what they took.
fn print_out(text: &str) {
    use std::io::Write as _;
    let mut stdout = io::stdout().lock();
    if let Err(error) = writeln!(stdout, "{text}") {
        if error.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("error: could not write output: {error}");
    }
}

#[derive(Debug, PartialEq)]
enum Action {
    Output(String),
    Exit,
}

struct Loaded {
    id: String,
    path: String,
    package: Package,
}

struct Session {
    host: HybridHost,
    settings: Map<String, Value>,
    session_path: Option<PathBuf>,
    store_path: Option<PathBuf>,
    store: Value,
    loaded: Vec<Loaded>,
    selected: Option<usize>,
    last_manga: Vec<Value>,
    last_chapters: Vec<Value>,
}

impl Session {
    fn new(settings: Map<String, Value>) -> Self {
        Self {
            host: HybridHost::new(),
            settings,
            session_path: None,
            store_path: None,
            store: Value::Null,
            loaded: Vec::new(),
            selected: None,
            last_manga: Vec::new(),
            last_chapters: Vec::new(),
        }
    }

    fn selected_name(&self) -> &str {
        self.selected
            .and_then(|index| self.loaded.get(index))
            .map(|loaded| loaded.id.as_str())
            .unwrap_or("none")
    }

    fn active(&self) -> Result<&Loaded, String> {
        self.selected
            .and_then(|index| self.loaded.get(index))
            .ok_or_else(|| "no source selected; `load <path>` first".to_owned())
    }

    /// Loads cookie jar and source store files, mirroring `wef run`.
    fn restore_all(&mut self) -> Result<(), String> {
        if let Some(path) = &self.session_path
            && path.exists()
        {
            self.host.load_jar(path)?;
        }
        if let Some(path) = &self.store_path
            && path.exists()
        {
            let snapshot: Value =
                serde_json::from_reader(BufReader::new(fs::File::open(path).map_err(|error| {
                    format!("could not open source store {}: {error}", path.display())
                })?))
                .map_err(|error| format!("invalid source store {}: {error}", path.display()))?;
            self.store = snapshot;
        }
        Ok(())
    }

    /// Best-effort persistence on `save` and exit: failures print, never trap
    /// the user inside the loop.
    fn persist_all(&self) {
        if let Some(path) = &self.session_path
            && let Err(error) = self.host.save_jar(path)
        {
            eprintln!("error: {error}");
        }
        if let Some(path) = &self.store_path {
            let snapshot = serde_json::to_string_pretty(&self.store)
                .map_err(|error| format!("could not serialize source store: {error}"))
                .and_then(|text| {
                    fs::write(path, text).map_err(|error| {
                        format!("could not save source store {}: {error}", path.display())
                    })
                });
            if let Err(error) = snapshot {
                eprintln!("error: {error}");
            }
        }
    }

    fn run_core(&mut self, operation: Operation, input: Value) -> Result<Value, String> {
        self.with_engine(|engine, package| {
            engine
                .run(package, operation, input)
                .map_err(|error| error.to_string())
        })
    }

    fn run_extension(
        &mut self,
        operation: ExtensionOperation,
        input: Value,
    ) -> Result<Value, String> {
        self.with_engine(|engine, package| {
            engine
                .run_extension(package, operation, input)
                .map_err(|error| error.to_string())
        })
    }

    /// Builds a fresh engine around the shared host, carries the store
    /// snapshot across, and writes it back. Every command goes through here
    /// so cookies, sessions, and storage survive the whole session.
    fn with_engine(
        &mut self,
        run: impl FnOnce(&Engine, &Package) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let engine = Engine::with_host(self.host.clone()).with_settings(self.settings.clone());
        engine.restore_store(&self.store);
        let index = self
            .selected
            .filter(|index| *index < self.loaded.len())
            .ok_or_else(|| "no source selected; `load <path>` first".to_owned())?;
        let output = run(&engine, &self.loaded[index].package)?;
        self.store = engine.store_snapshot();
        Ok(output)
    }
}

/// Plain HTTP always, CDP only when a source actually calls `browser.run`.
///
/// Clones share one inner state (`Rc<RefCell<…>>`), so cookies, browser
/// sessions, and the lazily attached CDP host survive across REPL commands —
/// each command builds a fresh `Engine` around a cheap clone. Ports: the
/// same lazy-holder shape works wherever a host outlives one operation.
#[derive(Clone)]
struct HybridHost {
    inner: Rc<RefCell<HybridInner>>,
}

struct HybridInner {
    http: UreqHost,
    cdp_url: Option<String>,
    origins: BTreeSet<String>,
    cdp: Option<CdpBrowserHost>,
}

impl HybridHost {
    fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(HybridInner {
                http: UreqHost::default(),
                cdp_url: None,
                origins: BTreeSet::new(),
                cdp: None,
            })),
        }
    }

    /// Stores the endpoint after validating its shape. A dry construction
    /// only parses the URL — no socket is opened, so this never contacts a
    /// browser. Returns true when a live CDP host already exists.
    fn attached(&self) -> bool {
        self.inner.borrow().cdp.is_some()
    }

    fn cdp_url(&self) -> Option<String> {
        self.inner.borrow().cdp_url.clone()
    }

    fn set_cdp_url(&self, url: Option<String>) -> Result<(), String> {
        if let Some(url) = &url {
            let policy = BrowserPolicy::for_origins(Vec::new());
            CdpBrowserHost::new(url, policy).map_err(|error| error.to_string())?;
        }
        self.inner.borrow_mut().cdp_url = url;
        Ok(())
    }

    /// Grows the browser allowlist with newly loaded packages' base URLs and
    /// pushes them into an already-attached CDP host.
    fn add_origins(&self, origins: &[String]) {
        let mut inner = self.inner.borrow_mut();
        for origin in origins {
            inner.origins.insert(origin.clone());
            if let Some(cdp) = inner.cdp.as_mut() {
                cdp.add_allowed_origin(origin.clone());
            }
        }
    }

    fn ensure_cdp(&self) -> Result<(), HostError> {
        let mut inner = self.inner.borrow_mut();
        if inner.cdp.is_none() {
            let url = inner.cdp_url.clone().ok_or(HostError::Unsupported)?;
            let mut policy = BrowserPolicy::for_origins(inner.origins.iter().cloned());
            policy.consent_granted = true;
            let host = CdpBrowserHost::new(&url, policy)?;
            eprintln!("[cdp] attaching to {url} (first browser use)");
            inner.cdp = Some(host);
        }
        Ok(())
    }

    fn load_jar(&self, path: &PathBuf) -> Result<(), String> {
        let file = fs::File::open(path).map_err(|error| {
            format!("could not open cookie session {}: {error}", path.display())
        })?;
        self.inner
            .borrow()
            .http
            .load_cookie_jar_json(BufReader::new(file))
            .map_err(|error| error.to_string())
    }

    fn save_jar(&self, path: &PathBuf) -> Result<(), String> {
        let file = fs::File::create(path).map_err(|error| {
            format!("could not save cookie session {}: {error}", path.display())
        })?;
        self.inner
            .borrow()
            .http
            .save_cookie_jar_json(&mut BufWriter::new(file))
            .map_err(|error| error.to_string())
    }
}

impl WefHost for HybridHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        if request.browser_session.is_some() {
            // Session-authenticated requests belong to the browser profile.
            self.ensure_cdp()?;
            return self
                .inner
                .borrow_mut()
                .cdp
                .as_mut()
                .expect("CDP host ensured above")
                .request(request);
        }
        self.inner.borrow_mut().http.request(request)
    }

    fn set_rate_limit(&mut self, limit: Option<wef_core::RateLimit>) {
        self.inner.borrow_mut().http.set_rate_limit(limit.clone());
        if let Some(cdp) = self.inner.borrow_mut().cdp.as_mut() {
            cdp.set_rate_limit(limit);
        }
    }

    fn run_browser(&mut self, request: BrowserRunRequest) -> Result<BrowserRunResult, HostError> {
        // Without a configured endpoint the capability is simply absent and
        // sources take their no-browser path — same as plain `wef run`.
        self.ensure_cdp()?;
        self.inner
            .borrow_mut()
            .cdp
            .as_mut()
            .expect("CDP host ensured above")
            .run_browser(request)
    }
}

fn dispatch(session: &mut Session, line: &str) -> Result<Action, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(Action::Output(String::new()));
    }
    let (command, rest) = split_head(line);
    match command {
        "help" => Ok(Action::Output(HELP.into())),
        "quit" | "exit" | "q" => Ok(Action::Exit),
        "load" => cmd_load(session, rest),
        "unload" => cmd_unload(session, rest),
        "sources" => cmd_sources(session),
        "use" => cmd_use(session, rest),
        "search" => cmd_search(session, rest),
        "listing" => cmd_listing(session, rest),
        "update" => cmd_update(session, rest),
        "pages" => cmd_pages(session, rest),
        "filters" => cmd_filters(session),
        "resolve" => cmd_resolve(session, rest),
        "validate" => cmd_validate(session, rest),
        "lint" => cmd_lint(session, rest),
        "test" => cmd_test(session, rest),
        "settings" => cmd_settings(session, rest),
        "cdp" => cmd_cdp(session, rest),
        "session" => cmd_session(session, rest),
        "store" => cmd_store(session, rest),
        "save" => {
            session.persist_all();
            Ok(Action::Output("saved".into()))
        }
        _ => Err(format!("unknown command {command:?}; type `help`")),
    }
}

/// Splits the command word from its raw remainder so JSON arguments keep
/// their spacing for the balanced-brace scanner.
fn split_head(line: &str) -> (&str, &str) {
    match line.find(char::is_whitespace) {
        Some(index) => (&line[..index], line[index..].trim_start()),
        None => (line, ""),
    }
}

/// Splits one argument off the front: balanced `{…}`/`[…]` JSON, a quoted
/// string, or a bare token. Returns the argument and the remainder.
fn take_arg(rest: &str) -> Result<(String, &str), String> {
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Err("expected another argument".into());
    }
    let first = rest.chars().next().expect("non-empty checked above");
    if first == '{' || first == '[' {
        return take_balanced(rest);
    }
    if first == '"' || first == '\'' {
        // Span first (so a broken quote later in the line cannot fail this
        // argument), then reuse the line splitter for escape handling.
        let end = scan_quoted_end(rest).ok_or_else(|| "unfinished quote".to_owned())?;
        let token = split_line(&rest[..end])?
            .into_iter()
            .next()
            .unwrap_or_default();
        return Ok((token, rest[end..].trim_start()));
    }
    match rest.find(char::is_whitespace) {
        Some(index) => Ok((rest[..index].to_owned(), rest[index..].trim_start())),
        None => Ok((rest.to_owned(), "")),
    }
}

/// Byte offset just past the closing quote of a quoted prefix.
fn scan_quoted_end(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    let (_, quote) = chars.next()?;
    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return Some(index + ch.len_utf8());
        }
    }
    None
}

/// Scans balanced braces/brackets, respecting strings and escapes, so pasted
/// JSON survives as one argument.
fn take_balanced(rest: &str) -> Result<(String, &str), String> {
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for (index, ch) in rest.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_string = Some(ch),
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok((
                        rest[..index + ch.len_utf8()].to_owned(),
                        rest[index + ch.len_utf8()..].trim_start(),
                    ));
                }
            }
            _ => {}
        }
    }
    Err("unbalanced JSON argument".into())
}

/// Whitespace splitting with single/double quotes and backslash escapes.
fn split_line(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else if ch == '\\' {
                match chars.next() {
                    Some(escaped) => current.push(escaped),
                    None => return Err("unfinished escape".into()),
                }
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
            in_token = true;
        } else if ch.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut current));
                in_token = false;
            }
        } else {
            current.push(ch);
            in_token = true;
        }
    }
    if quote.is_some() {
        return Err("unfinished quote".into());
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Resolves a manga/chapter argument: `@file`, inline JSON, or an index into
/// a previous result list.
fn resolve_reference(arg: &str, slot: &[Value], label: &str) -> Result<Value, String> {
    if arg.starts_with('@') {
        return json_argument(arg, label);
    }
    if arg.starts_with('{') || arg.starts_with('[') {
        return serde_json::from_str(arg).map_err(|error| format!("invalid {label} JSON: {error}"));
    }
    if let Ok(index) = arg.parse::<usize>() {
        return slot
            .get(index)
            .cloned()
            .ok_or_else(|| format!("{label} index {index} is out of range (0..{})", slot.len()));
    }
    Err(format!(
        "expected {label} as inline JSON, @file, or a result index (0..{})",
        slot.len()
    ))
}

/// Splits trailing `--page N` and `--filters <json|@file>` flags off a raw
/// query/id remainder. The filters argument is pulled with the balanced
/// scanner first (inline JSON may contain spaces), then the page flag.
fn split_page_and_filters(rest: &str) -> Result<(String, u32, Map<String, Value>), String> {
    let (without_filters, filters) = split_filters(rest)?;
    let (query, page) = split_page(&without_filters)?;
    Ok((query, page, filters))
}

/// Pulls one `--filters <arg>` pair out of the raw remainder, returning the
/// remainder without it plus the parsed object (empty when absent).
fn split_filters(rest: &str) -> Result<(String, Map<String, Value>), String> {
    let marker = "--filters";
    let Some(start) = rest.find(marker) else {
        return Ok((rest.to_owned(), Map::new()));
    };
    let before = rest[..start].trim_end().to_owned();
    let after = rest[start + marker.len()..].trim_start().to_owned();
    if after.is_empty() {
        return Err("--filters requires a JSON object or @file".into());
    }
    let (arg, remaining) = take_arg(&after)?;
    let value = if arg.starts_with('@') {
        json_argument(&arg, "filters")?
    } else {
        serde_json::from_str(&arg).map_err(|error| format!("invalid filters JSON: {error}"))?
    };
    let filters = value
        .as_object()
        .cloned()
        .ok_or_else(|| "--filters must be a JSON object".to_owned())?;
    let cleaned = format!("{before} {remaining}").trim().to_owned();
    Ok((cleaned, filters))
}

/// Splits a trailing `--page N` off a raw query/id remainder.
fn split_page(rest: &str) -> Result<(String, u32), String> {
    let tokens = split_line(rest)?;
    if tokens.len() >= 2 && tokens[tokens.len() - 2] == "--page" {
        let page = tokens[tokens.len() - 1]
            .parse::<u32>()
            .map_err(|_| format!("invalid page number {:?}", tokens[tokens.len() - 1]))?;
        if page == 0 {
            return Err("page must start at 1".into());
        }
        return Ok((tokens[..tokens.len() - 2].join(" "), page));
    }
    Ok((rest.trim().to_owned(), 1))
}

fn find_source(session: &Session, reference: &str) -> Option<usize> {
    if let Ok(index) = reference.parse::<usize>() {
        return session.loaded.get(index).map(|_| index);
    }
    session
        .loaded
        .iter()
        .position(|loaded| loaded.id == reference || loaded.path == reference)
}

fn cmd_load(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: load <path>".into());
    }
    let package = load_package(rest)?;
    let id = package.manifest().id.clone();
    session.host.add_origins(&package.manifest().base_urls);
    if let Some(index) = session.loaded.iter().position(|loaded| loaded.id == id) {
        session.loaded[index] = Loaded {
            id: id.clone(),
            path: rest.to_owned(),
            package,
        };
        session.selected = Some(index);
        return Ok(Action::Output(format!("reloaded {id} (already loaded)")));
    }
    session.loaded.push(Loaded {
        id: id.clone(),
        path: rest.to_owned(),
        package,
    });
    session.selected = Some(session.loaded.len() - 1);
    let manifest = &session.loaded[session.loaded.len() - 1].package.manifest();
    Ok(Action::Output(format!(
        "loaded {} — {} (requires: {})",
        id,
        manifest.name,
        manifest
            .requires
            .iter()
            .map(|capability| format!("{capability:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

fn cmd_unload(session: &mut Session, rest: &str) -> Result<Action, String> {
    let index = if rest.is_empty() {
        session
            .selected
            .ok_or_else(|| "nothing loaded".to_owned())?
    } else {
        find_source(session, rest).ok_or_else(|| format!("no loaded source matches {rest:?}"))?
    };
    let removed = session.loaded.remove(index);
    session.selected = match session.selected {
        Some(selected) if selected == index => {
            if session.loaded.is_empty() {
                None
            } else {
                Some(index.min(session.loaded.len() - 1))
            }
        }
        Some(selected) if selected > index => Some(selected - 1),
        other => other,
    };
    Ok(Action::Output(format!("unloaded {}", removed.id)))
}

fn cmd_sources(session: &mut Session) -> Result<Action, String> {
    if session.loaded.is_empty() {
        return Ok(Action::Output("no sources loaded".into()));
    }
    let lines = session
        .loaded
        .iter()
        .enumerate()
        .map(|(index, loaded)| {
            let marker = if Some(index) == session.selected {
                "*"
            } else {
                " "
            };
            format!(
                "{marker} {index} {} — {} ({})",
                loaded.id,
                loaded.package.manifest().name,
                loaded.path
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Action::Output(lines))
}

fn cmd_use(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: use <id|index>".into());
    }
    let index =
        find_source(session, rest).ok_or_else(|| format!("no loaded source matches {rest:?}"))?;
    session.selected = Some(index);
    Ok(Action::Output(format!(
        "selected {}",
        session.loaded[index].id
    )))
}

fn cmd_search(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: search <query> [--page N] [--filters <json|@file>]".into());
    }
    let (query, page, filters) = split_page_and_filters(rest)?;
    let output = session.run_core(
        Operation::Search,
        json!({"query": query, "page": page, "filters": filters}),
    )?;
    let items = output
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let next = output.get("hasNextPage").cloned().unwrap_or(Value::Null);
    session.last_manga = items.clone();
    Ok(Action::Output(format!(
        "{} item(s), hasNextPage: {next}\n{}",
        items.len(),
        pretty_json(&output)?
    )))
}

fn cmd_listing(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: listing <id> [--page N] [--filters <json|@file>]".into());
    }
    let (id, page, filters) = split_page_and_filters(rest)?;
    let output = session.run_core(
        Operation::GetMangaList,
        json!({"listingId": id, "page": page, "filters": filters}),
    )?;
    let items = output
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let next = output.get("hasNextPage").cloned().unwrap_or(Value::Null);
    session.last_manga = items.clone();
    Ok(Action::Output(format!(
        "{} item(s), hasNextPage: {next}\n{}",
        items.len(),
        pretty_json(&output)?
    )))
}

fn cmd_update(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: update <manga-json|@file|index> [options]".into());
    }
    let (manga_arg, options) = take_arg(rest)?;
    let manga = resolve_reference(&manga_arg, &session.last_manga, "manga")?;
    let tokens = split_line(options)?;
    let (chapters, fetch_details, fetch_chapters) = parse_update_options(&tokens)?;
    let output = session.run_core(
        Operation::GetMangaUpdate,
        json!({
            "manga": manga,
            "chapters": chapters,
            "fetchDetails": fetch_details,
            "fetchChapters": fetch_chapters,
        }),
    )?;
    let chapters = output
        .get("chapters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    session.last_chapters = chapters.clone();
    if let Some(manga) = output.get("manga") {
        session.last_manga = vec![manga.clone()];
    }
    let details = if output.get("manga").is_some() {
        "details + "
    } else {
        ""
    };
    Ok(Action::Output(format!(
        "{details}{} chapter(s)\n{}",
        chapters.len(),
        pretty_json(&output)?
    )))
}

fn cmd_pages(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: pages <manga-json|@file|index> <chapter-json|@file|index>".into());
    }
    let (manga_arg, rest) = take_arg(rest)?;
    let (chapter_arg, extra) = take_arg(rest)?;
    if !extra.is_empty() {
        return Err("pages accepts only manga and chapter arguments".into());
    }
    let manga = resolve_reference(&manga_arg, &session.last_manga, "manga")?;
    let chapter = resolve_reference(&chapter_arg, &session.last_chapters, "chapter")?;
    let output = session.run_core(
        Operation::GetPages,
        json!({"manga": manga, "chapter": chapter}),
    )?;
    let pages = output
        .as_array()
        .map(Vec::len)
        .or_else(|| output.get("pages").and_then(Value::as_array).map(Vec::len))
        .unwrap_or(0);
    // GetPages returns a bare page array (unlike the `{items}` envelopes).
    Ok(Action::Output(format!(
        "{pages} page(s)\n{}",
        pretty_json(&output)?
    )))
}

fn cmd_filters(session: &mut Session) -> Result<Action, String> {
    let output = session.run_extension(ExtensionOperation::GetFilters, json!({}))?;
    pretty_json(&output).map(Action::Output)
}

fn cmd_resolve(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return Err("usage: resolve <url>".into());
    }
    let output =
        session.run_extension(ExtensionOperation::ResolveUrl, json!({"url": rest.trim()}))?;
    pretty_json(&output).map(Action::Output)
}

fn package_reference(session: &Session, rest: &str) -> Result<String, String> {
    if rest.is_empty() {
        return session
            .active()
            .map(|loaded| loaded.path.clone())
            .map_err(|_| "usage: <command> [id]".to_owned());
    }
    let index =
        find_source(session, rest).ok_or_else(|| format!("no loaded source matches {rest:?}"))?;
    Ok(session.loaded[index].path.clone())
}

fn cmd_validate(session: &mut Session, rest: &str) -> Result<Action, String> {
    let path = package_reference(session, rest)?;
    super::validate(&[path]).map(Action::Output)
}

fn cmd_lint(session: &mut Session, rest: &str) -> Result<Action, String> {
    let path = package_reference(session, rest)?;
    super::lint(&[path]).map(Action::Output)
}

fn cmd_test(session: &mut Session, rest: &str) -> Result<Action, String> {
    let path = package_reference(session, rest)?;
    super::test(&[path]).map(Action::Output)
}

fn cmd_settings(session: &mut Session, rest: &str) -> Result<Action, String> {
    if rest.is_empty() {
        return pretty_json(&Value::Object(session.settings.clone())).map(Action::Output);
    }
    let value = json_argument(rest, "settings")?;
    session.settings = value
        .as_object()
        .cloned()
        .ok_or_else(|| "settings must be a JSON object".to_owned())?;
    Ok(Action::Output(format!(
        "settings updated ({} key(s))",
        session.settings.len()
    )))
}

fn cmd_cdp(session: &mut Session, rest: &str) -> Result<Action, String> {
    match rest {
        "" => Ok(Action::Output(format!(
            "cdp: {} (attached: {})",
            session
                .host
                .cdp_url()
                .map(|url| format!("{url} (lazy — unused until a browser run)"))
                .unwrap_or_else(|| "off".into()),
            session.host.attached()
        ))),
        "off" => {
            session.host.set_cdp_url(None)?;
            Ok(Action::Output("cdp off".into()))
        }
        url => {
            if session.session_path.is_some() {
                return Err(
                    "--cdp uses the browser profile for cookies; `session off` first".into(),
                );
            }
            session.host.set_cdp_url(Some(url.to_owned()))?;
            Ok(Action::Output(
                "cdp endpoint set (attaches on first browser use)".into(),
            ))
        }
    }
}

fn cmd_session(session: &mut Session, rest: &str) -> Result<Action, String> {
    match rest {
        "" => Ok(Action::Output(format!(
            "session: {}",
            session
                .session_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "off".into())
        ))),
        "off" => {
            session.session_path = None;
            Ok(Action::Output("session off".into()))
        }
        path => {
            if session.host.cdp_url().is_some() {
                return Err("a CDP endpoint is set; `cdp off` first".into());
            }
            let path = PathBuf::from(path);
            if path.exists() {
                session.host.load_jar(&path)?;
            }
            session.session_path = Some(path);
            Ok(Action::Output("session updated".into()))
        }
    }
}

fn cmd_store(session: &mut Session, rest: &str) -> Result<Action, String> {
    match rest {
        "" => Ok(Action::Output(format!(
            "store: {}",
            session
                .store_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "off (memory only)".into())
        ))),
        "off" => {
            session.store_path = None;
            Ok(Action::Output("store off (memory only)".into()))
        }
        path => {
            let path = PathBuf::from(path);
            if path.exists() {
                let snapshot: Value =
                    serde_json::from_reader(BufReader::new(fs::File::open(&path).map_err(
                        |error| format!("could not open source store {}: {error}", path.display()),
                    )?))
                    .map_err(|error| format!("invalid source store {}: {error}", path.display()))?;
                session.store = snapshot;
            }
            session.store_path = Some(path);
            Ok(Action::Output("store updated".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_tokens() {
        assert_eq!(
            split_line(r#"search "solo leveling""#).unwrap(),
            ["search", "solo leveling"]
        );
        assert_eq!(split_line("use  'a b'  ").unwrap(), ["use", "a b"]);
        assert!(split_line(r#"search "oops"#).is_err());
    }

    #[test]
    fn takes_balanced_json_with_braces_in_strings() {
        let (arg, rest) =
            take_arg(r#"{"a": "{not a brace}", "b": [1, 2]} --chapters-only"#).unwrap();
        assert_eq!(arg, r#"{"a": "{not a brace}", "b": [1, 2]}"#);
        assert_eq!(rest, "--chapters-only");
    }

    #[test]
    fn takes_bare_and_quoted_args() {
        let (arg, rest) = take_arg("0 --details-only").unwrap();
        assert_eq!((arg.as_str(), rest), ("0", "--details-only"));
        let (arg, rest) = take_arg("'a b' c").unwrap();
        assert_eq!((arg.as_str(), rest), ("a b", "c"));
    }

    #[test]
    fn resolves_indexes_against_result_slots() {
        let slot = vec![json!({"key": "a"}), json!({"key": "b"})];
        assert_eq!(
            resolve_reference("1", &slot, "manga").unwrap(),
            json!({"key": "b"})
        );
        assert!(resolve_reference("5", &slot, "manga").is_err());
        assert!(resolve_reference("nope", &slot, "manga").is_err());
    }

    #[test]
    fn splits_trailing_page_flags() {
        assert_eq!(split_page("latest --page 3").unwrap(), ("latest".into(), 3));
        assert_eq!(
            split_page("solo leveling").unwrap(),
            ("solo leveling".into(), 1)
        );
        assert!(split_page("x --page 0").is_err());
    }

    #[test]
    fn hybrid_host_stays_lazy_without_browser_use() {
        let host = HybridHost::new();
        host.set_cdp_url(Some("http://127.0.0.1:9".into())).unwrap();
        assert!(!host.attached());
        let mut host = host;
        let error = host
            .run_browser(BrowserRunRequest {
                url: "https://example.com".into(),
                html: None,
                timeout_ms: None,
                task: wef_engine_rs::BrowserTask::Snapshot(wef_engine_rs::BrowserSnapshotTask {
                    selector: "x".into(),
                }),
            })
            .unwrap_err();
        // Attaching attempted (no listener on port 9) — the point is the
        // endpoint was untouched until this call.
        assert!(matches!(error, HostError::Message(_)));
        assert!(host.attached());
    }

    #[test]
    fn hybrid_host_reports_unsupported_without_an_endpoint() {
        let mut host = HybridHost::new();
        let error = host
            .run_browser(BrowserRunRequest {
                url: "https://example.com".into(),
                html: None,
                timeout_ms: None,
                task: wef_engine_rs::BrowserTask::Snapshot(wef_engine_rs::BrowserSnapshotTask {
                    selector: "x".into(),
                }),
            })
            .unwrap_err();
        assert!(matches!(error, HostError::Unsupported));
    }

    #[test]
    fn hybrid_host_rejects_bad_endpoints_eagerly() {
        let host = HybridHost::new();
        assert!(
            host.set_cdp_url(Some("https://example.com:9222".into()))
                .is_err()
        );
        assert!(
            host.set_cdp_url(Some("http://127.0.0.1:9222".into()))
                .is_ok()
        );
    }

    #[test]
    fn repl_rejects_combined_cdp_and_session() {
        let error = repl(&[
            "--cdp".to_owned(),
            "http://127.0.0.1:9222".to_owned(),
            "--session".to_owned(),
            "cookies.json".to_owned(),
        ])
        .unwrap_err();
        assert!(error.contains("do not combine"));
    }

    #[test]
    fn missing_host_capability_names_availability() {
        // Documents the contract lazy CDP relies on: without an endpoint the
        // browser capability degrades to Unsupported, never to a connection.
        assert_eq!(
            HostError::Unsupported.to_string(),
            "host capability is unavailable"
        );
    }

    #[test]
    fn dispatch_rejects_unknown_commands_and_guards_empty_state() {
        let mut session = Session::new(Map::new());
        assert!(dispatch(&mut session, "bogus").is_err());
        assert!(dispatch(&mut session, "").unwrap() == Action::Output(String::new()));
        assert!(dispatch(&mut session, "search x").is_err());
        assert!(matches!(
            dispatch(&mut session, "quit").unwrap(),
            Action::Exit
        ));
    }
}
