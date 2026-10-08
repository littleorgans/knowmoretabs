"""Dataset independent startup, lazy migrations and snapshot experiment isolation."""

import json
import os
import subprocess
from dataclasses import replace
from pathlib import Path
from unittest.mock import Mock, patch

import numpy as np
import pytest
import synthetic_archive
from test_app import DIM, build, fake_embed, fake_encode, fake_image

from tagger import cli, dataset, embed
from tagger.app import engine
from tagger.paths import Paths, resolve, resolve_root
from tagger.vector_store import VectorStore


@pytest.mark.parametrize("kind", ["root", "data"])
def test_empty_environment_is_unset(tmp_path, monkeypatch, kind):
    if kind == "root":
        assert resolve_root(None, "", tmp_path) == tmp_path / ".knowmoretabs"
    else:
        monkeypatch.setenv("KMT_TAGGER_DATA", "")
        assert resolve(None, tmp_path).data == tmp_path / "tagger"


def test_app_manifest_startup_does_not_read_dataset_or_legacy_bodies(tmp_path):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    engine.load(paths, root, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)
    paths.dataset.rename(tmp_path / "unused-dataset")
    legacy = Mock(side_effect=AssertionError("manifest startup must not compute legacy rows"))
    with (
        patch.object(dataset, "load", side_effect=AssertionError("dataset must not be read")),
        patch.object(dataset, "load_records", side_effect=AssertionError("dataset pages must not be opened")),
        patch.object(VectorStore, "_migrate", legacy),
    ):
        lib, stats = engine.load(paths, root, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)
    assert len(lib.records) > 0 and stats["vectors"]["embedded"] == 0
    legacy.assert_not_called()


def test_legacy_provider_is_called_only_when_migrating(tmp_path):
    store = VectorStore(tmp_path, "B")
    model, rows = {"dim": 2}, [{"id": "synthetic", "hash": "current"}]
    np.save(store.legacy, np.ones((1, 2), np.float32))
    store.legacy.with_suffix(".json").write_text(json.dumps(model))
    legacy = Mock(return_value=rows)
    store.reconcile(rows, model, Mock(side_effect=AssertionError("no embeddings")), legacy_rows=legacy)
    legacy.assert_called_once_with()
    legacy.reset_mock()
    store.reconcile(rows, model, Mock(side_effect=AssertionError("no embeddings")), legacy_rows=legacy)
    legacy.assert_not_called()


@pytest.mark.parametrize("images", [False, True])
def test_manifest_persistence_does_not_compute_legacy_rows(tmp_path, images):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    engine.load(paths, root, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)
    records = [dataset.record(embed.Archive.load(root), key) for key in embed.Archive.load(root).known]
    legacy = Mock(side_effect=AssertionError("manifest must not open historical inputs"))
    if images:
        _, counts = embed.persist_images(paths, records, fake_image, legacy_records=legacy, dim=DIM)
    else:
        _, counts = embed.persist_text(
            paths, records, embed.MODELS["eg2"], "B", fake_embed, legacy_records=legacy, dim=DIM
        )
    assert counts["embedded"] == 0
    legacy.assert_not_called()


@pytest.fixture
def encoders():
    with (
        patch.dict(embed.MODELS, {"eg2": replace(embed.MODELS["eg2"], dim=DIM)}),
        patch.object(embed, "IMAGE_MODEL", replace(embed.IMAGE_MODEL, dim=DIM)),
        patch.object(embed, "load_text", return_value=(object(), {})),
        patch.object(embed, "load", return_value=(object(), {})),
        patch.object(embed, "embed_input", side_effect=lambda st, model, rs, name: (fake_embed(rs, name), {})),
        patch.object(embed, "embed_image_rows", side_effect=lambda st, rs: (fake_image(rs), {})),
        patch.object(embed, "release"),
    ):
        yield


def test_bare_embed_bootstraps_app_without_dataset(tmp_path, monkeypatch, encoders, capsys):
    root, paths = tmp_path / "archive", Paths(tmp_path / "data")
    synthetic_archive.build(root, paths.data)
    (paths.data / "app").mkdir()
    monkeypatch.setenv("KMT_ROOT", str(root))
    monkeypatch.setenv("KMT_TAGGER_DATA", str(paths.data))
    monkeypatch.setattr("sys.argv", ["tagger", "embed", "--model", "eg2"])
    old_umask = os.umask(0o077)
    try:
        cli.main()
        capsys.readouterr()
        assert not paths.dataset.exists()
        lib, stats = engine.load(paths, root, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)
        assert len(lib.records) > 0 and stats["vectors"]["embedded"] == 0
        with patch.object(dataset, "load", side_effect=AssertionError("no dataset reads")):
            cli.main()
        counts = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
        assert counts and all(c["embedded"] == 0 for c in counts)
    finally:
        os.umask(old_umask)


@pytest.mark.parametrize("value", [None, ""])
def test_pipeline_refuses_implicit_data(tmp_path, value):
    script = Path(__file__).parents[1] / "run.sh"
    env = dict(os.environ)
    env.pop("KMT_TAGGER_DATA", None)
    if value is not None:
        env["KMT_TAGGER_DATA"] = value
    env["PATH"] = str(tmp_path)  # refusal must precede invoking uv
    result = subprocess.run(["/bin/sh", str(script)], env=env, capture_output=True, text=True)
    assert result.returncode == 1
    assert result.stderr.strip() == "run.sh requires KMT_TAGGER_DATA"
    assert not result.stdout


def test_pipeline_embeds_explicit_snapshot(tmp_path):
    script = Path(__file__).parents[1] / "run.sh"
    fake = tmp_path / "uv"
    # Record arguments only. The real model selector reads a synthetic CV pick.
    fake.write_text(
        "#!/usr/bin/env python3\n"
        "import json,os,sys\n"
        "with open(os.environ['CALL_LOG'],'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\n"
        "if 'python' in sys.argv:\n"
        " print(os.environ['KMT_TAGGER_SNAPSHOT'] if 'snapshot' in sys.argv[-1] else 'eg2')\n"
    )
    fake.chmod(0o700)
    log = tmp_path / "calls.jsonl"
    snapshot = tmp_path / "snapshot with spaces"
    env = dict(os.environ)
    env.update(KMT_TAGGER_DATA=str(tmp_path / "data"), KMT_TAGGER_SNAPSHOT=str(snapshot), CALL_LOG=str(log))
    env["PATH"] = str(tmp_path) + os.pathsep + env["PATH"]
    result = subprocess.run(["/bin/sh", str(script)], env=env, capture_output=True, text=True)
    assert result.returncode == 0
    calls = [json.loads(line) for line in log.read_text().splitlines()]
    embeds = [call for call in calls if "embed" in call]
    assert len(embeds) == 2
    for call in embeds:
        assert call[call.index("--root") + 1] == str(snapshot)
