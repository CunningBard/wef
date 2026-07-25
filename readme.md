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

## License

WEF is dual-licensed under the terms of either the MIT License or the
Apache License, Version 2.0, at your option.

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
