"""Controlled round trips through the shipped frontend scripts, without browser data."""

import shutil
import subprocess
from pathlib import Path

import pytest


@pytest.mark.parametrize(
    "case",
    [
        "search",
        "restore",
        "exactQuery",
        "searchAfterCut",
        "pickAfterCut",
        "exclude",
        "cut",
        "pickIncluded",
        "flips",
        "switchedSet",
        "acceptAfterFlip",
        "confirmAllPending",
        "exportWait",
        "exportCommand",
        "openControl",
        "openSearch",
        "openReview",
        "openNoDrag",
        "newTag",
        "newTagExisting",
        "newTagCancelAndRefusal",
        "newTagKeyboardFocus",
        "select",
        "tagToggle",
        "untagAndOpenDoNotSelect",
        "likeToggle",
        "newTagApplies",
        "keysBesideCheckbox",
        "escapeFromTextInputs",
        "likeNaming",
        "pages",
        "restorePage",
        "likeBackToPage",
    ],
)
def test_frontend_round_trips(case):
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is needed to execute the frontend scripts")
    result = subprocess.run(
        [node, str(Path(__file__).with_name("frontend.cjs")), case], capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 0, result.stderr
