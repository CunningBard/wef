# Web Extension Format (WEF) 0.1.0

**Status:** Final
**Version:** 0.1.0
**Consolidates:** WEF 0.0.1, 0.0.2, 0.0.3, 0.0.4 (all prior documents are
superseded for sources declaring `"wef": "0.1.0"`; they remain valid
historical references for older declarations)

---

## 1. Abstract

Web Extension Format (WEF) is a portable format for describing how a manga
source exposes listings, search results, manga metadata, chapters, and
readable pages.

A WEF source package contains metadata and source logic. A WEF engine loads
the package, executes standardized operations, and returns standard WEF data
structures to a reader.

WEF defines the boundary between source logic and an engine. It does not
define how a reader, browser, HTTP stack, JavaScript runtime, storage layer,
or extension repository must be implemented.

WEF 0.1.0 uses a JSON manifest and an ECMAScript module as its
representation. This document is self-contained: unlike the 0.0.x drafts,
no prior document is needed to implement it.

---

## 2. Compatibility goal

WEF is designed so that an adapter can map its operations to the current
source models used by Mihon/Tachiyomi-style readers and Aidoku without
requiring every source to be rewritten around reader-specific concepts.

Compatibility means that:

- the same WEF source package can be executed by different WEF engines;
- an engine can translate WEF values into a reader's native source models;
- common source behavior has a direct or reasonable mapping;
- reader-specific optional features can be exposed through optional WEF
  capabilities.

Compatibility does **not** mean that existing Mihon or Aidoku builds can
install a WEF package without first implementing or embedding a WEF engine.

A 0.1.0 engine MUST accept manifests declaring `"wef": "0.0.1"` through
`"wef": "0.1.0"` according to each version's rules, and MUST reject an
unsupported manifest version rather than silently interpreting it as a
known one. `"wef": "0.1.0"` is semantically identical to `"0.0.4"`: 0.1.0
finalizes the draft chain into one document and changes nothing on the
wire.

---

## 3. Design principles

1. **Write source logic once.**
2. **Standardize operations and exchanged data, not engine internals.**
3. **Use opaque source keys rather than reader-owned identifiers.**
4. **Pass enough source context back into later operations.**
5. **Allow selective metadata and chapter updates.**
6. **Treat array order as authoritative.**
7. **Keep optional reader features optional.**
8. **Keep browser and challenge handling engine-defined.**
9. **Sources send data only; hosts own every line of executed
   host-side and page-side code.**

---

## 4. Terminology

### 4.1 WEF source

A package that describes one content source.

### 4.2 WEF engine

An implementation that loads and executes WEF sources.

### 4.3 Reader

An application that browses and reads content using a WEF engine.

### 4.4 Host API

Engine-provided functionality available to source logic, such as HTTP
requests, URL resolution, and HTML parsing.

### 4.5 Operation

A standardized exported source function.

### 4.6 Key

An opaque, source-owned string identifying a manga or chapter.

### 4.7 Capability

An optional operation or behavior declared by a source.

---

## 5. Conformance language

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY**
describe requirements.

---

## 6. Package layout

A WEF source is a directory or archive containing:

```text
source.wef/
├── wef.json
├── source.js
└── icon.png        # optional
```

A package:

- MUST contain `wef.json`;
- MUST contain its declared entry module;
- MAY contain assets;
- MUST NOT access package files through host filesystem APIs;
- MUST NOT depend on files outside the package.

Module imports resolve against the package root. Engines MUST reject an
import that escapes the root, including via symlinks: resolve the
specifier, canonicalize the result, and require the canonical path to
remain under the root. Archive encoding and repository distribution are
not standardized in 0.1.0.

---

## 7. Manifest

The manifest is a UTF-8 JSON file named `wef.json`.

### 7.1 Example

```json
{
  "wef": "0.1.0",
  "id": "multi.wef.magadex",
  "name": "MangaDex",
  "version": "0.1.0",
  "entry": "source.js",
  "languages": ["en"],
  "baseUrls": [
    "https://mangadex.org",
    "https://api.mangadex.org"
  ],
  "requires": ["http", "html", "browser", "image", "storage"],
  "listings": [
    { "id": "popular", "name": "Popular" },
    { "id": "latest", "name": "Latest" }
  ],
  "capabilities": {
    "filters": true,
    "settings": true,
    "urlResolution": true,
    "imageRequests": true,
    "imageTransforms": true,
    "migrations": false
  },
  "network": {
    "rateLimit": { "maxRequests": 5, "windowMs": 1000 }
  }
}
```

