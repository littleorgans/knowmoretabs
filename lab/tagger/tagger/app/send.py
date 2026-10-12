"""Send to library: the library made equal to the app, through knowmoretabs. The app is the source of truth.

`preview` plans against the archive as it stands and writes nothing. Every name the app made or decided on joins the
library's vocabulary unless the vocabulary holds it, active or retired. Every page the library holds ends with
exactly the app's tags for it, as the owner's own (names matched without regard to case, as knowmoretabs matches
them); a page the app holds no tag for ends with none. A retired name in a page's `add` list counts as on the page
once the send brings the name back. An app "no" becomes a dismissal (the page's `remove` list) where the library
holds the name on neither list for that page; a retired name takes it without coming back. Pages only the app
forgot are forgotten; pages the library forgot stay forgotten and are counted, since Send never restores. Pages the
app holds that the library does not are left out and counted. Nothing is remembered between sends: each plans
afresh from the archive, so sending again after a partial failure converges. `apply` runs the plan the owner saw:
the export record, then `tags --create=NAME…`, then per tag `tag --add=NAME`, then `tag --remove=NAME` for tags
coming off, then for dismissals, then `forget`. Each call is atomic in knowmoretabs (one `library.json` replace under
its lock); the send is not. A failed call stops it; the calls before it stay applied. A call's stderr goes to the
screen only.
"""

import os
import shlex
import subprocess
import threading
from contextlib import contextmanager
from dataclasses import dataclass, field

from ..archive import Archive
from . import engine, kmt

TIMEOUT = 120  # seconds per call; knowmoretabs waits for the archive lock
ARGS = 100_000  # bytes of arguments per call, well under the system's argument limit


@dataclass
class Call:
    args: list[str]
    urls: list[str]

    def command(self, app) -> str:
        return shlex.join(kmt.argv(app, self.args))


@dataclass
class Plan:
    on: dict[str, list[str]] = field(default_factory=dict)  # tag -> pages it goes on
    off: dict[str, list[str]] = field(default_factory=dict)  # tag -> pages it comes off
    never: dict[str, list[str]] = field(default_factory=dict)  # of `off`, on pages the app never tagged
    cleared: int = 0  # pages showing tags that end with none
    dismiss: dict[str, list[str]] = field(default_factory=dict)  # tag -> pages it is dismissed on
    new: list[str] = field(default_factory=list)  # names the vocabulary never held, created first
    revived: list[str] = field(default_factory=list)  # of `on`, names it holds retired
    forget: list[str] = field(default_factory=list)
    kept_forgotten: int = 0  # pages the library forgot and the app did not, left forgotten
    missing: int = 0  # pages the app holds and the library does not

    def calls(self) -> list[Call]:
        # `=`: a name may start with a dash
        out = [Call(["tags", *part], []) for part in chunks([f"--create={tag}" for tag in self.new])]
        for verb, tags in (("add", self.on), ("remove", self.off), ("remove", self.dismiss)):
            for tag, urls in sorted(tags.items()):
                out += [Call(["tag", f"--{verb}={tag}", "--", *part], part) for part in chunks(urls)]
        return out + [Call(["forget", "--", *part], part) for part in chunks(self.forget)]

    def summary(self) -> dict:
        def count(tags: dict[str, list[str]]) -> tuple[int, int]:
            return sum(map(len, tags.values())), len({u for urls in tags.values() for u in urls})

        (tags_on, pages_on), (tags_off, pages_off), (never_tags, never_pages), (dismiss_tags, dismiss_pages) = map(
            count, (self.on, self.off, self.never, self.dismiss)
        )
        return {
            "tags_on": tags_on,
            "pages_on": pages_on,
            "tags_off": tags_off,
            "pages_off": pages_off,
            "never_tags": never_tags,
            "never_pages": never_pages,
            "cleared": self.cleared,
            "dismiss_tags": dismiss_tags,
            "dismiss_pages": dismiss_pages,
            "new": self.new,
            "revived": self.revived,
            "forget": len(self.forget),
            "kept_forgotten": self.kept_forgotten,
            "missing": self.missing,
            "ready": bool(self.new or self.on or self.off or self.dismiss or self.forget),
        }


def chunks(urls: list[str]) -> list[list[str]]:
    out, size = [], ARGS
    for url in urls:
        width = len(os.fsencode(url)) + 1
        if size + width > ARGS:
            out.append([])
            size = 0
        out[-1].append(url)
        size += width
    return out


