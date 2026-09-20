//! Publishes the offline site's assets and embedded library data.
//!
//! slice: library
//! why: JSON is inert data only if HTML cannot terminate its script element;
//!      staging complete files also prevents a failed write from truncating a site.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::archive::{self, SNAPSHOTS_DIR};
use crate::assets;
use crate::error::Error;
use crate::library::Library;

const START: &str = "<!-- library-data:start -->";
const END: &str = "<!-- library-data:end -->";

/// Caller holds the archive lock through reading inputs and publication.
pub fn write(root: &Path, destination: &Path, library: &Library) -> Result<(), Error> {
    let json = serde_json::to_string(library).map_err(|source| Error::Json {
        path: destination.join("index.html"),
        source,
    })?;
    let [(index_name, index), (css_name, css), (js_name, js)] = assets::load();
    let html = embed(&index, &json)?;
    check_destination(root, destination)?;
    archive::create_private_dir(destination)
        .map_err(Error::io("create export directory", destination))?;
    let stage = tempfile::Builder::new()
        .prefix(".export-")
        .tempdir_in(destination)
        .map_err(Error::io("stage export in", destination))?;
    let files = [
        (css_name, css.as_ref()),
        (js_name, js.as_ref()),
        (index_name, html.as_str()),
    ];
    for (name, content) in files {
        let path = stage.path().join(name);
        let mut file = fs::File::create(&path).map_err(Error::io("create", &path))?;
        file.write_all(content.as_bytes())
            .map_err(Error::io("write", &path))?;
        file.sync_all().map_err(Error::io("sync", &path))?;
    }
    // Each file is replaced atomically; the page referencing the assets goes last.
    // This is a rebuildable site, not an atomic multi-file snapshot publication.
    for (name, _) in files {
        let path = destination.join(name);
        fs::rename(stage.path().join(name), &path)
            .map_err(Error::io("publish export file", &path))?;
    }
    archive::sync_dir(destination)
}

/// An empty `json` yields the serve host's page: the element present, nothing in it.
pub fn embed(html: &str, json: &str) -> Result<String, Error> {
    let start = html.find(START).ok_or(Error::AssetMarkers)? + START.len();
    let end = html[start..].find(END).ok_or(Error::AssetMarkers)? + start;
    let safe = json.replace("</", "<\\/");
    Ok(format!(
        "{}\n<script id=\"library-data\" type=\"application/json\">{safe}</script>\n{}",
        &html[..start],
        &html[end..]
    ))
}

fn check_destination(root: &Path, destination: &Path) -> Result<(), Error> {
    let root = root
        .canonicalize()
        .map_err(Error::io("resolve archive root", root))?;
    let destination = resolve_destination(destination)?;
    if root.starts_with(&destination) || destination.starts_with(root.join(SNAPSHOTS_DIR)) {
        return Err(Error::ExportDestination(destination));
    }
    Ok(())
}

/// Resolve existing ancestors (including symlinks) before creating any output.
fn resolve_destination(path: &Path) -> Result<std::path::PathBuf, Error> {
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
