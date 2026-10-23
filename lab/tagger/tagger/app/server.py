"""`tagger app`: search, select and tag on one screen, as a local web app on 127.0.0.1.

Static files come from `static/`; page images are served by their hashed name from the resolved archive; the
JSON API runs the engine on the archive's decisions, followed from `library.json` before every request that shows
them; only knowmoretabs writes that file (the screens through the Library shell's own API, Add link through its
CLI). API work is serialized; static and image transfers run independently.
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
from dataclasses import replace
import re
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer, ThreadingHTTPServer
from pathlib import Path
from threading import Lock, Thread

import numpy as np

from .. import chunks
from ..archive import STATE_FILE, LibraryState, Unreadable, signature
from ..paths import Paths, read_json
from ..zeroshot import prototype, query_variants
from . import add, engine, kmt, service_auth
from .store import Exclusions, Keys

STATIC = Path(__file__).with_name("static")
MAX_RESULTS = 50  # web/app.js RANK_TOP shows the same number of ranked pages
# Every reply names the archive this service loaded; src/model_service.rs reads the same header
ARCHIVE_HEADER = "X-KMT-Archive"
# Seconds the Library's rank waits for the model; an Add link index holds it for longer, and the Library then
# searches by keyword instead of queueing behind it
RANK_WAIT = 0.5
# The largest request body read; the shell forwards nothing larger (src/http.rs BODY_LIMIT)
BODY_LIMIT = 4 * 1024 * 1024
RANK_REPLY_LIMIT = 8 * 1024 * 1024  # the signed client's existing response bound
SNIPPET_CHARS = 200
ADD_K_SD = 0.7  # training fitted margin for about 1.2 Add suggestions per page with three visible
CSP = (
    "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; connect-src 'self'; "
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
)


class App:
    def __init__(self, lib: engine.Library, folder: Path, encode: engine.Encode, embed: engine.Embed):
        """`folder`: the app's own lists, `<data>/app`."""
        self.lib, self.encode, self.embed = lib, encode, embed
        self.lock = Lock()  # the model and mutable app state have one caller at a time
        self._refit_thread: Thread | None = None
        self._refit_pending = False
        self.binary = kmt.BINARY
        self.adds = add.Jobs(self)
        self.exclusions = Exclusions(folder)
        self.selection = Keys(folder, "selection.json")
        self.pins = Keys(folder, "pinned.json")
        with self.lock:
            self.follow()  # the file may have changed while the heads were fitted

    def follow(self) -> None:
        """The library in line with `library.json` as it is now: read again, and applied, only once its signature
        moved from the one the library follows. A damaged file raises `Unreadable`, leaving the library as it was,
        and is read again on the next call. Hold the lock."""
        if signature(self.lib.root / STATE_FILE) != self.lib.state.signature:
            previous = self.lib
            self.lib = engine.apply_state(self.lib, LibraryState.read(self.lib.root), self.encode)
            if self.lib.tags != previous.tags or not np.array_equal(self.lib.Y, previous.Y):
                self._queue_refit()

    def _queue_refit(self) -> None:
        """Hold the app lock: one worker, with at most one follow up on the latest library."""
        self._refit_pending = True
        if self._refit_thread is None:
            self._refit_thread = Thread(target=self._refit, daemon=True)
            self._refit_thread.start()

    def _refit(self) -> None:
        while True:
            with self.lock:
                lib = self.lib
                self._refit_pending = False
            try:
                fitted = engine.fit(lib.X, lib.Y, np.vstack([lib.rules.q, lib.added_q]))
            except Exception as err:
                print(json.dumps({"refit_failed": type(err).__name__}), flush=True)
                fitted = None
            with self.lock:
                if self._refit_pending:
                    continue  # a newer label or vector snapshot superseded this fit
                if fitted is not None:
                    self.lib = replace(self.lib, **fitted, added_q=lib.added_q[:0])
                self._refit_thread = None
                return

    def gone(self) -> list[int]:
        """Rows of the forgotten pages."""
        return np.flatnonzero(~self.lib.live).tolist()

    def live(self) -> np.ndarray:
        """Rows the screen shows: the pages the library has not forgotten."""
        return self.lib.live.copy()

    def sync(self, url: str, archive) -> dict | None:
        """After knowmoretabs changed one page: the library in line with the archive for it, and the page as the
        screen shows it with the tags `suggested` for it, or None when the screen does not show it. Hold the lock."""
        previous = self.lib
        self.lib = engine.sync(self.lib, archive, [url], self.embed)
        self.follow()
        if self._refit_thread is not None or not np.array_equal(self.lib.Y, previous.Y):
            self._queue_refit()  # a running fit must not publish scores for old page vectors
        row = self.lib.rows.get(url)
        if row is None or not self.lib.live[row]:
            return None
        page = self.page(row)
        return {**page, "suggested": self.suggested(row, page["tags"])}

    def suggested(self, row: int, held: list[str]) -> list[str]:
        """The active tags the page neither holds nor has turned down that the model would put on it, best first:
        each whose score clears its threshold plus `ADD_K_SD` score SD (`engine.prechecks`: a head's, or zero shot
        for a tag with too few positives or awaiting its refit), by confidence; at most `engine.SUGGEST`."""
        dismissed = self.lib.state.dismissed(self.lib.records[row]["key"])
        tags = [t for t in self.names() if t not in held and t.lower() not in dismissed]
        checked = [s for s in engine.prechecks(self.lib, [row], tags, margin_sd=ADD_K_SD)[0] if s["checked"]]
        return [s["tag"] for s in sorted(checked, key=lambda s: -s["p"])[: engine.SUGGEST]]

    def page(self, row: int) -> dict:
        """A page as the views show it, with its tags as the library holds them."""
        r = self.lib.records[row]
        return {
            "row": row,
            "title": r["title"],
            "host": r["host"],
            "url": r["key"],
            "image": Path(r["image_path"]).stem if r["image_ok"] else None,  # its address: `/img/<hashed name>`
            "tags": r["tags"],
        }

    def names(self) -> list[str]:
        """Every name the vocabulary holds active, by name."""
        return sorted(self.lib.state.active.values(), key=str.lower)

    def tag_lists(self) -> dict:
        """`app_tags`: every active name, by name. `recent`: those an add has put on a page, latest first (`used_at`:
        removing a tag or undoing an add leaves it as it was)."""
        names, used = self.names(), self.lib.state.used
        recent = [t for t in names if t.lower() in used]
        return {"app_tags": names, "recent": sorted(recent, key=lambda t: -used[t.lower()])}

    def library(self) -> dict:
        return {
            "pages": int(self.lib.live.sum()),
            "forgotten": self.gone(),
            "tags": engine.tag_info(self.lib),
            **self.tag_lists(),
            "images": self.lib.image is not None,
            "input": self.lib.input_name,
            "sizes": [20, MAX_RESULTS],
        }

    def untagged(self, tag: str | None = None) -> np.ndarray:
        """Shown pages without any tag, or without the given tag."""
        held = self.lib.Y.any(axis=1) if tag is None else self.lib.Y[:, self.lib.tags.index(tag)].astype(bool)
        return self.live() & ~held

    def search(self, body: dict) -> dict:
        """One page of ranked results for `query`, from rank `offset`; with `untagged`, among pages without a tag
        only. `total` counts the pages ranked. An empty query lists every page, newest first."""
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
        """Legacy all-page keys, or schema 1 details among the client's live page/body identities."""
        query = str(body.get("query", ""))
        if not query.strip():
            raise ValueError("type a search")
        if "schema_version" not in body:
            q = self.encode([query])[0]
            rows = engine.order(self.lib, q, query, np.ones(len(self.lib.records), bool))
            return {"keys": [self.lib.records[i]["key"] for i in rows]}
        if type(body["schema_version"]) is not int or body["schema_version"] != 1:
            raise ValueError("unsupported rank schema_version")
        candidates, chars = body.get("among"), body.get("snippet_chars", SNIPPET_CHARS)
        if not isinstance(candidates, list) or type(chars) is not int or not 1 <= chars <= SNIPPET_CHARS:
            raise ValueError("rank requires among and snippet_chars between 1 and 200")
        identities = {}
        for candidate in candidates:
            if not isinstance(candidate, dict) or not isinstance(candidate.get("key"), str):
                raise ValueError("invalid rank candidate")
            key, digest = candidate["key"], candidate.get("content_sha256")
            if "content_sha256" not in candidate or (
                digest is not None and (not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest))
            ):
                raise ValueError("invalid rank body identity")
            if key in identities:
                raise ValueError("duplicate rank candidate")
            identities[key] = digest
        among = self.live() & np.array([r["key"] in identities for r in self.lib.records], bool)
        chunk_among = among.copy()
        stale = set()
        if self.lib.chunks is not None:
            for i in np.flatnonzero(among):
                key = self.lib.records[i]["key"]
                frozen = self.lib.chunks.bodies.get(key)
                if frozen is not None and identities[key] != frozen.content_sha256:
                    chunk_among[i] = False
                    stale.add(int(i))
        if not among.any():
            return {"schema_version": 1, "keys": [], "hits": [], "fallback": []}
        q = self.encode([query])[0]
        ranked = engine.search(self.lib, q, query, int(among.sum()), among=among, chunk_among=chunk_among)
        hits, fallback_counts = [], {}
        for hit in ranked:
            i, ref = hit["row"], hit["chunk"]
            key = self.lib.records[i]["key"]
            status = "ready" if ref else "stale" if i in stale else self.lib.chunk_status.removeprefix("chunks_")
            if status == "ready" and ref is None:
                status = "missing"
            snippet, span, digest = None, None, None
            if ref is not None:
                frozen = self.lib.chunks.bodies[key]
                snippet = frozen.raw[ref["start"] : ref["end"]].decode("utf-8")[:chars]
                span = {"start": ref["start"], "end": ref["start"] + len(snippet.encode("utf-8"))}
                digest = frozen.content_sha256
            else:
                code = "chunks_" + status
                fallback_counts[code] = fallback_counts.get(code, 0) + 1
            hits.append(
                {
                    "key": key,
                    "score": hit["fused"],
                    "sources": hit["sources"],
                    "semantic_source": hit["semantic_source"],
                    "chunk_status": status,
                    "section": ref["section"] if ref else None,
                    "chunk": {k: ref[k] for k in ("id", "start", "end", "hash", "page_hash", "score")} if ref else None,
                    "snippet": snippet,
                    "snippet_range": span,
                    "content_sha256": digest,
                }
            )
        result = {
            "schema_version": 1,
            "keys": [h["key"] for h in hits],
            "hits": hits,
            "fallback": [
                {"stage": "chunks", "code": code, "affected_pages": n} for code, n in sorted(fallback_counts.items())
            ],
        }
        if len(json.dumps(result, ensure_ascii=False).encode()) > RANK_REPLY_LIMIT:
            return {
                "schema_version": 1,
                "keys": [],
                "hits": [],
                "fallback": [{"stage": "rank", "code": "rank_response_too_large"}],
            }
        return result

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

    def like(self, body: dict) -> dict:
        """The tag's name as a query moved toward the pages holding it and away from those it was taken off or turned
        down on (refine's prototype), ranked among pages without that tag: one page from rank `offset`, and `total`."""
        tag = str(body.get("tag", ""))
        if tag not in self.names():
            raise ValueError("name one of your tags")
        held = np.flatnonzero(self.lib.Y[:, self.lib.tags.index(tag)])
        off = [
            self.lib.rows[k]
            for k in self.lib.state.removes
            if k in self.lib.rows and tag.lower() in self.lib.state.dismissed(k)
        ]
        q = prototype(self.encode([tag])[0], self.lib.X[held], self.lib.X[off])
        among, offset = self.untagged(tag), _offset(body)
        hits = engine.search(self.lib, q, tag, _size(body), bool(body.get("images")), among, offset)
        return {"tag": tag, "hits": [self.page(h["row"]) for h in hits], "offset": offset, "total": int(among.sum())}

    def listed(self, keys: Keys) -> dict:
        """The selected or pinned pages this library holds, in the order added; a forgotten one stays listed for the
        screen to leave out, so restoring it puts it back in its place."""
        return {"pages": [self.page(self.lib.rows[k]) for k in keys.keys if k in self.lib.rows]}

    def replace(self, keys: Keys, body: dict) -> dict:
        """Replace the selection or the pins with the given pages (none clears them), forgotten ones included."""
        rows = self.rows(body, np.ones(len(self.lib.records), bool))
        keys.set([self.lib.records[r]["key"] for r in rows])
        return {"rows": rows}


