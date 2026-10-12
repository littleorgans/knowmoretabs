//! The shared frontend, embedded for release and editable in debug builds:
//! its files, the views its pages link, and what those pages may load.
//!
//! slice: library
//! why: Offline exports need no installed assets, while frontend edits during
//!      development should not require recompiling the Rust binary. The
//!      navigation and the content security policy are each defined once
//!      here, for the Library's page, the model service's page the shell
//!      serves, the shell's own page when that service cannot answer, and
//!      an export.

use std::borrow::Cow;

use crate::error::Error;

/// What the Library's page may load: its own scripts, styles, images and
/// connections, nothing inline and nothing from elsewhere. `web/index.html`
/// carries exactly this as its meta policy, so an exported copy keeps it.
const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'";

/// What an HTML page the shell serves may load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// The Library and the shell's own pages.
    Library,
    /// The model service's pages, whose scripts also set inline styles (a
    /// tag's coverage on Tag); still no inline script.
    Service,
}

impl Policy {
    /// As a header, which alone can also refuse framing.
    pub fn header(self) -> String {
        let policy = match self {
            Self::Library => Cow::Borrowed(POLICY),
            Self::Service => {
                Cow::Owned(POLICY.replace("style-src 'self';", "style-src 'self' 'unsafe-inline';"))
            }
        };
        format!("{policy}; frame-ancestors 'none'")
    }
}

const ASSETS: [(&str, &str); 5] = [
    ("index.html", include_str!("../web/index.html")),
    ("app.css", include_str!("../web/app.css")),
    ("app.js", include_str!("../web/app.js")),
    ("unavailable.html", include_str!("../web/unavailable.html")),
    ("unavailable.js", include_str!("../web/unavailable.js")),
];

/// The files an export writes beside its page, `index.html`; the
/// unavailable page and its script are the shell's alone.
pub const EXPORTED: [&str; 2] = ["app.css", "app.js"];

/// One of `ASSETS` by name: from the checkout in a debug build, falling back
/// to the embedded copy.
pub fn load(name: &str) -> Option<Cow<'static, str>> {
    let &(_, embedded) = ASSETS.iter().find(|&&(asset, _)| asset == name)?;
    Some(if cfg!(debug_assertions) {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("web")
                .join(name),
        )
        .map_or(Cow::Borrowed(embedded), Cow::Owned)
    } else {
        Cow::Borrowed(embedded)
    })
}

/// Where a page is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Live, from `serve`, the model service's screens beside the Library's.
    Serve,
    /// Files from `export`, the Library alone.
    Export,
}

/// One view in the nav: its name, where `serve` links it, where an export
/// does (none when only `serve` has it), and the id a router marks it by.
struct View {
    key: &'static str,
    name: &'static str,
    served: &'static str,
    exported: Option<&'static str>,
    id: Option<&'static str>,
}

/// Pages, Tag, Add link, Snapshots, as every page's nav shows them. Served
/// links are the shell's own paths, so moving between the Library and the
/// model service's screens loads that document; within one, only the
/// fragment changes. `nav-tag` and `nav-add` are the service router's.
const VIEWS: [View; 4] = [
    View {
        key: "pages",
        name: "Pages",
        served: "/#pages",
        exported: Some("#pages"),
        id: None,
    },
    View {
        key: "tag",
        name: "Tag",
        served: "/model/#tag",
        exported: None,
        id: Some("nav-tag"),
    },
    View {
        key: "add",
        name: "Add link",
        served: "/model/#add",
        exported: None,
        id: Some("nav-add"),
    },
    View {
        key: "snapshots",
        name: "Snapshots",
        served: "/#snapshots",
        exported: Some("#snapshots"),
        id: None,
    },
];

/// The nav for `surface`, the view keyed `current` marked as the page shown.
pub fn nav(surface: Surface, current: Option<&str>) -> String {
    let links: String = VIEWS
        .iter()
        .filter_map(|view| {
            let href = match surface {
                Surface::Serve => view.served,
                Surface::Export => view.exported?,
            };
            let id = view
                .id
                .map(|id| format!(r#" id="{id}""#))
                .unwrap_or_default();
            let marked = if current == Some(view.key) {
                r#" aria-current="page""#
            } else {
                ""
            };
            Some(format!(r#"<a href="{href}"{id}{marked}>{}</a>"#, view.name))
        })
        .collect();
    format!(r#"<nav class="views" aria-label="Views">{links}</nav>"#)
}

/// `html` with what lies between `<!-- {name}:start -->` and
/// `<!-- {name}:end -->` replaced by `content`, the markers kept.
pub fn fill(html: &str, name: &str, content: &str) -> Result<String, Error> {
    let (start, end) = (
        format!("<!-- {name}:start -->"),
        format!("<!-- {name}:end -->"),
    );
    let missing = || Error::AssetMarkers(name.to_owned());
    let from = html.find(&start).ok_or_else(missing)? + start.len();
    let to = html[from..].find(&end).ok_or_else(missing)? + from;
    Ok(format!("{}{content}{}", &html[..from], &html[to..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_page_carries_the_policy_its_header_extends() {
        let index = load("index.html").unwrap();
        assert!(index.contains(&format!(
            r#"<meta http-equiv="Content-Security-Policy" content="{POLICY}">"#
        )));
        assert_eq!(
            Policy::Library.header(),
            format!("{POLICY}; frame-ancestors 'none'")
        );
        let service = Policy::Service.header();
        assert!(service.contains("; style-src 'self' 'unsafe-inline'; "));
        assert_eq!(
            service.replace(" 'unsafe-inline'", ""),
            Policy::Library.header()
        );
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;

    #[test]
    fn serve_links_four_views_in_order_and_an_export_two_fragments() {
        assert_eq!(
            nav(Surface::Serve, None),
            r#"<nav class="views" aria-label="Views"><a href="/#pages">Pages</a><a href="/model/#tag" id="nav-tag">Tag</a><a href="/model/#add" id="nav-add">Add link</a><a href="/#snapshots">Snapshots</a></nav>"#
        );
        assert_eq!(
            nav(Surface::Export, Some("pages")),
            r##"<nav class="views" aria-label="Views"><a href="#pages" aria-current="page">Pages</a><a href="#snapshots">Snapshots</a></nav>"##
        );
        assert!(nav(Surface::Serve, Some("tag")).contains(r#"id="nav-tag" aria-current="page">"#));
    }

    /// The checked in page opens from disk as an export would show it.
    #[test]
    fn every_page_has_its_markers_and_the_librarys_holds_the_exported_nav() {
        let index = load("index.html").unwrap();
        let exported = format!(
            "<!-- nav:start -->{}<!-- nav:end -->",
            nav(Surface::Export, Some("pages"))
        );
        assert!(index.contains(&exported));
        let unavailable = load("unavailable.html").unwrap();
        assert!(fill(&unavailable, "nav", "N").is_ok());
        assert!(fill(&unavailable, "reason", "R").is_ok());
        for name in EXPORTED {
            assert!(load(name).is_some(), "{name}");
        }
    }

    #[test]
    fn fill_replaces_between_its_markers_only() {
        let html = "a<!-- x:start -->old<!-- x:end -->b<!-- x:end -->";
        assert_eq!(
            fill(html, "x", "new").unwrap(),
            "a<!-- x:start -->new<!-- x:end -->b<!-- x:end -->"
        );
        assert!(fill("no markers", "x", "new").is_err());
        assert!(fill("<!-- x:end --><!-- x:start -->", "x", "new").is_err());
    }
}
