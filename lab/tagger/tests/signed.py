"""Requests to the app as the Library shell sends them: proven with the test key, and every signed reply checked
against the request it answers. The key is the shared vectors' synthetic one, never a real one."""

import hashlib
import http.client
import json
import os
import time
from pathlib import Path

from tagger.app import server, service_auth

VECTORS = json.loads((Path(__file__).parents[3] / "tests" / "fixtures" / "service_auth.json").read_text())
KEY = bytes.fromhex(VECTORS["key"])


def proof(method: str, path: str, kind: str, body: bytes, key: bytes = KEY, at: int | None = None) -> dict:
    """The three proof headers for one request."""
    nonce = hashlib.sha256(os.urandom(32)).hexdigest()
    at = str(time.time_ns() // 1_000_000 if at is None else at)
    mac = service_auth.request_mac(key, nonce, at, method, path, kind, body)
    return {service_auth.NONCE: nonce, service_auth.TIME: at, service_auth.REQUEST_MAC: mac}


def call(port: int, method: str, path: str, body=None, headers=None, prove: bool = True, timeout: float = 10):
    """(status, JSON or bytes, response). `headers` replace the shell's (None drops one); a reply carrying a MAC
    must be this key's for this request, or the call fails."""
    data = b"" if body is None else json.dumps(body).encode()
    sent = {"Host": f"127.0.0.1:{port}", **({"Content-Type": "application/json"} if body is not None else {})}
    sent |= headers or {}
    if prove:
        sent = proof(method, path, sent.get("Content-Type") or "", data) | sent
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    conn.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
    for name, value in sent.items():
        if value is not None:
            conn.putheader(name, value)
    if data:
        conn.putheader("Content-Length", str(len(data)))
    conn.endheaders(data or None)
    r = conn.getresponse()
    reply = r.read()
    conn.close()
    kind = r.getheader("Content-Type", "")
    mac = r.getheader(service_auth.RESPONSE_MAC)
    if mac is not None:
        archive = r.getheader(server.ARCHIVE_HEADER, "")
        expected = service_auth.response_mac(
            KEY, sent[service_auth.NONCE], method, path, r.status, kind, archive, reply
        )
        assert mac == expected, f"{method} {path}: the reply's MAC is not this request's"
    return r.status, json.loads(reply) if kind.startswith("application/json") else reply, r


def key_file(folder: Path) -> str:
    """A private file in `folder` holding the test key, as `just up` writes one."""
    path = folder / "session.key"
    path.write_text(KEY.hex() + "\n")
    path.chmod(0o600)
    return str(path)