def plan(store, forgotten: list[str], archive: Archive) -> Plan:
    known, app_tags, decisions, p = set(archive.known), store.app_tags(), store.decisions(), Plan()
    decided = {key for key, _ in decisions}
    # one spelling per name, over every name the app made or decided on
    named = {tag.lower(): tag for tag in sorted({*store.tags(), *(tag for _, tag in decisions)}, reverse=True)}
    no: dict[str, set[str]] = {}  # page -> folded names its latest decisions take off
    for (key, tag), d in decisions.items():
        if not d["value"]:
            no.setdefault(key, set()).add(tag.lower())
    pages = [
        (key, {t.lower(): t for t in app_tags.get(key, ())}, {n.lower(): n for n in archive.state.adds.get(key, ())})
        for key in archive.known
    ]
    for key, want, has in pages:
        for folded in sorted(want.keys() - (has.keys() & archive.state.active.keys())):
            p.on.setdefault(want[folded], []).append(key)
    after = archive.state.active.keys() | {tag.lower() for tag in p.on}  # names shown once the adds ran
    for key, want, has in pages:
        for folded in sorted((has.keys() & after) - want.keys()):
            p.off.setdefault(has[folded], []).append(key)
            if key not in decided:
                p.never.setdefault(has[folded], []).append(key)
        p.cleared += bool(has.keys() & archive.state.active.keys() and not want)
        dismissed = {n.lower() for n in archive.state.removes.get(key, ())}
        for folded in sorted(no.get(key, set()) - want.keys() - has.keys() - dismissed):
            p.dismiss.setdefault(named[folded], []).append(key)
    adding = {tag.lower() for tag in p.on}
    for folded, tag in sorted(named.items()):
        if folded not in archive.state.active:
            if folded not in archive.state.retired:
                p.new.append(tag)
            elif folded in adding:
                p.revived.append(tag)
    gone = set(forgotten)
    p.forget = [key for key in forgotten if key in known and key not in archive.state.forgotten]
    p.kept_forgotten = len(archive.state.forgotten - gone)
    p.missing = len(({key for key, _ in decisions} | gone) - known)
    return p


def run(app, call: Call) -> str | None:
    """Run one call; None when it exits 0, else what went wrong, for the screen."""
    try:
        done = subprocess.run(
            kmt.argv(app, call.args), stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=TIMEOUT
        )
    except subprocess.TimeoutExpired:
        return f"knowmoretabs did not finish within {TIMEOUT} seconds"
    except OSError as err:
        return f"could not run knowmoretabs: {err.strerror or err}"
    return None if done.returncode == 0 else done.stderr.strip() or f"knowmoretabs exited {done.returncode}"


class Send:
    def __init__(self, app):
        self.app = app
        self.running = threading.Lock()  # one send at a time
        self.pending: Plan | None = None  # the plan the owner was shown
        self.preview_id = 0

    @contextmanager
    def _exclusive(self):
        if not self.running.acquire(blocking=False):
            raise ValueError("a send is running")
        try:
            yield
        finally:
            self.running.release()

    def preview(self) -> dict:
        with self._exclusive():
            if kmt.snapshot(self.app):
                raise ValueError("this app runs on a snapshot copy, which Send never writes")
            archive = Archive.load(self.app.lib.root)  # outside the app's lock: file reads only
            with self.app.lock:
                p = plan(self.app.store, self.app.forgotten.keys, archive)
                self.pending = p
                self.preview_id += 1
            return {**p.summary(), "preview": self.preview_id, "commands": [c.command(self.app) for c in p.calls()]}

    def apply(self, preview: int | None) -> dict:
        with self._exclusive():
            p = self.pending
            if p is None:
                raise ValueError("nothing previewed to send")
            archive = Archive.load(self.app.lib.root)
            steps, error, record, ran = [], None, None, []
            with self.app.lock:
                if preview != self.preview_id:
                    raise ValueError("preview replaced; preview again")
                if p != plan(self.app.store, self.app.forgotten.keys, archive):
                    raise ValueError("decisions or library changed; preview again")
                self.pending = None
                try:
                    record = self.app.store.export(self.app.forgotten.keys)
                except OSError as err:
                    error = f"could not write the send record: {err.strerror or err}"
            for call in p.calls() if error is None else []:
                ran.append(call)
                error = run(self.app, call)
                steps.append({"command": call.command(self.app), "ok": error is None})
                if error is not None:
                    break
            if hidden := [url for call in ran if call.args[0] == "forget" for url in call.urls]:
                try:
                    self._hide(hidden)
                except Exception as err:  # an embed or index failure: the pages hide right at the next start
                    error = error or f"the app could not hide the forgotten pages until a restart: {err}"
            return {**p.summary(), "ok": error is None, "error": error, "steps": steps, "record": record}

    def _hide(self, urls: list[str]) -> None:
        """Pages a forget touched, hidden in the app as the archive now has them (a failed call too: it may have
        written before it failed)."""
        archive = Archive.load(self.app.lib.root)  # outside the app's lock: file reads only
        with self.app.lock:
            self.app.lib = engine.sync(self.app.lib, archive, urls, self.app.embed)
