"""Review regressions: common inputs, one app variant, one log, optional images."""

import json
from dataclasses import replace
from pathlib import Path
from unittest.mock import Mock, patch

import numpy as np
from test_add import NEW, capture
from test_app import DIM, build, fake_embed, fake_encode, fake_image

from tagger import cli, dataset, embed
from tagger.app import engine, server
from tagger.archive import Archive
from tagger.vector_store import VectorStore


def test_embed_and_startup_reach_a_fixed_point(tmp_path):
    """The reviewer's snapshot/live body divergence through the actual CLI dispatch."""
    paths = build(tmp_path)
    root = tmp_path / "archive"
    records, _ = dataset.load(paths)
    row = next(r for r in records if r["text_ok"])
    snapshot = tmp_path / "snapshot-body.md"
    snapshot.write_text(Path(row["content_path"]).read_text() + "\nsnapshot era synthetic line")
    row["content_path"] = str(snapshot)
    (paths.dataset / "pages.jsonl").write_text("".join(json.dumps(r) + "\n" for r in records))
    capture(root, NEW)
    calls = Mock(side_effect=lambda st, model, records, name: (fake_embed(records, name), {}))
    model = replace(embed.MODELS["eg2"], dim=DIM)
    with (
        patch.dict(embed.MODELS, {"eg2": model}),
        patch.object(embed, "IMAGE_MODEL", replace(embed.IMAGE_MODEL, dim=DIM)),
        patch.object(embed, "load_text", return_value=(object(), {})),
        patch.object(embed, "load", return_value=(object(), {})),
        patch.object(embed, "embed_input", calls),
        patch.object(embed, "embed_image_rows", side_effect=lambda st, rs: (fake_image(rs), {})),
        patch.object(embed, "release"),
        patch("sys.argv", ["tagger", "--data", str(paths.data), "embed", "--model", "eg2", "--root", str(root)]),
    ):
        cli.main()
        # The first settle must refresh the divergent B input and index the gap.
        assert any(row["key"] == r["key"] for call in calls.call_args_list for r in call.args[2])
        calls.reset_mock()
        encode = Mock(side_effect=fake_embed)
        images = Mock(side_effect=fake_image)
        lib, stats = engine.load(paths, root, fake_encode, encode, embed_image=images, dim=DIM)
        assert NEW in lib.rows
        assert stats["vectors"]["embedded"] == 0
        encode.assert_not_called()
        images.assert_not_called()
        cli.main()
        calls.assert_not_called()
        _, stats = engine.load(paths, root, fake_encode, encode, embed_image=images, dim=DIM)
        assert stats["vectors"]["embedded"] == 0
        for name in ("A", "B"):
            manifest, _ = VectorStore(paths.emb / "eg2", name).read()
            current = [dataset.record(Archive.load(root), r["id"]) for r in manifest["rows"]]
            assert manifest["rows"] == embed.input_rows(current, model, name)


def test_app_maintains_only_the_variant_it_reads(tmp_path):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    for suffix in (".npy", ".json"):
        target = paths.emb / "eg2" / ("A" + suffix)
        target.write_bytes((paths.emb / "eg2" / ("B" + suffix)).read_bytes())
    encode = Mock(side_effect=fake_embed)
    lib, _ = engine.load(paths, root, fake_encode, encode, embed_image=fake_image, dim=DIM)
    assert lib.input_name == "B"
    assert not (paths.emb / "eg2/A/manifest.json").exists()
    capture(root, NEW)
    lib = engine.sync(lib, Archive.load(root), NEW, encode)
    assert encode.call_count == 1
    assert encode.call_args.args[1] == "B"
    assert lib.has_image[lib.rows[NEW]]
    assert not (paths.emb / "eg2/A/manifest.json").exists()


def test_one_counts_line_per_startup(tmp_path, capsys):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    capsys.readouterr()
    with (
        patch.dict(embed.MODELS, {"eg2": replace(embed.MODELS["eg2"], dim=DIM)}),
        patch.object(embed, "load_text", return_value=(object(), {"load_s": 0})),
        patch("tagger.zeroshot.encode_queries", side_effect=lambda st, texts: fake_encode(texts)),
        patch.object(server, "smoke", return_value={"searches": 0}),
    ):
        server.run(paths, root, 0, True)
    lines = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert sum("vectors" in line for line in lines) == 1


def test_image_failure_preserves_text_and_cached_images(tmp_path, capsys):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    baseline, _ = engine.load(paths, root, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)
    capture(root, NEW)
    images = Mock(side_effect=RuntimeError("synthetic image failure"))
    encode = Mock(side_effect=fake_embed)
    lib, stats = engine.load(paths, root, fake_encode, encode, embed_image=images, dim=DIM)
    row = lib.rows[NEW]
    assert stats["vectors"]["failed"] == 1
    assert lib.live[row] and not lib.has_image[row]
    assert not lib.image[row].any()
    kept = [lib.rows[key] for key in baseline.rows]
    np.testing.assert_array_equal(lib.image[kept], baseline.image)
    np.testing.assert_array_equal(lib.has_image[kept], baseline.has_image)
    assert NEW not in {r["id"] for r in VectorStore(paths.emb / "image", "eg2-full").read()[0]["rows"]}
    encode.reset_mock()
    _, stats = engine.load(paths, root, fake_encode, encode, embed_image=images, dim=DIM)
    assert stats["vectors"]["embedded"] == 0
    encode.assert_not_called()
    # Add another page while the image model remains unavailable.
    other = NEW + "-second"
    capture(root, other)
    capsys.readouterr()
    lib = engine.sync(lib, Archive.load(root), other, encode)
    assert lib.live[lib.rows[other]] and not lib.has_image[lib.rows[other]]
    assert json.loads(capsys.readouterr().out)["image_failed"] == 1
    assert other not in {r["id"] for r in VectorStore(paths.emb / "image", "eg2-full").read()[0]["rows"]}
    # A later successful start fills the absent image rows without redoing text.
    encode.reset_mock()
    recovered, stats = engine.load(paths, root, fake_encode, encode, embed_image=fake_image, dim=DIM)
    encode.assert_not_called()
    assert stats["vectors"]["embedded"] == 2
    assert recovered.has_image[recovered.rows[NEW]] and recovered.has_image[recovered.rows[other]]


def test_failed_stale_image_row_is_absent_and_retry_does_not_publish_again(tmp_path):
    model = {"model": "synthetic", "dim": 2}
    rows = [{"id": "first", "hash": "before"}, {"id": "second", "hash": "stable"}]
    store = VectorStore(tmp_path, "image")
    vectors = np.array([[1, 2], [3, 4]], np.float32)
    store.reconcile(rows, model, lambda indices: vectors[indices], mask=[True, True])
    rows[0]["hash"] = "after"
    failure = Mock(side_effect=RuntimeError("synthetic image failure"))
    output, counts = store.reconcile(rows, model, failure, mask=[True, True], allow_missing=True)
    assert counts["failed"] == 1 and counts["embedded"] == 0
    assert output["mask"].tolist() == [False, True]
    np.testing.assert_array_equal(output["vectors"][1], vectors[1])
    assert store.read()[0]["rows"] == rows[1:]
    with patch.object(store, "_publish", side_effect=AssertionError("failed retry must not republish")):
        store.reconcile(rows, model, failure, mask=[True, True], allow_missing=True)
