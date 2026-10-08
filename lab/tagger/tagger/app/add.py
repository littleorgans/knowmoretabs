"""Add link: each action on one page is a job, a thread that runs knowmoretabs on the app's archive and keeps what
it reports, so the screen can poll it. `add` streams the contract's `--json` stage events (library, content, image,
done), with the link's title, `--signed-in` (Try signed in) or `--retry` naming the failed stages (Retry, which runs
only those); `restore` runs before a plain `add`; `forget` stands alone. Once knowmoretabs lists the page, the job
brings the app's library in line with the archive under the app's lock (the Search stage: the page embedded and
indexed, or its files and visibility refreshed). `index` retries Search in the lab only. Roots inside the lab data
directory are refused before a worker starts. Job state has its own lock, so a poll never waits on the model.
Nothing a job reads from knowmoretabs is printed; its stderr is dropped.
"""

import itertools
import json
import subprocess
import threading
import time
from copy import deepcopy
from dataclasses import dataclass, field

from ..archive import Archive

STAGES = ("library", "content", "image", "search")
LISTED = ("added", "known")  # `library` done values after which the library lists the page
KEEP = 32  # finished jobs kept for polling
MAX = 8192  # the longest link or title taken
ACTIONS = ("add", "restore", "forget", "index")
RETRY = ("content", "image")  # the stages `add --retry` runs again


def commands(job: "Job") -> list[list[str]]:
    """The knowmoretabs runs a job makes, in order; `index` makes none (the lab's Search only)."""
    flags = [f for stage in job.retry for f in ("--retry", stage)] + (["--signed-in"] if job.signed_in else [])
    flags += [f"--title={job.title}"] if job.title else []  # `=`: a title may start with a dash
    add = ["add", "--json", *flags, "--", job.url]
    return {"add": [add], "restore": [["restore", "--", job.url], add], "forget": [["forget", "--", job.url]]}.get(
        job.action, []
    )


@dataclass
class Job:
    id: int
    url: str
    action: str
    retry: tuple[str, ...] = ()  # the stages an `add` retries
    signed_in: bool = False
    title: str | None = None  # the link's title, for its intake line
    restored: bool = False  # a `restore` succeeded; Retry adds only
    started: float = field(default_factory=time.monotonic)
    ended: float | None = None
    stages: dict = field(default_factory=dict)  # stage -> its latest event
    done: dict | None = None  # the closing `done` line
    failed: bool = False  # knowmoretabs could not run, or ended without a result
    page: dict | None = None  # the page as the screen shows it, once indexed

    def take(self, line: str) -> None:
        """One stdout line: a stage event replaces that stage's last; anything else is ignored."""
        try:
            event = json.loads(line)
        except ValueError:
            return
        if not isinstance(event, dict) or not isinstance(event.get("state"), str):
            return
        if event.get("stage") == "done":
            self.done = event
        elif event.get("stage") in STAGES[:3]:
            self.stages[event["stage"]] = event

    def view(self) -> dict:
        return deepcopy(
            {
                "id": self.id,
                "url": self.url,
                "action": self.action,
                "retry": list(self.retry),
                "signed_in": self.signed_in,
                "title": self.title,
                "restored": self.restored,
                "stages": self.stages,
                "finished": self.ended is not None,
                "failed": self.failed,
                "seconds": round((self.ended or time.monotonic()) - self.started, 1),
                "page": self.page,
            }
        )


