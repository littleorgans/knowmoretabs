//! The shared frontend, embedded for release and editable in debug builds.
//!
//! slice: library
//! why: Offline exports need no installed assets, while frontend edits during
//!      development should not require recompiling the Rust binary.

use std::borrow::Cow;

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
