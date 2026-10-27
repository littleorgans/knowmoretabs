"""The real Library shell (the knowmoretabs build `KMT_BIN` names) in front of the app, as `just up` runs the pair:
one archive, the launch key in a private file, spare ports. A request goes to the shell as a browser sends it."""

import http.client
import json
import os
import subprocess
import threading
from pathlib import Path

import signed

from tagger.app import server


def binary() -> str | None:
    return os.environ.get("KMT_BIN") or None


class Shell:
    def __init__(self, app: server.App, folder: Path, kmt: str):
        self.app, self.folder = app, folder
        self.service = self.serve(signed.KEY)
        self.service_port = self.service.server_address[1]
        args = ["--root", str(app.lib.root), "serve", "--port", "0", "--model-port", str(self.service_port)]
        with open(folder / "shell.stderr", "w") as stderr:
            self.process = subprocess.Popen(
                [kmt, *args, "--model-secret-file", signed.key_file(folder)],
                stdout=subprocess.PIPE,
                stderr=stderr,
                text=True,
            )
        banner = self.process.stdout.readline().strip()
        self.url = banner.removeprefix("Your library is at ")
        assert self.url.startswith("http://127.0.0.1:"), f"no banner: {banner!r}"
        self.port = int(self.url.rstrip("/").rsplit(":", 1)[1])

    def serve(self, key: bytes, port: int = 0):
        httpd = server.serve(self.app, port, key)
        threading.Thread(target=httpd.serve_forever, daemon=True).start()
        return httpd

    def stop_service(self) -> None:
        self.service.shutdown()
        self.service.server_close()

    def close(self) -> None:
        self.process.kill()
        self.process.wait()
        self.process.stdout.close()
        self.stop_service()
        self.app.close()

    def call(self, method: str, path: str, body=None, headers=None):
        """(status, headers by lower-case name, body bytes), as a browser on the Library's page asks."""
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=20)
        data = None if body is None else json.dumps(body).encode()
        sent = {"Host": f"127.0.0.1:{self.port}", **({"Content-Type": "application/json"} if data else {})}
        conn.request(method, path, data, sent | (headers or {}))
        r = conn.getresponse()
        reply = r.read()
        conn.close()
        return r.status, {k.lower(): v for k, v in r.getheaders()}, reply
