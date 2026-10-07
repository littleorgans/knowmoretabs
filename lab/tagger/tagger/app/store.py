"""The app's decisions, in one private file under <data>/app/ replaced whole on every change.

A session is one search and its picked tags: the result pages, by key, each with the model's prechecks, the
user's marks and a status (open, skipped, decided). Export folds every decided page over all sessions,
latest decision per page and tag, into a `tag --import` answers file and a decision log. Results marked not
relevant to a query live apart, in `Exclusions`, and are never exported. Nothing here touches an archive.
"""

import json
import os
import time
from datetime import UTC, datetime
from pathlib import Path

SOURCE = "kmt-tagger-app"
STATUSES = ("open", "skipped", "decided")


def _write(path: Path, text: str) -> None:
    """Stage and rename, so a crash leaves the old file or the new one; 0600 under the CLI's umask."""
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")
    part.write_text(text)
    os.chmod(part, 0o600)
    os.replace(part, path)


class Store:
    def __init__(self, root: Path):
        self.root = root
        self.path = root / "state.json"
        self.state = json.loads(self.path.read_text()) if self.path.exists() else {"version": 1, "sessions": []}

    def _save(self) -> None:
        _write(self.path, json.dumps(self.state, indent=1, ensure_ascii=False) + "\n")

    def sessions(self) -> list[dict]:
        return self.state["sessions"]

    def session(self, sid: int) -> dict | None:
        return next((s for s in self.state["sessions"] if s["id"] == sid), None)

    def create(self, query: str, images: bool, keys: list[str], picked: list[str], prechecks: list[list[dict]]) -> dict:
        """`prechecks[i]`: the model's suggestions for page `keys[i]` (engine.prechecks)."""
        session = {
            "id": max((s["id"] for s in self.state["sessions"]), default=0) + 1,
            "created_at": time.time(),
            "query": query,
            "images": images,
            "picked": picked,
            "pages": [
                {
                    "key": key,
                    "model": {s["tag"]: s["checked"] for s in sugg},
                    "p": {s["tag"]: round(s["p"], 4) for s in sugg},
                    "source": {s["tag"]: s["source"] for s in sugg},
                    "marks": {s["tag"]: s["checked"] for s in sugg},
                    "status": "open",
                    "at": None,
                }
                for key, sugg in zip(keys, prechecks, strict=True)
            ],
        }
        self.state["sessions"].append(session)
        self._save()
        return session

    def update(self, sid: int, index: int, marks: dict | None, status: str | None) -> dict:
        """Set a page's marks (only tags it was offered) and status; `at` orders decisions and skips."""
        page = self.session(sid)["pages"][index]
        if marks is not None:
            unknown = set(marks) - set(page["marks"])
            if unknown or not all(isinstance(v, bool) for v in marks.values()):
                raise ValueError("marks name only the page's offered tags, as true or false")
        if status is not None and status not in STATUSES:
            raise ValueError(f"status is one of {', '.join(STATUSES)}")
        if status == "decided" and page["status"] != "decided":
            page["before_marks"] = page["marks"].copy()
        elif status == "open" and "before_marks" in page:
            page["marks"] = page.pop("before_marks")
        if marks is not None:
            page["marks"].update(marks)
        if status is not None:
            page["status"] = status
        if status is not None or (marks is not None and page["status"] == "decided"):
            page["at"] = None if page["status"] == "open" else time.time()
        self._save()
        return page

    def decisions(self) -> dict[tuple[str, str], dict]:
        """The latest decision per (page key, tag) over every session's decided pages."""
        latest = {}
        for s in self.state["sessions"]:
            for page in s["pages"]:
                if page["status"] != "decided":
                    continue
                for tag, value in page["marks"].items():
                    k = (page["key"], tag)
                    if k not in latest or page["at"] >= latest[k]["at"]:
                        latest[k] = {"value": value, "model": page["model"][tag], "session": s["id"], "at": page["at"]}
        return latest

    def export(self) -> dict:
        """answers.jsonl (pages with a kept tag, the kept tags) and decisions.jsonl (every page, tag and
        answer) in a fresh folder under exports/; nothing else sits beside them, so `tag --import` finds no
        prompt there to check coverage against."""
        latest = self.decisions()
        kept: dict[str, list[str]] = {}
        for (key, tag), d in sorted(latest.items()):
            if d["value"]:
                kept.setdefault(key, []).append(tag)
        stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
        folder = self.root / "exports" / stamp
        n = 1
        while folder.exists():
            n += 1
            folder = self.root / "exports" / f"{stamp}-{n}"
        answers = "".join(
            json.dumps({"url": k, "tags": t, "source": SOURCE}, ensure_ascii=False) + "\n" for k, t in kept.items()
        )
        log = "".join(
            json.dumps(
                {
                    "url": key,
                    "tag": tag,
                    "answer": "yes" if d["value"] else "no",
                    "model": "yes" if d["model"] else "no",
                    "session": d["session"],
                    "at": datetime.fromtimestamp(d["at"], UTC).isoformat(timespec="seconds"),
                },
                ensure_ascii=False,
            )
            + "\n"
            for (key, tag), d in sorted(latest.items())
        )
        _write(folder / "answers.jsonl", answers)
        _write(folder / "decisions.jsonl", log)
        return {
            "folder": str(folder),
            "answers": str(folder / "answers.jsonl"),
            "decisions": str(folder / "decisions.jsonl"),
            "answer_pages": len(kept),
            "answer_tags": sum(len(t) for t in kept.values()),
            "decided": len(latest),
            "flipped": sum(d["value"] != d["model"] for d in latest.values()),
            "source": SOURCE,
        }


class Exclusions:
    """Results marked not relevant to a query, in <data>/app/not-relevant.json, apart from the tag decisions.

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