### 7.2 Required fields

#### `wef`

The WEF specification version. It MUST equal `"0.0.1"`, `"0.0.2"`,
`"0.0.3"`, `"0.0.4"`, or `"0.1.0"`. New sources SHOULD declare
`"0.1.0"`.

#### `id`

A stable source identifier.

It:

- MUST be non-empty;
- MUST contain only ASCII letters, digits, `.`, `-`, and `_`;
- SHOULD use lowercase reverse-domain notation;
- MUST remain stable across ordinary source updates.

#### `name`

A human-readable source name.

#### `version`

The source package version. Semantic versioning is RECOMMENDED.

#### `entry`

A package-relative path to the ECMAScript entry module. The entry MUST
resolve inside the package root after canonicalization.

#### `languages`

An array of BCP 47 language tags.

A reader that expects one source instance per language MAY expose one
logical source instance for each declared language.

#### `baseUrls`

Known website and API origins associated with the source. Engines MUST
use these origins for browser-visit policy: a browser run MUST target an
HTTP(S) URL on a listed origin, and session-authenticated follow-up
requests MUST also target listed origins (see section 19).

#### `requires`

Required host capabilities. Allowed values are:

- `http`
- `html`
- `browser`
- `image`
- `storage`

`browser`, `image`, and `storage` are host capabilities and MUST be
listed in `requires` before a source may use their context APIs.

### 7.3 Listings

`listings` is an array of source-defined browse listings:

```ts
interface Listing {
  id: string;
  name: string;
}
```

A source MUST declare at least one listing.

The canonical listing identifiers are:

- `popular`
- `latest`

A Mihon/Tachiyomi adapter SHOULD map `popular` and `latest` directly to
the reader's corresponding browse operations.

When a source has only one meaningful browse feed, it SHOULD declare it
as `popular`.

### 7.4 Capabilities

```ts
interface Capabilities {
  filters?: boolean;
  settings?: boolean;
  urlResolution?: boolean;
  imageRequests?: boolean;
  imageTransforms?: boolean;
  migrations?: boolean;
}
```

A capability set to `true` requires the corresponding optional exports
(`getFilters`/`getSettings`/`resolveUrl`/`getImageRequest`/
`transformImage`/migration functions). `imageTransforms` additionally
requires `"image"` in `requires`.

An omitted capability is `false`.

### 7.5 Network policy

```ts
interface NetworkPolicy {
  rateLimit?: {
    maxRequests: number;
    windowMs: number;
  };
}
```

`network.rateLimit` is declarative. Engines MUST treat it as an upper
bound, not as permission to exceed their own origin, repository, or host
policy. Engines MUST apply it to source HTTP, image candidate, and
browser-origin requests.

---

## 8. JavaScript execution profile

The entry module is an ECMAScript module.

The engine MUST support:

- ECMAScript 2020 syntax;
- modules (with the root jail of section 6);
- promises;
- `async` and `await`;
- standard JSON, arrays, objects, strings, numbers, dates, and regular
  expressions.

A source MUST NOT assume the presence of:

- Node.js modules;
- `require`;
- `process`;
- `Buffer`;
- filesystem APIs;
- `window`;
- `document`;
- global `fetch`;
- runtime-specific native objects.

All host functionality is accessed through `ctx`.

An engine MAY use any runtime that produces conforming behavior, and MUST
bound execution (loop iterations, wall-clock time) so a hostile or buggy
source cannot hang the host.

---

## 9. Entry module

A source exports the required operations:

```js
export async function getMangaList(ctx, input) {}
export async function search(ctx, input) {}
export async function getMangaUpdate(ctx, input) {}
export async function getPages(ctx, input) {}
```

Depending on declared capabilities, it MAY also export:

```js
export async function getFilters(ctx) {}
export async function getSettings(ctx) {}
export async function resolveUrl(ctx, input) {}
export async function getImageRequest(ctx, input) {}
export async function transformImage(ctx, input) {}
export async function migrateMangaKey(ctx, input) {}
export async function migrateChapterKey(ctx, input) {}
```

Every operation:

- MUST return a promise or be declared `async`;
- MUST receive the WEF context first;
- MUST accept and return JSON-compatible values unless otherwise
  specified (binary image data is the only exception, see section 16);
- MUST NOT expose engine-native objects.

---

## 10. Core operations

### 10.1 `getMangaList`

Retrieves one declared browse listing.

```ts
interface MangaListInput {
  listingId: string;
  page: number;
  filters?: Record<string, JsonValue>;
}

getMangaList(ctx, input: MangaListInput) -> MangaPage
```

