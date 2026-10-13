//! One page's capture as `serve` shows it: the latest content and image
//! lines, and its text.
//!
//! slice: content, sources
//! why: The drawer's Content tab reads what `content` recorded, never
//!      fetches. The two logs are joined here by the page's exact address,
//!      the key both stores use, so the drawer and the stores cannot
//!      disagree about which file is a page's; its text is `page_text`'s,
//!      so the drawer shows what search and read do, a local document's
//!      pinned text included. Only a page the library lists is answered,
//!      by the rule `forget` uses, so the route reveals nothing the library
//!      does not.

use std::path::Path;

use serde::Serialize;

use crate::archive::Archive;
use crate::error::Error;
use crate::page_text::Reader;
use crate::{content_store, image_store, library};

/// The route's reply, the lines in their recorded shape.
#[derive(Debug, Serialize)]
pub struct Served {
    pub url: String,
    pub content: Option<content_store::Line>,
    /// A web page's kept markdown, without its front matter, or a local
    /// document's text as pinned.
    pub text: Option<String>,
    pub image: Option<image_store::Line>,
}

/// `url`'s capture, or `None` when the library does not list it. The
/// lines are read under the archive lock, as the library is, so a `save`
/// or a capture in flight is seen whole; the text after it, through the
/// page reader (a document's through `checkouts`), since both stores
/// replace a file whole by a rename, so git and a PDF are read off the lock.
pub fn served(root: &Path, checkouts: Option<&Path>, url: &str) -> Result<Option<Served>, Error> {
    let (content, image) = {
        let archive = Archive::open(root)?;
        let _lock = archive.lock(|| {})?;
        if !library::known_urls(&library::load(&archive)?.snapshots).contains(url) {
            return Ok(None);
        }
        (
            content_store::read(root)?.pages.remove(url),
            image_store::read(root)?.pages.remove(url),
        )
    };
    Ok(Some(Served {
        url: url.to_owned(),
        content,
        text: Reader::new(root, checkouts)
            .text(url)?
            .text
            .map(|text| text.body),
        image,
    }))
}
