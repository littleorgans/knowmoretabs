"""Step 6: cost and latency on this machine, per model, for that model's best CV config.

Load time and RSS come from a warm load here; corpus throughput comes from the embed logs.
Single page latency: one page through embedding (and the image when the config uses it) plus every
tag head, fitted on all labelled pages, timed over a seeded sample of pages with text.
"""

import json
import platform
import subprocess
import sys
import time

import numpy as np

from .embed import IMAGE_MODEL, MODELS, embed_image_rows, embed_input, load, load_text, rss_mb, sync
from .evaluate import feature, labels
from .heads import HEADS, SEED
from .paths import Paths, read_json, resolve, write_json

LATENCY_PAGES = 20


def best_for(summary: dict, model: str) -> str:
    names = [n for n in summary if n.startswith(f"{model}-")]
    return max(names, key=lambda n: (summary[n]["macro"]["f1"], summary[n]["macro"]["ap"]))


def run(paths: Paths) -> None:
    """Each model in its own process, so load time and RSS are not shared with the previous one."""
    out = {
        "machine": subprocess.run(
            ["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True
        ).stdout.strip(),
        "memory_gb": round(
            int(subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True).stdout) / 2**30
        ),
        "python": platform.python_version(),
        "models": {},
    }
    for key in MODELS:
        subprocess.run([sys.executable, "-m", "tagger.cost", str(paths.data), key], check=True)
        out["models"][key] = read_json(paths.out / f"cost-{key}.json")
    write_json(paths.out / "cost.json", out)


def measure(paths: Paths, key: str) -> None:
    model = MODELS[key]
    summary = read_json(paths.eval / "cv_summary.json")["configs"]
    lab = labels(paths)
    image_meta = read_json(paths.emb / "image" / "eg2-full.json")
    sample = np.random.default_rng(SEED).choice(
        [i for i in lab.labelled if lab.records[i]["text_ok"]], LATENCY_PAGES, replace=False
    )
    config = best_for(summary, key)
    _, input_name, head = config.split("-")
    rss_start = rss_mb()
    st, stats = load_text(model)
    image, image_stats = load(IMAGE_MODEL) if input_name.startswith("C") else (None, {})
    throughput = {
        name: read_json(paths.emb / key / f"{name}.json")
        for name in ("A", "B", "B8k")
        if (paths.emb / key / f"{name}.json").exists()
    }
    text_input = "B" + input_name[1:] if input_name.startswith("C") else input_name
    feat, _ = feature(paths, lab, config)
    start = time.perf_counter()
    fitted = HEADS[head](feat, lab.Y, lab.labelled)
    heads_fit_s = time.perf_counter() - start
    # Rebuild: embed every known page for this input (and images for C), then fit the heads.
    rebuild = throughput[text_input]["seconds"] + (image_meta["seconds"] if image else 0) + heads_fit_s
    timings = []
    for i in sample:
        record = [lab.records[i]]
        sync()
        t = time.perf_counter()
        vector, _ = embed_input(st, model, record, text_input)
        if image is not None:
            pixels, _ = embed_image_rows(image, record)
            vector = np.hstack([vector, pixels, [[0.0 if record[0]["image_ok"] else 1.0]]])
        fitted.predict_matrix(vector.astype(np.float32))
        timings.append(time.perf_counter() - t)
    result = {
        "repo": model.repo,
        "config": config,
        "load_s_warm": stats["load_s"],
        "rss_start_mb": rss_start,
        "rss_after_load_mb": stats["rss_after_load_mb"],
        "rss_after_image_load_mb": image_stats.get("rss_after_load_mb"),
        "rss_end_mb": rss_mb(),
        "mps_driver_mb_after_load": stats.get("mps_driver_mb_after_load"),
        "image_model_load_s": image_stats.get("load_s"),
        "throughput_docs_per_s": {n: m["docs_per_s"] for n, m in throughput.items()},
        "embed_seconds_corpus": {n: m["seconds"] for n, m in throughput.items()},
        "images_per_s": image_meta["docs_per_s"],
        "heads_fit_s": round(heads_fit_s, 2),
        "full_rebuild_s": round(rebuild, 1),
        "single_page_ms": {
            "median": round(1000 * float(np.median(timings)), 1),
            "p90": round(1000 * float(np.percentile(timings, 90)), 1),
            "pages": len(timings),
        },
    }
    write_json(paths.out / f"cost-{key}.json", result)
    print(json.dumps({key: result}))


if __name__ == "__main__":
    measure(resolve(sys.argv[1]), sys.argv[2])
