import { requestJson } from "./api.js";
import { CHAPTER_PAGE_SIZE, DEFAULT_CONTENT_RATINGS, DEFAULT_LANGUAGES, MANGA_PAGE_SIZE, SITE_BASE, UPLOADS_BASE } from "./config.js";

function localizedValue(values, preferredLanguage = "en") {
    if (!values || typeof values !== "object") {
        return undefined;
    }

    if (typeof values[preferredLanguage] === "string" && values[preferredLanguage]) {
        return values[preferredLanguage];
    }
    if (typeof values.en === "string" && values.en) {
        return values.en;
    }

    for (const key of Object.keys(values)) {
        if (typeof values[key] === "string" && values[key]) {
            return values[key];
        }
    }
    return undefined;
}

function uniqueStrings(values) {
    const result = [];
    for (const value of values) {
        if (typeof value === "string" && value && !result.includes(value)) {
            result.push(value);
        }
    }
    return result;
}

function relationshipNames(relationships, type) {
    return uniqueStrings(
        (relationships || [])
            .filter((relationship) => relationship.type === type)
            .map((relationship) => relationship.attributes?.name),
    );
}

function mapContentRating(value) {
    switch (value) {
        case "safe":
            return "safe";
        case "suggestive":
            return "suggestive";
        case "erotica":
        case "pornographic":
            return "nsfw";
        default:
            return undefined;
    }
}

function mapManga(entity) {
    const attributes = entity.attributes || {};
    const relationships = entity.relationships || [];
    const preferredLanguage = attributes.originalLanguage || "en";
    const title =
        localizedValue(attributes.title, "en") ||
        localizedValue(attributes.title, preferredLanguage) ||
        entity.id;

    const manga = {
        key: entity.id,
        title,
        url: `${SITE_BASE}/title/${encodeURIComponent(entity.id)}`,
        updateStrategy: "always",
        extra: {
            mangaId: entity.id,
        },
    };

    const cover = relationships.find(
        (relationship) =>
            relationship.type === "cover_art" &&
            typeof relationship.attributes?.fileName === "string",
    );
    if (cover) {
        manga.coverUrl =
            `${UPLOADS_BASE}/covers/${encodeURIComponent(entity.id)}/` +
            `${encodeURIComponent(cover.attributes.fileName)}.256.jpg`;
    }

    const alternativeTitles = uniqueStrings(
        (attributes.altTitles || []).flatMap((entry) => Object.values(entry || {})),
    ).filter((alternativeTitle) => alternativeTitle !== title);
    if (alternativeTitles.length > 0) {
        manga.alternativeTitles = alternativeTitles;
    }

    const description =
        localizedValue(attributes.description, "en") ||
        localizedValue(attributes.description, preferredLanguage);
    if (description) {
        manga.description = description;
    }

    const authors = relationshipNames(relationships, "author");
    if (authors.length > 0) {
        manga.authors = authors;
    }

    const artists = relationshipNames(relationships, "artist");
    if (artists.length > 0) {
        manga.artists = artists;
    }

    const tags = uniqueStrings(
        (attributes.tags || []).map((tag) =>
            localizedValue(tag.attributes?.name, "en"),
        ),
    );
    if (tags.length > 0) {
        manga.tags = tags;
    }

    if (["ongoing", "completed", "hiatus", "cancelled"].includes(attributes.status)) {
        manga.status = attributes.status;
    }
    const contentRating = mapContentRating(attributes.contentRating);
    if (contentRating) {
        manga.contentRating = contentRating;
    }

    return manga;
}

function numericValue(value) {
    if (typeof value !== "string" || !value) {
        return undefined;
    }
    const number = Number(value);
    return Number.isFinite(number) ? number : undefined;
}

