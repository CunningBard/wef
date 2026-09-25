# Changelog

All notable changes to this project are documented here. Crate versions
follow SemVer; the `wef` manifest format versions (`0.0.x`, see
`crates/wef-spec/`) evolve independently.

## [0.1.0] - 2026-09-25

First release: reference engine, CLI, and reference sources, implementing
WEF manifest format `0.1.0` (consolidated in `crates/wef-spec/wef-0.1.0.md`,
wire-identical to the `0.0.4` draft it finalizes).

### Specification (`crates/wef-spec/`)

- Versioned format documents `0.0.1` through `0.0.4`, plus the standalone
  `0.1.0` consolidation (self-contained; wire-identical to `0.0.4`).
- `0.0.4` adds source storage (`ctx.store`), unmatched-observation
  envelopes (`capture.includeUnmatched`), and retires host-side
  click-pagination (multi-page lists are the source's job now).
- `0.1.0` standardizes extension repositories: `repo.json` metadata,
  generated `index.json` catalog, `sources/<id>/` layout with the
  `<language|multi>.wef.<site>` identity scheme, and a MUST-ignore rule
  for everything else. Display icons are conventional only
  (`res/icon.png`, never in `wef.json`, never validated).

### Reference engine (`crates/wef-engine-rs/`)

- Capability-gated Boa JS runtime (`http`, `html`, `browser`, `image`,
  `storage`, `settings`, `fail`); the full `ctx` contract is documented
  in `ctx.rs`.
- Jailed module loader: imports resolve against the package root, then
  canonicalize, then prefix-check — symlinks cannot escape.
- Desktop backends: CDP browser host (single-capture, fixed tap snippets,
  origin-checked session requests, random session tokens) and plain-HTTP
  host with cookie-jar persistence.
- `MockBrowserHost` conformance double; image transforms with enforced
  limits; stable machine-readable error codes (`code()`).
- Portable core isolated for ports: `wef_core` model, six-step browser
  algorithm, per-platform backend guides (`android`, `ios`, `desktop`).

### CLI (`crates/wef-cli/`)

- `validate`, `lint`, `run`, `test` commands with `--session`,
  `--settings`, `--cdp`, `--store`, `--filters` options.
- Interactive `repl` (aliases `interactive`, `shell`): load/unload
  sources, search/listing/update/pages with result-index references,
  lazy CDP that attaches on first browser use only.

### Reference sources (`sources/`)

- `multi.wef.magadex`, `en.wef.html-example`: public-API and HTML scraping
  patterns with fixtures.

### Security (`SECURITY.md`)

- Documented model: sandboxed sources, import jail, never-evaluates host,
  session containment, secret redaction, accepted risks (SSRF,
  trust-on-install), and a port checklist.