# Each takes the app's lock, and follows `library.json` before it runs
ROUTES = [
    ("GET", re.compile(r"/api/library"), lambda app, m, b: app.library()),
    ("POST", re.compile(r"/api/search"), lambda app, m, b: app.search(b)),
    ("POST", re.compile(r"/api/exclusions"), lambda app, m, b: app.exclude(b)),
    ("GET", re.compile(r"/api/selection"), lambda app, m, b: app.listed(app.selection)),
    ("POST", re.compile(r"/api/selection"), lambda app, m, b: app.replace(app.selection, b)),
    ("GET", re.compile(r"/api/pins"), lambda app, m, b: app.listed(app.pins)),
    ("POST", re.compile(r"/api/pins"), lambda app, m, b: app.replace(app.pins, b)),
    ("POST", re.compile(r"/api/like"), lambda app, m, b: app.like(b)),
]


# The Library's rank holds the app's lock like the routes above, but waits for it at most RANK_WAIT
RANK_ROUTES = [("POST", re.compile(r"/api/rank"), lambda app, m, b: app.rank(b))]


# Add link jobs run knowmoretabs; they take the app's lock only around its state, so the screen never waits on a run
JOB_ROUTES = [
    ("POST", re.compile(r"/api/add"), lambda app, m, b: app.adds.start(b)),
    ("GET", re.compile(r"/api/add/(\d+)"), lambda app, m, b: app.adds.view(int(m[1]))),
]


