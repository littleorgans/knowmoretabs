"""Durable vectors with deterministic fake embeddings and injected publication failures."""

import json
from dataclasses import replace
from pathlib import Path
from unittest.mock import Mock, patch

import numpy as np
import pytest
from test_app import DIM, build, fake_embed, fake_encode, fake_image
from test_add import capture, NEW

from tagger.app import engine
from tagger import chunks, dataset
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
    lib = engine.sync(lib, Archive.load(root), [NEW], fake_embed)
    record = lib.records[lib.rows[NEW]]
    with Path(record["content_path"]).open("a") as stream:
        stream.write("\nchanged synthetic body")
    encode = Mock(side_effect=fake_embed)
    engine.sync(lib, Archive.load(root), [NEW], encode)
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
    lib = engine.sync(lib, Archive.load(root), [NEW], encode)
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

    def follow(lib):  # as the app does after a sync: the page refreshed, then the decisions the file holds now
        archive = Archive.load(root)
        return engine.apply_state(engine.sync(lib, archive, [NEW], encode), archive.state, fake_encode)

    hidden = follow(lib)
    assert not hidden.live[row]
    state_path.write_text(json.dumps(state | {"forgotten": []}))
    restored = follow(hidden)
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


@pytest.fixture
def chunk_library(tmp_path):
    paths, root = build(tmp_path), tmp_path / "archive"
    archive = Archive.load(root)
    _, records = dataset.app_records(paths, archive)
    visible = [r for r in records if not r["forgotten"]]
    readable = [r for r in visible if r["text_ok"]][:4]
    for record in readable:
        Path(record["content_path"]).write_bytes(b"# Stable\r\n\r\nsame\r\n\r\n# Obsolete\r\n\r\nold\r\n")
    encoder = Mock(side_effect=lambda docs: (fake_encode(docs), 0))
    snapshot = chunks.reconcile(paths, visible, encoder, dim=DIM)
    store = VectorStore(paths.emb / "eg2", "C")
    encoder.reset_mock()
    lib, stats = engine.load(paths, root, fake_encode, fake_embed, embed_chunks=encoder, dim=DIM)
    assert stats["chunks"]["embedded"] == 0
    encoder.assert_not_called()
    return lib, archive, readable, store, encoder, snapshot


def test_c_partial_sync_reconciles_full_set_removals_and_zero_work_rerun(chunk_library):
    lib, archive, readable, store, encoder, snapshot = chunk_library
    changed, textless, forgotten, unchanged = readable
    old_manifest, old_arrays = store.read()
    Path(changed["content_path"]).write_bytes(b"# Stable\r\n\r\nchanged\r\n")
    Path(textless["content_path"]).write_bytes(b"")
    state = replace(lib.state, forgotten=lib.state.forgotten | {forgotten["key"]})
    lib = engine.project(replace(lib, state=state))
    assert not lib.live[lib.rows[forgotten["key"]]], "projection hides C before durable cleanup"
    lib = engine.sync(lib, archive, [changed["key"]], fake_embed)
    encoder.assert_called_once()
    current, arrays = store.read()
    assert {row["page_id"] for row in current["rows"]} == set(snapshot.bodies) - {textless["key"], forgotten["key"]}
    updated = [r for r in current["rows"] if r["page_id"] == changed["key"]]
    assert len(updated) == len(encoder.call_args.args[0]) == 1
    assert not any(r["section"] == "Obsolete" for r in updated)
    for i, row in enumerate(current["rows"]):
        if row["page_id"] == unchanged["key"]:
            np.testing.assert_array_equal(arrays["vectors"][i], old_arrays["vectors"][old_manifest["rows"].index(row)])
    encoder.reset_mock()
    with patch.object(VectorStore, "_publish", side_effect=AssertionError("unchanged C must not republish")):
        lib = engine.sync(lib, archive, [changed["key"]], fake_embed)
    assert lib.chunks.counts["embedded"] == 0
    encoder.assert_not_called()
    # A dropped key must be cleaned up even when none of the requested A/B keys is known.
    archive = replace(archive, known=[k for k in archive.known if k != unchanged["key"]])
    lib = engine.sync(lib, archive, ["synthetic-unknown"], fake_embed)
    assert unchanged["key"] not in lib.chunks.bodies
    assert unchanged["key"] not in {r["page_id"] for r in store.read()[0]["rows"]}
    encoder.assert_not_called()


@pytest.mark.parametrize("failure", ["read", "hash", "model", "tokenizer", "encoder"])
def test_c_failure_keeps_generation_disables_offsets_and_retries(chunk_library, failure):
    lib, archive, readable, store, encoder, _ = chunk_library
    old_manifest, old_arrays = store.read()
    Path(readable[0]["content_path"]).write_bytes(b"changed synthetic body")
    original_encoder = encoder.side_effect
    target = {"read": "body_pages", "hash": "_hash"}.get(failure)
    if target:
        failing = patch.object(
            chunks.embed if failure == "read" else chunks, target, side_effect=ValueError("synthetic failure")
        )
    elif failure == "model":
        failing = patch.object(chunks.embed, "model_identity", side_effect=ValueError("synthetic model failure"))
    elif failure == "tokenizer":
        st = Mock(max_seq_length=2048)
        st.tokenizer.side_effect = ValueError("synthetic tokenizer failure")
        failing = patch.object(encoder, "side_effect", lambda docs: chunks.encode_docs(st, docs))
    else:
        failing = patch.object(encoder, "side_effect", ValueError("synthetic encoder failure"))
    try:
        with failing:
            failed = engine.sync(lib, archive, [], fake_embed)
    finally:
        encoder.side_effect = original_encoder
    assert failed.chunks is None and failed.chunk_status == "chunks_unavailable"
    hit = engine.search(failed, fake_encode(["synthetic"])[0], "synthetic", 1)[0]
    assert hit["chunk"] is None and hit["semantic_source"] == "B"
    manifest, arrays = store.read()
    assert manifest == old_manifest
    np.testing.assert_array_equal(arrays["vectors"], old_arrays["vectors"])
    encoder.reset_mock()
    retried = engine.sync(failed, archive, [], fake_embed)
    assert retried.chunks is not None and retried.chunk_status == "ready"
    encoder.assert_called_once()


def test_c_missing_is_optional_and_detected_after_explicit_preparation(tmp_path):
    paths, root = build(tmp_path), tmp_path / "archive"
    encoder = Mock(side_effect=lambda docs: (fake_encode(docs), 0))
    lib, stats = engine.load(paths, root, fake_encode, fake_embed, embed_chunks=encoder, dim=DIM)
    assert stats["chunks"] == {"code": "chunks_missing"}
    assert not (paths.emb / "eg2" / "C").exists()
    encoder.assert_not_called()
    archive = Archive.load(root)
    _, records = dataset.app_records(paths, archive, keys=[k for k in archive.known if k not in lib.state.forgotten])
    chunks.reconcile(paths, records, encoder, dim=DIM)
    encoder.reset_mock()
    refreshed = engine.sync(lib, archive, [], fake_embed)
    assert refreshed.chunks is not None and refreshed.chunk_status == "ready"
    encoder.assert_not_called()


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
