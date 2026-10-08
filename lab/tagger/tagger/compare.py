"""`tagger compare OTHER`: the metric outputs of two data directories, timings and memory excluded.

Proves a rerun reproduces the numbers in results.md; prints counts and the largest difference only.
"""

import json
import sys
from pathlib import Path

from .paths import Paths, read_json

FILES = (
    "eval/cv_summary.json",
    "eval/final.json",
    "out/zeroshot.json",
    "out/clusters.json",
    "out/suggest-summary.json",
)
MACHINE_SUFFIXES = ("seconds", "_s", "_ms", "_mb", "_median", "_p90")
TOLERANCE = 1e-3


def leaves(value, path=""):
    if isinstance(value, dict):
        for k, v in value.items():
            yield from leaves(v, f"{path}/{k}")
    elif isinstance(value, list):
        for i, v in enumerate(value):
            yield from leaves(v, f"{path}/{i}")
    else:
        yield path, value


def run(paths: Paths, other: str) -> None:
    there = Paths(Path(other).resolve())
    compared = differing = 0
    worst = (0.0, "")
    for name in FILES:
        a = dict(leaves(read_json(paths.data / name)))
        b = dict(leaves(read_json(there.data / name)))
        for key in sorted(set(a) | set(b)):
            last = key.rsplit("/", 1)[-1]
            if "/latency/" in key or last.endswith(MACHINE_SUFFIXES):
                continue
            compared += 1
            x, y = a.get(key), b.get(key)
            if isinstance(x, (int, float)) and isinstance(y, (int, float)) and not isinstance(x, bool):
                gap = abs(x - y) if x == x or y == y else 0.0
                if gap > worst[0]:
                    worst = (gap, f"{name}{key}")
                differing += gap > TOLERANCE
            else:
                differing += x != y
    print(
        json.dumps(
            {
                "compared": compared,
                "differing": differing,
                "tolerance": TOLERANCE,
                "largest_gap": worst[0],
                "largest_gap_at": worst[1],
            }
        )
    )
    sys.exit(1 if differing else 0)
