"""Step 2: embeddings per model x input, cached as .npy rows aligned with dataset/pages.jsonl.

A: title + URL host and path + head metadata. B: A + page text (A when no text is ok).
C is assembled in `features` from B and the EG2 image embedding. Page text is read here only
and never printed; the log carries counts, timings and memory.
"""

import gc
import json
import time
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
import psutil
import torch

from . import dataset
from .archive import content_body
from .paths import Paths, write_json

MAX_TOKENS = 2048
CHUNKED_TOKENS = 8192
# Characters kept before tokenizing; generous enough that only text past the token limit is cut.
CHAR_CAP = 16


@dataclass(frozen=True)
class Model:
    key: str
    repo: str
    prompt: str  # recorded with every cache file
    config_kwargs: dict = field(default_factory=dict)

    def format(self, title: str, text: str) -> str:
        return self.prompt.format(title=title or "none", text=text)


# EG2 card: documents are `title: {title} | text: {content}`, `none` without a title.
# Qwen3 card: documents carry no instruction; queries alone take `Instruct: ...\nQuery:`.
MODELS = {
    "eg2": Model(
        "eg2", "google/embeddinggemma-2", "title: {title} | text: {text}", {"vision_config": None, "audio_config": None}
    ),
    "qwen3": Model("qwen3", "Qwen/Qwen3-Embedding-0.6B", "{title}\n{text}"),
}
IMAGE_MODEL = Model("eg2-full", "google/embeddinggemma-2", "(image, no prefix)")
DTYPE = torch.float32


def device() -> str:
    return "mps" if torch.backends.mps.is_available() else "cpu"


def sync() -> None:
    if device() == "mps":
        torch.mps.synchronize()


def rss_mb() -> int:
    return round(psutil.Process().memory_info().rss / 1e6)


def load(model: Model, *, local_files_only: bool = False):
    from sentence_transformers import SentenceTransformer

    start = time.perf_counter()
    st = SentenceTransformer(
        model.repo,
        device=device(),
        config_kwargs=model.config_kwargs,
        model_kwargs={"dtype": DTYPE},
        local_files_only=local_files_only,
    )
    stats = {
        "load_s": round(time.perf_counter() - start, 2),
        "rss_after_load_mb": rss_mb(),
        "params_m": round(sum(p.numel() for p in st.parameters()) / 1e6, 1),
        "device": device(),
        "dtype": str(DTYPE).removeprefix("torch."),
        "default_prompt_name": st.default_prompt_name,
    }
    if device() == "mps":
        stats["mps_driver_mb_after_load"] = round(torch.mps.driver_allocated_memory() / 1e6)
    return st, stats


def page_texts(records: list[dict], with_text: bool, chars: int) -> list[tuple[str, str]]:
    """(title, text) per page; text is A, plus the page body when asked for and ok."""
    out = []
    for r in records:
        text = r["a_text"]
        if with_text and r["text_ok"]:
            text = text + "\n\n" + content_body(Path(r["content_path"]))[:chars]
        out.append((r["title"], text))
    return out


def encode(st, docs: list, batch: int) -> tuple[np.ndarray, dict]:
    sync()
    start = time.perf_counter()
    emb = st.encode(docs, prompt="", batch_size=batch, normalize_embeddings=True, convert_to_numpy=True)
    sync()
    seconds = time.perf_counter() - start
    emb = np.asarray(emb, dtype=np.float32)
    assert np.isfinite(emb).all(), "non finite embedding"
    stats = {
        "docs": len(docs),
        "seconds": round(seconds, 2),
        "docs_per_s": round(len(docs) / seconds, 2),
        "dim": int(emb.shape[1]),
    }
    return emb, stats


def token_stats(st, docs: list[str]) -> dict:
    lengths = np.array([len(ids) for ids in st.tokenizer(docs, add_special_tokens=True)["input_ids"]])
    return {
        "tokens_mean_capped": round(float(np.minimum(lengths, st.max_seq_length).mean()), 1),
        "truncated": int((lengths > st.max_seq_length).sum()),
    }


