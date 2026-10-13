"""A local document's text reaches its vector through `knowmoretabs read`, and nowhere else: the record names the
command, the embedded input carries what it printed, and the input hash follows it. Every key, title and text here is
invented."""

import json
from pathlib import Path

from test_add import fake_binary

import fake_knowmoretabs
from tagger import dataset
from tagger.archive import Archive, sha256_hex
from tagger.embed import CHAR_CAP, MAX_TOKENS, MODELS, input_rows, page_texts
from tagger.paths import Paths

NOTE = "kmt-git:example.test/o/r//notes/quokka.md"
LOOSE = "kmt-blob:sha256-" + "ab" * 32
WEB = "https://web.example/plain"
UNPINNED = "kmt-git:example.test/o/r//notes/unpinned.md"


def archive(root: Path) -> Archive:
    """Four added pages: two pinned documents, one without a pin, one web page with no kept text."""
    (root / "snapshots").mkdir(parents=True)
    for url, title in ((NOTE, "Quokka note"), (LOOSE, "Loose paper"), (UNPINNED, "Unpinned"), (WEB, "Plain")):
        line = {"schema_version": 1, "url": url, "added_at": fake_knowmoretabs.AT, "title": title}
        fake_knowmoretabs.append(root / "pages/added.jsonl", line)
    for url in (NOTE, LOOSE):
        pin = {"schema_version": 1, "url": url, "kind": "md", "added_at": fake_knowmoretabs.AT}
        fake_knowmoretabs.append(root / "pages/sources.jsonl", pin)
    return Archive.load(root)


def test_a_documents_text_is_embedded_from_read_and_never_kept(tmp_path, monkeypatch):
    root = tmp_path / "archive"
    binary = fake_binary(tmp_path)
    runs = tmp_path / "runs.jsonl"
    monkeypatch.setenv("FAKE_KMT_LOG", str(runs))
    _, records = dataset.app_records(Paths(tmp_path / "data"), archive(root), binary=binary)
    by_key = {r["key"]: r for r in records}
    assert by_key[NOTE]["read"] == [binary, "--root", str(root), "read", "--", NOTE]
    assert by_key[WEB]["read"] is None
    assert not by_key[NOTE]["text_ok"] and by_key[NOTE]["content_path"] is None

    texts = dict(zip(by_key, page_texts(records, True, MAX_TOKENS * CHAR_CAP), strict=True))
    body = fake_knowmoretabs.SOURCE_BODY + "\n"
    assert texts[NOTE] == ("Quokka note", "example.test/o/r//notes/quokka.md\n\n" + body)
    assert texts[LOOSE][1].endswith("\n\n" + body)
    assert texts[UNPINNED] == ("Unpinned", "example.test/o/r//notes/unpinned.md"), "read refused: title and path only"
    assert texts[WEB] == ("Plain", "web.example/plain")
    # Every document is read by one run, refusals included.
    reads = [json.loads(line)["argv"] for line in runs.read_text().splitlines()]
    assert reads == [["--root", str(root), "read", "--json", "--", *(r["key"] for r in records if r["read"])]]
    assert page_texts(records, False, MAX_TOKENS * CHAR_CAP)[0][1] == by_key[records[0]["key"]]["a_text"]

    # The vector's input hash is over the document's text, so it moves when the pinned bytes do.
    model = MODELS["eg2"]
    hashes = {row["id"]: row["hash"] for row in input_rows(records, model, "B")}
    assert len(runs.read_text().splitlines()) == 2, "input B's documents are read together"
    assert hashes[NOTE] == sha256_hex(model.format("Quokka note", texts[NOTE][1]))
    assert hashes[NOTE] != {row["id"]: row["hash"] for row in input_rows(records, model, "A")}[NOTE]

    # Nothing of the text was written: not in the archive, not in a record.
    for path in root.rglob("*"):
        assert not path.is_file() or "quokka migrations" not in path.read_text(), path
    assert "quokka migrations" not in json.dumps(records)


def test_without_the_binary_a_document_embeds_its_title_and_path(tmp_path):
    root = tmp_path / "archive"
    _, records = dataset.app_records(Paths(tmp_path / "data"), archive(root), binary=str(tmp_path / "absent"))
    texts = dict(zip((r["key"] for r in records), page_texts(records, True, 100), strict=True))
    assert texts[NOTE] == ("Quokka note", "example.test/o/r//notes/quokka.md")
