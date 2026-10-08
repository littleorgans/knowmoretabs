"""Normal startup resolution and refusal of an unprepared data directory."""

import os
from unittest.mock import patch

import pytest

from tagger import cli
from tagger.app import engine, server
from tagger.paths import Paths, resolve, resolve_root


@pytest.mark.parametrize(
    ("flag", "env", "expected"),
    [("flag", "env", "flag"), (None, "env", "env"), (None, None, "home/.knowmoretabs")],
)
def test_root_precedence(tmp_path, flag, env, expected):
    flag = str(tmp_path / flag) if flag is not None else None
    env = str(tmp_path / env) if env is not None else None
    assert resolve_root(flag, env, tmp_path / "home") == tmp_path / expected


def test_data_default_follows_resolved_root(tmp_path, monkeypatch):
    monkeypatch.delenv("KMT_TAGGER_DATA", raising=False)
    monkeypatch.setenv("KMT_ROOT", str(tmp_path / "env"))
    assert resolve(None).data == tmp_path / "env/tagger"
    assert resolve(None, tmp_path / "flag").data == tmp_path / "flag/tagger"


def test_data_environment_and_retained_flag(tmp_path, monkeypatch):
    monkeypatch.setenv("KMT_TAGGER_DATA", str(tmp_path / "data-env"))
    assert resolve(None, tmp_path / "root").data == tmp_path / "data-env"
    assert resolve(str(tmp_path / "data-flag"), tmp_path / "root").data == tmp_path / "data-flag"


@pytest.mark.parametrize("present", [False, True])
def test_missing_or_empty_data_fails_before_model_load(tmp_path, present):
    paths = Paths(tmp_path / "data")
    if present:
        paths.data.mkdir()
    with patch("tagger.embed.load_text") as load, pytest.raises(SystemExit) as error:
        server.run(paths, tmp_path / "archive", 0, False)
    assert str(error.value) == f"missing tagger vectors at {paths.data}; run tagger embed --model eg2"
    load.assert_not_called()
    assert paths.data.exists() == present
    assert not present or not list(paths.data.iterdir())


@pytest.mark.parametrize("name", engine.TEXT_INPUTS)
@pytest.mark.parametrize("legacy", [False, True])
def test_existing_text_vectors_allow_startup(tmp_path, name, legacy):
    paths = Paths(tmp_path)
    store = engine.VectorStore(paths.emb / engine.MODEL, name)
    cache = store.legacy if legacy else store.folder / "manifest.json"
    cache.parent.mkdir(parents=True)
    cache.touch()
    engine.require_vectors(paths)


@pytest.mark.parametrize("position", ["before", "after", "env", "default"])
def test_cli_passes_resolved_root_and_default_data(tmp_path, monkeypatch, position):
    monkeypatch.delenv("KMT_TAGGER_DATA", raising=False)
    monkeypatch.setenv("KMT_ROOT", str(tmp_path / "env"))
    root = tmp_path / "flag"
    argv = ["tagger", "app", "--smoke"]
    if position == "before":
        argv[1:1] = ["--root", str(root)]
    elif position == "after":
        argv += ["--root", str(root)]
    elif position == "env":
        root = tmp_path / "env"
    else:
        monkeypatch.delenv("KMT_ROOT")
        root = tmp_path / "home/.knowmoretabs"
    old_umask = os.umask(0o077)
    try:
        with (
            patch("sys.argv", argv),
            patch("pathlib.Path.home", return_value=tmp_path / "home"),
            patch("tagger.app.server.run") as run,
            patch.dict(os.environ),
        ):
            cli.main()
        run.assert_called_once_with(Paths(root / "tagger"), root, cli.APP_PORT, True)
    finally:
        os.umask(old_umask)
