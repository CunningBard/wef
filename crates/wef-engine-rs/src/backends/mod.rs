//! Host backends: one portable contract, many platform implementations.
//!
//! Everything a source can observe is defined outside this module:
//! the task model and matching predicate (`wef_core`), the six-step
//! reference algorithm (`browser`), and the `ctx` function shapes (`ctx`).
//! A backend is conformant when it implements those with whatever the
//! platform provides — sources cannot tell backends apart.
//!
//! ```text
//! portable (port this)          platform (reimplement per OS)
//! ────────────────────          ─────────────────────────────
//! wef_core model + limits       backends::cdp      desktop Chromium
//! browser algorithm + policy    backends::ureq     desktop plain HTTP
//! ctx function shapes           backends::android  Android WebView guide
//! error code() strings          backends::ios      iOS WKWebView guide
//! MockBrowserHost (test double) backends::desktop  desktop quirk sheet
//! ```
//!
//! The desktop pair (`cdp`, `ureq`) is the only code here. The mobile
//! modules are quirk sheets, not stubs: they record exactly which platform
//! API backs each algorithm step so a Kotlin/Swift port starts from answers
//! instead of archaeology. Desktop quirks and mobile quirks never share a
//! file — that separation is the point of this module.

pub mod android;
pub mod cdp;
pub mod desktop;
pub mod ios;
pub mod ureq;
