"""Per tag and aggregate metrics, and the per tag F1 threshold."""

import numpy as np
from sklearn.metrics import average_precision_score, precision_recall_curve


def best_threshold(scores: np.ndarray, y: np.ndarray) -> float:
    """The score threshold (predict >= it) with the best F1 on these rows."""
    if y.sum() == 0:
        return float("inf")
    precision, recall, thresholds = precision_recall_curve(y, scores)
    f1 = 2 * precision[:-1] * recall[:-1] / np.maximum(precision[:-1] + recall[:-1], 1e-12)
    return float(thresholds[int(np.argmax(f1))])


def evaluate(Y: np.ndarray, scores: np.ndarray, pred: np.ndarray, tags: list[str]) -> dict:
    per_tag = {}
    for j, tag in enumerate(tags):
        y, p = Y[:, j].astype(bool), pred[:, j].astype(bool)
        tp, fp, fn = int((y & p).sum()), int((~y & p).sum()), int((y & ~p).sum())
        precision = tp / (tp + fp) if tp + fp else 0.0
        recall = tp / (tp + fn) if tp + fn else 0.0
        per_tag[tag] = {
            "precision": precision,
            "recall": recall,
            "f1": 2 * precision * recall / (precision + recall) if precision + recall else 0.0,
            "ap": float(average_precision_score(y, scores[:, j])) if y.any() else float("nan"),
            "support": int(y.sum()),
            "predicted": int(p.sum()),
        }
    y, p = Y.astype(bool), pred.astype(bool)
    tp, fp, fn = (y & p).sum(), (~y & p).sum(), (y & ~p).sum()
    micro_p = tp / (tp + fp) if tp + fp else 0.0
    micro_r = tp / (tp + fn) if tp + fn else 0.0
    return {
        "micro": {
            "precision": float(micro_p),
            "recall": float(micro_r),
            "f1": float(2 * micro_p * micro_r / (micro_p + micro_r)) if micro_p + micro_r else 0.0,
            "ap": float(average_precision_score(y.ravel(), scores.ravel())),
        },
        "macro": {k: float(np.nanmean([t[k] for t in per_tag.values()])) for k in ("precision", "recall", "f1", "ap")},
        "per_tag": per_tag,
    }
