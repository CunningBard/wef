//! Android quirk sheet (Mihon-style `WebView`).
//!
//! How each reference-algorithm step (`browser`, steps 1–6) maps to Android
//! APIs. There is no Android code in this crate — this sheet is the starting
//! point for the Kotlin port, kept next to the desktop backends so
//! divergences stay visible.
//!
//! ## Step 2 — isolated page
//!
//! - Fresh `WebView` instance per run (clear data) or an incognito context;
//!   destroy it on exit, success or failure.
//! - With `html`, use `loadDataWithBaseURL(url, html, …)` with `url` as the
//!   base URL — the equivalent of the desktop `<base href>` injection.
//!
//! ## Step 3 — snapshot read
//!
//! - No `DOM` domain exists: evaluate one fixed one-liner such as
//!   `document.querySelector(…).textContent` via `evaluateJavascript` and
//!   parse the JSON in Kotlin (`org.json`). The snippet is constant — task
//!   data travels as JSON arguments, never string-built (the
//!   prepared-statement pattern from `browser`).
//!
//! ## Step 4a — network bodies
//!
//! - `WebViewClient.shouldInterceptRequest`: read the response bytes and let
//!   the request continue (pass-through). Sharing the `WebView`'s cookie jar
//!   with the plain-HTTP client (OkHttp `CookieJar`) is what makes
//!   `browserSession` requests work without a second login.
//!
//! ## Step 4b — parsed values
//!
//! - Ship `tap_install.js` / `tap_drain.js` verbatim as asset strings and
//!   install them `addJavascriptInterface`-free via `evaluateJavascript`
//!   (no Java bridge object means no bridge-compat surface to maintain).
//! - Drain once per loop tick exactly like the desktop host; match in
//!   Kotlin with a line-for-line port of `browser_capture_matches`.
//!
//! ## Step 6 — session
//!
//! - `CookieManager.getInstance().getCookie(url)` after the run, stored
//!   under the same opaque-token scheme (`browserSession` is just a string
//!   to sources). Scope tokens to the source/profile like cookies.
//!
//! ## Timing
//!
//! - `evaluateJavascript` runs on the UI thread; keep the drain call small
//!   (it is — the drain snippet only moves two arrays) and never block it
//!   waiting for network. All desktop tuning values (`desktop` module) are
//!   free to change here; only `wef_core` limits are protocol.