Rules:

- `listingId` MUST match a manifest listing;
- `page` begins at `1`;
- `filters`, when present, carry the same leaf-value encoding as
  `SearchInput.filters` (section 12); absent means unfiltered;
- results MUST use source-defined order.

### 10.2 `search`

Searches for manga.

```ts
interface SearchInput {
  query: string | null;
  page: number;
  filters: Record<string, JsonValue>;
}

search(ctx, input: SearchInput) -> MangaPage
```

Rules:

- `page` begins at `1`;
- `query` MAY be `null` or empty;
- `filters` MUST be an object; it MUST be empty when no filters apply;
- unknown filter IDs SHOULD be ignored;
- a source that cannot perform an empty search MAY return an empty page.

### 10.3 `getMangaUpdate`

Selectively retrieves manga details, chapters, or both.

```ts
interface MangaUpdateInput {
  manga: Manga;
  chapters: Chapter[];
  fetchDetails: boolean;
  fetchChapters: boolean;
}

interface MangaUpdate {
  manga?: Manga;
  chapters?: Chapter[];
}

getMangaUpdate(ctx, input: MangaUpdateInput) -> MangaUpdate
```

Rules:

- at least one fetch flag MUST be `true`;
- the source SHOULD avoid duplicate requests when both are true;
- `manga` contains the reader's currently stored source data;
- `chapters` contains the reader's currently stored chapters and MAY be
  empty;
- when `fetchDetails` is true, the result MUST contain `manga`;
- when `fetchChapters` is true, the result MUST contain `chapters`;
- omitted result fields mean “not requested,” not “delete existing data.”

This combined operation exists because source sites often provide details
and chapters in one response.

### 10.4 `getPages`

Retrieves the readable pages of a chapter.

```ts
interface PagesInput {
  manga: Manga;
  chapter: Chapter;
}

getPages(ctx, input: PagesInput) -> Page[]
```

Rules:

- returned array order is reading order;
- page indices are not used to determine order;
- the complete manga and chapter are passed so sources can use keys,
  URLs, or opaque `extra` data;
- an empty array is valid for a chapter with no readable pages.

---

## 11. Optional operations

### 11.1 `getFilters`

Required when `capabilities.filters` is true.

```ts
getFilters(ctx) -> Filter[]
```

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
interface FilterOption { id: string; name: string; }
```

Rules:

- every leaf filter has a stable `id`, `name`, and optional default;
- setting and leaf-filter IDs MUST be unique, including nested groups;
- range and sort defaults MUST be valid;
- groups are semantic organization and namespace containers, not values
  passed to `SearchInput.filters`; the host MUST flatten selected leaf
  values into the `filters` record. A host that cannot present a control
  MAY obtain the value through another interface or omit it, but MUST
  NOT invent a different value encoding.

### 11.2 `getSettings`

Required when `capabilities.settings` is true. See section 13.

### 11.3 `resolveUrl`

Required when `capabilities.urlResolution` is true.

```ts
interface ResolveUrlInput {
  url: string;
}

type ResolvedUrl =
  | {
      type: "manga";
      mangaKey: string;
    }
  | {
      type: "chapter";
      mangaKey: string;
      chapterKey: string;
    }
  | {
      type: "listing";
      listingId: string;
    };

resolveUrl(ctx, input: ResolveUrlInput) -> ResolvedUrl | null
```

This operation allows a reader to open source website links directly.

### 11.4 `getImageRequest`

Required when `capabilities.imageRequests` is true.

```ts
interface ImageRequestInput {
  manga: Manga;
  chapter?: Chapter;
  page?: Page;
  url: string;
  context: "cover" | "chapter-thumbnail" | "page";
}

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

This allows a source to attach referer, authorization, or other
request-specific headers without requiring the reader to understand
source logic. The top-level request is attempted first; candidates are
attempted in order only after a retryable failure (default: HTTP 404,
410, or a transport error). An engine MUST enforce its redirect, origin,
request-count, and rate policies for every candidate, and MUST NOT retry
on an authentication or challenge error without explicit browser-session
recovery.

### 11.5 `transformImage`

Required when `capabilities.imageTransforms` is true. See section 16.

### 11.6 Key migration

Required when `capabilities.migrations` is true.

```ts
interface MangaKeyMigrationInput {
  key: string;
}

interface ChapterKeyMigrationInput {
  mangaKey: string;
  chapterKey: string;
}

migrateMangaKey(ctx, input: MangaKeyMigrationInput) -> string
migrateChapterKey(ctx, input: ChapterKeyMigrationInput) -> string
```

