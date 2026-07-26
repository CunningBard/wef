//! Shared WEF 0.0.1 manifest and operation data types.

use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const WEF_VERSION: &str = "0.0.1";
pub const WEF_VERSION_0_0_2: &str = "0.0.2";

pub type JsonValue = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub wef: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub languages: Vec<String>,
    #[serde(rename = "baseUrls")]
    pub base_urls: Vec<String>,
    pub requires: Vec<Capability>,
    pub listings: Vec<Listing>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub network: Option<NetworkPolicy>,
}

impl Manifest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.wef != WEF_VERSION && self.wef != WEF_VERSION_0_0_2 {
            return Err(ValidationError::InvalidField {
                field: "wef",
                reason: format!(
                    "expected {WEF_VERSION} or {WEF_VERSION_0_0_2}, got {}",
                    self.wef
                ),
            });
        }

        if self.wef == WEF_VERSION
            && (self.network.is_some()
                || self.capabilities.settings
                || self.capabilities.image_transforms
                || self.requires.iter().any(|capability| {
                    matches!(capability, Capability::Browser | Capability::Image)
                }))
        {
            return Err(ValidationError::InvalidField {
                field: "wef",
                reason: "0.0.2 capabilities require wef version 0.0.2".into(),
            });
        }

        if let Some(network) = &self.network {
            network.validate()?;
        }

        for (index, capability) in self.requires.iter().enumerate() {
            if self.requires[index + 1..].contains(capability) {
                return Err(ValidationError::InvalidField {
                    field: "requires",
                    reason: format!("duplicate capability {capability:?}"),
                });
            }
        }

        if self.capabilities.image_transforms && !self.requires.contains(&Capability::Image) {
            return Err(ValidationError::InvalidField {
                field: "capabilities.imageTransforms",
                reason: "requires the image host capability".into(),
            });
        }

        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
        {
            return Err(ValidationError::InvalidField {
                field: "id",
                reason: "must contain only ASCII letters, digits, '.', '-', and '_'".into(),
            });
        }

        if self.name.trim().is_empty() {
            return Err(ValidationError::InvalidField {
                field: "name",
                reason: "must not be empty".into(),
            });
        }

        if self.version.trim().is_empty() {
            return Err(ValidationError::InvalidField {
                field: "version",
                reason: "must not be empty".into(),
            });
        }

        validate_package_relative_path(&self.entry).map_err(|reason| {
            ValidationError::InvalidField {
                field: "entry",
                reason,
            }
        })?;

        if self
            .languages
            .iter()
            .any(|language| language.trim().is_empty())
        {
            return Err(ValidationError::InvalidField {
                field: "languages",
                reason: "language tags must not be empty".into(),
            });
        }

        for base_url in &self.base_urls {
            let parsed = Url::parse(base_url).map_err(|error| ValidationError::InvalidField {
                field: "baseUrls",
                reason: format!("invalid URL {base_url:?}: {error}"),
            })?;

            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(ValidationError::InvalidField {
                    field: "baseUrls",
                    reason: format!("must be an HTTP(S) origin: {base_url:?}"),
                });
            }
        }

        if self.listings.is_empty() {
            return Err(ValidationError::InvalidField {
                field: "listings",
                reason: "must contain at least one listing".into(),
            });
        }

        for (index, listing) in self.listings.iter().enumerate() {
            if listing.id.trim().is_empty() || listing.name.trim().is_empty() {
                return Err(ValidationError::InvalidField {
                    field: "listings",
                    reason: format!("listing {index} must have an id and name"),
                });
            }
        }

        for (index, left) in self.listings.iter().enumerate() {
            if self.listings[index + 1..]
                .iter()
                .any(|right| right.id == left.id)
            {
                return Err(ValidationError::InvalidField {
                    field: "listings",
                    reason: format!("duplicate listing id {:?}", left.id),
                });
            }
        }

        Ok(())
    }

    pub fn has_listing(&self, listing_id: &str) -> bool {
        self.listings.iter().any(|listing| listing.id == listing_id)
    }
}

