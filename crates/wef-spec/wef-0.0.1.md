# Web Extension Format (WEF) 0.0.1

**Status:** Experimental compatibility draft  
**Version:** 0.0.1  
**Compatibility targets:** Mihon/Tachiyomi-style source APIs and Aidoku source APIs

---

## 1. Abstract

Web Extension Format (WEF) is a portable format for describing how a manga source exposes listings, search results, manga metadata, chapters, and readable pages.

A WEF source package contains metadata and source logic. A WEF engine loads the package, executes standardized operations, and returns standard WEF data structures to a reader.

WEF defines the boundary between source logic and an engine. It does not define how a reader, browser, HTTP stack, JavaScript runtime, storage layer, or extension repository must be implemented.

WEF 0.0.1 uses a JSON manifest and an ECMAScript module as its initial representation. This representation is provisional.

---

## 2. Compatibility goal

WEF 0.0.1 is designed so that an adapter can map its operations to the current source models used by Mihon/Tachiyomi-style readers and Aidoku without requiring every source to be rewritten around reader-specific concepts.

Compatibility means that:

- the same WEF source package can be executed by different WEF engines;
- an engine can translate WEF values into a reader's native source models;
- common source behavior has a direct or reasonable mapping;
- reader-specific optional features can be exposed through optional WEF capabilities.

Compatibility does **not** mean that existing Mihon or Aidoku builds can install a WEF package without first implementing or embedding a WEF engine.

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
9. **Keep the initial format small enough to implement.**

---

## 4. Terminology

### 4.1 WEF source

A package that describes one content source.

### 4.2 WEF engine

An implementation that loads and executes WEF sources.

### 4.3 Reader

An application that browses and reads content using a WEF engine.

### 4.4 Host API

Engine-provided functionality available to source logic, such as HTTP requests, URL resolution, and HTML parsing.

### 4.5 Operation

A standardized exported source function.

### 4.6 Key

An opaque, source-owned string identifying a manga or chapter.

### 4.7 Capability

An optional operation or behavior declared by a source.

---

## 5. Conformance language

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** describe requirements.

WEF 0.0.1 is experimental. Later versions may make breaking changes.

---

## 6. Package layout

A WEF 0.0.1 source is a directory or archive containing:

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

Archive encoding and repository distribution are not standardized in 0.0.1.

---

## 7. Manifest

The manifest is a UTF-8 JSON file named `wef.json`.

### 7.1 Example

```json
{
  "wef": "0.0.1",
  "id": "org.mangadex",
  "name": "MangaDex",
  "version": "0.0.1",
  "entry": "source.js",
  "languages": ["en"],
  "baseUrls": [
    "https://mangadex.org",
    "https://api.mangadex.org"
  ],
  "requires": ["http"],
  "listings": [
    {
      "id": "popular",
      "name": "Popular"
    },
    {
      "id": "latest",
      "name": "Latest"
    }
  ],
  "capabilities": {
    "filters": false,
    "urlResolution": true,
    "imageRequests": false,
    "migrations": false
  }
}
```

### 7.2 Required fields

#### `wef`

The WEF specification version. It MUST equal `"0.0.1"`.

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

A package-relative path to the ECMAScript entry module.

#### `languages`

An array of BCP 47 language tags.

A reader that expects one source instance per language MAY expose one logical source instance for each declared language.

#### `baseUrls`

Known website and API origins associated with the source.

This is metadata in 0.0.1. Engines and repositories MAY use it for URL routing or policy enforcement.

#### `requires`

Required host capabilities.

Allowed values are:

- `http`
- `html`

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

A Mihon/Tachiyomi adapter SHOULD map `popular` and `latest` directly to the reader's corresponding browse operations.

When a source has only one meaningful browse feed, it SHOULD declare it as `popular`.

### 7.4 Capabilities

```ts
interface Capabilities {
  filters?: boolean;
  urlResolution?: boolean;
  imageRequests?: boolean;
  migrations?: boolean;
}
```

A capability set to `true` requires the corresponding optional exports.

An omitted capability is `false`.

---

## 8. JavaScript execution profile

The entry module is an ECMAScript module.

The engine MUST support:

- ECMAScript 2020 syntax;
- modules;
- promises;
- `async` and `await`;
- standard JSON, arrays, objects, strings, numbers, dates, and regular expressions.

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

An engine MAY use any runtime that produces conforming behavior.

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
export async function resolveUrl(ctx, input) {}
export async function getImageRequest(ctx, input) {}
export async function migrateMangaKey(ctx, input) {}
export async function migrateChapterKey(ctx, input) {}
```

Every operation:

- MUST return a promise or be declared `async`;
- MUST receive the WEF context first;
- MUST accept and return JSON-compatible values unless otherwise specified;
- MUST NOT expose engine-native objects.

---

## 10. Core operations

## 10.1 `getMangaList`

Retrieves one declared browse listing.

```ts
interface MangaListInput {
  listingId: string;
  page: number;
}

