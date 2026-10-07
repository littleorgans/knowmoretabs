"""Per tag heads. Every head is `fit(feat, Y, rows) -> Fitted`: it sees only `rows` (its training
rows) and tunes everything, C or k and each tag's threshold, on inner folds of those rows.
The caller never passes evaluation rows to `fit`, so selection cannot see them.
"""

import warnings
from dataclasses import dataclass, field

import numpy as np
from iterstrat.ml_stratifiers import MultilabelStratifiedKFold
from joblib import Parallel, delayed
from sklearn.exceptions import ConvergenceWarning
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import average_precision_score
from sklearn.preprocessing import StandardScaler

from .metrics import best_threshold

SEED = 20261007
INNER_FOLDS = 3
DENSE_C = (0.0001, 0.001, 0.01, 0.1, 1.0, 10.0)
SPARSE_C = (0.01, 0.1, 1.0, 10.0, 100.0)
KS = (5, 10, 20, 40, 80)


def inner_folds(Y: np.ndarray, rows: np.ndarray):
    folds = MultilabelStratifiedKFold(n_splits=INNER_FOLDS, shuffle=True, random_state=SEED)
    return [(rows[a], rows[b]) for a, b in folds.split(rows, Y[rows])]


@dataclass
class Fitted:
    """A head fitted on `rows`: `post` turns raw apply features into what `score` takes."""

    thresholds: np.ndarray
    params: dict
    rows: np.ndarray = field(repr=False)
    post: object = field(repr=False)
    score: object = field(repr=False)

    def predict(self, feat, apply_rows) -> np.ndarray:
        return self.score(self.post(feat.matrices(self.rows, apply_rows)[1]))

    def predict_matrix(self, X) -> np.ndarray:
        return self.score(self.post(X))


def _ap(y, s):
    return average_precision_score(y, s) if y.any() else np.nan


# Logistic regression


def _lr(C):
    return LogisticRegression(C=C, class_weight="balanced", max_iter=5000)


def _lr_tag(folds, y_by_fold, Cs):
    """Inner OOF scores per C for one tag; returns the C with the best AP and its threshold."""
    warnings.simplefilter("ignore", ConvergenceWarning)
    oof = {C: [] for C in Cs}
    ys = []
    for (Xa, Xb), (ya, yb) in zip(folds, y_by_fold, strict=True):
        ys.append(yb)
        for C in Cs:
            oof[C].append(_lr(C).fit(Xa, ya).decision_function(Xb) if ya.any() else np.zeros(len(yb)))
    y = np.concatenate(ys)
    aps = {C: _ap(y, np.concatenate(oof[C])) for C in Cs}
    C = max(Cs, key=lambda c: aps[c])
    return C, best_threshold(np.concatenate(oof[C]), y)


def _lr_refit(X, y, C):
    warnings.simplefilter("ignore", ConvergenceWarning)
    return _lr(C).fit(X, y)


def _scaled(feat, fit_rows, apply_rows, scaler=None):
    Xa, Xb = feat.matrices(fit_rows, apply_rows)
    if not feat.dense:
        return Xa, Xb, None
    scaler = StandardScaler().fit(Xa)
    return scaler.transform(Xa), scaler.transform(Xb), scaler


def fit_lr(feat, Y: np.ndarray, rows: np.ndarray) -> Fitted:
    Cs = DENSE_C if feat.dense else SPARSE_C
    folds, labels = [], []
    for a, b in inner_folds(Y, rows):
        Xa, Xb, _ = _scaled(feat, a, b)
        folds.append((Xa, Xb))
        labels.append((Y[a], Y[b]))
    tags = range(Y.shape[1])
    chosen = Parallel(n_jobs=-1)(delayed(_lr_tag)(folds, [(ya[:, j], yb[:, j]) for ya, yb in labels], Cs) for j in tags)
    # Refit on all training rows; `post` applies the same scaling to any later rows.
    Xfit, _, scaler = _scaled(feat, rows, rows[:1])
    models = Parallel(n_jobs=-1)(delayed(_lr_refit)(Xfit, Y[rows, j], chosen[j][0]) for j in tags)

    def score(X):
        return np.column_stack([m.decision_function(X) for m in models])

    post = scaler.transform if scaler is not None else (lambda X: X)
    return Fitted(np.array([t for _, t in chosen]), {"C": [c for c, _ in chosen]}, rows, post, score)


# Cosine kNN


def unit(X):
    return X / np.maximum(np.linalg.norm(X, axis=1, keepdims=True), 1e-12)


def _knn_scores(S: np.ndarray, Ya: np.ndarray, k: int) -> np.ndarray:
    """Similarity weighted share of the k nearest training rows carrying each tag."""
    top = np.argpartition(-S, kth=min(k, S.shape[1] - 1), axis=1)[:, :k]
    w = np.maximum(np.take_along_axis(S, top, axis=1), 0) + 1e-6
    return np.einsum("ik,ikt->it", w, Ya[top]) / w.sum(axis=1, keepdims=True)


def fit_knn(feat, Y: np.ndarray, rows: np.ndarray) -> Fitted:
    oof = {k: [] for k in KS}
    ys = []
    for a, b in inner_folds(Y, rows):
        Xa, Xb = feat.matrices(a, b)
        S = unit(Xb) @ unit(Xa).T
        ys.append(Y[b])
        for k in KS:
            oof[k].append(_knn_scores(S, Y[a].astype(np.float32), k))
    y = np.vstack(ys)
    stacked = {k: np.vstack(v) for k, v in oof.items()}
    macro_ap = {k: np.nanmean([_ap(y[:, j], s[:, j]) for j in range(y.shape[1])]) for k, s in stacked.items()}
    k = max(KS, key=lambda kk: macro_ap[kk])
    thresholds = np.array([best_threshold(stacked[k][:, j], y[:, j]) for j in range(y.shape[1])])
    Xfit = unit(feat.matrices(rows, rows[:1])[0])
    Yfit = Y[rows].astype(np.float32)

    def score(X):
        return _knn_scores(X @ Xfit.T, Yfit, k)

    return Fitted(thresholds, {"k": k}, rows, unit, score)


# Tag frequency prior: one constant score per tag; for a constant scorer the best F1 is to
# predict the tag everywhere, so every tag is predicted for every page.


def fit_prior(feat, Y: np.ndarray, rows: np.ndarray) -> Fitted:
    prevalence = Y[rows].mean(axis=0)

    def score(X):
        return np.tile(prevalence, (X.shape[0], 1))

    return Fitted(np.zeros(Y.shape[1]), {}, rows, lambda X: X, score)


HEADS = {"lr": fit_lr, "knn": fit_knn, "prior": fit_prior}
