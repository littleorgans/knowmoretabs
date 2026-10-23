"""Publication and privacy checks shared by the search and tag studies."""

from pathlib import Path

import pytest

from tagger import eval_runner
from tagger.paths import Paths, read_json


@pytest.fixture
def evaluation(tmp_path):
    def run(evaluate, **kwargs):
        return eval_runner.run(
            Paths(tmp_path / "data"),
            tmp_path / "archive",
            tmp_path / "out",
            evaluate,
            stem="result",
            log=tmp_path / "out" / "diagnostics.log",
            **kwargs,
        )

    return run


def test_private_diagnostics_and_bounded_failure(evaluation, tmp_path):
    def fail():
        print("private diagnostic")
        raise RuntimeError("private input")

    with pytest.raises(eval_runner.Invalid, match="^unreadable inputs or unavailable runtime$"):
        evaluation(fail)
    log = (tmp_path / "out" / "diagnostics.log").read_text()
    assert "private diagnostic" in log and "private input" in log
    assert not (tmp_path / "out" / "result.json").exists()


def test_nonfinite_results_refuse_before_publication(evaluation, tmp_path):
    with pytest.raises(eval_runner.Invalid, match="unreadable inputs"):
        evaluation(lambda: {"metric": float("nan")})
    assert not (tmp_path / "out" / "result.json").exists()


def test_command_roundtrip_and_existing_report_guard(evaluation, tmp_path):
    import shlex

    command = ["tag-eval", "--split", str(tmp_path / "split with spaces.json")]
    result = evaluation(lambda: {"metric": 1}, report=lambda result: "report", command=command, binary="test binary")
    args = shlex.split(result["command"])
    assert args[args.index("tag-eval") :] == [
        *command,
        "--out",
        str(tmp_path / "out"),
        "--kmt-bin",
        "test binary",
    ]
    assert read_json(tmp_path / "out" / "result.json") == result
    (tmp_path / "out" / "result.json").unlink()
    with pytest.raises(eval_runner.Invalid, match="already exist"):
        evaluation(lambda: pytest.fail("must not evaluate"), report=lambda result: "replacement")
    assert (tmp_path / "out" / "result.md").read_text() == "report"


def test_live_and_symlink_inputs_refuse_before_evaluation(tmp_path):
    for root in (Path.home() / ".knowmoretabs", tmp_path / "archive"):
        if root == tmp_path / "archive":
            root.mkdir()
            (root / "link").symlink_to(tmp_path / "absent")
        with pytest.raises(eval_runner.Invalid, match="scratch paths"):
            eval_runner.run(
                Paths(tmp_path / "data"),
                root,
                tmp_path / "out",
                lambda: pytest.fail("must not evaluate"),
                stem="result",
                log=tmp_path / "out" / "diagnostics.log",
            )
    assert not (tmp_path / "out").exists()
