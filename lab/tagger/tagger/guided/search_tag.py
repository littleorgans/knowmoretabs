"""Test S2: search, then tag the results. The user searches; EG2 returns the top N of the library; the user
picks a few of their tags; EG2 decides each picked tag on each result page; the user flips the wrong ones.

Per outer fold of the CV pool, the held out fold is the searched library and only the fit fold's labels
teach (`fit_rules`, `rates`). Queries: every owner tag's `name: description` (variant: the name), eg2 A
and B, N = 20 and 50. The oracle picks the tags held by at least 3 result pages (variant: plus 2 random
wrong tags). Step 3, `decide`, gets the result pages' scores, the picks and the fit fold rules; it has no
way to reach a result page's label. Baseline (blind): the supervised scorer choosing from every owner tag
on the same pages, its flips counted over its suggestions plus the owner tags it missed.
"""

import json
from dataclasses import dataclass

import numpy as np

from ..evaluate import labels, outer_folds, split
from ..features import Dense, dense
from ..heads import SEED, Fitted, fit_lr
from ..metrics import best_threshold
from ..paths import Paths, read_json, write_json
from ..zeroshot import query_variants, query_vectors
from . import oracle

MODEL = "eg2"
INPUTS = ("A", "B")
QUERIES = ("description", "name")
SIZES = (20, 50)
PICKS = {"oracle": 0, "distractors": 2}  # wrong tags the user adds
MIN_HEAD_POSITIVES = 10  # fit positives before a tag gets an LR head; fewer fall back to zero shot
SUGGEST_TOP = (5, 10)
SCORERS = ("zeroshot", "supervised", "in_set")


@dataclass
class Rules:
    """What the fit fold teaches: a zero shot threshold per tag, and LR heads for tags with enough positives.
    `supervised` is the head where a tag has one and zero shot elsewhere."""

    q: np.ndarray  # tag description queries
    heads: np.ndarray  # tag indices with a head
    fitted: Fitted | None
    thresholds: dict[str, np.ndarray]

    def scores(self, X: np.ndarray) -> dict[str, np.ndarray]:
        zs = X @ self.q.T
        sup = zs.copy()
        if self.fitted is not None:
            sup[:, self.heads] = self.fitted.predict_matrix(X)
        return {"zeroshot": zs, "supervised": sup}


def fit_rules(Xfit: np.ndarray, Yfit: np.ndarray, q: np.ndarray) -> Rules:
    """Zero shot thresholds are the best F1 on the fit fold; heads tune C and thresholds on its inner folds."""
    zs = Xfit @ q.T
    zs_threshold = np.array([best_threshold(zs[:, j], Yfit[:, j]) for j in range(Yfit.shape[1])])
    heads = np.flatnonzero(Yfit.sum(axis=0) >= MIN_HEAD_POSITIVES)
    fitted = fit_lr(Dense("fit", Xfit), Yfit[:, heads], np.arange(len(Xfit))) if len(heads) else None
    sup_threshold = zs_threshold.copy()
    if fitted is not None:
        sup_threshold[heads] = fitted.thresholds
    return Rules(q, heads, fitted, {"zeroshot": zs_threshold, "supervised": sup_threshold})


def search(X: np.ndarray, q: np.ndarray, n: int) -> np.ndarray:
    """Step 1: positions of the top `n` pages of `X` by cosine to the query."""
    return np.argsort(-(X @ q), kind="stable")[:n]


def rates(Xfit: np.ndarray, Yfit: np.ndarray, q_search: np.ndarray, n: int, size: int, rng) -> np.ndarray:
    """Per tag, the median share of a result set's pages holding it, over the fit fold sets that pick it.
    The same search and pick replayed on random fit fold slices the size of the held out fold; a tag never
    picked there gets the median over every pick."""
    shares = [[] for _ in range(Yfit.shape[1])]
    for chunk in np.array_split(rng.permutation(len(Xfit)), max(1, round(len(Xfit) / size))):
        for qv in q_search:
            top = chunk[search(Xfit[chunk], qv, n)]
            for t in oracle.picks(Yfit, top):
                shares[t].append(float(Yfit[top, t].mean()))
    every = [s for x in shares for s in x]
    fallback = float(np.median(every)) if every else 0.0
    return np.array([float(np.median(s)) if s else fallback for s in shares])


