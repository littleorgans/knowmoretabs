"""A stand in for `knowmoretabs --root <root> add|forget|restore` on a synthetic archive, for the Add link tests.

`add --json [--title <T>] [--signed-in] [--retry <content|image> …] -- <url>` prints the contract's stage events (one
JSON object per line, flushed) and writes what the real command would: an intake line, a content file and its log
line, an image and its log line. The URL's last path segment picks the first run's outcome (`SCENARIOS`; anything
else is a web page captured with an image). Later runs follow the page's log lines as the contract says: a plain add
reads the text again after an error and retries a failed image; `--signed-in` reads a page whose public tier was
blocked, behind a login or paywalled (`TRANSIENT` pages find Chrome out of reach unless it is a Retry); `--retry`
runs only the stages named, each only where it failed, and reports every other stage as recorded. A text read again
is kept (`stuck` fails every time) and the third failed run is `unavailable`. A known URL is `known`; a forgotten one
is refused as `forgotten`, anything but a public http(s) page as `refused`, and under `--retry` an unknown one as
`refused`, `not_in_library`, all exiting 1. `--title` or `--no-content` with `--retry` is a usage error: exit 2,
nothing on stdout. `forget` and `restore` edit `library.json`'s forgotten list. `FAKE_KMT_DELAY` (seconds) paces the
lines; `FAKE_KMT_LOG` names a file that gets each argument list, and with it the add right after a `restore` of a
`relapse` page fails with the error document. Every page, title and text here is invented.
"""

import json
import os
import sys
import time
import urllib.parse
from pathlib import Path

from tagger.archive import latest_log, sha256_hex

AT = "2026-10-08T12:00:00Z"
TITLE = "An invented page on zeppelin timetables"
BODY = "Zeppelin timetables and airship routes, invented for a test, with a night train or two."
RUNS = 3  # failed runs before a stage is unavailable
PUBLIC = ("web", "github", "x", "youtube", "headless")  # tiers a signed in read starts from
SIGN_IN = ("blocked", "behind_login", "paywalled")
UNRECORDED = ("off", "not_running", "not_allowed", "unknown")  # reported, never written


def text(status: str, tier: str | None = "web", http: int | None = None, reason: str | None = None) -> dict:
    return {"status": status, "tier": tier, "http_status": http, "reason": reason}


# segment: the first run's text and image (None: no image line, as when the text fails)
SCENARIOS = {
    "blocked": (text("blocked", http=403), "none"),
    "login": (text("behind_login", http=401), "none"),
    "paywalled": (text("paywalled"), "ok"),
    "timeout": (text("error", reason="timeout"), None),
    "error": (text("error", http=503), None),
    "stuck": (text("error", http=503), None),
    "missing": (text("not_found", http=404), "none"),
    "thin": (text("thin", "headless", 200), "ok"),
    "skipped": (text("skipped", None, reason="personal app"), "none"),
    "video": (text("unknown", None, reason="not_recorded"), None),
    "noimage": (text("ok", http=200), "none"),
    "imagefail": (text("ok", http=200), "error"),
    "bothfail": (text("error", http=503), "error"),  # as after an earlier run left a failed image
    "offline": (text("blocked", http=403), "none"),
    "asleep": (text("blocked", http=403), "none"),
    "refused": (text("blocked", http=403), "none"),
}
TRANSIENT = {"offline": "off", "asleep": "not_running", "refused": "not_allowed"}  # Chrome as a signed in add finds it
READ = {False: text("ok", http=200), True: text("ok", "signed_in", 200)}  # a text read again, by signed in or not


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
    return urls | set(latest_log(root / "pages" / "added.jsonl"))


def web(url: str) -> bool:
    parts = urllib.parse.urlsplit(url)
    return parts.scheme in ("http", "https") and parts.hostname not in (None, "localhost", "127.0.0.1")


def library(root: Path) -> dict:
    return json.loads((root / "library.json").read_text())


