//! Command-line runner for WEF source packages.

use std::{
    cell::RefCell,
    collections::VecDeque,
    fs,
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
    rc::Rc,
};

use serde::Deserialize;
use serde_json::{Value, json};
use wef_engine_rs::{
    BrowserPolicy, CdpBrowserHost, Engine, ExtensionOperation, HostError, HttpRequest,
    HttpResponse, Operation, Package, UreqHost, WefHost,
};

const USAGE: &str = r#"WEF reference CLI

Usage:
  wef validate <path>
  wef lint <path> [--json]
  wef run [--session <cookie-jar.json>] [--settings <json|@file>] [--cdp <local-url>] <path> listing <id> [--page <number>]
  wef run [--session <cookie-jar.json>] [--settings <json|@file>] [--cdp <local-url>] <path> search <query> [--page <number>]
  wef run [--session <cookie-jar.json>] [--settings <json|@file>] [--cdp <local-url>] <path> update <manga-json|@file> [--existing-chapters <json|@file>] [--details-only|--chapters-only]
  wef run [--session <cookie-jar.json>] [--settings <json|@file>] [--cdp <local-url>] <path> pages <manga-json|@file> <chapter-json|@file>
  wef test <path> [--json]

`run` performs real HTTP requests. `--cdp` attaches to an already-running local Chromium
browser and requires no `--session`; its browser profile retains cookies. `--session` persists only persistent cookies
in the named JSON file; treat that file as sensitive. `test` runs every `fixtures/*.json` file
with a deterministic mock HTTP host. A fixture contains `operation`, `input`,
`http` request/response steps, and `expected` output.
"#;

/// Runs a CLI command and returns text suitable for stdout.
pub fn run_with_args<I, S>(args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(USAGE.into());
    };

    match command {
        "help" | "--help" | "-h" => Ok(USAGE.into()),
        "validate" => validate(&args[1..]),
        "lint" => lint(&args[1..]),
        "run" => run(&args[1..]),
        "test" => test(&args[1..]),
        _ => Err(format!("unknown command {command:?}\n\n{USAGE}")),
    }
}

fn validate(args: &[String]) -> Result<String, String> {
    let (path, json_output) = path_and_json_flag(args, "validate <path> [--json]")?;
    let package = load_package(path)?;
    let manifest = serde_json::to_string_pretty(package.manifest())
        .map_err(|error| format!("could not serialize manifest: {error}"))?;
    if json_output {
        return pretty_json(
            &json!({"valid": true, "path": package.root(), "manifest": package.manifest()}),
        );
    }
    Ok(format!(
        "valid package: {}\n{manifest}",
        package.root().display()
    ))
}

