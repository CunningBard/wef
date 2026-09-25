//! Minimal Rust WEF reference engine.
//!
//! The reference engine loads and validates source packages, executes their
//! ECMAScript modules, and exposes a small injectable host capability surface.
//!
//! ## For porters: what to reimplement, in order
//!
//! 1. [`wef_core`] — the data model, limits, and matching predicate.
//! 2. [`browser`] — the six-step reference algorithm and policy shapes.
//! 3. [`ctx`] — the dozen `ctx` functions sources may call.
//! 4. [`host`] — the `WefHost` trait plus request/response shapes.
//! 5. [`backends`] — one backend per platform (desktop code + mobile guides).
//!
//! [`Package`], [`Engine`], and [`EngineError`] are the remaining runtime
//! pieces; `runtime` (Boa internals) is desktop-only and never ported.

pub mod backends;
pub mod browser;
pub mod ctx;
pub mod host;

mod engine;
mod error;
mod image;
mod loader;
mod package;
mod runtime;

pub use backends::{cdp::CdpBrowserHost, ureq::UreqHost};
pub use browser::{
    BrowserCaptureSpec, BrowserCaptureTask, BrowserPolicy, BrowserRunRequest, BrowserRunResult,
    BrowserSnapshotSpec, BrowserSnapshotTask, BrowserTask, InteractiveBrowserHost,
    InteractiveBrowserSurface, MAX_PAYLOAD_BYTES, MockBrowserHost, MockBrowserReply,
};
pub use engine::{
    Engine, ExtensionOperation, ImageTransformInput, ImageTransformOutput, Operation,
};
pub use error::EngineError;
pub use host::{BinaryHttpResponse, HostError, HttpRequest, HttpResponse, WefHost};
pub use package::Package;
pub use wef_core;

#[cfg(test)]
mod tests;
