//! The images a page names in its markup: its head's tags and JSON-LD,
//! the `<img>`s of its main text, and the pictures a README shows.
//!
//! slice: content
//! why: A page says which image it wants shown when shared in its head,
//!      and shows its subject in its main text, but most of its `<img>`s
//!      are chrome: logos, icons, avatars, badges, ads and tracking pixels.
//!      Reading them is markup work, kept apart from the ladder that ranks
//!      what any route found: the head in the order its tags are trusted,
//!      the article's images (else the page's, less its menus, banners and
//!      footers) largest declared first, a `srcset`'s smallest entry wide
//!      enough to keep, and anything named or shaped like chrome refused
//!      before a request is made. Pure, and tested on synthetic pages.

use dom_query::Document;
use serde_json::Value;
use url::Url;

use crate::extract;
use crate::head::{self, Head};
use crate::image_pick::{Candidate, Found, Source, resolve};
use crate::jsonld;

/// The `srcset` width chosen: the smallest at least this wide.
pub const SRCSET_WIDTH: u32 = 768;
/// Words in an image's address, class, id or alt that mark it as not the
/// page's subject; a plural counts too.
const NOT_THE_SUBJECT: [&str; 14] = [
    "logo", "icon", "favicon", "avatar", "sprite", "badge", "emoji", "pixel", "spacer", "ad",
    "advert", "tracking", "tracker", "shields",
];

/// What a page read over HTTP names: its head, then its images, from
/// readability's `article` when its text came from there, else the whole
/// page less its chrome. `base` is where the page was fetched from.
pub fn page(html: &str, article: Option<&str>, base: &Url) -> Found {
    let mut found = head_images(&head::scan(html), base);
    let from_article = article.map(|article| body_images(&Document::from(article), base));
    match from_article {
        Some(images) if !images.is_empty() => found.extend(images),
        _ => found.extend(body_images(&extract::without_chrome(html), base)),
    }
    Found::Candidates(found)
}

/// The head's images in the order they are trusted: Open Graph (its
/// secure address first), Twitter, JSON-LD, then `itemprop`.
pub fn head_images(head: &Head, base: &Url) -> Vec<Candidate> {
    let meta = [
        ("og:image:secure_url", Source::OgImage),
        ("og:image", Source::OgImage),
        ("og:image:url", Source::OgImage),
        ("twitter:image", Source::TwitterImage),
        ("twitter:image:src", Source::TwitterImage),
    ];
    let mut found: Vec<Candidate> = meta
        .iter()
        .filter_map(|(key, source)| {
            resolve(base, head.meta(key)?).map(|url| Candidate::new(url, *source))
        })
        .collect();
    for value in jsonld::parse(&head.jsonld) {
        jsonld::walk(&value, &mut |map, top| {
            if !top {
                return;
            }
            for key in ["image", "thumbnailUrl"] {
                if let Some(image) = map.get(key) {
                    found.extend(
                        jsonld_urls(image)
                            .filter_map(|raw| resolve(base, raw))
                            .map(|url| Candidate::new(url, Source::JsonldImage)),
                    );
                }
            }
        });
    }
    // `itemprop="image"` is kept under its bare name.
    if let Some(url) = head.meta("image").and_then(|raw| resolve(base, raw)) {
        found.push(Candidate::new(url, Source::ItempropImage));
    }
    found
}

/// A JSON-LD image: an address, an `ImageObject`, or a list of either.
fn jsonld_urls(value: &Value) -> Box<dyn Iterator<Item = &str> + '_> {
    match value {
        Value::String(url) => Box::new(std::iter::once(url.as_str())),
        Value::Array(items) => Box::new(items.iter().flat_map(jsonld_urls)),
        Value::Object(map) => Box::new(
            ["url", "contentUrl"]
                .into_iter()
                .find_map(|key| map.get(key)?.as_str())
                .into_iter(),
        ),
        _ => Box::new(std::iter::empty()),
    }
}

/// An `<img>` worth fetching: its address, how large the page says it is,
/// and where it stands.
struct Img {
    url: String,
    /// Declared width × height, else the widest `srcset` width squared.
    area: Option<u64>,
    position: usize,
}

