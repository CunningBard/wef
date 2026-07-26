export async function getMangaList(ctx) { return { items: [{ key: 'settings', title: ctx.settings.mode }], hasNextPage: false }; }
export async function search() { return { items: [], hasNextPage: false }; }
export async function getMangaUpdate() { return { chapters: [] }; }
export async function getPages() { return []; }
export async function getSettings() { return [{ id: 'mode', name: 'Mode', type: 'select', options: [{ id: 'safe', name: 'Safe' }], default: 'safe' }, { id: 'token', name: 'Token', type: 'text', secret: true }]; }
export async function getFilters(ctx) { const response = await ctx.http.request({ url: 'https://example.org/options' }); return [{ id: 'group', name: 'Group', type: 'group', children: [{ id: 'tags', name: response.body, type: 'tri-state', options: [{ id: 'a', name: 'A' }] }, { id: 'year', name: 'Year', type: 'range', min: 2000, max: 2030, step: 1 }, { id: 'sort', name: 'Sort', type: 'sort', options: [{ id: 'title', name: 'Title' }] }] }]; }
export async function getImageRequest() { return { url: 'https://example.org/primary', candidates: [{ url: 'https://example.org/fallback', headers: { Referer: 'https://example.org/' } }] }; }
