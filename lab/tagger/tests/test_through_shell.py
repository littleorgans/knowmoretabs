"""The real Library shell in front of the app on the synthetic archive, with the fake encoder: what a browser asks
the shell for below /model/ comes back from the app, proven and typed, and only that; a tag the shell writes is the
app's on its next answer; when the app stops, or something without the launch key answers on its port, Tag and Add
link say so and the Library goes on."""

import dataclasses
import json
import shutil
import tempfile
import unittest
from pathlib import Path

import shell
import synthetic_archive
from test_app import Fixture, fake_embed, fake_encode

from tagger.app import server

STATIC = Path(server.__file__).with_name("static")


@unittest.skipUnless(shell.binary(), "KMT_BIN names no knowmoretabs build")
class ThroughShellTests(Fixture):
    def setUp(self):
        self.data = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.data)
        shutil.copytree(self.tmp / "archive", self.data / "archive")
        lib = dataclasses.replace(self.lib, root=self.data / "archive")
        self.app = server.App(lib, self.data / "app", fake_encode, fake_embed)
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
        self.assertEqual(404, self.shell.call("GET", "/model/api/sessions/999")[0])

    def test_a_tag_the_shell_writes_is_the_apps_on_its_next_answer(self):
        hits = json.loads(self.shell.call("POST", "/model/api/search", {"query": "night train", "n": 5})[2])["hits"]
        urls = [h["url"] for h in hits[:2]]
        body = {"urls": urls, "add": ["Sleeper  cars"], "remove": []}
        status, _, reply = self.shell.call("POST", "/api/tags", body, {"Origin": f"http://127.0.0.1:{self.shell.port}"})
        self.assertEqual(200, status, reply)
        self.assertEqual(
            {url: True for url in urls}, {u: "Sleeper cars" in t for u, t in json.loads(reply)["tags"].items()}
        )
        again = json.loads(self.shell.call("POST", "/model/api/search", {"query": "night train", "n": 5})[2])["hits"]
        self.assertEqual([True, True], ["Sleeper cars" in h["tags"] for h in again[:2]])
        library = json.loads(self.shell.call("GET", "/model/api/library")[2])
        self.assertEqual("Sleeper cars", library["recent"][0])
        self.assertIn({"name": "Sleeper cars", "positives": 2, "source": "zero shot"}, library["tags"])
        for path in ("/model/api/apply", "/model/api/tags", "/model/api/send"):
            self.assertEqual(404, self.shell.call("POST", path, {})[0], path)

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

    def test_malformed_vocabulary_is_refused_by_both_readers(self):
        root = self.app.lib.root
        good = json.loads((root / "library.json").read_text())
        shown = self.app.lib
        for field, value in (
            ("created_at", "not a time"),
            ("created_at", "2026-10-01T00:00:00"),
            ("retired_at", "not a time"),
            ("retired_at", "2026-10-01T00:00:00"),
            ("definition", 7),
        ):
            with self.subTest(field=field, value=value):
                damaged = json.loads(json.dumps(good))
                damaged["vocabulary"]["Coffee"][field] = value
                synthetic_archive.write_state(root, damaged)
                self.assertEqual(500, self.shell.call("GET", "/api/library")[0], "Rust refuses the state")
                self.assertEqual(503, self.shell.call("GET", "/model/api/library")[0], "the service refuses it too")
                self.assertIs(shown, self.app.lib, "a failed read never publishes a projection")
        synthetic_archive.write_state(root, good)
        self.assertEqual(200, self.shell.call("GET", "/api/library")[0])
        self.assertEqual(200, self.shell.call("GET", "/model/api/library")[0], "a later valid replacement recovers")

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
        self.assertNotIn(b'id="a-panel"', page, "not one byte of the app's page")
        self.assertEqual(502, self.shell.call("POST", "/model/api/rank", {"query": "x"})[0])