getMangaList(ctx, input: MangaListInput) -> MangaPage
```

Rules:

- `listingId` MUST match a manifest listing;
- `page` begins at `1`;
- results MUST use source-defined order.

---

## 10.2 `search`

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
- `filters` MUST be an empty object when no filters are applied;
- unknown filter IDs SHOULD be ignored;
- a source that cannot perform an empty search MAY return an empty page.

---

## 10.3 `getMangaUpdate`

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
- `chapters` contains the reader's currently stored chapters and MAY be empty;
- when `fetchDetails` is true, the result MUST contain `manga`;
- when `fetchChapters` is true, the result MUST contain `chapters`;
- omitted result fields mean “not requested,” not “delete existing data.”

This combined operation exists because source sites often provide details and chapters in one response.

---

## 10.4 `getPages`

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
- the complete manga and chapter are passed so sources can use keys, URLs, or opaque `extra` data;
- an empty array is valid for a chapter with no readable pages.

---

## 11. Optional operations

## 11.1 `getFilters`

Required when `capabilities.filters` is true.

```ts
getFilters(ctx) -> Filter[]
```

WEF 0.0.1 defines a minimal filter model:

```ts
type Filter =
  | {
      id: string;
      name: string;
      type: "text";
      default?: string;
    }
  | {
      id: string;
      name: string;
      type: "toggle";
      default?: boolean;
    }
  | {
      id: string;
      name: string;
      type: "select";
      options: FilterOption[];
      default?: string;
    }
  | {
      id: string;
      name: string;
      type: "multi-select";
      options: FilterOption[];
      default?: string[];
    };

interface FilterOption {
  value: string;
  label: string;
}
```

Reader adapters MAY translate these into their native filter controls.

---

## 11.2 `resolveUrl`

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

---

## 11.3 `getImageRequest`

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
}

getImageRequest(ctx, input: ImageRequestInput) -> ImageRequest
```

This allows a source to attach referer, authorization, or other request-specific headers without requiring the reader to understand source logic.

Binary image post-processing is not part of WEF 0.0.1.

---

## 11.4 Key migration

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

Keys SHOULD remain stable. Migration exists for unavoidable source URL or identifier changes.

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

Functions, symbols, cyclic objects, class instances, and runtime-native objects MUST NOT be returned.

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
- `extra` MAY preserve source-specific state required by later operations;
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

