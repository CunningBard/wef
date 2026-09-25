# WEF conformance fixtures

`core-source` exercises every required 0.0.1 operation. `0.0.2-source` covers
settings, rich filters, and image candidates. Both use the deterministic
`wef test` fixture format used by real sources.

```sh
cargo run -p wef-cli -- test fixtures/conformance/core-source
cargo run -p wef-cli -- test fixtures/conformance/0.0.2-source
```
