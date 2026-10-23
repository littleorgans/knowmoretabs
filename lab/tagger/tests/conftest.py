"""Shared synthetic tag evaluation corpus."""

import numpy as np
import pytest

from tagger import tag_eval
from tagger.heads import unit


@pytest.fixture
def corpus():
    tags = ["Common", "Other", "Boundary", "Rare"]
    Y = np.zeros((80, len(tags)), np.int8)
    Y[:30, 0] = 1
    Y[20:45, 1] = 1
    Y[42:52, 2] = 1
    Y[54:56, 3] = 1
    X = np.random.default_rng(12).normal(size=(80, 6))
    X[:, : len(tags)] += Y * 3
    records = [
        {"key": f"synthetic-page-{i}", "tags": [t for j, t in enumerate(tags) if Y[i, j]], "forgotten": False}
        for i in range(len(Y))
    ]
    return tag_eval.Corpus(
        records, tags, Y, np.flatnonzero(Y.any(axis=1)), unit(X), np.eye(6)[: len(tags)], {"input": "synthetic"}
    )
