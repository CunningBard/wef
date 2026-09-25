//! Standalone, machine-readable validation and lint diagnostics for WEF packages.
//!
//! Error codes WEF001–WEF005 cover manifest/package loading. WEF006–WEF007
//! are the static browser-task checks the spec requires of validators
//! (executable strings and click-pagination must never reach
//! `ctx.browser.run`). WEF008–WEF010 cover the offline fixture contract.
//! WEF011+ cover repository layout (`lint_repo`). WEF1xx are warnings.

use std::{
    fs,
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use serde::Serialize;
use wef_core::Manifest;
use wef_engine_rs::{Engine, Package};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

pub fn lint_package(root: impl AsRef<Path>) -> Vec<Diagnostic> {
    let root = root.as_ref();
    let manifest_path = root.join("wef.json");
    let source = match fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(error) => {
            return vec![diagnostic(
                Severity::Error,
                "WEF001",
                error.to_string(),
                manifest_path,
                None,
            )];
        }
    };
    let manifest: Manifest = match serde_json::from_str(&source) {
        Ok(manifest) => manifest,
        Err(error) => {
            return vec![Diagnostic {
                severity: Severity::Error,
                code: "WEF002",
                message: error.to_string(),
                path: manifest_path,
                line: Some(error.line()),
                column: Some(error.column()),
            }];
        }
    };
    if let Err(error) = manifest.validate() {
        let field = match &error {
            wef_core::ValidationError::MissingField { field }
            | wef_core::ValidationError::InvalidField { field, .. } => *field,
        };
        return vec![diagnostic(
            Severity::Error,
            "WEF003",
            error.to_string(),
            manifest_path,
            field_location(&source, field),
        )];
    }
    let package = match Package::load(root) {
        Ok(package) => package,
        Err(error) => {
            return vec![diagnostic(
                Severity::Error,
                "WEF004",
                error.to_string(),
                root.to_path_buf(),
                None,
            )];
        }
    };
    if let Err(error) = Engine::without_host().validate_package(&package) {
        return vec![diagnostic(
            Severity::Error,
            "WEF005",
            error.to_string(),
            root.to_path_buf(),
            None,
        )];
    }
    let mut diagnostics = Vec::new();
    if manifest.languages.is_empty() {
        diagnostics.push(diagnostic(
            Severity::Warning,
            "WEF101",
            "source declares no languages".into(),
            manifest_path.clone(),
            field_location(&source, "languages"),
        ));
    }
    if manifest.base_urls.len() > 1 {
        diagnostics.push(diagnostic(
            Severity::Warning,
            "WEF102",
            "multiple base URLs require source-side selection".into(),
            manifest_path,
            field_location(&source, "baseUrls"),
        ));
    }
    diagnostics.extend(lint_browser_tasks(root));
    diagnostics.extend(lint_fixtures(root, &manifest));
    diagnostics
}

/// Rejects executable browser-task strings statically. The engine rejects
/// them at runtime too, but lint must catch them before a package ships:
/// `script`/`initializationScript` keys must never reach `ctx.browser.run`.
///
/// Implemented with Boa's own parser — the same grammar the engine
/// executes — so comments, strings, and exotic syntax can neither hide a
/// violation nor trigger a false one. Only statically visible
/// `*.browser.run({...})` calls are inspected; computed keys and optional
/// chains are out of scope for static analysis (the engine's
/// unknown-field rejection still guards them at runtime).
fn lint_browser_tasks(root: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for path in source_scripts(root) {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(_) => continue,
        };
        for hit in scan_browser_tasks(&source) {
            let code = if hit.key == "paginate" {
                "WEF007"
            } else {
                "WEF006"
            };
            diagnostics.push(diagnostic(
                Severity::Error,
                code,
                format!(
                    "browser task field `{}` is removed: sources submit data-only tasks",
                    hit.key
                ),
                path.clone(),
                Some((hit.line, hit.column)),
            ));
        }
    }
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.column.cmp(&right.column))
    });
    diagnostics
}

