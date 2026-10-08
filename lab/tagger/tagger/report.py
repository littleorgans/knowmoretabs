"""Step 7: out/results.md from the step outputs. Aggregates only: no page text, no titles; cluster
descriptor terms (title words and hosts) are the one exception, as the brief allows.
"""

import collections

from .embed import IMAGE_MODEL, MODELS
from .evaluate import BASELINES, OUTER_FOLDS, TEST_SIZE
from .heads import DENSE_C, INNER_FOLDS, KS, SEED, SPARSE_C
from .paths import Paths, read_json


def f(x, digits=3) -> str:
    return "n/a" if x is None or x != x else f"{x:.{digits}f}"


def table(header: list[str], rows: list[list]) -> list[str]:
    out = [
        "| " + " | ".join(header) + " |",
        "| " + " | ".join("---" if i == 0 else "---:" for i in range(len(header))) + " |",
    ]
    return out + ["| " + " | ".join(str(c) for c in row) + " |" for row in rows]


def zeroshot_section(z: dict, best: str, best_cv: dict, cv: dict) -> list[str]:
    results = z["results"]
    top = max(results, key=lambda n: results[n]["macro_ap"])
    L = [
        "",
        "## Zero shot: tags as queries, no labels",
        "",
        f"Each card's query prompt; queries are the tag name, or `name: description` ({z['descriptions_written']} descriptions written from the names alone). "
        "Cosine against cached document embeddings, AP over the CV pool (pooled, and the mean over the same outer folds). "
        "C averages the text and image cosines where an image is ok; `eg2-full-image` is the text query against the image embedding alone (pages without an image rank last).",
        "",
    ]
    L += table(
        ["Config", "Macro AP", "Fold mean macro AP", "Micro AP"],
        [
            [n, f(r["macro_ap"]), f(r["fold_macro_ap_mean"]), f(r["micro_ap"])]
            for n, r in sorted(results.items(), key=lambda kv: -kv[1]["macro_ap"])
        ],
    )
    L += ["", f"Per tag AP: best zero shot ({top}) against the supervised pick ({best}, CV).", ""]
    L += table(
        ["Tag", "CV support", "Zero shot AP", "Supervised AP"],
        [
            [t, m["support"], f(results[top]["per_tag_ap"][t]), f(m["ap"])]
            for t, m in sorted(best_cv["per_tag"].items(), key=lambda kv: -kv[1]["support"])
        ],
    )
    c = z["curve"]
    curve_key = f"{c['model']}-{c['input']}-{c['query']}"
    L += [
        "",
        f"### Label efficiency ({c['model']}-{c['input']}, {c['query']} query)",
        "",
        f"Prototype = unit(query + mean of k positives sampled from each outer fit fold), {c['repeats']} seeded repeats, mean ± SD. "
        f"Threshold per tag: best F1 on the sampled labelled pages; at k = 0 there are no labels, so {c['zero_k_rule']}. "
        "Where a fold has fewer than k positives for a tag, all are used. "
        "Sampled pages are fully labelled reviews: their union supplies positives and implicit negatives for every tag. "
        "Thresholds use those fit pages, including the positives forming the prototype; evaluation remains on held out outer rows. "
        "k counts selected positives per tag. Full labels on the union reveal additional positives and negatives.",
        f"The k = 0 AP ({f(c['by_k']['0']['macro_ap_mean'])}) uses {curve_key}, the supervised pick's text input. "
        f"The best zero shot AP ({f(results[top]['macro_ap'])}) uses {top}. "
        "This is a fixed input learning curve; its zero point matches the zero shot score for that same input. "
        "Input and query variant selection use CV labels; zero shot scores and the k = 0 threshold do not fit labels.",
        "",
    ]
    rows = [
        [
            k,
            f"{v['macro_ap_mean']:.3f} ± {v['macro_ap_sd']:.3f}",
            f"{v['macro_f1_mean']:.3f} ± {v['macro_f1_sd']:.3f}",
            f"{v['micro_f1_mean']:.3f} ± {v['micro_f1_sd']:.3f}",
            v["tags_with_fewer_positives_than_k_fold_count"],
        ]
        for k, v in c["by_k"].items()
    ]
    full = cv["configs"][best]
    rows.append([f"all ({best})", f(full["macro"]["ap"]), f(full["macro"]["f1"]), f(full["micro"]["f1"]), "n/a"])
    L += table(["k positives per tag", "Macro AP", "Macro F1", "Micro F1", "Tag folds capped"], rows)
    L += [
        "",
        "Query latency (one query, batch 1) and cosine search over every known page (matrix product and top 20, CPU):",
        "",
    ]
    L += table(
        ["Model", "Query prompt", "Query ms p50", "Query ms p90", "Search ms p50", "Pages"],
        [
            [k, f"`{v['prompt']!r}`", v["query_ms_median"], v["query_ms_p90"], v["search_ms_median"], v["search_pages"]]
            for k, v in z["latency"].items()
        ],
    )
    return L


