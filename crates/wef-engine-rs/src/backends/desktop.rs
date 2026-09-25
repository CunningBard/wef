//! Desktop quirk sheet (Chromium + plain HTTP).
//!
//! This records every behavior of the two desktop backends (`cdp`, `ureq`)
//! that is platform choice rather than spec. A porter reimplementing on
//! mobile needs this list to know what is safe to change — and a desktop
//! maintainer needs it to avoid "fixing" a quirk a source depends on.
//!
//! ## Connection and lifecycle (`cdp`)
//!
//! - The endpoint is an already-running local Chromium (`http://127.0.0.1`,
//!   `localhost`, or `::1` only — never launched, never remote).
//! - Each `run_browser` opens one ephemeral tab (`json/new`) and closes it
//!   on exit, success or failure. No state survives except the browser
//!   profile's own cookies.
//! - With `html`, the tab starts at `about:blank` and the document is
//!   installed via `Page.setDocumentContent` with a `<base href>` injection
//!   so relative URLs resolve — no navigation, no script evaluation.
//!
//! ## Observation channels (`cdp`)
//!
//! - Network bodies: `Network.enable` with 20 MB total / 5 MB per-resource
//!   buffers; finished bodies are pulled with `Network.getResponseBody`.
//!   Base64 (binary), oversized (> 8 MB), and unparseable bodies are
//!   skipped, never fatal.
//! - Parsed values: the fixed `tap_install.js` proxy on `JSON.parse` plus an
//!   `atob` byte hook, drained once per event-loop tick with `tap_drain.js`
//!   through `Runtime.evaluate`. Both snippets are `include_str!` constants.
//! - Snapshot: `DOM.getDocument` (depth −1, pierce) → `DOM.querySelector` →
//!   `DOM.getOuterHTML`, JSON parsed from the element's inner text in Rust.
//!
//! ## Tuning vs spec
//!
//! These are desktop choices, free to change per backend — they are NOT
//! protocol: 250 ms socket read slices, 15 s single round-trip timeout,
//! overall `timeoutMs` (policy-capped at 30 s), tap buffers (100 parsed /
//! 20 binary values), 200-value match/unmatched caps. Spec-mandated bounds
//! live in `wef_core::browser_limits` and `store_limits` instead.
//!
//! ## Sessions and cookies
//!
//! - After a run, `Network.getCookies` is flattened to a `name=value` header
//!   and stored under an opaque random token (`cdp-session-` + 128 bits).
//!   Sources pass the token back as `browserSession` on plain requests; the
//!   token is scoped to this host instance and means nothing outside it.
//!   Session requests are origin-checked against the policy: the jar holds
//!   the whole profile, so it must never ride along to a non-package URL.
//! - The reported URL comes from the navigation history's selected entry
//!   (falling back to the task URL), re-checked against the policy.
//!
//! ## Plain HTTP (`ureq`)
//!
//! - One blocking agent: follows redirects, keeps cookies for the host's
//!   lifetime, 30 s default timeout. Sources that rely on cookies or
//!   redirect chains must reuse the same host — see the `UreqHost` docs.
//! - Cookie-jar JSON import/export exists for CLI persistence only; mobile
//!   ports use their platform cookie stores instead (see `android`, `ios`).
