"""Run Clippy with the toolchain pinned by CI, without installing a toolchain."""

import os
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parents[1]
versions = set(
    re.findall(
        r"uses: dtolnay/rust-toolchain@\S+ # branch ([0-9.]+)",
        (root / ".github/workflows/ci.yml").read_text(),
    )
)
if len(versions) != 1:
    raise SystemExit("CI must pin one Rust toolchain version")
version = versions.pop()
try:
    found = subprocess.run(
        ["rustup", "which", "--toolchain", version, "cargo-clippy"],
        capture_output=True,
        text=True,
    )
except FileNotFoundError:
    raise SystemExit(
        f"install Rust {version} with Clippy before running just check"
    ) from None
if found.returncode or not Path(found.stdout.strip()).is_file():
    raise SystemExit(f"install Rust {version} with Clippy before running just check")
env = dict(os.environ)
env["PATH"] = str(Path(found.stdout.strip()).parent) + os.pathsep + env["PATH"]
raise SystemExit(
    subprocess.call(
        [
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        cwd=root,
        env=env,
    )
)
