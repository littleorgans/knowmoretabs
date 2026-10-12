//! The shared frontend, embedded for release and editable in debug builds,
//! and what its pages may load.
//!
//! slice: library
//! why: Offline exports need no installed assets, while frontend edits during
//!      development should not require recompiling the Rust binary. The
//!      content security policy is defined once here for the exported page's
//!      meta element and every served page's header.

use std::borrow::Cow;

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

const ASSETS: [(&str, &str); 3] = [
    ("index.html", include_str!("../web/index.html")),
    ("app.css", include_str!("../web/app.css")),
    ("app.js", include_str!("../web/app.js")),
];

pub fn load() -> [(&'static str, Cow<'static, str>); 3] {
    ASSETS.map(|(name, embedded)| {
        let content = if cfg!(debug_assertions) {
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("web")
                    .join(name),
            )
            .map_or(Cow::Borrowed(embedded), Cow::Owned)
        } else {
            Cow::Borrowed(embedded)
        };
        (name, content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_page_carries_the_policy_its_header_extends() {
        let [(_, index), ..] = load();
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
