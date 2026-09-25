# WEF conformance fixtures

`core-source` exercises every required 0.0.1 operation. `0.0.2-source`
covers settings, rich filters, and image candidates. `binary` covers
byte/XOR and grid-descramble image transforms with strict limits. All
use the deterministic `wef test` fixture format used by real sources.

```sh
cargo run -p wef-cli -- test fixtures/conformance/core-source
cargo run -p wef-cli -- test fixtures/conformance/0.0.2-source
```

`binary/` holds shared image blobs (referenced by engine unit tests),
not a runnable package.

## Coverage matrix

| Spec surface | Fixtures | Engine unit tests |
|---|---|---|
| core operations (listing/search/update/pages) | `core-source`, real sources | `engine` op tests |
| settings, rich filters | `0.0.2-source` | `validate_filters` tests |
| image candidates + transforms | `0.0.2-source`, `binary` | transform limit tests |
| rate limits | — | `UreqHost` rate-window tests |
| source storage (`ctx.store`) | n/a (fresh engine per fixture) | store persistence/isolation/limit tests |
| browser capture predicate | n/a (mock hosts return payloads literally) | capture matching + tap invariant tests |
| import jail | n/a | traversal + symlink escape tests |
| error codes | n/a | `code()` contract tests |
| manifest validation | n/a | `wef-core` validation tests |

`n/a` rows are covered one layer up or down on purpose: fixtures pin
JSON behavior with a deterministic mock host, while stateful or live
behavior (storage, browser observation, module loading) is pinned by
engine unit tests that own their hosts. Browser runs against real pages
stay manual (REPL + CDP) — fixtures MUST NOT embed live captures.

Real sources extend the matrix: `examples/*/fixtures` MUST cover every
operation their manifest enables (`wef lint` warns per missing
operation, code `WEF009`).