function mapChapter(entity) {
    const attributes = entity.attributes || {};
    const number = attributes.chapter || undefined;
    const volume = attributes.volume || undefined;
    const title = attributes.title || undefined;
    const label = [];

    if (volume) {
        label.push(`Vol. ${volume}`);
    }
    if (number) {
        label.push(`Ch. ${number}`);
    }
    if (label.length === 0) {
        label.push("Oneshot");
    }

    const chapter = {
        key: entity.id,
        name: title ? `${label.join(" ")} — ${title}` : label.join(" "),
        url: attributes.externalUrl || `${SITE_BASE}/chapter/${encodeURIComponent(entity.id)}`,
        language: attributes.translatedLanguage,
        publishedAt: attributes.readableAt || attributes.publishAt,
        locked: attributes.isUnavailable === true,
        extra: {
            chapterId: entity.id,
            groupKey: volume ? `volume:${volume}` : "volume:unknown",
            order: { volume: numericValue(volume), chapter: numericValue(number), readableAt: attributes.readableAt || attributes.publishAt },
        },
    };

    if (title) {
        chapter.title = title;
    }
    if (number) {
        chapter.number = number;
        const numberValue = numericValue(number);
        if (numberValue !== undefined) {
            chapter.numberValue = numberValue;
        }
    }
    if (volume) {
        chapter.volume = volume;
        const volumeValue = numericValue(volume);
        if (volumeValue !== undefined) {
            chapter.volumeValue = volumeValue;
        }
    }

    const scanlators = relationshipNames(entity.relationships, "scanlation_group");
    if (scanlators.length > 0) {
        chapter.scanlators = scanlators;
    }
    if (attributes.externalUrl) {
        chapter.extra.externalUrl = attributes.externalUrl;
    }

    return chapter;
}

function selectedValues(filters, key, defaults) {
    const value = filters?.[key];
    return Array.isArray(value) && value.every((item) => typeof item === "string") && value.length > 0
        ? value
        : defaults;
}

function mangaQuery(page, order, title = undefined, filters = undefined) {
    const query = {
        limit: String(MANGA_PAGE_SIZE),
        offset: String((page - 1) * MANGA_PAGE_SIZE),
        "contentRating[]": selectedValues(filters, "contentRatings", DEFAULT_CONTENT_RATINGS),
        "availableTranslatedLanguage[]": selectedValues(filters, "languages", DEFAULT_LANGUAGES),
        "includes[]": ["cover_art", "author", "artist"],
        hasAvailableChapters: "true",
    };
    query[`order[${order}]`] = "desc";
    if (title) {
        query.title = title;
    }
    return query;
}

function mangaPage(payload) {
    const data = Array.isArray(payload.data) ? payload.data : [];
    const offset = typeof payload.offset === "number" ? payload.offset : 0;
    const total = typeof payload.total === "number" ? payload.total : undefined;
    return {
        items: data.map(mapManga),
        hasNextPage:
            total === undefined
                ? data.length === MANGA_PAGE_SIZE
                : offset + data.length < total,
    };
}

async function fetchManga(ctx, mangaId) {
    const payload = await requestJson(
        ctx,
        `/manga/${encodeURIComponent(mangaId)}`,
        {
            "includes[]": ["cover_art", "author", "artist"],
        },
    );
    if (!payload.data) {
        ctx.fail("INVALID_RESPONSE", "MangaDex response did not contain manga data");
    }
    return mapManga(payload.data);
}

async function fetchChapters(ctx, mangaId) {
    const chapters = [];
    let offset = 0;

    for (;;) {
        const payload = await requestJson(
            ctx,
            `/manga/${encodeURIComponent(mangaId)}/feed`,
            {
                limit: String(CHAPTER_PAGE_SIZE),
                offset: String(offset),
                "translatedLanguage[]": DEFAULT_LANGUAGES,
                "contentRating[]": DEFAULT_CONTENT_RATINGS,
                "includes[]": ["scanlation_group"],
                "order[volume]": "desc",
                "order[chapter]": "desc",
                includeEmptyPages: "0",
                includeExternalUrl: "0",
                includeUnavailable: "0",
            },
        );
        const data = Array.isArray(payload.data) ? payload.data : [];
        chapters.push(...data.map(mapChapter));
        offset += data.length;

        if (
            data.length === 0 ||
            data.length < CHAPTER_PAGE_SIZE ||
            (typeof payload.total === "number" && offset >= payload.total)
        ) {
            break;
        }
    }

    chapters.sort((left, right) => {
        const leftOrder = left.extra?.order || {};
        const rightOrder = right.extra?.order || {};
        const volume = (rightOrder.volume ?? -1) - (leftOrder.volume ?? -1);
        if (volume !== 0) return volume;
        const chapter = (rightOrder.chapter ?? -1) - (leftOrder.chapter ?? -1);
        if (chapter !== 0) return chapter;
        return String(rightOrder.readableAt || "").localeCompare(String(leftOrder.readableAt || ""));
    });
    return chapters;
}