/// Collects lintable source scripts: every `.js` file except maintenance
/// (`scripts/`) and recordings (`fixtures/`), which are not source logic.
fn source_scripts(root: &Path) -> Vec<PathBuf> {
    let mut scripts = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|name| name == "fixtures" || name == "scripts")
                {
                    continue;
                }
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                {
                    continue;
                }
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "js") {
                scripts.push(path);
            }
        }
    }
    scripts.sort();
    scripts
}

/// One forbidden key inside a `browser.run` argument, with its position.
struct BrowserTaskHit {
    key: String,
    line: usize,
    column: usize,
}

/// Task fields that must never reach `ctx.browser.run`.
fn forbidden_browser_key(name: &str) -> bool {
    matches!(name, "script" | "initializationScript" | "paginate")
}

/// Scans one module with Boa's parser. Unparseable files yield nothing —
/// the engine already reports them (WEF004/WEF005).
fn scan_browser_tasks(source: &str) -> Vec<BrowserTaskHit> {
    use boa_ast::{scope::Scope, visitor::VisitWith};
    use boa_interner::Interner;
    use boa_parser::{Parser, Source};

    let mut interner = Interner::new();
    let parser_source = Source::from_bytes(source.as_bytes());
    let mut parser = Parser::new(parser_source);
    let scope = Scope::new_global();
    let Ok(module) = parser.parse_module(&scope, &mut interner) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    let mut scan = BrowserRunScan {
        interner: &interner,
        hits: &mut hits,
    };
    let _ = module.visit_with(&mut scan);
    hits
}

/// Outer walk: finds statically visible `*.browser.run(...)` calls and
/// inspects their arguments. Everything else uses default recursion.
struct BrowserRunScan<'a> {
    interner: &'a boa_interner::Interner,
    hits: &'a mut Vec<BrowserTaskHit>,
}

impl<'a> boa_ast::visitor::Visitor<'a> for BrowserRunScan<'a> {
    type BreakTy = ();

    fn visit_call(&mut self, node: &'a boa_ast::expression::Call) -> ControlFlow<()> {
        use boa_ast::visitor::VisitWith;

        if is_browser_run(node.function(), self.interner) {
            let mut keys = ForbiddenKeyScan {
                interner: self.interner,
                hits: self.hits,
            };
            for arg in node.args() {
                let _ = arg.visit_with(&mut keys);
            }
        }
        node.visit_with(self)
    }
}

/// Matches `browser.run` and `*.browser.run` callees (e.g.
/// `ctx.browser.run`). Optional chains and computed fields are out of
/// scope: they cannot be resolved statically.
fn is_browser_run(
    callee: &boa_ast::expression::Expression,
    interner: &boa_interner::Interner,
) -> bool {
    use boa_ast::expression::{
        Expression,
        access::{PropertyAccess, PropertyAccessField},
    };

    fn field_name(
        field: &PropertyAccessField,
        interner: &boa_interner::Interner,
    ) -> Option<String> {
        if let PropertyAccessField::Const(identifier) = field {
            return identifier_name(interner, identifier);
        }
        None
    }

    let Expression::PropertyAccess(access) = callee else {
        return false;
    };
    let PropertyAccess::Simple(access) = access else {
        return false;
    };
    if field_name(access.field(), interner).as_deref() != Some("run") {
        return false;
    }
    match access.target() {
        Expression::Identifier(identifier) => {
            identifier_name(interner, identifier).as_deref() == Some("browser")
        }
        Expression::PropertyAccess(target) => matches!(target, PropertyAccess::Simple(target)
            if field_name(target.field(), interner).as_deref() == Some("browser")),
        _ => false,
    }
}

/// Inner walk: records forbidden keys in any object literal under a
/// `browser.run` call — `Property` definitions by literal name plus
/// shorthand references (`{paginate}` sends `paginate` too).
struct ForbiddenKeyScan<'a> {
    interner: &'a boa_interner::Interner,
    hits: &'a mut Vec<BrowserTaskHit>,
}

