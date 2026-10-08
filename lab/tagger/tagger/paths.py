"""Archive and data locations shared by the app and experiment steps."""

import json
import os
from dataclasses import dataclass
from pathlib import Path

SNAPSHOT_NAME = "snapshot-2026-10-07"


@dataclass(frozen=True)
class Paths:
    data: Path

    @property
    def snapshot(self) -> Path:
        return Path(os.environ.get("KMT_TAGGER_SNAPSHOT", self.data / SNAPSHOT_NAME))

    @property
    def dataset(self) -> Path:
        return self.data / "dataset"

    @property
    def emb(self) -> Path:
        return self.data / "emb"

    @property
    def eval(self) -> Path:
        return self.data / "eval"

    @property
    def out(self) -> Path:
        return self.data / "out"

    @property
    def guided(self) -> Path:
        return self.data / "guided"

    @property
    def retrieval(self) -> Path:
        return self.data / "retrieval"

    @property
    def search_tag(self) -> Path:
        return self.data / "search-tag"

    @property
    def import_check(self) -> Path:
        return self.data / "import-check"


def resolve_root(flag: str | None, env: str | None, home: Path) -> Path:
    return Path(flag if flag is not None else env if env is not None else home / ".knowmoretabs").resolve()


def resolve(data: str | None, root: Path | None = None) -> Paths:
    root = root if root is not None else resolve_root(None, os.environ.get("KMT_ROOT"), Path.home())
    value = data if data is not None else os.environ.get("KMT_TAGGER_DATA")
    return Paths(Path(value).resolve() if value is not None else root / "tagger")


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=1, sort_keys=True) + "\n")


def read_json(path: Path):
    return json.loads(path.read_text())
