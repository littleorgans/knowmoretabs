"""Test R1: tags as saved searches. How many pages must a user see, and tick, before a search finds
50, 80 and 90% of a tag's library pages, and how well does the saved search rank new pages?

Every owner tag with a positive, queries `name` and `name: description`, inputs eg2 A and B, over the
outer folds of the CV pool. The fit fold is the library: a session (`session.grids`) shows grids and
the oracle ticks the tag's pages among them. The simulated user stops at the stop rule; the replay
keeps going until 90% recall to price each target. The saved search at the stop scores the held out
fold. Baselines: random order (expected, exact), and the query without feedback (zero shot).
"""

import json
import math
from dataclasses import dataclass

import numpy as np

from ..evaluate import labels, outer_folds, split
from ..features import dense
from ..metrics import best_threshold, evaluate
from ..paths import Paths, read_json, write_json
from ..zeroshot import query_variants, query_vectors
from . import oracle, session

MODEL = "eg2"
INPUTS = ("A", "B")
QUERIES = ("name", "description")
ARMS = {"feedback": True, "zeroshot": False}
RECALLS = (0.5, 0.8, 0.9)
TOP = (10, 20)
SUPERVISED_MACRO_AP = 0.572  # eg2-B-lr, CV pool


@dataclass
class Trace:
    shown: list[np.ndarray]  # library positions per grid
    ticks: list[np.ndarray]
    stop: int  # grids seen when the stop rule fired


def need(recall: float, positives: int) -> int:
    return math.ceil(recall * positives - 1e-9)


def replay(X: np.ndarray, q: np.ndarray, ask, positives: int, feedback: bool) -> Trace:
    """Grids until the stop rule has fired and the ticks reach the top recall target (or the library ends)."""
    target = need(max(RECALLS), positives)
    trace = Trace([], [], 0)
    for shown, ticks in session.grids(X, q, ask, feedback):
        trace.shown.append(shown)
        trace.ticks.append(ticks)
        counts = [int(t.sum()) for t in trace.ticks]
        if not trace.stop and session.stops(counts):
            trace.stop = len(counts)
        if trace.stop and sum(counts) >= target:
            break
    trace.stop = trace.stop or len(trace.shown)
    return trace


def precision_at(hits: np.ndarray) -> dict:
    return {f"p_at_{k}": float(hits[:k].mean()) if len(hits) else 0.0 for k in TOP}


def fit_metrics(trace: Trace, positives: int) -> dict:
    """Whole-grid costs. A library with no positives still costs views at stop; recall is undefined."""
    pages = np.cumsum([len(s) for s in trace.shown])
    ticks = np.cumsum([int(t.sum()) for t in trace.ticks])
    at = trace.stop - 1
    out = {
        "stop_grids": trace.stop,
        "stop_pages": int(pages[at]),
        "stop_ticks": int(ticks[at]),
        **precision_at(trace.ticks[0]),
    }
    if not positives:
        return out
    out["stop_recall"] = ticks[at] / positives
    for r in RECALLS:
        g = int(np.argmax(ticks >= need(r, positives)))
        pct = round(100 * r)
        out |= {f"pages_{pct}": int(pages[g]), f"ticks_{pct}": int(ticks[g]), f"within_stop_{pct}": g < trace.stop}
    return out


def random_cost(n: int, positives: int, k: int, grid: int = session.GRID) -> tuple[float, float]:
    """Expected pages and ticks, in whole grids, until a random order shows `k` of `positives` in `n`.
    T, the position of the k-th positive, is negative hypergeometric; the grid holding T is seen whole."""
    t = np.arange(k, n - positives + k + 1)
    lc = np.vectorize(lambda a, b: math.lgamma(a + 1) - math.lgamma(b + 1) - math.lgamma(a - b + 1))
    pmf = np.exp(lc(t - 1, k - 1) + lc(n - t, positives - k) - lc(n, positives))
    seen = np.minimum(grid * np.ceil(t / grid), n)
    ticks = k + (seen - t) * (positives - k) / np.maximum(n - t, 1)
    return float(pmf @ seen), float(pmf @ ticks)


def random_metrics(n: int, positives: int) -> dict:
    out = {f"p_at_{k}": positives / n for k in TOP}
    for r in RECALLS:
        pct = round(100 * r)
        out[f"pages_{pct}"], out[f"ticks_{pct}"] = random_cost(n, positives, need(r, positives))
    return out


def held_out(trace: Trace, X: np.ndarray, q: np.ndarray, docs: np.ndarray, feedback: bool):
    """The saved search at the stop scores held out `docs`; its threshold is the best F1 on the pages seen."""
    rows = np.concatenate(trace.shown[: trace.stop])
    ticked = np.concatenate(trace.ticks[: trace.stop])
    v = session.saved(X, q, rows, ticked) if feedback else q
    s = docs @ v
    return s, s >= best_threshold(X[rows] @ v, ticked)


def tag_means(folds: list[dict]) -> dict | None:
    """Mean over folds where each metric is defined. Stop costs include zero-positive libraries;
    recall targets exclude them. `folds` counts libraries with positives."""
    if not folds:
        return None
    metrics = dict.fromkeys(m for f in folds for m in f if m != "fold")
    return {m: float(np.mean([f[m] for f in folds if m in f])) for m in metrics} | {
        "folds": sum(bool(f["positives"]) for f in folds)
    }


def summarise(per_tag: dict, pooled: dict, group: list[str]) -> dict:
    """Median, mean and total over the group's tags of each fold mean metric; pooled held out macro scores."""
    means = [per_tag[t] for t in group if per_tag[t]]
    out = {"tags": len(group), "tags_scored": sum(bool(x["folds"]) for x in means)}
    for m in dict.fromkeys(m for x in means for m in x):
        values = [x[m] for x in means if m in x]
        out[m] = {"median": float(np.median(values)), "mean": float(np.mean(values)), "total": float(np.sum(values))}
    for m in [m for m in ("ap", "precision", "recall", "f1") if m in pooled[group[0]]]:
        out[f"held_out_macro_{m}"] = float(np.nanmean([pooled[t][m] for t in group]))
    return out


