"""The screens in headless Chrome (`browser.cjs`) on the app serving the synthetic archive, through the real Library
shell (`KMT_BIN`) as `just up` runs them: lit-html under the shell's CSP, every request to the shell's /model/ or the
library's own writes, tile nodes kept across updates, result pages, ≈ back to the same page, Forget, Undo and Pin, and
Add link driven against a fake knowmoretabs, each on a fresh copy of the archive."""

import dataclasses
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from unittest.mock import patch
from urllib.parse import quote

import pytest
import shell
import synthetic_archive
from test_add import Synthetic, fake_binary
from test_app import DIM, Fixture, fake_embed, fake_encode, fake_image

from tagger.app import engine, server
from tagger.archive import Archive
from tagger.paths import Paths

MAC_CHROME = Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")


def chrome() -> str | None:
    found = next(filter(None, map(shutil.which, ("google-chrome", "chromium", "chrome"))), None)
    return found or (str(MAC_CHROME) if MAC_CHROME.exists() else None)


def node_case(browser: str, profile: str, url: str, case: str) -> subprocess.CompletedProcess:
    script = str(Path(__file__).with_name("browser.cjs"))
    return subprocess.run(["node", script, browser, profile, url, case], capture_output=True, text=True, timeout=60)


def test_a_chrome_gone_mid_case_fails_it():
    """The case waits on Chrome's answers: a Chrome that exits fails it, never a quiet exit 0."""
    if shutil.which("node") is None:
        pytest.skip("Node is needed to run the browser harness")
    with tempfile.TemporaryDirectory() as profile:
        result = node_case(shutil.which("false"), profile, "about:blank", "identity")
    assert result.returncode == 1, result.stderr


class Browser:
    def start(self, lib) -> server.App:
        if shutil.which("node") is None or chrome() is None:
            pytest.skip("Node and Chrome are needed to run the screen in a browser")
        # Not a skip: the app answers only the shell, so without it these cases would test nothing.
        assert shell.binary(), "set KMT_BIN to a knowmoretabs build (cargo build --locked): the screens run behind it"
        self.data = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.data)
        app = server.App(lib, self.data / "app", fake_encode, fake_embed)
        self.shell = shell.Shell(app, self.data, shell.binary())
        self.addCleanup(self.shell.close)
        return app

    def run_case(self, case: str, fragment: str = "") -> str:
        """Run the case on the shell's /model/ page; what it returns, as JSON (empty when it returns nothing)."""
        url = f"{self.shell.url}model/{fragment}"
        with tempfile.TemporaryDirectory() as profile:
            result = node_case(chrome(), profile, url, case)
            self.assertEqual(0, result.returncode, result.stderr)  # before the profile's removal can hide it
        return result.stdout


class BrowserTests(Browser, Fixture):
    def setUp(self):
        root = Path(tempfile.mkdtemp()) / "archive"
        self.addCleanup(shutil.rmtree, root.parent)
        shutil.copytree(self.tmp / "archive", root)  # the library's writes land on this test's own copy
        self.app = self.start(dataclasses.replace(self.lib, root=root))

    def test_startup_reply_preserves_newer_page_tags(self):
        saved = [p["row"] for p in self.app.search({"query": "night train", "n": 50})["hits"][:2]]
        self.app.replace(self.app.selection, {"rows": saved[:1]})
        self.app.replace(self.app.pins, {"rows": saved[1:]})
        self.run_case("startupTags")
        self.assertTrue(self.app.page(saved[0])["tags"])

    def test_startup_forget_undo_preserves_saved_selection_and_pins(self):
        saved = [p["row"] for p in self.app.search({"query": "night train", "n": 50})["hits"][:3]]
        for case in ("startupForgetUndoEarly", "startupForgetUndoLoaded"):
            with self.subTest(case=case):
                self.app.replace(self.app.selection, {"rows": saved})
                self.app.replace(self.app.pins, {"rows": saved})
                self.run_case(case)
                for keys in (self.app.selection, self.app.pins):
                    self.assertEqual(saved, [p["row"] for p in self.app.listed(keys)["pages"]])

    def test_a_selection_or_tag_update_keeps_the_tile_node(self):
        self.run_case("identity")

    def test_startup_finishing_during_next_keeps_page_two(self):
        self.run_case("startupDuringPage")

    def test_forget_undo_after_navigation_refreshes_the_rank_and_restores_lists(self):
        self.run_case("forgetUndoAfterNavigation")

    def test_forget_undo_and_pin_select_nothing_and_persist_over_a_reload(self):
        self.run_case("forgetPin")

    def test_previous_and_next_replace_the_hits_from_the_top_keeping_the_selection(self):
        self.run_case("pages")

    def test_leaving_more_like_this_returns_to_the_same_page_and_scroll(self):
        self.run_case("likeBack")

    def test_a_new_search_returns_to_page_one_and_the_top(self):
        self.run_case("newSearch")

    def test_pagination_focuses_the_first_replacement_tile(self):
        self.run_case("pageFocus")

    def test_back_and_forward_restore_result_pages_and_scroll(self):
        self.run_case("pageHistory")

    def test_reload_restores_the_page_and_scroll(self):
        self.run_case("reloadScroll")

    def test_back_after_a_long_read_returns_to_the_page_left(self):
        self.run_case("longScroll")


