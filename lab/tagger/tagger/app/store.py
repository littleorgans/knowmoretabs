"""The app's own lists, each one private file under <data>/app/ replaced whole on every change: the selected and the
pinned pages (`Keys`) and the results marked not relevant to a query (`Exclusions`). The owner's decisions, tags and
forgotten pages, are the library's: `library.json`, which only knowmoretabs writes. The files an earlier app kept
its own decisions and Send records in (`state.json`, `forgotten.json`, `exports/`) stay where they are, unread.
"""

import json
import os
import time
from pathlib import Path


def _write(path: Path, text: str) -> None:
    """Stage and rename, so a crash leaves the old file or the new one; 0600 under the CLI's umask."""
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")
    part.write_text(text)
    os.chmod(part, 0o600)
    os.replace(part, path)


class Keys:
    """Page keys in order, one private file under <data>/app/ replaced whole on every change: the selection
    (selection.json, kept across searches and restarts until cleared) and the pinned pages (pinned.json)."""

    def __init__(self, root: Path, name: str):
        self.path = root / name
        self.keys: list[str] = json.loads(self.path.read_text())["keys"] if self.path.exists() else []

    def set(self, keys: list[str]) -> None:
        self.keys = list(dict.fromkeys(keys))
        _write(self.path, json.dumps({"version": 1, "keys": self.keys}, ensure_ascii=False) + "\n")


class Exclusions:
    """Results marked not relevant to a query, in <data>/app/not-relevant.json, apart from the library's decisions.

    Per query (as searched): each excluded page key with its rank when excluded, when, and by what (a click
    on its tile or a cut), and the cut, if any: the page every result below is excluded under, and the pages
    it has seen. A result first shown later below the cut (show more, refine, images) is excluded as it appears;
    one seen before stays as the owner left it.
    """

    ACTIONS = ("exclude", "include", "cut", "uncut")

    def __init__(self, root: Path):
        self.path = root / "not-relevant.json"
        self.state = json.loads(self.path.read_text()) if self.path.exists() else {"version": 1, "queries": {}}

    def _save(self) -> None:
        _write(self.path, json.dumps(self.state, indent=1, ensure_ascii=False) + "\n")

    def keys(self, query: str) -> set[str]:
        return set(self.state["queries"].get(query, {}).get("excluded", {}))

    def view(self, query: str, keys: list[str]) -> tuple[set[str], str | None]:
        """The excluded keys among `keys` (shown results in rank order) and the cut's key, once the cut has
        covered every result below it that it had not seen."""
        q = self.state["queries"].get(query)
        if q is None:
            return set(), None
        cut = q["cut"]
        if cut and cut["key"] in keys and not set(keys) <= set(cut["seen"]):
            below = range(keys.index(cut["key"]) + 1, len(keys))
            self._exclude(q, keys, [i for i in below if keys[i] not in cut["seen"]], "cut")
            cut["seen"] = sorted(set(cut["seen"]) | set(keys))
            self._save()
        return set(q["excluded"]) & set(keys), cut["key"] if cut else None

    @staticmethod
    def _exclude(q: dict, keys: list[str], ranks, by: str) -> None:
        for i in ranks:
            q["excluded"].setdefault(keys[i], {"rank": i + 1, "at": time.time(), "by": by})

    def apply(self, query: str, keys: list[str], action: str, key: str | None) -> None:
        """Exclude or include one shown result, cut below one (replacing any earlier cut), or undo the cut."""
        if action not in self.ACTIONS:
            raise ValueError(f"action is one of {', '.join(self.ACTIONS)}")
        if action != "uncut" and key not in keys:
            raise ValueError("name one of the shown results")
        q = self.state["queries"].setdefault(query, {"excluded": {}, "cut": None})
        if action == "exclude":
            self._exclude(q, keys, [keys.index(key)], "click")
        elif action == "include":
            q["excluded"].pop(key, None)
        else:
            q["excluded"] = {k: v for k, v in q["excluded"].items() if v["by"] != "cut"}
            q["cut"] = None
            if action == "cut":
                self._exclude(q, keys, range(keys.index(key) + 1, len(keys)), "cut")
                q["cut"] = {"key": key, "seen": sorted(keys)}
        if not q["excluded"] and not q["cut"]:
            del self.state["queries"][query]
        self._save()
