"""Zero shot arm: each tag as a query, scored by cosine against the cached document embeddings.

Queries: the tag name, and `name: description` from data/zeroshot/descriptions.json (written from the
name alone, kept out of the repo). Each model card's `query` prompt. Inputs A, B (and B8k), C (text
and image cosines averaged where an image is ok) and the image alone (text query vs EG2 image, the
cross modal path). Metrics on the CV pool only, over the same outer folds; no labels are used.

Label efficiency: a prototype, unit(query + mean of k sampled positives), on the best config's model
and text input, k positives per tag drawn from each outer fit fold, seeded, a few repeats.
"""

import json
import time

import numpy as np
from sklearn.metrics import average_precision_score

from .embed import IMAGE_MODEL, MODELS, load, release, sync
from .evaluate import labels, outer_folds, split
from .heads import SEED, unit
from .metrics import best_threshold, evaluate
from .paths import Paths, read_json, write_json

KS = (0, 5, 10, 20)
REPEATS = 3
QUERY_PROMPT = "query"  # both cards ship it: EG2 `task: search result | query: `, Qwen3 `Instruct: ...\nQuery:`
ZERO_K_SD = 2.0  # k = 0 has no labels to tune a threshold: predict scores above mean + 2 SD on fit rows
SEARCH_REPEATS = 200


def embed_queries(st, queries: list[str]) -> tuple[np.ndarray, dict]:
    emb = st.encode(queries, prompt_name=QUERY_PROMPT, normalize_embeddings=True, convert_to_numpy=True)
    timings = []
    for q in queries:
        sync()
        t = time.perf_counter()
        st.encode([q], prompt_name=QUERY_PROMPT, normalize_embeddings=True, convert_to_numpy=True)
        sync()
        timings.append(time.perf_counter() - t)
    return np.asarray(emb, np.float32), {
        "prompt": st.prompts[QUERY_PROMPT],
        "query_ms_median": round(1000 * float(np.median(timings)), 1),
        "query_ms_p90": round(1000 * float(np.percentile(timings, 90)), 1),
    }


def search_ms(docs: np.ndarray, q: np.ndarray) -> float:
    timings = []
    for _ in range(SEARCH_REPEATS):
        t = time.perf_counter()
        s = docs @ q
        np.argpartition(-s, 20)[:20]
        timings.append(time.perf_counter() - t)
    return round(1000 * float(np.median(timings)), 3)


def macro_ap(Y: np.ndarray, S: np.ndarray) -> tuple[float, list[float]]:
    per = [float(average_precision_score(Y[:, j], S[:, j])) for j in range(Y.shape[1])]
    return float(np.mean(per)), per


def score_sets(paths: Paths, key: str, q: np.ndarray, q_image: np.ndarray) -> dict:
    """Cosine scores for every known page and tag, per input."""
    image = np.load(paths.emb / "image" / "eg2-full.npy")
    mask = np.load(paths.emb / "image" / "eg2-full-mask.npy")
    image_cos = np.where(mask[:, None], image @ q_image.T, -1.0)
    out = {}
    for name in ("A", "B", "B8k"):
        path = paths.emb / key / f"{name}.npy"
        if path.exists():
            out[name] = np.load(path) @ q.T
    for name in [n for n in ("B", "B8k") if n in out]:
        out["C" + name[1:]] = np.where(mask[:, None], (out[name] + image_cos) / 2, out[name])
    out["image"] = image_cos
    return out


