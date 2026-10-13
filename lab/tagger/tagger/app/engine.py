"""The app's model side, loaded once: the library's cached vectors, a keyword index, and test S2's supervised
rules fitted on every owner labelled page. Search fuses EG2 cosine with keywords (and images on request) by
reciprocal rank; a result set gets suggested tags (S2's mean z ranking) and, for the picked tags, prechecks.
The decisions follow `library.json` (`apply_state`): a name the vocabulary gains joins as a zero shot tag with its
name as the query (`add_tags`), a retired one keeps its column out of sight, and every page's tags, labels and
visibility are the file's, without touching a vector, the keyword index or a fitted head.
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
from . import kmt
from ..archive import Archive, LibraryState
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
        raise SystemExit(f"missing tagger vectors at {paths.data}; run tagger embed --model eg2")


Encode = Callable[[list[str]], np.ndarray]
Embed = Callable[[list[dict], str], np.ndarray]  # page records, text input name -> unit document vectors


@dataclass
class Library:
    records: list[dict]  # dataset rows, content and image paths under the given root
    tags: list[str]  # a column per name: those active at load, most used first, then those gained; never removed
    Y: np.ndarray  # pages x tags, the active names each page holds
    X: np.ndarray  # text vectors
    input_name: str
    image: np.ndarray | None  # pages x d image vectors; rows without an image are zero
    has_image: np.ndarray
    live: np.ndarray  # rows a view may show: known and not forgotten
    keywords: TfidfVectorizer
    K: object  # pages x terms TF-IDF
    rules: Rules
    S: np.ndarray  # supervised score, pages x tags
    threshold: np.ndarray
    spread: np.ndarray  # per tag SD of S over the library
    root: Path  # the archive the records point into
    state: LibraryState  # the decisions the records' tags, labels and visibility follow (`Archive.state`)
    paths: Paths  # durable document stores
    embed_image: Callable | None  # lazy image encoder, shared by startup and sync
    dim: int
    added_q: np.ndarray  # queries of the tags `add_tags` appended, in tag order after the rules' own
    binary: str = kmt.BINARY  # the knowmoretabs that prints a local document's text (`kmt.read_argv`)
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


def load(
    paths: Paths, root: Path, encode: Encode, embed: Embed, *, embed_image=None, dim=None, binary: str = kmt.BINARY
) -> tuple[Library, dict]:
    dim = MODELS[MODEL].dim if dim is None else dim
    timings = {}
    start = time.perf_counter()
    archive = Archive.load(root)
    legacy, records = dataset.app_records(paths, archive, binary=binary)
    counts = {n: 0 for n in archive.state.active.values()}
    for r in records:
        for t in r["tags"]:
            counts[t] += 1
    tags = sorted(counts, key=lambda n: (-counts[n], n.lower()))
    Y = _labels(records, tags)
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
        state=archive.state,
        paths=paths,
        embed_image=embed_image,
        dim=dim,
        added_q=np.zeros((0, X.shape[1]), X.dtype),
        binary=binary,
    )

    stats = {
        "pages": len(lib.records),
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


def _labels(records: list[dict], tags: list[str]) -> np.ndarray:
    """Pages x tags: 1 where the record holds the tag."""
    return np.array([[t in r["tags"] for t in tags] for r in records], np.int8).reshape(len(records), len(tags))


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


def active(lib: Library) -> np.ndarray:
    """The tag columns whose names the vocabulary holds active: the ones a view lists, suggests or takes."""
    return np.array([t.lower() in lib.state.active for t in lib.tags], bool)


def project(lib: Library) -> Library:
    """Every loaded record's tags, labels and visibility, `Y` and `live`, as `lib.state` holds them."""
    state = lib.state
    records = [
        {**r, "tags": (tags := state.tags(r["key"])), "labelled": bool(tags), "forgotten": r["key"] in state.forgotten}
        for r in lib.records
    ]
    live = np.array([not r["forgotten"] for r in records], bool)
    return replace(lib, records=records, Y=_labels(records, lib.tags), live=live)


def apply_state(lib: Library, state: LibraryState, encode: Encode) -> Library:
    """The library following `state`: each active name without a column joins as a zero shot tag (a retired name
    brought back takes its own column again), and every record follows the state's decisions."""
    lib = add_tags(lib, list(state.active.values()), encode)
    return project(replace(lib, state=state))


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


