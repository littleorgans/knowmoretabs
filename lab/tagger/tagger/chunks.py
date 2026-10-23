"""Full bodies to structural UTF-8 references and incremental EG2 embeddings in Store C."""

from bisect import bisect_left, bisect_right
from collections import defaultdict
from dataclasses import dataclass
import json
from pathlib import Path
import re

from . import dataset, embed
from .app import kmt
from .archive import Archive, sha256_hex
from .paths import Paths
from .vector_store import VectorStore

MIN_TOKENS = 256
MAX_TOKENS = 512
OVERLAP_TOKENS = 32
BYTES_PER_TOKEN = 4
CHUNKER = {
    "grammar": "markdown-blocks-v1",
    "counter": "utf8-bytes-div4-v1",
    "target": [MIN_TOKENS, MAX_TOKENS],
    "overlap": OVERLAP_TOKENS,
}
ATX = re.compile(r" {0,3}(#{1,6})(?:[ \t]+(.*?)|[ \t]*)$")
SETEXT = re.compile(r" {0,3}(?:=+|-+)[ \t]*$")
FENCE = re.compile(r" {0,3}(`{3,}|~{3,})(.*)$")
LIST = re.compile(r"[ \t]*(?:[-+*]|[0-9]+[.)])[ \t]+")
INDENT = re.compile(r"(?: {4}|\t)")


@dataclass(frozen=True)
class Block:
    start: int  # character boundaries inside the exact body
    end: int
    section: str
    kind: str


def _heading(lines: list[str], i: int) -> tuple[str, int] | None:
    line = lines[i].rstrip("\r\n")
    if match := ATX.fullmatch(line):
        return re.sub(r"[ \t]+#+[ \t]*$", "", match[2] or "").strip(), 1
    if i + 1 < len(lines) and line.strip() and SETEXT.fullmatch(lines[i + 1].rstrip("\r\n")):
        return line.strip(), 2
    return None


def blocks(body: str) -> list[Block]:
    """Scan original lines. Separators belong to the preceding block, leading blanks to the first."""
    lines = body.splitlines(keepends=True)
    positions = [0]
    for line in lines:
        positions.append(positions[-1] + len(line))
    result = []
    i, start, section = 0, 0, ""
    while i < len(lines):
        if not lines[i].strip():
            i += 1
            continue
        line = lines[i].rstrip("\r\n")
        fence = FENCE.fullmatch(line)
        if fence:
            kind = "code"
            closing = re.compile(r" {0,3}" + re.escape(fence[1][0]) + "{" + str(len(fence[1])) + r",}[ \t]*$")
            i += 1
            while i < len(lines):
                closed = closing.fullmatch(lines[i].rstrip("\r\n"))
                i += 1
                if closed:
                    break
        elif INDENT.match(line):
            kind = "code"
            i += 1
            while i < len(lines) and (not lines[i].strip() or INDENT.match(lines[i])):
                i += 1
        elif LIST.match(line):
            kind = "list"
            i += 1
            while i < len(lines):
                if not lines[i].strip():
                    following = i
                    while following < len(lines) and not lines[following].strip():
                        following += 1
                    if following == len(lines) or not (
                        LIST.match(lines[following]) or lines[following].startswith((" ", "\t"))
                    ):
                        break
                    i = following
                if not lines[i].startswith((" ", "\t")) and (
                    _heading(lines, i) or FENCE.fullmatch(lines[i].rstrip("\r\n"))
                ):
                    break
                i += 1
        elif heading := _heading(lines, i):
            section, width = heading
            kind = "heading"
            i += width
        else:
            kind = "paragraph"
            i += 1
            while i < len(lines) and lines[i].strip():
                if (
                    _heading(lines, i)
                    or FENCE.fullmatch(lines[i].rstrip("\r\n"))
                    or LIST.match(lines[i])
                    or INDENT.match(lines[i])
                ):
                    break
                i += 1
        while i < len(lines) and not lines[i].strip():
            i += 1
        result.append(Block(start, positions[i], section, kind))
        start = positions[i]
    if start < len(body):
        result.append(Block(start, len(body), section, "paragraph"))
    return result


def _hash(*values) -> str:
    return sha256_hex(json.dumps(values, ensure_ascii=False, separators=(",", ":")))


