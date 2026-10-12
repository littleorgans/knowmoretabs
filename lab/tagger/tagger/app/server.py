"""`tagger app`: search, select and tag on one screen, as a local web app on 127.0.0.1.

Static files come from `static/`; page images are served by their hashed name from the resolved archive; the
JSON API runs the engine and the store. API work is serialized; static and image transfers run independently.
Only the Library shell talks to this server, under its `/model/` prefix: every request must name this server as
Host (no DNS rebinding), come from no other origin, be bounded (JSON for a POST, empty for a GET) and carry the
shell's proof (`service_auth`) before anything routes; every reply to a proven request is signed. The process
prints counts and timings only; the request log is off, since nothing about a page belongs in a terminal.
`/api/rank` is the Library's search: knowmoretabs `serve` forwards its query here and keeps only replies whose
archive header names its own archive.
`--smoke` loads everything, times searches with the owner's tag descriptions as queries, prints numbers, exits.
"""

import json
import mimetypes
from contextlib import contextmanager, nullcontext
import re
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer, ThreadingHTTPServer
from pathlib import Path
from threading import Lock

import numpy as np

from ..paths import Paths, read_json
from ..zeroshot import prototype, query_variants
from . import add, engine, kmt, send, service_auth
from .store import Exclusions, Keys, Store, tag_name

STATIC = Path(__file__).with_name("static")
MAX_RESULTS = 50  # web/app.js RANK_TOP shows the same number of ranked pages
# Every reply names the archive this service loaded; src/model_service.rs reads the same header
ARCHIVE_HEADER = "X-KMT-Archive"
# Seconds the Library's rank waits for the model; an Add link index holds it for longer, and the Library then
# searches by keyword instead of queueing behind it
RANK_WAIT = 0.5
# The largest request body read; the shell forwards nothing larger (src/http.rs BODY_LIMIT)
BODY_LIMIT = 4 * 1024 * 1024
CSP = (
    "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; connect-src 'self'; "
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
)


