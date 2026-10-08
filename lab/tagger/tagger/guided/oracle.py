"""The simulated user: the owner's tags. The only reader of labels in the guided replays; a scorer or a
session sees an answer only for the pages it shows.
"""

import numpy as np

from ..evaluate import Labels
from ..paths import Paths, read_json

MIN_PICK_PAGES = 3  # result pages holding a tag before the user picks it


def answers(Y: np.ndarray, library: np.ndarray, j: int):
    """The owner's answer on tag `j` for chosen library positions; the scorer sees nothing else of Y."""
    return lambda rows: Y[library[np.asarray(rows, int)], j].astype(bool)


def owner_matrix(paths: Paths, lab: Labels) -> tuple[list[str], np.ndarray]:
    """Every active owner tag with at least one positive, largest first, over all known pages."""
    positives = read_json(paths.dataset / "tags.json")["positives"]
    tags = [t for t, n in positives.items() if n >= 1]
    Y = np.array([[t in r["tags"] for t in tags] for r in lab.records], dtype=np.int8)
    return tags, Y


def picks(Y: np.ndarray, rows: np.ndarray, min_pages: int = MIN_PICK_PAGES) -> np.ndarray:
    """The tags the user picks for a result set: those held by at least `min_pages` of its pages."""
    return np.flatnonzero(Y[np.asarray(rows, int)].sum(axis=0) >= min_pages)


def distractors(picked: np.ndarray, n_tags: int, k: int, rng) -> np.ndarray:
    """`k` tags the user picks wrongly: drawn at random from the tags the oracle left out."""
    rest = np.setdiff1d(np.arange(n_tags), picked)
    return rng.choice(rest, min(k, len(rest)), replace=False) if k else np.array([], int)
