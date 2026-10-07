"""Step 4: suggestions from the best config as `tag --import` JSONL, then a dry run import.

Labelled pages get out of fold predictions (5 folds over all labelled pages); pages without owner
tags get predictions from a fit on every labelled page. Only tags the page lacks are kept.
"""

import csv
import json
import shutil
import subprocess
from pathlib import Path

import numpy as np

from .evaluate import feature, fit_and_score, labels, outer_folds
from .metrics import evaluate
from .paths import Paths, read_json, write_json

REPO = Path(__file__).resolve().parents[3]
BINARY = REPO / "target" / "release" / "knowmoretabs"


def source_name(best: str) -> str:
    return f"kmt-tagger-{best}"


def predictions(paths: Paths, lab, best: str):
    """Scores and decisions for every known page: OOF for labelled rows, full fit for the rest."""
    feat, head = feature(paths, lab, best)
    n = len(lab.records)
    scores = np.full((n, len(lab.tags)), np.nan)
    pred = np.zeros((n, len(lab.tags)), dtype=bool)
    thresholds = np.full((n, len(lab.tags)), np.nan)
    rows = lab.labelled
    for a, b in outer_folds(lab, rows):
        fitted, s, p = fit_and_score(feat, head, lab, a, b)
        scores[b], pred[b], thresholds[b] = s, p, fitted.thresholds
    rest = np.array([i for i in range(n) if i not in set(rows.tolist())])
    if len(rest):
        fitted, s, p = fit_and_score(feat, head, lab, rows, rest)
        scores[rest], pred[rest], thresholds[rest] = s, p, fitted.thresholds
    return scores, pred, thresholds


def dry_run(paths: Paths, answers: Path, source: str) -> dict:
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=REPO, check=True, capture_output=True)
    if not paths.import_check.exists():
        shutil.copytree(paths.snapshot, paths.import_check, symlinks=True)
    done = subprocess.run(
        [
            str(BINARY),
            "--root",
            str(paths.import_check),
            "--json",
            "tag",
            "--import",
            str(answers),
            "--dry-run",
            "--source",
            source,
        ],
        capture_output=True,
        text=True,
    )
    # stderr may name a page, so it is kept beside the report and never printed.
    (paths.out / "import-dry-run.stderr.txt").write_text(done.stderr)
    report = {"exit_code": done.returncode, "report": json.loads(done.stdout) if done.returncode == 0 else None}
    write_json(paths.out / "import-dry-run.json", report)
    return report


def run(paths: Paths) -> None:
    best = read_json(paths.eval / "final.json")["best"]
    lab = labels(paths)
    scores, pred, thresholds = predictions(paths, lab, best)
    oof = evaluate(lab.Y[lab.labelled], scores[lab.labelled], pred[lab.labelled], lab.tags)
    paths.out.mkdir(parents=True, exist_ok=True)
    answers = paths.out / f"{source_name(best)}.jsonl"
    side = paths.out / f"{source_name(best)}-scores.csv"
    pages = associations = 0
    with answers.open("w") as jsonl, side.open("w", newline="") as f:
        table = csv.writer(f)
        table.writerow(["url", "title", "owner_tags", "suggested_tag", "score", "threshold", "prediction"])
        for i, r in enumerate(lab.records):
            owned = set(r["tags"])
            tags = sorted(t for j, t in enumerate(lab.tags) if pred[i, j] and t not in owned)
            if not tags:
                continue
            pages += 1
            associations += len(tags)
            jsonl.write(json.dumps({"url": r["key"], "tags": tags}, ensure_ascii=False) + "\n")
            for j, t in enumerate(lab.tags):
                if t in tags:
                    table.writerow(
                        [
                            r["key"],
                            r["title"],
                            "; ".join(r["tags"]),
                            t,
                            f"{scores[i, j]:.4f}",
                            f"{thresholds[i, j]:.4f}",
                            "out of fold" if r["labelled"] else "full fit",
                        ]
                    )
    report = dry_run(paths, answers, source_name(best))
    counts = (pred & ~lab.Y.astype(bool)).sum(axis=0)
    summary = {
        "best": best,
        "source": source_name(best),
        "pages_with_suggestions": pages,
        "suggested_associations": associations,
        "labelled_pages_with_suggestions": sum(
            1
            for i in lab.labelled
            if any(pred[i, j] and t not in lab.records[i]["tags"] for j, t in enumerate(lab.tags))
        ),
        "dry_run": report,
        "oof_micro_precision_proxy": oof["micro"]["precision"],
        "suggestion_tag_weighted_precision_proxy": (
            float(sum(counts[j] * oof["per_tag"][t]["precision"] for j, t in enumerate(lab.tags)) / associations)
            if associations else None
        ),
    }
    write_json(paths.out / "suggest-summary.json", summary)
    print(json.dumps(summary))