def _size(body: dict) -> int:
    return min(max(int(body.get("n", 20)), 1), MAX_RESULTS)


def _offset(body: dict) -> int:
    return max(int(body.get("offset", 0)), 0)


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
            tables = (
                (ROUTES, app.lock, True),
                (RANK_ROUTES, bounded(app.lock, RANK_WAIT), True),
                (JOB_ROUTES, nullcontext(), False),
            )
            for routes, lock, follows in tables:
                for verb, pattern, run in routes:
                    if verb == method and (m := pattern.fullmatch(path)):
                        try:
                            with lock:
                                if follows:
                                    app.follow()
                                result = run(app, m, body)
                            return self.json(HTTPStatus.OK, result)
                        except Busy:
                            return self.json(HTTPStatus.SERVICE_UNAVAILABLE, {"error": "busy"})
                        except Unreadable as err:  # nothing shown as fresh from a file that cannot be read
                            return self.json(
                                HTTPStatus.SERVICE_UNAVAILABLE, {"error": f"the library cannot be read: {err}"}
                            )
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
    """Serving needs the launch's key before any model loads; `--smoke` serves nothing and needs none. Add link, and
    the text of a local document, run `kmt_bin`, the launch's own knowmoretabs, else the one on PATH."""
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

    binary = kmt_bin or kmt.BINARY
    lib, stats = engine.load(
        paths,
        root.resolve(),
        encode,
        embed,
        embed_image=embed_image,
        embed_chunks=lambda docs: chunks.encode_docs(st, docs),
        binary=binary,
    )
    app = App(lib, paths.data / "app", encode, embed)
    app.binary = binary
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
