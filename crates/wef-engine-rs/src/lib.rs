//! Minimal Rust WEF 0.0.1 engine.
//!
//! The reference engine loads and validates source packages, executes their
//! ECMAScript modules, and exposes a small injectable host capability surface.

mod browser;
mod cdp;
mod engine;
mod error;
mod host;
mod image;
mod package;
mod runtime;

pub use browser::{
    BrowserPolicy, BrowserRunRequest, BrowserRunResult, InteractiveBrowserHost,
    InteractiveBrowserSurface, MockBrowserHost, MockBrowserReply,
};
pub use cdp::CdpBrowserHost;
pub use engine::{
    Engine, ExtensionOperation, ImageTransformInput, ImageTransformOutput, Operation,
};
pub use error::EngineError;
pub use host::{
    BinaryHttpResponse, HostError, HttpRequest, HttpResponse, NoHost, UreqHost, WefHost,
};
pub use package::Package;
pub use wef_core;

#[cfg(test)]
mod tests;
