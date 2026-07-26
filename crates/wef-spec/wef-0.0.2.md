# Web Extension Format (WEF) 0.0.2

**Status:** Experimental compatibility draft
**Version:** 0.0.2
**Supersedes:** WEF 0.0.1 for sources declaring `"wef": "0.0.2"`

## 1. Scope and compatibility

WEF 0.0.2 retains the 0.0.1 package layout, manifest fields, four core
operations, data models, HTTP API, HTML API, URL API, error model, and
optional URL-resolution/key-migration operations. A 0.0.2 engine MUST reject
an unsupported manifest version rather than silently interpreting it as 0.0.1.

This version adds the minimum portable surfaces needed by real-world HTML
sources such as Comix:

- host-supplied, source-scoped configuration;
- expressive search filters;
- image request candidates and binary image transforms;
- an opt-in browser session bridge; and
- source-declared network rate policy.

The new browser and image capabilities are privileged. An engine MAY decline
them by policy; a source MUST handle the resulting `UNSUPPORTED` error.

## 2. Manifest additions

```ts
interface Manifest {
  wef: "0.0.2";
  // Existing 0.0.1 fields omitted.
  requires: ("http" | "html" | "browser" | "image")[];
  capabilities?: {
    filters?: boolean;
    settings?: boolean;
    urlResolution?: boolean;
    imageRequests?: boolean;
    imageTransforms?: boolean;
    migrations?: boolean;
  };
  network?: NetworkPolicy;
}

interface NetworkPolicy {
  rateLimit?: {
    maxRequests: number;
    windowMs: number;
  };
}
```

`settings` requires `getSettings`. `imageTransforms` requires
`transformImage` and also requires `"image"`. `browser` and `image` are host
capabilities and MUST be listed in `requires` before a source may use their
context APIs.

`network.rateLimit` is declarative. Engines MUST treat it as an upper bound,
not as permission to exceed their own origin, repository, or host policy.

## 3. Settings

Settings are host-supplied configuration values scoped to one source identity
and configuration profile. A host MAY persist them, prompt for them, receive
them from a command line, or inject them from another application; that storage
and presentation is outside WEF. Source code never receives a filesystem,
preferences object, or another source's values.

```ts
interface WefContext {
  settings: Record<string, JsonValue>;
  browser?: BrowserApi;
  image?: ImageApi;
  // Existing context fields omitted.
}

getSettings(ctx) -> Setting[]

type Setting =
  | { id: string; name: string; type: "text"; default?: string; secret?: boolean }
  | { id: string; name: string; type: "toggle"; default?: boolean }
  | { id: string; name: string; type: "select"; options: FilterOption[]; default?: string }
  | { id: string; name: string; type: "multi-select"; options: FilterOption[]; default?: string[] };
```

Rules:

- setting IDs MUST be unique and stable;
- hosts MUST merge absent values with defaults before exposing `ctx.settings`;
- a `secret` setting MUST be redacted from diagnostics, fixtures, logs, and
  exported source state;
- settings affect only later source invocations; `getSettings` MUST NOT mutate
  them;
- source packages MUST NOT read or write settings except through this context.

## 4. Rich filters

`getFilters` remains capability-gated, but replaces the 0.0.1 minimal model.
Every leaf filter has a stable `id`, `name`, and optional default. Every option
uses `{ value, label }`.

```ts
type Filter =
  | FilterGroup
  | FilterText | FilterToggle | FilterSelect | FilterMultiSelect
  | FilterTriState | FilterRange | FilterSort;

interface FilterGroup {
  type: "group";
  id: string;
  name: string;
  children: Filter[];
  presentation?: "section" | "inline";
}
interface FilterText { type: "text"; id: string; name: string; default?: string; placeholder?: string; }
interface FilterToggle { type: "toggle"; id: string; name: string; default?: boolean; }
interface FilterSelect { type: "select"; id: string; name: string; options: FilterOption[]; default?: string; }
interface FilterMultiSelect { type: "multi-select"; id: string; name: string; options: FilterOption[]; default?: string[]; }
interface FilterTriState { type: "tri-state"; id: string; name: string; options: FilterOption[]; default?: Record<string, "include" | "exclude" | "neutral">; }
interface FilterRange { type: "range"; id: string; name: string; min?: number; max?: number; step?: number; default?: { min?: number; max?: number }; }
interface FilterSort { type: "sort"; id: string; name: string; options: FilterOption[]; default?: { value: string; direction: "asc" | "desc" }; }
interface FilterOption { value: string; label: string; }
```

Groups are semantic organization and namespace containers, not values passed
to `SearchInput.filters`. The host MUST flatten selected leaf values into the
existing `filters` record. A host that cannot present a control MAY obtain the
value through another interface or omit it, but MUST NOT invent a different
value encoding.

## 5. Image requests and candidates

`getImageRequest` may return one request or ordered candidates:

```ts
interface ImageRequest {
  url: string;
  headers?: Record<string, string>;
  candidates?: ImageRequestCandidate[];
}
interface ImageRequestCandidate {
  url: string;
  headers?: Record<string, string>;
}
```

The top-level request is attempted first. Candidates are attempted in order
only after a retryable failure (default: HTTP 404, 410, or a transport error).
An engine MUST enforce its redirect, origin, request-count, and rate policies
for every candidate. It MUST NOT retry on an authentication or challenge error
without explicit browser-session recovery.

