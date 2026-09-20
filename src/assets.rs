//! The shared frontend, embedded for release and editable in debug builds.
//!
//! slice: library
//! why: Offline exports need no installed assets, while frontend edits during
//!      development should not require recompiling the Rust binary.

use std::borrow::Cow;

// slice: library
// why: Slice 2b will move these three assets to web/ and change this seam.
const ASSETS: [(&str, &str); 3] = [
    ("index.html", include_str!("../design-b/index.html")),
    ("app.css", include_str!("../design-b/app.css")),
    ("app.js", include_str!("../design-b/app.js")),
];

pub fn load() -> [(&'static str, Cow<'static, str>); 3] {
    ASSETS.map(|(name, embedded)| {
        let content = if cfg!(debug_assertions) {
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("design-b")
                    .join(name),
            )
            .map_or(Cow::Borrowed(embedded), Cow::Owned)
        } else {
            Cow::Borrowed(embedded)
        };
        (name, content)
    })
}