def curve(lab, docs: np.ndarray, q: np.ndarray, pool: np.ndarray) -> dict:
    """Prototype at each k, pooled over the outer folds of the CV pool, per repeat."""
    folds = outer_folds(lab, pool)
    position = {row: i for i, row in enumerate(pool)}
    result = {}
    for k in KS:
        runs = []
        capped = 0
        for r in range(REPEATS):
            rng = np.random.default_rng([SEED, r, k])
            scores = np.zeros((len(pool), len(lab.tags)))
            pred = np.zeros_like(scores, dtype=bool)
            for a, b in folds:
                sampled = {}
                for j in range(len(lab.tags)):
                    positives = a[lab.Y[a, j] == 1]
                    capped += r == 0 and len(positives) < k
                    sampled[j] = (
                        rng.choice(positives, min(k, len(positives)), replace=False) if k else np.array([], int)
                    )
                labelled = np.unique(np.concatenate(list(sampled.values()))) if k else np.array([], int)
                at = [position[row] for row in b]
                for j in range(len(lab.tags)):
                    v = q[j] if k == 0 else unit((q[j] + docs[sampled[j]].mean(axis=0))[None])[0]
                    s = docs[b] @ v
                    if k == 0:
                        fit = docs[a] @ v
                        threshold = fit.mean() + ZERO_K_SD * fit.std()
                    else:
                        threshold = best_threshold(docs[labelled] @ v, lab.Y[labelled, j])
                    scores[at, j], pred[at, j] = s, s >= threshold
            m = evaluate(lab.Y[pool], scores, pred, lab.tags)
            runs.append(
                {
                    "macro_ap": m["macro"]["ap"],
                    "macro_f1": m["macro"]["f1"],
                    "micro_f1": m["micro"]["f1"],
                    "micro_ap": m["micro"]["ap"],
                }
            )
        result[k] = {
            **{f"{m}_mean": float(np.mean([x[m] for x in runs])) for m in runs[0]},
            **{f"{m}_sd": float(np.std([x[m] for x in runs])) for m in runs[0]},
            "tags_with_fewer_positives_than_k_fold_count": int(capped),
        }
    return result


def run(paths: Paths) -> None:
    lab = labels(paths)
    pool = np.asarray(split(paths, lab)["train"])  # the CV pool; the test rows are not read here
    descriptions = read_json(paths.data / "zeroshot" / "descriptions.json")
    variants = {
        "name": list(lab.tags),
        "description": [f"{t}: {descriptions[t]}" if descriptions.get(t) else t for t in lab.tags],
    }
    folds = outer_folds(lab, pool)
    image_st, _ = load(IMAGE_MODEL)
    q_image = {v: embed_queries(image_st, qs)[0] for v, qs in variants.items()}
    release(image_st)
    results, queries, latency = {}, {}, {}
    for key, model in MODELS.items():
        st, _ = load(model)
        for variant, qs in variants.items():
            q, stats = embed_queries(st, qs)
            queries[(key, variant)] = q
            latency[key] = stats
            for name, S in score_sets(paths, key, q, q_image[variant]).items():
                if name == "image" and key != "eg2":
                    continue
                label = f"eg2-full-image-{variant}" if name == "image" else f"{key}-{name}-{variant}"
                pooled, per = macro_ap(lab.Y[pool], S[pool])
                fold_means = [macro_ap(lab.Y[b], S[b])[0] for _, b in folds]
                results[label] = {
                    "macro_ap": pooled,
                    "fold_macro_ap_mean": float(np.mean(fold_means)),
                    "micro_ap": float(average_precision_score(lab.Y[pool].ravel(), S[pool].ravel())),
                    "per_tag_ap": dict(zip(lab.tags, per, strict=True)),
                }
                print(json.dumps({"zeroshot": label, "macro_ap": round(pooled, 4)}))
        docs = np.load(paths.emb / key / "B.npy")
        latency[key]["search_ms_median"] = search_ms(docs, queries[(key, "name")][0])
        latency[key]["search_pages"] = int(docs.shape[0])
        release(st)
    best = read_json(paths.eval / "final.json")["best"]
    model_key, input_name, _ = best.split("-")
    text_input = "B" + input_name[1:] if input_name.startswith("C") else input_name
    variant = max(variants, key=lambda v: results[f"{model_key}-{text_input}-{v}"]["macro_ap"])
    docs = np.load(paths.emb / model_key / f"{text_input}.npy")
    efficiency = curve(lab, docs, queries[(model_key, variant)], pool)
    out = {
        "results": results,
        "latency": latency,
        "curve": {
            "model": model_key,
            "input": text_input,
            "query": variant,
            "repeats": REPEATS,
            "zero_k_rule": f"mean + {ZERO_K_SD} SD of fit row scores",
            "by_k": efficiency,
        },
        "descriptions_written": sum(bool(descriptions.get(t)) for t in lab.tags),
    }
    write_json(paths.out / "zeroshot.json", out)
    print(
        json.dumps(
            {
                "curve": {
                    k: {m: round(v[m], 4) for m in ("macro_ap_mean", "macro_f1_mean")} for k, v in efficiency.items()
                },
                "latency": latency,
            }
        )
    )
