"""Cross tag ranking, nested training cuts and paired comparisons on covered C pages.

Model and cut fitting never sees an outer held out label. The two ranking choices
for abstention and the best fusion scorer are exploratory selections by outer P.
"""

import json
import shlex
import subprocess
import traceback
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import numpy as np

from . import tag_eval, tag_grid
from .app import kmt
from .archive import sha256_hex
from .heads import SEED, inner_folds
from .paths import Paths, read_json, write_json
from .search_eval import Invalid, _files, _safe_paths, fingerprint

STRATEGIES = (*tag_eval.APP_RANKINGS, "knn")
TARGETS = (1.0, 1.5, 2.0)
CURVE_TARGETS = tuple(i / 4 for i in range(1, 13))


def fit_views(corpus, train, held, strategies):
    """App ranking variants share one fit, preserving identical scores and thresholds."""
    fits = {}
    views = {}
    for strategy in strategies:
        base = "app" if strategy in tag_eval.APP_RANKINGS else strategy
        if base not in fits:
            fits[base] = tag_eval.STRATEGIES[base](
                corpus.X[train], corpus.Y[train], corpus.q, corpus.tags, corpus.X[held]
            )
        views[strategy] = fits[base]
    return views


def choose_cut(scores, pages, target):
    """Closest training OOF suggestion volume; ties prefer the higher cut.

    A global score cut keeps complete ties and may only approximate the target.
    """
    values = np.sort(np.asarray(scores, dtype=float))
    if pages <= 0 or not len(values) or not np.isfinite(values).all():
        raise Invalid("finite training suggestions and positive page count required")
    cuts = np.r_[np.unique(values), np.nextafter(values[-1], np.inf)]
    shown = (len(values) - np.searchsorted(values, cuts, side="left")) / pages
    index = min(range(len(cuts)), key=lambda i: (abs(shown[i] - target), -cuts[i]))
    return {"cut": float(cuts[index]), "training_shown_per_page": float(shown[index])}


def training_cuts(corpus, saved, strategies):
    """Inner validation suggestions from only each outer training pool choose cuts."""
    cuts = {strategy: [] for strategy in strategies}
    for fold, _, train, _ in tag_eval.fold_indices(corpus, saved):
        pool = train[corpus.Y[train].any(axis=1)]
        scores = {strategy: [] for strategy in strategies}
        traces = []
        for inner, (_, held) in enumerate(inner_folds(corpus.Y, pool)):
            fitting = train[~np.isin(train, held)]
            views = fit_views(corpus, fitting, held, strategies)
            for strategy, view in views.items():
                scores[strategy].extend(
                    h["z"] for i in range(len(held)) for h in tag_eval.rank_suggestions(view, i, strategy)[: tag_eval.K]
                )
            traces.append(
                {
                    "inner_fold": inner,
                    "training_pages": len(fitting),
                    "validation_ids": [sha256_hex(corpus.records[i]["key"]) for i in held],
                }
            )
        for strategy in strategies:
            cuts[strategy].append(
                {
                    "fold": fold,
                    "training_labelled_pages": len(pool),
                    "inner_folds": traces,
                    "operating_points": [
                        {"target": target, **choose_cut(scores[strategy], len(pool), target)}
                        for target in CURVE_TARGETS
                    ],
                }
            )
        print(json.dumps({"calibration_fold": fold, "strategies": strategies}), flush=True)
    return cuts


def with_predictions(corpus, row, predictions):
    """Recompute the same group metrics for a subset or for fewer shown suggestions."""
    columns = {t: j for j, t in enumerate(corpus.tags)}
    Y = np.zeros((len(predictions), len(columns)), bool)
    pred = np.zeros_like(Y)
    heads = np.zeros_like(Y)
    for i, page in enumerate(predictions):
        Y[i, [columns[t] for t in page["truth"]]] = True
        pred[i, [columns[h["tag"]] for h in page["top_3"]]] = True
        heads[i] = row["folds"][page["fold"]]["has_head"]
    metrics = tag_eval.summaries(Y, pred, heads, corpus.Y.sum(axis=0) >= tag_eval.MIN_HEAD_POSITIVES)
    counts = metrics["overall"]["counts"]
    return {
        "page_source": row["page_source"],
        "strategy": row["strategy"],
        "metrics": metrics,
        "predictions": predictions,
        "shown_per_page": counts["predictions"] / len(predictions) if predictions else 0.0,
        "none_shown_share": 1 - counts["prediction_pages"] / len(predictions) if predictions else 0.0,
    }


