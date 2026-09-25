# WEF source repository

This is the **standards** repository: it ships the format, the reference
engine, the conformance fixtures, and only neutral reference sources
(public APIs, fictional examples). Site scrapers that defeat access
controls — cipher harvesting, WAF handling, challenge bypass — MUST NOT
be published here; they live in the separate **extensions** repository.

Every source lives in its own directory directly under `sources/` (or
under `sources/third-party/`, see below), alongside the engine
(`crates/`), the versioned format (`crates/wef-spec/`), and the
conformance fixtures (`fixtures/`).

## Directory rule

The directory name MUST equal the manifest `id`:

```text
sources/<manifest-id>/
```

`org.example.demo/wef.json` declares `"id": "org.example.demo"`, and so on.
Tooling (`wef validate|lint|test|run`, the REPL's `load`, future repo
indexing) relies on this instead of scanning manifests. The engine itself
stays path-agnostic — this is a repository convention, not a format rule,
so `wef.json` needs no field for it.

## Standard layout

```text
sources/org.example.demo/
├── wef.json      # manifest: id, entry, requires, listings, capabilities
├── source.js     # entry module: the four core operations + capabilities
├── api.js        # optional: site client, query builders, signing
├── config.js     # optional: static tables (genres, sorts, statuses)
├── fixtures/     # mock-host fixtures, runnable offline via `wef test`
├── scripts/      # optional: maintenance (tag refresh, fixture regen)
├── icon.png      # optional
└── README.md     # purpose, site quirks, fixture instructions
```

Rules and conventions:

- `wef.json` + entry module are required; everything else follows the
  spec (`crates/wef-spec/wef-0.1.0.md`).
- Shared code travels by copy, not by import: the import jail
  (`crates/wef-engine-rs/src/loader.rs`) forbids leaving the package
  root, so there is no cross-source `lib/` — factor common patterns into
  `templates/` instead (planned).
- `fixtures/*.json` MUST cover every operation the source exposes and
  MUST NOT embed secrets or live browser captures (mock host only).
- `scripts/` helpers MUST be reproducible from public endpoints and
  MUST NOT contain credentials.
- Keep generated artifacts out: no committed store files, cookie jars,
  or capture dumps.

## Local scratch and third-party sources

`sources/third-party/` is git-ignored scratch space for unpacked
third-party sources under test — typically a checkout of the extensions
repository, or individual extension sources being developed against this
engine. Never commit it.

Tooling treats both locations identically: `wef validate|lint|test|run`
and the REPL's `load` take any directory path, so an extensions-repo
source runs unchanged from its mount point, e.g.
`sources/third-party/org.example.demo`.
