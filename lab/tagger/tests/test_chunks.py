"""Synthetic reader bodies and scratch Store C publications, without model weights."""

import json
from types import SimpleNamespace
from unittest.mock import Mock, patch

import numpy as np
import pytest
from test_sources import LOOSE, NOTE, UNPINNED, WEB, archive

from tagger import chunks, dataset, embed
from tagger.app import kmt
from tagger.archive import Archive, sha256_hex
from tagger.embed import encode as encode_documents
from tagger.paths import Paths
from tagger.vector_store import VectorStore


def test_reference_roundtrip():
    intro = "\r\nBefore café 🦊 e\u0301 �.\r\n\r\n"
    first = (
        "# First **section** ###\r\n\r\n"
        "- parent\r\n  + nested\r\n\r\n  continuation\r\n1) next\r\nlazy continuation\r\n"
        "```python\r\n\r\n# inside fence\r\n- code marker\r\n``\r\n````\r\n\r\n"
        "    # indented code\r\n\r\n    - still code\r\n\r\n"
        "<table>literal</table>\r\n\r\n"
    )
    second = "Second section\r\n===\r\n\r\nOrdinary paragraph.\r\n\r\n~~~\r\n## unclosed code\r\n"
    body = intro + first + second + "# Final"
    found = chunks.blocks(body)
    assert found[0].start == 0 and found[-1].end == len(body)
    assert all(a.end == b.start for a, b in zip(found, found[1:], strict=False))
    assert [block.kind for block in found] == [
        "paragraph",
        "heading",
        "list",
        "code",
        "code",
        "paragraph",
        "heading",
        "paragraph",
        "code",
    ]
    listed = next(block for block in found if block.kind == "list")
    assert body[listed.start : listed.end].endswith("lazy continuation\r\n")
    rows = chunks.chunk_page("synthetic", "Reader title", body)
    raw = body.encode("utf-8")
    assert [raw[row["start"] : row["end"]].decode("utf-8") for row in rows] == [intro, first, second + "# Final"]
    assert [row["section"] for row in rows] == ["", "First **section**", "Second section"]
    assert rows[0]["start"] == 0 and rows[-1]["end"] == len(raw)
    assert all(a["end"] == b["start"] for a, b in zip(rows, rows[1:], strict=False))
    assert all(set(row) == {"id", "page_id", "start", "end", "section", "hash", "page_hash"} for row in rows)
    final = chunks.chunk_page("synthetic", "", "# Same\n# Same\n# Last")
    assert [row["section"] for row in final] == ["Same", "Same", "Last"]


def test_content_hash():
    body = "# Context\n\ncafé <|image|> body\n"
    original = chunks.chunk_page("page", "Title", body)[0]
    assert chunks.chunk_page("page", "Title", body)[0] == original
    assert original["hash"] == sha256_hex(
        json.dumps(("Title", "Context", body), ensure_ascii=False, separators=(",", ":"))
    )
    assert original["id"] == sha256_hex(json.dumps(("page", 0, len(body.encode())), separators=(",", ":")))
    for title, changed in [
        ("Title", body + "edit"),
        ("New title", body),
        ("Title", body.replace("Context", "Changed")),
    ]:
        row = chunks.chunk_page("page", title, changed)[0]
        assert row["hash"] != original["hash"] and row["page_hash"] != original["page_hash"]
    moved = chunks.chunk_page("another page", "Title", "Earlier words.\n\n" + body)[1]
    assert moved["start"] > original["start"] and moved["id"] != original["id"]
    assert moved["hash"] == original["hash"]
    same_text_elsewhere = chunks.chunk_page("page", "Title", body + body)
    assert len({row["id"] for row in same_text_elsewhere}) == 2
    assert {row["hash"] for row in same_text_elsewhere} == {original["hash"]}
    long = "same paragraph words " * 200
    alpha = chunks.chunk_page("page", "Title", "# Alpha\n\n" + long)[1]
    bravo = chunks.chunk_page("page", "Title", "# Bravo\n\n" + long)[1]
    assert (alpha["start"], alpha["end"]) == (bravo["start"], bravo["end"])
    assert alpha["hash"] != bravo["hash"], "section context changes the hash of otherwise identical text"
    assert chunks.chunk_page("page", "", body)[0]["hash"] == chunks.chunk_page("page", "none", body)[0]["hash"]