def sync(lib: Library, archive: Archive, urls: list[str], embed: Embed) -> Library:
    """Refresh and persist the given known pages, preserving all other keyed cache rows. Their decisions are the ones
    `lib.state` holds, not the archive's, which may be older: the caller follows the file afterwards."""
    known = set(archive.known)
    urls = [url for url in dict.fromkeys(urls) if url in known]
    if not urls:
        return lib
    _, fresh = dataset.app_records(lib.paths, archive, keys=urls, binary=lib.binary)
    vectors, image_rows, mask, counts = _vectors(
        lib.paths,
        fresh,
        lib.input_name,
        embed,
        lib.embed_image,
        lib.dim,
        retain=True,
    )
    if counts["failed"]:
        print(json.dumps({"image_failed": counts["failed"]}))
    records, rows = lib.records.copy(), dict(lib.rows)
    X = lib.X.copy()
    image = None if lib.image is None else lib.image.copy()
    has_image = lib.has_image.copy()
    for i, (url, record) in enumerate(zip(urls, fresh, strict=True)):
        row = rows.get(url)
        if row is None:
            rows[url] = len(records)
            records.append(record)
            X = np.vstack([X, vectors[i : i + 1]])
            if image is not None:
                image = np.vstack([image, image_rows[i : i + 1]])
            has_image = np.concatenate([has_image, mask[i : i + 1]])
        else:
            records[row], X[row] = record, vectors[i]
            if image is not None:
                image[row] = image_rows[i]
            has_image[row] = mask[i]
    S = np.hstack([lib.rules.scores(X)["supervised"], X @ lib.added_q.T])
    keywords, K = _keyword_index(records)
    return project(
        replace(
            lib,
            records=records,
            X=X,
            image=image,
            has_image=has_image,
            S=S,
            spread=_spread(S),
            keywords=keywords,
            K=K,
        )
    )


def _ranks(scores: np.ndarray, eligible: np.ndarray) -> np.ndarray:
    """1 based rank of each eligible row by score, 0 elsewhere."""
    rank = np.zeros(len(scores), int)
    rows = np.flatnonzero(eligible)
    rank[rows[np.argsort(-scores[rows], kind="stable")]] = np.arange(1, len(rows) + 1)
    return rank


def _fused(lib: Library, q: np.ndarray, text: str, images: bool, among: np.ndarray) -> tuple[np.ndarray, dict, dict]:
    """Reciprocal rank fusion over the rows `among` of EG2 cosine, keyword score and, with `images`, the image
    cosine: the fused score (-inf outside `among`), each source's scores and its 1 based ranks (0: not ranked)."""
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
    return fused, sources, ranks


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
    fused, sources, ranks = _fused(lib, q, text, images, among)
    top = np.argsort(-fused, kind="stable")[offset : min(offset + n, int(among.sum()))]
    return [
        {
            "row": int(i),
            "fused": float(fused[i]),
            "sources": {k: {"score": float(s[i]), "rank": int(ranks[k][i])} for k, (s, _) in sources.items()},
        }
        for i in top
    ]


def order(lib: Library, q: np.ndarray, text: str, among: np.ndarray) -> np.ndarray:
    """Every row `among`, best first, as `search` ranks text and keywords; ties keep row order."""
    fused, _, _ = _fused(lib, q, text, False, among)
    return np.argsort(-fused, kind="stable")[: int(among.sum())]


def newest(lib: Library, among: np.ndarray) -> np.ndarray:
    """Every row `among`, the latest to enter the library first (`Archive.added`), then those without a time;
    ties follow URL order, including rows appended by sync."""
    added = np.array([-np.inf if r["added"] is None else r["added"] for r in lib.records])
    keys = np.array([r["key"] for r in lib.records])
    rows = np.flatnonzero(among)
    return rows[np.lexsort((keys[rows], -added[rows]))]


def suggest(lib: Library, rows: list[int], k: int = SUGGEST) -> list[dict]:
    """Active tags ranked by the mean, over the result pages, of the supervised score's z against the library."""
    z = (lib.S[rows] - lib.S.mean(axis=0)) / lib.spread
    mean, on = z.mean(axis=0), active(lib)
    order = [j for j in np.argsort(-mean, kind="stable") if on[j]]
    return [{"tag": lib.tags[j], "z": float(mean[j])} for j in order[:k]]


def tag_info(lib: Library) -> list[dict]:
    """Every active tag, in column order: how many pages hold it, and whether a head or its name scores it."""
    positives, on = lib.Y.sum(axis=0), active(lib)
    heads = set(lib.rules.heads.tolist())
    return [
        {"name": t, "positives": int(positives[j]), "source": "head" if j in heads else "zero shot"}
        for j, t in enumerate(lib.tags)
        if on[j]
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
