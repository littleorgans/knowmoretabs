"""`tagger app`: the search then tag prototype as a local web app on 127.0.0.1.

Static files come from `static/`; page images are served by row from the archive given with `--root`; the
JSON API runs the engine and the store. One request at a time (the model is not shared across threads).
Requests must name this server as Host (no DNS rebinding) and POSTs must be same origin JSON. The process
prints counts and timings only; the request log is off, since nothing about a page belongs in a terminal.
`--smoke` loads everything, times searches with the owner's tag descriptions as queries, prints numbers, exits.
"""

import json
import mimetypes
import re
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import numpy as np

from ..paths import Paths, read_json
from ..zeroshot import query_variants
from . import engine
from .store import Store

STATIC = Path(__file__).with_name("static")
MAX_RESULTS = 50
CSP = (
    "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; connect-src 'self'; "
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
)


class App:
    def __init__(self, lib: engine.Library, store: Store, encode: engine.Encode):
        self.lib, self.store, self.encode = lib, store, encode

    def own(self, row: int) -> list[str]:
        return [t for j, t in enumerate(self.lib.tags) if self.lib.Y[row, j]]

    def page(self, row: int) -> dict:
        r = self.lib.records[row]
        return {
            "row": row,
            "title": r["title"],
            "host": r["host"],
            "url": r["key"],
            "image": r["image_ok"],
            "own": self.own(row),
        }

    def library(self) -> dict:
        return {
            "pages": int(self.lib.live.sum()),
            "tags": engine.tag_info(self.lib),
            "images": self.lib.image is not None,
            "input": self.lib.input_name,
            "sizes": [20, MAX_RESULTS],
        }

    def search(self, body: dict) -> dict:
        query = str(body.get("query", "")).strip()
        if not query:
            raise ValueError("type a search")
        n = min(max(int(body.get("n", 20)), 1), MAX_RESULTS)
        start = time.perf_counter()
        q = self.encode([query])[0]
        encoded = time.perf_counter()
        hits = engine.search(self.lib, q, query, n, bool(body.get("images")))
        suggested = engine.suggest(self.lib, [h["row"] for h in hits])
        done = time.perf_counter()
        return {
            "hits": [{**self.page(h["row"]), "fused": h["fused"], "sources": h["sources"]} for h in hits],
            "suggested": suggested,
            "ms": {"encode": round(1000 * (encoded - start), 1), "rank": round(1000 * (done - encoded), 1)},
        }

    def create(self, body: dict) -> dict:
        rows = [int(r) for r in body.get("rows", [])]
        picked = [str(t) for t in body.get("picked", [])]
        if not rows or not picked:
            raise ValueError("pick at least one tag for at least one page")
        if any(r < 0 or r >= len(self.lib.records) or not self.lib.live[r] for r in rows):
            raise ValueError("a row is not a page of this library")
        if unknown := set(picked) - set(self.lib.tags):
            raise ValueError(f"{len(unknown)} picked tag(s) are not in the vocabulary")
        keys = [self.lib.records[r]["key"] for r in rows]
        session = self.store.create(
            str(body.get("query", "")), bool(body.get("images")), keys, picked, engine.prechecks(self.lib, rows, picked)
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
    ("POST", re.compile(r"/api/sessions"), lambda app, m, b: app.create(b)),
    ("POST", re.compile(r"/api/sessions/(\d+)/pages/(\d+)"), lambda app, m, b: app.update(int(m[1]), int(m[2]), b)),
    ("POST", re.compile(r"/api/export"), lambda app, m, b: app.store.export()),
]


def _found(value, view):
    if value is None:
        raise LookupError
    return view(value)


def handler(app: App, port: int):
    hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}

    class Handler(BaseHTTPRequestHandler):
        server_version = "kmt-tagger"
        sys_version = ""

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
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, value) -> None:
            self.send(status, json.dumps(value, ensure_ascii=False).encode(), "application/json; charset=utf-8")

        def allowed(self) -> bool:
            if self.headers.get("Host") not in hosts:
                self.json(HTTPStatus.MISDIRECTED_REQUEST, {"error": "unknown host"})
                return False
            return True

        def do_GET(self):
            if not self.allowed():
                return
            path = self.path.split("?", 1)[0]
            if path.startswith("/api/"):
                return self.api("GET", path, None)
            if m := re.fullmatch(r"/img/(\d+)", path):
                return self.image(int(m[1]))
            name = "index.html" if path == "/" else path.lstrip("/")
            file = STATIC / name
            if "/" in name or not file.is_file():
                return self.json(HTTPStatus.NOT_FOUND, {"error": "not found"})
            kind = mimetypes.guess_type(name)[0] or "application/octet-stream"
            self.send(HTTPStatus.OK, file.read_bytes(), kind + ("; charset=utf-8" if kind.startswith("text/") else ""))

        def do_POST(self):
            if not self.allowed():
                return
            origin = self.headers.get("Origin")
            if origin is not None and origin.removeprefix("http://") not in hosts:
                return self.json(HTTPStatus.FORBIDDEN, {"error": "cross origin"})
            if self.headers.get("Content-Type", "").split(";")[0] != "application/json":
                return self.json(HTTPStatus.UNSUPPORTED_MEDIA_TYPE, {"error": "send JSON"})
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
            except ValueError:
                return self.json(HTTPStatus.BAD_REQUEST, {"error": "malformed JSON"})
            self.api("POST", self.path, body if isinstance(body, dict) else {})

        def api(self, method: str, path: str, body) -> None:
            for verb, pattern, run in ROUTES:
                if verb == method and (m := pattern.fullmatch(path)):
                    try:
                        return self.json(HTTPStatus.OK, run(app, m, body))
                    except LookupError:
                        return self.json(HTTPStatus.NOT_FOUND, {"error": "not found"})
                    except (ValueError, TypeError) as err:
                        return self.json(HTTPStatus.BAD_REQUEST, {"error": str(err)})
            self.json(HTTPStatus.NOT_FOUND, {"error": "not found"})

        def image(self, row: int) -> None:
            if row >= len(app.lib.records) or not app.lib.records[row]["image_ok"]:
                return self.json(HTTPStatus.NOT_FOUND, {"error": "no image"})
            self.send(HTTPStatus.OK, Path(app.lib.records[row]["image_path"]).read_bytes(), "image/jpeg", cache=True)

    return Handler


def serve(app: App, port: int) -> HTTPServer:
    """Bound to the loopback interface only; port 0 picks a free one."""
    server = HTTPServer(("127.0.0.1", port), None)
    server.RequestHandlerClass = handler(app, server.server_address[1])
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


def run(paths: Paths, root: Path | None, port: int, smoke_only: bool) -> None:
    from ..embed import MODELS, load_text
    from ..zeroshot import encode_queries

    start = time.perf_counter()
    st, model = load_text(MODELS[engine.MODEL])

    def encode(texts: list[str]) -> np.ndarray:
        return encode_queries(st, texts)

    lib, stats = engine.load(paths, (root or paths.snapshot).resolve(), encode)
    app = App(lib, Store(paths.data / "app"), encode)
    stats |= {"model_load_s": model["load_s"], "startup_s": round(time.perf_counter() - start, 2)}
    print(json.dumps(stats))
    if smoke_only:
        print(json.dumps(smoke(paths, app)))
        return
    server = serve(app, port)
    print(f"serving http://127.0.0.1:{server.server_address[1]}/ (ctrl-c stops)", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
