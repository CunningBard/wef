# Web Extension Format (WEF) 0.0.3

**Status:** Experimental compatibility draft
**Version:** 0.0.3
**Supersedes:** WEF 0.0.2 for sources declaring `"wef": "0.0.3"`

## 1. Scope and compatibility

WEF 0.0.3 retains the 0.0.2 package layout, manifest fields, four core
operations, data models, HTTP API, HTML API, URL API, settings, filters,
image requests/transforms, error model, and network policy. A 0.0.3 engine
MUST reject an unsupported manifest version rather than silently
interpreting it as an earlier version.

The single change from 0.0.2 is the browser bridge: free-form
`initializationScript`/`script` strings are removed. Sources submit
data-only tasks; the host owns every line of JavaScript that runs in the
page. This closes the arbitrary-code-execution channel that 0.0.2 gave to
source packages while keeping every real capture pattern (network sniffing,
embedded-JSON fallback, click pagination).

```ts
interface Manifest {
  wef: "0.0.3";
  // All other 0.0.2 fields unchanged.
}
```

## 2. Browser tasks

```ts
interface BrowserRunInput {
  url: string;
  html?: string;
  timeoutMs?: number;
  task: BrowserTask;
}
interface BrowserRunResult {
  url: string;
  payload?: JsonValue;
  session: string;
}
interface BrowserApi {
  run(input: BrowserRunInput): Promise<BrowserRunResult>;
}

type BrowserTask = BrowserSnapshotTask | BrowserCaptureTask;

interface BrowserSnapshotTask {
  kind: "snapshot";
  selector: string; // CSS selector, e.g. "script#initial-data"
}

interface BrowserCaptureTask {
  kind: "capture";
  capture: BrowserCaptureSpec;
  snapshot?: BrowserSnapshotSpec; // DOM fallback when nothing is captured
  paginate?: BrowserPaginateSpec; // click-pagination loop
}

interface BrowserCaptureSpec {
  urlContains?: string; // substring match on the response URL
  jsonPath: string; // dot path, e.g. "result.items"
  requireNonEmpty?: boolean; // matched value must not be empty
  requireItemField?: string; // matched array must contain an item with this field
}

interface BrowserSnapshotSpec {
  selector: string;
}

interface BrowserPaginateSpec {
  nextSelector: string; // CSS selector matching pagination buttons
  maxSteps: number; // 1..=50
  settleMs?: number; // 0..=2000, default 500
  nextLabelContains?: string; // default "next", case-insensitive
}
```

`BrowserRunInput.script` and `BrowserRunInput.initializationScript` do not
exist in 0.0.3. An engine MUST reject a browser request carrying either
field, and a 0.0.3 source MUST NOT send them.

### 2.1 Examples

Browse with API sniffing and embedded-JSON fallback:

```json
{
  "url": "https://example.org/browse?page=1",
  "timeoutMs": 10000,
  "task": {
    "kind": "capture",
    "capture": {"urlContains": "/api/v1/manga", "jsonPath": "result.items", "requireNonEmpty": true},
    "snapshot": {"selector": "script#initial-data"}
  }
}
```

Chapter list with click pagination (returns an array of response bodies):

```json
{
  "url": "https://example.org/title/demo",
  "timeoutMs": 25000,
  "task": {
    "kind": "capture",
    "capture": {"jsonPath": "result.items", "requireNonEmpty": true, "requireItemField": "number"},
    "paginate": {"nextSelector": ".mchap-foot button", "maxSteps": 50}
  }
}
```

Embedded JSON only, no network observation:

```json
{
  "url": "https://example.org/title/demo",
  "task": {"kind": "snapshot", "selector": "script#__NEXT_DATA__"}
}
```

## 3. Matching predicate

Hosts MUST evaluate `BrowserCaptureSpec` against each observed parsed-JSON
response body with exactly this logic:

1. Walk `jsonPath` segment by segment through objects. If any segment is
   missing (or the current value is not an object), the body does not match.
2. If `requireNonEmpty` is true, reject `null`, empty arrays, and empty
   objects.
3. If `requireItemField` is set, the matched value MUST be a non-empty array
   with at least one object item containing that field.

`urlContains`, when set, additionally requires the response URL to contain
the substring. The site-specific part (which path, which substring, which
selector) is source data; the predicate itself is fixed host code. Parsing
and mapping the returned payload into manga/chapter/page models stays in
source JavaScript, inside the engine sandbox.