def latest(root: Path, log: str, url: str) -> dict | None:
    return latest_log(root / "pages" / log).get(url)


def failures(root: Path, log: str, url: str) -> int:
    path = root / "pages" / log
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()] if path.exists() else []
    return sum(row["url"] == url and row["status"] == "error" for row in rows)


def record(root: Path, log: str, url: str, row: dict) -> None:
    if row["status"] == "error" and failures(root, log, url) == RUNS - 1:
        row = {**row, "status": "unavailable", "reason": f"failed on {RUNS} runs"}
    line = {"schema_version": 1, "url": url, "attempted_at": AT, **{k: v for k, v in row.items() if v is not None}}
    append(root / "pages" / log, line)


def reported(root: Path, url: str, row: dict | None) -> dict:
    """A content `done` line from what is recorded (or from an unrecorded outcome)."""
    row = row or text("unknown", None, reason="not_recorded")
    done = {"stage": "content", "state": "done", "status": row["status"], "tier": row.get("tier")}
    done |= {k: row[k] for k in ("reason", "http_status") if row.get(k) is not None}
    if (root / "pages" / "content" / f"{sha256_hex(url)}.md").exists():
        done["title"] = TITLE
    return done


def eligible(row: dict | None, signed_in: bool) -> bool:
    if signed_in:
        return row is not None and row["status"] in SIGN_IN and row.get("tier") in PUBLIC
    return row is None or row["status"] == "error"


def capture_text(root: Path, url: str, seg: str, args: dict, fresh: bool) -> tuple[str, bool]:
    """The content stage: its status, and whether a text settled this run."""
    row, signed_in = latest(root, "content.jsonl", url), args["signed_in"]
    if (args["retry"] and "content" not in args["retry"]) or not eligible(row, signed_in):
        done = reported(root, url, None if signed_in and row is None else row)
        emit(done)
        return done["status"], False
    if signed_in:
        transient = TRANSIENT.get(seg) if not args["retry"] else None
        out = text(transient, "signed_in") if transient else READ[True]
    else:
        out = SCENARIOS.get(seg, (READ[False],))[0] if fresh or seg == "stuck" else READ[False]
    if out["tier"] == "headless":
        emit({"stage": "content", "state": "running", "tier": "web"})
    emit({"stage": "content", "state": "running", "tier": out["tier"]})
    if out["reason"] == "timeout":
        emit({"stage": "content", "state": "retrying", "after_s": 2})
    if out["status"] in UNRECORDED:
        emit(reported(root, url, out))
        return out["status"], False
    if out["status"] == "ok":
        content = root / "pages" / "content" / f"{sha256_hex(url)}.md"
        content.parent.mkdir(parents=True, exist_ok=True)
        content.write_text(f"---\nurl: {url}\n---\n# {TITLE}\n\n{BODY}\n")
    record(root, "content.jsonl", url, out)
    intake = latest(root, "added.jsonl", url)
    if out["status"] in ("ok", "thin") and intake and not intake.get("title"):  # a title joins the page's line
        append(root / "pages" / "added.jsonl", {"schema_version": 1, "url": url, "added_at": AT, "title": TITLE})
    done = reported(root, url, latest(root, "content.jsonl", url))
    emit(done)
    return done["status"], done["status"] != "error"


def capture_image(root: Path, url: str, seg: str, args: dict, fresh: bool, settled: bool) -> str:
    row = latest(root, "images.jsonl", url)
    emit({"stage": "image", "state": "running"})
    if fresh:
        status = SCENARIOS.get(seg, (None, "ok"))[1]
    elif row is None and settled:  # the image follows a text this run settled
        status = "ok"
    elif row and row["status"] == "error" and ("image" in args["retry"] or not (args["retry"] or args["signed_in"])):
        status = "ok"
    else:
        status = None
    if status == "ok":
        from PIL import Image

        path = root / "pages" / "images" / f"{sha256_hex(url)}.jpg"
        path.parent.mkdir(parents=True, exist_ok=True)
        Image.new("RGB", (64, 36), (200, 120, 40)).save(path, "JPEG")
    if status is not None:
        record(root, "images.jsonl", url, {"status": status})
    row = latest(root, "images.jsonl", url)
    status = row["status"] if row else "unknown"
    emit({"stage": "image", "state": "done", "status": status})
    return status