def test_size_policy():
    paragraph = "ordinary words " * 65 + "\n\n"
    packed_body = paragraph * 3
    packed = chunks.chunk_page("page", "Title", packed_body)
    assert len(packed) == 2
    assert packed[0]["end"] == len((paragraph * 2).encode())
    assert packed[1]["start"] == packed[0]["end"]
    assert (packed[0]["end"] - packed[0]["start"] + 3) // 4 >= chunks.MIN_TOKENS
    long = "paragraph café 🦊 words " * 260 + "\n\n"
    before, after = "# Long\n\n", "## Tail\n\nshort\n"
    body = before + long + after
    rows = chunks.chunk_page("page", "Title", body)
    raw = body.encode()
    assert rows[0]["end"] == len(before.encode())
    assert rows[-1]["start"] == len((before + long).encode())
    for row in rows:
        assert (row["end"] - row["start"] + 3) // 4 <= chunks.MAX_TOKENS
        raw[row["start"] : row["end"]].decode("utf-8")
    for left, right in zip(rows[1:-2], rows[2:-1], strict=True):
        assert left["start"] < right["start"] < left["end"]
        assert left["end"] - right["start"] <= chunks.OVERLAP_TOKENS * chunks.BYTES_PER_TOKEN
    assert rows[0]["end"] == rows[1]["start"] and rows[-2]["end"] == rows[-1]["start"]
    unicode_body = "🦊" * 1900
    unicode_rows = chunks.chunk_page("page", "Title", unicode_body)
    assert len(unicode_rows) > 1 and unicode_rows[-1]["end"] == len(unicode_body.encode())
    for row in unicode_rows:
        text = unicode_body.encode()[row["start"] : row["end"]].decode()
        assert 0 < len(text.encode()) <= chunks.MAX_TOKENS * chunks.BYTES_PER_TOKEN
    assert all(a["start"] < b["start"] for a, b in zip(unicode_rows, unicode_rows[1:], strict=False))
    listing = "- " + "list words " * 250 + "\n  - nested\n\n"
    code = "~~~python\n" + "# code words\n" * 250 + "~~~\n"
    oversized = chunks.chunk_page("page", "Title", listing + code)
    assert [((listing + code).encode()[row["start"] : row["end"]]).decode() for row in oversized] == [listing, code]
    assert all(row["end"] - row["start"] > chunks.MAX_TOKENS * chunks.BYTES_PER_TOKEN for row in oversized)