class App:
    def __init__(self, lib: engine.Library, store: Store, encode: engine.Encode, embed: engine.Embed):
        # every tag name the store holds: made here, or decided (a name the archive since retired included)
        names = [*store.tags(), *dict.fromkeys(tag for _, tag in store.decisions())]
        self.lib, self.store, self.encode, self.embed = engine.add_tags(lib, names, encode), store, encode, embed
        self.lock = Lock()  # the model and mutable app state have one caller at a time
        self.binary = kmt.BINARY
        self.adds = add.Jobs(self)
        self.sends = send.Send(self)
        self.exclusions = Exclusions(store.root)
        self.selection = Keys(store.root, "selection.json")
        self.pins = Keys(store.root, "pinned.json")
        self.forgotten = Keys(store.root, "forgotten.json")

    def gone(self) -> list[int]:
        """Rows of the pages forgotten in the app."""
        return [self.lib.rows[k] for k in self.forgotten.keys if k in self.lib.rows]

    def live(self) -> np.ndarray:
        """Rows the screen shows: the library's live pages less those forgotten in the app."""
        mask = self.lib.live.copy()
        mask[self.gone()] = False
        return mask

    def own(self, row: int) -> list[str]:
        return [t for j, t in enumerate(self.lib.tags) if self.lib.Y[row, j]]

    def sync(self, url: str, archive) -> dict | None:
        """After knowmoretabs changed one page: the library in line with the archive for it, and the page as the
        screen shows it with the tags `suggested` for it, or None when the screen does not show it. Hold the lock."""
        self.lib = engine.sync(self.lib, archive, [url], self.embed)
        row = self.lib.rows.get(url)
        if row is None or not self.live()[row]:
            return None
        page = self.page(row)
        return {**page, "suggested": self.suggested(row, page["tags"])}

    def suggested(self, row: int, held: list[str]) -> list[str]:
        """The app tags the page lacks that the model would put on it, best first: each whose score clears its
        threshold (`engine.prechecks`: a head's, or zero shot for a tag with too few positives or made here), by
        confidence; at most `engine.SUGGEST`."""
        tags = [t for t in self.tag_lists()["app_tags"] if t not in held]
        checked = [s for s in engine.prechecks(self.lib, [row], tags)[0] if s["checked"]]
        return [s["tag"] for s in sorted(checked, key=lambda s: -s["p"])[: engine.SUGGEST]]

    def page(self, row: int) -> dict:
        """A page as the views show it: `tags` are its app tags; `own`, its archive tags, only the review shows."""
        r = self.lib.records[row]
        return {
            "row": row,
            "title": r["title"],
            "host": r["host"],
            "url": r["key"],
            "image": Path(r["image_path"]).stem if r["image_ok"] else None,  # its address: `/img/<hashed name>`
            "own": self.own(row),
            "tags": self.store.app_tags().get(r["key"], []),
        }

    def tag_lists(self) -> dict:
        """`app_tags`: tags made or applied in the app, by name. `recent`: those used, latest first, a use being a
        direct decision putting one on a page or an add the archive records doing so; names it holds retired are
        left out."""
        held = {t for tags in self.store.app_tags().values() for t in tags}
        names = sorted(held | set(self.store.tags()), key=str.lower)
        direct = self.store.used()
        when = {t: max(direct.get(t, 0), self.lib.used.get(t.lower(), 0)) for t in names}
        recent = [t for t in names if when[t] and t.lower() not in self.lib.retired]
        return {"app_tags": names, "recent": sorted(recent, key=lambda t: -when[t])}

    def library(self) -> dict:
        return {
            "pages": int(self.live().sum()),
            "forgotten": self.gone(),
            "tags": engine.tag_info(self.lib),
            **self.tag_lists(),
            "images": self.lib.image is not None,
            "input": self.lib.input_name,
            "sizes": [20, MAX_RESULTS],
        }

    def untagged(self, tag: str | None = None) -> np.ndarray:
        """Shown pages without any app tag, or without the given app tag."""
        mask = self.live()
        mask[
            [
                self.lib.rows[k]
                for k, tags in self.store.app_tags().items()
                if k in self.lib.rows and (tag is None or tag in tags)
            ]
        ] = False
        return mask

    def search(self, body: dict) -> dict:
        """One page of ranked results for `query`, from rank `offset`; with `untagged`, among pages without an app
        tag only. `total` counts the pages ranked. An empty query lists every page, newest first."""
        query = str(body.get("query", ""))
        n, offset = _size(body), _offset(body)
        among = self.untagged() if body.get("untagged") else self.live()
        if not query.strip():
            rows = engine.newest(self.lib, among)[offset : offset + n]
            return {"hits": [self.page(int(r)) for r in rows], "offset": offset, "total": int(among.sum())}
        start = time.perf_counter()
        q = self.encode([query])[0]
        encoded = time.perf_counter()
        images = bool(body.get("images"))
        plain = engine.search(self.lib, q, query, n, images, among, offset)
        while True:
            hits = plain
            if body.get("refine"):
                hits = engine.search(self.lib, self.refined(query, q, plain), query, n, images, among, offset)
            before = self.exclusions.keys(query)
            judged = self.judged(query, [h["row"] for h in hits])
            # A cut can discover new exclusions in the refined ranking. Settle against those too,
            # so reload uses the same inputs. Exclusions only grow here, bounded by the library.
            if not body.get("refine") or self.exclusions.keys(query) == before:
                break
        done = time.perf_counter()
        return {
            "hits": [{**self.page(h["row"]), "fused": h["fused"], "sources": h["sources"]} for h in hits],
            "offset": offset,
            "total": int(among.sum()),
            **judged,
            "ms": {"encode": round(1000 * (encoded - start), 1), "rank": round(1000 * (done - encoded), 1)},
        }

    def rank(self, body: dict) -> dict:
        """Every page this service holds, forgotten ones included, best first for `query` by text and keywords:
        their keys. The Library applies its own filters to the order."""
        query = str(body.get("query", ""))
        if not query.strip():
            raise ValueError("type a search")
        q = self.encode([query])[0]
        rows = engine.order(self.lib, q, query, np.ones(len(self.lib.records), bool))
        return {"keys": [self.lib.records[i]["key"] for i in rows]}

    def refined(self, query: str, q: np.ndarray, hits: list[dict]) -> np.ndarray:
        """The query moved toward the plain ranking's included results and away from every excluded page."""
        self.exclusions.view(query, [self.lib.records[h["row"]]["key"] for h in hits])
        out = self.exclusions.keys(query)
        included = [h["row"] for h in hits if self.lib.records[h["row"]]["key"] not in out]
        excluded = [self.lib.rows[k] for k in out if k in self.lib.rows]
        return prototype(q, self.lib.X[included], self.lib.X[excluded])

    def judged(self, query: str, rows: list[int]) -> dict:
        """The shown results excluded for this query, the cut's row, and tags suggested from the rest."""
        keys = [self.lib.records[r]["key"] for r in rows]
        out, cut = self.exclusions.view(query, keys)
        included = [r for r, k in zip(rows, keys, strict=True) if k not in out]
        return {
            "excluded": [r for r, k in zip(rows, keys, strict=True) if k in out],
            "cut": rows[keys.index(cut)] if cut in keys else None,
            "suggested": engine.suggest(self.lib, included) if included else [],
        }

    def rows(self, body: dict, among: np.ndarray | None = None) -> list[int]:
        """The body's rows, each a page `among` (default: the shown pages)."""
        among = self.live() if among is None else among
        rows = [int(r) for r in body.get("rows", [])]
        if any(r < 0 or r >= len(self.lib.records) or not among[r] for r in rows):
            raise ValueError("a row is not a page of this library")
        return rows

    def exclude(self, body: dict) -> dict:
        """Apply one exclusion action to the shown results (`rows`, in rank order) of a search."""
        query, rows = str(body.get("query", "")), self.rows(body)
        if not query or not rows:
            raise ValueError("name a search and its results")
        keys = [self.lib.records[r]["key"] for r in rows]
        row = body.get("row")
        key = keys[rows.index(int(row))] if row is not None and int(row) in rows else None
        self.exclusions.apply(query, keys, str(body.get("action")), key)
        return self.judged(query, rows)

    def tag(self, body: dict) -> dict:
        """The tag named `name`: the library's own in any case, else made and kept as a new zero shot tag."""
        name = tag_name(str(body.get("name", "")))
        have = engine.find_tag(self.lib, name)
        if have is None:
            self.store.add_tag(name)
            self.lib = engine.add_tags(self.lib, [name], self.encode)
        return {"tag": have or name, "created": have is None, "tags": engine.tag_info(self.lib)}

    def like(self, body: dict) -> dict:
        """The tag's name as a query moved toward the pages holding it and away from those it was taken off
        (refine's prototype), ranked among pages without that app tag: one page from rank `offset`, and `total`."""
        tag = str(body.get("tag", ""))
        if tag not in self.lib.tags:
            raise ValueError("name one of your tags")
        held: dict[bool, list[int]] = {True: [], False: []}
        for (key, t), d in self.store.decisions().items():
            if t == tag and key in self.lib.rows:
                held[d["value"]].append(self.lib.rows[key])
        q = prototype(self.encode([tag])[0], self.lib.X[held[True]], self.lib.X[held[False]])
        among, offset = self.untagged(tag), _offset(body)
        hits = engine.search(self.lib, q, tag, _size(body), bool(body.get("images")), among, offset)
        return {"tag": tag, "hits": [self.page(h["row"]) for h in hits], "offset": offset, "total": int(among.sum())}

    def apply(self, body: dict) -> dict:
        """Add one tag to (`value` true) or take it off the given pages, as direct decisions."""
        rows, tag, value = self.rows(body), str(body.get("tag", "")), body.get("value")
        if not rows or tag not in self.lib.tags or not isinstance(value, bool):
            raise ValueError("name pages, one of your tags, and true or false")
        changed, batch = self.store.apply([self.lib.records[r]["key"] for r in rows], tag, value)
        pages = [{"row": r, "tags": self.page(r)["tags"]} for r in rows]
        return {"pages": pages, "changed": len(changed), "batch": batch, **self.tag_lists()}

    def undo(self, body: dict) -> dict:
        """Retract one apply's decisions (its `batch`): each page it changed back as it was."""
        keys = self.store.undo(int(body.get("batch", 0)))
        rows = [self.lib.rows[k] for k in dict.fromkeys(keys) if k in self.lib.rows]
        return {"pages": [{"row": r, "tags": self.page(r)["tags"]} for r in rows], **self.tag_lists()}

    def listed(self, keys: Keys) -> dict:
        """The selected or pinned pages this library still shows, in the order added."""
        live = self.live()
        rows = [self.lib.rows[k] for k in keys.keys if k in self.lib.rows]
        return {"pages": [self.page(r) for r in rows if live[r]]}

    def replace(self, keys: Keys, body: dict) -> dict:
        """Replace the selection or the pins with the given pages (none clears them)."""
        rows = self.rows(body)
        keys.set([self.lib.records[r]["key"] for r in rows])
        return {"rows": rows}

    def forget(self, body: dict) -> dict:
        """Forget the given pages (`value` true): they leave every search, ≈, the selection and the pins, and the
        next Send to library forgets them in the library. `value` false brings them back (Undo puts back the
        selection and pins)."""
        value = body.get("value")
        if not isinstance(value, bool):
            raise ValueError("name pages, and true or false")
        among = self.lib.live.copy()
        if not value:
            among[self.gone()] = True
        rows = self.rows(body, None if value else among)
        if not rows:
            raise ValueError("name pages, and true or false")
        keys = {self.lib.records[r]["key"] for r in rows}
        if value:
            self.forgotten.set(self.forgotten.keys + sorted(keys))
            for kept in (self.selection, self.pins):
                if keys & set(kept.keys):
                    kept.set([k for k in kept.keys if k not in keys])
        else:
            self.forgotten.set([k for k in self.forgotten.keys if k not in keys])
            self.lib.live[rows] = True  # Undo shows app pages immediately; Send leaves the library forgotten.
        return {"pages": int(self.live().sum())}

    def create(self, body: dict) -> dict:
        """A result set of the given rows less any excluded for the query, so excluded pages never reach review."""
        query = str(body.get("query", ""))
        out = self.exclusions.keys(query)
        rows = [r for r in self.rows(body) if self.lib.records[r]["key"] not in out]
        picked = [str(t) for t in body.get("picked", [])]
        if not rows or not picked:
            raise ValueError("pick at least one tag for at least one included page")
        if unknown := set(picked) - set(self.lib.tags):
            raise ValueError(f"{len(unknown)} picked tag(s) are not in the vocabulary")
        keys = [self.lib.records[r]["key"] for r in rows]
        session = self.store.create(
            query, bool(body.get("images")), keys, picked, engine.prechecks(self.lib, rows, picked)
        )
        return self.session_view(session)

    def page_view(self, page: dict) -> dict | None:
        row = self.lib.rows.get(page["key"])
        if row is None:
            return None
        sugg = [
            {
                "tag": t,
                "checked": page["marks"][t],
                "model": page["model"][t],
                "p": page["p"][t],
                "source": page["source"][t],
            }
            for t in page["marks"]
        ]
        return {**self.page(row), "sugg": sugg, "status": page["status"], "at": page["at"]}

    def session_view(self, s: dict) -> dict:
        pages = [{"index": i, **v} for i, p in enumerate(s["pages"]) if (v := self.page_view(p)) is not None]
        return {k: s[k] for k in ("id", "created_at", "query", "images", "picked")} | {"pages": pages}

    def sessions(self) -> list[dict]:
        return [
            {
                "id": s["id"],
                "created_at": s["created_at"],
                "query": s["query"],
                "picked": s["picked"],
                "pages": sum(bool(p["marks"]) for p in s["pages"]),
                "decided": sum(p["status"] == "decided" and bool(p["marks"]) for p in s["pages"]),
            }
            for s in reversed(self.store.sessions())
        ]

    def update(self, sid: int, index: int, body: dict) -> dict:
        s = self.store.session(sid)
        if s is None or not 0 <= index < len(s["pages"]):
            raise LookupError
        page = self.store.update(sid, index, body.get("marks"), body.get("status"))
        return {"index": index, **self.page_view(page)}