def decide(scores: dict[str, np.ndarray], rules: Rules, picked: np.ndarray, rate: np.ndarray) -> dict:
    """Step 3: each picked tag on each result page (rows of `scores`), as (pages x picked) decisions.
    zeroshot and supervised: the fit fold threshold. in_set: the supervised score ranks the set's pages,
    and the tag applies to its top max(1, round(rate x N)) pages."""
    out = {name: scores[name][:, picked] >= rules.thresholds[name][picked] for name in ("zeroshot", "supervised")}
    s = scores["supervised"][:, picked]
    k = np.maximum(1, np.rint(rate[picked] * len(s))).astype(int)
    rank = np.argsort(np.argsort(-s, axis=0, kind="stable"), axis=0, kind="stable")
    out["in_set"] = rank < k
    return out


def counts(pred: np.ndarray, truth: np.ndarray, tags: np.ndarray) -> dict:
    """Decision counts of one set: flips are the decisions the user changes (false and missed tags)."""
    truth = truth.astype(bool)
    flips = (pred != truth).sum(axis=1)
    return {
        "tp": int((pred & truth).sum()),
        "fp": int((pred & ~truth).sum()),
        "fn": int((~pred & truth).sum()),
        "pages": len(pred),
        "flips": int(flips.sum()),
        "zero_flip_pages": int((flips == 0).sum()),
        "per_tag": {
            int(t): [int((p & y).sum()), int((p & ~y).sum()), int((~p & y).sum())]
            for t, p, y in zip(tags, pred.T, truth.T, strict=True)
        },
    }


def f1(tp: int, fp: int, fn: int) -> float:
    """1.0 when there is nothing to find and nothing was predicted (a distractor rightly left empty)."""
    return 2 * tp / (2 * tp + fp + fn) if tp + fp + fn else 1.0


def suggest_recall(z: np.ndarray, picked: np.ndarray) -> dict:
    """The model's tag suggestions for a set: tags ranked by mean z score over its pages."""
    order = np.argsort(-z.mean(axis=0), kind="stable")
    return {f"recall_at_{k}": len(np.intersect1d(order[:k], picked)) / len(picked) for k in SUGGEST_TOP}


def simulate_fold(Y, docs, q_tags, q_search, a, b, f) -> list[dict]:
    """Every query and size on one outer fold; one record per result set with at least one oracle pick."""
    rules = fit_rules(docs[a], Y[a], q_tags)
    held = rules.scores(docs[b])
    sup = held["supervised"]
    z = (sup - sup.mean(axis=0)) / np.maximum(sup.std(axis=0), 1e-12)  # against the searched library
    blind = sup >= rules.thresholds["supervised"]
    every = np.arange(Y.shape[1])
    sets = []
    for n in SIZES:
        rate = rates(docs[a], Y[a], q_search, n, len(b), np.random.default_rng([SEED, f, n]))
        for j, qv in enumerate(q_search):
            top = search(docs[b], qv, n)
            rows = b[top]
            picked = oracle.picks(Y, rows)
            if not len(picked):
                sets.append({"fold": f, "query": j, "n": n, "picked": 0})
                continue
            wrong = oracle.distractors(picked, Y.shape[1], PICKS["distractors"], np.random.default_rng([SEED, f, n, j]))
            record = {
                "fold": f,
                "query": j,
                "n": n,
                "rows": rows.tolist(),
                "picked": len(picked),
                "distractors": wrong.tolist(),
                "unpicked_owner_tags": int(Y[rows].sum() - Y[np.ix_(rows, picked)].sum()),
                "suggest": suggest_recall(z[top], picked),
                "blind": counts(blind[top], Y[rows], every),
            }
            for pick, extra in PICKS.items():
                chosen = np.concatenate([picked, wrong[:extra]]).astype(int)
                decisions = decide({k: v[top] for k, v in held.items()}, rules, chosen, rate)
                truth = Y[np.ix_(rows, chosen)]
                record[pick] = {name: counts(decisions[name], truth, chosen) for name in SCORERS}
            sets.append(record)
    return sets