@pytest.fixture
def chunk_run(tmp_path, monkeypatch):
    root, paths = tmp_path / "archive", Paths(tmp_path / "data")
    archive(root)
    # The kept web page belongs only to a snapshot, outside the CLI reader's added page scope.
    added = root / "pages/added.jsonl"
    added.write_text("".join(line + "\n" for line in added.read_text().splitlines() if json.loads(line)["url"] != WEB))
    snapshot = root / "snapshots/synthetic"
    snapshot.mkdir()
    (snapshot / "snapshot.json").write_text(
        json.dumps({"schema_version": 1, "tabs": [{"url": WEB, "title": "Snapshot title"}]})
    )
    content = root / "pages/content" / (sha256_hex(WEB) + ".md")
    content.parent.mkdir()
    content.write_bytes(b"---\nkept: synthetic\n---\n# Kept\r\n\r\nbody caf\xc3\xa9\r\n")
    (root / "pages/content.jsonl").write_text(
        json.dumps({"url": WEB, "status": "ok", "attempted_at": "2026-01-01T00:00:00Z"}) + "\n"
    )
    bodies = {
        NOTE: "# First\n\nUnicode separators: \u2028 \u2029 \u0085\n\n"
        + "synthetic paragraph " * 250
        + "\n\n# Stable\n\nUnchanged sibling.\n\n# Obsolete\n\nold\n",
        LOOSE: "document body\n",
    }
    titles = {NOTE: "Reader title"}

    def reader(argv, **kwargs):
        keys = argv[argv.index("--") + 1 :]
        assert WEB not in keys, "kept web bodies must bypass the added page reader"
        answers = [
            {"schema_version": 1, "url": key, "text": bodies[key], "title": titles.get(key)}
            if key in bodies
            else {"schema_version": 1, "url": key, "error": {"kind": "source_read"}}
            for key in keys
        ]
        return SimpleNamespace(
            returncode=int(UNPINNED in keys),
            stdout="\n".join(json.dumps(answer, ensure_ascii=False) for answer in answers).encode(),
        )

    reader = Mock(side_effect=reader)
    st = SimpleNamespace(
        max_seq_length=embed.MAX_TOKENS,
        tokenizer=Mock(side_effect=lambda docs, **kwargs: {"input_ids": [list(range(len(doc))) for doc in docs]}),
    )
    load = Mock(return_value=(st, {}))
    encode = Mock(
        side_effect=lambda st, docs, batch, **kwargs: (
            np.stack([np.full(embed.MODELS["eg2"].dim, len(doc), np.float32) for doc in docs]),
            {},
        )
    )
    release = Mock()
    monkeypatch.setattr(kmt.subprocess, "run", reader)
    monkeypatch.setattr(kmt, "READ_BATCH_SIZE", 2)
    monkeypatch.setattr(embed, "load_text", load)
    monkeypatch.setattr(embed, "encode", encode)
    monkeypatch.setattr(embed, "release", release)
    return SimpleNamespace(
        root=root,
        paths=paths,
        bodies=bodies,
        titles=titles,
        content=content,
        st=st,
        load=load,
        encode=encode,
        release=release,
        reader=reader,
        store=VectorStore(paths.emb / "eg2", "C"),
    )


def test_chunk_batches_release_memory(chunk_run, monkeypatch):
    run = chunk_run
    run.bodies[NOTE] = ("synthetic paragraph " * 60 + "\n\n") * 19
    cached, encoded = [], []

    def encode(docs, *, batch_size, **kwargs):
        for start in range(0, len(docs), batch_size):
            if cached:
                raise RuntimeError("synthetic retained batch memory")
            cached.extend(docs[start : start + batch_size])
            encoded.extend(cached)
        return np.array([[len(doc)] * embed.MODELS["eg2"].dim for doc in docs], dtype=np.float32)

    run.st.encode = encode
    run.release.side_effect = lambda st: cached.clear()
    monkeypatch.setattr(embed, "sync", lambda: None)
    monkeypatch.setattr(embed, "encode", encode_documents)
    result = chunks.ingest(run.paths, run.root, "synthetic-reader")
    assert result["embedded"] == len(encoded) > embed.BATCH["B"]
    assert len(encoded) % embed.BATCH["B"]
    vectors = run.store.read()[1]["vectors"]
    np.testing.assert_array_equal(np.sort(vectors[:, 0]), sorted(len(doc) for doc in encoded))
    assert not cached


def test_bounded_batches_keep_length_order():
    docs = [str(i) * n for i, n in enumerate((3, 9, 1, 7, 5, 9, 2, 8, 4, 6))]
    calls = []

    def encode(group, **kwargs):
        calls.append([len(doc) for doc in group])
        return np.array([[int(doc[0]), len(doc)] for doc in group], np.float32)

    vectors, _ = encode_documents(SimpleNamespace(encode=encode), docs, 4, release_batches=True)
    assert [len(call) for call in calls] == [4, 4, 2]
    assert all(min(a) >= max(b) for a, b in zip(calls, calls[1:], strict=False))
    np.testing.assert_array_equal(vectors[:, 0], range(len(docs)))


