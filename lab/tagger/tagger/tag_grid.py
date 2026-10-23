"""Fixed split tag quality grid and paired page bootstrap against a saved baseline."""

from pathlib import Path

import numpy as np

from . import eval_runner, tag_eval
from .app import kmt
from .heads import SEED
from .paths import Paths, read_json
from .eval_runner import Invalid
from .search_eval import fingerprint

BOOTSTRAP_SAMPLES = 10_000
GRID_STRATEGIES = ("app", "proto_fallback", "proto_all")


def page_rates(result: dict) -> tuple[list, np.ndarray]:
    """Align paired pages by digest; rankings carry no page text or address."""
    pages = sorted(result["predictions"], key=lambda p: p["id"])
    if not pages or len({p["id"] for p in pages}) != len(pages):
        raise Invalid("unique evaluated pages required")
    rates = []
    for p in pages:
        truth = set(p["truth"])
        predicted = [h["tag"] for h in p["top_3"]]
        if not truth or len(predicted) > tag_eval.K or len(set(predicted)) != len(predicted):
            raise Invalid("invalid paired page labels or predictions")
        tp = len(truth.intersection(predicted))
        rates.append([tp / len(predicted) if predicted else np.nan, tp / len(truth)])
    identity = [(p["id"], p["fold"], sorted(p["truth"])) for p in pages]
    return identity, np.array(rates)


def paired_bootstrap(candidate: dict, baseline: dict) -> dict:
    """Percentile intervals for mean paired differences, sampling whole pages."""
    identity, rates = page_rates(candidate)
    baseline_identity, baseline_rates = page_rates(baseline)
    if identity != baseline_identity:
        raise Invalid("baseline paired pages, folds or truth differ")

    def average(values, axis):
        # Match measure: precision averages pages with suggestions; recall all truth pages.
        return tag_eval.ratio(np.nansum(values, axis=axis), np.isfinite(values).sum(axis=axis))

    rng = np.random.default_rng(SEED)
    means = np.empty((BOOTSTRAP_SAMPLES, rates.shape[1]))
    for start in range(0, BOOTSTRAP_SAMPLES, 256):
        end = min(start + 256, BOOTSTRAP_SAMPLES)
        indices = rng.integers(len(rates), size=(end - start, len(rates)))
        means[start:end] = average(rates[indices], 1) - average(baseline_rates[indices], 1)
    bounds = np.quantile(means, [0.025, 0.975], axis=0)
    delta = average(rates, 0) - average(baseline_rates, 0)
    return {
        "seed": SEED,
        "samples": BOOTSTRAP_SAMPLES,
        "pages": len(rates),
        "confidence": 0.95,
        "method": "paired page percentile bootstrap",
        **{
            name: {"difference": float(delta[j]), "interval": bounds[:, j].tolist()}
            for j, name in enumerate(("precision_at_3", "recall_at_3"))
        },
    }


def validate_baseline(corpus: tag_eval.Corpus, saved: dict, baseline: dict) -> None:
    """Common provenance boundary for fixed split tag experiments."""
    if (
        baseline["split_digest"] != fingerprint(saved)
        or baseline["page_source"] != "app"
        or baseline["strategy"] != "app"
        or baseline["k"] != tag_eval.K
        or baseline["seed"] != SEED
    ):
        raise Invalid("saved app baseline provenance differs")
    if any(
        corpus.metadata[k] != baseline["inputs"][k] for k in ("vectors_digest", "queries_digest", "descriptions_digest")
    ):
        raise Invalid("saved app baseline vectors or queries differ")


def evaluate_row(corpus, saved, baseline, source, strategy, *, fitted_folds=None):
    """One metric and pairing path, including exact app baseline reproduction."""
    result = tag_eval.cross_validate(corpus, saved, strategy, fitted_folds=fitted_folds)
    result.update(page_source=source, strategy=strategy)
    if source == "app" and strategy == "app":
        if result["metrics"] != baseline["metrics"] or result["predictions"] != baseline["predictions"]:
            raise Invalid("recomputed app ranking or metrics differ from baseline")
    result["paired_vs_baseline"] = paired_bootstrap(result, baseline)
    return result


def evaluate(corpora: dict, saved: dict, baseline: dict) -> dict:
    """Every model fits per fold; only the named ceiling reads held out truth."""
    validate_baseline(corpora["app"], saved, baseline)
    rows = {}
    for source, corpus in corpora.items():
        for strategy in GRID_STRATEGIES:
            rows[f"{source}/{strategy}"] = evaluate_row(corpus, saved, baseline, source, strategy)
    for strategy in ("prior", "ceiling"):
        rows[strategy] = evaluate_row(corpora["app"], saved, baseline, "reference", strategy)
    return {
        "rows": rows,
        "best_row": max(
            (key for key in rows if "/" in key),
            key=lambda key: rows[key]["metrics"]["overall"]["macro"]["precision_at_3"],
        ),
    }


