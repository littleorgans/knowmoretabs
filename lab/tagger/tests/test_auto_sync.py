"""The cutover end to end with the real binary: the Library shell (`KMT_BIN`) in front of the app on a synthetic archive
with the fake encoder, Tag and Add link driven in headless Chrome (`browser.cjs` autoSync), then the archive, the
service and the shell's own Library read back. Every tag decision, new name, forget and restore is the library's own
write; other callers' writes meet them in the one archive; the app's earlier decision, forgotten and export files stay
byte for byte, unread. Every page and name here is invented."""

import hashlib
import json
import time
import unittest

import shell
import signed
import synthetic_archive
from test_add import Synthetic, fake_binary
from test_browser import Browser

from tagger.archive import Archive, LibraryState

AT = synthetic_archive.AT
RETIRED = "2026-10-05T00:00:00Z"


def digest(paths) -> dict:
    return {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}


@unittest.skipUnless(shell.binary(), "KMT_BIN names no knowmoretabs build")
class RealAutoSyncTests(Browser, Synthetic):
    def seed(self) -> None:
        """Mixed decisions, a retired name with history, use times and fields this build does not model."""
        archive = Archive.load(self.root)
        coffee = [k for k in archive.known if "/coffee/" in k and archive.state.tags(k) == ["Coffee"]]
        self.dismissed, self.history = coffee[0], coffee[1]

        def change(state):
            state["tags"][self.dismissed]["remove"] = ["Rust"]
            synthetic_archive.decide(state, [self.history], "Woodworking")
            state["tags"][self.history]["note"] = "kept by a newer build"
            vocabulary = state["vocabulary"]
            vocabulary["Woodworking"]["retired_at"] = RETIRED
            vocabulary["Coffee"]["used_at"] = "2026-10-02T00:00:00Z"
            vocabulary["Rust"]["used_at"] = "2026-10-03T00:00:00Z"
            vocabulary["Recipes"]["colour"] = "blue"
            state["opaque"] = {"from": "a newer build"}

        self.before = synthetic_archive.edit_state(self.root, change)

    def old_files(self, keys: list[str]) -> list:
        """What the app kept before the cutover, contradicting the archive: never read again, never touched."""
        folder = self.data / "app"
        stamp = folder / "exports" / "20261001T000000Z"
        stamp.mkdir(parents=True)
        files = {
            folder / "state.json": {
                "version": 1,
                "sessions": [],
                "tags": [{"name": "Only in the old app", "created_at": 1.0}],
                "direct": [{"key": keys[0], "tag": "Only in the old app", "value": True, "at": 2.0, "batch": 1}],
            },
            folder / "forgotten.json": {"version": 1, "keys": keys[1:2]},
            stamp / "answers.jsonl": {"url": keys[0], "tags": ["Only in the old app"], "source": "kmt-tagger-app"},
        }
        for path, value in files.items():
            path.write_text(json.dumps(value) + "\n")
        return list(files)

    def post(self, path: str, body: dict):
        """A write from another caller, as the Library's page sends it."""
        status, _, reply = self.shell.call("POST", path, body, {"Origin": f"http://127.0.0.1:{self.shell.port}"})
        return status, json.loads(reply)

    def state(self) -> dict:
        return json.loads((self.root / "library.json").read_text())

    def test_tag_add_undo_and_external_edits_share_the_archive(self):
        self.seed()
        self.app = self.start(self.load()[0])
        self.app.binary = fake_binary(self.work)
        espresso = [h["row"] for h in self.app.search({"query": "espresso", "n": 20})["hits"]]
        selected, pinned = espresso[2:5], [espresso[3], espresso[6]]
        self.assertEqual(200, signed.call(self.shell.service_port, "POST", "/api/selection", {"rows": selected})[0])
        self.assertEqual(200, signed.call(self.shell.service_port, "POST", "/api/pins", {"rows": pinned})[0])
        key = lambda row: self.app.lib.records[row]["key"]  # noqa: E731
        lists = {name: (self.data / "app" / name).read_text() for name in ("selection.json", "pinned.json")}
        old = self.old_files([key(r) for r in espresso[:3]])
        kept = digest([*old, *sorted((self.root / "snapshots").rglob("*.json"))])

        seen = json.loads(self.run_case("autoSync"))
        state, vocabulary = self.state(), self.state()["vocabulary"]
        adds = lambda url: state["tags"].get(url, {}).get("add", [])  # noqa: E731
        removes = lambda url: state["tags"].get(url, {}).get("remove", [])  # noqa: E731

        # Tag shows the archive: its tags, every active name, recency from the library's use times
        self.assertEqual(sorted({*synthetic_archive.DEFINITIONS} - {"Woodworking"}, key=str.lower), seen["appTags"])
        self.assertEqual(["Rust", "Coffee"], seen["recent"])
        seeded = LibraryState.parse(json.dumps(self.before).encode())
        self.assertEqual(seeded.tags(seen["shown"]["url"]), seen["shown"]["tags"], "a tile shows the archive's tags")
        # a new name in the library's spelling; an Undo restores the pages it touched, despite the other caller
        self.assertIn("Sleeper cars", vocabulary)
        self.assertEqual(["Sleeper cars"], [n for n in adds(seen["made"]) if n == "Sleeper cars"])
        for url in seen["touched"]:
            self.assertNotIn(
                "Sleeper cars", adds(url) + removes(url), "the touched decision as it was before the write"
            )
        self.assertEqual({"status": 200, "unknown": 1}, seen["elsewhere"], "the other caller's unknown page counted")
        self.assertIn("Recipes", adds(seen["unrelated"]), "the unrelated edit survives the Undo")
        # a retired name brought back by a write, retired again by its Undo; its history kept
        self.assertEqual({"url": seen["revived"]["url"], "listed": True, "shown": True}, seen["revived"])
        self.assertEqual(RETIRED, vocabulary["Woodworking"]["retired_at"])
        self.assertNotIn("Woodworking", adds(seen["revived"]["url"]))
        self.assertIn("Woodworking", adds(self.history))
        self.assertTrue(seen["refused"]["kept"])
        self.assertIn("longer than 40 characters", seen["refused"]["toast"])
        # Forget and its Undo through Tag: the lists keep the page in its place throughout
        forgot = seen["forgot"]
        self.assertEqual(forgot["saved"], forgot["kept"])
        self.assertNotIn(forgot["url"], forgot["shown"] + forgot["pinned"])
        self.assertEqual(forgot["saved"], seen["restored"]["shown"])
        self.assertEqual([key(r) for r in pinned], seen["restored"]["pinned"])
        self.assertNotIn(forgot["url"], state["forgotten"])
        self.assertEqual(lists, {n: (self.data / "app" / n).read_text() for n in lists}, "durable lists unchanged")
        # another caller's retire and forget, followed by the service's next read without a restart
        self.assertEqual({"typography": False, "external": False}, seen["followed"])
        self.assertIn("retired_at", vocabulary["Typography"])
        self.assertEqual(sorted({*self.before["forgotten"], seen["external"]}), sorted(state["forgotten"]))
        # Add link: picks held until Indexed, one write on the key, its Undo; the use stays recorded
        url = seen["add"]["url"]
        self.assertEqual(["pend", "pend"], seen["pending"])
        self.assertEqual(
            [{"path": "/api/tags", "body": {"urls": [url], "add": ["Coffee", "Recipes"], "remove": []}}],
            seen["add"]["writes"][:1],
        )
        self.assertEqual(2, seen["add"]["after"], "one write and one Undo")
        self.assertEqual([], [n for n in adds(url) if n in ("Coffee", "Recipes")])
        for name in ("Coffee", "Recipes"):
            self.assertGreater(vocabulary[name]["used_at"], "2026-10-03T00:00:00Z", f"{name}: an undone add was a use")
        # the rest of the library as it was: dismissals, fields this build does not model, old files, snapshots
        self.assertEqual(["Rust"], removes(self.dismissed))
        self.assertEqual("kept by a newer build", state["tags"][self.history]["note"])
        self.assertEqual("blue", vocabulary["Recipes"]["colour"])
        self.assertEqual({"from": "a newer build"}, state["opaque"])
        self.assertEqual(kept, digest([*old, *sorted((self.root / "snapshots").rglob("*.json"))]))
        self.assertEqual(["20261001T000000Z"], [p.name for p in (self.data / "app" / "exports").iterdir()])
        self.assertNotIn("Only in the old app", vocabulary)
        # the Library, reloaded through the shell, shows the same archive
        library = json.loads(self.shell.call("GET", "/api/library")[2])
        pages = {p["url"]: p for p in library["pages"]}
        self.assertIn("Sleeper cars", pages[seen["made"]]["tags"])
        self.assertIn("Recipes", pages[seen["unrelated"]]["tags"])
        self.assertTrue(pages[seen["external"]].get("forgotten"))
        self.assertFalse(pages[forgot["url"]].get("forgotten", False))
        for path in (
            "/model/api/apply",
            "/model/api/undo",
            "/model/api/tags",
            "/model/api/send",
            "/model/api/sessions",
        ):
            self.assertEqual(404, self.shell.call("POST", path, {})[0], path)

        # the library's writes never wait on the service: with its lock held, or with it stopped
        target = seen["made"]
        with self.app.lock:
            start = time.monotonic()
            self.assertEqual(200, self.post("/api/tags", {"urls": [target], "add": ["Held"], "remove": []})[0])
            self.assertLess(time.monotonic() - start, 5)
        library = json.loads(self.shell.call("GET", "/model/api/library")[2])
        self.assertIn("Held", library["app_tags"], "followed once the lock is free")
        self.assertIn("Held", library["recent"][:3], "a use, among the latest (to the second)")
        bytes_before = (self.root / "library.json").read_bytes()
        status, refused = self.post("/api/tags", {"urls": [target], "add": ["x" * 41], "remove": []})
        self.assertEqual(400, status, refused)
        self.assertEqual(bytes_before, (self.root / "library.json").read_bytes(), "a refused write changes nothing")
        self.shell.stop_service()
        self.assertEqual(200, self.post("/api/tags", {"urls": [target], "add": ["Down"], "remove": []})[0])
        self.shell.service = self.shell.serve(signed.KEY, self.shell.service_port)
        hits = json.loads(self.shell.call("POST", "/model/api/search", {"query": "night train", "n": 50})[2])["hits"]
        shown = next(h for h in hits if h["url"] == target)["tags"]
        self.assertEqual(["Down", "Held", "Sleeper cars"], [t for t in shown if t in ("Down", "Held", "Sleeper cars")])
