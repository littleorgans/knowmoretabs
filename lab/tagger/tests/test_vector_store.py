"""Durable vectors with deterministic fake embeddings and injected publication failures."""

import json
from pathlib import Path
from unittest.mock import Mock, patch

import numpy as np
import pytest
from test_app import DIM, build, fake_embed, fake_encode, fake_image
from test_add import capture, NEW

from tagger.app import engine
from tagger.archive import Archive
from tagger.vector_store import VectorStore

MODEL = {"model": "synthetic", "prompt": "{text}", "dim": 2, "max_tokens": 20}


def rows(*pairs):
    return [{"id": key, "hash": value} for key, value in pairs]


def encoder(indices):
    return np.array([[i + 1, 10] for i in indices], np.float32)


def test_append_reorder_freshness_and_second_startup(tmp_path):
    store = VectorStore(tmp_path, "B")
    a = rows(("first", "a"), ("second", "b"))
    original, _ = store.reconcile(a, MODEL, encoder)
    encode = Mock(side_effect=encoder)
    wanted = rows(("second", "b"), ("first", "a"), ("third", "c"))
    result, counts = store.reconcile(wanted, MODEL, encode)
    encode.assert_called_once_with([2])
    assert counts == dict(reused=2, embedded=1, stale=0, dropped=0, migrated=0, cold_migration=0, failed=0)
    np.testing.assert_array_equal(result["vectors"][:2], original["vectors"][[1, 0]])
    encode.reset_mock()
    store.reconcile(wanted, MODEL, encode)
    encode.assert_not_called()
    with patch.object(store, "_publish", side_effect=AssertionError("unchanged rows must not republish")):
        reordered, _ = store.reconcile(list(reversed(wanted)), MODEL, encode)
    np.testing.assert_array_equal(reordered["vectors"], result["vectors"][::-1])
    wanted[1]["hash"] = "changed"
    _, counts = store.reconcile(wanted, MODEL, encode)
    encode.assert_called_once_with([1])
    assert (counts["embedded"], counts["stale"]) == (1, 1)


@pytest.mark.parametrize("phase", ["after_arrays", "mid_manifest"])
def test_crash_keeps_old_arrays_and_image_mask(tmp_path, phase):
    store = VectorStore(tmp_path, "image")
    old_rows = rows(("first", "a"), ("second", "b"))
    store.reconcile(old_rows, MODEL, encoder, mask=[True, False])
    old_manifest, old_arrays = store.read()
    stage = store._stage

    def fail(filename, write):
        if filename == "manifest.json":
            if phase == "mid_manifest":

                def broken(stream):
                    stream.write(b'{"version":')
                    raise OSError("simulated crash")

                return stage(filename, broken)
            raise OSError("simulated crash")
        return stage(filename, write)

    with patch.object(store, "_stage", side_effect=fail), pytest.raises(OSError):
        store.reconcile(rows(("second", "changed"), ("third", "c")), MODEL, encoder, mask=[True, True])
    manifest, arrays = store.read()
    assert manifest == old_manifest
    for key in arrays:
        np.testing.assert_array_equal(arrays[key], old_arrays[key])
    store.reconcile(old_rows + rows(("third", "c")), MODEL, encoder, mask=[True, False, True])
    assert set(p.name for p in store.folder.iterdir()) == {"manifest.json", "lock", *store.read()[0]["arrays"].values()}


@pytest.mark.parametrize("mismatch", [False, True])
def test_legacy_migration(tmp_path, mismatch):
    store = VectorStore(tmp_path, "B")
    legacy = np.array([[1, 3], [2, 4]], np.float32)
    np.save(store.legacy, legacy)
    store.legacy.with_suffix(".json").write_text(json.dumps(MODEL))
    wanted = rows(("first", "a")) if mismatch else rows(("first", "a"), ("second", "b"))
    encode = Mock(side_effect=encoder)
    arrays, counts = store.reconcile(wanted, MODEL, encode)
    assert counts["embedded"] == (1 if mismatch else 0)
    assert counts["migrated"] == (0 if mismatch else 2)
    assert counts["cold_migration"] == int(mismatch)
    if not mismatch:
        encode.assert_not_called()
        np.testing.assert_array_equal(arrays["vectors"], legacy)
    np.testing.assert_array_equal(np.load(store.legacy), legacy)


