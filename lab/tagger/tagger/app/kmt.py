"""How the app runs knowmoretabs: for Add link, `App.binary` (`tagger app --kmt-bin`, the binary `just up` serves
the Library with; else this name, found on PATH) on the app's own archive, and never on a snapshot copy inside the
app's data directory; for local documents' text, `knowmoretabs --json read <key>…`, so a vector covers what its
document says without the archive or the lab keeping a copy, and git and PDF reading stay in knowmoretabs alone.
"""

import json
import subprocess

BINARY = "knowmoretabs"
# Keys `add --file` mints for local documents (`src/source_key.rs`)
SOURCES = ("kmt-git:", "kmt-blob:")
READ_TIMEOUT = 60  # seconds each document's text may take: a git read and a PDF parse
READ_BATCH_SIZE = 256  # keys each `read` run takes, so a large library stays under the process argument limit


def snapshot(app) -> bool:
    """The app's archive lies inside its data directory (a snapshot copy), so nothing may write it. Both paths are
    resolved so an alias cannot write into a snapshot copy."""
    return app.lib.root.resolve().is_relative_to(app.lib.paths.data.resolve())


def command(binary: str, root, args: list[str]) -> list[str]:
    return [binary, "--root", str(root), *args]


def argv(app, args: list[str]) -> list[str]:
    return command(app.binary, app.lib.root, args)


def read_argv(binary: str, root, key: str) -> list[str] | None:
    """The command that prints a local document's text, or None for any other page."""
    return command(binary, root, ["read", "--", key]) if key.startswith(SOURCES) else None


def read(argvs: list[list[str]]) -> list[str]:
    """The text `read` prints for each `read_argv`, or "" where it gives none: a refused key, no binary, a timeout.
    One `--json read` per binary and archive answers each bounded batch of keys. Never shown; only embedded."""
    batches: dict[tuple[str, ...], list[str]] = {}
    for argv in argvs:
        batches.setdefault(tuple(argv[:-2]), []).append(argv[-1])
    texts = {
        (head, key): text
        for head, keys in batches.items()
        for start in range(0, len(keys), READ_BATCH_SIZE)
        for key, text in _read(list(head), keys[start : start + READ_BATCH_SIZE]).items()
    }
    return [texts.get((tuple(argv[:-2]), argv[-1]), "") for argv in argvs]


def _read(head: list[str], keys: list[str]) -> dict[str, str]:
    """Each answered key's text from one `read` run; its lines are parsed on exit 0 (all answered) or 1 (some refused)."""
    try:
        done = subprocess.run(
            [*head, "--json", "--", *keys],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=READ_TIMEOUT * len(keys),
        )
    except (OSError, subprocess.TimeoutExpired):
        return {}
    if done.returncode not in (0, 1):
        return {}
    texts = {}
    for line in done.stdout.decode("utf-8", errors="replace").splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if isinstance(row, dict) and isinstance(row.get("text"), str):
            texts[row["url"]] = row["text"]
    return texts