fn lint(args: &[String]) -> Result<String, String> {
    let (path, json_output) = path_and_json_flag(args, "lint <path> [--json]")?;
    let diagnostics = wef_lint::lint_package(path);
    let errors = diagnostics
        .iter()
        .filter(|diagnostic| matches!(diagnostic.severity, wef_lint::Severity::Error))
        .count();
    if json_output {
        return pretty_json(&json!({"valid": errors == 0, "diagnostics": diagnostics}));
    }
    if diagnostics.is_empty() {
        return Ok("no lint diagnostics".into());
    }
    let output = diagnostics
        .iter()
        .map(|diagnostic| {
            let location = match (diagnostic.line, diagnostic.column) {
                (Some(line), Some(column)) => {
                    format!("{}:{line}:{column}", diagnostic.path.display())
                }
                _ => diagnostic.path.display().to_string(),
            };
            format!(
                "{location}: {:?}[{}]: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if errors == 0 { Ok(output) } else { Err(output) }
}

fn run(args: &[String]) -> Result<String, String> {
    let (session_path, args) = extract_session_option(args)?;
    let (settings, args) = extract_settings_option(&args)?;
    let (cdp_url, args) = extract_cdp_option(&args)?;
    if args.len() < 2 {
        return Err(format!("expected a run command\n\n{USAGE}"));
    }

    let package = load_package(&args[0])?;
    let command = args[1].as_str();
    let (operation, input) = match command {
        "listing" => {
            let id = args
                .get(2)
                .ok_or_else(|| "expected listing id".to_owned())?;
            let page = parse_page(&args[3..])?;
            (
                Operation::GetMangaList,
                json!({"listingId": id, "page": page}),
            )
        }
        "search" => {
            let query = args
                .get(2)
                .ok_or_else(|| "expected search query".to_owned())?;
            let page = parse_page(&args[3..])?;
            (
                Operation::Search,
                json!({"query": query, "page": page, "filters": {}}),
            )
        }
        "update" => {
            let manga = json_argument(
                args.get(2)
                    .ok_or_else(|| "expected manga JSON".to_owned())?,
                "manga",
            )?;
            let (chapters, fetch_details, fetch_chapters) = parse_update_options(&args[3..])?;
            (
                Operation::GetMangaUpdate,
                json!({
                    "manga": manga,
                    "chapters": chapters,
                    "fetchDetails": fetch_details,
                    "fetchChapters": fetch_chapters,
                }),
            )
        }
        "pages" => {
            let manga = json_argument(
                args.get(2)
                    .ok_or_else(|| "expected manga JSON".to_owned())?,
                "manga",
            )?;
            let chapter = json_argument(
                args.get(3)
                    .ok_or_else(|| "expected chapter JSON".to_owned())?,
                "chapter",
            )?;
            if args.len() > 4 {
                return Err("pages accepts only manga and chapter JSON arguments".into());
            }
            (
                Operation::GetPages,
                json!({"manga": manga, "chapter": chapter}),
            )
        }
        _ => return Err(format!("unknown run command {command:?}\n\n{USAGE}")),
    };

    if cdp_url.is_some() && session_path.is_some() {
        return Err(
            "--cdp uses the browser profile for cookies; do not combine it with --session".into(),
        );
    }
    if let Some(cdp_url) = cdp_url {
        let mut policy = BrowserPolicy::for_origins(["https://comix.to".into()])
            .scoped(package.manifest().id.clone(), "default");
        policy.consent_granted = true;
        return Engine::with_host(
            CdpBrowserHost::new(&cdp_url, policy).map_err(|error| error.to_string())?,
        )
        .with_settings(settings)
        .run(&package, operation, input)
        .map_err(|error| error.to_string())
        .and_then(|output| pretty_json(&output));
    }

    let host = UreqHost::default();
    if let Some(session_path) = &session_path
        && session_path.exists()
    {
        let file = fs::File::open(session_path).map_err(|error| {
            format!(
                "could not open cookie session {}: {error}",
                session_path.display()
            )
        })?;
        host.load_cookie_jar_json(BufReader::new(file))
            .map_err(|error| error.to_string())?;
    }

    let result = Engine::with_host(host.clone())
        .with_settings(settings)
        .run(&package, operation, input);
    if let Some(session_path) = &session_path {
        let file = fs::File::create(session_path).map_err(|error| {
            format!(
                "could not save cookie session {}: {error}",
                session_path.display()
            )
        })?;
        host.save_cookie_jar_json(&mut BufWriter::new(file))
            .map_err(|error| error.to_string())?;
    }
    let output = result.map_err(|error| error.to_string())?;
    pretty_json(&output)
}

fn extract_cdp_option(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut cdp_url = None;
    let mut remaining = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--cdp" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| "--cdp requires a local URL".to_owned())?;
            if cdp_url.replace(value.clone()).is_some() {
                return Err("--cdp may be specified only once".into());
            }
            index += 2;
        } else {
            remaining.push(args[index].clone());
            index += 1;
        }
    }
    Ok((cdp_url, remaining))
}

fn extract_settings_option(
    args: &[String],
) -> Result<(serde_json::Map<String, Value>, Vec<String>), String> {
    let mut settings = serde_json::Map::new();
    let mut seen = false;
    let mut remaining = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--settings" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| "--settings requires an object JSON value or @file".to_owned())?;
            if seen {
                return Err("--settings may be specified only once".into());
            }
            settings = json_argument(value, "settings")?
                .as_object()
                .cloned()
                .ok_or_else(|| "--settings must be a JSON object".to_owned())?;
            seen = true;
            index += 2;
        } else {
            remaining.push(args[index].clone());
            index += 1;
        }
    }
    Ok((settings, remaining))
}