def test_model_change_and_partial_sync(tmp_path):
    store = VectorStore(tmp_path, "B")
    wanted = rows(("first", "a"), ("second", "b"))
    store.reconcile(wanted, MODEL, encoder)
    encode = Mock(side_effect=encoder)
    store.reconcile(wanted[:1], MODEL, encode, retain=True)
    encode.assert_not_called()
    assert {r["id"] for r in store.read()[0]["rows"]} == {"first", "second"}
    store.reconcile(rows(("first", "changed")), MODEL, encode, retain=True)
    assert {r["id"] for r in store.read()[0]["rows"]} == {"first", "second"}
    encode.reset_mock()
    _, counts = store.reconcile(wanted, MODEL | {"prompt": "changed"}, encode)
    assert (counts["embedded"], counts["stale"]) == (2, 2)
    encode.assert_called_once_with([0, 1])
    _, counts = store.reconcile(wanted[:1], MODEL | {"prompt": "changed"}, encode)
    assert counts["dropped"] == 1


def test_sync_changed_content_reembeds(tmp_path):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    lib, _ = engine.load(paths, root, fake_encode, fake_embed, embed_image=fake_image, dim=DIM)
    capture(root, NEW)
    lib = engine.sync(lib, Archive.load(root), NEW, fake_embed)
    record = lib.records[lib.rows[NEW]]
    with Path(record["content_path"]).open("a") as stream:
        stream.write("\nchanged synthetic body")
    encode = Mock(side_effect=fake_embed)
    engine.sync(lib, Archive.load(root), NEW, encode)
    encode.assert_called_once()


def test_engine_restart_add_images_forget_restore_and_reorder(tmp_path):
    paths = build(tmp_path)
    root = tmp_path / "archive"
    encode = Mock(side_effect=fake_embed)
    images = Mock(side_effect=fake_image)
    lib, stats = engine.load(paths, root, fake_encode, encode, embed_image=images, dim=DIM)
    assert stats["vectors"]["migrated"] == 2 * len(lib.records)
    encode.assert_not_called()
    images.assert_not_called()
    capture(root, NEW)
    lib = engine.sync(lib, Archive.load(root), NEW, encode)
    images.assert_called_once()
    encode.assert_called_once()
    row = lib.rows[NEW]
    assert lib.has_image[row]
    vectors = lib.X[row].copy()
    image = lib.image[row].copy()
    state_path = root / "library.json"
    state = json.loads(state_path.read_text())
    state_path.write_text(json.dumps(state | {"forgotten": [NEW]}))
    encode.reset_mock()
    images.reset_mock()
    hidden = engine.sync(lib, Archive.load(root), NEW, encode)
    assert not hidden.live[row]
    state_path.write_text(json.dumps(state | {"forgotten": []}))
    restored = engine.sync(hidden, Archive.load(root), NEW, encode)
    assert restored.live[row]
    encode.assert_not_called()
    images.assert_not_called()
    np.testing.assert_array_equal(restored.X[row], vectors)
    np.testing.assert_array_equal(restored.image[row], image)
    # Rebuilding the dataset in another order cannot move a vector to another id.
    dataset_path = paths.dataset / "pages.jsonl"
    dataset_path.write_text("\n".join(reversed(dataset_path.read_text().splitlines())) + "\n")
    restarted, stats = engine.load(paths, root, fake_encode, encode, embed_image=images, dim=DIM)
    assert stats["vectors"]["embedded"] == 0
    encode.assert_not_called()
    images.assert_not_called()
    for key, old_row in restored.rows.items():
        new_row = restarted.rows[key]
        np.testing.assert_array_equal(restarted.X[new_row], restored.X[old_row])
        np.testing.assert_array_equal(restarted.image[new_row], restored.image[old_row])
        assert restarted.has_image[new_row] == restored.has_image[old_row]


def test_cli_embed_refreshes_hashes_without_loading_unchanged_models(tmp_path):
    from tagger import dataset, embed
    from tagger.paths import Paths

    paths = Paths(tmp_path)
    records = [{"key": "synthetic", "title": "before", "a_text": "words", "text_ok": False}]
    fake = Mock(side_effect=lambda st, docs, batch: (np.ones((len(docs), 2), np.float32), {}))
    model = embed.Model("fake", "synthetic", "{title} {text}", dim=2)
    with (
        patch.object(embed, "load_text", return_value=(object(), {})) as load,
        patch.object(embed, "encode", fake),
        patch.object(embed, "token_stats", return_value={}),
        patch.object(embed, "release"),
        patch.object(dataset, "load", return_value=(records, [])),
    ):
        embed.embed_text(paths, records, model, False)
        assert fake.call_count == 2
        fake.reset_mock()
        load.reset_mock()
        embed.embed_text(paths, records, model, False)
        fake.assert_not_called()
        load.assert_not_called()
        records[0]["title"] = "after"
        embed.embed_text(paths, records, model, False)
        assert fake.call_count == 2
        for name in ("A", "B"):
            manifest, _ = VectorStore(paths.emb / "fake", name).read()
            assert manifest["rows"] == embed.input_rows(records, model, name)
        extra = [{**records[0], "key": "added-in-app"}]
        for name in ("A", "B"):
            embed.persist_text(paths, extra, model, name, lambda rs, _: encoder([0]), retain=True)
        fake.reset_mock()
        embed.embed_text(paths, records, model, False)
        fake.assert_not_called()
        for name in ("A", "B"):
            manifest, _ = VectorStore(paths.emb / "fake", name).read()
            assert {row["id"] for row in manifest["rows"]} == {"synthetic", "added-in-app"}


