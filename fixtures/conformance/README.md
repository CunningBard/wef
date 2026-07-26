# WEF conformance fixtures

`core-source` is a dependency-free package that exercises every required core
operation through the same deterministic `wef test` fixture format used by
real sources. Engines and adapters can reuse it as a minimal compatibility
check: `cargo run -p wef-cli --bin wef -- test fixtures/conformance/core-source`.
