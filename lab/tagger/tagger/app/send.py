"""Send to library: the app's tags and forgotten pages, applied to its archive as the owner's own, by knowmoretabs.

`preview` plans against the archive as it stands and writes nothing. Per page and tag, the app's latest decision
(a decision Undo took back with none before it counts as off) where it differs from what earlier sends left; a page
forgotten in the app, or one the library does not hold, is left out. A pair the library already holds as wanted
needs no call. Then the pages forgotten in the app that the library still shows. `apply` runs the plan the owner
saw: the export record, then `tag <urls> --add NAME` or `--remove NAME` per tag and value, then `forget`. Each call
is atomic in knowmoretabs (one `library.json` replace under its lock); the send is not. A failed call stops it:
the calls before it stay applied and marked sent, so sending again runs only the rest. The sent marks also keep a
later send from undoing what the owner changed in the Library since. A call's stderr goes to the screen only.
"""

import shlex
import subprocess
import threading
from dataclasses import dataclass, field

from ..archive import Archive
from . import kmt

TIMEOUT = 120  # seconds per call; knowmoretabs waits for the archive lock
ARGS = 100_000  # bytes of addresses per call, well under the system's argument limit


@dataclass
class Call:
    args: list[str]
    urls: list[str]
    tag: str | None = None  # a tag call's name and value; None for a forget
    value: bool = True

    def command(self, app) -> str:
        return shlex.join(kmt.argv(app, self.args))


@dataclass
class Plan:
    on: dict[str, list[str]] = field(default_factory=dict)  # tag -> pages it goes on
    off: dict[str, list[str]] = field(default_factory=dict)  # tag -> pages it comes off
    held: list[tuple[str, str, bool]] = field(default_factory=list)  # already so in the library
    new: list[str] = field(default_factory=list)  # names the vocabulary lacks
    revived: list[str] = field(default_factory=list)  # names it holds retired
    forget: list[str] = field(default_factory=list)
    settled: list[str] = field(default_factory=list)  # forgotten in the app and in the library already
    skipped: int = 0  # pages the library does not hold

    def calls(self) -> list[Call]:
        out = []
        for value, tags in ((True, self.on), (False, self.off)):
            for tag, urls in sorted(tags.items()):
                flag = f"--{'add' if value else 'remove'}={tag}"  # `=`: a name may start with a dash
                out += [Call(["tag", flag, "--", *part], part, tag, value) for part in chunks(urls)]
        return out + [Call(["forget", "--", *part], part) for part in chunks(self.forget)]

    def summary(self) -> dict:
        return {
            "tags_on": sum(map(len, self.on.values())),
            "pages_on": len({u for urls in self.on.values() for u in urls}),
            "tags_off": sum(map(len, self.off.values())),
            "pages_off": len({u for urls in self.off.values() for u in urls}),
            "new": self.new,
            "revived": self.revived,
            "forget": len(self.forget),
            "skipped": self.skipped,
            "matched": len(self.held),
            "settled": len(self.settled),
            "ready": bool(self.on or self.off or self.forget or self.held or self.settled),
        }


def chunks(urls: list[str]) -> list[list[str]]:
    out, size = [], ARGS
    for url in urls:
        if size + len(url) + 1 > ARGS:
            out.append([])
            size = 0
        out[-1].append(url)
        size += len(url) + 1
    return out


def plan(store, forgotten: list[str], archive: Archive) -> Plan:
    known, gone, p = set(archive.known), set(forgotten), Plan()
    wanted = {k: d["value"] for k, d in store.decisions().items()}
    sent = store.sent()
    for pair in sent:
        wanted.setdefault(pair, False)
    adding = {
        tag.lower()
        for (key, tag), value in wanted.items()
        if value and sent.get((key, tag)) != value and key in known and key not in gone
    }
    skipped = set()
    for (key, tag), value in sorted(wanted.items()):
        if sent.get((key, tag)) == value or key in gone:
            continue
        if key not in known:
            skipped.add(key)
            continue
        folded = tag.lower()
        held = (
            folded in {t.lower() for t in archive.owner_tags.get(key, ())}
            if value
            else (
                folded in {t.lower() for t in archive.tag_removals.get(key, ())}
                or folded not in archive.active
                and folded not in archive.retired
                and folded not in adding
            )
        )
        if held:
            p.held.append((key, tag, value))
            continue
        (p.on if value else p.off).setdefault(tag, []).append(key)
    for tag in sorted(p.on, key=str.lower):
        if tag.lower() not in archive.active:
            (p.revived if tag.lower() in archive.retired else p.new).append(tag)
    for key in forgotten:
        if key not in known:
            skipped.add(key)
        elif key in archive.forgotten:
            p.settled.append(key)
        else:
            p.forget.append(key)
    p.skipped = len(skipped)
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

    def preview(self) -> dict:
        if kmt.snapshot(self.app):
            raise ValueError("this app runs on a snapshot copy, which Send never writes")
        archive = Archive.load(self.app.lib.root)  # outside the app's lock: file reads only
        with self.app.lock:
            p = plan(self.app.store, self.app.forgotten.keys, archive)
            self.pending = p
        return {**p.summary(), "commands": [c.command(self.app) for c in p.calls()]}

    def apply(self) -> dict:
        if not self.running.acquire(blocking=False):
            raise ValueError("a send is running")
        try:
            with self.app.lock:
                p, self.pending = self.pending, None
                if p is None:
                    raise ValueError("nothing previewed to send")
                record = self.app.store.export(self.app.forgotten.keys)
                self.app.store.mark_sent(p.held)
            steps, error = [], None
            for call in p.calls():
                error = run(self.app, call)
                steps.append({"command": call.command(self.app), "ok": error is None})
                if error is not None:
                    break
                self._settle(call)
            if error is None and p.settled:
                self._forgotten(p.settled)
            return {**p.summary(), "ok": error is None, "error": error, "steps": steps, "record": record}
        finally:
            self.running.release()

    def _settle(self, call: Call) -> None:
        """What one applied call changed, in the app: a tag call's sent marks, a forget's pages."""
        if call.tag is not None:
            with self.app.lock:
                self.app.store.mark_sent([(url, call.tag, call.value) for url in call.urls])
        else:
            self._forgotten(call.urls)

    def _forgotten(self, urls: list[str]) -> None:
        """Pages the library now hides: in line with the archive in the app, and off its own forget list, so a
        page the owner brings back in the Library is not forgotten again by the next send."""
        archive = Archive.load(self.app.lib.root)
        with self.app.lock:
            for url in urls:
                self.app.sync(url, archive)
            done = set(urls)
            self.app.forgotten.set([k for k in self.app.forgotten.keys if k not in done])