def test_rerun_skips(chunk_run):
    run = chunk_run
    _, records = dataset.app_records(run.paths, Archive.load(run.root), binary="synthetic-reader")
    rows = [
        row
        for record, page in zip(records, embed.body_pages(records, strict=True), strict=True)
        if page.get("text")
        for row in chunks.chunk_page(record["key"], page.get("title") or record["title"], page["text"])
    ]
    run.store.reconcile(
        rows,
        {"dim": 0, "chunker": chunks.CHUNKER},
        lambda indices: np.empty((len(indices), 0), np.float32),
        retain=False,
    )
    run.reader.reset_mock()
    with patch.object(chunks, "chunk_page", side_effect=AssertionError("unchanged pages must not scan")):
        first = chunks.ingest(run.paths, run.root, "synthetic-reader")
    manifest, arrays = run.store.read()
    assert (first["pages"], first["skipped"], first["embedded"]) == (4, 3, len(rows))
    assert manifest["rows"] == rows
    assert arrays["vectors"].shape == (len(rows), 768) and arrays["vectors"].dtype == np.float32
    assert manifest["model"] == {
        key: value for key, value in embed.model_identity(embed.MODELS["eg2"]).items() if key != "char_cap"
    } | {"chunker": chunks.CHUNKER, "input_version": "section-chunk-v1"}
    run.load.assert_called_once_with(embed.MODELS["eg2"], local_files_only=True)
    run.release.assert_called_once_with(run.st)
    (run.root / "pages/metadata.jsonl").write_text(
        json.dumps({"url": NOTE, "status": "ok", "description": "Unrelated synthetic metadata"}) + "\n"
    )
    run.load.reset_mock()
    run.encode.reset_mock()
    with (
        patch.object(chunks, "chunk_page", side_effect=AssertionError("unchanged pages must not scan")),
        patch.object(VectorStore, "_publish", side_effect=AssertionError("unchanged rows must not republish")),
    ):
        second = chunks.ingest(run.paths, run.root, "synthetic-reader")
    assert (second["skipped"], second["embedded"], second["reused"]) == (3, 0, len(rows))
    run.load.assert_not_called()
    run.encode.assert_not_called()
    assert run.store.read()[0] == manifest
    assert run.reader.call_count == 4
    assert {key for call in run.reader.call_args_list for key in call.args[0][call.args[0].index("--") + 1 :]} == {
        NOTE,
        LOOSE,
        UNPINNED,
    }


def test_changed_page_embeds_only_its_rows(chunk_run):
    run = chunk_run
    chunks.ingest(run.paths, run.root, "synthetic-reader")
    manifest, arrays = run.store.read()
    run.bodies[NOTE] = run.bodies[NOTE].replace("Unicode", "Changed", 1).split("# Obsolete")[0]
    run.encode.reset_mock()
    with patch.object(chunks, "chunk_page", wraps=chunks.chunk_page) as segment:
        changed = chunks.ingest(run.paths, run.root, "synthetic-reader")
    segment.assert_called_once()
    assert segment.call_args.args[0] == NOTE and changed["skipped"] == 2
    current, vectors = run.store.read()
    old_note = [row for row in manifest["rows"] if row["page_id"] == NOTE]
    new_note = [row for row in current["rows"] if row["page_id"] == NOTE]
    assert len(new_note) < len(old_note)
    stable = next(row for row in new_note if row["section"] == "Stable")
    prior = next(row for row in old_note if row["id"] == stable["id"])
    assert stable["hash"] == prior["hash"], "one sibling keeps both its offsets and content hash"
    assert new_note[0]["page_hash"] != old_note[0]["page_hash"]
    obsolete = {row["id"] for row in old_note if row["section"] == "Obsolete"}
    assert obsolete and not obsolete & {row["id"] for row in current["rows"]}
    assert changed["embedded"] == len(new_note) == len(run.encode.call_args.args[1])
    run.encode.assert_called_once()
    for i, row in enumerate(current["rows"]):
        if row["page_id"] != NOTE:
            j = manifest["rows"].index(row)
            np.testing.assert_array_equal(vectors["vectors"][i], arrays["vectors"][j])


