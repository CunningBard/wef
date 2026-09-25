//! The `ctx` contract: every function a source may call.
//!
//! Ports do NOT translate `runtime.rs` — Boa internals stay on desktop.
//! They reimplement the dozen functions below in their own JS engine
//! (JavaScriptCore, Hermes, …) backed by the portable pieces: `wef_core`
//! for validation and limits, `browser` for the browser algorithm, and a
//! `backends` implementation for HTTP and observation.
//!
//! Capability gating (port this rule, not the code): each namespace exists
//! on `ctx` **only** when the manifest lists it in `requires` (`http`,
//! `html`, `browser`, `image`, `storage`). Anything else is a clean
//! "missing capability" failure before the source runs.
//!
//! ```text
//! ctx.fail(code, message, details?)   always present; throws {__wefError, code, message, details}
//! ctx.url.resolve(url, base?)         always present; absolute-URL resolution
//! ctx.settings                        always present; effective settings object
//! ctx.http.request({url, method?, headers?, query?, body?, browserSession?})
//!   → {status, url, headers, body}    requires "http"
//! ctx.html.parse(html) → document     requires "html"
//!   document.select(sel) → element? | document.selectAll(sel) → element[]
//!   element.select(sel)? | element.selectAll(sel)[] | element.text()
//!   element.html() | element.attr(name)
//! ctx.browser.run({url, html?, timeoutMs?, task}) → {url, payload, session}
//!                                     requires "browser" (see `browser`)
//! ctx.image.*                           requires "image" (see `image`)
//! ctx.store.get(key) → value | null   requires "storage"
//! ctx.store.set(key, value) → true    `set(key, null)` deletes the key
//! ```
//!
//! Store rules (spec 0.0.4 §3, enforced identically everywhere): keys are
//! `1..=128` chars, values are JSON of `<= 65536` bytes, at most 128 keys
//! per source identity. Violations fail the call, never truncate.
//!
//! Source-thrown failures carry `{code, message, details?}` and surface as
//! `EngineError::Source`; host failures surface as `HostError` with a stable
//! `code()` string (see [`crate::host::HostError::code`] and
//! [`crate::EngineError::code`]). Both taxonomies are part of the contract —
//! conformance tests assert codes, not prose.