def chunk_docs(st, model: Model, pages: list[tuple[str, str]]) -> tuple[list[str], list[int]]:
    """Each page split into MAX_TOKENS windows, up to CHUNKED_TOKENS in all; returns docs and owner index."""
    docs, owner = [], []
    for index, (title, text) in enumerate(pages):
        prefix = len(st.tokenizer(model.format(title, ""), add_special_tokens=True)["input_ids"])
        window = MAX_TOKENS - prefix - 2
        ids = st.tokenizer(text, add_special_tokens=False)["input_ids"][: window * (CHUNKED_TOKENS // MAX_TOKENS)]
        for start in range(0, max(len(ids), 1), window):
            docs.append(model.format(title, st.tokenizer.decode(ids[start : start + window])))
            owner.append(index)
    return docs, owner


def save(paths: Paths, key: str, name: str, emb: np.ndarray, meta: dict) -> None:
    folder = paths.emb / key
    folder.mkdir(parents=True, exist_ok=True)
    np.save(folder / f"{name}.npy", emb)
    write_json(folder / f"{name}.json", meta)
    print(json.dumps({"saved": f"{key}/{name}", **{k: v for k, v in meta.items() if k != "prompt"}}))


BATCH = {"A": 32, "B": 8, "B8k": 8}


def embed_input(st, model: Model, records: list[dict], name: str) -> tuple[np.ndarray, dict]:
    """One text input for these pages: A, B (truncated at MAX_TOKENS) or B8k (chunked mean)."""
    if name in ("A", "B"):
        docs = [model.format(t, x) for t, x in page_texts(records, name == "B", MAX_TOKENS * CHAR_CAP)]
        emb, run = encode(st, docs, BATCH[name])
        return emb, {**run, "max_tokens": MAX_TOKENS, **token_stats(st, docs)}
    start = time.perf_counter()
    pages = page_texts(records, True, CHUNKED_TOKENS * CHAR_CAP)
    docs, owner = chunk_docs(st, model, pages)
    chunks, run = encode(st, docs, BATCH[name])
    owner = np.array(owner)
    emb = np.stack([chunks[owner == i].mean(axis=0) for i in range(len(pages))])
    emb /= np.linalg.norm(emb, axis=1, keepdims=True)
    seconds = time.perf_counter() - start
    run.update(seconds=round(seconds, 2), chunks=len(docs), docs=len(pages), docs_per_s=round(len(pages) / seconds, 2))
    return emb, {**run, "max_tokens": CHUNKED_TOKENS, "pooling": f"mean of {MAX_TOKENS} token chunks"}


def load_text(model: Model, *, local_files_only: bool = False):
    st, stats = load(model, local_files_only=local_files_only)
    st.max_seq_length = MAX_TOKENS
    return st, stats


def release(st) -> None:
    del st
    gc.collect()
    if device() == "mps":
        torch.mps.empty_cache()


def embed_text(paths: Paths, records: list[dict], model: Model, chunked: bool) -> None:
    wanted = ["B8k"] if chunked else ["A", "B"]
    todo = [n for n in wanted if not (paths.emb / model.key / f"{n}.npy").exists()]
    if not todo:
        return
    st, stats = load_text(model)
    for name in todo:
        emb, run = embed_input(st, model, records, name)
        save(paths, model.key, name, emb, {"model": model.repo, "prompt": model.prompt, **stats, **run})
    release(st)


def embed_image_rows(st, records: list[dict]) -> tuple[np.ndarray, dict]:
    """EG2 image embeddings for pages with an ok image; zero rows elsewhere."""
    from PIL import Image

    rows = [i for i, r in enumerate(records) if r["image_ok"]]
    images = [Image.open(records[i]["image_path"]).convert("RGB") for i in rows]
    emb, run = encode(st, [{"image": im} for im in images], 8) if rows else (np.zeros((0, 768), np.float32), {})
    full = np.zeros((len(records), emb.shape[1]), dtype=np.float32)
    full[rows] = emb
    return full, run


def embed_images(paths: Paths, records: list[dict]) -> None:
    if (paths.emb / "image" / "eg2-full.npy").exists():
        return
    st, stats = load(IMAGE_MODEL)
    full, run = embed_image_rows(st, records)
    save(paths, "image", "eg2-full", full, {"model": IMAGE_MODEL.repo, "prompt": IMAGE_MODEL.prompt, **stats, **run})
    np.save(paths.emb / "image" / "eg2-full-mask.npy", np.array([r["image_ok"] for r in records]))
    release(st)


def run(paths: Paths, models: list[str] | None, chunked: bool) -> None:
    records, _ = dataset.load(paths)
    for key in models or list(MODELS):
        embed_text(paths, records, MODELS[key], chunked)
    if not chunked:
        embed_images(paths, records)
