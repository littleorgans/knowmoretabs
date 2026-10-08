"""A stand in for `knowmoretabs --root <root> add|forget|restore` on a synthetic archive, for the Add link tests.

`add --json [--signed-in] -- <url>` prints the contract's stage events (one JSON object per line, flushed) and writes
what the real command would: an intake line, a content file and its log line, an image and its log line. The URL's
last path segment picks the outcome (`SCENARIOS`; anything else is a web page captured with an image). A known URL
is `known`, a forgotten one is refused as `forgotten` and anything but a public http(s) page as `refused`, both
exiting 1. `forget` and `restore` edit `library.json`'s forgotten list. `FAKE_KMT_DELAY` (seconds) paces the lines;
`FAKE_KMT_LOG` names a file that gets each argument list. Every page, title and text here is invented.
"""

import hashlib
import json
import os
import sys
import time
import urllib.parse
from pathlib import Path

AT = "2026-10-08T12:00:00Z"
TITLE = "An invented page on zeppelin timetables"
BODY = "Zeppelin timetables and airship routes, invented for a test, with a night train or two."
# segment: (content status, tier, http status, image status, exit code); status None: content never runs
SCENARIOS = {
    "blocked": ("blocked", "web", 403, "none", 0),
    "login": ("behind_login", "web", 401, "none", 0),
    "paywalled": ("paywalled", "web", None, "ok", 0),
    "timeout": ("timeout", "web", None, "none", 0),
    "error": ("error", "web", 503, "none", 0),
    "missing": ("not_found", "web", 404, "none", 0),
    "thin": ("thin", "headless", 200, "ok", 0),
    "skipped": ("skipped", "web", None, "none", 0),
    "noimage": ("ok", "web", 200, "none", 0),
    "unreachable": ("chrome_not_running", "signed_in", None, None, 0),
}


def emit(event: dict) -> None:
    time.sleep(float(os.environ.get("FAKE_KMT_DELAY", "0")))
    print(json.dumps(event), flush=True)


def append(path: Path, row: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as f:
        f.write(json.dumps(row) + "\n")


def known(root: Path) -> set[str]:
    urls = set()
    for snapshot in (root / "snapshots").glob("*/snapshot.json"):
        urls |= {tab["url"] for tab in json.loads(snapshot.read_text())["tabs"]}
    added = root / "pages" / "added.jsonl"
    if added.exists():
        urls |= {json.loads(line)["url"] for line in added.read_text().splitlines() if line.strip()}
    return urls


def web(url: str) -> bool:
    parts = urllib.parse.urlsplit(url)
    return parts.scheme in ("http", "https") and parts.hostname not in (None, "localhost", "127.0.0.1")


def library(root: Path) -> dict:
    return json.loads((root / "library.json").read_text())


def capture(root: Path, url: str, scenario: tuple, signed_in: bool, added: bool) -> None:
    status, tier, http, image, _ = scenario
    name = hashlib.sha256(url.encode()).hexdigest()
    if signed_in and status != "chrome_not_running":
        tier, status, http, image = "signed_in", "ok", 200, "ok"
    if tier == "headless":
        emit({"stage": "content", "state": "running", "tier": "web"})
    emit({"stage": "content", "state": "running", "tier": tier})
    if status == "timeout":
        emit({"stage": "content", "state": "retrying", "after_s": 2})
    done = {"stage": "content", "state": "done", "status": status, "tier": tier}
    if http is not None:
        done["http_status"] = http
    if status in ("ok", "thin"):
        done["title"] = TITLE
    if status in ("ok", "thin") and added:  # a known page's address is never written
        append(root / "pages" / "added.jsonl", {"schema_version": 1, "url": url, "added_at": AT, "title": TITLE})
    if status == "ok":
        content = root / "pages" / "content" / f"{name}.md"
        content.parent.mkdir(parents=True, exist_ok=True)
        content.write_text(f"---\nurl: {url}\n---\n# {TITLE}\n\n{BODY}\n")
    append(root / "pages" / "content.jsonl", {"schema_version": 1, "url": url, "status": status, "attempted_at": AT})
    emit(done)
    if image is None:
        return
    emit({"stage": "image", "state": "running"})
    if image == "ok":
        from PIL import Image

        path = root / "pages" / "images" / f"{name}.jpg"
        path.parent.mkdir(parents=True, exist_ok=True)
        Image.new("RGB", (64, 36), (200, 120, 40)).save(path, "JPEG")
    append(root / "pages" / "images.jsonl", {"schema_version": 1, "url": url, "status": image, "attempted_at": AT})
    emit({"stage": "image", "state": "done", "status": image})


def add(root: Path, url: str, signed_in: bool) -> int:
    emit({"stage": "library", "state": "running"})
    scenario = SCENARIOS.get(
        urllib.parse.urlsplit(url).path.rstrip("/").rsplit("/", 1)[-1], ("ok", "web", 200, "ok", 0)
    )
    if not web(url):
        emit({"stage": "library", "state": "done", "value": "refused", "reason": "not_web"})
        value = "refused"
    elif url in library(root).get("forgotten", []):
        emit({"stage": "library", "state": "done", "value": "forgotten"})
        value = "forgotten"
    else:
        value = "known" if url in known(root) else "added"
        if value == "added":
            append(root / "pages" / "added.jsonl", {"schema_version": 1, "url": url, "added_at": AT})
        emit({"stage": "library", "state": "done", "value": value})
    if value not in ("added", "known"):
        emit({"stage": "done", "state": "done", "url": url, "value": value, "content": None, "image": None})
        return 1
    capture(root, url, scenario, signed_in, value == "added")
    emit({"stage": "done", "state": "done", "url": url, "value": value, "content": scenario[0], "image": scenario[3]})
    return scenario[4]


def main(argv: list[str]) -> int:
    if log := os.environ.get("FAKE_KMT_LOG"):
        append(Path(log), {"argv": argv})
    assert argv[0] == "--root", argv
    root, command, rest = Path(argv[1]), argv[2], argv[3:]
    url = rest[rest.index("--") + 1]
    if command == "add":
        assert "--json" in rest, rest
        return add(root, url, "--signed-in" in rest)
    state = library(root)
    forgotten = [u for u in state.get("forgotten", []) if u != url]
    state["forgotten"] = forgotten + [url] if command == "forget" else forgotten
    (root / "library.json").write_text(json.dumps(state, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