## 6. Binary image transforms

An image-transform operation is an internal binary boundary; unlike normal
operations, its byte fields are `ArrayBuffer` values and MUST NOT be serialized
into ordinary operation output or fixtures.

```ts
interface ImageTransformInput {
  request: ImageRequest;
  page: Page;
  status: number;
  headers: Record<string, string>;
  mimeType?: string;
  body: ArrayBuffer;
}
interface ImageTransformOutput {
  mimeType: string;
  body: ArrayBuffer;
}

transformImage(ctx, input: ImageTransformInput) -> Promise<ImageTransformOutput>
```

Sources receive `ctx.image` only with `requires: ["image"]`:

```ts
interface ImageApi {
  decode(bytes: ArrayBuffer): Promise<ImageBitmap>;
  create(width: number, height: number): ImageBitmap;
  blit(target: ImageBitmap, source: ImageBitmap, sourceRect: Rect, targetRect: Rect): void;
  encode(image: ImageBitmap, mimeType: "image/jpeg" | "image/png" | "image/webp", quality?: number): Promise<ArrayBuffer>;
}
interface Rect { x: number; y: number; width: number; height: number; }
```

`ImageBitmap` is an opaque host object. It MUST only be accepted by `ctx.image`
methods and MUST NOT cross normal JSON operation boundaries. Engines MUST set
maximum input bytes, decoded pixels, output bytes, and transform duration.
They MUST fail with `UNSUPPORTED` when the requested codec is unavailable.

## 7. Browser sessions

The browser API is for sites whose page JavaScript, client-side storage, or
anti-bot flow is necessary to obtain a source response. It is not a general
browser automation or CAPTCHA-solving API.

```ts
interface BrowserRunInput {
  url: string;
  html?: string;
  initializationScript?: string;
  script: string;
  timeoutMs?: number;
}
interface BrowserRunResult {
  url: string;
  payload?: JsonValue;
  session: string;
}
interface BrowserApi {
  run(input: BrowserRunInput): Promise<BrowserRunResult>;
}

interface HttpRequest {
  // Existing fields omitted.
  browserSession?: string;
}
```

Rules:

- `url` MUST be an HTTP(S) origin listed in `baseUrls`, after redirects;
- engines MUST obtain explicit, user-visible consent before first browser use
  for a source/version and show the destination origin;
- an engine MUST isolate sessions by source identity and configuration profile
  and MUST NOT expose
  cookies, credentials, or storage values to source code;
- `session` is opaque and may only be used as `HttpRequest.browserSession` by
  the source that received it;
- scripts execute in the page's sandboxed browser context, not the engine or
  host process; engines MUST apply navigation, network-origin, CPU, memory,
  and wall-clock limits;
- engines MUST NOT silently solve CAPTCHAs or bypass user checkpoints;
- `html`, when supplied, is loaded with `url` as its base URL. This permits
  safe inspection of already-fetched markup without an uncontrolled navigation.

An engine MAY provide only interactive browser recovery and return
`UNSUPPORTED` for scripted capture. A source requiring scripted capture SHOULD
report `CHALLENGE_REQUIRED` with a useful fallback message when it cannot
continue.

## 8. Errors, validation, and engine requirements

0.0.2 adds no error codes. `UNSUPPORTED`, `RATE_LIMITED`, and
`CHALLENGE_REQUIRED` cover denied codecs, browser capability, policy, and user
interaction.

A 0.0.2 validator MUST additionally check:

- capability/export consistency for settings and image transforms;
- unique setting and leaf-filter IDs, including nested groups;
- valid range and sort defaults;
- a valid positive rate-limit policy;
- browser/image usage only when the matching required capability is declared;
- image transform fixtures by a binary fixture format that stores hashes or
  named binary blobs, never inline secret/session data.

A conforming engine MUST:

- preserve existing 0.0.1 behavior for a 0.0.1 package it claims to support;
- keep settings, browser sessions, cookie jars, and image buffers scoped to a
  source identity and configuration profile;
- enforce capability, consent, origin, byte, pixel, CPU, and timeout limits;
- redact secret settings and opaque browser session IDs from logs; and
- apply `network.rateLimit` to source HTTP, image candidate, and browser-origin
  requests.

## 9. Migration notes

0.0.1 sources remain valid only when loaded by a 0.0.1-compatible engine mode.
To adopt 0.0.2, a source changes `wef` to `"0.0.2"` and declares only the new
capabilities it uses. A source that uses only richer filters/settings needs no
browser or image permission.

## 10. Reference acceptance target

WEF 0.0.2 is ready to leave draft status when the reference implementation has:

- settings and rich-filter conformance fixtures;
- an origin- and consent-gated browser-session mock plus integration tests;
- byte/XOR and grid-descramble image-transform fixtures with strict limits;
- image candidate fallback tests;
- rate-limit tests; and
- a Comix-derived reference source that exercises all newly standardized
  surfaces without embedding secrets or live browser fixtures.

## 11. License

The WEF 0.0.2 specification is dual-licensed under MIT or Apache-2.0, at the
implementer's option. See `LICENSE-MIT` and `LICENSE-APACHE`.
