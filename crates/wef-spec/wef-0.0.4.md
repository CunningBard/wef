# Web Extension Format (WEF) 0.0.4

**Status:** Experimental compatibility draft
**Version:** 0.0.4
**Supersedes:** WEF 0.0.3 for sources declaring `"wef": "0.0.4"`

## 1. Scope and compatibility

WEF 0.0.4 retains the 0.0.3 package layout, manifest fields, four core
operations, data models, HTTP API, HTML API, URL API, settings, filters,
image requests/transforms, error model, network policy, and the data-only
browser bridge. A 0.0.4 engine MUST reject an unsupported manifest version
rather than silently interpreting it as an earlier version.

0.0.4 adds two things, both site-agnostic:

- **source storage** (`ctx.store`): persistent per-source key-value state, so
  sources can remember runtime bootstrap material (cipher keys, tokens)
  across invocations instead of re-running the browser every call;
- **unmatched capture observations** (`capture.includeUnmatched`): lets a
  source see observed values the match predicate rejected, so site-specific
  recognition and decoding stays in source code, never in the host.

## 2. Manifest

```ts
interface Manifest {
  wef: "0.0.4";
  requires: ("http" | "html" | "browser" | "image" | "storage")[];
  // All other 0.0.3 fields unchanged.
}
```

`storage` is a host capability like `http`: it MUST be listed in `requires`
before a source may use `ctx.store`. The reference engine always provides it
(engine-owned map, see section 3); a host that cannot persist across restarts
MAY keep it in memory, but MUST keep it for the lifetime of the source
session. A host that provides no storage at all MUST fail closed with
`UNSUPPORTED` when a `storage`-requiring source uses `ctx.store`.

## 3. Source storage

```ts
interface StoreApi {
  get(key: string): JsonValue | null;
  set(key: string, value: JsonValue): boolean;
}
interface WefContext {
  store?: StoreApi; // present only with requires: ["storage"]
  // Existing context fields omitted.
}
```

Rules:

- Keys are `1..=128` chars. Values are JSON, `<= 65536` bytes serialized,
  at most 128 keys per source. Violations fail the operation.
- Storage is scoped to one source identity and configuration profile — the
  same scoping as cookies, browser sessions, and settings. Sources MUST NOT
  expect another source's values, and MUST treat a missing key (`null`) as
  "bootstrap again".
- Hosts MUST redact store contents from diagnostics, logs, and fixtures like
  secret settings (bootstrap material is credential-equivalent).
- `set` returns `true` on success. Hosts MAY persist to disk or keep memory
  only; sources MUST handle cold starts on every operation.
- The reference pattern for rotation: on upstream rejection
  (`Invalid token`, decrypt failure), the source clears the entry
  (`set(key, null)` deletes) and re-bootstraps, then retries once — the same
  state machine as dropping a stale cookie.

## 4. Unmatched observations

```ts
interface BrowserCaptureSpec {
  // Existing 0.0.3 fields omitted.
  includeUnmatched?: boolean; // default false
}
```

Without the flag, payloads are exactly per 0.0.3 section 4. With
`includeUnmatched: true`, `payload` is instead:

```ts
interface UnmatchedEnvelope {
  matched: JsonValue;   // the normal section-4 payload for this task
  unmatched: JsonValue[]; // observed values the predicate rejected
}
```

- `unmatched` holds network response bodies (parsed as JSON) and observed
  parsed/binary-decoded values that did not match, in observation order,
  deduplicated by exact JSON equality, capped like any other payload.
- Sources MUST handle the envelope explicitly (`"unmatched" in payload`).
  Hosts MUST NOT interpret unmatched values — recognition, decoding, and
  mapping stay in source JavaScript inside the engine sandbox.
- Mock/conformance hosts return configured payloads literally and do not
  synthesize `unmatched`.

## 5. Retired: click-pagination

0.0.3's `paginate` task field (auto-clicking `nextSelector` buttons host-side)
is removed in 0.0.4. Rationale, from reference-source experience: paginated
content is cheaper and more robustly collected by the source itself — sign
plain requests from `ctx.store` material and walk `meta` pagination natively
— than by driving clicks through a live page. Hosts MUST reject a `paginate`
field (unknown-field rejection, not silent ignore) so stale sources fail
loudly. The retired bounds (`maxSteps`, `settleMs`, `nextLabelContains`) no
longer apply to anything.

## 6. Observation channels (reference-host note)
0.0.3 hosts observe network bodies and `JSON.parse` values. 0.0.4 reference
hosts additionally observe binary-decode outputs (`atob()` results) through
the same fixed, interpolation-free tap mechanism — some sites hand ciphertext
to page code that never reappears as a named JSON envelope. Like the parse
tap, the decode tap carries no task data; matched values flow through section
3 of the 0.0.3 spec predicate unchanged, the rest only surface under
`includeUnmatched`.

## 7. Validation and limits

Every host MUST enforce, in addition to the 0.0.3 bounds:

- `requires` may contain `storage`; `ctx.store` MUST be absent without it.
- `capture.includeUnmatched` is a boolean when present.
- unknown `BrowserCaptureTask` fields (including the retired `paginate`)
  MUST be rejected, not ignored.
- store keys `1..=128` chars, values `<= 65536` bytes of JSON, `<= 128` keys.
- the `UnmatchedEnvelope` counts against the host payload byte cap as one
  payload.

## 8. Reference acceptance target

WEF 0.0.4 is ready to leave draft status when the reference implementation
has:

- a `ctx.store` conformance test proving values survive across operations in
  one engine and isolate across sources;
- a capture test proving `includeUnmatched` changes nothing unless set, and
  delivers rejected values when set;
- a reference source that bootstraps once over the browser and serves steady
  state over plain HTTP (browser as recovery only).

## 9. License

The WEF 0.0.4 specification is dual-licensed under MIT or Apache-2.0, at the
implementer's option. See `LICENSE-MIT` and `LICENSE-APACHE`.