/// The document's own images, largest declared first, then in page order;
/// those with no declared size after every sized one.
fn body_images(document: &Document, base: &Url) -> Vec<Candidate> {
    let mut images: Vec<Img> = document
        .select("img")
        .iter()
        .enumerate()
        .filter_map(|(position, img)| {
            let attr = |name: &str| img.attr(name).map(|v| v.to_string());
            let src = ["src", "data-src"]
                .into_iter()
                .filter_map(&attr)
                .find(|raw| resolve(base, raw).is_some());
            let srcset = ["srcset", "data-srcset"]
                .into_iter()
                .find_map(|name| attr(name).and_then(|v| choose_srcset(&v)));
            let url = srcset
                .as_ref()
                .and_then(|(chosen, _)| resolve(base, chosen))
                .or_else(|| resolve(base, src.as_deref()?))?;
            let named: String = ["class", "id", "alt"]
                .into_iter()
                .filter_map(&attr)
                .chain(src)
                .chain([url.clone()])
                .collect::<Vec<_>>()
                .join(" ");
            if names_something_else(&named) {
                return None;
            }
            let side = |name: &str| attr(name).and_then(|v| pixels(&v));
            let (width, height) = (side("width"), side("height"));
            if width.is_some_and(|w| w <= 1) || height.is_some_and(|h| h <= 1) {
                return None;
            }
            if is_vector(&url) {
                return None;
            }
            let area = match (width, height, &srcset) {
                (Some(w), Some(h), _) => Some(u64::from(w) * u64::from(h)),
                (_, _, Some((_, Some(widest)))) => Some(u64::from(*widest).pow(2)),
                _ => None,
            };
            Some(Img {
                url,
                area,
                position,
            })
        })
        .collect();
    images.sort_by_key(|img| (std::cmp::Reverse(img.area), img.position));
    images
        .into_iter()
        .map(|img| Candidate::new(img.url, Source::BodyImg))
        .collect()
}

/// `width="640"` or `640px`; nothing for a percentage or a word.
fn pixels(value: &str) -> Option<u32> {
    value.trim().trim_end_matches("px").trim().parse().ok()
}

/// The `srcset` entry to fetch: the smallest at least [`SRCSET_WIDTH`]
/// wide, else the widest, else the highest density; and the widest width
/// it offers, when it gives widths.
fn choose_srcset(value: &str) -> Option<(String, Option<u32>)> {
    let mut widths: Vec<(u32, &str)> = Vec::new();
    let mut densities: Vec<(f32, &str)> = Vec::new();
    let mut rest = value;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() {
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let url = &rest[..end];
        rest = &rest[end..];
        if let Some(url_only) = url.strip_suffix(',') {
            densities.push((1.0, url_only.trim_end_matches(',')));
            continue;
        }
        let end = rest.find(',').unwrap_or(rest.len());
        let descriptor = rest[..end].trim();
        rest = &rest[end..];
        if let Some(w) = descriptor.strip_suffix('w').and_then(|w| w.parse().ok()) {
            widths.push((w, url));
        } else {
            let x = descriptor
                .strip_suffix('x')
                .and_then(|x| x.parse().ok())
                .unwrap_or(1.0);
            densities.push((x, url));
        }
    }
    let widest = widths.iter().map(|(w, _)| *w).max();
    let by_width = widths
        .iter()
        .filter(|(w, _)| *w >= SRCSET_WIDTH)
        .min_by_key(|(w, _)| *w)
        .or_else(|| widths.iter().max_by_key(|(w, _)| *w))
        .map(|(_, url)| *url);
    let chosen = by_width.or_else(|| {
        densities
            .iter()
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, url)| *url)
    })?;
    Some((chosen.to_owned(), widest))
}

/// An SVG by its address: vector art is icons and logos, and no decoder
/// for it is compiled in.
fn is_vector(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        url.path()
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
    })
}

/// Whether `text` (an address, a class, an id or alt text) names a logo,
/// an icon, an avatar, a badge, a tracking pixel or an ad.
fn names_something_else(text: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|token| {
            NOT_THE_SUBJECT
                .iter()
                .any(|word| token == *word || token.strip_suffix('s') == Some(word))
        })
}

