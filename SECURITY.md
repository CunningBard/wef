# WEF Security Model

WEF runs community-written source packages — the trust model is
trust-on-install, like Mihon extensions and Aidoku sources. "Safe" here
means: a malicious source is contained to its sandbox and its declared
capabilities, untrusted strings never become code on the host, and secrets
never leave through side channels. It does **not** mean a malicious source
can't misbehave at all (see [Accepted risks](#accepted-risks)).

## 1. Source sandbox

Sources execute in an embedded ECMAScript engine (Boa on desktop) with no
I/O except the `ctx` bridges (`http`, `html`, `browser`, `image`,
`store`). There is no `fetch`, no filesystem, no process access — only
what `ctx` exposes. Each namespace exists only when the manifest lists it
in `requires`; otherwise the capability is absent before the source runs
(`crates/wef-engine-rs/src/runtime.rs`, `context_value`).

Bridges deserialize inputs into typed structs and re-validate outputs
against operation schemas, so malformed or hostile values fail closed.
Execution is bounded: a loop-iteration limit aborts runaway scripts
(`engine.rs`, `runtime_limits_mut`), and HTTP/browser/image flows carry
timeouts, body caps, and match caps.

## 2. Import jail

`import` specifiers resolve against the package root and **cannot leave
it**, enforced twice, in this order (`loader.rs`, `JailedModuleLoader`):

1. Boa's resolver rejects lexical `..` escapes
   (`"path is outside the module root"`).
2. The resolved path is **canonicalized** (symlinks resolved) and rejected
   unless it still lands under the root
   (`"module … escapes the package root"`).

Step 2 is the load-bearing one: lexical checks alone are defeated by an
in-package symlink (`link.js -> /etc/secret.js`), which normalizes to an
in-root path and would otherwise be read straight off disk and exfiltrated
via `ctx.http`. The entry module gets the same treatment at package load
(`package.rs`: canonicalize + prefix check). Regression tests pin both
cases (`module_imports_cannot_climb_above_the_package_root`,
`module_imports_cannot_follow_symlinks_out_of_the_package`).

## 3. The host never evaluates

The host evaluates exactly **two** strings in a page — `tap_install.js`
and `tap_drain.js` — both `include_str!` compile-time constants, identical
for every task (pinned by `cdp_tap_snippets_carry_no_task_data`). There is
no other `evaluate`/`eval`/`Function` in host code. Everything else
crosses boundaries as data: selectors and URLs travel as JSON parameters
to CDP methods, bodies parse via `serde_json`, CSS via `Selector::parse`,
URLs via `Url::parse` plus form-encoding. A hostile page can only degrade
observations (which a hostile source could return directly anyway);
matching happens host-side in Rust, never in page code.

## 4. Browser and sessions (CDP)

- The endpoint must be an already-running **local** Chromium
  (`127.0.0.1`/`localhost`/`::1`); the host never launches or contacts a
  remote browser.
- `run_browser` is policy-gated (consent, allowed origins, timeout cap).
- The cookie jar collected after a run holds the **whole profile**, so
  session-authenticated requests are origin-checked against the policy
  (`CdpBrowserHost::request`): the jar can never ride along to a
  non-package URL. The unscoped collection is deliberate — session flows
  span hosts (API host + static-asset host) — with containment enforced
  at request time instead.
- Session tokens are 128-bit OS random (`mint_session_token`), never
  sequential: tokens authenticate the jar, and sequential ids are
  guessable across sources sharing one host.

## 5. Secrets

Secret settings and source-store contents are scrubbed from errors and
diagnostics (`redact_*` in `engine.rs`); the spec requires hosts to redact
store contents like secret settings. Cookie jars, store files, and
challenge material are sensitive files — persisted via the `--session` /
`--store` paths the operator chooses and protects.

## Accepted risks

- **SSRF by design.** `ctx.http.request` allows any `http(s)` URL (image
  CDNs require it). Same tradeoff as Mihon. A malicious source can prod
  the operator's LAN.
- **Trust-on-install exfiltration.** Secret settings and store values are
  handed to sources on purpose; a malicious source sends them anywhere.
  Install sources you trust.
- **Downstream markup.** Descriptions/synopses return source HTML.
  Consuming apps must treat source output as untrusted markup (sanitize or
  render as text) — the engine does not strip it.
- **Caps are bounds, not guarantees.** Timeouts and byte caps limit abuse;
  they don't make a hostile source cheap.

## Port checklist

A Kotlin/Swift port keeps this model only if it replicates: capability
gating, resolve-then-`realpath`-then-prefix-check **in that order** for
every module/resource load, fixed-byte page snippets with a no-task-data
test, origin-checked session requests, random session tokens, secret
redaction, and execution limits. Miss any one and the model has a hole —
see each item's section above for the failure it prevents.