class AddBrowserTests(Browser, Synthetic):
    def setUp(self):
        super().setUp()
        self.app = self.start(self.load()[0])
        self.app.binary = fake_binary(self.work)

    def paced(self, seconds: float) -> None:
        """Each stage line this long after the last; a state shorter than a poll may never show."""
        env = patch.dict(os.environ, {"FAKE_KMT_DELAY": str(seconds)})
        env.start()
        self.addCleanup(env.stop)

    def logged(self):
        """A reader of each knowmoretabs run's arguments, root left out."""
        log = self.work / "argv.jsonl"
        env = patch.dict(os.environ, {"FAKE_KMT_LOG": str(log)})
        env.start()
        self.addCleanup(env.stop)
        return lambda: [json.loads(line)["argv"][2:] for line in log.read_text().splitlines()]

    def test_a_link_is_added_tagged_and_found(self):
        self.paced(0.3)
        self.run_case("addFlow", "#add")

    def test_cells_hold_their_place_from_link_to_index(self):
        self.paced(0.3)
        names = ["Airships", "Archive", "Atlas", "Canals", "Coffee", "Docs", "Éclair", "Ferries", "iOS", "Maps"]
        names += ["An invented tag name cut in its cell", "3D printing"]
        names += ["Night trains", "Sleeper cabins", "Timetables"]  # the fake page's night train: the model's picks
        used = ["Archive", "Atlas", "Canals", "Coffee", "Docs", "Ferries", "iOS", "Maps"]  # more than its line fits

        def seed(state):
            for name in names:
                state["vocabulary"].setdefault(name, {"created_at": synthetic_archive.AT})
            for i, name in enumerate(used):
                state["vocabulary"][name]["used_at"] = f"2026-10-0{i + 1}T00:00:00Z"
            for name in synthetic_archive.DEFINITIONS:  # only the model's names are suggested; the rest are retired
                if name not in names:
                    state["vocabulary"][name]["retired_at"] = "2026-10-09T00:00:00Z"

        synthetic_archive.edit_state(self.root, seed)
        measured = json.loads(self.run_case("addStable", "#add"))
        print("addStable", json.dumps(measured))
        self.assertEqual(["Sleeper cabins", "Timetables"], measured["suggested"], "best first, less the pick")
        tags = Archive.load(self.root).state.tags("https://added.example/notes/stable")
        self.assertEqual([measured["tag"], "Timetables"], tags, "the pick and the suggestion reached the library")
        self.assertNotIn(measured["tag"], used, "the pick was not recent before")

    def test_a_deep_link_fills_the_box_and_waits_for_enter(self):
        runs = self.logged()
        link, title = "https://added.example/a b?x=1&y=é", "A deep linked title"
        self.run_case("addDeepLink", f"#add={quote(link, safe='')}&title={quote(title, safe='')}")
        self.assertEqual(
            [
                ["add", "--json", f"--title={title}", "--", link],
                ["add", "--json", "--", "https://added.example/notes/two"],
            ],
            runs(),
        )

    def test_each_failure_shows_its_value_and_its_action(self):
        runs = self.logged()
        self.run_case("addFailures", "#add")
        self.assertEqual(["add", "--json", "--retry", "content"], runs()[-1][:-2], "Retry reads the text alone")

    def test_retry_runs_only_the_failed_stages(self):
        runs = self.logged()
        self.run_case("addRetry", "#add")
        retries = [run[2:-2] for run in runs() if run[0] == "add" and ("--retry" in run or "--signed-in" in run)]
        signed_in = [["--retry", "content", "--signed-in"]] * 2
        self.assertEqual(
            [
                ["--retry", "content"],
                *signed_in * 3,
                ["--retry", "image"],
                ["--retry", "content", "--retry", "image"],
                ["--retry", "content"],
                ["--retry", "content"],
                ["--retry", "content"],
            ],
            retries,
        )
        relapse = [run[0] for run in runs() if run[-1] == "https://added.example/relapse"]
        self.assertEqual(["add", "forget", "restore", "add", "add"], relapse, "one restore")

    def test_try_signed_in_selects_thin_403_and_failed_image_even_without_chrome(self):
        runs = self.logged()
        self.run_case("addSignedStages", "#add")
        selected = [run[2:-2] for run in runs() if "--signed-in" in run]
        self.assertEqual([["--retry", "content", "--retry", "image", "--signed-in"]] * 2, selected)

    def test_a_pasted_web_address_starts_at_once(self):
        self.run_case("addPaste", "#add")

    def test_snapshot_refusal_replaces_all_segments_and_disables_the_tile(self):
        protected = self.data / "snapshot-synthetic"
        shutil.copytree(self.root, protected)
        lib = engine.load(self.paths, protected, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)[0]
        self.app.lib = dataclasses.replace(lib, paths=Paths(self.data))  # a copy inside the app's data directory
        with patch.object(self.app.adds, "_command") as command:
            self.run_case("addSnapshot", "#add")
            command.assert_not_called()
