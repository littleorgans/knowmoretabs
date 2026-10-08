"""Add link: each action on one page is a job, a thread that runs knowmoretabs on the app's archive and keeps what
it reports, so the screen can poll it. `add` and `add --signed-in` stream the contract's `--json` stage events
(library, content, image, done); `restore` runs before an `add`; `forget` stands alone. Once knowmoretabs lists the
page, the job brings the app's library in line with the archive under the app's lock (the Search stage: the page
embedded and indexed, or its files and visibility refreshed). Job state has its own lock, so a poll never waits on
the model. Nothing a job reads from knowmoretabs is printed; its stderr is dropped.
"""

import itertools
import json
import subprocess
import threading
import time
from dataclasses import dataclass, field

from ..archive import Archive

STAGES = ("library", "content", "image", "search")
LISTED = ("added", "known")  # `library` done values after which the library lists the page
KEEP = 32  # finished jobs kept for polling
MAX_URL = 8192


def _add(*flags: str):
    return lambda url: [["add", "--json", *flags, "--", url]]


ACTIONS = {
    "add": _add(),
    "signed_in": _add("--signed-in"),
    "restore": lambda url: [["restore", "--", url], *_add()(url)],
    "forget": lambda url: [["forget", "--", url]],
}


@dataclass
class Job:
    id: int
    url: str
    action: str
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
        return {
            "id": self.id,
            "url": self.url,
            "action": self.action,
            "stages": self.stages,
            "finished": self.ended is not None,
            "failed": self.failed,
            "seconds": round((self.ended or time.monotonic()) - self.started, 1),
            "page": self.page,
        }


class Jobs:
    def __init__(self, app, binary: str = "knowmoretabs"):
        self.app, self.binary = app, binary  # the binary: a name found on PATH, or a path
        self.lock = threading.Lock()  # the job table and job state only
        self.jobs: dict[int, Job] = {}
        self.ids = itertools.count(1)

    def start(self, body: dict) -> dict:
        url, action = body.get("url"), body.get("action", "add")
        if not isinstance(url, str) or not url.strip() or len(url) > MAX_URL:
            raise ValueError("give a link")
        if action not in ACTIONS:
            raise ValueError(f"action is one of {', '.join(ACTIONS)}")
        with self.lock:
            job = Job(next(self.ids), url, action)
            if action != "forget":
                job.stages["library"] = {"stage": "library", "state": "running"}
            self.jobs[job.id] = job
            for old in sorted(self.jobs)[:-KEEP]:
                if self.jobs[old].ended is not None:
                    del self.jobs[old]
            view = job.view()
        threading.Thread(target=self._run, args=(job,), daemon=True).start()
        return view

    def view(self, job_id: int) -> dict:
        with self.lock:
            if job_id not in self.jobs:
                raise LookupError
            return self.jobs[job_id].view()

    def _run(self, job: Job) -> None:
        try:
            ok = all(self._command(job, args) for args in ACTIONS[job.action](job.url))
            if job.action == "forget":
                if ok:
                    self._sync(job)
                    with self.lock:
                        job.stages = {"library": {"stage": "library", "state": "done", "value": "forgotten"}}
                else:
                    job.failed = True
            elif job.done is None:
                job.failed = True
            elif job.done.get("value") in LISTED:
                self._search(job)
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
