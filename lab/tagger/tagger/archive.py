"""Read only view of a knowmoretabs archive copy, following the Rust code's rules.

Known pages: public tab URLs across readable snapshots (`library::known_urls`, `public_domain`), and the pages
added one at a time (`pages/added.jsonl`, folded by `intake::snapshot` as one more snapshot after the dated ones).
Title: the most recent non blank tab title, snapshots ascending (`library::build`); an added page's comes from the
latest intake line for its URL.
Owner labels: `library.json` tags[url].add resolved through active vocabulary (`State::page_tags`).
Last use: an active term's `used_at`, when an add last put it on a page (`tags::apply`); a time that does not parse
is no use.
Capture ok: the latest valid schema 1 log line is `ok` and its hash named file exists
(`content_store::file_name`, `image_store::read_kept`). Latest follows file order (`jsonl::parse`).
Page metadata: latest `pages/metadata.jsonl` record, used only when status is ok (`prompt::PageLine::enrich`).
Added: when a page entered the library, the earliest of its snapshots' `captured_at` and its latest intake line's
`added_at`. Per page: the Library's First seen dates every intake page by the log's first line.
"""

import hashlib
import ipaddress
import json
import urllib.parse
from dataclasses import dataclass
from datetime import datetime
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


def instant(raw) -> float | None:
    """Seconds since the epoch of an RFC 3339 time with its offset, as `jiff::Timestamp` writes it; else None."""
    try:
        at = datetime.fromisoformat(raw)
    except (TypeError, ValueError):
        return None
    return at.timestamp() if at.tzinfo else None


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
    added: dict  # url -> when it entered the library, epoch seconds
    titles: dict
    forgotten: set
    active: dict  # folded name -> spelling
    retired: set  # folded names the vocabulary holds retired
    used: dict  # folded active name -> its last use, epoch seconds
    owner_tags: dict  # url -> sorted active tags
    tag_adds: dict  # url -> the names its `add` list holds, retired ones included
    tag_removes: dict  # url -> the names its `remove` list holds (dismissed), retired ones included
    metadata: dict
    text_ok: dict  # url -> content path
    image_ok: dict  # url -> image path

    @classmethod
    def load(cls, root: Path) -> "Archive":
        state = json.loads((root / "library.json").read_bytes())
        assert state["schema_version"] == 1
        vocab = state.get("vocabulary", {})
        active = {name.lower(): name for name, term in vocab.items() if term.get("retired_at") is None}
        retired = {name.lower() for name, term in vocab.items() if term.get("retired_at") is not None}
        used = {
            name.lower(): at
            for name, term in vocab.items()
            if term.get("retired_at") is None and (at := instant(term.get("used_at"))) is not None
        }
        titles: dict = {}
        known: set = set()
        added: dict = {}

        def seen(url, at):
            if at is not None:
                added[url] = min(added.get(url, at), at)

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
                seen(tab["url"], instant(record.get("captured_at")))
                titles.setdefault(tab["url"], "")
                if tab.get("title", "").strip():
                    titles[tab["url"]] = tab["title"]
        for url, row in latest_log(root / "pages/added.jsonl", ("url", "added_at")).items():
            if not public(url):
                continue
            known.add(url)
            seen(url, instant(row["added_at"]))
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
            added=added,
            titles=titles,
            forgotten=set(state.get("forgotten", [])),
            active=active,
            retired=retired,
            used=used,
            owner_tags=owner_tags,
            tag_adds={url: page.get("add", []) for url, page in state.get("tags", {}).items()},
            tag_removes={url: page.get("remove", []) for url, page in state.get("tags", {}).items()},
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
