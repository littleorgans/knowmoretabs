"""Step 5: unsupervised clusters over the best config's embeddings, compared with the owner's tags.

K-means on unit rows of the labelled pages. k is the number of trained tags, for a like for
like comparison; the cosine silhouette curve is reported, and on this data it is flat. Descriptors are
c-TF-IDF terms from titles and URL hosts only; page text is never read here.
"""

import collections
import json
import re

import numpy as np
from sklearn.cluster import KMeans
from sklearn.feature_extraction.text import ENGLISH_STOP_WORDS, CountVectorizer
from sklearn.metrics import silhouette_score

from . import dataset
from .evaluate import feature, labels
from .heads import SEED, unit
from .paths import Paths, read_json, write_json

KS = tuple(range(10, 61, 5))
NO_MATCH_JACCARD = 0.1
DESCRIPTOR_TERMS = 5


def terms(record: dict) -> list[str]:
    words = re.findall(r"[a-z][a-z0-9+#]{2,}", record["title"].lower())
    return [w for w in words if w not in ENGLISH_STOP_WORDS] + ([record["host"]] if record["host"] else [])


def descriptors(records: list[dict], assign: np.ndarray, k: int) -> list[list[str]]:
    """c-TF-IDF (BERTopic): term frequency per cluster, weighted by log(1 + mean words per cluster / term frequency)."""
    docs = [[t for i in np.flatnonzero(assign == c) for t in terms(records[i])] for c in range(k)]
    vec = CountVectorizer(analyzer=lambda d: d, min_df=2)
    tf = vec.fit_transform(docs).toarray().astype(float)
    idf = np.log(1 + tf.sum(axis=1).mean() / np.maximum(tf.sum(axis=0), 1))
    weight = tf / np.maximum(tf.sum(axis=1, keepdims=True), 1) * idf
    vocab = vec.get_feature_names_out()
    return [[vocab[j] for j in np.argsort(-row)[:DESCRIPTOR_TERMS] if row[j] > 0] for row in weight]


def run(paths: Paths) -> None:
    best = read_json(paths.eval / "final.json")["best"]
    lab = labels(paths)
    _, tag_info = dataset.load(paths)
    feat, _ = feature(paths, lab, best)
    rows = lab.labelled
    X = unit(feat.X[rows])
    curve = {}
    fits = {}
    for k in KS:
        fits[k] = KMeans(n_clusters=k, n_init=10, random_state=SEED).fit_predict(X)
        curve[k] = float(silhouette_score(X, fits[k], metric="cosine"))
    k = len(lab.tags)
    assign = fits[k] if k in fits else KMeans(n_clusters=k, n_init=10, random_state=SEED).fit_predict(X)
    curve.setdefault(k, float(silhouette_score(X, assign, metric="cosine")))
    records = [lab.records[i] for i in rows]
    tags = [t for t, n in tag_info["positives"].items() if n > 0]
    members = {c: set(np.flatnonzero(assign == c)) for c in range(k)}
    holders = {t: {i for i, r in enumerate(records) if t in r["tags"]} for t in tags}
    words = descriptors(records, assign, k)
    clusters = []
    for c in range(k):
        m = members[c]
        jacc = {t: len(m & h) / len(m | h) for t, h in holders.items()}
        top = max(tags, key=lambda t: jacc[t])
        counts = collections.Counter(t for i in m for t in records[i]["tags"])
        clusters.append(
            {
                "cluster": c,
                "size": len(m),
                "descriptor": words[c],
                "best_tag": top,
                "jaccard": jacc[top],
                "purity": max(counts.values()) / len(m),
                "purity_tag": counts.most_common(1)[0][0],
                "matched": jacc[top] >= NO_MATCH_JACCARD,
            }
        )
    per_tag = []
    for t, h in holders.items():
        f1 = {c: 2 * len(members[c] & h) / (len(members[c]) + len(h)) for c in range(k)}
        c = max(f1, key=f1.get)
        shares = sorted((len(members[cc] & h) / len(h) for cc in range(k)), reverse=True)
        per_tag.append(
            {
                "tag": t,
                "positives": len(h),
                "best_cluster": c,
                "best_cluster_f1": f1[c],
                "largest_share": shares[0],
                "clusters_with_10pct": sum(s >= 0.1 for s in shares),
                "split": shares[0] < 0.5,
            }
        )
    trained = set(lab.tags)
    out = {
        "config": best,
        "pages": len(rows),
        "k": k,
        "silhouette": curve,
        "clusters": clusters,
        "tags": per_tag,
        "summary": {
            "clusters_without_matching_tag": sum(not c["matched"] for c in clusters),
            "tags_split": sum(t["split"] for t in per_tag),
            "mean_best_cluster_f1_trained": float(
                np.mean([t["best_cluster_f1"] for t in per_tag if t["tag"] in trained])
            ),
            "no_match_jaccard_below": NO_MATCH_JACCARD,
        },
    }
    write_json(paths.out / "clusters.json", out)
    print(
        json.dumps(
            {
                "k": k,
                "silhouette": round(curve[k], 4),
                "silhouette_range": [round(min(curve.values()), 4), round(max(curve.values()), 4)],
                **out["summary"],
            }
        )
    )
