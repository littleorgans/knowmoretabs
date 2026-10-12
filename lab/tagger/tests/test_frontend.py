"""Controlled round trips through the shipped frontend scripts, without browser data."""

import re
import shutil
import subprocess
from pathlib import Path

import pytest


CASES = {
    "frontend.cjs": [
        "openControl",
        "select",
        "chipActsOnSelection",
        "bulkUndo",
        "tagPageUndo",
        "writesAreTheLibrarys",
        "tagRecent",
        "writesNeverWaitOnTheService",
        "searchKeepsNewerTags",
        "forgetAndUndo",
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
        "addPicksFailure",
        "addKeepsNewerTags",
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
    """The Library serves this page below /model/: a root path there would reach the Library, not the service. The
    one root path is core's `K.shell`, the library's own writes, and every write to the library goes through it;
    only the shell's nav names other root paths."""
    static = Path(__file__).parents[1] / "tagger" / "app" / "static"
    scripts = {f.name: f.read_text() for f in sorted(static.glob("*.js")) if f.name != "lit-html.js"}
    assert sorted(scripts) == ["add.js", "app.js", "core.js", "lit.js", "pages.js", "panel.js", "strip.js", "views.js"]
    shell = "  K.shell = (route, body) => K.api(`/api/${route}`, body);"
    assert [line for line in scripts["core.js"].splitlines() if re.search(r"""["'`]/(api|img)/""", line)] == [shell]
    for name, text in scripts.items():
        if name != "core.js":
            assert not re.search(r"""["'`]/(api|img)/""", text), name
        assert not re.search(r"""K\.api\(["'`]api/(tags|vocabulary|forget|restore|apply|undo)""", text), name
    writes = re.findall(r'mutate\("(\w+)"', scripts["pages.js"])
    assert sorted(set(writes)) == ["forget", "restore", "tags", "vocabulary"]
    assert 'src="img/${p.image}"' in scripts["core.js"]
    assert "<!-- nav:start --><!-- nav:end -->" in (static / "index.html").read_text()
