"""Install receipt: do the installed apps match this checkout?

`just install` records a receipt once both installs succeed; `just up`
recomputes it and refuses to launch on any difference. The receipt lives in
the user cache, never the archive. Standard library only, Python 3.9 or
later, so the launcher can run it with whatever `python3` is on PATH.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 1
ADVICE = "run `just install`, then `just up`"

# The files each installed app is built from, relative to the checkout.
INPUTS = {
    "rust": ("Cargo.toml", "Cargo.lock", "build.rs", "src", "web"),
    "tagger": ("lab/tagger/pyproject.toml", "lab/tagger/uv.lock", "lab/tagger/tagger"),
}
# Caches, bytecode, tests and docs never reach an installed app.
SKIP_DIRS = {"__pycache__", "tests", "fixtures"}
SKIP_SUFFIXES = (".pyc", ".pyo", ".md", "_test.rs", "_tests.rs")


class Refused(Exception):
    pass


def kept(name: str) -> bool:
    return not name.startswith(".") and not name.endswith(SKIP_SUFFIXES)


def files(root: Path, entries: tuple[str, ...]) -> list[str]:
    found = []
    for entry in entries:
        path = root / entry
        if path.is_file():
            found.append(entry)
        for top, dirs, names in os.walk(path):
            dirs[:] = [d for d in dirs if d not in SKIP_DIRS and kept(d)]
            base = Path(top).relative_to(root)
            found += [(base / n).as_posix() for n in names if kept(n)]
    return sorted(found)


def fingerprint(root: Path, entries: tuple[str, ...]) -> str:
    """Hash each kept file's relative path and bytes, in path order, so edits, additions and deletions all count."""
    digest = hashlib.sha256()
    for name in files(root, entries):
        for part in (name.encode(), (root / name).read_bytes()):
            digest.update(len(part).to_bytes(8, "big"))
            digest.update(part)
    return digest.hexdigest()


def inputs(root: Path = ROOT) -> dict[str, str]:
    return {name: fingerprint(root, entries) for name, entries in INPUTS.items()}


def receipt(kmt: str, tagger: str, root: Path = ROOT) -> dict:
    for name, path in (("knowmoretabs", kmt), ("tagger", tagger)):
        if not path or not os.path.isfile(path):
            raise Refused(f"{name} is not on PATH")
    return {
        "schema": SCHEMA,
        "inputs": inputs(root),
        "knowmoretabs": {
            "path": os.path.abspath(kmt),
            "sha256": hashlib.sha256(Path(kmt).read_bytes()).hexdigest(),
        },
        "tagger": {"path": os.path.abspath(tagger)},
    }


def location() -> Path:
    cache = os.environ.get("XDG_CACHE_HOME", "")
    if not os.path.isabs(cache):
        cache = Path.home() / (
            "Library/Caches" if sys.platform == "darwin" else ".cache"
        )
    return Path(cache) / "knowmoretabs" / "install.json"


def flat(value, prefix: str = "") -> dict:
    if not isinstance(value, dict):
        return {prefix: value}
    return {
        k: v
        for key, item in value.items()
        for k, v in flat(item, f"{prefix}{key}.").items()
    }


def begin() -> dict[str, str]:
    """Forget the old receipt, so a failed install never leaves one that matches, and return the inputs it starts from."""
    location().unlink(missing_ok=True)
    return inputs()


def record(kmt: str, tagger: str, before: dict, installed_kmt: str) -> None:
    current = receipt(kmt, tagger)
    if not Path(kmt).samefile(installed_kmt):
        raise Refused(
            "PATH selects a different knowmoretabs than cargo just installed; "
            "put the cargo install bin directory first on PATH, then run `just install`"
        )
    if current["inputs"] != before:
        raise Refused("sources changed during the install; run it again")
    path = location()
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", dir=path.parent, prefix=".install-", delete=False
    ) as stage:
        json.dump(current, stage, indent=2, sort_keys=True)
        stage.flush()
        os.fsync(stage.fileno())
    os.replace(stage.name, path)


def check(kmt: str, tagger: str) -> None:
    try:
        found = json.loads(location().read_text())
    except FileNotFoundError:
        raise Refused("no install receipt") from None
    except (OSError, ValueError):
        raise Refused("the install receipt is unreadable") from None
    if not isinstance(found, dict):
        raise Refused("the install receipt is unreadable")
    expected, found = flat(receipt(kmt, tagger)), flat(found)
    changed = sorted(
        k for k in expected.keys() | found.keys() if expected.get(k) != found.get(k)
    )
    if changed:
        raise Refused(
            f"the installed apps do not match this checkout ({', '.join(changed)})"
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser(
        "begin", help="forget the receipt and print the inputs the install starts from"
    )
    record_, check_ = commands.add_parser("record"), commands.add_parser("check")
    record_.add_argument("--before", required=True, help="what begin printed")
    record_.add_argument(
        "--installed-kmt", required=True, help="the binary cargo just installed"
    )
    for command in (record_, check_):
        command.add_argument(
            "--kmt", required=True, help="knowmoretabs as found on PATH"
        )
        command.add_argument("--tagger", required=True, help="tagger as found on PATH")
    args = parser.parse_args(argv)
    try:
        if args.command == "begin":
            print(json.dumps(begin()))
        elif args.command == "record":
            record(args.kmt, args.tagger, json.loads(args.before), args.installed_kmt)
        else:
            check(args.kmt, args.tagger)
    except Refused as refused:
        advice = f"; {ADVICE}" if args.command == "check" else ""
        print(f"knowmoretabs: {refused}{advice}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
