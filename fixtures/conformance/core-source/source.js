export async function getMangaList(_ctx, input) { return { items: [{ key: `list-${input.page}`, title: "Listing" }], hasNextPage: false }; }
export async function search(_ctx, input) { return { items: input.query ? [{ key: "search", title: input.query }] : [], hasNextPage: false }; }
export async function getMangaUpdate(_ctx, input) { return { manga: input.fetchDetails ? input.manga : undefined, chapters: input.fetchChapters ? input.chapters : undefined }; }
export async function getPages() { return [{ imageUrl: "https://example.org/page.jpg" }]; }