def aggregate(cs: list[dict]) -> dict:
    """Pooled over sets (macro F1 pools each tag's counts first) and the median over sets."""
    tp, fp, fn, pages, flips, zero = (
        sum(c[k] for c in cs) for k in ("tp", "fp", "fn", "pages", "flips", "zero_flip_pages")
    )
    per_tag = {}
    for c in cs:
        for t, v in c["per_tag"].items():
            per_tag[t] = np.add(per_tag.get(t, 0), v)

    def macro(c):
        return float(np.mean([f1(*v) for v in c["per_tag"].values()]))

    return {
        "micro_f1": f1(tp, fp, fn),
        "macro_f1": float(np.mean([f1(*v) for v in per_tag.values()])),
        "flips_per_page": flips / pages,
        "zero_flip_share": zero / pages,
        "median": {
            "micro_f1": float(np.median([f1(c["tp"], c["fp"], c["fn"]) for c in cs])),
            "macro_f1": float(np.median([macro(c) for c in cs])),
            "flips_per_page": float(np.median([c["flips"] / c["pages"] for c in cs])),
            "zero_flip_share": float(np.median([c["zero_flip_pages"] / c["pages"] for c in cs])),
        },
    }


def summarise(sets: list[dict]) -> dict:
    scored = [s for s in sets if s["picked"]]
    picked = [s["picked"] for s in sets]
    pages = sum(len(s["rows"]) for s in scored)
    out = {
        "sets": len(sets),
        "sets_with_picks": len(scored),
        "picked_per_set": {"mean": float(np.mean(picked)), "median": float(np.median(picked))},
        "picked_per_scored_set": {"mean": float(np.mean([s["picked"] for s in scored]))},
        "unpicked_owner_tags_per_page": sum(s["unpicked_owner_tags"] for s in scored) / pages,
        "suggest": {
            m: {"mean": float(np.mean(v)), "median": float(np.median(v))}
            for m in scored[0]["suggest"]
            for v in [[s["suggest"][m] for s in scored]]
        },
        "blind": aggregate([s["blind"] for s in scored]),
    }
    for pick in PICKS:
        out[pick] = {name: aggregate([s[pick][name] for s in scored]) for name in SCORERS}
    return out


def run(paths: Paths) -> None:
    lab = labels(paths)
    pool = np.asarray(split(paths, lab)["train"])  # the CV pool; the test rows are not read here
    folds = outer_folds(lab, pool)
    tags, Y = oracle.owner_matrix(paths, lab)
    variants = query_variants(tags, read_json(paths.data / "zeroshot" / "descriptions.json"))
    q = {v: query_vectors(paths, MODEL, variants[v]) for v in QUERIES}
    configs = {}
    paths.search_tag.mkdir(parents=True, exist_ok=True)
    with (paths.search_tag / "sets.jsonl").open("w") as out:
        for input_name in INPUTS:
            docs = dense(paths, MODEL, input_name).X
            for variant in QUERIES:
                sets = [
                    s
                    for f, (a, b) in enumerate(folds)
                    for s in simulate_fold(Y, docs, q["description"], q[variant], a, b, f)
                ]
                for s in sets:
                    out.write(json.dumps({"input": input_name, "query_variant": variant, **s}) + "\n")
                for n in SIZES:
                    configs[f"{MODEL}-{input_name}-{variant}-n{n}"] = summarise([s for s in sets if s["n"] == n])
    result = {
        "config": {
            "model": MODEL,
            "inputs": INPUTS,
            "queries": QUERIES,
            "sizes": SIZES,
            "picks": PICKS,
            "min_pick_pages": oracle.MIN_PICK_PAGES,
            "min_head_positives": MIN_HEAD_POSITIVES,
            "owner_tags": len(tags),
            "folds": len(folds),
        },
        "configs": configs,
    }
    write_json(paths.search_tag / "search_tag.json", result)
    for label, c in configs.items():
        line = {"config": label, "picked": round(c["picked_per_scored_set"]["mean"], 2), "sets": c["sets_with_picks"]}
        line |= {f"{s}_flips": round(c["oracle"][s]["flips_per_page"], 3) for s in SCORERS}
        line["blind_flips"] = round(c["blind"]["flips_per_page"], 3)
        print(json.dumps(line))