## 4. Payload shapes

- `snapshot`: `payload` is the parsed JSON text of `selector`, or `null`
  when the element is absent or unparseable.
- `capture` without `paginate`: `payload` is the first matching body, else
  the `snapshot` fallback JSON when present, else `null`.
- `capture` with `paginate`: `payload` is the array of matching bodies in
  capture order (deduplicated by exact JSON equality, possibly empty). When
  empty and a `snapshot` fallback is present, `payload` is a one-element
  array holding the snapshot JSON. Sources flatten and map the array; the
  host never interprets site formats.

## 5. Pagination semantics

With `paginate`, the host repeats up to `maxSteps` times, within the overall
`timeoutMs` deadline:

1. Wait `settleMs` for pending responses to arrive.
2. Collect enabled, visible elements matching `nextSelector`. If none, stop.
3. Pick the element whose accessible name (`aria-label`, `title`, or text
   content) contains `nextLabelContains` (default `"next"`,
   case-insensitive). If exactly one element matched and no name matches,
   pick it. Otherwise stop (never click an ambiguous button).
4. Click it and continue.

## 6. Reference algorithm

Any language can reimplement the host with these observable steps. The JSON
shapes above are the compatibility contract; CDP method names, hook
techniques (network interception or fixed page hooks), and DOM libraries
are implementation choices:

1. Validate: require explicit user-visible consent for the source/version
   and destination origin; check `url` is HTTP(S) on a `baseUrls` origin;
   require `timeoutMs` within the host maximum; validate the task per
   section 8.
2. Open an ephemeral tab with a fresh profile (no user data, isolated per
   source identity and configuration profile). If `html` is set, load it
   with `url` as the base URL and skip network navigation.
3. `snapshot`: extract and return per section 4.
4. `capture`: observe network response bodies and parsed JSON values,
   keeping those matching section 3; dedupe exact-equal values.
5. Without `paginate`: wait for the first match up to `timeoutMs`, else
   snapshot fallback, else `null`.
6. With `paginate`: run the section 5 loop, then return per section 4.
7. Enforce a payload byte cap, close the tab even on error, mint an opaque
   `session` usable only as `HttpRequest.browserSession` by the receiving
   source, and never expose cookies, credentials, or storage to source code.

Engines MUST apply navigation, network-origin, CPU, memory, and wall-clock
limits, MUST NOT silently solve CAPTCHAs or bypass user checkpoints, and
MAY offer only interactive recovery (`UNSUPPORTED` for scripted capture).

## 7. Migration from 0.0.2

- Delete `initializationScript`/`script` from every `browser.run` call.
- Express each call as a `task`: network sniffing becomes `capture`,
  `script#initial-data` (or `__NEXT_DATA__`, etc.) reads become `snapshot`
  or the `snapshot` fallback, auto-click loops become `paginate`.
- Move response-shape handling (`queries` maps, `meta`/`pagination`,
  envelope unwrapping) into source helpers operating on `result.payload`.
- A `capture` whose `jsonPath` never matches now yields `null` (or `[]`
  with `paginate`) instead of hanging until the script's own deadline;
  sources MUST handle those cases as "no data", not as errors.

## 8. Validation and limits

Task validation is part of the contract; every host MUST enforce the same
bounds:

- `selector`, `snapshot.selector`, `nextSelector`: 1..=256 chars, no NUL or
  ASCII control characters (other than space).
- `urlContains`, `nextLabelContains`: 1..=256 chars, no NUL or control
  characters.
- `jsonPath`: 1..=8 dot-separated segments, each 1..=64 chars of
  `[A-Za-z0-9_$]`. `requireItemField` follows the single-segment rule.
- `paginate.maxSteps`: 1..=50. `paginate.settleMs`: 0..=2000.
- `timeoutMs`: within the host maximum (reference: 30000).
- Returned payloads are capped by host byte policy (reference: 5 MiB JSON).

A 0.0.3 validator MUST additionally check that no source file sends
`script`/`initializationScript` to `ctx.browser.run`. The existing 0.0.2
error codes apply unchanged: `UNSUPPORTED`, `RATE_LIMITED`, and
`CHALLENGE_REQUIRED` cover denied tasks, policy, and user interaction.

## 9. License

The WEF 0.0.3 specification is dual-licensed under MIT or Apache-2.0, at the
implementer's option. See `LICENSE-MIT` and `LICENSE-APACHE`.
