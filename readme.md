# WEF — Web Extension Format

WEF is a proposed portable format for defining manga sources across different readers.

Instead of maintaining separate source extensions for every app, a source can be written once in WEF and executed by any compatible WEF engine.

## Core idea

WEF describes how to retrieve source data such as:

* manga search results
* manga details
* chapter lists
* page or image lists

WEF itself is only a format.

A **WEF engine** is an implementation that reads and executes WEF source definitions. Different readers may implement their engines however they want, as long as they follow the WEF specification.

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

## Planned components

* WEF specification
* Rust reference engine
* command-line tools
* validator and linter
* source test runner
* example source implementations

## Initial roadmap

1. Define the core data models and operations
2. Implement a minimal Rust engine
3. Create a MangaDex source using its documented API
4. Add a simple HTML-parsing source
5. Build validation and conformance tests
6. Document how readers can implement WEF engines

## Status

Very early development. The format and APIs are not stable yet.

The reference engine currently loads and validates a package, executes the
four core async operations, exposes `url.resolve` and `ctx.fail`, and provides
a synchronous `UreqHost` for text HTTP requests. A host can be attached with:

```rust
use wef_engine_rs::{Engine, UreqHost};

let engine = Engine::with_host(UreqHost::default());
```

An initial MangaDex source implementation is available in
[`source/mangadex`](source/mangadex). It supports latest and popular listings,
search, manga details, English chapter feeds, and MangaDex@Home pages.

## CLI

The reference CLI validates packages, runs core operations, and replays source
fixtures:

```text
cargo run -p wef-cli --bin wef -- validate source/mangadex
cargo run -p wef-cli --bin wef -- test source/mangadex
cargo run -p wef-cli --bin wef -- run source/mangadex listing latest --page 1
cargo run -p wef-cli --bin wef -- run --session mangadex-cookies.json source/mangadex listing latest
```

`run` uses the production HTTP host. `test` uses `fixtures/*.json` request and
response recordings, so it is deterministic and does not contact the source.
The optional `--session` argument persists only persistent cookies in an
explicit JSON file; that file may contain authenticated session material and
should be protected accordingly.

See [ROADMAP.md](ROADMAP.md) for implementation status and next milestones.
For the experimental capability migration, see
[Migrating to 0.0.2](docs/MIGRATING-0.0.2.md).

## License

WEF is dual-licensed under the terms of either the MIT License or the
Apache License, Version 2.0, at your option.

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
