"""Guided discovery gate (design D1, its cheapest test): do a tag's description query and 20 or fewer
owner answers per tag reach 0.51 macro AP on eg2-B?

The `zeroshot.curve` setup: the outer folds of the CV pool, each fit fold being the library. Per tag,
pages are chosen from the library at random (3 seeds) or by doubt (batches of 5, refitting after each);
the oracle answers for the chosen pages only. Held out rows are only scored.
"""

import json

import numpy as np

from ..evaluate import labels, outer_folds, split
from ..features import dense
from ..heads import SEED
from ..metrics import evaluate
from ..paths import Paths, read_json, write_json
from ..zeroshot import query_variants, query_vectors
from . import model

MODEL, INPUT, QUERY = "eg2", "B", "description"
BUDGETS = (0, 5, 10, 20)
BATCH = 5
SEEDS = 3
PASS_MACRO_AP = 0.51


def oracle(Y: np.ndarray, library: np.ndarray, j: int):
    """The owner's answer on tag `j` for chosen library positions; the scorer sees nothing else of Y."""
    return lambda rows: Y[library[rows], j].astype(bool)


def doubt_order(lib: model.Library, q: np.ndarray, ask, budget: int) -> tuple[np.ndarray, np.ndarray]:
    """The most doubtful unlabelled library pages, a batch at a time, refitting on the answers so far."""
    rows, y = np.array([], int), np.array([], bool)
    while len(rows) < budget:
        d = model.fit(lib, q, rows, y).doubt(lib.X)
        d[rows] = -np.inf
        pick = np.argsort(-d, kind="stable")[:BATCH]
        rows, y = np.concatenate([rows, pick]), np.concatenate([y, ask(pick)])
    return rows, y


def random_order(size: int, ask, budget: int, rng) -> tuple[np.ndarray, np.ndarray]:
    rows = rng.choice(size, budget, replace=False)
    return rows, ask(rows)


def replay(Y: np.ndarray, tags: list[str], docs: np.ndarray, q: np.ndarray, folds) -> dict:
    """Metrics per (arm, seed, budget), pooled over the held out folds; budgets are prefixes of one order."""
    pool = np.concatenate([b for _, b in folds])
    position = {row: i for i, row in enumerate(pool)}
    runs = [("doubt", 0)] + [("random", s) for s in range(SEEDS)]
    keys = [(arm, seed, n, blend) for arm, seed in runs for n in BUDGETS for blend in (True, False)]
    scores = {k: np.zeros((len(pool), len(tags))) for k in keys}
    pred = {k: np.zeros((len(pool), len(tags)), bool) for k in keys}
    positives = {(arm, seed, n): [] for arm, seed in runs for n in BUDGETS}
    for f, (a, b) in enumerate(folds):
        lib = model.Library.of(docs[a])
        at = [position[row] for row in b]
        for j in range(len(tags)):
            ask = oracle(Y, a, j)
            for arm, seed in runs:
                if arm == "doubt":
                    rows, y = doubt_order(lib, q[j], ask, max(BUDGETS))
                else:
                    rows, y = random_order(len(a), ask, max(BUDGETS), np.random.default_rng([SEED, seed, f, j]))
                for n in BUDGETS:
                    positives[(arm, seed, n)].append(int(y[:n].sum()))
                    for blend in (True, False):
                        scorer = model.fit(lib, q[j], rows[:n], y[:n], blend)
                        s = scorer.score(docs[b])
                        scores[(arm, seed, n, blend)][at, j] = s
                        pred[(arm, seed, n, blend)][at, j] = s >= scorer.threshold
    out = {}
    for arm, seed in runs:
        for n in BUDGETS:
            blended = evaluate(Y[pool], scores[(arm, seed, n, True)], pred[(arm, seed, n, True)], tags)
            alone = evaluate(Y[pool], scores[(arm, seed, n, False)], pred[(arm, seed, n, False)], tags)
            out[(arm, seed, n)] = {
                "macro_ap": blended["macro"]["ap"],
                "macro_f1": blended["macro"]["f1"],
                "micro_f1": blended["micro"]["f1"],
                "prototype_macro_ap": alone["macro"]["ap"],
                "prototype_macro_f1": alone["macro"]["f1"],
                "positives_per_tag": float(np.mean(positives[(arm, seed, n)])),
            }
    return out


def summarise(by_run: dict) -> dict:
    """Mean and SD over seeds per arm and budget."""
    table = {}
    for arm in ("random", "doubt"):
        for n in BUDGETS:
            runs = [m for (a, _, k), m in by_run.items() if a == arm and k == n]
            table[f"{arm}-{n}"] = {
                "arm": arm,
                "labels_per_tag": n,
                "seeds": len(runs),
                **{f"{m}_mean": float(np.mean([r[m] for r in runs])) for m in runs[0]},
                **{f"{m}_sd": float(np.std([r[m] for r in runs])) for m in runs[0]},
            }
    return table


def run(paths: Paths) -> None:
    lab = labels(paths)
    pool = np.asarray(split(paths, lab)["train"])  # the CV pool; the test rows are not read here
    folds = outer_folds(lab, pool)
    texts = query_variants(lab.tags, read_json(paths.data / "zeroshot" / "descriptions.json"))[QUERY]
    q = query_vectors(paths, MODEL, texts)
    table = summarise(replay(lab.Y, lab.tags, dense(paths, MODEL, INPUT).X, q, folds))
    best = max((r for r in table.values() if r["labels_per_tag"] > 0), key=lambda r: r["macro_ap_mean"])
    out = {
        "config": {"model": MODEL, "input": INPUT, "query": QUERY, "budgets": BUDGETS, "batch": BATCH},
        "table": table,
        "pass_macro_ap": PASS_MACRO_AP,
        "passed": best["macro_ap_mean"] >= PASS_MACRO_AP,
        "best": f"{best['arm']}-{best['labels_per_tag']}",
        "trained_tags": len(lab.tags),
        "tag_answers_at_max_budget": len(lab.tags) * max(BUDGETS),
        "owner_associations_per_library": float(
            np.mean([sum(len(lab.records[i]["tags"]) for i in a) for a, _ in folds])
        ),
        "owner_trained_associations_per_library": float(np.mean([lab.Y[a].sum() for a, _ in folds])),
        "library_pages": float(np.mean([len(a) for a, _ in folds])),
    }
    write_json(paths.guided / "gate.json", out)
    for name, r in table.items():
        print(json.dumps({"gate": name, **{m: round(r[f"{m}_mean"], 4) for m in ("macro_ap", "macro_f1")}}))
    print(json.dumps({k: out[k] for k in ("passed", "best", "tag_answers_at_max_budget")}))
