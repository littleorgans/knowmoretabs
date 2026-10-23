"""Fixed split tag quality grid and paired page bootstrap against a saved baseline."""

import json
import shlex
import subprocess
import traceback
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import numpy as np

from . import tag_eval
from .app import kmt
from .heads import SEED
from .paths import Paths, read_json, write_json
from .search_eval import Invalid, _safe_paths, fingerprint

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
        if not truth or len(predicted) != tag_eval.K or len(set(predicted)) != tag_eval.K:
            raise Invalid("invalid paired page labels or predictions")
        tp = len(truth.intersection(predicted))
        rates.append([tp / tag_eval.K, tp / len(truth)])
    identity = [(p["id"], p["fold"], sorted(p["truth"])) for p in pages]
    return identity, np.array(rates)


def paired_bootstrap(candidate: dict, baseline: dict) -> dict:
    """Percentile intervals for mean paired differences, sampling whole pages."""
    identity, rates = page_rates(candidate)
    baseline_identity, baseline_rates = page_rates(baseline)
    if identity != baseline_identity:
        raise Invalid("baseline paired pages, folds or truth differ")
    delta = rates - baseline_rates
    rng = np.random.default_rng(SEED)
    means = np.empty((BOOTSTRAP_SAMPLES, delta.shape[1]))
    for start in range(0, BOOTSTRAP_SAMPLES, 256):
        end = min(start + 256, BOOTSTRAP_SAMPLES)
        indices = rng.integers(len(delta), size=(end - start, len(delta)))
        means[start:end] = delta[indices].mean(axis=1)
    bounds = np.quantile(means, [0.025, 0.975], axis=0)
    return {
        "seed": SEED,
        "samples": BOOTSTRAP_SAMPLES,
        "pages": len(delta),
        "confidence": 0.95,
        "method": "paired page percentile bootstrap",
        **{
            name: {"difference": float(delta[:, j].mean()), "interval": bounds[:, j].tolist()}
            for j, name in enumerate(("precision_at_3", "recall_at_3"))
        },
    }


def evaluate(corpora: dict, saved: dict, baseline: dict) -> dict:
    """Every model fits per fold; only the named ceiling reads held out truth."""
    if (
        baseline["split_digest"] != fingerprint(saved)
        or baseline["page_source"] != "app"
        or baseline["strategy"] != "app"
        or baseline["k"] != tag_eval.K
        or baseline["seed"] != SEED
    ):
        raise Invalid("saved app baseline provenance differs")
    if any(
        corpora["app"].metadata[k] != baseline["inputs"][k]
        for k in ("vectors_digest", "queries_digest", "descriptions_digest")
    ):
        raise Invalid("saved app baseline vectors or queries differ")
    rows = {}
    for source, corpus in corpora.items():
        for strategy in GRID_STRATEGIES:
            result = tag_eval.cross_validate(corpus, saved, strategy)
            result.update(page_source=source, strategy=strategy)
            result["paired_vs_baseline"] = paired_bootstrap(result, baseline)
            rows[f"{source}/{strategy}"] = result
    app = rows["app/app"]
    if app["metrics"] != baseline["metrics"] or app["predictions"] != baseline["predictions"]:
        raise Invalid("recomputed app ranking or metrics differ from baseline")
    for strategy in ("prior", "ceiling"):
        result = tag_eval.cross_validate(corpora["app"], saved, strategy)
        result.update(page_source="reference", strategy=strategy)
        result["paired_vs_baseline"] = paired_bootstrap(result, baseline)
        rows[strategy] = result
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
    _safe_paths(paths, root, out)
    if not split.is_file():
        raise Invalid("grid requires an existing saved split")
    if any((out / name).exists() for name in ("grid.json", "grid.md")):
        raise Invalid("grid outputs already exist; choose a separate output directory")
    logs = out / "grid-logs"
    logs.mkdir(parents=True, exist_ok=True)
    with (logs / "tag-grid.log").open("a") as log, redirect_stdout(log), redirect_stderr(log):
        try:
            app = tag_eval.app_vectors(paths, root, binary)
            corpora = {"app": app, "cmean": tag_eval.cmean_corpus(app, paths)}
            saved = tag_eval.page_split(app, split)
            before = split.read_bytes()
            reference = read_json(baseline)
            evaluated = evaluate(corpora, saved, reference)
            if split.read_bytes() != before:
                raise Invalid("saved split changed during evaluation")
            result = {
                "schema": 1,
                "source_revision": subprocess.run(
                    ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
                ).stdout.strip(),
                "seed": SEED,
                "k": tag_eval.K,
                "split_digest": fingerprint(saved),
                "baseline_digest": fingerprint(reference),
                "inputs": {source: corpus.metadata for source, corpus in corpora.items()},
                "counts": reference["counts"],
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
                    "--grid-baseline",
                    str(baseline),
                    "--kmt-bin",
                    binary,
                ]
            )
            json.dumps(result, allow_nan=False)
            write_json(out / "grid.json", result)
            (out / "grid.md").write_text(report(result))
            return result
        except Exception as err:
            traceback.print_exc()
            raise Invalid(
                str(err) if isinstance(err, Invalid) else "unreadable inputs or unavailable runtime"
            ) from None
