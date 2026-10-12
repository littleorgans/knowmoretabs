//! One page's capture as `serve` shows it: the latest content and image
//! lines, and the text kept for it.
//!
//! slice: content
//! why: The drawer's Content tab reads what `content` recorded, never
//!      fetches. The two logs and the text file are joined here by the
//!      page's exact address, the key both stores use, so the drawer and
//!      the stores cannot disagree about which file is a page's. Only a
//!      page the library lists is answered, by the rule `forget` uses, so
//!      the route reveals nothing the library does not.

use std::path::Path;

use serde::Serialize;

use crate::archive::Archive;
use crate::error::Error;
use crate::{content_store, image_store, jsonl, library};

/// The route's reply, the lines in their recorded shape.
#[derive(Debug, Serialize)]
pub struct Served {
    pub url: String,
    pub content: Option<content_store::Line>,
    /// The kept markdown, without its front matter.
    pub text: Option<String>,
    pub image: Option<image_store::Line>,
}

/// `url`'s capture, or `None` when the library does not list it. Read
/// under the archive lock, as the library is, so a `save` or a capture in
/// flight is seen whole.
pub fn served(root: &Path, url: &str) -> Result<Option<Served>, Error> {
    let archive = Archive::open(root)?;
    let _lock = archive.lock(|| {})?;
    if !library::known_urls(&library::load(&archive)?.snapshots).contains(url) {
        return Ok(None);
    }
    let file = content_store::dir(root).join(content_store::file_name(url));
    let text = jsonl::kept_file(&file)?.map(|bytes| {
        let text = String::from_utf8_lossy(&bytes);
        content_store::parse(&text).map_or_else(|| text.to_string(), |(_, body)| body.to_owned())
    });
    Ok(Some(Served {
        url: url.to_owned(),
        content: content_store::read(root)?.pages.remove(url),
        text,
        image: image_store::read(root)?.pages.remove(url),
    }))
}
