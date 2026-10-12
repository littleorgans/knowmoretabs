"""Controlled round trips through the shipped frontend scripts, without browser data."""

import shutil
import subprocess
from pathlib import Path

import pytest


CASES = {
    "frontend.cjs": [
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
        "sendWait",
        "sendPreview",
        "sendNothing",
        "sendRestore",
        "sendUncertain",
        "sendResult",
        "openControl",
        "openSearch",
        "openReview",
        "openNoDrag",
        "newTag",
        "newTagExisting",
        "newTagCancelAndRefusal",
        "newTagKeyboardFocus",
        "select",
        "chipActsOnSelection",
        "bulkUndo",
        "tagPageUndo",
        "tagPageOverlap",
        "tagPageSiblingFailure",
        "forgetAndUndo",
        "forgetUndoOrder",
        "forgetThenNext",
        "forgetUndoAfterNavigation",
        "startupDuringPage",
        "pinView",
        "untagAndOpenDoNotSelect",
        "likeToggle",
        "newTagApplies",
        "keysBesideCheckbox",
        "escapeFromTextInputs",
        "likeNaming",
        "pages",
        "restorePage",
        "likeBackToPage",
        "likeWaitsForTag",
        "backDuringPage",
        "startupListsKeepChanges",
        "startupEdits",
        "startupForgetAndUndo",
        "startupTagsKeepChanges",
    ],
    "frontend_add.cjs": [
        "addStates",
        "addRequests",
        "addSignedInStages",
        "panelLetters",
        "addPicksHeld",
        "addPicksBackground",
        "addPicksOutcomes",
        "addFilterEnter",
        "addPicksAddress",
        "addPicksBeforeReply",
        "addPicksStalePoll",
    ],
}


@pytest.mark.parametrize(("script", "case"), [(script, case) for script, cases in CASES.items() for case in cases])
def test_frontend_round_trips(script, case):
    node = shutil.which("node")
    if node is None:
        pytest.skip("Node is needed to execute the frontend scripts")
    result = subprocess.run(
        [node, str(Path(__file__).with_name(script)), case], capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 0, result.stderr
