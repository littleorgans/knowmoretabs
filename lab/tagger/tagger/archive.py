"""Read only view of a knowmoretabs archive copy, following the Rust code's rules.

Known pages: public tab URLs across readable snapshots (`library::known_urls`, `public_domain`), and the pages
added one at a time (`pages/added.jsonl`, folded by `intake::snapshot` as one more snapshot after the dated ones).
Title: the most recent non blank tab title, snapshots ascending (`library::build`); an added page's comes from the
latest intake line for its URL.
Decisions: `library.json` read as `State::read` reads it (`LibraryState`): a missing file is an empty library, a
damaged or unsupported one an error. A page's tags are its `add` names the vocabulary holds active, in tag order
(`State::page_tags`); its dismissals, its `remove` names. Retired names keep their decisions. Last use: an active
term's `used_at`, when an add last put it on a page (`tags::apply`); a time that does not parse is no use.
Capture ok: the latest valid schema 1 log line is `ok` and its hash named file exists
(`content_store::file_name`, `image_store::read_kept`). Latest follows file order (`jsonl::parse`).
Page metadata: latest `pages/metadata.jsonl` record, used only when status is ok (`prompt::PageLine::enrich`).
Added: when a page entered the library, the earliest of its snapshots' `captured_at` and its latest intake line's
`added_at`. Per page: the Library's First seen dates every intake page by the log's first line.
"""

import hashlib
import ipaddress
import json
import os
import urllib.parse
from dataclasses import dataclass, field
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


STATE_FILE = "library.json"  # `library::STATE_FILE`
READS = 3  # reads of a file replaced while it was read, before giving up


class Unreadable(Exception):
    """`library.json` is damaged, of another schema, or kept changing while it was read: nothing may be shown from it."""


def signature(path: Path) -> tuple | None:
    """What changes whenever the file is replaced or written: device, inode, size and nanosecond mtime; None while it
    is missing."""
    try:
        return _signature(os.stat(path))
    except FileNotFoundError:
        return None


def _signature(st: os.stat_result) -> tuple:
    return (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns)


def _strings(value, what: str) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
        raise Unreadable(f"{what} is not a list of names")
    return value


def tag_key(name: str) -> tuple[str, str]:
    """`library::tag_order`: alphabetical ignoring case, the exact spelling only to break a tie."""
    return name.lower(), name


@dataclass(frozen=True)
class LibraryState:
    """The owner's decisions in `library.json`, read only: what each page's `add` and `remove` lists hold (retired
    names included, in the file's spelling), the forgotten pages, the active and retired vocabulary by folded name,
    and each active name's last use. `signature` names the bytes it was parsed from."""

    signature: tuple | None = None
    adds: dict = field(default_factory=dict)  # url -> names its `add` list holds
    removes: dict = field(default_factory=dict)  # url -> names its `remove` list holds
    forgotten: frozenset = frozenset()
    active: dict = field(default_factory=dict)  # folded name -> spelling
    retired: dict = field(default_factory=dict)  # folded name -> spelling
    used: dict = field(default_factory=dict)  # folded active name -> its last use, epoch seconds

    @classmethod
    def read(cls, root: Path) -> "LibraryState":
        """The file as one consistent read: its bytes and the signature they came with, read again while the path
        names another file by the time the read ends."""
        path = root / STATE_FILE
        for _ in range(READS):
            try:
                with path.open("rb") as f:
                    seen, raw = _signature(os.fstat(f.fileno())), f.read()
            except FileNotFoundError:
                if signature(path) is None:
                    return cls()
                continue
            except OSError as err:
                raise Unreadable(f"cannot read {STATE_FILE}: {err.strerror}") from None
            if signature(path) == seen:
                return cls.parse(raw, seen)
        raise Unreadable(f"{STATE_FILE} kept changing while it was read")

    @classmethod
    def parse(cls, raw: bytes, seen: tuple | None = None) -> "LibraryState":
        """The fields `State` reads, checked as strictly as serde checks them; any other field is not read."""
        try:
            state = json.loads(raw)
        except ValueError:
            raise Unreadable(f"{STATE_FILE} is not JSON") from None
        if not isinstance(state, dict) or type(state.get("schema_version")) is not int:
            raise Unreadable(f"{STATE_FILE} has no schema_version")
        if state["schema_version"] != 1:
            raise Unreadable(f"{STATE_FILE} has an unsupported schema_version")
        forgotten = frozenset(_strings(state.get("forgotten"), "forgotten"))
        pages, vocabulary = state.get("tags", {}), state.get("vocabulary", {})
        if not isinstance(pages, dict) or not all(isinstance(p, dict) for p in pages.values()):
            raise Unreadable("tags is not a map of pages")
        if not isinstance(vocabulary, dict) or not all(isinstance(t, dict) for t in vocabulary.values()):
            raise Unreadable("vocabulary is not a map of terms")
        active, retired, used = {}, {}, {}
        for name, term in vocabulary.items():
            if (
                instant(term.get("created_at")) is None
                or term.get("retired_at") is not None
                and instant(term["retired_at"]) is None
                or not isinstance(term.get("definition"), str | None)
            ):
                raise Unreadable(f"vocabulary term {name!r} is malformed")
            if term.get("retired_at") is not None:
                retired[name.lower()] = name
                continue
            active[name.lower()] = name
            if (at := instant(term.get("used_at"))) is not None:
                used[name.lower()] = at
        return cls(
            signature=seen,
            adds={url: _strings(p.get("add", []), "a page's add list") for url, p in pages.items()},
            removes={url: _strings(p.get("remove", []), "a page's remove list") for url, p in pages.items()},
            forgotten=forgotten,
            active=active,
            retired=retired,
            used=used,
        )

    def tags(self, url: str) -> list[str]:
        """The page's tags as the library shows them: its `add` names the vocabulary holds active, in its spelling."""
        return sorted({self.active[n.lower()] for n in self.adds.get(url, ()) if n.lower() in self.active}, key=tag_key)

    def dismissed(self, url: str) -> set[str]:
        """Folded names the page's `remove` list holds: taken off it, or a suggestion the owner turned down."""
        return {n.lower() for n in self.removes.get(url, ())}


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
    state: LibraryState
    metadata: dict
    text_ok: dict  # url -> content path
    image_ok: dict  # url -> image path

    @classmethod
    def load(cls, root: Path) -> "Archive":
        state = LibraryState.read(root)
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
            state=state,
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
