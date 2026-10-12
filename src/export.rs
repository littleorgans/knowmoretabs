//! Publishes the offline site's assets and embedded library data.
//!
//! slice: library
//! why: JSON is inert data only if HTML cannot terminate its script element;
//!      staging complete files also prevents a failed write from truncating a site.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::archive::{self, SNAPSHOTS_DIR};
use crate::assets::{self, Surface};
use crate::error::Error;
use crate::library::Library;

/// The page `export` writes, whose files are `assets::EXPORTED`.
const PAGE: &str = "index.html";

/// Caller holds the archive lock through reading inputs and publication.
pub fn write(root: &Path, destination: &Path, library: &Library) -> Result<(), Error> {
    let json = serde_json::to_string(library).map_err(|source| Error::Json {
        path: destination.join(PAGE),
        source,
    })?;
    let html = page(Surface::Export, &json)?;
    let mut files: Vec<(&str, std::borrow::Cow<'static, str>)> = assets::EXPORTED
        .iter()
        .filter_map(|&name| Some((name, assets::load(name)?)))
        .collect();
    // Each file is replaced atomically; the page referencing the assets goes last.
    // This is a rebuildable site, not an atomic multi-file snapshot publication.
    files.push((PAGE, html.into()));
    check_destination(root, destination)?;
    archive::create_private_dir(destination)
        .map_err(Error::io("create export directory", destination))?;
    let stage = tempfile::Builder::new()
        .prefix(".export-")
        .tempdir_in(destination)
        .map_err(Error::io("stage export in", destination))?;
    for (name, content) in &files {
        let path = stage.path().join(name);
        let mut file = fs::File::create(&path).map_err(Error::io("create", &path))?;
        file.write_all(content.as_bytes())
            .map_err(Error::io("write", &path))?;
        file.sync_all().map_err(Error::io("sync", &path))?;
    }
    for (name, _) in &files {
        let path = destination.join(name);
        fs::rename(stage.path().join(name), &path)
            .map_err(Error::io("publish export file", &path))?;
    }
    archive::sync_dir(destination)
}

/// The Library's page for `surface`: its nav, and `json` as its data.
pub fn page(surface: Surface, json: &str) -> Result<String, Error> {
    let html = assets::load(PAGE).ok_or_else(|| Error::AssetMarkers(PAGE.to_owned()))?;
    let html = assets::fill(&html, "nav", &assets::nav(surface, Some("pages")))?;
    embed(&html, json)
}

/// An empty `json` yields the serve host's page: the element present, nothing in it.
pub fn embed(html: &str, json: &str) -> Result<String, Error> {
    let safe = json.replace("</", "<\\/");
    let element =
        format!("\n<script id=\"library-data\" type=\"application/json\">{safe}</script>\n");
    assets::fill(html, "library-data", &element)
}

fn check_destination(root: &Path, destination: &Path) -> Result<(), Error> {
    let root = root
        .canonicalize()
        .map_err(Error::io("resolve archive root", root))?;
    let destination = resolve_destination(destination)?;
    if crate::platform::contains_path(&destination, &root)
        || crate::platform::contains_path(&root.join(SNAPSHOTS_DIR), &destination)
    {
        return Err(Error::ExportDestination(destination));
    }
    Ok(())
}

/// Resolve existing ancestors (including symlinks) before creating any output.
pub fn resolve_destination(path: &Path) -> Result<std::path::PathBuf, Error> {
    let absolute =
        std::path::absolute(path).map_err(Error::io("resolve export directory", path))?;
    let mut resolved = std::path::PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            _ => resolved.push(component),
        }
        // A Windows path begins `C:` then `\`, and `C:` on its own is
        // drive-relative: resolving it would name the current directory on
        // that drive rather than its root. Nothing is resolved until there
        // is a directory to resolve.
        if matches!(
            component,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        ) {
            continue;
        }
        match resolved.canonicalize() {
            Ok(path) => resolved = path,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(Error::io("resolve export directory", path)(err)),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "<!-- library-data:start -->";
    const END: &str = "<!-- library-data:end -->";

    #[test]
    fn embedding_replaces_the_fixture_and_escapes_script_terminators() {
        let template = format!("before{START}old fixture{END}after");
        let json = r#"{"title":"</script><script>bad</script>"}"#;
        let html = embed(&template, json).unwrap();
        assert!(!html.contains("old fixture"));
        assert!(html.contains(r"<\/script>"));
        assert_eq!(html.matches("</script>").count(), 1);
        assert!(html.ends_with(&format!("{END}after")));
        assert!(embed("missing markers", "{}").is_err());
        assert!(embed(&format!("{END}{START}"), "{}").is_err());
    }
}