fn extract_session_option(args: &[String]) -> Result<(Option<PathBuf>, Vec<String>), String> {
    let mut session_path = None;
    let mut remaining = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--session" {
            let path = args
                .get(index + 1)
                .ok_or_else(|| "--session requires a cookie-jar path".to_owned())?;
            if session_path.replace(PathBuf::from(path)).is_some() {
                return Err("--session may be specified only once".into());
            }
            index += 2;
        } else {
            remaining.push(args[index].clone());
            index += 1;
        }
    }
    Ok((session_path, remaining))
}

fn test(args: &[String]) -> Result<String, String> {
    let (path, json_output) = path_and_json_flag(args, "test <path> [--json]")?;
    let package = load_package(path)?;
    let fixtures = fixture_files(package.root())?;
    if fixtures.is_empty() {
        return if json_output {
            pretty_json(&json!({"passed": 0, "path": package.root()}))
        } else {
            Ok(format!("no fixtures found in {}", package.root().display()))
        };
    }

    for fixture_path in &fixtures {
        run_fixture(&package, fixture_path)?;
    }
    if json_output {
        pretty_json(&json!({"passed": fixtures.len(), "path": package.root()}))
    } else {
        Ok(format!("{} fixture(s) passed", fixtures.len()))
    }
}

fn path_and_json_flag<'a>(args: &'a [String], usage: &str) -> Result<(&'a str, bool), String> {
    match args {
        [path] => Ok((path, false)),
        [path, flag] if flag == "--json" => Ok((path, true)),
        _ => Err(format!("usage: wef {usage}")),
    }
}

fn load_package(path: &str) -> Result<Package, String> {
    Package::load(path).map_err(|error| error.to_string())
}

fn parse_page(args: &[String]) -> Result<u32, String> {
    if args.is_empty() {
        return Ok(1);
    }
    if args.len() != 2 || args[0] != "--page" {
        return Err("expected optional --page <number>".into());
    }
    let page = args[1]
        .parse::<u32>()
        .map_err(|_| format!("invalid page number {:?}", args[1]))?;
    if page == 0 {
        return Err("page must start at 1".into());
    }
    Ok(page)
}

fn parse_update_options(args: &[String]) -> Result<(Value, bool, bool), String> {
    let mut chapters = Value::Array(Vec::new());
    let mut fetch_details = true;
    let mut fetch_chapters = true;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--details-only" => {
                fetch_details = true;
                fetch_chapters = false;
                index += 1;
            }
            "--chapters-only" => {
                fetch_details = false;
                fetch_chapters = true;
                index += 1;
            }
            "--existing-chapters" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--existing-chapters requires JSON".to_owned())?;
                chapters = json_argument(value, "existing chapters")?;
                if !chapters.is_array() {
                    return Err("--existing-chapters must be a JSON array".into());
                }
                index += 2;
            }
            option => return Err(format!("unknown update option {option:?}")),
        }
    }

    Ok((chapters, fetch_details, fetch_chapters))
}

fn json_argument(value: &str, label: &str) -> Result<Value, String> {
    let source = match value.strip_prefix('@') {
        Some(path) => fs::read_to_string(path)
            .map_err(|error| format!("could not read {label} JSON file {path:?}: {error}"))?,
        None => value.to_owned(),
    };
    serde_json::from_str(&source).map_err(|error| format!("invalid {label} JSON: {error}"))
}

fn pretty_json(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map_err(|error| format!("could not serialize output: {error}"))
}