Keys SHOULD remain stable. Migration exists for unavoidable source URL or
identifier changes.

---

## 12. Data model rules

Every operation value MUST be representable as JSON:

```ts
type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };
```

Functions, symbols, cyclic objects, class instances, and runtime-native
objects MUST NOT be returned. (Binary image data crosses only the
dedicated transform boundary of section 16.)

Unknown optional fields MUST be ignored by engines.

---

## 13. Manga page

```ts
interface MangaPage {
  items: Manga[];
  hasNextPage: boolean;
}
```

Search and listing results MAY return partial manga records.

Each result MUST contain at least:

- `key`
- `title`

A cover URL SHOULD be included when available.

---

## 14. Manga

```ts
type MangaStatus =
  | "unknown"
  | "ongoing"
  | "completed"
  | "hiatus"
  | "cancelled"
  | "licensed"
  | "publishing-finished";

type ContentRating =
  | "unknown"
  | "safe"
  | "suggestive"
  | "nsfw";

type Viewer =
  | "unknown"
  | "left-to-right"
  | "right-to-left"
  | "vertical"
  | "webtoon";

type UpdateStrategy =
  | "always"
  | "never";

interface Manga {
  key: string;
  title: string;

  url?: string;
  coverUrl?: string;
  alternativeTitles?: string[];
  description?: string;
  authors?: string[];
  artists?: string[];
  tags?: string[];

  status?: MangaStatus;
  contentRating?: ContentRating;
  viewer?: Viewer;
  updateStrategy?: UpdateStrategy;
  nextUpdateAt?: string;

  extra?: Record<string, JsonValue>;
}
```

Rules:

- `key` is opaque and source-owned;
- `key` MUST be stable when reasonably possible;
- readers MUST NOT parse or alter `key`;
- `url`, when present, SHOULD be absolute;
- `nextUpdateAt`, when present, MUST be ISO 8601;
- missing metadata SHOULD be omitted rather than invented;
- `extra` MAY preserve source-specific state required by later
  operations;
- readers MUST round-trip `extra` unchanged;
- sources SHOULD keep `extra` small and JSON-compatible.

---

## 15. Chapter

```ts
interface Chapter {
  key: string;
  name: string;

  url?: string;
  title?: string;

  number?: string;
  numberValue?: number;

  volume?: string;
  volumeValue?: number;

  language?: string;
  publishedAt?: string;
  scanlators?: string[];

  thumbnailUrl?: string;
  locked?: boolean;

  extra?: Record<string, JsonValue>;
}
```

Rules:

- `key` and `name` are required;
- `name` is the display label;
- `number` and `volume` preserve source text;
- numeric companion fields SHOULD be included when safely parseable;
- `language` SHOULD use BCP 47;
- `publishedAt` MUST be ISO 8601;
- chapter array order is source-defined;
- readers MAY reorder chapters;
- readers MUST round-trip `extra` unchanged.

The dual string/numeric number fields avoid losing values such as
`10.5`, `Extra`, or source-specific numbering while still mapping
efficiently to readers that use numeric chapter values.

---

## 16. Page

```ts
interface Page {
  url?: string;
  imageUrl?: string;
  thumbnailUrl?: string;
  description?: string;
  headers?: Record<string, string>;
  context?: Record<string, JsonValue>;
}
```

Rules:

- at least one of `url` or `imageUrl` MUST be present;
- `imageUrl` is a directly readable image URL when known;
- `url` MAY identify an intermediate page or source endpoint used to
  resolve an image;
- all present URLs SHOULD be absolute;
- array order is authoritative;
- `headers` MAY contain per-image HTTP headers;
- `context` MAY be passed back to optional image-request behavior;
- readers MUST NOT depend on a numeric page index.

A source SHOULD return `imageUrl` directly whenever possible.

---

## 17. Operation context

```ts
interface WefContext {
  http?: HttpApi;
  html?: HtmlApi;
  browser?: BrowserApi;
  image?: ImageApi;
  store?: StoreApi;
  url: UrlApi;
  settings: Record<string, JsonValue>;
  fail(
    code: ErrorCode,
    message?: string,
    details?: JsonValue
  ): never;
}
```

`http`, `html`, `browser`, `image`, and `store` exist on `ctx` only when
the manifest lists the matching capability in `requires`. An engine MUST
provide every manifest-required capability, and MUST fail closed with
`UNSUPPORTED` when a declared capability cannot be served.