def test_removed_page_drops_rows(chunk_run):
    run = chunk_run
    chunks.ingest(run.paths, run.root, "synthetic-reader")
    manifest, arrays = run.store.read()
    run.load.reset_mock()
    run.encode.reset_mock()
    state = run.root / "library.json"
    state.write_text(json.dumps({"schema_version": 1, "forgotten": [NOTE]}))
    removed = chunks.ingest(run.paths, run.root, "synthetic-reader")
    current, vectors = run.store.read()
    keep = [i for i, row in enumerate(manifest["rows"]) if row["page_id"] != NOTE]
    assert removed["embedded"] == 0 and removed["dropped"] == len(manifest["rows"]) - len(keep)
    assert current["rows"] == [manifest["rows"][i] for i in keep]
    np.testing.assert_array_equal(vectors["vectors"], arrays["vectors"][keep])
    state.write_text(json.dumps({"schema_version": 1, "forgotten": [NOTE, LOOSE, WEB, UNPINNED]}))
    empty = chunks.ingest(run.paths, run.root, "synthetic-reader")
    assert empty["chunks"] == empty["embedded"] == 0
    assert run.store.read()[1]["vectors"].shape == (0, 768)
    run.load.assert_not_called()
    run.encode.assert_not_called()


def test_chunk_embedding_input(chunk_run):
    run = chunk_run
    run.titles[NOTE] = "Reader <|image|>"
    run.bodies[NOTE] = "# Café <|audio|>\r\n\r\nbody 🦊 e\u0301 � <|image|> <|video|>\r\n"
    run.bodies[LOOSE] = "plain <|audio|> 🦊\r\n"
    added = run.root / "pages/added.jsonl"
    added.write_text(
        "".join(
            json.dumps(json.loads(line) | ({"title": ""} if json.loads(line)["url"] == LOOSE else {})) + "\n"
            for line in added.read_text().splitlines()
        )
    )
    result = chunks.ingest(run.paths, run.root, "synthetic-reader")
    docs = run.encode.call_args.args[1]
    assert set(docs) == {
        "title: Reader < |image|> | text: Café < |audio|> - "
        "# Café < |audio|>\r\n\r\nbody 🦊 e\u0301 � < |image|> < |video|>\r\n",
        "title: none | text:  - plain < |audio|> 🦊\r\n",
        "title: Snapshot title | text: Kept - # Kept\r\n\r\nbody café\r\n",
    }
    assert result["over_capacity"] == 0
    assert run.encode.call_args.args[2] == embed.BATCH["B"]
    run.st.tokenizer.assert_called_once_with(docs, add_special_tokens=True, truncation=False, padding=False)
    # The title/prefix alone pushes this small body over the loaded encoder's capacity.
    run.titles[NOTE] = "T" * embed.MAX_TOKENS
    run.bodies[NOTE] = "small"
    run.encode.reset_mock()
    result = chunks.ingest(run.paths, run.root, "synthetic-reader")
    assert result["over_capacity"] == result["embedded"] == 1
    assert run.encode.call_args.args[1] == ["title: " + run.titles[NOTE] + " | text:  - small"]
    manifest, arrays = run.store.read()
    for failure in ("limit", "tokenizer"):
        run.bodies[NOTE] += " changed"
        run.st.max_seq_length = None if failure == "limit" else embed.MAX_TOKENS
        if failure == "tokenizer":
            run.st.tokenizer.side_effect = RuntimeError("synthetic tokenizer failure")
        run.encode.reset_mock()
        run.release.reset_mock()
        with pytest.raises((ValueError, RuntimeError)):
            chunks.ingest(run.paths, run.root, "synthetic-reader")
        run.encode.assert_not_called()
        run.release.assert_called_once_with(run.st)
        current, vectors = run.store.read()
        assert current == manifest
        np.testing.assert_array_equal(vectors["vectors"], arrays["vectors"])
