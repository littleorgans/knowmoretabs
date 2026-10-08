"""The app's model side, loaded once: the library's cached vectors, a keyword index, and test S2's supervised
rules fitted on every owner labelled page. Search fuses EG2 cosine with keywords (and images on request) by
reciprocal rank; a result set gets suggested tags (S2's mean z ranking) and, for the picked tags, prechecks.
Tags the owner makes in the app join the library as zero shot tags with their name as the query (`add_tags`).

`encode` turns texts into unit EG2 query vectors; the text model alone is loaded, since its query vectors
equal the full model's, so one vector scores text and images. Page text feeds the keyword index only.
"""

import time
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from pathlib import Path

import numpy as np
from sklearn.feature_extraction.text import TfidfVectorizer

from .. import dataset
from ..archive import Archive
from ..embed import CHAR_CAP, MAX_TOKENS, page_texts
from ..guided.search_tag import Rules, fit_rules
from ..paths import Paths, read_json
from ..zeroshot import ZERO_K_SD, query_variants

MODEL = "eg2"
TEXT_INPUTS = ("B", "A")  # the first cached one ranks and trains
RRF_K = 60
SUGGEST = 8

Encode = Callable[[list[str]], np.ndarray]


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
    rows: dict = field(init=False)  # page key -> row

    def __post_init__(self):
        self.rows = {r["key"]: i for i, r in enumerate(self.records)}


def _records(paths: Paths, archive: Archive) -> list[dict]:
    """Dataset rows, which the cached vectors follow, checked against the archive and pointed at its files."""
    records, _ = dataset.load(paths)
    if [r["key"] for r in records] != archive.known:
        raise SystemExit(
            f"the dataset ({len(records)} pages) was built from another snapshot ({len(archive.known)} pages); "
            "rerun `tagger dataset` and `tagger embed` on it"
        )
    for r in records:
        content, image = archive.text_ok.get(r["key"]), archive.image_ok.get(r["key"])
        r.update(
            text_ok=content is not None,
            content_path=str(content) if content else None,
            image_ok=image is not None,
            image_path=str(image) if image else None,
            forgotten=r["key"] in archive.forgotten,
        )
    return records


def _text_vectors(paths: Paths) -> tuple[str, np.ndarray]:
    for name in TEXT_INPUTS:
        path = paths.emb / MODEL / f"{name}.npy"
        if path.exists():
            return name, np.load(path)
    raise SystemExit(f"no cached {MODEL} vectors; run `tagger embed --model {MODEL}`")


def _image_vectors(paths: Paths, n: int) -> tuple[np.ndarray | None, np.ndarray]:
    path = paths.emb / "image" / "eg2-full.npy"
    if not path.exists():
        return None, np.zeros(n, bool)
    return np.load(path), np.load(paths.emb / "image" / "eg2-full-mask.npy")


def _keyword_index(records: list[dict]) -> tuple[TfidfVectorizer, object]:
    """TF-IDF over title, URL host and path, metadata and page text (as much as input B reads)."""
    docs = [f"{title}\n{text}" for title, text in page_texts(records, True, MAX_TOKENS * CHAR_CAP)]
    vec = TfidfVectorizer(sublinear_tf=True, stop_words="english", lowercase=True)
    return vec, vec.fit_transform(docs)


def load(paths: Paths, root: Path, encode: Encode) -> tuple[Library, dict]:
    timings = {}
    start = time.perf_counter()
    archive = Archive.load(root)
    records = _records(paths, archive)
    order = {n: i for i, n in enumerate(archive.active.values())}
    counts = {n: 0 for n in order}
    for tags in archive.owner_tags.values():
        for t in tags:
            counts[t] += 1
    tags = sorted(counts, key=lambda n: (-counts[n], n.lower()))
    Y = np.array([[t in archive.owner_tags.get(r["key"], ()) for t in tags] for r in records], np.int8)
    input_name, X = _text_vectors(paths)
    image, has_image = _image_vectors(paths, len(records))
    timings["load_s"] = time.perf_counter() - start

    start = time.perf_counter()
    descriptions_path = paths.data / "zeroshot" / "descriptions.json"
    descriptions = read_json(descriptions_path) if descriptions_path.exists() else {}
    q = encode(query_variants(tags, descriptions)["description"])
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
    )
    stats = {
        "pages": len(records),
        "live_pages": int(lib.live.sum()),
        "labelled_pages": len(fit),
        "tags": len(tags),
        "heads": len(rules.heads),
        "zero_shot_tags": len(tags) - len(rules.heads),
        "image_pages": int(lib.has_image.sum()),
        "input": input_name,
        **{k: round(v, 2) for k, v in timings.items()},
    }
    return lib, stats


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
    S = lib.X @ encode(new).T
    return replace(
        lib,
        tags=[*lib.tags, *new],
        Y=np.hstack([lib.Y, np.zeros((len(lib.Y), len(new)), lib.Y.dtype)]),
        S=np.hstack([lib.S, S]),
        threshold=np.concatenate([lib.threshold, _zero_shot_threshold(S)]),
        spread=np.concatenate([lib.spread, _spread(S)]),
    )


def _ranks(scores: np.ndarray, eligible: np.ndarray) -> np.ndarray:
    """1 based rank of each eligible row by score, 0 elsewhere."""
    rank = np.zeros(len(scores), int)
    rows = np.flatnonzero(eligible)
    rank[rows[np.argsort(-scores[rows], kind="stable")]] = np.arange(1, len(rows) + 1)
    return rank


def search(lib: Library, q: np.ndarray, text: str, n: int, images: bool = False) -> list[dict]:
    """The top `n` live pages by reciprocal rank fusion of EG2 cosine, keyword score and, with `images`, the
    image cosine. Each hit carries every source's score and rank (0: the source did not rank it)."""
    sources = {"text": (lib.X @ q, lib.live)}
    kw = np.asarray((lib.K @ lib.keywords.transform([text]).T).todense()).ravel()
    sources["keyword"] = (kw, lib.live & (kw > 0))
    if images and lib.image is not None:
        sources["image"] = (lib.image @ q, lib.live & lib.has_image)
    fused = np.zeros(len(lib.records))
    ranks = {}
    for name, (scores, eligible) in sources.items():
        ranks[name] = _ranks(scores, eligible)
        fused += np.where(ranks[name] > 0, 1 / (RRF_K + ranks[name]), 0)
    fused[~lib.live] = -np.inf
    top = np.argsort(-fused, kind="stable")[: min(n, int(lib.live.sum()))]
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