ROUTES = [
    ("GET", re.compile(r"/api/library"), lambda app, m, b: app.library()),
    ("GET", re.compile(r"/api/sessions"), lambda app, m, b: app.sessions()),
    (
        "GET",
        re.compile(r"/api/sessions/(\d+)"),
        lambda app, m, b: _found(app.store.session(int(m[1])), app.session_view),
    ),
    ("POST", re.compile(r"/api/search"), lambda app, m, b: app.search(b)),
    ("POST", re.compile(r"/api/exclusions"), lambda app, m, b: app.exclude(b)),
    ("GET", re.compile(r"/api/selection"), lambda app, m, b: app.listed(app.selection)),
    ("POST", re.compile(r"/api/selection"), lambda app, m, b: app.replace(app.selection, b)),
    ("GET", re.compile(r"/api/pins"), lambda app, m, b: app.listed(app.pins)),
    ("POST", re.compile(r"/api/pins"), lambda app, m, b: app.replace(app.pins, b)),
    ("POST", re.compile(r"/api/forget"), lambda app, m, b: app.forget(b)),
    ("POST", re.compile(r"/api/undo"), lambda app, m, b: app.undo(b)),
    ("POST", re.compile(r"/api/tags"), lambda app, m, b: app.tag(b)),
    ("POST", re.compile(r"/api/apply"), lambda app, m, b: app.apply(b)),
    ("POST", re.compile(r"/api/like"), lambda app, m, b: app.like(b)),
    ("POST", re.compile(r"/api/sessions"), lambda app, m, b: app.create(b)),
    ("POST", re.compile(r"/api/sessions/(\d+)/pages/(\d+)"), lambda app, m, b: app.update(int(m[1]), int(m[2]), b)),
]


