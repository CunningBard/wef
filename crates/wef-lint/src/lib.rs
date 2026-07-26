//! Standalone, machine-readable validation and lint diagnostics for WEF packages.

use std::{
    fs,
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
    diagnostics
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
