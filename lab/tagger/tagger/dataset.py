"""Step 1: one record per owner labelled page plus the known pages without owner tags.

Input A mirrors what `tag --prompt` hands an agent (`src/prompt.rs` PageLine, export shape, no History
signals): tab title, URL host and path, and the page's own head metadata from `enrich`.
"""

import collections
import json
import re
import urllib.parse

from .archive import Archive
from .paths import Paths, write_json

MIN_POSITIVES = 10
README_LIMIT = 1500  # prompt.rs README_LIMIT


def collapse(text: str | None) -> str:
    return re.sub(r"\s+", " ", text or "").strip()


def host_path(url: str) -> str:
    parts = urllib.parse.urlsplit(url)
    host = (parts.hostname or "").removeprefix("www.")
    path = urllib.parse.unquote(parts.path).rstrip("/")
    return host + path


def metadata_lines(title: str, record: dict | None) -> list[str]:
    """The fields `prompt::PageLine::enrich` keeps, as labelled lines."""
    if not record:
        return []
    og = record.get("og") or {}
    twitter = record.get("twitter") or {}
    lines = []
    page_title = collapse(record.get("title"))
    if page_title and page_title.lower() != title.lower():
        lines.append(f"page title: {page_title}")
    description = next(
        (
            d.strip()
            for d in (record.get("description"), og.get("description"), twitter.get("description"))
            if d and d.strip()
        ),
        None,
    )
    if description:
        lines.append(f"description: {collapse(description)}")
    if collapse(og.get("site_name")):
        lines.append(f"site: {collapse(og.get('site_name'))}")
    kinds = record.get("jsonld_types") or []
    kind = ", ".join(kinds) if kinds else collapse(og.get("type"))
    if kind and kind != "website":
        lines.append(f"kind: {kind}")
    github = record.get("github") or {}
    if github.get("topics"):
        lines.append("topics: " + ", ".join(github["topics"]))
    if collapse(github.get("readme")):
        lines.append("readme: " + collapse(github["readme"])[:README_LIMIT])
    return lines


def record(archive: Archive, url: str) -> dict:
    """One known page's record; content and image paths point under the archive's root."""
    title = collapse(archive.titles.get(url))
    tags = archive.owner_tags.get(url, [])
    return {
        "key": url,
        "title": title,
        "host": (urllib.parse.urlsplit(url).hostname or "").removeprefix("www."),
        "a_text": "\n".join([host_path(url), *metadata_lines(title, archive.metadata.get(url))]),
        "has_metadata": url in archive.metadata,
        "tags": tags,
        "labelled": bool(tags),
        "forgotten": url in archive.forgotten,
        "text_ok": url in archive.text_ok,
        "content_path": str(archive.text_ok[url]) if url in archive.text_ok else None,
        "image_ok": url in archive.image_ok,
        "image_path": str(archive.image_ok[url]) if url in archive.image_ok else None,
    }


def run(paths: Paths) -> None:
    archive = Archive.load(paths.snapshot)
    records = [record(archive, url) for url in archive.known]
    counts = collections.Counter(t for r in records for t in r["tags"])
    order = sorted(archive.active.values(), key=lambda n: (-counts[n], n.lower()))
    trained = [n for n in order if counts[n] >= MIN_POSITIVES]
    paths.dataset.mkdir(parents=True, exist_ok=True)
    with (paths.dataset / "pages.jsonl").open("w") as f:
        for r in records:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    labelled = [r for r in records if r["labelled"]]
    summary = {
        "known_pages": len(records),
        "labelled_pages": len(labelled),
        "unlabelled_pages": len(records) - len(labelled),
        "owner_associations": sum(counts.values()),
        "active_tags": len(order),
        "trained_tags": len(trained),
        "labelled_text_ok": sum(r["text_ok"] for r in labelled),
        "labelled_image_ok": sum(r["image_ok"] for r in labelled),
        "labelled_both_ok": sum(r["text_ok"] and r["image_ok"] for r in labelled),
        "labelled_with_metadata": sum(r["has_metadata"] for r in labelled),
    }
    write_json(
        paths.dataset / "tags.json",
        {
            "positives": {n: counts[n] for n in order},
            "trained": trained,
            "not_trained": [n for n in order if n not in trained],
        },
    )
    write_json(paths.dataset / "summary.json", summary)
    print(json.dumps(summary))


def load(paths: Paths) -> tuple[list[dict], dict]:
    with (paths.dataset / "pages.jsonl").open() as f:
        records = [json.loads(line) for line in f]
    return records, json.loads((paths.dataset / "tags.json").read_text())
