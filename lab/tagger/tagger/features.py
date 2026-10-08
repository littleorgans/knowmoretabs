"""Feature sets a head can train on. `matrices(fit_rows, apply_rows)` fits anything learned
(TF-IDF vocabulary, scaling) on `fit_rows` only, so inner folds never see their own validation rows.
Rows index the full dataset (all known pages).
"""

from dataclasses import dataclass

import numpy as np
from sklearn.feature_extraction.text import TfidfVectorizer

from .paths import Paths

INPUTS = ("A", "B", "C")
CHUNKED_INPUTS = ("B8k", "C8k")


@dataclass
class Dense:
    name: str
    X: np.ndarray
    dense: bool = True

    def matrices(self, fit_rows, apply_rows):
        return self.X[fit_rows], self.X[apply_rows]


@dataclass
class TfidfTitle:
    titles: list
    name: str = "tfidf-title"
    dense: bool = False

    def matrices(self, fit_rows, apply_rows):
        vec = TfidfVectorizer(sublinear_tf=True, ngram_range=(1, 2), min_df=2, lowercase=True)
        fit = vec.fit_transform([self.titles[i] for i in fit_rows])
        return fit, vec.transform([self.titles[i] for i in apply_rows])


def available(paths: Paths, model: str) -> list[str]:
    have = {p.stem for p in (paths.emb / model).glob("*.npy")}
    image = (paths.emb / "image" / "eg2-full.npy").exists()
    out = [i for i in ("A", "B", "B8k") if i in have]
    if image:
        out += [f"C{i[1:]}" for i in ("B", "B8k") if i in have]
    return sorted(out, key=lambda i: (len(i), i))


def dense(paths: Paths, model: str, input_name: str) -> Dense:
    """C = [B, image or zeros, missing indicator]; each block keeps its own unit norm."""
    if input_name.startswith("C"):
        text = np.load(paths.emb / model / f"B{input_name[1:]}.npy")
        image = np.load(paths.emb / "image" / "eg2-full.npy")
        mask = np.load(paths.emb / "image" / "eg2-full-mask.npy")
        X = np.hstack([text, image, (~mask).astype(np.float32)[:, None]])
    else:
        X = np.load(paths.emb / model / f"{input_name}.npy")
    return Dense(f"{model}-{input_name}", X.astype(np.float32))