An engine MAY expose development extensions, but portable sources MUST
NOT depend on undeclared or non-standard functions.

---

## 18. HTTP API

A source requiring `http` receives `ctx.http`.

```ts
interface HttpApi {
  request(request: HttpRequest): Promise<HttpResponse>;
}

interface HttpRequest {
  method?: string;
  url: string;
  headers?: Record<string, string>;
  query?: Record<string, string | string[]>;
  body?: string;
  browserSession?: string;
}

interface HttpResponse {
  status: number;
  url: string;
  headers: Record<string, string>;
  body: string;
}
```

Rules:

- `method` defaults to `GET`;
- query values are encoded by the engine: strings append as-is, arrays
  repeat the key, anything else fails the request;
- `url` in the response is the final URL after redirects;
- response headers SHOULD use lowercase names;
- `body` is decoded text;
- JSON is parsed with `JSON.parse`;
- non-2xx statuses are returned, never thrown;
- `browserSession` authenticates with cookies minted by a prior
  `browser.run` (section 19); a host without browser state MUST reject
  such requests with `UNSUPPORTED`.

The engine defines:

- HTTP library;
- cookie persistence;
- redirects;
- caching;
- proxy support;
- TLS behavior;
- user agent;
- rate limiting;
- browser-assisted retry behavior.

A conforming engine SHOULD preserve cookies within a source session.

Binary source-operation responses are deferred. Reader image fetching is
not performed through this text response model.

---

## 19. Browser bridge

