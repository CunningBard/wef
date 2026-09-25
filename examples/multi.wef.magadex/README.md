# MangaDex WEF source

The source uses package-local ES modules, supports filter discovery and MangaDex
title/chapter URL resolution, and preserves grouping/order metadata in chapter
`extra`. The default language/content-rating selections are English and
safe/suggestive/erotica; search callers can override them through filters.

Run `wef test` for the deterministic fixtures (the default test path);
use the REPL for live requests.

This package implements the four WEF 0.0.1 core operations against the public
MangaDex API:

- `getMangaList` provides latest and popular listings.
- `search` queries the manga endpoint.
- `getMangaUpdate` retrieves expanded manga metadata and English chapters.
- `getPages` resolves MangaDex@Home full-quality image URLs.

The implementation follows the MangaDex API v5 OpenAPI document.

Consumers must visibly credit MangaDex and the scanlation groups exposed on
chapter records, as required by the acceptable-use policy in that API document.

## Fixtures

`fixtures/*.json` files can be run without network access with:

```text
wef test examples/multi.wef.magadex
```

Each fixture declares an operation input, the exact HTTP request/response
sequence, and the expected WEF result.