def run(paths: Paths) -> None:
    data = read_json(paths.dataset / "summary.json")
    tags = read_json(paths.dataset / "tags.json")
    cv = read_json(paths.eval / "cv_summary.json")
    final = read_json(paths.eval / "final.json")
    best = final["best"]
    best_cv = read_json(paths.eval / "cv" / f"{best}.json")
    suggest = read_json(paths.out / "suggest-summary.json")
    cost = read_json(paths.out / "cost.json")
    clusters = read_json(paths.out / "clusters.json")
    L = ["# kmt-tagger results", ""]
    L += [
        f"Snapshot: {data['known_pages']} known pages, {data['labelled_pages']} owner labelled, {data['owner_associations']} owner tag associations; "
        f"{data['labelled_text_ok']} labelled pages with text ok, {data['labelled_image_ok']} with image ok, {data['labelled_both_ok']} both, "
        f"{data['labelled_with_metadata']} with head metadata.",
        f"Trained tags (>=10 positives): {len(tags['trained'])}. Not trained: "
        + ", ".join(f"{t} ({tags['positives'][t]})" for t in tags["not_trained"])
        + ". Retired tags and imported suggestions are not labels.",
        "",
        "## Protocol",
        "",
        f"- Test split: {cv['test_pages']} pages ({TEST_SIZE:.0%}) by iterative multilabel stratification (seed {SEED}), scored once after the pick. CV pool: {cv['train_pages']} pages.",
        f"- Selection: {OUTER_FOLDS} fold multilabel stratified CV on the CV pool; out of fold predictions pooled for metrics. Each head is fitted on outer training rows only and tunes on {INNER_FOLDS} inner folds of them.",
        f"- LR: per tag, class balanced, standardised features, C in {list(DENSE_C)} (TF-IDF: {list(SPARSE_C)}) by inner AP. kNN: cosine, similarity weighted, k in {list(KS)} by inner macro AP. Threshold per tag: best inner F1.",
        "- Baselines: tag frequency prior (a constant score, so best F1 predicts every tag; AP equals prevalence) and per tag LR on TF-IDF of the tab title (1 and 2 grams, fitted inside each fold).",
        "- Inputs: A = tab title, URL host and path, head metadata (as `tag --prompt` hands an agent). B = A + page text where text is ok, else A; 2,048 tokens. "
        "C = [B, EG2 image embedding or zeros, image missing flag]. B8k/C8k = B/C with mean pooling of up to four 2,048 token chunks (8,192 tokens).",
        "- Precision fp32 on MPS. Prompts: "
        + "; ".join(f"{m.key} `{m.prompt!r}`" for m in MODELS.values())
        + f"; images: {IMAGE_MODEL.prompt}.",
        "",
        "## Config grid (CV pool, out of fold)",
        "",
    ]
    rows = sorted(cv["configs"].items(), key=lambda kv: -kv[1]["macro"]["f1"])
    L += table(
        ["Config", "Micro P", "Micro R", "Micro F1", "Micro AP", "Macro F1", "Macro AP", "Fold SD macro F1"],
        [
            [
                f"**{n}**" if n == best else n,
                f(r["micro"]["precision"]),
                f(r["micro"]["recall"]),
                f(r["micro"]["f1"]),
                f(r["micro"]["ap"]),
                f(r["macro"]["f1"]),
                f(r["macro"]["ap"]),
                f(r["fold_macro_f1_std"]),
            ]
            for n, r in rows
        ],
    )
    chosen = collections.Counter(v for p in best_cv["params"] for v in (p["C"] if "C" in p else [p.get("k")]))
    L += [
        "",
        f"Best by CV macro F1: **{best}**. Hyperparameter picks over tags and folds: "
        + ", ".join(f"{v}: {n}" for v, n in sorted(chosen.items()))
        + ".",
        "",
    ]
    L += ["## Test split (scored once)", ""]
    L += table(
        ["Config", "Micro P", "Micro R", "Micro F1", "Micro AP", "Macro F1", "Macro AP"],
        [
            [
                n,
                f(r["micro"]["precision"]),
                f(r["micro"]["recall"]),
                f(r["micro"]["f1"]),
                f(r["micro"]["ap"]),
                f(r["macro"]["f1"]),
                f(r["macro"]["ap"]),
            ]
            for n, r in final["test"].items()
        ],
    )
    test_tags = final["test"][best]["per_tag"]
    L += [
        "",
        f"## Per tag, {best}",
        "",
        "CV is out of fold over the CV pool; test is the held out split. Negatives are implicit (owner never removed a tag), so precision is a lower bound.",
        "",
    ]
    L += table(
        [
            "Tag",
            "CV support",
            "CV P",
            "CV R",
            "CV F1",
            "CV AP",
            "Test support",
            "Test P",
            "Test R",
            "Test F1",
            "Test AP",
        ],
        [
            [
                t,
                m["support"],
                f(m["precision"]),
                f(m["recall"]),
                f(m["f1"]),
                f(m["ap"]),
                test_tags[t]["support"],
                f(test_tags[t]["precision"]),
                f(test_tags[t]["recall"]),
                f(test_tags[t]["f1"]),
                f(test_tags[t]["ap"]),
            ]
            for t, m in sorted(best_cv["per_tag"].items(), key=lambda kv: -kv[1]["support"])
        ],
    )
    L += zeroshot_section(read_json(paths.out / "zeroshot.json"), best, best_cv, cv)
    dry = suggest["dry_run"]
    report = (dry.get("report") or {}).get("imported", {})
    L += [
        "",
        "## Suggestions",
        "",
        f"Source `{suggest['source']}`: {suggest['suggested_associations']} suggested tags on {suggest['pages_with_suggestions']} pages "
        f"({suggest['labelled_pages_with_suggestions']} labelled pages, out of fold; the rest from a fit on all labelled pages). "
        f"Dry run against a snapshot copy: exit {dry['exit_code']}, "
        + ", ".join(
            f"{k} {report[k]}"
            for k in ("dry_run", "pages", "tagged", "empty", "suggestions", "unchanged", "missing")
            if k in report
        )
        + f", created {len(report.get('created', []))}, revived {len(report.get('revived', []))}.",
        f"OOF micro precision on all labelled pages: {f(suggest['oof_micro_precision_proxy'])}. "
        f"Weighting each tag's OOF precision by its exported suggestion count gives a set precision proxy of "
        f"{f(suggest['suggestion_tag_weighted_precision_proxy'])}. "
        "Exported suggestions exclude known positives, so these proxies cannot establish the precision of missing tags. "
        "Every exported labelled association is an implicit negative under the recorded labels; owner review is needed to estimate true precision.",
        "",
        "## Cost on this Mac",
        "",
        f"{cost['machine']}, {cost['memory_gb']} GB, Python {cost['python']}, MPS fp32. Throughput is over the whole corpus ({data['known_pages']} pages) from the embed logs; load and RSS from a warm load in a fresh process; single page = embed (plus image for C) and every tag head, over sampled pages with text.",
        "",
    ]
    L += table(
        [
            "Model",
            "Config",
            "Load s (warm)",
            "RSS after load MB",
            "MPS driver MB",
            "A docs/s",
            "B docs/s",
            "B8k docs/s",
            "Images/s",
            "Heads fit s",
            "Full rebuild s",
            "Single page ms p50",
            "p90",
        ],
        [
            [
                k,
                m["config"],
                f(m["load_s_warm"], 1),
                m["rss_after_image_load_mb"] or m["rss_after_load_mb"],
                m["mps_driver_mb_after_load"],
                m["throughput_docs_per_s"].get("A", "n/a"),
                m["throughput_docs_per_s"].get("B", "n/a"),
                m["throughput_docs_per_s"].get("B8k", "n/a"),
                m["images_per_s"],
                m["heads_fit_s"],
                m["full_rebuild_s"],
                m["single_page_ms"]["median"],
                m["single_page_ms"]["p90"],
            ]
            for k, m in cost["models"].items()
        ],
    )
    s = clusters["summary"]
    L += [
        "",
        "## Unsupervised clusters vs owner tags",
        "",
        f"K-means on unit rows of {clusters['config']} embeddings of {clusters['pages']} labelled pages; k = {clusters['k']}, the number of trained tags. "
        f"Cosine silhouette is flat across k ({', '.join(f'{k}: {v:.3f}' for k, v in sorted(clusters['silhouette'].items(), key=lambda kv: int(kv[0])))}), so it cannot choose k. Descriptors: c-TF-IDF over title words and URL hosts.",
        f"Clusters with no matching tag (best Jaccard < {s['no_match_jaccard_below']}): {s['clusters_without_matching_tag']}. "
        f"Tags split across clusters (no cluster holds half its pages): {s['tags_split']} of {len(clusters['tags'])}. "
        f"Mean best cluster F1 over trained tags: {f(s['mean_best_cluster_f1_trained'])}.",
        "Best match Jaccard is intersection / union. Purity is the largest single tag intersection / cluster size. "
        "Per tag best cluster F1 is the maximum of 2 * intersection / (cluster size + tag support). "
        "Centroids are ordinary k-means means and are not constrained to unit norm.",
        "",
    ]
    L += table(
        ["Cluster", "Size", "Descriptor", "Best tag", "Jaccard", "Purity", "Purity tag"],
        [
            [
                c["cluster"],
                c["size"],
                ", ".join(c["descriptor"]),
                c["best_tag"] if c["matched"] else f"none ({c['best_tag']})",
                f(c["jaccard"]),
                f(c["purity"]),
                c["purity_tag"],
            ]
            for c in sorted(clusters["clusters"], key=lambda c: -c["size"])
        ],
    )
    L += [""]
    L += table(
        ["Tag", "Positives", "Best cluster", "Best cluster F1", "Largest share", "Clusters with >=10%", "Split"],
        [
            [
                t["tag"],
                t["positives"],
                t["best_cluster"],
                f(t["best_cluster_f1"]),
                f(t["largest_share"]),
                t["clusters_with_10pct"],
                "yes" if t["split"] else "no",
            ]
            for t in sorted(clusters["tags"], key=lambda t: -t["positives"])
        ],
    )
    L += ["", f"Baselines in every table: {', '.join(BASELINES)}. Reproduce: see lab/tagger/README.md.", ""]
    (paths.out / "results.md").write_text("\n".join(L))
    print(f"wrote {paths.out / 'results.md'} ({len(L)} lines)")
