"""The real Library shell in front of the app on the synthetic archive, with the fake encoder: what a browser asks
the shell for below /model/ comes back from the app, proven and typed, and only that; when the app stops, or
something without the launch key answers on its port, Tag and Add link say so and the Library goes on."""

import json
import shutil
import tempfile
import unittest
from pathlib import Path

import shell
from test_app import Fixture, fake_embed, fake_encode

from tagger.app import server
from tagger.app.store import Store

STATIC = Path(server.__file__).with_name("static")


@unittest.skipUnless(shell.binary(), "KMT_BIN names no knowmoretabs build")
class ThroughShellTests(Fixture):
    def setUp(self):
        self.data = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.data)
        self.app = server.App(self.lib, Store(self.data / "app"), fake_encode, fake_embed)
        self.shell = shell.Shell(self.app, self.data, shell.binary())
        self.addCleanup(self.shell.close)

    def test_the_screens_page_files_api_and_images_come_through(self):
        status, headers, page = self.shell.call("GET", "/model/")
        self.assertEqual((200, "text/html; charset=utf-8"), (status, headers["content-type"]))
        self.assertIn("style-src 'self' 'unsafe-inline'", headers["content-security-policy"])
        self.assertIn(b'<a href="/model/#add" id="nav-add">Add link</a>', page)
        self.assertIn(b'<a href="/#pages">Pages</a>', page)
        self.assertNotIn("x-kmt-response-mac", headers)
        for name, kind in (("core.js", "text/javascript"), ("lit.js", "text/javascript"), ("app.css", "text/css")):
            status, headers, body = self.shell.call("GET", f"/model/{name}")
            self.assertEqual((200, f"{kind}; charset=utf-8"), (status, headers["content-type"]), name)
            self.assertEqual((STATIC / name).read_bytes(), body, name)
        status, _, body = self.shell.call("GET", "/model/api/library")
        self.assertEqual(200, status)
        self.assertEqual(self.app.library()["pages"], json.loads(body)["pages"])
        status, _, body = self.shell.call("POST", "/model/api/search", {"query": "night train", "n": 20})
        hits = json.loads(body)["hits"]
        self.assertEqual(200, status)
        shown = next(h for h in hits if h["image"])
        status, headers, jpeg = self.shell.call("GET", f"/model/img/{shown['image']}")
        self.assertEqual((200, "image/jpeg"), (status, headers["content-type"]))
        self.assertEqual(Path(self.lib.records[shown["row"]]["image_path"]).read_bytes(), jpeg)
        status, _, body = self.shell.call("POST", "/model/api/rank", {"query": "night train"})
        self.assertEqual((200, len(self.lib.records)), (status, len(json.loads(body)["keys"])))
        rows = [h["row"] for h in hits[:2]]
        tag = self.app.lib.tags[0]
        status, _, _ = self.shell.call("POST", "/model/api/apply", {"rows": rows, "tag": tag, "value": True})
        self.assertEqual(200, status)
        self.assertEqual({self.lib.records[r]["key"] for r in rows}, set(self.app.store.app_tags()))
        self.assertEqual(404, self.shell.call("GET", "/model/api/sessions/999")[0])

    def test_the_shell_refuses_what_it_does_not_forward(self):
        for method, path, body, headers, status in (
            ("GET", "/model/server.py", None, None, 404),
            ("GET", "/model/../tagger/app/server.py", None, None, 404),
            ("GET", "/model/static/core.js", None, None, 404),
            ("POST", "/model/api/search", {"query": "x"}, {"Content-Type": "text/plain"}, 415),
            ("POST", "/model/api/search", {"query": "x"}, {"Origin": "http://attacker.example"}, 403),
            ("GET", "/model/api/library", None, {"Host": "attacker.example"}, 403),
        ):
            self.assertEqual(status, self.shell.call(method, path, body, headers)[0], path)

    def test_when_the_app_stops_tag_and_add_link_say_so_and_the_library_goes_on(self):
        self.shell.stop_service()
        status, headers, page = self.shell.call("GET", "/model/")
        self.assertEqual((503, "text/html; charset=utf-8"), (status, headers["content-type"]))
        self.assertIn(b"The model service is not answering", page)
        self.assertIn(b'id="nav-tag"', page)
        self.assertNotIn("unsafe-inline", headers["content-security-policy"])
        status, headers, _ = self.shell.call("GET", "/model/api/library")
        self.assertEqual((503, "application/json"), (status, headers["content-type"]))
        self.assertEqual(200, self.shell.call("GET", "/api/library")[0])

    def test_an_app_without_the_launch_key_is_not_believed(self):
        self.shell.stop_service()
        self.shell.service = self.shell.serve(b"another launch's key, not ours!!", self.shell.service_port)
        status, _, page = self.shell.call("GET", "/model/")
        self.assertEqual(502, status)
        self.assertIn(b"did not prove", page)
        self.assertNotIn(b"Send to library", page, "not one byte of the app's page")
        self.assertEqual(502, self.shell.call("POST", "/model/api/rank", {"query": "x"})[0])