def relapsed(url: str) -> bool:
    """A `relapse` page's add fails once, right after its restore."""
    log = os.environ.get("FAKE_KMT_LOG")
    if not log or not url.endswith("/relapse"):
        return False
    before = [json.loads(line)["argv"] for line in Path(log).read_text().splitlines()][-2:-1]
    return bool(before) and before[0][2] == "restore" and before[0][-1] == url


def add(root: Path, url: str, args: dict) -> int:
    if relapsed(url):
        print(json.dumps({"error": {"kind": "io", "message": "could not write", "detail": None}}), flush=True)
        return 1
    emit({"stage": "library", "state": "running"})
    if not web(url):
        emit({"stage": "library", "state": "done", "value": "refused", "reason": "not_web"})
        value = "refused"
    elif url in library(root).get("forgotten", []):
        emit({"stage": "library", "state": "done", "value": "forgotten"})
        value = "forgotten"
    elif args["retry"] and url not in known(root):
        emit({"stage": "library", "state": "done", "value": "refused", "reason": "not_in_library"})
        value = "refused"
    else:
        value = "known" if url in known(root) else "added"
        if value == "added":
            title = {"title": args["title"]} if args["title"] else {}
            append(root / "pages" / "added.jsonl", {"schema_version": 1, "url": url, "added_at": AT, **title})
        emit({"stage": "library", "state": "done", "value": value})
    if value not in ("added", "known"):
        emit({"stage": "done", "state": "done", "url": url, "value": value, "content": None, "image": None})
        return 1
    seg = urllib.parse.urlsplit(url).path.rstrip("/").rsplit("/", 1)[-1]
    fresh = not args["signed_in"] and not latest(root, "content.jsonl", url) and not latest(root, "images.jsonl", url)
    content, settled = capture_text(root, url, seg, args, fresh)
    image = capture_image(root, url, seg, args, fresh, settled)
    emit({"stage": "done", "state": "done", "url": url, "value": value, "content": content, "image": image})
    return 0


def parse(flags: list[str]) -> dict | None:
    """`add`'s flags, or None for a usage error."""
    args = {"retry": [], "title": None, "signed_in": False, "no_content": False, "json": False}
    it = iter(flags)
    for flag in it:
        name, _, value = flag.partition("=")
        if name in ("--retry", "--title"):
            value = value or next(it, "")
            if name == "--retry":
                args["retry"] += value.split(",")
            else:
                args["title"] = value
        elif flag in ("--signed-in", "--no-content", "--json"):
            args[flag[2:].replace("-", "_")] = True
        else:
            return None
    if not set(args["retry"]) <= {"content", "image"}:
        return None
    if args["retry"] and (args["title"] is not None or args["no_content"]):
        return None
    if args["no_content"] and args["signed_in"]:
        return None
    return args


def main(argv: list[str]) -> int:
    if log := os.environ.get("FAKE_KMT_LOG"):
        append(Path(log), {"argv": argv})
    assert argv[0] == "--root", argv
    root, command, rest = Path(argv[1]), argv[2], argv[3:]
    url = rest[rest.index("--") + 1]
    if command == "add":
        args = parse(rest[: rest.index("--")])
        if args is None:
            print("error: usage", file=sys.stderr)
            return 2
        assert args["json"] and not args["no_content"], rest
        return add(root, url, args)
    state = library(root)
    forgotten = [u for u in state.get("forgotten", []) if u != url]
    state["forgotten"] = forgotten + [url] if command == "forget" else forgotten
    (root / "library.json").write_text(json.dumps(state, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
