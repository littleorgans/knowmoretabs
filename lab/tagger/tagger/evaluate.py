"""Step 3: a fixed 20% test split, a 5 fold CV grid on the other 80%, then one test score.

Leakage boundary: `cross_validate` receives only the training rows; `score_test_once` is the only
reader of the test rows, and it runs after the pick is made from CV results alone.
"""

import json
import time
from dataclasses import dataclass

import numpy as np
from iterstrat.ml_stratifiers import MultilabelStratifiedKFold, MultilabelStratifiedShuffleSplit

from . import dataset, features
from .embed import MODELS
from .heads import HEADS, SEED
from .metrics import evaluate
from .paths import Paths, read_json, write_json

OUTER_FOLDS = 5
TEST_SIZE = 0.2
GRID_HEADS = ("lr", "knn")
BASELINES = ("prior", "tfidf-title-lr")


@dataclass
class Labels:
    records: list
    tags: list
    Y: np.ndarray  # all known pages x trained tags
    labelled: np.ndarray  # row indices of owner labelled pages


def labels(paths: Paths) -> Labels:
    records, tags = dataset.load(paths)
    trained = tags["trained"]
    Y = np.array([[t in r["tags"] for t in trained] for r in records], dtype=np.int8)
    labelled = np.array([i for i, r in enumerate(records) if r["labelled"]])
    return Labels(records, trained, Y, labelled)


def split(paths: Paths, lab: Labels) -> dict:
    """Iterative multilabel stratification over the trained tags, seeded, computed once."""
    path = paths.eval / "split.json"
    if path.exists():
        return read_json(path)
    splitter = MultilabelStratifiedShuffleSplit(n_splits=1, test_size=TEST_SIZE, random_state=SEED)
    a, b = next(splitter.split(lab.labelled, lab.Y[lab.labelled]))
    out = {"train": sorted(lab.labelled[a].tolist()), "test": sorted(lab.labelled[b].tolist()), "seed": SEED}
    write_json(path, out)
    return out


def feature(paths: Paths, lab: Labels, name: str):
    """A config name is `<model>-<input>-<head>` or a baseline name."""
    if name == "prior":
        return features.Dense("prior", np.zeros((len(lab.records), 1), dtype=np.float32)), "prior"
    if name == "tfidf-title-lr":
        return features.TfidfTitle([r["title"] for r in lab.records]), "lr"
    model, input_name, head = name.split("-")
    return features.dense(paths, model, input_name), head


def grid(paths: Paths) -> list[str]:
    names = list(BASELINES)
    for model in MODELS:
        names += [f"{model}-{i}-{h}" for i in features.available(paths, model) for h in GRID_HEADS]
    return names


def outer_folds(lab: Labels, rows) -> list[tuple[np.ndarray, np.ndarray]]:
    """The seeded outer folds over `rows`, as (fit rows, held out rows) dataset indices."""
    rows = np.asarray(rows)
    folds = MultilabelStratifiedKFold(n_splits=OUTER_FOLDS, shuffle=True, random_state=SEED)
    return [(rows[a], rows[b]) for a, b in folds.split(rows, lab.Y[rows])]


def fit_and_score(feat, head: str, lab: Labels, fit_rows, apply_rows):
    fitted = HEADS[head](feat, lab.Y, np.asarray(fit_rows))
    scores = fitted.predict(feat, np.asarray(apply_rows))
    return fitted, scores, scores >= fitted.thresholds


def cross_validate(paths: Paths, lab: Labels, name: str, train: list[int]) -> dict:
    out_path = paths.eval / "cv" / f"{name}.json"
    if out_path.exists():
        return read_json(out_path)
    feat, head = feature(paths, lab, name)
    train = np.asarray(train)
    position = {row: i for i, row in enumerate(train)}
    scores = np.zeros((len(train), len(lab.tags)))
    pred = np.zeros_like(scores, dtype=bool)
    fold_macro_f1, params = [], []
    start = time.perf_counter()
    for a, b in outer_folds(lab, train):
        fitted, s, p = fit_and_score(feat, head, lab, a, b)
        at = [position[row] for row in b]
        scores[at], pred[at] = s, p
        fold_macro_f1.append(evaluate(lab.Y[b], s, p, lab.tags)["macro"]["f1"])
        params.append(fitted.params)
    result = evaluate(lab.Y[train], scores, pred, lab.tags)
    result.update(name=name, fold_macro_f1=fold_macro_f1, params=params, seconds=round(time.perf_counter() - start, 1))
    write_json(out_path, result)
    print(
        json.dumps(
            {
                "cv": name,
                "micro_f1": round(result["micro"]["f1"], 4),
                "macro_f1": round(result["macro"]["f1"], 4),
                "macro_ap": round(result["macro"]["ap"], 4),
                "seconds": result["seconds"],
            }
        )
    )
    return result


def pick(results: dict) -> str:
    candidates = [n for n in results if n not in BASELINES]
    return max(candidates, key=lambda n: (results[n]["macro"]["f1"], results[n]["macro"]["ap"]))


def score_test_once(paths: Paths, lab: Labels, chosen: list[str], sp: dict) -> dict:
    """The only reader of the test rows: fit on all training rows, score the test rows."""
    out = {}
    for name in chosen:
        feat, head = feature(paths, lab, name)
        fitted, scores, pred = fit_and_score(feat, head, lab, sp["train"], sp["test"])
        result = evaluate(lab.Y[sp["test"]], scores, pred, lab.tags)
        out[name] = {**result, "params": fitted.params}
    return out


def run(paths: Paths, final: bool) -> None:
    lab = labels(paths)
    sp = split(paths, lab)
    results = {name: cross_validate(paths, lab, name, sp["train"]) for name in grid(paths)}
    best = pick(results)
    summary = {
        n: {"micro": r["micro"], "macro": r["macro"], "fold_macro_f1_std": float(np.std(r["fold_macro_f1"]))}
        for n, r in results.items()
    }
    write_json(
        paths.eval / "cv_summary.json",
        {"best": best, "configs": summary, "train_pages": len(sp["train"]), "test_pages": len(sp["test"])},
    )
    print(json.dumps({"best": best, "train_pages": len(sp["train"]), "test_pages": len(sp["test"])}))
    if not final:
        return
    path = paths.eval / "final.json"
    if path.exists() and read_json(path)["best"] != best:
        raise SystemExit("final.json holds a different pick; the test split is scored once")
    test = score_test_once(paths, lab, [best, *BASELINES], sp)
    write_json(path, {"best": best, "test": test})
    for name, r in test.items():
        print(
            json.dumps(
                {
                    "test": name,
                    "micro_f1": round(r["micro"]["f1"], 4),
                    "macro_f1": round(r["macro"]["f1"], 4),
                    "micro_ap": round(r["micro"]["ap"], 4),
                    "macro_ap": round(r["macro"]["ap"], 4),
                }
            )
        )
