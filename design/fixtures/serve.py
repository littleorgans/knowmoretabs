#!/usr/bin/env python3
"""A stand-in for `knowmoretabs serve`, so the live half of the UI is testable.

slice: triage
why: The mode seam is only believable if both sides of it actually run. This is
     the smallest server that behaves the way the Rust one will: it serves the
     same index.html with the embedded blob emptied out, and answers the three
     JSON endpoints the frontend calls. It is a fixture, not a product.

    python3 fixtures/serve.py [--port 8777]
"""

import json
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIBRARY = ROOT / "fixtures" / "library.json"
STATE = ROOT / "fixtures" / "forgotten.json"     # stands in for ~/.knowmoretabs/library.json

TYPES = {".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8",
         ".js": "text/javascript; charset=utf-8", ".json": "application/json"}


def forgotten():
    if STATE.exists():
        return set(json.loads(STATE.read_text()))
    return set()


def document():
    doc = json.loads(LIBRARY.read_text(encoding="utf-8"))
    gone = forgotten()
    for page in doc["pages"]:
        page.pop("forgotten", None)
        if page["url"] in gone:
            page["forgotten"] = True
    doc["counts"]["forgotten"] = sum(1 for p in doc["pages"] if p.get("forgotten"))
    return doc


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def send(self, status, body, ctype="application/json"):
        raw = body if isinstance(body, bytes) else body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Referrer-Policy", "no-referrer")
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/api/library":
            return self.send(200, json.dumps(document()))
        name = "index.html" if path == "/" else path.lstrip("/")
        target = (ROOT / name).resolve()
        if ROOT not in target.parents or not target.is_file():
            return self.send(404, '{"error":"not found"}')
        if target.name == "index.html":
            # `serve` never embeds the blob: that is the whole mode switch.
            html = target.read_text(encoding="utf-8")
            html = re.sub(r'(<script id="library" type="application/json">).*?(</script>)',
                          r"\1null\2", html, count=1, flags=re.S)
            return self.send(200, html, TYPES[".html"])
        return self.send(200, target.read_bytes(),
                         TYPES.get(target.suffix, "application/octet-stream"))

    def do_POST(self):
        path = self.path.split("?")[0]
        if path not in ("/api/forget", "/api/restore"):
            return self.send(404, '{"error":"not found"}')
        size = int(self.headers.get("Content-Length", 0))
        urls = json.loads(self.rfile.read(size) or b"{}").get("urls", [])
        gone = forgotten()
        gone.update(urls) if path.endswith("forget") else gone.difference_update(urls)
        STATE.write_text(json.dumps(sorted(gone), indent=1))
        doc = document()
        return self.send(200, json.dumps({"urls": urls, "counts": doc["counts"]}))

    def log_message(self, fmt, *args):
        sys.stderr.write("  %s\n" % (fmt % args))


def main(argv):
    port = int(argv[argv.index("--port") + 1]) if "--port" in argv else 8777
    print("serve mode on http://127.0.0.1:%d/  (ctrl-c to stop)" % port)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main(sys.argv[1:])