fn fixture_files(package_root: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = package_root.join("fixtures");
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut paths = fs::read_dir(&directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not read fixture entry: {error}"))?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn run_fixture(package: &Package, path: &Path) -> Result<(), String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("could not read fixture {}: {error}", path.display()))?;
    let fixture: Fixture = serde_json::from_str(&source)
        .map_err(|error| format!("invalid fixture {}: {error}", path.display()))?;
    let Fixture {
        name,
        operation,
        input,
        http,
        expected,
    } = fixture;
    let remaining_steps = Rc::new(RefCell::new(VecDeque::from(http)));
    let host = FixtureHost {
        steps: Rc::clone(&remaining_steps),
    };
    let engine = Engine::with_host(host);
    let actual = match operation.core() {
        Some(operation) => engine.run(package, operation, input),
        None => engine.run_extension(package, operation.extension(), input),
    }
    .map_err(|error| format!("fixture {name} failed: {error}"))?;

    if actual != expected {
        return Err(format!(
            "fixture {} output differs\nexpected:\n{}\nactual:\n{}",
            name,
            pretty_json(&expected)?,
            pretty_json(&actual)?,
        ));
    }
    if !remaining_steps.borrow().is_empty() {
        return Err(format!(
            "fixture {} did not consume every expected HTTP step",
            name
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    name: String,
    operation: FixtureOperation,
    input: Value,
    #[serde(default)]
    http: Vec<HttpFixture>,
    expected: Value,
}

#[derive(Debug, Deserialize)]
struct HttpFixture {
    request: HttpRequest,
    response: HttpResponse,
}

#[derive(Debug)]
struct FixtureHost {
    steps: Rc<RefCell<VecDeque<HttpFixture>>>,
}

impl WefHost for FixtureHost {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        let step =
            self.steps.borrow_mut().pop_front().ok_or_else(|| {
                HostError::Message(format!("unexpected HTTP request: {request:?}"))
            })?;
        if request != step.request {
            return Err(HostError::Message(format!(
                "HTTP request differs\nexpected: {:?}\nactual: {request:?}",
                step.request
            )));
        }
        Ok(step.response)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FixtureOperation {
    GetMangaList,
    Search,
    GetMangaUpdate,
    GetPages,
    GetSettings,
    GetFilters,
    GetImageRequest,
}

impl FixtureOperation {
    fn core(&self) -> Option<Operation> {
        match self {
            Self::GetMangaList => Some(Operation::GetMangaList),
            Self::Search => Some(Operation::Search),
            Self::GetMangaUpdate => Some(Operation::GetMangaUpdate),
            Self::GetPages => Some(Operation::GetPages),
            _ => None,
        }
    }

    fn extension(&self) -> ExtensionOperation {
        match self {
            Self::GetSettings => ExtensionOperation::GetSettings,
            Self::GetFilters => ExtensionOperation::GetFilters,
            Self::GetImageRequest => ExtensionOperation::GetImageRequest,
            _ => unreachable!("core operation has no extension equivalent"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_help_without_a_command() {
        let output = run_with_args(Vec::<String>::new()).unwrap();
        assert!(output.contains("wef validate <path>"));
    }

    #[test]
    fn rejects_page_zero() {
        assert_eq!(
            parse_page(&["--page".into(), "0".into()]).unwrap_err(),
            "page must start at 1"
        );
    }

    #[test]
    fn extracts_an_explicit_cookie_session_path() {
        let (session, remaining) = extract_session_option(&[
            "--session".into(),
            "cookies.json".into(),
            "source/mangadex".into(),
            "listing".into(),
            "latest".into(),
        ])
        .unwrap();
        assert_eq!(session, Some(PathBuf::from("cookies.json")));
        assert_eq!(remaining, ["source/mangadex", "listing", "latest"]);
    }

    #[test]
    fn extracts_settings_json() {
        let (settings, remaining) = extract_settings_option(&[
            "--settings".into(),
            r#"{"quality":"high"}"#.into(),
            "source".into(),
        ])
        .unwrap();
        assert_eq!(settings["quality"], "high");
        assert_eq!(remaining, ["source"]);
    }

    #[test]
    fn runs_the_mangadex_fixture() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../source/mangadex");
        let output = run_with_args(["test", path.to_str().unwrap()]).unwrap();
        assert_eq!(output, "1 fixture(s) passed");
    }

    #[test]
    fn validates_the_mangadex_package() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../source/mangadex");
        let output = run_with_args(["validate", path.to_str().unwrap()]).unwrap();
        assert!(output.contains("valid package:"));
        assert!(output.contains("\"id\": \"org.mangadex\""));
    }
}
