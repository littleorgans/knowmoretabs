"""The proof the Library shell and this service exchange on every request, and the key file it rests on.

`just up` writes one random key to `<archive>/logs/session.key` (`ensure`, this file run as a script before either
app starts) and both apps read it at start (`read`). A request carries a fresh nonce, the time and an HMAC of its
framed transcript; the reply to a proven request carries an HMAC binding that nonce, the request, the status, the
type, the archive's identity and the body. A process squatting on the service's port sees a request but cannot
answer it. `src/service_auth.rs` is the shell's side; `tests/fixtures/service_auth.json` holds both to the same
bytes.

Stdlib only: the launcher runs this file with the system Python.
"""

import hashlib
import hmac
import os
import re
import secrets
import stat
import sys
from pathlib import Path

# Header names, as src/service_auth.rs writes them
NONCE = "X-KMT-Nonce"
TIME = "X-KMT-Time"
REQUEST_MAC = "X-KMT-Request-MAC"
RESPONSE_MAC = "X-KMT-Response-MAC"
REQUEST_VERSION = "kmt-s2-request-v1"
RESPONSE_VERSION = "kmt-s2-response-v1"
# A request's time may differ from this clock by at most this many milliseconds
WINDOW_MS = 30_000
HEX64 = re.compile(r"[0-9a-f]{64}")
MILLIS = re.compile(r"[0-9]{1,15}")


class UnsafeKey(ValueError):
    """The key file is missing, malformed, a link, someone else's, or readable by others."""


def read(path: Path) -> bytes:
    """The key: exactly 64 lowercase hex characters and a newline, in a regular file of this user's that only this
    user can read. The file is opened without following a link, then judged by what was opened."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as err:
        raise UnsafeKey(f"cannot open the key file: {err.strerror}") from None
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid() or st.st_mode & 0o077:
            raise UnsafeKey("the key file must be a regular file of yours, mode 0600")
        data = os.read(fd, 66)
    finally:
        os.close(fd)
    if len(data) != 65 or not data.endswith(b"\n") or not HEX64.fullmatch(data[:64].decode("latin-1")):
        raise UnsafeKey("the key file must hold 64 lowercase hex characters and a newline")
    return bytes.fromhex(data[:64].decode())


def ensure(path: Path) -> None:
    """The key at `path`: kept when there is one (and refused when it is unsafe), else made once, staged beside it
    and linked into place so a key another launch made first is never replaced."""
    folder = path.parent
    folder.mkdir(mode=0o700, parents=True, exist_ok=True)
    st = os.lstat(folder)
    if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.getuid() or st.st_mode & 0o022:
        raise UnsafeKey("the key's folder must be a folder of yours that no one else can write")
    if os.path.lexists(path):
        read(path)
        return
    stage = folder / f".{path.name}.{secrets.token_hex(8)}"
    fd = os.open(stage, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        try:
            os.write(fd, (secrets.token_bytes(32).hex() + "\n").encode())
            os.fsync(fd)
        finally:
            os.close(fd)
        try:
            os.link(stage, path)
        except FileExistsError:
            pass
    finally:
        os.unlink(stage)
    read(path)
    dir_fd = os.open(folder, os.O_RDONLY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def framed(*fields: str) -> bytes:
    """Each field as its UTF-8 length, unsigned 64 bit big endian, then its bytes: no field can pass for two."""
    out = bytearray()
    for field in fields:
        data = field.encode()
        out += len(data).to_bytes(8, "big") + data
    return bytes(out)


def _mac(key: bytes, *fields: str) -> str:
    return hmac.new(key, framed(*fields), hashlib.sha256).hexdigest()


def request_mac(key: bytes, nonce: str, at: str, method: str, path: str, kind: str, body: bytes) -> str:
    return _mac(key, REQUEST_VERSION, nonce, at, method, path, kind, str(len(body)), hashlib.sha256(body).hexdigest())


def response_mac(
    key: bytes, nonce: str, method: str, path: str, status: int, kind: str, archive: str, body: bytes
) -> str:
    digest = hashlib.sha256(body).hexdigest()
    return _mac(key, RESPONSE_VERSION, nonce, method, path, str(status), kind, archive, str(len(body)), digest)


def proven(key: bytes, headers, method: str, path: str, body: bytes, now_ms: int) -> str | None:
    """The request's nonce when its proof holds: each proof field given once and well formed, its time within
    `WINDOW_MS` of `now_ms`, and its MAC this key's over exactly this method, path, type and body; else None."""
    given = [headers.get_all(name) or [] for name in (NONCE, TIME, REQUEST_MAC, "Content-Type")]
    if any(len(values) != 1 for values in given[:3]) or len(given[3]) > 1:
        return None
    nonce, at, mac = given[0][0], given[1][0], given[2][0]
    if not (HEX64.fullmatch(nonce) and HEX64.fullmatch(mac) and MILLIS.fullmatch(at)):
        return None
    if abs(now_ms - int(at)) > WINDOW_MS:
        return None
    kind = given[3][0] if given[3] else ""
    return nonce if hmac.compare_digest(request_mac(key, nonce, at, method, path, kind, body), mac) else None


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] != "ensure":
        sys.exit("usage: service_auth.py ensure <key file>")
    try:
        ensure(Path(sys.argv[2]))
    except (UnsafeKey, OSError) as err:
        sys.exit(f"session key: {err}; run `just down`, then `just up`")
