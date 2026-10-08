"""The app's model side, loaded once: the library's cached vectors, a keyword index, and test S2's supervised
rules fitted on every owner labelled page. Search fuses EG2 cosine with keywords (and images on request) by
reciprocal rank; a result set gets suggested tags (S2's mean z ranking) and, for the picked tags, prechecks.
Tags the owner makes in the app join the library as zero shot tags with their name as the query (`add_tags`).
Document vectors are persisted by page id. Startup and `sync` refresh changed inputs
and reuse matching rows after knowmoretabs adds, forgets or restores a page.

`encode` turns texts into unit EG2 query vectors, shared by text and image search.
The image model is loaded only when a missing or stale image needs embedding.
"""

import json
import time
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from pathlib import Path

import numpy as np
from sklearn.feature_extraction.text import TfidfVectorizer

from .. import dataset
from ..archive import Archive
from ..embed import CHAR_CAP, MAX_TOKENS, MODELS, page_texts, persist_text, persist_images
from ..vector_store import VectorStore
from ..guided.search_tag import Rules, fit_rules
from ..paths import Paths, read_json
from ..zeroshot import ZERO_K_SD, query_variants

MODEL = "eg2"
TEXT_INPUTS = ("B", "A")  # the first cached one ranks and trains
RRF_K = 60
SUGGEST = 8


def require_vectors(paths: Paths) -> None:
    """Refuse an unprepared data directory before startup can rebuild or create it."""
    if not any(VectorStore(paths.emb / MODEL, name).exists for name in TEXT_INPUTS):
        raise SystemExit(f"missing tagger data at {paths.data}; set KMT_TAGGER_DATA")


Encode = Callable[[list[str]], np.ndarray]
Embed = Callable[[list[dict], str], np.ndarray]  # page records, text input name -> unit document vectors


@dataclass
class Library:
    records: list[dict]  # dataset rows, content and image paths under the given root
    tags: list[str]  # every active owner tag, most used first
    Y: np.ndarray  # pages x tags, owner labels
    X: np.ndarray  # text vectors
    input_name: str
    image: np.ndarray | None  # pages x d image vectors; rows without an image are zero
    has_image: np.ndarray
    live: np.ndarray  # rows a search may return: known and not forgotten
    keywords: TfidfVectorizer
    K: object  # pages x terms TF-IDF
    rules: Rules
    S: np.ndarray  # supervised score, pages x tags
    threshold: np.ndarray
    spread: np.ndarray  # per tag SD of S over the library
    root: Path  # the archive the records point into
    paths: Paths  # durable document stores
    embed_image: Callable | None  # lazy image encoder, shared by startup and sync
    dim: int
    added_q: np.ndarray  # queries of the tags `add_tags` appended, in tag order after the rules' own
    rows: dict = field(init=False)  # page key -> row

    def __post_init__(self):
        self.rows = {r["key"]: i for i, r in enumerate(self.records)}


def _vectors(paths, records, name, embed, embed_image, dim, *, legacy_records=None, retain=False):
    arrays, counts = persist_text(
        paths,
        records,
        MODELS[MODEL],
        name,
        embed,
        legacy_records=legacy_records,
        retain=retain,
        dim=dim,
    )
    text = arrays["vectors"]
    image, mask = None, np.zeros(len(records), bool)
    if embed_image is not None:
        arrays, stats = persist_images(
            paths,
            records,
            embed_image,
            legacy_records=legacy_records,
            retain=retain,
            dim=dim,
            allow_missing=True,
        )
        image, mask = arrays["vectors"], arrays["mask"]
        for key in counts:
            counts[key] += stats[key]
    return text, image, mask, counts


def _keyword_index(records: list[dict]) -> tuple[TfidfVectorizer, object]:
    """TF-IDF over title, URL host and path, metadata and page text (as much as input B reads)."""
    docs = [f"{title}\n{text}" for title, text in page_texts(records, True, MAX_TOKENS * CHAR_CAP)]
    vec = TfidfVectorizer(sublinear_tf=True, stop_words="english", lowercase=True)
    return vec, vec.fit_transform(docs)


