const BASE = 'https://example.org';

async function document(ctx, path) {
  const response = await ctx.http.request({ url: `${BASE}${path}` });
  return ctx.html.parse(response.body);
}

function mangaList(doc) {
  return doc.selectAll('.manga').map((node) => {
      const id = node.attr('data-id');
      return { key: id, title: node.select('.title').text(), url: `${BASE}/manga/${id}` };
  });
}

async function mangaPage(ctx, path) {
  return { items: mangaList(await document(ctx, path)), hasNextPage: false };
}

export async function getMangaList(ctx, input) {
  return mangaPage(ctx, `/latest?page=${input.page}`);
}

export async function search(ctx, input) {
  return mangaPage(ctx, `/search?q=${encodeURIComponent(input.query)}&page=${input.page}`);
}

export async function getMangaUpdate(ctx, input) {
  const doc = await document(ctx, `/manga/${input.manga.key}`);
  return {
    manga: { ...input.manga, description: doc.select('.description').text() },
    chapters: doc.selectAll('.chapter').map(node => ({ key: node.attr('data-id'), name: node.text(), number: node.attr('data-number'), numberValue: Number(node.attr('data-number')), url: `${BASE}/chapter/${node.attr('data-id')}` })),
  };
}

export async function getPages(ctx, input) {
  const doc = await document(ctx, `/chapter/${input.chapter.key}`);
  return doc.selectAll('.page').map(node => ({ imageUrl: node.attr('src') }));
}