class Jobs:
    def __init__(self, app, binary: str = "knowmoretabs"):
        self.app, self.binary = app, binary  # the binary: a name found on PATH, or a path
        self.lock = threading.Lock()  # the job table and job state only
        self.jobs: dict[int, Job] = {}
        self.ids = itertools.count(1)

    def start(self, body: dict) -> dict:
        url, action, retry = body.get("url"), body.get("action", "add"), body.get("retry", [])
        signed_in, title = body.get("signed_in", False), body.get("title")
        if not isinstance(url, str) or not url.strip() or len(url) > MAX:
            raise ValueError("give a link")
        if action not in ACTIONS:
            raise ValueError(f"action is one of {', '.join(ACTIONS)}")
        if not isinstance(retry, list) or not set(retry) <= set(RETRY) or len(set(retry)) != len(retry):
            raise ValueError(f"retry lists stages among {', '.join(RETRY)}")
        if not isinstance(signed_in, bool) or not (title is None or isinstance(title, str) and len(title) <= MAX):
            raise ValueError("signed_in is true or false; a title is text")
        if (action != "add" and (retry or signed_in or title)) or (retry and title):
            raise ValueError("only an add takes flags, and a Retry no title")
        retry, title = tuple(s for s in RETRY if s in retry), (title or "").strip() or None
        with self.lock:
            job = Job(next(self.ids), url, action, retry, signed_in, title)
            # The store lives at <data>/app. Resolve both paths so an alias cannot write into a snapshot copy.
            if self.app.lib.root.resolve().is_relative_to(self.app.store.root.parent.resolve()):
                job.stages["library"] = {"stage": "library", "state": "done", "value": "refused", "reason": "snapshot"}
                job.ended = time.monotonic()
            elif action == "index":
                job.stages["library"] = {"stage": "library", "state": "done", "value": "known"}
            elif action != "forget":
                job.stages["library"] = {"stage": "library", "state": "running"}
            self.jobs[job.id] = job
            for old in sorted(self.jobs)[:-KEEP]:
                if self.jobs[old].ended is not None:
                    del self.jobs[old]
            view = job.view()
        if job.ended is None:
            threading.Thread(target=self._run, args=(job,), daemon=True).start()
        return view

    def view(self, job_id: int) -> dict:
        with self.lock:
            if job_id not in self.jobs:
                raise LookupError
            return self.jobs[job_id].view()

    def _run(self, job: Job) -> None:
        try:
            if job.action == "index":
                self._search(job)
                return
            ok = True
            for args in commands(job):
                ok = self._command(job, args)
                if not ok:
                    break
                if args[0] == "restore":
                    with self.lock:
                        job.restored = True
            if job.action == "forget":
                if ok:
                    self._sync(job)
                    with self.lock:
                        job.stages = {"library": {"stage": "library", "state": "done", "value": "forgotten"}}
                else:
                    with self.lock:
                        job.failed = True
            elif job.done is None:
                with self.lock:
                    job.failed = True
            elif job.done.get("value") in LISTED:
                self._search(job)
        except Exception:  # A command or archive read failed; finish the job so the screen can offer Retry.
            with self.lock:
                job.failed = True
        finally:
            with self.lock:
                job.ended = time.monotonic()

    def _command(self, job: Job, args: list[str]) -> bool:
        """Run knowmoretabs on the app's archive, taking its stdout line by line; true on exit 0. A non zero exit
        is a refusal the events report (forgotten, not a web page) or a failure the job reports."""
        try:
            proc = subprocess.Popen(
                [self.binary, "--root", str(self.app.lib.root), *args],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
            )
        except OSError:
            return False
        with proc:
            for line in proc.stdout:
                with self.lock:
                    job.take(line)
        return proc.returncode == 0

    def _sync(self, job: Job) -> dict | None:
        archive = Archive.load(self.app.lib.root)  # outside the app's lock: file reads only
        with self.app.lock:
            return self.app.sync(job.url, archive)

    def _search(self, job: Job) -> None:
        with self.lock:
            job.stages["search"] = {"stage": "search", "state": "running"}
        try:
            page = self._sync(job)
        except Exception:  # an embed or index failure: the page joins at the next start; the screen offers Retry
            page = None
        with self.lock:
            job.page = page
            value = "indexed" if page is not None else "not_indexed"
            job.stages["search"] = {"stage": "search", "state": "done", "value": value}
