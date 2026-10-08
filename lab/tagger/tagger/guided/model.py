"""The tag scorer of design D1: a Rocchio prototype of the tag's query and its labelled pages,
blended with a per tag LR head once the tag has 3 positives and 3 negatives.

A scorer is fitted on a library (the user's pages) and its labelled positions in it. Scaling, z scores,
thresholds and the doubt spread come from library rows only; any other rows are just scored.
"""

from dataclasses import dataclass

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.preprocessing import StandardScaler

from ..heads import lr_refit
from ..metrics import best_threshold
from ..zeroshot import ZERO_K_SD, prototype

HEAD_C = 0.01
MIN_PER_CLASS = 3  # labels of each class before the LR head and the F1 threshold apply
ALPHA_LABELS = 10  # alpha = 10 / (10 + n_labels): the query's weight falls as labels arrive


@dataclass
class Library:
    X: np.ndarray
    scaler: StandardScaler

    @classmethod
    def of(cls, X: np.ndarray) -> "Library":
        return cls(X, StandardScaler().fit(X))


def _z(s: np.ndarray, ref: np.ndarray) -> np.ndarray:
    return (s - ref.mean()) / max(float(ref.std()), 1e-12)


@dataclass
class Scorer:
    lib: Library
    v: np.ndarray
    head: LogisticRegression | None
    alpha: float
    threshold: float = np.inf
    spread: float = 1.0

    def score(self, X: np.ndarray) -> np.ndarray:
        s_q = _z(X @ self.v, self.lib.X @ self.v)
        if self.head is None:
            return s_q
        s_h = self.head.decision_function(self.lib.scaler.transform(X))
        ref = self.head.decision_function(self.lib.scaler.transform(self.lib.X))
        return self.alpha * s_q + (1 - self.alpha) * _z(s_h, ref)

    def doubt(self, X: np.ndarray) -> np.ndarray:
        """1 - |2p - 1|, p = sigmoid((s - threshold) / SD(s on the library))."""
        p = 1 / (1 + np.exp(-(self.score(X) - self.threshold) / self.spread))
        return 1 - np.abs(2 * p - 1)


def fit(lib: Library, q: np.ndarray, rows: np.ndarray, y: np.ndarray, blend: bool = True) -> Scorer:
    """`rows`: labelled library positions, `y`: their answers. `blend=False` keeps the prototype alone."""
    rows, y = np.asarray(rows, int), np.asarray(y, bool)
    v = prototype(q, lib.X[rows[y]], lib.X[rows[~y]])
    both = min(int(y.sum()), int((~y).sum())) >= MIN_PER_CLASS
    head = lr_refit(lib.scaler.transform(lib.X[rows]), y, HEAD_C) if blend and both else None
    scorer = Scorer(lib, v, head, ALPHA_LABELS / (ALPHA_LABELS + len(rows)))
    fit_scores = scorer.score(lib.X)
    scorer.spread = max(float(fit_scores.std()), 1e-12)
    if both:
        scorer.threshold = best_threshold(fit_scores[rows], y)
    else:
        scorer.threshold = float(fit_scores.mean() + ZERO_K_SD * fit_scores.std())
    return scorer
