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


def read_argv(binary: str, root, key: str) -> list[str]:
    """The command that prints a page's text through the shared reader."""
    return command(binary, root, ["read", "--", key])


def read_pages(argvs: list[list[str]], *, strict: bool = True) -> list[dict | None]:
    """Structured answers, including empty bodies and source refusals. Strict callers abort on failed batches."""
    batches: dict[tuple[str, ...], list[str]] = {}
    for argv in argvs:
        batches.setdefault(tuple(argv[:-2]), []).append(argv[-1])
    pages = {
        (head, key): page
        for head, keys in batches.items()
        for start in range(0, len(keys), READ_BATCH_SIZE)
        for key, page in _read(list(head), keys[start : start + READ_BATCH_SIZE], strict=strict).items()
    }
    return [pages.get((tuple(argv[:-2]), argv[-1])) for argv in argvs]


def _read(head: list[str], keys: list[str], *, strict: bool) -> dict[str, dict]:
    """Answers from one run on exit 0 (all answered) or 1 (some refused)."""
    try:
        done = subprocess.run(
            [*head, "--json", "--", *keys],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=READ_TIMEOUT * len(keys),
        )
    except (OSError, subprocess.TimeoutExpired):
        if strict:
            raise RuntimeError("page reader could not complete the batch") from None
        return {}
    if done.returncode not in (0, 1):
        if strict:
            raise RuntimeError("page reader failed the batch")
        return {}
    pages = {}
    for line in done.stdout.decode("utf-8", errors="strict" if strict else "replace").split("\n"):
        if not line:
            continue
        try:
            row = json.loads(line)
        except ValueError:
            if strict:
                raise ValueError("page reader returned malformed JSON") from None
            continue
        if not isinstance(row, dict) or not isinstance(row.get("url"), str):
            if strict:
                raise ValueError("page reader returned a malformed answer")
            continue
        if strict:
            error = row.get("error")
            refused = isinstance(error, dict) and error.get("kind") == "source_read"
            if (
                row.get("schema_version") != 1
                or row["url"] not in keys
                or row["url"] in pages
                or not isinstance(row.get("title"), str | None)
                or (not isinstance(row.get("text"), str) and not refused)
                or (error is not None and not refused)
            ):
                raise ValueError("page reader returned an unexpected answer")
        elif not isinstance(row.get("text"), str):
            continue
        pages[row["url"]] = row
    if strict and set(pages) != set(keys):
        raise ValueError("page reader did not answer every key")
    return pages
