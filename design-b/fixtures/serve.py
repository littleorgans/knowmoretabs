#!/usr/bin/env python3
"""Stand-in for `knowmoretabs serve`, so the serve host can be exercised.

slice: triage
why:   The same index.html must work with the blob stripped and the data
       fetched. This is the smallest server that proves it: static files,
       GET api/library, POST api/forget and api/restore, state in memory.
       Not the real backend — the Rust server implements the same contract.

    python3 serve.py            # http://127.0.0.1:7878/
"""
import json, sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIB = json.loads((ROOT / "fixtures" / "library.json").read_text(encoding="utf-8"))
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 7878


def by_url():
    return {p["url"]: p for p in LIB["pages"]}


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=str(ROOT), **kw)

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.command, self.path))

    def send_json(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/api/library":
            LIB["stats"]["forgotten"] = sum(1 for p in LIB["pages"] if p.get("forgotten"))
            return self.send_json(LIB)
        if self.path in ("/", "/index.html"):
            # The serve host ships the same file with an empty data element.
            html = (ROOT / "index.html").read_text(encoding="utf-8")
            a = html.index("<!-- library-data:start -->")
            b = html.index("<!-- library-data:end -->")
            html = html[:a] + '<script id="library-data" type="application/json"></script>\n' + html[b:]
            body = html.encode()
            self.send_response(200)
            self.send_header("content-type", "text/html; charset=utf-8")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            return self.wfile.write(body)
        return super().do_GET()

    def do_POST(self):
        if self.path not in ("/api/forget", "/api/restore"):
            return self.send_json({"error": "not found"}, 404)
        n = int(self.headers.get("content-length") or 0)
        urls = json.loads(self.rfile.read(n) or b"{}").get("urls", [])
        pages = by_url()
        done = []
        for u in urls:
            if u in pages:
                pages[u]["forgotten"] = self.path.endswith("forget")
                done.append(u)
        key = "forgotten" if self.path.endswith("forget") else "restored"
        return self.send_json({key: done})


if __name__ == "__main__":
    print(f"serving {ROOT} on http://127.0.0.1:{PORT}/  (Ctrl-C to stop)")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