/// Resolves an identifier through the parse interner. Non-UTF-8 names
/// cannot be forbidden keys and resolve to `None`.
fn identifier_name(
    interner: &boa_interner::Interner,
    identifier: &boa_ast::expression::Identifier,
) -> Option<String> {
    interner
        .resolve(identifier.sym())
        .and_then(|name| name.utf8())
        .map(str::to_owned)
}

impl<'a> boa_ast::visitor::Visitor<'a> for ForbiddenKeyScan<'a> {
    type BreakTy = ();

    fn visit_object_literal(
        &mut self,
        node: &'a boa_ast::expression::literal::ObjectLiteral,
    ) -> ControlFlow<()> {
        use boa_ast::{
            Spanned, expression::literal::PropertyDefinition, property::PropertyName,
            visitor::VisitWith,
        };

        for property in node.properties() {
            let found = match property {
                PropertyDefinition::IdentifierReference(identifier) => {
                    identifier_name(self.interner, identifier).map(|name| (name, identifier.span()))
                }
                PropertyDefinition::Property(name, _) => match name {
                    PropertyName::Literal(identifier) => identifier_name(self.interner, identifier)
                        .map(|name| (name, identifier.span())),
                    // Computed keys cannot be resolved statically; the
                    // engine's unknown-field rejection guards them.
                    PropertyName::Computed(_) => None,
                },
                _ => None,
            };
            if let Some((name, span)) = found
                && forbidden_browser_key(&name)
            {
                self.hits.push(BrowserTaskHit {
                    key: name,
                    line: span.start().line_number() as usize,
                    column: span.start().column_number() as usize,
                });
            }
        }
        node.visit_with(self)
    }
}

/// Core operations every fixture suite SHOULD cover, plus the capability
/// gates that pull in extension operations.
fn expected_fixture_operations(manifest: &Manifest) -> Vec<(&'static str, &'static str)> {
    let mut expected = vec![
        ("search", "search"),
        ("getMangaList", "listing"),
        ("getMangaUpdate", "update"),
        ("getPages", "pages"),
    ];
    let capabilities = &manifest.capabilities;
    if capabilities.filters {
        expected.push(("getFilters", "filters"));
    }
    if capabilities.settings {
        expected.push(("getSettings", "settings"));
    }
    if capabilities.url_resolution {
        expected.push(("resolveUrl", "URL resolution"));
    }
    if capabilities.image_requests {
        expected.push(("getImageRequest", "image requests"));
    }
    if capabilities.image_transforms {
        expected.push(("transformImage", "image transforms"));
    }
    if capabilities.migrations {
        expected.push(("migrateMangaKey", "key migration"));
        expected.push(("migrateChapterKey", "key migration"));
    }
    expected
}

const KNOWN_OPERATIONS: &[&str] = &[
    "getMangaList",
    "search",
    "getMangaUpdate",
    "getPages",
    "getSettings",
    "getFilters",
    "getImageRequest",
    "resolveUrl",
    "migrateMangaKey",
    "migrateChapterKey",
    "transformImage",
];

/// Validates the offline fixture contract: schema (WEF008), operation
/// coverage (WEF009), and committed secrets (WEF010). Fixtures record
/// request/response pairs — a real cookie, token, or session value in one
/// ships credentials to every reader of the repository.
fn lint_fixtures(root: &Path, manifest: &Manifest) -> Vec<Diagnostic> {
    let directory = root.join("fixtures");
    if !directory.exists() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    let mut covered: Vec<String> = Vec::new();
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    for path in &paths {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    "WEF008",
                    format!("could not read fixture: {error}"),
                    path.clone(),
                    None,
                ));
                continue;
            }
        };
        let fixture: serde_json::Value = match serde_json::from_str(&source) {
            Ok(fixture) => fixture,
            Err(error) => {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    "WEF008",
                    format!("invalid fixture JSON: {error}"),
                    path.clone(),
                    None,
                ));
                continue;
            }
        };
        let name = fixture.get("name").and_then(|value| value.as_str());
        let operation = fixture.get("operation").and_then(|value| value.as_str());
        let has_input = fixture.get("input").is_some();
        let has_expected = fixture.get("expected").is_some();
        match (name, operation) {
            (Some(_), Some(operation)) if KNOWN_OPERATIONS.contains(&operation) => {
                covered.push(operation.to_owned());
            }
            _ => {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    "WEF008",
                    "fixture must declare a name and a known operation with input and expected output"
                        .into(),
                    path.clone(),
                    None,
                ));
                continue;
            }
        }
        if !(has_input && has_expected) {
            diagnostics.push(diagnostic(
                Severity::Error,
                "WEF008",
                "fixture must declare input and expected output".into(),
                path.clone(),
                None,
            ));
        }
        if let Some(steps) = fixture.get("http").and_then(|value| value.as_array()) {
            for step in steps {
                check_fixture_step(path, step, &mut diagnostics);
            }
        }
    }
    for (operation, label) in expected_fixture_operations(manifest) {
        if !covered.iter().any(|covered| covered == operation) {
            diagnostics.push(diagnostic(
                Severity::Warning,
                "WEF009",
                format!("no fixture covers {label} ({operation})"),
                directory.clone(),
                None,
            ));
        }
    }
    diagnostics
}

