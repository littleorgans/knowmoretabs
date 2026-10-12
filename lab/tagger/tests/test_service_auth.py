"""The shell's proof on the service's side: the shared vectors, the key file, and the app refusing every request
that is not proven before anything routes, signing every reply to one that is."""

import http.client
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import signed
from test_app import Served

from tagger.app import server, service_auth
from tagger.paths import Paths


class VectorTests(unittest.TestCase):
    def test_requests_and_replies_mac_to_the_shared_vectors(self):
        v = signed.VECTORS
        for r in v["requests"]:
            mac = service_auth.request_mac(
                signed.KEY, r["nonce"], r["time"], r["method"], r["path"], r["content_type"], r["body"].encode()
            )
            self.assertEqual(r["mac"], mac)
        for r in v["responses"]:
            args = (r["nonce"], r["method"], r["path"], r["status"], r["content_type"], r["archive"])
            self.assertEqual(r["mac"], service_auth.response_mac(signed.KEY, *args, r["body"].encode()))
            self.assertNotEqual(r["mac"], service_auth.response_mac(signed.KEY, *args, b"{}"))
        self.assertEqual(v["framed"]["hex"], service_auth.framed(*v["framed"]["fields"]).hex())


class KeyFileTests(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        self.dir.chmod(0o700)

    def write(self, name: str, text: str, mode: int = 0o600) -> Path:
        path = self.dir / name
        path.write_text(text)
        path.chmod(mode)
        return path

    def test_only_a_private_well_formed_key_is_read(self):
        good = "ab" * 32 + "\n"
        self.assertEqual(bytes.fromhex("ab" * 32), service_auth.read(self.write("good", good)))
        self.assertEqual(bytes.fromhex("ab" * 32), service_auth.read(self.write("owner-only", good, 0o400)))
        bad = {
            "shared": (good, 0o640),
            "short": ("ab" * 31 + "\n", 0o600),
            "upper": ("AB" * 32 + "\n", 0o600),
            "bare": ("ab" * 32, 0o600),
            "long": (good + "\n", 0o600),
        }
        for name, (text, mode) in bad.items():
            with self.subTest(name), self.assertRaises(service_auth.UnsafeKey):
                service_auth.read(self.write(name, text, mode))
        (self.dir / "link").symlink_to(self.dir / "good")
        for name in ("link", "missing"):
            with self.subTest(name), self.assertRaises(service_auth.UnsafeKey):
                service_auth.read(self.dir / name)

    def test_ensure_makes_one_private_key_and_then_keeps_it(self):
        path = self.dir / "logs" / "session.key"
        service_auth.ensure(path)
        made = path.read_bytes()
        self.assertEqual(0o600, stat.S_IMODE(path.stat().st_mode))
        self.assertEqual(0o700, stat.S_IMODE(path.parent.stat().st_mode))
        self.assertEqual(32, len(service_auth.read(path)))
        service_auth.ensure(path)
        self.assertEqual(made, path.read_bytes(), "a key that is there is kept")
        self.assertEqual(["session.key"], os.listdir(path.parent), "no stage is left behind")
        other = self.dir / "other" / "session.key"
        service_auth.ensure(other)
        self.assertNotEqual(made, other.read_bytes(), "each new key is random")

    def test_ensure_refuses_an_unsafe_key_or_folder_and_changes_nothing(self):
        logs = self.dir / "logs"
        logs.mkdir(mode=0o700)
        key = self.write("logs/session.key", "ab" * 32 + "\n", 0o644)
        with self.assertRaises(service_auth.UnsafeKey):
            service_auth.ensure(key)
        self.assertEqual("ab" * 32 + "\n", key.read_text())
        key.unlink()
        (logs / "session.key").symlink_to(self.dir / "elsewhere")
        with self.assertRaises(service_auth.UnsafeKey):
            service_auth.ensure(key)
        self.assertFalse((self.dir / "elsewhere").exists(), "a link is never written through")
        open_folder = self.dir / "open"
        open_folder.mkdir()
        open_folder.chmod(0o777)
        with self.assertRaises(service_auth.UnsafeKey):
            service_auth.ensure(open_folder / "session.key")
        self.assertEqual([], os.listdir(open_folder))

    def test_the_launcher_runs_it_as_a_script_with_the_system_python(self):
        script = Path(service_auth.__file__)
        path = self.dir / "logs" / "session.key"
        made = subprocess.run([sys.executable, "-I", str(script), "ensure", str(path)], capture_output=True)
        self.assertEqual((0, b"", b""), (made.returncode, made.stdout, made.stderr))
        self.assertEqual(32, len(service_auth.read(path)))
        path.chmod(0o644)
        refused = subprocess.run([sys.executable, "-I", str(script), "ensure", str(path)], capture_output=True)
        self.assertEqual(1, refused.returncode)
        self.assertIn(b"just down", refused.stderr)
        self.assertNotIn(path.read_bytes().strip(), refused.stderr, "the key is never printed")


class ProofTests(Served):
    """Every refusal is made before a route runs and is unsigned; every reply to a proven request is signed."""

    def setUp(self):
        super().setUp()
        self.routed = []
        api = self.server.RequestHandlerClass.api

        def recorded(handler, method, path, body):
            self.routed.append(path)
            return api(handler, method, path, body)

        patcher = patch.object(self.server.RequestHandlerClass, "api", recorded)
        patcher.start()
        self.addCleanup(patcher.stop)

    def refused(self, method: str, path: str, body=None, headers=None, prove=True) -> int:
        status, _, r = self.call(method, path, body, headers, prove)
        self.assertIsNone(r.getheader(service_auth.RESPONSE_MAC), f"{method} {path}: a refusal is never signed")
        return status

    def test_proven_requests_are_answered_and_every_reply_signed(self):
        for method, path, body in (
            ("GET", "/api/library", None),
            ("GET", "/", None),
            ("GET", "/app.css", None),
            ("GET", "/api/nothing", None),
            ("POST", "/api/search", {"query": "night train"}),
            ("POST", "/api/search", {"query": ""}),
        ):
            status, _, r = self.call(method, path, body)
            self.assertIn(status, (200, 400, 404), path)
            self.assertIsNotNone(r.getheader(service_auth.RESPONSE_MAC), f"{path}: signed")  # checked by signed.call
        self.assertEqual(["/api/library", "/api/nothing", "/api/search", "/api/search"], self.routed)

    def test_a_request_without_a_good_proof_is_refused_before_it_routes(self):
        body = {"query": "night train"}
        raw, kind = b'{"query": "night train"}', "application/json"
        now = int(time.time() * 1000)
        cases = {
            "none": ({}, False),
            "no nonce": ({service_auth.NONCE: None}, True),
            "no time": ({service_auth.TIME: None}, True),
            "no mac": ({service_auth.REQUEST_MAC: None}, True),
            "wrong mac": ({service_auth.REQUEST_MAC: "0" * 64}, True),
            "upper mac": ({service_auth.REQUEST_MAC: "A" * 64}, True),
            "short nonce": ({service_auth.NONCE: "ab"}, True),
            "stale": (signed.proof("POST", "/api/search", kind, raw, at=now - service_auth.WINDOW_MS - 5_000), False),
            "ahead": (signed.proof("POST", "/api/search", kind, raw, at=now + service_auth.WINDOW_MS + 5_000), False),
            "another key": (signed.proof("POST", "/api/search", kind, raw, key=b"k" * 32), False),
            "another method": (signed.proof("GET", "/api/search", kind, raw), False),
            "another path": (signed.proof("POST", "/api/like", kind, raw), False),
            "another body": (signed.proof("POST", "/api/search", kind, b'{"query": "night"}'), False),
            "another type": (signed.proof("POST", "/api/search", "application/json; charset=utf-8", raw), False),
        }
        for name, (headers, prove) in cases.items():
            with self.subTest(name):
                headers = {"Host": f"127.0.0.1:{self.port}", "Content-Type": "application/json", **headers}
                self.assertEqual(403, self.refused("POST", "/api/search", body, headers, prove))
        self.assertEqual([], self.routed)

    def test_a_proof_field_given_twice_is_refused(self):
        for name in (service_auth.NONCE, service_auth.TIME, service_auth.REQUEST_MAC):
            with self.subTest(name):
                status, _ = self.raw_twice(name)
                self.assertEqual(403, status)
        self.assertEqual([], self.routed)

    def raw_twice(self, name: str):
        sent = signed.proof("GET", "/api/library", "", b"")
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        conn.putrequest("GET", "/api/library", skip_host=True, skip_accept_encoding=True)
        conn.putheader("Host", f"127.0.0.1:{self.port}")
        for field, value in sent.items():
            conn.putheader(field, value)
        conn.putheader(name, sent[name])
        conn.endheaders()
        r = conn.getresponse()
        r.read()
        conn.close()
        return r.status, r.getheader(service_auth.RESPONSE_MAC)

    def test_host_origin_type_and_body_are_judged_first_and_unsigned(self):
        self.assertEqual(421, self.refused("GET", "/api/library", headers={"Host": "attacker.example"}))
        self.assertEqual(403, self.refused("GET", "/", headers={"Origin": "http://attacker.example"}))
        self.assertEqual(403, self.refused("POST", "/api/send", {}, {"Origin": "http://attacker.example"}))
        self.assertEqual(415, self.refused("POST", "/api/send", {}, {"Content-Type": "text/plain"}))
        self.assertEqual([], self.routed)

    def test_writes_and_jobs_never_start_unproven(self):
        rows = [p["row"] for p in self.app.search({"query": "night train", "n": 3})["hits"]]
        tag = self.app.lib.tags[0]
        with patch.object(self.app.adds, "start") as start, patch.object(self.app.sends, "apply") as send:
            for path, body in (
                ("/api/apply", {"rows": rows, "tag": tag, "value": True}),
                ("/api/forget", {"rows": rows, "value": True}),
                ("/api/add", {"url": "https://added.example/new"}),
                ("/api/send", {"preview": 1}),
            ):
                self.assertEqual(403, self.refused("POST", path, body, prove=False), path)
            start.assert_not_called()
            send.assert_not_called()
        self.assertEqual({}, self.app.store.app_tags())
        self.assertEqual([], self.app.forgotten.keys)


class StartupTests(unittest.TestCase):
    def test_serving_needs_a_good_key_before_any_model_loads(self):
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        bad = folder / "bad.key"
        bad.write_text("ab" * 32 + "\n")
        bad.chmod(0o644)
        with patch("tagger.app.engine.require_vectors") as load:
            for secret in (None, str(folder / "missing.key"), str(bad)):
                with self.subTest(secret), self.assertRaises(SystemExit) as stopped:
                    server.run(Paths(folder), folder, 0, False, secret)
                self.assertIn("tagger app", str(stopped.exception.code))
            load.assert_not_called()
            load.side_effect = RuntimeError("model loading starts")
            with self.assertRaisesRegex(RuntimeError, "model loading starts"):
                server.run(Paths(folder), folder, 0, False, signed.key_file(folder))
            with self.assertRaisesRegex(RuntimeError, "model loading starts"):
                server.run(Paths(folder), folder, 0, True)  # --smoke serves nothing and needs no key
