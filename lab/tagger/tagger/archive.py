"""Read only view of a knowmoretabs archive copy, following the Rust code's rules.

Known pages: public tab URLs across readable snapshots (`library::known_urls`, `public_domain`), and the pages
added one at a time (`pages/added.jsonl`, folded by `intake::snapshot` as one more snapshot after the dated ones).
Title: the most recent non blank tab title, snapshots ascending (`library::build`); an added page's comes from the
latest intake line for its URL.
Owner labels: `library.json` tags[url].add resolved through active vocabulary (`State::page_tags`).
Capture ok: the latest valid schema 1 log line is `ok` and its hash named file exists
(`content_store::file_name`, `image_store::read_kept`). Latest follows file order (`jsonl::parse`).
Page metadata: latest `pages/metadata.jsonl` record, used only when status is ok (`prompt::PageLine::enrich`).
"""

import hashlib
import ipaddress
import json
import urllib.parse
from dataclasses import dataclass
from pathlib import Path


def sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def public(raw: str) -> bool:
    try:
        parsed = urllib.parse.urlsplit(raw)
        host = parsed.hostname
        if parsed.scheme == "file":
            return False
        if host:
            if host == "localhost" or host.endswith(".localhost"):
                return False
            try:
                if ipaddress.ip_address(host).is_loopback:
                    return False
            except ValueError:
                pass
        return True
    except ValueError:
        return True


def latest_log(path: Path, required=("url",)) -> dict:
    result = {}
    if not path.is_file():
        return result
    for raw in path.read_bytes().splitlines():
        if not raw.strip():
            continue
        try:
            row = json.loads(raw)
            if not all(isinstance(row.get(key), str) for key in required):
                continue
            if "schema_version" in row and row["schema_version"] != 1:
                continue
            result[row["url"]] = row
        except ValueError:
            continue
    return result


@dataclass
class Archive:
    root: Path
    known: list  # page keys, sorted
    titles: dict
    forgotten: set
    active: dict  # folded name -> spelling
    owner_tags: dict  # url -> sorted active tags
    metadata: dict
    text_ok: dict  # url -> content path
    image_ok: dict  # url -> image path

    @classmethod
    def load(cls, root: Path) -> "Archive":
        state = json.loads((root / "library.json").read_bytes())
        assert state["schema_version"] == 1
        vocab = state.get("vocabulary", {})
        active = {name.lower(): name for name, term in vocab.items() if term.get("retired_at") is None}
        titles: dict = {}
        known: set = set()
        for directory in sorted((root / "snapshots").iterdir(), key=lambda p: p.name):
            if directory.name.startswith(".") or not directory.is_dir():
                continue
            try:
                record = json.loads((directory / "snapshot.json").read_bytes())
                if record["schema_version"] != 1:
                    continue
                tabs = record["tabs"]
            except (OSError, ValueError, KeyError):
                continue
            for tab in tabs:
                if not public(tab["url"]):
                    continue
                known.add(tab["url"])
                titles.setdefault(tab["url"], "")
                if tab.get("title", "").strip():
                    titles[tab["url"]] = tab["title"]
        for url, row in latest_log(root / "pages/added.jsonl", ("url", "added_at")).items():
            if not public(url):
                continue
            known.add(url)
            titles.setdefault(url, "")
            title = row.get("title")
            if isinstance(title, str) and title.strip():
                titles[url] = title
        owner_tags = {}
        for url, page in state.get("tags", {}).items():
            if url not in known:
                continue
            tags = sorted({active[n.lower()] for n in page.get("add", []) if n.lower() in active})
            if tags:
                owner_tags[url] = tags
        status = ("url", "status", "attempted_at")
        text_ok = {}
        for url, row in latest_log(root / "pages/content.jsonl", status).items():
            path = root / "pages/content" / (sha256_hex(url) + ".md")
            if row["status"] == "ok" and path.is_file():
                text_ok[url] = path
        image_ok = {}
        for url, row in latest_log(root / "pages/images.jsonl", status).items():
            path = root / "pages/images" / (sha256_hex(url) + ".jpg")
            if row["status"] == "ok" and path.is_file():
                image_ok[url] = path
        metadata = {
            url: row
            for url, row in latest_log(root / "pages/metadata.jsonl", ("url", "status")).items()
            if row["status"] == "ok"
        }
        return cls(
            root=root,
            known=sorted(known),
            titles=titles,
            forgotten=set(state.get("forgotten", [])),
            active=active,
            owner_tags=owner_tags,
            metadata=metadata,
            text_ok=text_ok,
            image_ok=image_ok,
        )


def content_body(path: Path) -> str:
    """The markdown body of a content file, front matter dropped (`content_store::parse`).

    Only `embed` calls this; the body never leaves the process except as an embedding.
    """
    text = path.read_bytes().decode("utf-8", errors="replace")
    if not text.startswith("---\n"):
        return text
    rest = text[4:]
    if rest.startswith("---\n"):
        return rest[4:]
    end = rest.find("\n---\n")
    return text if end < 0 else rest[end + 5 :]