export async function getMangaList(ctx, input) {
    const order =
        input.listingId === "popular" ? "followedCount" : "latestUploadedChapter";
    const payload = await requestJson(ctx, "/manga", mangaQuery(input.page, order));
    return mangaPage(payload);
}

export async function search(ctx, input) {
    const query = typeof input.query === "string" ? input.query.trim() : "";
    const order = query ? "relevance" : "latestUploadedChapter";
    const payload = await requestJson(
        ctx,
        "/manga",
        mangaQuery(input.page, order, query || undefined, input.filters),
    );
    return mangaPage(payload);
}

export async function getFilters() {
    return [
        {
            id: "languages",
            name: "Translated languages",
            type: "multi-select",
            options: [
                { id: "en", name: "English" }, { id: "ja", name: "Japanese" },
                { id: "ko", name: "Korean" }, { id: "zh", name: "Chinese" },
            ],
            default: DEFAULT_LANGUAGES,
        },
        {
            id: "contentRatings",
            name: "Content ratings",
            type: "multi-select",
            options: [
                { id: "safe", name: "Safe" }, { id: "suggestive", name: "Suggestive" },
                { id: "erotica", name: "Erotica" }, { id: "pornographic", name: "Pornographic" },
            ],
            default: DEFAULT_CONTENT_RATINGS,
        },
    ];
}

export async function resolveUrl(ctx, input) {
    const match = /^https?:\/\/(?:www\.)?mangadex\.org\/([^?#]*)/i.exec(input.url);
    if (!match) { return null; }
    const parts = match[1].split("/").filter(Boolean);
    const titleIndex = parts.indexOf("title");
    if (titleIndex >= 0 && parts[titleIndex + 1]) {
        return { type: "manga", mangaKey: parts[titleIndex + 1] };
    }
    const chapterIndex = parts.indexOf("chapter");
    if (chapterIndex < 0 || !parts[chapterIndex + 1]) { return null; }
    const chapterKey = parts[chapterIndex + 1];
    const payload = await requestJson(ctx, `/chapter/${encodeURIComponent(chapterKey)}`, { "includes[]": ["manga"] });
    const manga = payload.data?.relationships?.find((relationship) => relationship.type === "manga");
    if (!manga?.id) { ctx.fail("INVALID_RESPONSE", "MangaDex chapter response did not include its manga"); }
    return { type: "chapter", mangaKey: manga.id, chapterKey };
}

export async function getMangaUpdate(ctx, input) {
    const update = {};
    const mangaId = input.manga.extra?.mangaId || input.manga.key;

    if (input.fetchDetails) {
        update.manga = await fetchManga(ctx, mangaId);
    }
    if (input.fetchChapters) {
        update.chapters = await fetchChapters(ctx, mangaId);
    }

    return update;
}

export async function getPages(ctx, input) {
    const externalUrl = input.chapter.extra?.externalUrl;
    if (externalUrl) {
        return [{ url: externalUrl }];
    }

    const chapterId = input.chapter.extra?.chapterId || input.chapter.key;
    const payload = await requestJson(
        ctx,
        `/at-home/server/${encodeURIComponent(chapterId)}`,
        { forcePort443: "true" },
    );
    const baseUrl = payload.baseUrl;
    const hash = payload.chapter?.hash;
    const files = payload.chapter?.data;

    if (
        typeof baseUrl !== "string" ||
        typeof hash !== "string" ||
        !Array.isArray(files)
    ) {
        ctx.fail("INVALID_RESPONSE", "MangaDex@Home response was incomplete");
    }

    const base = baseUrl.endsWith("/") ? baseUrl.slice(0, -1) : baseUrl;
    return files.map((fileName) => ({
        imageUrl:
            `${base}/data/${encodeURIComponent(hash)}/` +
            encodeURIComponent(fileName),
    }));
}