/// The first image a README shows that is a picture of the project: not
/// a badge, an SVG, a logo or an icon. Relative addresses resolve against
/// `base`, where the repository's files are served.
pub fn readme_image(markdown: &str, base: &Url) -> Option<String> {
    let lower = markdown.to_ascii_lowercase();
    let mut at = 0;
    while at < markdown.len() {
        let image = lower[at..].find("![").map(|i| (at + i, false));
        let tag = lower[at..].find("<img").map(|i| (at + i, true));
        let Some((start, is_tag)) = [image, tag].into_iter().flatten().min() else {
            break;
        };
        let (raw, named, next) = if is_tag {
            let (attrs, end) = head::attributes(markdown, start + "<img".len());
            let get = |name: &str| {
                attrs
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            (get("src"), format!("{} {}", get("alt"), get("class")), end)
        } else {
            let Some(close) = markdown[start..].find("](").map(|i| start + i) else {
                break;
            };
            let target = markdown[close + 2..]
                .trim_start()
                .trim_start_matches('<')
                .split(|c: char| c.is_whitespace() || c == ')' || c == '>')
                .next()
                .unwrap_or_default();
            (
                target.to_owned(),
                markdown[start + 2..close].to_owned(),
                close + 2,
            )
        };
        at = next.max(start + 1);
        let Some(url) = resolve(base, &raw) else {
            continue;
        };
        if !is_vector(&url) && !names_something_else(&format!("{named} {url}")) {
            return Some(url);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://a.test/post/1").unwrap()
    }

    fn urls(found: &[Candidate]) -> Vec<&str> {
        found.iter().map(|c| c.url.as_str()).collect()
    }

    fn candidates(found: Found) -> Vec<Candidate> {
        let Found::Candidates(found) = found else {
            panic!("a page read over HTTP names candidates");
        };
        found
    }

    #[test]
    fn the_head_is_read_in_trust_order_and_resolved() {
        let html = r#"<html><head>
            <meta name="twitter:image" content="/tw.jpg">
            <meta property="og:image" content="https://cdn.a.test/og.jpg">
            <meta property="og:image:secure_url" content="https://cdn.a.test/og-secure.jpg">
            <meta itemprop="image" content="item.jpg">
            <script type="application/ld+json">{"@graph":[{"@type":"Article",
              "image":[{"@type":"ImageObject","url":"//cdn.a.test/ld.jpg"}],
              "author":{"@type":"Person","image":"https://a.test/face.jpg"}}]}</script>
            <meta property="og:image" content="data:image/png;base64,AAAA">
            </head><body></body></html>"#;
        let found = candidates(page(html, None, &base()));
        let named: Vec<(&str, Source)> = found.iter().map(|c| (c.url.as_str(), c.source)).collect();
        assert_eq!(
            named,
            [
                ("https://cdn.a.test/og-secure.jpg", Source::OgImage),
                ("https://cdn.a.test/og.jpg", Source::OgImage),
                ("https://a.test/tw.jpg", Source::TwitterImage),
                ("https://cdn.a.test/ld.jpg", Source::JsonldImage),
                ("https://a.test/post/item.jpg", Source::ItempropImage),
            ],
            "an author's picture inside the JSON-LD is not the page's"
        );
    }

    #[test]
    fn body_images_rank_by_declared_size_and_refuse_what_is_not_the_subject() {
        let html = r#"<html><body>
            <img src="/unsized.jpg">
            <img src="/small.jpg" width="300" height="200">
            <img src="/site-logo.png" width="900" height="900">
            <img src="/a.jpg" class="avatar" width="900" height="900">
            <img src="/b.jpg" alt="Sponsored ad" width="900" height="900">
            <img src="/track.gif" width="1" height="1">
            <img src="/art.svg" width="900" height="900">
            <img src="data:image/gif;base64,AAAA">
            <img src="/big.jpg" width="1200" height="800">
            <img src="/wide.jpg" srcset="/wide-480.jpg 480w, /wide-1024.jpg 1024w, /wide-800.jpg 800w">
            <img src="/later.jpg" width="1200" height="800">
            </body></html>"#;
        let found = candidates(page(html, None, &base()));
        assert_eq!(
            urls(&found),
            [
                "https://a.test/wide-800.jpg",
                "https://a.test/big.jpg",
                "https://a.test/later.jpg",
                "https://a.test/small.jpg",
                "https://a.test/unsized.jpg",
            ],
            "a srcset of up to 1024 wide counts as 1024 square"
        );
        assert!(found.iter().all(|c| c.source == Source::BodyImg));
    }

    #[test]
    fn responsive_images_need_no_fetchable_src_and_filter_the_chosen_address() {
        let html = r#"<main>
            <img srcset="/small.jpg 320w, /subject.jpg 800w">
            <img src="data:image/gif;base64,AAAA" data-srcset="/lazy.jpg 900w">
            <img src="/placeholder.jpg" srcset="/avatar.jpg 1200w">
            </main>"#;
        assert_eq!(
            urls(&candidates(page(html, None, &base()))),
            ["https://a.test/lazy.jpg", "https://a.test/subject.jpg"]
        );
    }

    #[test]
    fn the_article_is_looked_in_first_and_page_chrome_never() {
        let html = r#"<html><body>
            <header><img src="/banner.jpg" width="2000" height="1000"></header>
            <main><p>text</p><img src="/in-main.jpg" width="600" height="400"></main>
            <footer><img src="/footer.jpg" width="2000" height="1000"></footer>
            </body></html>"#;
        assert_eq!(
            urls(&candidates(page(html, None, &base()))),
            ["https://a.test/in-main.jpg"]
        );
        let article = r#"<div><img src="https://a.test/lead.jpg"></div>"#;
        assert_eq!(
            urls(&candidates(page(html, Some(article), &base()))),
            ["https://a.test/lead.jpg"]
        );
        let empty = r"<div><p>no images</p></div>";
        assert_eq!(
            urls(&candidates(page(html, Some(empty), &base()))),
            ["https://a.test/in-main.jpg"],
            "an article without images falls back to the page"
        );
    }

    #[test]
    fn srcset_takes_the_smallest_wide_enough_entry() {
        let chosen = |value: &str| choose_srcset(value).map(|(url, _)| url);
        assert_eq!(
            chosen("a.jpg 320w, b.jpg 1600w, c.jpg 768w, d.jpg 1024w").as_deref(),
            Some("c.jpg")
        );
        assert_eq!(
            chosen("a.jpg 320w,b.jpg 640w").as_deref(),
            Some("b.jpg"),
            "none is wide enough: the widest"
        );
        assert_eq!(
            chosen("a.jpg, b.jpg 2x, c.jpg 1.5x").as_deref(),
            Some("b.jpg")
        );
        assert_eq!(
            chosen("https://c.test/w_300,h_200/x.jpg 300w, https://c.test/w_900,h_600/x.jpg 900w")
                .as_deref(),
            Some("https://c.test/w_900,h_600/x.jpg"),
            "commas inside an address do not split it"
        );
        assert_eq!(
            choose_srcset("a.jpg 320w, b.jpg 1600w").unwrap().1,
            Some(1600)
        );
        assert_eq!(chosen(""), None);
    }

    #[test]
    fn names_are_matched_as_words() {
        for refused in [
            "/img/logo.png",
            "site-icons",
            "favicon.ico",
            "user_avatar",
            "https://ads.cdn.test/x.jpg",
            "Tracking pixel",
            "img.shields.io/badge/x",
        ] {
            assert!(names_something_else(refused), "{refused}");
        }
        for kept in [
            "header-photo.jpg",
            "download.png",
            "silicon.jpg",
            "adventure",
        ] {
            assert!(!names_something_else(kept), "{kept}");
        }
    }

    #[test]
    fn readme_images_skip_badges_svg_and_logos_and_resolve_relative_paths() {
        let base = Url::parse("https://raw.githubusercontent.com/owner/repo/HEAD/").unwrap();
        let readme = r#"# Project
[![CI](https://github.com/owner/repo/actions/workflows/ci.yml/badge.svg)](x)
[![crates](https://img.shields.io/crates/v/x)](y)
<p align="center"><img src="docs/logo.png" alt="Project logo" width="200"></p>
![diagram](docs/arch.svg)
<IMG SRC="docs/screenshot.png" alt="The app">
![later](https://cdn.test/later.png)
"#;
        assert_eq!(
            readme_image(readme, &base).as_deref(),
            Some("https://raw.githubusercontent.com/owner/repo/HEAD/docs/screenshot.png")
        );
        let markdown_first =
            "Intro\n\n![Screen shot](<img/one.jpg> \"title\")\n<img src=\"two.jpg\">";
        assert_eq!(
            readme_image(markdown_first, &base).as_deref(),
            Some("https://raw.githubusercontent.com/owner/repo/HEAD/img/one.jpg")
        );
        assert_eq!(readme_image("no images ![alt", &base), None);
        assert_eq!(readme_image("", &base), None);
    }
}