def _paragraph_spans(body: str, start: int, end: int, offsets: list[int]):
    """Bound each piece and overlap in bytes, always advancing by at least one Unicode character."""
    while start < end:
        limit = bisect_right(offsets, offsets[start] + MAX_TOKENS * BYTES_PER_TOKEN, start + 1, end + 1) - 1
        stop = limit
        if stop < end:
            lower = bisect_left(offsets, offsets[start] + MIN_TOKENS * BYTES_PER_TOKEN, start + 1, stop + 1)
            while stop > lower and not body[stop - 1].isspace():
                stop -= 1
            if stop == lower:
                stop = limit
        yield start, stop
        if stop == end:
            break
        start = bisect_left(offsets, offsets[stop] - OVERLAP_TOKENS * BYTES_PER_TOKEN, start + 1, stop + 1)


def chunk_page(page_id: str, title: str, body: str, *, page_hash: str | None = None) -> list[dict]:
    """Reference rows only. Hash original text and title before model formatting."""
    title = title or "none"
    page_hash = page_hash or _hash(title, body)
    offsets = [0]
    for char in body:
        offsets.append(offsets[-1] + len(char.encode("utf-8")))
    spans = []
    pending = None
    for block in blocks(body):
        if pending is not None and (
            block.kind == "heading" or offsets[block.end] - offsets[pending.start] > MAX_TOKENS * BYTES_PER_TOKEN
        ):
            spans.append(pending)
            pending = None
        if offsets[block.end] - offsets[block.start] > MAX_TOKENS * BYTES_PER_TOKEN:
            if block.kind == "paragraph":
                spans.extend(
                    Block(start, end, block.section, block.kind)
                    for start, end in _paragraph_spans(body, block.start, block.end, offsets)
                )
            else:
                spans.append(block)
        else:
            pending = Block(pending.start if pending else block.start, block.end, block.section, block.kind)
    if pending is not None:
        spans.append(pending)
    return [
        {
            "id": _hash(page_id, offsets[span.start], offsets[span.end]),
            "page_id": page_id,
            "start": offsets[span.start],
            "end": offsets[span.end],
            "section": span.section,
            "hash": _hash(title, span.section, body[span.start : span.end]),
            "page_hash": page_hash,
        }
        for span in spans
    ]


def ingest(paths: Paths, root: Path, binary: str = kmt.BINARY) -> dict:
    """Freeze visible bodies, reuse segmentation, then embed only stale or missing rows."""
    spec = embed.MODELS["eg2"]
    model = embed.model_identity(spec)
    del model["char_cap"]
    model.update(chunker=CHUNKER, input_version="section-chunk-v1")
    store = VectorStore(paths.emb / "eg2", "C")
    old = store.read()
    previous = defaultdict(list)
    if old and old[0]["model"].get("chunker") == CHUNKER:
        for row in old[0]["rows"]:
            previous[row["page_id"]].append(row)
    archive = Archive.load(root)
    _, records = dataset.app_records(
        paths, archive, keys=[key for key in archive.known if key not in archive.state.forgotten], binary=binary
    )
    pages = embed.body_pages(records, strict=True)
    sources = {}
    rows, skipped = [], 0
    for record, page in zip(records, pages, strict=True):
        body = page.get("text")
        if not body:
            continue
        title = page.get("title") or record["title"] or "none"
        page_hash = _hash(title, body)
        sources[record["key"]] = title, body.encode("utf-8"), page_hash
        prior = previous[record["key"]]
        if prior and all(row.get("page_hash") == page_hash for row in prior):
            rows.extend(prior)
            skipped += 1
        else:
            rows.extend(chunk_page(record["key"], title, body, page_hash=page_hash))
    over_capacity = 0

    def encode(indices):
        nonlocal over_capacity
        docs = []
        for i in indices:
            row = rows[i]
            title, raw, page_hash = sources[row["page_id"]]
            text = raw[row["start"] : row["end"]].decode("utf-8")
            if row["page_hash"] != page_hash or row["hash"] != _hash(title, row["section"], text):
                raise ValueError("chunk reference does not match the frozen body")
            docs.append(spec.format(title, f"{row['section']} - {text}"))
        st, _ = embed.load_text(spec, local_files_only=True)
        try:
            limit = st.max_seq_length
            if not isinstance(limit, int) or limit <= 0:
                raise ValueError("chunk encoder has no known token limit")
            over_capacity = int((embed.token_lengths(st, docs) > limit).sum())
            vectors, _ = embed.encode(st, docs, embed.BATCH["B"])
            return vectors
        finally:
            embed.release(st)

    _, counts = store.reconcile(rows, model, encode, retain=False)
    return {
        "pages": len(records),
        "chunks": len(rows),
        "skipped": skipped,
        "oversize": sum(row["end"] - row["start"] > MAX_TOKENS * BYTES_PER_TOKEN for row in rows),
        "over_capacity": over_capacity,
        **counts,
    }
