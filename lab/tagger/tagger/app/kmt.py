"""How the app runs knowmoretabs: for Add link, `App.binary` (`tagger app --kmt-bin`, the binary `just up` serves
the Library with; else this name, found on PATH) on the app's own archive, and never on a snapshot copy inside the
app's data directory; for a local document's text, `knowmoretabs read <key>`, so its vector covers what the document
says without the archive or the lab keeping a copy, and its git and PDF reading stay in knowmoretabs alone.
"""

import subprocess

BINARY = "knowmoretabs"
# Keys `add --file` mints for local documents (`src/source_key.rs`)
SOURCES = ("kmt-git:", "kmt-blob:")
READ_TIMEOUT = 60  # seconds a document's text may take: a git read and a PDF parse


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


def read(argv: list[str]) -> str:
    """The text `read` printed, or "" when it printed none: no binary, an unresolved document, a timeout. Never shown;
    only embedded."""
    try:
        done = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=READ_TIMEOUT)
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return done.stdout.decode("utf-8", errors="replace") if done.returncode == 0 else ""
