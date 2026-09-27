// WEF 0.1.0 conformance source: exercises the full wire against
// https://example.org with deterministic fixture responses. Every export
// is covered by a fixture in fixtures/ (see the conformance README).
const API = "https://example.org/api";

async function getJson(ctx, path, query) {
  const request = {
    method: "GET",
    url: `${API}${path}`,
    headers: { Accept: "application/json" },
  };
  if (query) request.query = query;
  const response = await ctx.http.request(request);
  return JSON.parse(response.body);
}

function toManga(item) {
  const manga = {
    key: item.id,
    title: item.title,
    url: `https://example.org/manga/${item.id}`,
    status: item.status || "unknown",
    extra: { edition: "conformance" },
  };
  if (item.cover) manga.coverUrl = item.cover;
  return manga;
}

export async function getMangaList(ctx, input) {
  const data = await getJson(
    ctx,
    `/list/${input.listingId}/${input.page}`,
    { genre: input.filters && input.filters.genre ? input.filters.genre : "all" },
  );
  return {
    items: data.results.map(toManga),
    hasNextPage: data.hasMore,
  };
}

export async function search(ctx, input) {
  // Scripted browser capture is unavailable to the fixture host, so this
  // pins the specified fallback: UNSUPPORTED means plain HTTP instead.
  try {
    await ctx.browser.run({
      url: "https://example.org/search",
      task: { kind: "snapshot", selector: "script#initial-data" },
      timeoutMs: 1000,
    });
  } catch (error) {
    if (!error || error.code !== "UNSUPPORTED") throw error;
  }
  const query = { title: input.query || "", page: String(input.page) };
  if (input.filters && input.filters.genre) query.genre = input.filters.genre;
  const data = await getJson(ctx, "/search", query);
  return {
    items: data.results.map(toManga),
    hasNextPage: data.hasMore,
  };
}

export async function getMangaUpdate(ctx, input) {
  const data = await getJson(ctx, `/manga/${input.manga.key}`);
  const output = {};
  if (input.fetchDetails) {
    const paragraph = ctx.html.parse(data.descriptionHtml).select("p");
    output.manga = toManga(data);
    output.manga.description = paragraph ? paragraph.text().trim() : "";
  }
  if (input.fetchChapters) {
    output.chapters = data.chapters.map((chapter) => ({
      key: chapter.id,
      name: chapter.name,
      number: chapter.number,
      language: "en",
    }));
  }
  return output;
}

export async function getPages(ctx, input) {
  const cacheKey = `pages:${input.chapter.key}`;
  const cached = ctx.store.get(cacheKey);
  if (cached) return cached;
  const data = await getJson(ctx, `/pages/${input.chapter.key}`);
  const pages = data.images.map((entry) => ({
    imageUrl: entry.url,
    headers: { Referer: "https://example.org/" },
  }));
  ctx.store.set(cacheKey, pages);
  return ctx.store.get(cacheKey);
}

export async function getFilters() {
  return [
    {
      type: "group",
      id: "content",
      name: "Content",
      children: [
        {
          type: "select",
          id: "genre",
          name: "Genre",
          options: [
            { id: "all", name: "All" },
            { id: "action", name: "Action" },
            { id: "drama", name: "Drama" },
          ],
          default: "all",
        },
        {
          type: "multi-select",
          id: "tags",
          name: "Tags",
          options: [
            { id: "complete", name: "Complete" },
            { id: "oneshot", name: "Oneshot" },
          ],
          default: ["complete"],
        },
      ],
    },
    {
      type: "text",
      id: "author",
      name: "Author",
      placeholder: "Author name",
    },
    { type: "toggle", id: "finished", name: "Finished only", default: false },
    {
      type: "tri-state",
      id: "demographic",
      name: "Demographic",
      options: [
        { id: "shounen", name: "Shounen" },
        { id: "seinen", name: "Seinen" },
      ],
      default: { shounen: "include" },
    },
    {
      type: "range",
      id: "chapters",
      name: "Chapters",
      min: 0,
      max: 1000,
      step: 10,
      default: { min: 10 },
    },
    {
      type: "sort",
      id: "order",
      name: "Order",
      options: [
        { id: "title", name: "Title" },
        { id: "updated", name: "Updated" },
      ],
      default: { value: "updated", direction: "desc" },
    },
  ];
}

export async function getSettings() {
  return [
    { id: "username", name: "Username", type: "text" },
    { id: "token", name: "API token", type: "text", secret: true },
    { id: "nsfw", name: "Show NSFW", type: "toggle", default: false },
    {
      id: "language",
      name: "Language",
      type: "select",
      options: [
        { id: "en", name: "English" },
        { id: "ja", name: "Japanese" },
      ],
      default: "en",
    },
    {
      id: "content",
      name: "Content",
      type: "multi-select",
      options: [
        { id: "safe", name: "Safe" },
        { id: "suggestive", name: "Suggestive" },
      ],
      default: ["safe"],
    },
  ];
}

export async function resolveUrl(ctx, input) {
  let parts;
  try {
    parts = ctx.url.parse(input.url);
  } catch (error) {
    return null;
  }
  if (parts.host !== "example.org") return null;
  const match = parts.path.match(
    /^\/manga\/([A-Za-z0-9-]+)(?:\/chapter\/([A-Za-z0-9-]+))?$/,
  );
  if (match) {
    if (match[2]) {
      return { type: "chapter", mangaKey: match[1], chapterKey: match[2] };
    }
    return { type: "manga", mangaKey: match[1] };
  }
  if (parts.path === "/latest") {
    return { type: "listing", listingId: "latest" };
  }
  return null;
}

export async function getImageRequest(ctx, input) {
  return {
    url: input.url,
    headers: { Referer: "https://example.org/" },
    candidates: [
      {
        url: `${input.url}?fallback=1`,
        headers: { Referer: "https://example.org/" },
      },
    ],
  };
}

export async function transformImage(ctx, input) {
  if (input.mimeType === "application/x-xor") {
    const bytes = new Uint8Array(input.body);
    const out = new Uint8Array(bytes.length);
    for (let i = 0; i < bytes.length; i++) out[i] = bytes[i] ^ 0xff;
    return { mimeType: "application/x-xor", body: out.buffer };
  }
  const bitmap = await ctx.image.decode(input.body);
  const canvas = ctx.image.create(bitmap.width, bitmap.height);
  const rect = { x: 0, y: 0, width: bitmap.width, height: bitmap.height };
  ctx.image.blit(canvas, bitmap, rect, rect);
  return {
    mimeType: "image/png",
    body: await ctx.image.encode(canvas, "image/png"),
  };
}

export async function migrateMangaKey(ctx, input) {
  return input.key.replace(/^legacy:/, "v2:");
}

export async function migrateChapterKey(ctx, input) {
  return `${input.mangaKey}#${input.chapterKey}`;
}