def load(paths: Paths, root: Path, encode: Encode, embed: Embed, *, embed_image=None, dim=None) -> tuple[Library, dict]:
    dim = MODELS[MODEL].dim if dim is None else dim
    timings = {}
    start = time.perf_counter()
    archive = Archive.load(root)
    legacy, records = dataset.app_records(paths, archive)
    order = {n: i for i, n in enumerate(archive.active.values())}
    counts = {n: 0 for n in order}
    for tags in archive.owner_tags.values():
        for t in tags:
            counts[t] += 1
    tags = sorted(counts, key=lambda n: (-counts[n], n.lower()))
    Y = np.array([[t in archive.owner_tags.get(r["key"], ()) for t in tags] for r in records], np.int8)
    input_name = next((name for name in TEXT_INPUTS if VectorStore(paths.emb / MODEL, name).exists), TEXT_INPUTS[0])
    X, image, has_image, vector_counts = _vectors(
        paths,
        records,
        input_name,
        embed,
        embed_image,
        dim,
        legacy_records=legacy,
    )
    timings["load_s"] = time.perf_counter() - start

    start = time.perf_counter()
    descriptions_path = paths.data / "zeroshot" / "descriptions.json"
    descriptions = read_json(descriptions_path) if descriptions_path.exists() else {}
    q = _queries(encode, query_variants(tags, descriptions)["description"], X.shape[1])
    timings["tag_queries_s"] = time.perf_counter() - start

    start = time.perf_counter()
    fit = np.flatnonzero(Y.any(axis=1))
    rules = fit_rules(X[fit], Y[fit], q)
    S = rules.scores(X)["supervised"]
    threshold = rules.thresholds["supervised"].copy()
    unseen = ~np.isfinite(threshold)
    threshold[unseen] = _zero_shot_threshold(S[:, unseen])
    timings["heads_s"] = time.perf_counter() - start

    start = time.perf_counter()
    keywords, K = _keyword_index(records)
    timings["keywords_s"] = time.perf_counter() - start

    lib = Library(
        records=records,
        tags=tags,
        Y=Y,
        X=X,
        input_name=input_name,
        image=image,
        has_image=has_image.astype(bool),
        live=np.array([not r["forgotten"] for r in records]),
        keywords=keywords,
        K=K,
        rules=rules,
        S=S,
        threshold=threshold,
        spread=_spread(S),
        root=root,
        paths=paths,
        embed_image=embed_image,
        dim=dim,
        added_q=np.zeros((0, X.shape[1]), X.dtype),
    )

    stats = {
        "pages": len(lib.records),
        "gap_pages": len(records) - len(legacy),
        "live_pages": int(lib.live.sum()),
        "labelled_pages": len(fit),
        "tags": len(tags),
        "heads": len(rules.heads),
        "zero_shot_tags": len(tags) - len(rules.heads),
        "image_pages": int(lib.has_image.sum()),
        "input": input_name,
        "vectors": vector_counts,
        **{k: round(v, 2) for k, v in timings.items()},
    }
    return lib, stats


def _queries(encode: Encode, texts: list[str], dim: int) -> np.ndarray:
    """Unit query vectors, one row per text; with no text, none (an encoder may return a flat empty array)."""
    return encode(texts) if texts else np.zeros((0, dim), np.float32)


def _zero_shot_threshold(S: np.ndarray) -> np.ndarray:
    """Per column with no positives: the zero shot k = 0 rule over the library."""
    return S.mean(axis=0) + ZERO_K_SD * S.std(axis=0)


def _spread(S: np.ndarray) -> np.ndarray:
    return np.maximum(S.std(axis=0), 1e-12)


def find_tag(lib: Library, name: str) -> str | None:
    """The library's spelling of `name` in any case, as `tag --import` matches names."""
    folded = name.lower()
    return next((t for t in lib.tags if t.lower() == folded), None)


def add_tags(lib: Library, names: list[str], encode: Encode) -> Library:
    """The library with each name it lacks in any case appended as a zero shot tag: no positives, the name
    as its query, scored and thresholded as `load` scores an owner tag nobody holds yet."""
    new: list[str] = []
    for name in names:
        if find_tag(lib, name) is None and name.lower() not in {n.lower() for n in new}:
            new.append(name)
    if not new:
        return lib
    q = encode(new)
    S = lib.X @ q.T
    return replace(
        lib,
        tags=[*lib.tags, *new],
        Y=np.hstack([lib.Y, np.zeros((len(lib.Y), len(new)), lib.Y.dtype)]),
        S=np.hstack([lib.S, S]),
        threshold=np.concatenate([lib.threshold, _zero_shot_threshold(S)]),
        spread=np.concatenate([lib.spread, _spread(S)]),
        added_q=np.vstack([lib.added_q, q]),
    )