def test_text_and_image_hash_the_frozen_input(tmp_path):
    from tagger import embed
    from tagger.paths import Paths

    body = tmp_path / "body.md"
    body.write_text("initial body")
    picture = tmp_path / "image.bin"
    picture.write_bytes(b"initial image")
    record = {
        "key": "synthetic",
        "title": "title",
        "a_text": "words",
        "text_ok": True,
        "content_path": str(body),
        "image_ok": True,
        "image_path": str(picture),
    }
    paths = Paths(tmp_path / "data")
    model = embed.Model("fake", "synthetic", "{title} {text}", dim=2)
    expected_text = embed.input_rows([record], model, "B")
    expected_image = embed.image_rows([record])

    def text_encoder(records, name):
        body.write_text("changed while embedding")
        assert embed.input_rows(records, model, name) == expected_text
        return encoder([0])

    def image_encoder(records):
        picture.write_bytes(b"changed while embedding")
        assert embed.image_rows(records) == expected_image
        return encoder([0])

    embed.persist_text(paths, [record], model, "B", text_encoder)
    embed.persist_images(paths, [record], image_encoder, dim=2)
    assert VectorStore(paths.emb / "fake", "B").read()[0]["rows"] == expected_text
    assert VectorStore(paths.emb / "image", "eg2-full").read()[0]["rows"] == expected_image


def test_concurrent_writers_reconcile_under_lock(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    from threading import Event

    first_in_embed, release_first, second_in_embed = Event(), Event(), Event()
    store = VectorStore(tmp_path, "B")

    def first_encoder(indices):
        first_in_embed.set()
        assert release_first.wait(5)
        return encoder(indices)

    def second_encoder(indices):
        second_in_embed.set()
        return encoder(indices)

    with ThreadPoolExecutor(2) as pool:
        first = pool.submit(store.reconcile, rows(("first", "a")), MODEL, first_encoder, retain=True)
        assert first_in_embed.wait(5)
        second = pool.submit(store.reconcile, rows(("second", "b")), MODEL, second_encoder, retain=True)
        try:
            assert not second_in_embed.wait(0.2)
        finally:
            release_first.set()
        first.result()
        second.result()
    manifest, arrays = store.read()
    assert {row["id"] for row in manifest["rows"]} == {"first", "second"}
    assert arrays["vectors"].shape == (2, 2)


def test_cli_images_preserve_added_rows_and_refresh_changed_bytes(tmp_path):
    from tagger import embed
    from tagger.paths import Paths

    paths = Paths(tmp_path / "data")
    picture = tmp_path / "image.bin"
    picture.write_bytes(b"initial synthetic image")
    records = [{"key": "synthetic", "image_ok": True, "image_path": str(picture)}]
    model = embed.Model("eg2-full", "synthetic", "image", dim=2)
    with (
        patch.object(embed, "IMAGE_MODEL", model),
        patch.object(embed, "load", return_value=(object(), {})) as load,
        patch.object(embed, "embed_image_rows", return_value=(encoder([0]), {})) as encode,
        patch.object(embed, "release"),
    ):
        embed.embed_images(paths, records)
        encode.assert_called_once()
        extra = [{**records[0], "key": "added-in-app"}]
        embed.persist_images(paths, extra, lambda rs: encoder([0]), retain=True)
        encode.reset_mock()
        load.reset_mock()
        embed.embed_images(paths, records)
        encode.assert_not_called()
        load.assert_not_called()
        store = VectorStore(paths.emb / "image", model.key)
        assert {row["id"] for row in store.read()[0]["rows"]} == {"synthetic", "added-in-app"}
        picture.write_bytes(b"changed synthetic image")
        embed.embed_images(paths, records)
        encode.assert_called_once()
        manifest, arrays = store.read()
        assert next(row for row in manifest["rows"] if row["id"] == "synthetic") == embed.image_rows(records)[0]
        assert arrays["mask"].all()
