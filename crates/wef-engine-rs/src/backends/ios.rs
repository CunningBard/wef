//! iOS quirk sheet (Aidoku-style `WKWebView`).
//!
//! How each reference-algorithm step (`browser`, steps 1–6) maps to
//! WebKit APIs. There is no Swift code in this crate — this sheet is the
//! starting point for the Aidoku port. iOS diverges from desktop in one
//! structural way (no network-body API), so read step 4 first.
//!
//! ## Step 2 — isolated page
//!
//! - Fresh `WKWebView` with a non-persistent `WKWebsiteDataStore` per run;
//!   discard it on exit, success or failure.
//! - With `html`, use `loadHTMLString(_:baseURL:)` with the task URL as base.
//!
//! ## Step 3 — snapshot read
//!
//! - Fixed `evaluateJavaScript` one-liner (`document.querySelector(…)` +
//!   `textContent`), parsed with `JSONSerialization` in Swift. Constant
//!   code, JSON arguments — same prepared-statement pattern as Android.
//!
//! ## Step 4 — observation channels (inverted vs desktop)
//!
//! - `WKWebView` exposes **no** HTTPS response-body API, so the tap is the
//!   *primary* channel here, not the fallback: inject `tap_install.js` as a
//!   `WKUserScript` (injection time document-start, main frame only) and
//!   drain it with `evaluateJavaScript` on each loop tick.
//! - For network bodies (step 4a), fetch the API natively with cookies from
//!   `WKHTTPCookieStore` — the exact equivalent of the desktop
//!   cookie-to-`browserSession` mapping — or skip 4a entirely when the
//!   tap already yields the values the predicate needs. Either choice
//!   conforms as long as matching stays a port of
//!   `browser_capture_matches`.
//!
//! ## Step 6 — session
//!
//! - `WKHTTPCookieStore.getAllCookies` after the run, stored under the same
//!   opaque-token scheme. Tokens are host-instance-scoped strings.
//!
//! ## Timing
//!
//! - `WKUserScript` at document-start is what guarantees the tap is present
//!   before page scripts run — installing late (post-load `evaluate…`)
//!   misses early `JSON.parse`/`atob` calls. All desktop tuning values
//!   (`desktop` module) are free to change here; only `wef_core` limits
//!   are protocol.