pub fn validate_package_relative_path(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("must not be empty".into());
    }

    if value.contains('\\') {
        return Err("must use package-relative forward-slash paths".into());
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return Err("must be relative to the package root".into());
    }

    if path.components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("must not contain '.', '..', or root components".into());
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Listing {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Http,
    Html,
    Browser,
    Image,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    #[serde(default)]
    pub filters: bool,
    #[serde(default)]
    pub url_resolution: bool,
    #[serde(default)]
    pub image_requests: bool,
    #[serde(default)]
    pub migrations: bool,
    #[serde(default)]
    pub settings: bool,
    #[serde(default)]
    pub image_transforms: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
}

impl NetworkPolicy {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(rate_limit) = &self.rate_limit
            && (rate_limit.max_requests == 0 || rate_limit.window_ms == 0)
        {
            return Err(ValidationError::InvalidField {
                field: "network.rateLimit",
                reason: "maxRequests and windowMs must be positive".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub max_requests: u32,
    pub window_ms: u64,
}

/// Host-supplied configuration declared by `getSettings` in WEF 0.0.2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(flatten)]
    pub kind: SettingKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SettingKind {
    Text {
        #[serde(default)]
        default: Option<String>,
    },
    Toggle {
        #[serde(default)]
        default: Option<bool>,
    },
    Select {
        options: Vec<FilterOption>,
        #[serde(default)]
        default: Option<String>,
    },
    MultiSelect {
        options: Vec<FilterOption>,
        #[serde(default)]
        default: Option<Vec<String>>,
    },
}

/// A source-defined search or browsing control exposed by `getFilters`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: FilterKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FilterKind {
    Group {
        children: Vec<Filter>,
        #[serde(default)]
        presentation: Option<FilterPresentation>,
    },
    Text {
        #[serde(default)]
        placeholder: Option<String>,
    },
    Toggle {
        #[serde(default)]
        default: Option<bool>,
    },
    Select {
        options: Vec<FilterOption>,
        #[serde(default)]
        default: Option<String>,
    },
    MultiSelect {
        options: Vec<FilterOption>,
        #[serde(default)]
        default: Option<Vec<String>>,
    },
    TriState {
        options: Vec<FilterOption>,
        #[serde(default)]
        default: Option<BTreeMap<String, TriStateValue>>,
    },
    Range {
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default)]
        default: Option<RangeValue>,
    },
    Sort {
        options: Vec<FilterOption>,
        #[serde(default)]
        default: Option<SortValue>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FilterPresentation {
    Section,
    Inline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TriStateValue {
    Include,
    Exclude,
    Neutral,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RangeValue {
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SortValue {
    pub value: String,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FilterOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolveUrlInput {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ResolvedUrl {
    Manga {
        manga_key: String,
    },
    Chapter {
        manga_key: String,
        chapter_key: String,
    },
    Listing {
        listing_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequestInput {
    pub manga: Manga,
    #[serde(default)]
    pub chapter: Option<Chapter>,
    #[serde(default)]
    pub page: Option<Page>,
    pub url: String,
    #[serde(default)]
    pub context: Option<serde_json::Map<String, JsonValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequest {
    pub url: String,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub candidates: Option<Vec<ImageRequestCandidate>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequestCandidate {
    pub url: String,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MigrateMangaKeyInput {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MigrateChapterKeyInput {
    pub manga_key: String,
    pub chapter_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MangaPage {
    pub items: Vec<Manga>,
    pub has_next_page: bool,
}

impl MangaPage {
    pub fn validate(&self) -> Result<(), ValidationError> {
        for manga in &self.items {
            manga.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manga {
    pub key: String,
    pub title: String,
    pub url: Option<String>,
    pub cover_url: Option<String>,
    pub alternative_titles: Option<Vec<String>>,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub artists: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub status: Option<MangaStatus>,
    pub content_rating: Option<ContentRating>,
    pub viewer: Option<Viewer>,
    pub update_strategy: Option<UpdateStrategy>,
    pub next_update_at: Option<String>,
    pub extra: Option<serde_json::Map<String, JsonValue>>,
}

impl Manga {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.key.trim().is_empty() {
            return Err(ValidationError::InvalidField {
                field: "manga.key",
                reason: "must not be empty".into(),
            });
        }
        if self.title.trim().is_empty() {
            return Err(ValidationError::InvalidField {
                field: "manga.title",
                reason: "must not be empty".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MangaStatus {
    Unknown,
    Ongoing,
    Completed,
    Hiatus,
    Cancelled,
    Licensed,
    PublishingFinished,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContentRating {
    Unknown,
    Safe,
    Suggestive,
    Nsfw,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Viewer {
    Unknown,
    LeftToRight,
    RightToLeft,
    Vertical,
    Webtoon,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateStrategy {
    Always,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub key: String,
    pub name: String,
    pub url: Option<String>,
    pub title: Option<String>,
    pub number: Option<String>,
    pub number_value: Option<f64>,
    pub volume: Option<String>,
    pub volume_value: Option<f64>,
    pub language: Option<String>,
    pub published_at: Option<String>,
    pub scanlators: Option<Vec<String>>,
    pub thumbnail_url: Option<String>,
    pub locked: Option<bool>,
    pub extra: Option<serde_json::Map<String, JsonValue>>,
}

impl Chapter {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.key.trim().is_empty() {
            return Err(ValidationError::InvalidField {
                field: "chapter.key",
                reason: "must not be empty".into(),
            });
        }
        if self.name.trim().is_empty() {
            return Err(ValidationError::InvalidField {
                field: "chapter.name",
                reason: "must not be empty".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub url: Option<String>,
    pub image_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub description: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
    pub context: Option<serde_json::Map<String, JsonValue>>,
}

impl Page {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.url.is_none() && self.image_url.is_none() {
            return Err(ValidationError::InvalidField {
                field: "page",
                reason: "must contain url or imageUrl".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MangaListInput {
    pub listing_id: String,
    pub page: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchInput {
    pub query: Option<String>,
    pub page: u32,
    pub filters: serde_json::Map<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MangaUpdateInput {
    pub manga: Manga,
    pub chapters: Vec<Chapter>,
    pub fetch_details: bool,
    pub fetch_chapters: bool,
}

impl MangaUpdateInput {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.fetch_details && !self.fetch_chapters {
            return Err(ValidationError::InvalidField {
                field: "fetchDetails/fetchChapters",
                reason: "at least one fetch flag must be true".into(),
            });
        }
        self.manga.validate()?;
        for chapter in &self.chapters {
            chapter.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MangaUpdate {
    pub manga: Option<Manga>,
    pub chapters: Option<Vec<Chapter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PagesInput {
    pub manga: Manga,
    pub chapter: Chapter,
}

impl MangaUpdate {
    pub fn validate_for(&self, input: &MangaUpdateInput) -> Result<(), ValidationError> {
        if input.fetch_details {
            let manga = self
                .manga
                .as_ref()
                .ok_or(ValidationError::MissingField { field: "manga" })?;
            manga.validate()?;
        }
        if input.fetch_chapters {
            let chapters = self
                .chapters
                .as_ref()
                .ok_or(ValidationError::MissingField { field: "chapters" })?;
            for chapter in chapters {
                chapter.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("missing required field {field:?}")]
    MissingField { field: &'static str },
    #[error("invalid {field}: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            wef: WEF_VERSION.into(),
            id: "org.example.demo".into(),
            name: "Demo".into(),
            version: "0.1.0".into(),
            entry: "source.js".into(),
            languages: vec!["en".into()],
            base_urls: vec!["https://example.com".into()],
            requires: vec![],
            listings: vec![Listing {
                id: "latest".into(),
                name: "Latest".into(),
            }],
            capabilities: Capabilities::default(),
            network: None,
        }
    }

    #[test]
    fn validates_minimal_manifest() {
        assert!(manifest().validate().is_ok());
    }

    #[test]
    fn rejects_unsafe_entry_paths() {
        let mut value = manifest();
        value.entry = "../source.js".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn serializes_camel_case_fields() {
        let input = MangaListInput {
            listing_id: "latest".into(),
            page: 1,
        };
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            serde_json::json!({"listingId": "latest", "page": 1})
        );
    }

    #[test]
    fn image_transforms_require_the_image_host_capability() {
        let mut value = manifest();
        value.wef = WEF_VERSION_0_0_2.into();
        value.capabilities.image_transforms = true;
        assert!(value.validate().is_err());
        value.requires.push(Capability::Image);
        assert!(value.validate().is_ok());
    }
}
