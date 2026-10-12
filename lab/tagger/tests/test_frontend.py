"""Controlled round trips through the shipped frontend scripts, without browser data."""

import re
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
        "tagRecent",
        "tagPageOverlap",
        "tagPageSiblingFailure",
        "tagPagePartialToast",
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
        "clearShowsAll",
        "emptySearchShowsAll",
        "likeBackToPage",
        "likeWaitsForTag",
        "backDuringPage",
        "startupListsKeepChanges",
        "startupEdits",
        "startupForgetAndUndo",
        "startupTagsKeepChanges",
        "outage",
    ],
    "frontend_add.cjs": [
        "addStates",
        "addRequests",
        "addSignedInStages",
        "panelLetters",
        "panelRecent",
        "addLibraryWrites",
        "addPicksHeld",
        "addPicksBackground",
        "addPicksOutcomes",
        "addFilterEnter",
        "addPicksAddress",
        "addPicksBeforeReply",
        "addPicksStalePoll",
        "addPicksDetour",
        "addSuggested",
        "addFollowUpNamesTheKey",
        "addFollowUpPicksBackground",
        "addFollowUpStalePoll",
        "addRestoreFollowsTheKey",
        "addPollFails",
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


def test_every_service_path_is_relative_to_the_page():
    """The Library serves this page below /model/: a root path there would reach the Library, not the service.
    Both thumbnail emitters and the dormant scripts included; only the shell's nav names root paths."""
    static = Path(__file__).parents[1] / "tagger" / "app" / "static"
    scripts = [f for f in sorted(static.glob("*.js")) if f.name != "lit-html.js"]
    assert len(scripts) == 12
    for script in scripts:
        assert not re.search(r"""["'`]/(api|img)/""", script.read_text()), script.name
    for emitter in ("core.js", "review.js"):
        assert 'src="img/${p.image}"' in (static / emitter).read_text(), emitter
    assert "<!-- nav:start --><!-- nav:end -->" in (static / "index.html").read_text()
