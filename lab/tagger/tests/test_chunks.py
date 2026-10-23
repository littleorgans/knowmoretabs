"""Synthetic reader bodies and scratch Store C publications, without model weights."""

import json
from types import SimpleNamespace
from unittest.mock import Mock, patch

from test_sources import LOOSE, NOTE, UNPINNED, WEB, archive

from tagger import chunks
from tagger.app import kmt
from tagger.archive import sha256_hex
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


def test_rerun_skips(tmp_path, monkeypatch):
    root, paths = tmp_path / "archive", Paths(tmp_path / "data")
    archive(root)
    bodies = {
        NOTE: "# First\n\nUnicode separators: \u2028 \u2029 \u0085\n\n" + "synthetic paragraph " * 250,
        LOOSE: "# Kept\n\nbody\n",
        WEB: "",
    }
    calls = []

    def reader(argv, **kwargs):
        keys = argv[argv.index("--") + 1 :]
        calls.append(keys)
        answers = [
            {"schema_version": 1, "url": key, "text": bodies[key], "title": "Reader title" if key == NOTE else None}
            if key in bodies
            else {"schema_version": 1, "url": key, "error": {"kind": "source_read"}}
            for key in keys
        ]
        return SimpleNamespace(
            returncode=int(UNPINNED in keys),
            stdout="\n".join(json.dumps(answer, ensure_ascii=False) for answer in answers).encode(),
        )

    monkeypatch.setattr(kmt.subprocess, "run", reader)
    monkeypatch.setattr(kmt, "READ_BATCH_SIZE", 2)
    first = chunks.ingest(paths, root, "synthetic-reader")
    store = VectorStore(paths.emb / "eg2", "C")
    manifest, arrays = store.read()
    assert first == {"pages": 4, "chunks": len(manifest["rows"]), "skipped": 0, "oversize": 0}
    assert arrays["vectors"].shape == (first["chunks"], 0) and str(arrays["vectors"].dtype) == "float32"
    assert manifest["model"] == chunks.REFERENCE_MODEL
    (root / "pages/metadata.jsonl").write_text(
        json.dumps({"url": NOTE, "status": "ok", "description": "Unrelated synthetic metadata"}) + "\n"
    )
    with (
        patch.object(chunks, "chunk_page", side_effect=AssertionError("unchanged pages must not scan")),
        patch.object(VectorStore, "_publish", side_effect=AssertionError("unchanged rows must not republish")),
    ):
        second = chunks.ingest(paths, root, "synthetic-reader")
    assert second == first | {"skipped": 2}
    assert store.read()[0] == manifest
    assert len(calls) == 4 and {key for batch in calls[2:] for key in batch} == set(bodies) | {UNPINNED}
    bodies[NOTE] = "# Replacement\n\nshort body\n"
    segment = Mock(wraps=chunks.chunk_page)
    monkeypatch.setattr(chunks, "chunk_page", segment)
    third = chunks.ingest(paths, root, "synthetic-reader")
    segment.assert_called_once()
    assert segment.call_args.args[0] == NOTE and third["skipped"] == 1
    changed = store.read()[0]["rows"]
    old_note = [row for row in manifest["rows"] if row["page_id"] == NOTE]
    new_note = [row for row in changed if row["page_id"] == NOTE]
    assert len(new_note) == 1 < len(old_note)
    assert new_note[0]["page_hash"] != old_note[0]["page_hash"]
    assert not {row["id"] for row in old_note} & {row["id"] for row in new_note}
    assert [row for row in changed if row["page_id"] == LOOSE] == [
        row for row in manifest["rows"] if row["page_id"] == LOOSE
    ]