def run_arm(Y, tags, docs, q, folds, pool, feedback, traces, label) -> tuple[dict, dict]:
    """Per tag fold metrics and pooled held out metrics of one arm; each session's grids go to `traces`."""
    position = {row: i for i, row in enumerate(pool)}
    scores = np.zeros((len(pool), len(tags)))
    pred = np.zeros_like(scores, dtype=bool)
    per_tag = {t: [] for t in tags}
    for f, (a, b) in enumerate(folds):
        X = docs[a]
        at = [position[row] for row in b]
        for j, tag in enumerate(tags):
            positives = int(Y[a, j].sum())
            trace = replay(X, q[j], oracle.answers(Y, a, j), positives, feedback)
            s, p = held_out(trace, X, q[j], docs[b], feedback)
            scores[at, j], pred[at, j] = s, p
            row = {
                "fold": f,
                "positives": positives,
                "held_out_positives": int(Y[b, j].sum()),
                **fit_metrics(trace, positives),
                **{f"held_out_{k}": v for k, v in precision_at(Y[b[np.argsort(-s, kind="stable")], j]).items()},
            }
            per_tag[tag].append(row)
            traces.append(
                {
                    "config": label,
                    "fold": f,
                    "tag": j,
                    "stop": trace.stop,
                    "grids": [
                        {"rows": a[g].tolist(), "ticks": t.tolist()}
                        for g, t in zip(trace.shown, trace.ticks, strict=True)
                    ],
                }
            )
    return per_tag, evaluate(Y[pool], scores, pred, tags)["per_tag"]


def random_arm(Y, tags, folds, pool) -> tuple[dict, dict]:
    """Exact random-order costs and pooled AP expectation, with no score ties.
    E[AP] = H_n/n + (m-1)(n-H_n)/(n(n-1)), for n pages and m positives, m > 0.
    Prevalence is AP for constant scores, rather than the random-ranking expectation."""
    per_tag = {
        t: [
            {"fold": f, "positives": int(Y[a, j].sum()), **random_metrics(len(a), int(Y[a, j].sum()))}
            if Y[a, j].any()
            else {"fold": f, "positives": 0}
            for f, (a, _) in enumerate(folds)
        ]
        for j, t in enumerate(tags)
    }
    n = len(pool)
    harmonic = float(np.sum(1.0 / np.arange(1, n + 1)))
    return per_tag, {
        t: {"ap": float(1 if n == 1 else harmonic / n + (m - 1) * (n - harmonic) / (n * (n - 1))) if m else np.nan}
        for t, m in zip(tags, Y[pool].sum(axis=0), strict=True)
    }


def run(paths: Paths) -> None:
    lab = labels(paths)
    pool = np.asarray(split(paths, lab)["train"])  # the CV pool; the test rows are not read here
    folds = outer_folds(lab, pool)
    tags, Y = oracle.owner_matrix(paths, lab)
    groups = {"trained": list(lab.tags), "small": [t for t in tags if t not in lab.tags], "all": tags}
    variants = query_variants(tags, read_json(paths.data / "zeroshot" / "descriptions.json"))
    configs, traces = {}, []
    arms = {"random": random_arm(Y, tags, folds, pool)}
    for input_name in INPUTS:
        docs = dense(paths, MODEL, input_name).X
        for variant in QUERIES:
            q = query_vectors(paths, MODEL, variants[variant])
            name = f"{MODEL}-{input_name}-{variant}"
            for arm, feedback in ARMS.items():
                label = f"{name}-{arm}"
                arms[label] = run_arm(Y, tags, docs, q, folds, pool, feedback, traces, label)
    for label, (per_tag, pooled) in arms.items():
        means = {t: tag_means(per_tag[t]) for t in tags}
        configs[label] = {g: summarise(means, pooled, members) for g, members in groups.items()}
        configs[label]["per_tag"] = {t: {"fold_means": means[t], "held_out": pooled[t]} for t in tags}
    effort = {
        "library_pages": float(np.mean([len(a) for a, _ in folds])),
        "owner_associations_per_library": float(np.mean([Y[a].sum() for a, _ in folds])),
    }
    out = {
        "config": {
            "model": MODEL,
            "inputs": INPUTS,
            "queries": QUERIES,
            "grid": session.GRID,
            "cap": session.CAP,
            "min_ticks": session.MIN_TICKS,
            "recalls": RECALLS,
            "groups": {g: len(m) for g, m in groups.items()},
        },
        "owner_effort": effort,
        "supervised_macro_ap": SUPERVISED_MACRO_AP,
        "configs": configs,
    }
    paths.retrieval.mkdir(parents=True, exist_ok=True)
    write_json(paths.retrieval / "retrieval.json", out)
    with (paths.retrieval / "sessions.jsonl").open("w") as f:
        for t in traces:
            f.write(json.dumps(t) + "\n")
    print(json.dumps({"owner_effort": effort, "groups": out["config"]["groups"]}))
    for label, c in configs.items():
        for g in ("trained", "small"):
            s = c[g]
            print(
                json.dumps(
                    {
                        "config": label,
                        "group": g,
                        "pages_80": {k: round(v, 1) for k, v in s.get("pages_80", {}).items()},
                        "ticks_80": {k: round(v, 1) for k, v in s.get("ticks_80", {}).items()},
                        "held_out_macro_ap": round(s["held_out_macro_ap"], 4),
                    }
                )
            )
