# WEF — Web Extension Format

WEF is a proposed portable format for defining manga sources across different readers.

Instead of maintaining separate source extensions for every app, a source
can be written once in WEF and executed by any compatible WEF engine.

## Core idea

WEF describes how to retrieve source data such as:

* manga search results
* manga details
* chapter lists
* page or image lists

WEF itself is only a format.

A **WEF engine** is an implementation that reads and executes WEF source
definitions. Different readers may implement their engines however they
want, as long as they follow the WEF specification.

## Goals

* Allow source definitions to work across multiple manga readers
* Standardize source operations and returned data
* Keep the format independent from any specific reader
* Support API-based and HTML-based sources
* Provide a reference engine and tooling written in Rust

## Non-goals

WEF does not define:

* reader interfaces
* library management
* download behavior
* browser implementation
* extension repository governance
* how an engine must be internally implemented

## Status

Experimental. The format and APIs can change.

The repository contains the versioned format documents, a Rust reference
engine, deterministic conformance fixtures, and API/HTML example sources.

Reference sources live in `examples/` under their repository identity
(`<language|multi>.wef.<site>`); site scrapers belong in a separate
extensions repository (see `examples/README.md`). The machine-readable
extension-repository layout — `repo.json`, generated `index.json`,
`sources/<id>/` — is standardized in `crates/wef-spec/wef-0.1.0.md`,
section 31.

## Implementing WEF in another language

An engine only needs to:

1. Load and validate `wef.json`.
2. Load the declared module and require the manifest-enabled exports.
3. Invoke an operation with its JSON input and a capability-gated `ctx`.
4. Validate JSON output against the versioned WEF model.
5. Run the conformance fixtures.

The engine, HTTP stack, cookie storage, browser bridge, image codec, and reader
UI are implementation choices. The format documents and fixtures—not the Rust
types—are the compatibility contract.

## CLI

The reference CLI validates packages, runs core operations, and replays source
fixtures:

```text
cargo run -p wef-cli -- validate examples/multi.wef.magadex
cargo run -p wef-cli -- test examples/multi.wef.magadex
cargo run -p wef-cli -- test examples/en.wef.html-example
cargo run -p wef-cli -- test fixtures/conformance/core-source
cargo run -p wef-cli -- test fixtures/conformance/0.0.2-source
cargo run -p wef-cli -- repl
```

`run` uses the production HTTP host. `test` uses `fixtures/*.json` request and
response recordings, so it is deterministic and does not contact the source.
The optional `--session` argument persists only persistent cookies in an
explicit JSON file; that file may contain authenticated session material and
should be protected accordingly.

## License

WEF is dual-licensed under the terms of either the MIT License or the
Apache License, Version 2.0, at your option.

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
