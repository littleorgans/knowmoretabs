"""The simulated user: the owner's tags. The only reader of labels in the guided replays; a scorer or a
session sees an answer only for the pages it shows.
"""

import numpy as np

from ..evaluate import Labels
from ..paths import Paths, read_json


def answers(Y: np.ndarray, library: np.ndarray, j: int):
    """The owner's answer on tag `j` for chosen library positions; the scorer sees nothing else of Y."""
    return lambda rows: Y[library[np.asarray(rows, int)], j].astype(bool)


def owner_matrix(paths: Paths, lab: Labels) -> tuple[list[str], np.ndarray]:
    """Every active owner tag with at least one positive, largest first, over all known pages."""
    positives = read_json(paths.dataset / "tags.json")["positives"]
    tags = [t for t, n in positives.items() if n >= 1]
    Y = np.array([[t in r["tags"] for t in tags] for r in lab.records], dtype=np.int8)
    return tags, Y