/// Flags committed secrets in one fixture HTTP step: cookie, authorization,
/// and session material with real values.
fn check_fixture_step(path: &Path, step: &serde_json::Value, diagnostics: &mut Vec<Diagnostic>) {
    for section in ["request", "response"] {
        let Some(object) = step.get(section) else {
            continue;
        };
        if let Some(session) = object
            .get("browserSession")
            .and_then(|value| value.as_str())
            && !session.is_empty()
        {
            diagnostics.push(diagnostic(
                Severity::Error,
                "WEF010",
                format!("fixture {section} carries a browser session value"),
                path.to_path_buf(),
                None,
            ));
        }
        if let Some(headers) = object.get("headers").and_then(|value| value.as_object()) {
            for (name, value) in headers {
                let sensitive = [
                    "cookie",
                    "set-cookie",
                    "authorization",
                    "proxy-authorization",
                ];
                if sensitive
                    .iter()
                    .any(|banned| banned.eq_ignore_ascii_case(name))
                    && value.as_str().is_some_and(|text| !text.is_empty())
                {
                    diagnostics.push(diagnostic(
                        Severity::Error,
                        "WEF010",
                        format!("fixture {section} header `{name}` looks like a committed secret"),
                        path.to_path_buf(),
                        None,
                    ));
                }
            }
        }
    }
}

/// Lints an extension repository: every immediate subdirectory holding a
/// `wef.json` is one source and MUST satisfy the repository standard
/// (directory equals manifest `id`, spec identity scheme), then lints as a
/// package. Skips dot-directories and non-source directories silently.
pub fn lint_repo(root: impl AsRef<Path>) -> Vec<Diagnostic> {
    let root = root.as_ref();
    let mut diagnostics = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            return vec![diagnostic(
                Severity::Error,
                "WEF001",
                error.to_string(),
                root.to_path_buf(),
                None,
            )];
        }
    };
    let mut sources = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        if !path.join("wef.json").exists() {
            continue;
        }
        sources.push(path);
    }
    sources.sort();
    for path in sources {
        let manifest_path = path.join("wef.json");
        let source = match fs::read_to_string(&manifest_path) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    "WEF001",
                    error.to_string(),
                    manifest_path,
                    None,
                ));
                continue;
            }
        };
        let manifest: Manifest = match serde_json::from_str(&source) {
            Ok(manifest) => manifest,
            Err(error) => {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    "WEF002",
                    error.to_string(),
                    manifest_path,
                    None,
                ));
                continue;
            }
        };
        let directory = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if directory != manifest.id {
            diagnostics.push(diagnostic(
                Severity::Error,
                "WEF011",
                format!(
                    "source directory `{directory}` must equal manifest id `{}`",
                    manifest.id
                ),
                manifest_path.clone(),
                None,
            ));
        }
        if !valid_source_id(&manifest.id) {
            diagnostics.push(diagnostic(
                Severity::Error,
                "WEF012",
                format!(
                    "source id `{}` must match <language|multi>.wef.<site>",
                    manifest.id
                ),
                manifest_path.clone(),
                None,
            ));
        }
        if !valid_semver(&manifest.version) {
            diagnostics.push(diagnostic(
                Severity::Warning,
                "WEF103",
                format!(
                    "source version `{}` should be semantic (X.Y.Z) for update checks",
                    manifest.version
                ),
                manifest_path,
                None,
            ));
        }
        diagnostics.extend(lint_package(&path));
    }
    diagnostics
}

