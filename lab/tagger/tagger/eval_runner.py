"""Private evaluation diagnostics, provenance and result publication."""

import json
import shlex
import subprocess
import traceback
from collections.abc import Callable
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

from .app import kmt
from .paths import Paths, write_json


class Invalid(ValueError):
    """A bounded reason safe to show outside private diagnostics."""


def safe_paths(paths: Paths, root: Path, out: Path) -> None:
    live = (Path.home() / ".knowmoretabs").resolve()
    for path in (root, paths.data, out):
        if path.resolve().is_relative_to(live):
            raise Invalid("scratch paths required outside the live archive")
        if path.exists() and any(p.is_symlink() for p in path.rglob("*")):
            raise Invalid("scratch paths must contain ordinary copies")


def run(
    paths: Paths,
    root: Path,
    out: Path,
    evaluate: Callable[[], dict],
    *,
    stem: str,
    log: Path,
    report: Callable[[dict], str] | None = None,
    command: list[str] | None = None,
    binary: str = kmt.BINARY,
    label: str | None = None,
) -> dict:
    """Keep failures private and publish only finite results to fresh outputs."""
    safe_paths(paths, root, out)
    outputs = [out / f"{stem}.json"]
    if report is not None:
        outputs.append(out / f"{stem}.md")
    if any(path.exists() for path in outputs):
        raise Invalid(f"{label or stem} already exist; choose a separate output directory")
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("a") as stream, redirect_stdout(stream), redirect_stderr(stream):
        try:
            result = evaluate()
            result["source_revision"] = subprocess.run(
                ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
            ).stdout.strip()
            if command is not None:
                result["command"] = shlex.join(
                    [
                        "uv",
                        "run",
                        "--locked",
                        "--project",
                        "lab/tagger",
                        "tagger",
                        "--root",
                        str(root),
                        "--data",
                        str(paths.data),
                        *command,
                        "--out",
                        str(out),
                        "--kmt-bin",
                        binary,
                    ]
                )
            json.dumps(result, allow_nan=False)
            write_json(outputs[0], result)
            if report is not None:
                outputs[1].write_text(report(result))
            return result
        except Exception as err:
            traceback.print_exc()
            raise Invalid(
                str(err) if isinstance(err, Invalid) else "unreadable inputs or unavailable runtime"
            ) from None
