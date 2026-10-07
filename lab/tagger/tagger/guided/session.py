"""A tag as a saved search: the user types a query, sees a grid of the top unseen library pages, ticks
the ones that belong; ticks and unticked shown pages re-rank the rest (Rocchio relevance feedback).

The session knows only the library vectors, the query and `ask`, which answers for the pages shown.
It never sees a label of a page it has not shown. The saved search is the query plus the session's
answers; its vector scores new pages.
"""

from collections.abc import Callable, Iterator

import numpy as np

from ..zeroshot import prototype

GRID = 20  # pages per grid
CAP = 10  # grids before the session stops anyway
MIN_TICKS = 2  # a grid with fewer ticks ends the session


def saved(X: np.ndarray, q: np.ndarray, rows: np.ndarray, ticked: np.ndarray) -> np.ndarray:
    """The saved search vector: the query and the answers on shown library positions `rows`."""
    return prototype(q, X[rows[ticked]], X[rows[~ticked]])


def grids(
    X: np.ndarray, q: np.ndarray, ask: Callable[[np.ndarray], np.ndarray], feedback: bool = True
) -> Iterator[tuple[np.ndarray, np.ndarray]]:
    """Yield (shown library positions, ticks) per grid until the library is exhausted; the caller stops
    iterating when its user would. `feedback=False` keeps the query's ranking (zero shot)."""
    seen = np.zeros(len(X), bool)
    rows, ticked = np.array([], int), np.array([], bool)
    v = q
    while not seen.all():
        s = X @ v
        s[seen] = -np.inf
        shown = np.argsort(-s, kind="stable")[: min(GRID, int((~seen).sum()))]
        ticks = np.asarray(ask(shown), bool)
        seen[shown] = True
        rows, ticked = np.concatenate([rows, shown]), np.concatenate([ticked, ticks])
        if feedback:
            v = saved(X, q, rows, ticked)
        yield shown, ticks


def stops(ticks_per_grid: list[int]) -> bool:
    """The stop rule after the latest grid: fewer than MIN_TICKS ticks in it, or CAP grids seen."""
    return ticks_per_grid[-1] < MIN_TICKS or len(ticks_per_grid) >= CAP
