use std::{collections::BTreeMap, path::PathBuf};

use serde_json::{Map, Value, json};
use wef_engine_rs::{Engine, HostError, HttpRequest, HttpResponse, Operation, Package, WefHost};

const MANGA_ID: &str = "11111111-1111-4111-8111-111111111111";
const CHAPTER_ID: &str = "22222222-2222-4222-8222-222222222222";

fn package() -> Package {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../source/mangadex");
    Package::load(root).unwrap()
}

fn manga_entity() -> Value {
    json!({
        "id": MANGA_ID,
        "type": "manga",
        "attributes": {
            "title": {"en": "Demo Manga", "ja": "デモ漫画"},
            "altTitles": [{"en": "Demo Alternative"}, {"ja": "デモ"}],
            "description": {"en": "A test description."},
            "originalLanguage": "ja",
            "status": "ongoing",
            "contentRating": "suggestive",
            "tags": [{
                "id": "tag-1",
                "type": "tag",
                "attributes": {"name": {"en": "Action"}}
            }]
        },
        "relationships": [
            {
                "id": "cover-1",
                "type": "cover_art",
                "attributes": {"fileName": "cover-file.jpg"}
            },
            {
                "id": "author-1",
                "type": "author",
                "attributes": {"name": "Writer"}
            },
            {
                "id": "artist-1",
                "type": "artist",
                "attributes": {"name": "Artist"}
            }
        ]
    })
}

fn chapter_entity() -> Value {
    json!({
        "id": CHAPTER_ID,
        "type": "chapter",
        "attributes": {
            "title": "A New Start",
            "volume": "2",
            "chapter": "10.5",
            "translatedLanguage": "en",
            "readableAt": "2026-01-02T03:04:05+00:00",
            "isUnavailable": false
        },
        "relationships": [{
            "id": "group-1",
            "type": "scanlation_group",
            "attributes": {"name": "Demo Group"}
        }]
    })
}

#[derive(Default)]
struct MangaDexMock;

impl MangaDexMock {
    fn response(body: Value) -> HttpResponse {
        HttpResponse {
            status: 200,
            url: "https://api.mangadex.org/mock".into(),
            headers: BTreeMap::new(),
            body: serde_json::to_string(&body).unwrap(),
        }
    }

    fn query(request: &HttpRequest) -> &Map<String, Value> {
        request.query.as_ref().expect("request should have a query")
    }
}

impl WefHost for MangaDexMock {
    fn request(&mut self, request: HttpRequest) -> Result<HttpResponse, HostError> {
        assert_eq!(request.method.as_deref(), Some("GET"));
        assert_eq!(
            request.headers.as_ref().unwrap().get("Accept"),
            Some(&"application/json".to_owned())
        );

        if request.url == "https://api.mangadex.org/manga" {
            let query = Self::query(&request);
            assert_eq!(query["limit"], "24");
            assert_eq!(
                query["includes[]"],
                json!(["cover_art", "author", "artist"])
            );
            assert!(
                query.contains_key("order[latestUploadedChapter]")
                    || query.contains_key("order[followedCount]")
                    || query.contains_key("order[relevance]")
            );
            if query.contains_key("title") {
                assert_eq!(query["order[relevance]"], "desc");
            }
            return Ok(Self::response(json!({
                "result": "ok",
                "response": "collection",
                "data": [manga_entity()],
                "limit": 24,
                "offset": query["offset"],
                "total": 25
            })));
        }

        if request.url == format!("https://api.mangadex.org/manga/{MANGA_ID}") {
            assert_eq!(
                Self::query(&request)["includes[]"],
                json!(["cover_art", "author", "artist"])
            );
            return Ok(Self::response(json!({
                "result": "ok",
                "response": "entity",
                "data": manga_entity()
            })));
        }

        if request.url == format!("https://api.mangadex.org/manga/{MANGA_ID}/feed") {
            let query = Self::query(&request);
            assert_eq!(query["translatedLanguage[]"], json!(["en"]));
            assert_eq!(query["includes[]"], json!(["scanlation_group"]));
            return Ok(Self::response(json!({
                "result": "ok",
                "response": "collection",
                "data": [chapter_entity()],
                "limit": 500,
                "offset": 0,
                "total": 1
            })));
        }

        if request.url == format!("https://api.mangadex.org/at-home/server/{CHAPTER_ID}") {
            assert_eq!(Self::query(&request)["forcePort443"], "true");
            return Ok(Self::response(json!({
                "result": "ok",
                "baseUrl": "https://uploads.example.test/token",
                "chapter": {
                    "hash": "chapter-hash",
                    "data": ["page-1.png", "page 2.png"],
                    "dataSaver": ["page-1.jpg", "page-2.jpg"]
                }
            })));
        }

        panic!("unexpected MangaDex request: {request:?}");
    }
}