def grid_table(rows: dict) -> list[str]:
    """P/R/H are page macro; pooled columns are association micro."""
    lines = [
        "| Source / scorer | All P/R/H % | Micro P/R % | Head P/R/H % | Head micro P/R % | Fallback P/R/H % | Fallback micro P/R % | ΔP pp [95%] | ΔR pp [95%] |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for key, row in rows.items():
        cells = [key]
        groups = [row["metrics"]["overall"], *row["metrics"]["by_fold_head"].values()]
        for m in groups:
            for kind, metrics in (
                ("macro", ("precision_at_3", "recall_at_3", "hit_at_3")),
                ("micro", ("precision_at_3", "recall_at_3")),
            ):
                cells.append("/".join(f"{100 * m[kind][metric]:.2f}" for metric in metrics))
        for metric in ("precision_at_3", "recall_at_3"):
            paired = row["paired_vs_baseline"][metric]
            lo, hi = paired["interval"]
            cells.append(f"{100 * paired['difference']:+.2f} [{100 * lo:+.2f}, {100 * hi:+.2f}]")
        lines.append("| " + " | ".join(cells) + " |")
    return lines


def report(result: dict) -> str:
    inputs, rows = result["inputs"]["cmean"], result["rows"]
    fallback_count = sum(any(not f["has_head"] for f in t["folds"]) for t in rows["app/app"]["per_tag"].values())
    lines = [
        f"5 saved folds; {result['counts']['evaluation_pages']} evaluated pages; k=3. C mean: {inputs['pages_with_chunks']} pages, {inputs['pages_without_chunks']} use B ({inputs['evaluated_pages_without_chunks']} evaluated); {inputs['chunks_used']} chunks by page_id.",
        f"Fit on training only. Prototype=unit(mean(positive page vectors)); unseen tags keep app query. Mix: replace fallback cosine, keep LR; subtract each tag's all training page mean, divide by max(SD,1e-12), rank together. P/R/H=macro, micro hit=macro hit; groups use app head eligibility (10+ positives). Paired page percentile bootstrap: {BOOTSTRAP_SAMPLES} draws, seed {SEED}; Δ in pp.",
        "",
        *grid_table(rows),
        "",
        f"Best deployable P@3: {result['best_row']}. Prior uses training frequencies with stable app ties; ceiling P=min(3,true count)/3, oracle truth ties follow prior. Full data fallback tags below; all {fallback_count} fold fallback tags and every metric are in JSON. Cells are pooled P/R %, app versus proto_fallback on each source.",
        "",
        "| Fallback tag | Positives | B app | B prototype | C mean app | C mean prototype |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for tag, t in rows["app/app"]["per_tag"].items():
        if t["has_head_full"]:
            continue
        cells = [tag.replace("|", "&#124;"), str(t["positives"])]
        for source in ("app", "cmean"):
            for strategy in ("app", "proto_fallback"):
                micro = rows[f"{source}/{strategy}"]["per_tag"][tag]["overall"]["micro"]
                cells.append("/".join(f"{100 * micro[k]:.2f}" for k in ("precision_at_3", "recall_at_3")))
        lines.append("| " + " | ".join(cells) + " |")
    lines += ["", f"Run: `{result['command']}`"]
    return "\n".join(lines) + "\n"


def run(paths: Paths, root: Path, split: Path, out: Path, baseline: Path, *, binary=kmt.BINARY) -> dict:
    if not split.is_file():
        raise Invalid("grid requires an existing saved split")

    def evaluate_grid():
        app = tag_eval.app_vectors(paths, root, binary)
        corpora = {"app": app, "cmean": tag_eval.cmean_corpus(app, paths)}
        saved = tag_eval.page_split(app, split)
        before = split.read_bytes()
        reference = read_json(baseline)
        evaluated = evaluate(corpora, saved, reference)
        if split.read_bytes() != before:
            raise Invalid("saved split changed during evaluation")
        return {
            "schema": 1,
            "seed": SEED,
            "k": tag_eval.K,
            "split_digest": fingerprint(saved),
            "baseline_digest": fingerprint(reference),
            "inputs": {source: corpus.metadata for source, corpus in corpora.items()},
            "counts": reference["counts"],
            **evaluated,
        }

    return eval_runner.run(
        paths,
        root,
        out,
        evaluate_grid,
        stem="grid",
        label="grid outputs",
        log=out / "grid-logs" / "tag-grid.log",
        report=report,
        binary=binary,
        command=["tag-eval", "--split", str(split), "--grid-baseline", str(baseline)],
    )