def abstention(corpus, rows, cuts):
    points = {}
    for strategy, folds in cuts.items():
        row = rows[f"app/{strategy}"]
        for index, operating in enumerate(folds[0]["operating_points"]):
            target = operating["target"]
            by_fold = {str(f["fold"]): f["operating_points"][index]["cut"] for f in folds}
            predictions = [
                {**p, "top_3": [h for h in p["top_3"] if h["z"] >= by_fold[str(p["fold"])]]} for p in row["predictions"]
            ]
            points[f"app/{strategy}@{target}"] = {
                **with_predictions(corpus, row, predictions),
                "target": target,
                "cuts_by_fold": by_fold,
            }
    return points


def evaluate(app, cmean, saved, baseline):
    tag_grid.validate_baseline(app, saved, baseline)
    fitted = {strategy: {} for strategy in STRATEGIES}
    for fold, _, train, held in tag_eval.fold_indices(app, saved):
        for strategy, view in fit_views(app, train, held, STRATEGIES).items():
            fitted[strategy][fold] = view
    rows = {}
    for strategy in STRATEGIES:
        row = tag_eval.cross_validate(app, saved, strategy, fitted_folds=fitted[strategy])
        row.update(page_source="app", strategy=strategy)
        row["paired_vs_baseline"] = tag_grid.paired_bootstrap(row, baseline)
        rows[f"app/{strategy}"] = row
    if rows["app/app"]["metrics"] != baseline["metrics"] or rows["app/app"]["predictions"] != baseline["predictions"]:
        raise Invalid("recomputed app ranking or metrics differ from baseline")
    alternatives = sorted(
        STRATEGIES[1:], key=lambda s: -rows[f"app/{s}"]["metrics"]["overall"]["macro"]["precision_at_3"]
    )
    best = alternatives[0]
    cuts = training_cuts(app, saved, ["app", *alternatives[:2]])
    points = abstention(app, rows, cuts)
    if cmean.chunk_coverage is None or not cmean.chunk_coverage[app.labelled].any():
        raise Invalid("evaluated C coverage required")
    covered = {sha256_hex(app.records[i]["key"]) for i in app.labelled if cmean.chunk_coverage[i]}
    c_row = tag_eval.cross_validate(cmean, saved, "app")
    c_row.update(page_source="cmean", strategy="app")
    restricted = {}
    for name, row in (("app/app covered", rows["app/app"]), ("cmean/app covered", c_row)):
        restricted[name] = with_predictions(app, row, [p for p in row["predictions"] if p["id"] in covered])
        restricted[name]["paired_vs_baseline"] = tag_grid.paired_bootstrap(
            restricted[name], restricted["app/app covered"]
        )
    bc = tag_eval.bc_corpus(app, cmean)
    bc_fitted = {strategy: {} for strategy in ("app", best)}
    for fold, _, train, held in tag_eval.fold_indices(bc, saved):
        for strategy, view in fit_views(bc, train, held, bc_fitted).items():
            bc_fitted[strategy][fold] = view
    for strategy in bc_fitted:
        row = tag_eval.cross_validate(bc, saved, strategy, fitted_folds=bc_fitted[strategy])
        row.update(page_source="bc", strategy=strategy)
        row["paired_vs_baseline"] = tag_grid.paired_bootstrap(row, baseline)
        restricted[f"bc/{strategy}"] = row
    all_rows = {**rows, **{name: row for name, row in restricted.items() if name.startswith("bc/")}}
    return {
        "rows": rows,
        "abstention": points,
        "training_cuts": cuts,
        "abstention_strategies": ["app", *alternatives[:2]],
        "fair_c": restricted,
        "best_b_scorer": best,
        "best_row": max(all_rows, key=lambda s: all_rows[s]["metrics"]["overall"]["macro"]["precision_at_3"]),
        "inputs": {"app": app.metadata, "cmean": cmean.metadata, "bc": bc.metadata},
    }


