# WEF example sources

This directory holds the reference sources shipped with the **standards**
repository: a public-API source and a fictional HTML source. They exist to
exercise the engine, not to serve readers. Real extension repositories
live elsewhere (see the `third-party` mount below) and follow the
repository standard in `crates/wef-spec/wef-0.1.0.md`, section 31.

Site scrapers that defeat access controls — cipher harvesting, WAF
handling, challenge bypass — MUST NOT be published here; they belong in
an extensions repository.

## Directory rule

The directory name MUST equal the manifest `id`, using the repository
identity scheme `<language|multi>.wef.<site>`:

```text
examples/multi.wef.magadex/   # multi-language public API source
examples/en.wef.html-example/ # English fictional HTML source
```

## Standard layout

```text
examples/en.wef.demo/
├── wef.json      # manifest: id, entry, requires, listings, capabilities
├── source.js     # entry module: the four core operations + capabilities
├── api.js        # optional: site client, query builders, signing
├── config.js     # optional: static tables (genres, sorts, statuses)
├── fixtures/     # mock-host fixtures, runnable offline via `wef test`
├── scripts/      # optional: maintenance (tag refresh, fixture regen)
├── res/icon.png  # optional display icon: conventional only, never in
│                 # wef.json, never validated; readers MAY fetch it
└── README.md     # purpose, site quirks, fixture instructions
```

Rules and conventions:

- `wef.json` + entry module are required; everything else follows the
  spec (`crates/wef-spec/wef-0.1.0.md`).
- Shared code travels by copy, not by import: the import jail
  (`crates/wef-engine-rs/src/loader.rs`) forbids leaving the package
  root, so there is no cross-source `lib/`.
- `fixtures/*.json` MUST cover every operation the source exposes and
  MUST NOT embed secrets or live browser captures (mock host only).
- `scripts/` helpers MUST be reproducible from public endpoints and
  MUST NOT contain credentials.
- Keep generated artifacts out: no committed store files, cookie jars,
  or capture dumps.

## Local scratch and third-party sources

`examples/third-party/` is git-ignored scratch space for unpacked
third-party sources under test — typically a checkout of the extensions
repository, or individual extension sources being developed against this
engine. Never commit it.

Tooling treats every location identically: `wef validate|lint|test|run`
and the REPL's `load` take any directory path, so an extensions-repo
source runs unchanged from its mount point.
