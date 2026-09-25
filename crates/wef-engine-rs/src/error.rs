use std::path::PathBuf;

use serde_json::Value;
use thiserror::Error;
use wef_core::ValidationError;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid manifest at {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid manifest: {0}")]
    Manifest(#[from] ValidationError),
    #[error("invalid package: {message}")]
    InvalidPackage { message: String },
    #[error("package requires unavailable host capability {capability:?}")]
    MissingHostCapability { capability: &'static str },
    #[error("host capability {capability:?} is not implemented yet")]
    UnsupportedCapability { capability: &'static str },
    #[error("invalid input for {operation}: {message}")]
    InvalidInput {
        operation: &'static str,
        message: String,
    },
    #[error("missing source export {operation:?}")]
    MissingExport { operation: &'static str },
    #[error("optional operation {operation:?} is not enabled by the manifest")]
    ExtensionNotEnabled { operation: &'static str },
    #[error("invalid response from {operation}: {message}")]
    InvalidResponse {
        operation: &'static str,
        message: String,
    },
    #[error("JavaScript error in {operation}: {message}")]
    Javascript {
        operation: &'static str,
        message: String,
    },
    #[error("source error in {operation}: [{code}] {message}")]
    Source {
        operation: &'static str,
        code: String,
        message: String,
        details: Option<Value>,
    },
}

impl EngineError {
    /// Stable machine code for conformance tests. Display prose may change;
    /// these strings are contract — every port reports the same codes.
    /// Ports: a switch/if-chain on the variant, returning the literal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "IO",
            Self::ManifestParse { .. } => "MANIFEST_PARSE",
            Self::Manifest(_) => "MANIFEST",
            Self::InvalidPackage { .. } => "INVALID_PACKAGE",
            Self::MissingHostCapability { .. } => "MISSING_HOST_CAPABILITY",
            Self::UnsupportedCapability { .. } => "UNSUPPORTED_CAPABILITY",
            Self::InvalidInput { .. } => "INVALID_INPUT",
            Self::MissingExport { .. } => "MISSING_EXPORT",
            Self::ExtensionNotEnabled { .. } => "EXTENSION_NOT_ENABLED",
            Self::InvalidResponse { .. } => "INVALID_RESPONSE",
            Self::Javascript { .. } => "JAVASCRIPT",
            Self::Source { .. } => "SOURCE",
        }
    }
}
