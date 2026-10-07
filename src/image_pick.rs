//! Which image best shows what a page is, chosen before any image is
//! fetched: what a candidate is, the rung it stands on, and the order the
//! candidates of a page are tried in.
//!
//! slice: content
//! why: The owner keeps one image per page, the best one, so a library row
//!      and a later tagger can see the page's subject rather than a site's
//!      logo. A route that reads a document through an API knows its image
//!      best (a post's photo, a video's thumbnail, a repository's social
//!      preview); otherwise the page's head says what it wants shown when
//!      shared, and failing that its main text holds its images
//!      (`image_page` reads both). A site's default image, shown for every
//!      page, is tried only after the page's own, and a picture too small or
//!      too narrow to show is not kept. Pure, so every rule is tested on
//!      synthetic data.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::guard;
use crate::metadata;

/// The candidates a line keeps, so a retry needs no page.
pub const KEPT: usize = 5;
/// The least either side of a kept image may be, in pixels.
pub const MIN_SIDE: u32 = 200;
/// The longest side may be at most this many times the shortest.
const MAX_ASPECT: u32 = 3;
/// Library pages on one host sharing an image make it the site's default.
pub const SITE_DEFAULT: usize = 3;
/// GitHub's generated repository cards.
const GITHUB_CARD_HOST: &str = "opengraph.githubassets.com";

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The page is an image.
    PageItself,
    XMedia,
    YoutubeThumbnail,
    /// A repository's own social preview.
    GithubSocial,
    GithubReadme,
    /// GitHub's generated repository card.
    GithubCard,
    OgImage,
    TwitterImage,
    JsonldImage,
    ItempropImage,
    BodyImg,
    /// A source a newer build wrote.
    #[serde(other)]
    Unknown,
}

impl Source {
    /// The rung of the ladder it stands on: the page itself, the route's
    /// own, the head, the main text.
    fn rung(self) -> u8 {
        match self {
            Self::PageItself => 0,
            Self::XMedia
            | Self::YoutubeThumbnail
            | Self::GithubSocial
            | Self::GithubReadme
            | Self::GithubCard => 1,
            Self::OgImage | Self::TwitterImage | Self::JsonldImage | Self::ItempropImage => 2,
            Self::BodyImg | Self::Unknown => 3,
        }
    }
}

/// An image address and what named it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub url: String,
    pub source: Source,
}

impl Candidate {
    pub fn new(url: String, source: Source) -> Self {
        Self { url, source }
    }

    /// A card a service drew from the page's name, not an image of it.
    pub fn generated(&self) -> bool {
        self.source == Source::GithubCard
            || Url::parse(&self.url).is_ok_and(|url| guard::host_key(&url) == GITHUB_CARD_HOST)
    }
}

/// What a capture learnt about a page's image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Found {
    /// The capture read nothing of the page itself, so what `enrich`
    /// recorded of its head stands in.
    #[default]
    Unread,
    /// What the page names, rung by rung; empty when it names nothing.
    Candidates(Vec<Candidate>),
    /// A post with no media has no image of its own.
    TextOnly,
}

/// `raw` as an absolute web address against `base`; `None` for an empty
/// value, a `data:` URI or anything not `http` or `https`.
pub fn resolve(base: &Url, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw
            .get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
    {
        return None;
    }
    let url = base.join(raw).ok()?;
    guard::is_web(&url).then(|| url.to_string())
}

/// What a page that is not HTML names: itself when it is an image, as
/// served from `final_url`, else nothing.
pub fn not_html(mime: &str, final_url: &str) -> Found {
    Found::Candidates(if mime.starts_with("image/") {
        vec![Candidate::new(final_url.to_owned(), Source::PageItself)]
    } else {
        Vec::new()
    })
}

/// Why an image `width` × `height` pixels is not kept: too small to show,
/// or a strip rather than a picture. `None` when it fits.
pub fn misfit(width: u32, height: u32) -> Option<&'static str> {
    if width < MIN_SIDE || height < MIN_SIDE {
        return Some("too small");
    }
    let (long, short) = (width.max(height), width.min(height));
    (u64::from(long) > u64::from(short) * u64::from(MAX_ASPECT)).then_some("out of proportion")
}

/// What `enrich` recorded of a page's head, for a page this run did not
/// read: its Open Graph image, then its Twitter image.
pub fn recorded(record: &metadata::Record) -> Vec<Candidate> {
    if record.status != metadata::Status::Ok {
        return Vec::new();
    }
    let Ok(base) = Url::parse(record.final_url.as_deref().unwrap_or(&record.url)) else {
        return Vec::new();
    };
    let og = record.og.as_ref().and_then(|og| og.image.as_deref());
    let twitter = record.twitter.as_ref().and_then(|t| t.image.as_deref());
    [(og, Source::OgImage), (twitter, Source::TwitterImage)]
        .into_iter()
        .filter_map(|(raw, source)| resolve(&base, raw?).map(|url| Candidate::new(url, source)))
        .collect()
}

/// Images a site gives many of its pages: one shared by [`SITE_DEFAULT`]
/// or more library pages on the same host, by what `enrich` recorded.
#[derive(Debug, Default)]
pub struct SiteDefaults(HashSet<(String, String)>);

impl SiteDefaults {
    pub fn of(metadata: &metadata::Metadata) -> Self {
        let mut pages: HashMap<(String, String), HashSet<&str>> = HashMap::new();
        for record in metadata.pages.values() {
            let Ok(page) = Url::parse(&record.url) else {
                continue;
            };
            for candidate in recorded(record) {
                pages
                    .entry((guard::host_key(&page), candidate.url))
                    .or_default()
                    .insert(&record.url);
            }
        }
        Self(
            pages
                .into_iter()
                .filter(|(_, pages)| pages.len() >= SITE_DEFAULT)
                .map(|(key, _)| key)
                .collect(),
        )
    }