# The Library's rank holds the app's lock like the routes above, but waits for it at most RANK_WAIT
RANK_ROUTES = [("POST", re.compile(r"/api/rank"), lambda app, m, b: app.rank(b))]


# Add link jobs and sends run knowmoretabs; they take the app's lock only around its state, so the screen never
# waits on a run
JOB_ROUTES = [
    ("POST", re.compile(r"/api/add"), lambda app, m, b: app.adds.start(b)),
    ("GET", re.compile(r"/api/add/(\d+)"), lambda app, m, b: app.adds.view(int(m[1]))),
    ("POST", re.compile(r"/api/send/preview"), lambda app, m, b: app.sends.preview()),
    ("POST", re.compile(r"/api/send"), lambda app, m, b: app.sends.apply(b.get("preview"))),
]


def _size(body: dict) -> int:
    return min(max(int(body.get("n", 20)), 1), MAX_RESULTS)


def _offset(body: dict) -> int:
    return max(int(body.get("offset", 0)), 0)


def _found(value, view):
    if value is None:
        raise LookupError
    return view(value)


class Busy(Exception):
    """The model stayed in use past a route's wait."""


@contextmanager
def bounded(lock: Lock, wait: float):
    """Hold `lock`, waiting at most `wait` seconds for it, else raise `Busy`."""
    if not lock.acquire(timeout=wait):
        raise Busy
    try:
        yield
    finally:
        lock.release()