The dual string/numeric number fields avoid losing values such as `10.5`, `Extra`, or source-specific numbering while still mapping efficiently to readers that use numeric chapter values.

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
- `url` MAY identify an intermediate page or source endpoint used to resolve an image;
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
  url: UrlApi;
  fail(
    code: ErrorCode,
    message?: string,
    details?: JsonValue
  ): never;
}
```

An engine MUST provide every manifest-required capability.

An engine MAY expose development extensions, but portable sources MUST NOT depend on undeclared or non-standard functions.

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
- query values are encoded by the engine;
- string arrays produce repeated query fields;
- `url` in the response is the final URL after redirects;
- response headers SHOULD use lowercase names;
- `body` is decoded text;
- JSON is parsed with `JSON.parse`.

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

Binary source-operation responses are deferred. Reader image fetching is not performed through this text response model.

---

## 19. URL API

```ts
interface UrlApi {
  resolve(base: string, value: string): string;
}
```

`resolve` converts relative and protocol-relative URLs into absolute URLs.

---

## 20. HTML API

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

WEF 0.0.1 engines MUST support:

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

Whitespace normalization is implementation-defined. Sources SHOULD call `.trim()` and MUST NOT depend on exact internal whitespace.

---

## 21. Errors

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

`CHALLENGE_REQUIRED` tells the engine that normal HTTP access was insufficient. WEF 0.0.1 does not prescribe how a browser or user-assisted checkpoint is implemented.

---

## 22. Adapter mapping

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

An adapter MUST preserve opaque keys and `extra` data as far as the reader's storage model allows.

---

## 23. MangaDex reference-source expectations

The first reference source SHOULD use the MangaDex API and require only `http`.

It SHOULD implement:

- a `popular` listing;
- a `latest` listing;
- search;
- selective manga details and chapter updates;
- page retrieval;
- URL resolution for manga and chapter URLs.

The MangaDex source validates:

- JSON requests and parsing;
- query parameters;
- pagination;
- stable opaque keys;
- multilingual metadata;
- chapter grouping and ordering;
- image-server page results;
- selective update behavior.

The second reference source SHOULD use ordinary HTML parsing and require `http` and `html`.

---

## 24. Validator requirements

A WEF 0.0.1 validator SHOULD check:

- valid `wef.json`;
- required fields;
- supported WEF version;
- valid source ID;
- valid entry path;
- at least one listing;
- canonical listing IDs are used correctly;
- required exports exist;
- optional exports match capabilities;
- result values are JSON-compatible;
- search and listing pages have valid manga entries;
- manga and chapter keys are non-empty;
- chapter names are non-empty;
- page entries contain `url` or `imageUrl`;
- required URLs are absolute;
- page order is preserved;
- selective update results contain requested fields;
- filters have unique IDs;
- manifest base URLs are valid absolute origins.

Static proof of arbitrary JavaScript behavior is not required.

---

## 25. Engine requirements

A conforming WEF 0.0.1 engine MUST:

1. load and validate `wef.json`;
2. reject unsupported WEF versions;
3. load the entry module;
4. provide required host capabilities;
5. invoke operations using the standard signatures;
6. safely validate or consume results;
7. convert unexpected exceptions into WEF errors;
8. scope keys to their source;
9. preserve opaque `extra` values when round-tripped;
10. use array order for chapters and pages;
11. avoid exposing native host objects.

An engine MAY:

- be written in any language;
- use any JavaScript runtime;
- compile source logic ahead of time;
- cache requests or results;
- enforce repository-specific policy;
- expose WEF through FFI, IPC, HTTP, or an in-process API;
- translate WEF values into native reader models;
- provide browser-assisted recovery after HTTP challenges.

An engine MUST NOT require a source to know which reader or engine is executing it.

---

## 26. Reader responsibilities

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

Readers MAY map WEF data into native models and MAY ignore optional fields they cannot represent.

---

## 27. Repository and trust policy

WEF 0.0.1 does not define repository governance or a mandatory sandbox.

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

## 28. Deferred features

The following are intentionally deferred:

- arbitrary home-page layouts;
- custom reader UI;
- advanced filter controls and nested groups;
- source settings;
- login protocols;
- browser automation APIs;
- CAPTCHA handling;
- binary HTTP responses in source operations;
- image transformation and decryption;
- archive and text pages;
- alternate covers;
- notifications;
- persistent source-owned storage;
- dynamic listings;
- dynamic base URLs;
- package signatures;
- repository indexes;
- source dependencies;
- WebAssembly representation;
- declarative selector-only representation;
- localization bundles.

The data model leaves room for several of these without changing the four core operations.

---

## 29. Implementation milestone

WEF 0.0.1 is considered implemented when the project has:

- a manifest parser;
- a package loader;
- an ECMAScript module runtime;
- standard data models;
- the four core operations;
- the HTTP and URL host APIs;
- selective update validation;
- a MangaDex source;
- an HTML-parsing source;
- a command-line runner;
- a validator and linter;
- fixture-based conformance tests;
- a documented Mihon/Tachiyomi adapter mapping;
- a documented Aidoku adapter mapping.

---

## 30. Suggested Rust workspace

The reference implementation is non-normative.

```text
wef/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── wef-spec/
│   ├── wef-engine/
│   └── wef-cli/
├── sources/
│   ├── mangadex/
│   └── html-example/
└── fixtures/
```

Suggested responsibilities:

### `wef-spec`

- manifest types;
- operation inputs and results;
- manga, chapter, page, listing, and filter models;
- error codes;
- validation rules.

### `wef-engine`

- package loading;
- JavaScript execution;
- host APIs;
- operation dispatch;
- result validation;
- adapter-facing Rust API.

### `wef-cli`

Possible commands:

```text
wef validate <path>
wef run <path> listing <id> --page 1
wef run <path> search <query> --page 1
wef run <path> update <manga-json>
wef run <path> pages <manga-json> <chapter-json>
wef test <path>
```

---

## 31. Open questions for 0.0.2

1. Should filter groups, sort controls, and tri-state filters enter the core?
2. Should binary image processing be standardized?
3. Should text and archive-backed pages be supported?
4. Should listings be static, dynamic, or both?
5. Should source settings use the same schema as filters?
6. Should browser session acquisition become a host capability?
7. Should keys support a formal breaking-change version?
8. Should `extra` receive a maximum encoded size?
9. Should JavaScript remain the only 0.x executable representation?
10. Should the package receive a canonical archive encoding and `.wef` extension?

---

## 32. License

The WEF specification is dual-licensed under the terms of either the MIT
License or the Apache License, Version 2.0, at your option.

This permissive licensing allows independent readers and engines to implement
WEF. See the repository's `LICENSE-MIT` and `LICENSE-APACHE` files.