def report(result):
    lines = [
        f"5 saved page folds; {result['counts']['evaluation_pages']} evaluated pages; top 3. P/R/H and groups follow grid.md; all rates in %.",
        "Heads only: app z among heads. Heads first: threshold passing heads by app z, then fallback by app z. Margin: raw score minus app threshold, all tags. kNN: raw weighted shares; k tuned on inner folds of tagged training rows.",
        f"Rare thresholds: app best F1 on tagged training rows; unseen: app training mean + {tag_eval.engine.ZERO_K_SD:g} SD. All ranking references use training pages only. Δ: paired page bootstrap, {tag_grid.BOOTSTRAP_SAMPLES} draws, seed {SEED}, 95% intervals, pp.",
        *tag_grid.grid_table(result["rows"]),
        "Abstention: score cuts fit to inner OOF top 3 volume on each outer training pool; full ties kept. Choices of two scorers and best row use outer P and are exploratory. Shown P is pooled; R columns include pages with none.",
        "| Scorer | Target/page | Shown/page | Shown P % | Macro R % | Micro R % | None % |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in result["abstention"].values():
        if row["target"] not in TARGETS:
            continue
        m = row["metrics"]["overall"]
        lines.append(
            f"| {row['strategy']} | {row['target']:.1f} | {row['shown_per_page']:.3f} | {100 * m['micro']['precision_at_3']:.2f} | {100 * m['macro']['recall_at_3']:.2f} | {100 * m['micro']['recall_at_3']:.2f} | {100 * row['none_shown_share']:.2f} |"
        )
    covered = result["fair_c"]["app/app covered"]["metrics"]["overall"]["counts"]["pages"]
    lines += [
        f"Fair C: covered pair has {covered} identical evaluated pages; models still train on all training pages. Covered Δ uses covered app/app; bc Δ uses all page app/app. bc=unit(B+cmean), missing C retains B; heads refit per source.",
        *tag_grid.grid_table(result["fair_c"]),
        f"Best deployable fixed top 3 row: {result['best_row']}. Untagged pages remain training reference only; missing owner labels count negative. No app behavior changed.",
        f"Run: `{result['command']}`",
        "Checks: see calib-logs/checks.json and calib-logs/verification.json; source commit and full 0.25 to 3.0 training volume curves are in calib.json.",
    ]
    return "\n".join(lines) + "\n"


def input_snapshot(root: Path, data: Path):
    """Content, membership and timestamps, including lock metadata, for frozen inputs."""
    roots = [root] if data.resolve().is_relative_to(root.resolve()) else [root, data]
    return {
        str(folder): {
            "content": fingerprint(_files(folder)),
            "metadata": fingerprint(
                [
                    (str(p.relative_to(folder)), p.stat().st_size, p.stat().st_mtime_ns)
                    for p in sorted(folder.rglob("*"))
                ]
            ),
        }
        for folder in roots
    }


def run(paths: Paths, root: Path, split: Path, out: Path, baseline: Path, *, binary=kmt.BINARY):
    _safe_paths(paths, root, out)
    if not split.is_file():
        raise Invalid("calibration requires an existing saved split")
    if any((out / name).exists() for name in ("calib.json", "calib.md")):
        raise Invalid("calibration outputs already exist; choose a separate output directory")
    logs = out / "calib-logs"
    logs.mkdir(parents=True, exist_ok=True)
    with (logs / "tag-calib.log").open("a") as log, redirect_stdout(log), redirect_stderr(log):
        try:
            before = input_snapshot(root, paths.data)
            split_before = split.read_bytes()
            app = tag_eval.app_vectors(paths, root, binary)
            cmean = tag_eval.cmean_corpus(app, paths)
            saved = tag_eval.page_split(app, split)
            reference = read_json(baseline)
            evaluated = evaluate(app, cmean, saved, reference)
            if input_snapshot(root, paths.data) != before or split.read_bytes() != split_before:
                raise Invalid("frozen data or saved split changed during evaluation")
            result = {
                "schema": 1,
                "source_revision": subprocess.run(
                    ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
                ).stdout.strip(),
                "seed": SEED,
                "k": tag_eval.K,
                "counts": reference["counts"],
                "split_digest": fingerprint(saved),
                "baseline_digest": fingerprint(reference),
                "frozen_data_digest": fingerprint(before),
                **evaluated,
            }
            result["command"] = shlex.join(
                [
                    "uv",
                    "run",
                    "--locked",
                    "--project",
                    "lab/tagger",
                    "tagger",
                    "--root",
                    str(root),
                    "--data",
                    str(paths.data),
                    "tag-eval",
                    "--split",
                    str(split),
                    "--out",
                    str(out),
                    "--calib-baseline",
                    str(baseline),
                    "--kmt-bin",
                    binary,
                ]
            )
            json.dumps(result, allow_nan=False)
            write_json(out / "calib.json", result)
            (out / "calib.md").write_text(report(result))
            return result
        except Exception as err:
            traceback.print_exc()
            raise Invalid(
                str(err) if isinstance(err, Invalid) else "unreadable inputs or unavailable runtime"
            ) from None