def archive_identity(root: Path) -> str:
    """`<device>:<inode>` of the archive directory, both unsigned 64 bit as Rust's `MetadataExt` gives them, so a
    path spelled differently on a case insensitive volume still names the same archive."""
    st = root.stat()
    return f"{st.st_dev % (1 << 64)}:{st.st_ino}"


def handler(app: App, port: int, key: bytes):
    hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}
    archive = archive_identity(app.lib.root)

    class Handler(BaseHTTPRequestHandler):
        server_version = "kmt-tagger"
        sys_version = ""
        proof = None  # (nonce, method, path) once the request is proven: every reply to it is signed

        def log_message(self, format, *args):  # noqa: A002 (the base class's name)
            pass

        def send(self, status: int, body: bytes, kind: str, cache: bool = False) -> None:
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Security-Policy", CSP)
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Cache-Control", "private, max-age=3600" if cache else "no-store")
            self.send_header(ARCHIVE_HEADER, archive)
            if self.proof:
                mac = service_auth.response_mac(key, *self.proof, int(status), kind, archive, body)
                self.send_header(service_auth.RESPONSE_MAC, mac)
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, value) -> None:
            self.send(status, json.dumps(value, ensure_ascii=False).encode(), "application/json; charset=utf-8")

        def admitted(self, method: str) -> bytes | None:
            """The body of a request that names this server, comes from no other origin, is bounded (JSON for a
            POST, empty for a GET) and is proven; else None, once an unsigned refusal is sent. Nothing routes, and
            no app state is touched, before this."""
            if self.headers.get("Host") not in hosts:
                return self.json(HTTPStatus.MISDIRECTED_REQUEST, {"error": "unknown host"})
            origin = self.headers.get("Origin")
            if origin is not None and origin.removeprefix("http://") not in hosts:
                return self.json(HTTPStatus.FORBIDDEN, {"error": "cross origin"})
            if method == "POST" and self.headers.get("Content-Type", "").split(";")[0] != "application/json":
                return self.json(HTTPStatus.UNSUPPORTED_MEDIA_TYPE, {"error": "send JSON"})
            lengths = self.headers.get_all("Content-Length") or ["0"]
            if len(lengths) != 1 or not re.fullmatch(r"[0-9]{1,10}", lengths[0]):
                return self.json(HTTPStatus.BAD_REQUEST, {"error": "bad Content-Length"})
            length = int(lengths[0])
            if length > BODY_LIMIT or (method == "GET" and length):
                return self.json(HTTPStatus.REQUEST_ENTITY_TOO_LARGE, {"error": "body too large"})
            body = self.rfile.read(length)
            nonce = service_auth.proven(key, self.headers, method, self.path, body, time.time_ns() // 1_000_000)
            if nonce is None:
                return self.json(HTTPStatus.FORBIDDEN, {"error": "unproven request"})
            self.proof = (nonce, method, self.path)
            return body

        def do_GET(self):
            if self.admitted("GET") is None:
                return
            path = self.path.split("?", 1)[0]
            if path.startswith("/api/"):
                return self.api("GET", path, None)
            if m := re.fullmatch(r"/img/([0-9a-f]{64})", path):
                return self.image(m[1])
            name = "index.html" if path == "/" else path.lstrip("/")
            file = STATIC / name
            if "/" in name or not file.is_file():
                return self.json(HTTPStatus.NOT_FOUND, {"error": "not found"})
            kind = mimetypes.guess_type(name)[0] or "application/octet-stream"
            self.send(HTTPStatus.OK, file.read_bytes(), kind + ("; charset=utf-8" if kind.startswith("text/") else ""))

        def do_POST(self):
            if (raw := self.admitted("POST")) is None:
                return
            try:
                body = json.loads(raw or b"{}")
            except ValueError:
                return self.json(HTTPStatus.BAD_REQUEST, {"error": "malformed JSON"})
            self.api("POST", self.path, body if isinstance(body, dict) else {})

        def api(self, method: str, path: str, body) -> None:
            tables = ((ROUTES, app.lock), (RANK_ROUTES, bounded(app.lock, RANK_WAIT)), (JOB_ROUTES, nullcontext()))
            for routes, lock in tables:
                for verb, pattern, run in routes:
                    if verb == method and (m := pattern.fullmatch(path)):
                        try:
                            with lock:
                                result = run(app, m, body)
                            return self.json(HTTPStatus.OK, result)
                        except Busy:
                            return self.json(HTTPStatus.SERVICE_UNAVAILABLE, {"error": "busy"})
                        except LookupError:
                            return self.json(HTTPStatus.NOT_FOUND, {"error": "not found"})
                        except (ValueError, TypeError) as err:
                            return self.json(HTTPStatus.BAD_REQUEST, {"error": str(err)})
            self.json(HTTPStatus.NOT_FOUND, {"error": "not found"})

        def image(self, name: str) -> None:
            """A page's image by its hashed name, so a cached image stays with its page whatever its row."""
            record = next((r for r in app.lib.records if r["image_ok"] and Path(r["image_path"]).stem == name), None)
            if record is None:
                return self.json(HTTPStatus.NOT_FOUND, {"error": "no image"})
            self.send(HTTPStatus.OK, Path(record["image_path"]).read_bytes(), "image/jpeg", cache=True)

    return Handler


class Server(ThreadingHTTPServer):
    # Accept a page of picture requests without resets; their transfers never hold up the API.
    request_queue_size = 128


def serve(app: App, port: int, key: bytes) -> HTTPServer:
    """Bound to the loopback interface only; port 0 picks a free one. `key` proves the shell's requests."""
    server = Server(("127.0.0.1", port), None)
    server.RequestHandlerClass = handler(app, server.server_address[1], key)
    return server


def smoke(paths: Paths, app: App) -> dict:
    """Search, suggest and precheck timings over the owner's tag descriptions as queries; numbers only."""
    descriptions_path = paths.data / "zeroshot" / "descriptions.json"
    descriptions = read_json(descriptions_path) if descriptions_path.exists() else {}
    queries = query_variants(app.lib.tags, descriptions)["description"]
    timings = {"search": [], "tag_step": []}
    for i, query in enumerate(queries):
        start = time.perf_counter()
        result = app.search({"query": query, "n": 20, "images": i % 2 == 1})
        timings["search"].append(time.perf_counter() - start)
        rows = [h["row"] for h in result["hits"]]
        start = time.perf_counter()
        engine.prechecks(app.lib, rows, [s["tag"] for s in result["suggested"][:3]])
        timings["tag_step"].append(time.perf_counter() - start)
    return {
        "queries": len(queries),
        **{f"{k}_ms_median": round(1000 * float(np.median(v)), 2) for k, v in timings.items()},
        **{f"{k}_ms_p90": round(1000 * float(np.percentile(v, 90)), 2) for k, v in timings.items()},
    }


def run(
    paths: Paths, root: Path, port: int, smoke_only: bool, secret_file: str | None = None, kmt_bin: str | None = None
) -> None:
    """Serving needs the launch's key before any model loads; `--smoke` serves nothing and needs none. Add link and
    Send run `kmt_bin`, the launch's own knowmoretabs, else the one on PATH."""
    key = None
    if not smoke_only:
        if secret_file is None:
            raise SystemExit(
                "tagger app: pass --secret-file, the key `just up` writes; the app answers only the Library"
            )
        try:
            key = service_auth.read(Path(secret_file))
        except service_auth.UnsafeKey as err:
            raise SystemExit(f"tagger app: {err}") from None
    engine.require_vectors(paths)
    from ..embed import IMAGE_MODEL, MODELS, embed_image_rows, embed_input, load, load_text, release
    from ..zeroshot import encode_queries

    start = time.perf_counter()
    st, model = load_text(MODELS[engine.MODEL], local_files_only=True)

    def encode(texts: list[str]) -> np.ndarray:
        return encode_queries(st, texts)

    def embed(records: list[dict], name: str) -> np.ndarray:
        return embed_input(st, MODELS[engine.MODEL], records, name)[0]

    def embed_image(records):
        image_st, _ = load(IMAGE_MODEL, local_files_only=True)
        try:
            return embed_image_rows(image_st, records)[0]
        finally:
            release(image_st)

    lib, stats = engine.load(paths, root.resolve(), encode, embed, embed_image=embed_image)
    app = App(lib, Store(paths.data / "app"), encode, embed)
    app.binary = kmt_bin or app.binary
    stats |= {"model_load_s": model["load_s"], "startup_s": round(time.perf_counter() - start, 2)}
    print(json.dumps(stats))
    if smoke_only:
        print(json.dumps(smoke(paths, app)))
        return
    server = serve(app, port, key)
    print(f"serving the Library on 127.0.0.1:{server.server_address[1]} (ctrl-c stops)", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