    fn contains(&self, page: &Url, image: &str) -> bool {
        self.0.contains(&(guard::host_key(page), image.to_owned()))
    }
}

/// The candidates in the order they are tried: rung by rung, with a head
/// image that is its site's default after the page's own images, each
/// address once, at most [`KEPT`].
pub fn ladder(found: Vec<Candidate>, page: &Url, defaults: &SiteDefaults) -> Vec<Candidate> {
    let mut ranked: Vec<(u8, Candidate)> = found
        .into_iter()
        .map(|candidate| {
            let rung = candidate.source.rung();
            let demoted = rung == 2 && defaults.contains(page, &candidate.url);
            (if demoted { 4 } else { rung }, candidate)
        })
        .collect();
    ranked.sort_by_key(|(rung, _)| *rung);
    let mut seen = HashSet::new();
    ranked
        .into_iter()
        .map(|(_, candidate)| candidate)
        .filter(|candidate| seen.insert(candidate.url.clone()))
        .take(KEPT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(found: &[Candidate]) -> Vec<&str> {
        found.iter().map(|c| c.url.as_str()).collect()
    }

    #[test]
    fn a_kept_image_is_big_enough_and_not_a_strip() {
        assert_eq!(misfit(1200, 630), None);
        assert_eq!(misfit(200, 600), None);
        assert_eq!(misfit(199, 400), Some("too small"));
        assert_eq!(misfit(400, 120), Some("too small"));
        assert_eq!(misfit(1201, 400), Some("out of proportion"));
        assert_eq!(misfit(300, 901), Some("out of proportion"));
    }

    fn record(url: &str, og: Option<&str>, twitter: Option<&str>) -> metadata::Record {
        let line = serde_json::json!({
            "url": url,
            "fetched_at": "2026-10-07T09:00:00Z",
            "status": "ok",
            "og": og.map(|image| serde_json::json!({"image": image})),
            "twitter": twitter.map(|image| serde_json::json!({"image": image})),
        });
        serde_json::from_value(line).unwrap()
    }

    #[test]
    fn what_enrich_recorded_stands_in_for_an_unread_head() {
        let found = recorded(&record(
            "https://a.test/p",
            Some("/og.jpg"),
            Some("https://a.test/tw.jpg"),
        ));
        let named: Vec<(&str, Source)> = found.iter().map(|c| (c.url.as_str(), c.source)).collect();
        assert_eq!(
            named,
            [
                ("https://a.test/og.jpg", Source::OgImage),
                ("https://a.test/tw.jpg", Source::TwitterImage),
            ]
        );
        let mut failed = record("https://a.test/q", Some("/og.jpg"), None);
        failed.status = metadata::Status::Error;
        assert_eq!(recorded(&failed), []);
    }

    #[test]
    fn a_site_default_is_tried_after_the_page_own_images() {
        let mut metadata = metadata::Metadata::default();
        for (url, og) in [
            ("https://a.test/1", "/default.jpg"),
            ("https://a.test/2", "https://a.test/default.jpg"),
            ("https://a.test/3", "/default.jpg"),
            ("https://a.test/4", "/own.jpg"),
            ("https://b.test/1", "https://a.test/default.jpg"),
        ] {
            metadata
                .pages
                .insert(url.to_owned(), record(url, Some(og), None));
        }
        let defaults = SiteDefaults::of(&metadata);
        let page = Url::parse("https://a.test/5").unwrap();
        let found = vec![
            Candidate::new("https://a.test/default.jpg".into(), Source::OgImage),
            Candidate::new("https://a.test/tw.jpg".into(), Source::TwitterImage),
            Candidate::new("https://a.test/body.jpg".into(), Source::BodyImg),
            Candidate::new("https://a.test/tw.jpg".into(), Source::BodyImg),
        ];
        assert_eq!(
            urls(&ladder(found.clone(), &page, &defaults)),
            [
                "https://a.test/tw.jpg",
                "https://a.test/body.jpg",
                "https://a.test/default.jpg"
            ]
        );
        let elsewhere = Url::parse("https://b.test/2").unwrap();
        assert_eq!(
            urls(&ladder(found, &elsewhere, &defaults)),
            [
                "https://a.test/default.jpg",
                "https://a.test/tw.jpg",
                "https://a.test/body.jpg"
            ],
            "only two b.test pages share it"
        );
    }

    #[test]
    fn the_ladder_puts_route_images_first_and_keeps_five() {
        let page = Url::parse("https://a.test/").unwrap();
        let mut found: Vec<Candidate> = (0..6)
            .map(|i| Candidate::new(format!("https://a.test/{i}.jpg"), Source::BodyImg))
            .collect();
        found.push(Candidate::new(
            "https://a.test/og.jpg".into(),
            Source::OgImage,
        ));
        found.push(Candidate::new(
            "https://a.test/native.jpg".into(),
            Source::XMedia,
        ));
        let tried = ladder(found, &page, &SiteDefaults::default());
        assert_eq!(
            urls(&tried),
            [
                "https://a.test/native.jpg",
                "https://a.test/og.jpg",
                "https://a.test/0.jpg",
                "https://a.test/1.jpg",
                "https://a.test/2.jpg"
            ]
        );
    }

    #[test]
    fn generated_cards_are_marked() {
        let card = Candidate::new(
            "https://opengraph.githubassets.com/abc/owner/repo".into(),
            Source::OgImage,
        );
        assert!(card.generated());
        assert!(Candidate::new("https://x.test/c.png".into(), Source::GithubCard).generated());
        assert!(!Candidate::new("https://x.test/c.png".into(), Source::GithubSocial).generated());
    }
}