The browser API is for sites whose page JavaScript, client-side storage,
or anti-bot flow is necessary to obtain a source response. It is not a
general browser automation or CAPTCHA-solving API. Sources submit
data-only tasks; the host owns every line of JavaScript that runs in the
page. There is no script field: an engine MUST reject any browser request
carrying executable source strings.

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
  includeUnmatched?: boolean; // default false; see section 20
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
```

### 19.1 Matching predicate

Hosts MUST evaluate `BrowserCaptureSpec` against each observed parsed-JSON
response body with exactly this logic:

1. Walk `jsonPath` segment by segment through objects. If any segment is
   missing (or the current value is not an object), the body does not
   match.
2. If `requireNonEmpty` is true, reject `null`, empty arrays, and empty
   objects.
3. If `requireItemField` is set, the matched value MUST be a non-empty
   array with at least one object item containing that field.

`urlContains`, when set, additionally requires the response URL to
contain the substring. The site-specific part (which path, which
substring, which selector) is source data; the predicate itself is fixed
host code. Parsing and mapping the returned payload into manga/chapter/
page models stays in source JavaScript, inside the engine sandbox.

### 19.2 Payload shapes

- `snapshot`: `payload` is the parsed JSON text of `selector`, or `null`
  when the element is absent or unparseable.
- `capture`: `payload` is the first matching body (in observation order,
  deduplicated by exact JSON equality), else the `snapshot` fallback JSON
  when present, else `null`. With `includeUnmatched`, the envelope of
  section 20 applies instead.

### 19.3 Reference algorithm

Any language can reimplement the host with these observable steps. Task
JSON shapes are the compatibility contract; browser-driving techniques
(CDP domains, interception, fixed page hooks, DOM libraries) are
implementation choices:

1. Validate: require explicit user-visible consent for the source/version
   and destination origin; check `url` is HTTP(S) on a `baseUrls` origin;
   require `timeoutMs` within the host maximum; validate the task per
   section 23.
2. Open an ephemeral tab with a fresh profile (no user data, isolated per
   source identity and configuration profile). If `html` is set, load it
   with `url` as the base URL and skip network navigation.
3. `snapshot`: extract and return per section 19.2.
4. `capture`: observe network response bodies and parsed page values,
   keeping those matching section 19.1; dedupe exact-equal values.
5. Wait until the first match or `timeoutMs`, then return it; else try
   the `snapshot` fallback; else return `null`. Multi-page lists are the
   source's job: it signs plain requests and walks result pagination
   natively instead of driving page clicks.
6. Enforce the payload byte cap, close the tab even on error, and mint an
   opaque `session` usable only as `HttpRequest.browserSession` by the
   receiving source.

Engines MUST apply navigation, network-origin, CPU, memory, and
wall-clock limits, MUST NOT silently solve CAPTCHAs or bypass user
checkpoints, MAY offer only interactive recovery (`UNSUPPORTED` for
scripted capture), and MUST NEVER expose cookies, credentials, or
storage to source code.

### 19.4 Sessions

- An engine MUST isolate sessions by source identity and configuration
  profile.
- `session` is opaque and may only be used as
  `HttpRequest.browserSession` by the source that received it; tokens
  MUST be unpredictable across sources sharing one host.
- The collected cookie set spans the browser profile, so a
  session-authenticated request MUST target a policy (`baseUrls`) origin:
  the jar must never ride along to an arbitrary URL.
- `html`, when supplied, is loaded with `url` as its base URL. This
  permits safe inspection of already-fetched markup without an
  uncontrolled navigation.

An engine MAY provide only interactive browser recovery and return
`UNSUPPORTED` for scripted capture. A source requiring scripted capture
SHOULD report `CHALLENGE_REQUIRED` with a useful fallback message when it
cannot continue.

---

## 20. Unmatched observations and source storage

### 20.1 Unmatched envelope

With `includeUnmatched: true`, `payload` is instead:

```ts
interface UnmatchedEnvelope {
  matched: JsonValue;   // the normal section-19.2 payload for this task
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

### 20.2 Source storage

```ts
interface StoreApi {
  get(key: string): JsonValue | null;
  set(key: string, value: JsonValue): boolean;
}
```

`storage` is a host capability like `http`: it MUST be listed in
`requires` before a source may use `ctx.store`. A host that cannot persist
across restarts MAY keep storage in memory, but MUST keep it for the
lifetime of the source session. A host that provides no storage at all
MUST fail closed with `UNSUPPORTED` when a `storage`-requiring source
uses `ctx.store`.

Rules:

- Keys are `1..=128` chars. Values are JSON, `<= 65536` bytes serialized,
  at most 128 keys per source. Violations fail the operation.
- Storage is scoped to one source identity and configuration profile —
  the same scoping as cookies, browser sessions, and settings. Sources
  MUST NOT expect another source's values, and MUST treat a missing key
  (`null`) as "bootstrap again".
- Hosts MUST redact store contents from diagnostics, logs, and fixtures
  like secret settings (bootstrap material is credential-equivalent).
- `set` returns `true` on success; `set(key, null)` deletes the key.
  Hosts MAY persist to disk or keep memory only; sources MUST handle cold
  starts on every operation.
- The reference pattern for rotation: on upstream rejection, the source
  clears the entry and re-bootstraps, then retries once — the same state
  machine as dropping a stale cookie.

---

## 21. URL API

```ts
interface UrlApi {
  resolve(base: string, value: string): string;
}
```

`resolve` converts relative and protocol-relative URLs into absolute URLs.

---

## 22. HTML API

A source requiring `html` receives `ctx.html`.

```ts
interface HtmlApi {
  parse(source: string): HtmlDocument;
}

interface HtmlDocument {
  select(selector: string): HtmlElement | null;
  selectAll(selector: string): HtmlElement[];
}

interface HtmlElement {
  select(selector: string): HtmlElement | null;
  selectAll(selector: string): HtmlElement[];
  text(): string;
  html(): string;
  attr(name: string): string | null;
}
```

This is a parser interface, not a browser DOM.

Engines MUST support:

- type selectors;
- class selectors;
- ID selectors;
- descendant combinators;
- child combinators;
- attribute presence selectors;
- exact attribute value selectors;
- `:first-child`;
- `:last-child`;
- `:nth-child()`.

`text()` MUST decode HTML entities.

Whitespace normalization is implementation-defined. Sources SHOULD call
`.trim()` and MUST NOT depend on exact internal whitespace.

---

## 23. Settings

Settings are host-supplied configuration values scoped to one source
identity and configuration profile. A host MAY persist them, prompt for
them, receive them from a command line, or inject them from another
application; that storage and presentation is outside WEF. Source code
never receives a filesystem, preferences object, or another source's
values.

```ts
interface WefContext {
  settings: Record<string, JsonValue>;
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
- hosts MUST merge absent values with defaults before exposing
  `ctx.settings`;
- a `secret` setting MUST be redacted from diagnostics, fixtures, logs,
  and exported source state;
- settings affect only later source invocations; `getSettings` MUST NOT
  mutate them;
- source packages MUST NOT read or write settings except through this
  context.

---

## 24. Image requests and transforms

`getImageRequest` may return one request or ordered candidates (section
11.4). An image-transform operation is an internal binary boundary;
unlike normal operations, its byte fields are `ArrayBuffer` values and
MUST NOT be serialized into ordinary operation output or fixtures.

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

`ImageBitmap` is an opaque host object. It MUST only be accepted by
`ctx.image` methods and MUST NOT cross normal JSON operation boundaries.
Engines MUST set maximum input bytes, decoded pixels, output bytes, and
transform duration. They MUST fail with `UNSUPPORTED` when the requested
codec is unavailable.

---

## 25. Errors

```ts
type ErrorCode =
  | "BAD_INPUT"
  | "NOT_FOUND"
  | "HTTP_ERROR"
  | "INVALID_RESPONSE"
  | "AUTH_REQUIRED"
  | "RATE_LIMITED"
  | "CHALLENGE_REQUIRED"
  | "UNSUPPORTED"
  | "SOURCE_ERROR";
```

A source reports an error with:

```js
ctx.fail("NOT_FOUND", "Manga was not found");
```

Unexpected source exceptions MUST become `SOURCE_ERROR`.

`CHALLENGE_REQUIRED` tells the engine that normal HTTP access was
insufficient. `UNSUPPORTED` covers denied capabilities, codecs, policy,
and scripted-capture refusal.

---

## 26. Validation and limits

Task and manifest validation is part of the contract; every host MUST
enforce the same bounds:

- `id`: non-empty ASCII letters, digits, `.`, `-`, `_`.
- `entry`: package-relative, resolving inside the package root after
  canonicalization.
- at least one listing; `listingId` inputs MUST match a manifest listing.
- `requires` entries unique and known; `browser`/`image`/`storage` gate
  their context APIs (`ctx.browser` etc. MUST be absent without them).
- capability/export consistency (`settings` requires `getSettings`,
  `imageTransforms` requires `transformImage` plus `"image"`, and so on).
- unique setting and leaf-filter IDs, including nested groups; valid
  range and sort defaults; a valid positive rate-limit policy.
- `selector` fields: 1..=256 chars, no NUL or ASCII control characters
  (other than space).
- `capture.urlContains`: 1..=256 chars, no NUL or control characters.
- `capture.jsonPath`: 1..=8 dot-separated segments, each 1..=64 chars of
  `[A-Za-z0-9_$]`. `requireItemField` follows the single-segment rule.
- `capture.includeUnmatched` is a boolean when present.
- unknown `BrowserCaptureTask` fields (including the retired `paginate`)
  MUST be rejected, not ignored.
- `timeoutMs`: within the host maximum (reference: 30000).
- returned payloads (including `UnmatchedEnvelope` as one payload) are
  capped by host byte policy (reference: 5 MiB JSON).
- store keys `1..=128` chars, values `<= 65536` bytes of JSON,
  `<= 128` keys per source.
- a validator MUST reject any source file sending executable strings to
  `ctx.browser.run`.

---

## 27. Engine requirements

A conforming engine MUST:

1. load and validate `wef.json`, rejecting unsupported versions;
2. load the entry module, jailing imports to the package root;
3. provide required host capabilities;
4. invoke operations using the standard signatures;
5. safely validate or consume results;
6. convert unexpected exceptions into WEF errors;
7. scope keys, cookies, sessions, settings, storage, and image buffers to
   their source identity and configuration profile;
8. preserve opaque `extra` values when round-tripped;
9. use array order for chapters and pages;
10. avoid exposing native host objects;
11. enforce capability, consent, origin, byte, pixel, CPU, and timeout
    limits;
12. redact secret settings, opaque session IDs, and store contents from
    logs and diagnostics;
13. apply `network.rateLimit` to source HTTP, image candidate, and
    browser-origin requests;
14. bound script execution so a hostile source cannot hang the host.

An engine MAY:

- be written in any language;
- use any JavaScript runtime;
- compile source logic ahead of time;
- cache requests or results;
- enforce repository-specific policy;
- expose WEF through FFI, IPC, HTTP, or an in-process API;
- translate WEF values into native reader models;
- provide browser-assisted recovery after HTTP challenges.

An engine MUST NOT require a source to know which reader or engine is
executing it.

---

## 28. Adapter mapping

This section is non-normative.

| WEF operation or model | Mihon/Tachiyomi-style mapping | Aidoku mapping |
|---|---|---|
| `getMangaList` with `popular` | popular manga operation | listing provider |
| `getMangaList` with `latest` | latest updates operation | listing provider |
| `search` | search manga operation | search manga list |
| `getMangaUpdate` | selective manga update | selective manga update |
| `getPages(manga, chapter)` | page list; manga may be ignored | page list with manga and chapter |
| `getFilters` | native source filters | dynamic or configured filters |
| `resolveUrl` | URL search/deep-link routing | deep-link handler |
| `getImageRequest` | image headers/request customization | image request provider |
| migration operations | adapter-managed source migration | migration handler |
| `Manga.extra` / `Chapter.extra` | source memo or adapter state | opaque adapter/source state |
| page array order | page list order | page list order |

An adapter MAY omit unsupported optional fields.

An adapter MUST preserve opaque keys and `extra` data as far as the
reader's storage model allows.

---

## 29. Reader responsibilities

WEF does not define:

- source installation UI;
- library and history storage;
- downloads;
- chapter sorting;
- viewer implementation;
- extension repository governance;
- browser UI;
- cookie management UI;
- authentication UI;
- error presentation;
- migration UI;
- update scheduling.

Readers MAY map WEF data into native models and MAY ignore optional fields
they cannot represent. Readers MUST treat source output (descriptions,
synopses, tags) as untrusted markup.

---

## 30. Repository and trust policy

WEF does not define repository governance or a mandatory sandbox.
Trust-on-install applies: a source receives its declared capabilities'
data (including secret settings), so repositories and users MUST install
sources they trust.

A repository MAY:

- manually review sources;
- restrict dependencies;
- enforce network origins;
- sign packages;
- reject obfuscated code;
- require tests;
- control source ownership;
- remove unmaintained sources.

An engine MAY impose additional restrictions.

---

## 31. Extension repositories

A repository hosts many sources for discovery, installation, and update.
This section standardizes the machine-read paths; everything else in a
repository is free-form.

```text
<repo>/
├── repo.json          # REQUIRED: repository metadata (below)
├── index.json         # GENERATED catalog of sources (below)
├── sources/
│   ├── en.wef.example/
│   │   ├── wef.json
│   │   ├── source.js
│   │   └── res/icon.png   # conventional, optional (below)
│   └── multi.wef.demo/
└── README.md              # OPTIONAL: readers MAY show it as description
```

### 31.1 Source identity

A repository-hosted source directory MUST be named exactly its manifest
`id`, and the `id` MUST match:

```text
^(?:[a-z]{2,3}|multi)\.wef\.[a-z0-9][a-z0-9-]*$
```

a language tag (`en`, `ja`, …) or `multi` for multi-language sources,
the fixed `wef` infix, and a lowercase site slug. Versions of
repository-hosted sources MUST be semantic versions so readers can
compare them for updates. Enforcement belongs to repository tooling and
review, not the engine, which stays agnostic to `id` shape beyond
section 26.

### 31.2 Repository metadata

`repo.json` is REQUIRED at the repository root:

```ts
interface RepoMetadata {
  name: string;         // display name, the only required field
  description?: string;
  icon?: string;        // relative path to a display icon
  website?: string;
  version?: number;     // integer bumped on any change; cheap poll target
}
```

Readers MUST ignore unknown `repo.json` fields.

### 31.3 Source catalog

`index.json` is GENERATED by repository tooling, never hand-edited:

```ts
interface RepoIndex {
  version?: number;
  sources: RepoSource[];
}
interface RepoSource {
  id: string;
  version: string;
  path: string;     // source directory, relative to the repository root
  files: string[];  // closed reachable file set, relative to `path`
}
```

Update flow: readers poll `index.json`, compare per-source versions
against installed copies, and fetch the listed `files` for changed
sources. There is no build step: WEF sources are plain JavaScript,
fetch-and-run.

### 31.4 Freedom and restraint

Readers MUST ignore repository files they do not recognize — root
extras, per-source extras (`fixtures/`, `scripts/`, `README.md`), and
unknown catalog fields. That rule is what lets repositories evolve
without breaking readers, and lets readers stay minimal without
restraining repositories.

Display assets follow the same principle with zero normative weight: a
source MAY ship `res/icon.png`, which readers MAY fetch (relative to
the source root) for lists and descriptions. It is NOT declared in
`wef.json`, NOT validated, and its absence is never an error.

Explicitly NOT standardized: packaging beyond plain files, signing,
review policy, icon formats, screenshots, per-source changelogs, scaffolds,
and generator templates. Those are repository policy.

---

## 32. Deferred features

The following are intentionally deferred:

- arbitrary home-page layouts;
- custom reader UI;
- login protocols;
- binary HTTP responses in source operations;
- archive and text pages;
- alternate covers;
- notifications;
- dynamic listings;
- dynamic base URLs;
- package signatures;
- repository indexes;
- source dependencies;
- WebAssembly representation;
- declarative selector-only representation;
- localization bundles.

---

## 33. License

The WEF 0.1.0 specification is dual-licensed under MIT or Apache-2.0, at
the implementer's option. See `LICENSE-MIT` and `LICENSE-APACHE`.