def sync(lib: Library, archive: Archive, url: str, embed: Embed) -> Library:
    """Refresh and persist one known page, preserving all other keyed cache rows."""
    if url not in set(archive.known):
        return lib
    _, records = dataset.app_records(lib.paths, archive, keys=[url])
    record = records[0]
    vectors, image_rows, mask, counts = _vectors(
        lib.paths,
        records,
        lib.input_name,
        embed,
        lib.embed_image,
        lib.dim,
        retain=True,
    )
    if counts["failed"]:
        print(json.dumps({"image_failed": counts["failed"]}))
    row = lib.rows.get(url)
    records = lib.records.copy()
    X, Y, live = lib.X.copy(), lib.Y.copy(), lib.live.copy()
    image = None if lib.image is None else lib.image.copy()
    has_image = lib.has_image.copy()
    if row is None:
        records.append(record)
        X = np.vstack([X, vectors])
        Y = np.vstack([Y, np.zeros((1, Y.shape[1]), Y.dtype)])
        live = np.concatenate([live, [not record["forgotten"]]])
        if image is not None:
            image = np.vstack([image, image_rows])
        has_image = np.concatenate([has_image, mask])
    else:
        records[row], X[row], live[row] = record, vectors[0], not record["forgotten"]
        if image is not None:
            image[row] = image_rows[0]
        has_image[row] = mask[0]
    S = np.hstack([lib.rules.scores(X)["supervised"], X @ lib.added_q.T])
    keywords, K = _keyword_index(records)
    return replace(
        lib,
        records=records,
        X=X,
        Y=Y,
        live=live,
        image=image,
        has_image=has_image,
        S=S,
        spread=_spread(S),
        keywords=keywords,
        K=K,
    )


def _ranks(scores: np.ndarray, eligible: np.ndarray) -> np.ndarray:
    """1 based rank of each eligible row by score, 0 elsewhere."""
    rank = np.zeros(len(scores), int)
    rows = np.flatnonzero(eligible)
    rank[rows[np.argsort(-scores[rows], kind="stable")]] = np.arange(1, len(rows) + 1)
    return rank


def search(
    lib: Library,
    q: np.ndarray,
    text: str,
    n: int,
    images: bool = False,
    among: np.ndarray | None = None,
    offset: int = 0,
) -> list[dict]:
    """The `n` pages `among` (default: the live ones) from rank `offset` (0 based) by reciprocal rank fusion of EG2
    cosine, keyword score and, with `images`, the image cosine. Each hit carries every source's score and rank (0:
    the source did not rank it)."""
    among = lib.live if among is None else among
    sources = {"text": (lib.X @ q, among)}
    kw = np.asarray((lib.K @ lib.keywords.transform([text]).T).todense()).ravel()
    sources["keyword"] = (kw, among & (kw > 0))
    if images and lib.image is not None:
        sources["image"] = (lib.image @ q, among & lib.has_image)
    fused = np.zeros(len(lib.records))
    ranks = {}
    for name, (scores, eligible) in sources.items():
        ranks[name] = _ranks(scores, eligible)
        fused += np.where(ranks[name] > 0, 1 / (RRF_K + ranks[name]), 0)
    fused[~among] = -np.inf
    top = np.argsort(-fused, kind="stable")[offset : min(offset + n, int(among.sum()))]
    return [
        {
            "row": int(i),
            "fused": float(fused[i]),
            "sources": {k: {"score": float(s[i]), "rank": int(ranks[k][i])} for k, (s, _) in sources.items()},
        }
        for i in top
    ]


def suggest(lib: Library, rows: list[int], k: int = SUGGEST) -> list[dict]:
    """Tags ranked by the mean, over the result pages, of the supervised score's z against the library."""
    z = (lib.S[rows] - lib.S.mean(axis=0)) / lib.spread
    mean = z.mean(axis=0)
    return [{"tag": lib.tags[j], "z": float(mean[j])} for j in np.argsort(-mean, kind="stable")[:k]]


def tag_info(lib: Library) -> list[dict]:
    positives = lib.Y.sum(axis=0)
    heads = set(lib.rules.heads.tolist())
    return [
        {"name": t, "positives": int(positives[j]), "source": "head" if j in heads else "zero shot"}
        for j, t in enumerate(lib.tags)
    ]


def prechecks(lib: Library, rows: list[int], picked: list[str]) -> list[list[dict]]:
    """Per result page, one suggestion per picked tag it does not already hold: checked where the supervised
    score clears the tag's threshold (S2 `decide`, supervised); p = sigmoid((s - threshold) / SD)."""
    cols = [lib.tags.index(t) for t in picked]
    heads = set(lib.rules.heads.tolist())
    out = []
    for i in rows:
        page = []
        for j in cols:
            if lib.Y[i, j]:
                continue
            s = float(lib.S[i, j])
            page.append(
                {
                    "tag": lib.tags[j],
                    "checked": bool(s >= lib.threshold[j]),
                    "p": float(1 / (1 + np.exp(-(s - lib.threshold[j]) / lib.spread[j]))),
                    "source": "head" if j in heads else "zero shot",
                }
            )
        out.append(page)
    return out
