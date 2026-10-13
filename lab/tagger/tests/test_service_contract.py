"""The service's side of what the shell and this service each restate about the other: its constants against the
shared contract that `src/model_service.rs` holds the shell's to."""

import json
import mimetypes
import re
import unittest
from pathlib import Path

from tagger.app import kmt, server

CONTRACT = json.loads((Path(__file__).parents[3] / "tests" / "fixtures" / "service_contract.json").read_text())


class ContractTests(unittest.TestCase):
    def test_the_service_restates_the_shell_as_the_shared_contract(self):
        files = {f.name: mimetypes.guess_type(f.name)[0] for f in server.STATIC.iterdir() if f.name != "index.html"}
        self.assertEqual(CONTRACT["files"], files)
        # Expand the fixture's decimal segment, preserving the service's regex text and flags.
        api = {
            (
                method,
                re.compile(
                    "/api/" + "/".join(r"(\d+)" if part == "#" else re.escape(part) for part in path.split("/"))
                ),
            )
            for method, path in CONTRACT["api"]
        }
        self.assertEqual(
            api, {(method, pattern) for method, pattern, _ in server.ROUTES + server.RANK_ROUTES + server.JOB_ROUTES}
        )
        self.assertEqual(CONTRACT["archive_header"], server.ARCHIVE_HEADER)
        self.assertEqual(CONTRACT["body_limit"], server.BODY_LIMIT)
        self.assertEqual(set(CONTRACT["page_policy"]), set(server.CSP.split("; ")))
        self.assertEqual(CONTRACT["rank_top"], server.MAX_RESULTS)
        self.assertEqual(CONTRACT["source_prefixes"], list(kmt.SOURCES))