/// Repository identity scheme: `<language|multi>.wef.<site>`, e.g.
/// `en.wef.demo` or `multi.wef.demo`.
fn valid_source_id(id: &str) -> bool {
    let mut parts = id.split('.');
    let language = parts.next().unwrap_or("");
    let infix = parts.next().unwrap_or("");
    let site = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return false;
    }
    let language_ok = language == "multi"
        || ((2..=3).contains(&language.len())
            && language.bytes().all(|byte| byte.is_ascii_lowercase()));
    let site_ok = !site.is_empty()
        && site
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && site
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    language_ok && infix == "wef" && site_ok
}

/// Semantic core `X.Y.Z` with optional pre-release/build suffix.
fn valid_semver(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let mut parts = core.split('.');
    let numeric = [parts.next(), parts.next(), parts.next()];
    if parts.next().is_some() {
        return false;
    }
    numeric.into_iter().all(|part| {
        part.is_some_and(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn diagnostic(
    severity: Severity,
    code: &'static str,
    message: String,
    path: PathBuf,
    location: Option<(usize, usize)>,
) -> Diagnostic {
    Diagnostic {
        severity,
        code,
        message,
        path,
        line: location.map(|l| l.0),
        column: location.map(|l| l.1),
    }
}

fn field_location(source: &str, field: &str) -> Option<(usize, usize)> {
    let needle = format!("\"{field}\"");
    let offset = source.find(&needle)?;
    let before = &source[..offset];
    Some((
        before.bytes().filter(|byte| *byte == b'\n').count() + 1,
        offset - before.rfind('\n').map(|index| index + 1).unwrap_or(0) + 1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(source: &str) -> Vec<(String, usize, usize)> {
        scan_browser_tasks(source)
            .into_iter()
            .map(|hit| (hit.key, hit.line, hit.column))
            .collect()
    }

    #[test]
    fn flags_executable_keys_inside_browser_run() {
        let hits = scan(
            "export async function x(ctx) {\n  await ctx.browser.run({ url, task: { kind: 'snapshot', selector: 's' }, script: 'evil' });\n}",
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "script");
        assert_eq!((hits[0].1, hits[0].2), (2, 75));
    }

    #[test]
    fn flags_paginate_and_shorthand_forms() {
        let hits = scan("await browser.run({ task, paginate });");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "paginate");
        let hits = scan(
            "await ctx.browser.run({ task: t, paginate: { nextSelector: 'b', maxSteps: 1 } });",
        );
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn ignores_keys_outside_browser_run() {
        // Same keys in ordinary code, comments, and strings are not violations.
        let clean = [
            "const script = 'x'; // script: часа trick\n/* paginate: nope */",
            "const description = 'a script: fine';",
            "await other.run({ script: 1 });",
            "await ctx.browser.run({ task });",
        ];
        for source in clean {
            assert_eq!(scan(source), vec![], "false positive in {source:?}");
        }
    }

    #[test]
    fn skips_unparseable_files_silently() {
        assert_eq!(scan("export async function broken( {{"), vec![]);
    }

    #[test]
    fn validates_repository_identities() {
        assert!(valid_source_id("en.wef.demo"));
        assert!(valid_source_id("multi.wef.demo-site"));
        assert!(!valid_source_id("org.example.demo"));
        assert!(!valid_source_id("en.wef."));
        assert!(!valid_source_id("EN.wef.demo"));
        assert!(valid_semver("1.2.3"));
        assert!(valid_semver("0.1.0-beta.1"));
        assert!(!valid_semver("1.2"));
        assert!(!valid_semver("latest"));
    }
}
