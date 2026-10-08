"""The one screen in headless Chrome (`browser.cjs`) on the app serving the synthetic archive: lit-html under the
server's CSP, tile nodes kept across updates, result pages, and ≈ back to the same page."""

import shutil
import subprocess
import tempfile
import threading
from pathlib import Path

import pytest
from test_app import Fixture, fake_encode

from tagger.app import server
from tagger.app.store import Store

MAC_CHROME = Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")


def chrome() -> str | None:
    found = next(filter(None, map(shutil.which, ("google-chrome", "chromium", "chrome"))), None)
    return found or (str(MAC_CHROME) if MAC_CHROME.exists() else None)


class BrowserTests(Fixture):
    def setUp(self):
        if shutil.which("node") is None or chrome() is None:
            pytest.skip("Node and Chrome are needed to run the screen in a browser")
        self.data = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.data)
        self.server = server.serve(server.App(self.lib, Store(self.data), fake_encode), 0)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def run_case(self, case: str) -> None:
        url = f"http://127.0.0.1:{self.server.server_address[1]}/"
        with tempfile.TemporaryDirectory() as profile:
            script = str(Path(__file__).with_name("browser.cjs"))
            result = subprocess.run(
                ["node", script, chrome(), profile, url, case], capture_output=True, text=True, timeout=60
            )
        self.assertEqual(0, result.returncode, result.stderr)

    def test_a_selection_or_tag_update_keeps_the_tile_node(self):
        self.run_case("identity")

    def test_previous_and_next_replace_the_hits_from_the_top_keeping_the_selection(self):
        self.run_case("pages")

    def test_leaving_more_like_this_returns_to_the_same_page_and_scroll(self):
        self.run_case("likeBack")