fn engine() -> Engine {
    Engine::with_host(MangaDexMock)
}

fn minimal_manga() -> Value {
    json!({"key": MANGA_ID, "title": "Demo Manga"})
}

#[test]
fn loads_the_mangadex_package() {
    let package = package();
    assert_eq!(package.manifest().id, "org.mangadex");
    assert_eq!(package.manifest().listings.len(), 2);
}

#[test]
fn maps_latest_and_search_pages() {
    let package = package();
    let listing = engine()
        .run(
            &package,
            Operation::GetMangaList,
            json!({"listingId": "latest", "page": 1}),
        )
        .unwrap();

    assert_eq!(listing["items"][0]["key"], MANGA_ID);
    assert_eq!(listing["items"][0]["title"], "Demo Manga");
    assert_eq!(listing["items"][0]["authors"], json!(["Writer"]));
    assert_eq!(listing["items"][0]["artists"], json!(["Artist"]));
    assert_eq!(listing["items"][0]["tags"], json!(["Action"]));
    assert_eq!(listing["items"][0]["status"], "ongoing");
    assert_eq!(listing["items"][0]["contentRating"], "suggestive");
    assert_eq!(
        listing["items"][0]["coverUrl"],
        format!("https://uploads.mangadex.org/covers/{MANGA_ID}/cover-file.jpg.256.jpg")
    );
    assert_eq!(listing["hasNextPage"], true);

    let popular = engine()
        .run(
            &package,
            Operation::GetMangaList,
            json!({"listingId": "popular", "page": 1}),
        )
        .unwrap();
    assert_eq!(popular["items"][0]["title"], "Demo Manga");

    let search = engine()
        .run(
            &package,
            Operation::Search,
            json!({"query": "demo", "page": 1, "filters": {}}),
        )
        .unwrap();
    assert_eq!(search["items"][0]["title"], "Demo Manga");
}

#[test]
fn maps_manga_details_and_chapters() {
    let package = package();
    let update = engine()
        .run(
            &package,
            Operation::GetMangaUpdate,
            json!({
                "manga": minimal_manga(),
                "chapters": [],
                "fetchDetails": true,
                "fetchChapters": true
            }),
        )
        .unwrap();

    assert_eq!(update["manga"]["description"], "A test description.");
    assert_eq!(
        update["manga"]["alternativeTitles"],
        json!(["Demo Alternative", "デモ"])
    );
    assert_eq!(update["chapters"][0]["key"], CHAPTER_ID);
    assert_eq!(
        update["chapters"][0]["name"],
        "Vol. 2 Ch. 10.5 — A New Start"
    );
    assert_eq!(update["chapters"][0]["numberValue"], 10.5);
    assert_eq!(update["chapters"][0]["volumeValue"], 2.0);
    assert_eq!(update["chapters"][0]["scanlators"], json!(["Demo Group"]));
}

#[test]
fn resolves_full_quality_at_home_pages() {
    let package = package();
    let pages = engine()
        .run(
            &package,
            Operation::GetPages,
            json!({
                "manga": minimal_manga(),
                "chapter": {
                    "key": CHAPTER_ID,
                    "name": "Chapter 10.5"
                }
            }),
        )
        .unwrap();

    assert_eq!(pages.as_array().unwrap().len(), 2);
    assert_eq!(
        pages[0]["imageUrl"],
        "https://uploads.example.test/token/data/chapter-hash/page-1.png"
    );
    assert_eq!(
        pages[1]["imageUrl"],
        "https://uploads.example.test/token/data/chapter-hash/page%202.png"
    );
}
