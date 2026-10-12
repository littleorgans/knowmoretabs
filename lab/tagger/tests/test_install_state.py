"""`just up` refuses an install that does not match the checkout, before it starts anything.

Each test runs the real justfile in a synthetic copy of the checkout, with fake cargo, uv and apps on a scratch PATH,
a scratch HOME, cache and archive, and spare ports. The owner's install, PATH and archive are never touched.
"""

import json
import os
import shutil
import socket
import subprocess
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
JUST = shutil.which("just")  # rust-just, a lab dev dependency
ADVICE = "run `just install`, then `just up`"

SOURCES = {
    "Cargo.toml": "[package]\nname = 'knowmoretabs'\n",
    "Cargo.lock": "version = 4\n",
    "src/main.rs": "fn main() {}\n",
    "src/extra.rs": "pub fn extra() {}\n",
    "src/main_tests.rs": "#[test]\nfn works() {}\n",
    "web/index.html": "<!doctype html>\n",
    "web/NOTES.md": "notes\n",
    "web/fixtures/case.json": "{}\n",
    "README.md": "readme\n",
    "lab/tagger/README.md": "readme\n",
    "lab/tagger/pyproject.toml": "[project]\nname = 'kmt-tagger'\n",
    "lab/tagger/uv.lock": "version = 1\n",
    "lab/tagger/tagger/cli.py": "def main():\n    pass\n",
    "lab/tagger/tagger/extra.py": "EXTRA = 1\n",
    "lab/tagger/tagger/app/static/app.js": "export {};\n",
}

# Records its arguments, then holds its --port until `just down` stops it.
APP = f"""#!{sys.executable}
import os, socket, sys, time
with open(os.environ["CALLS"], "a") as calls:
    calls.write(" ".join(sys.argv) + "\\n")
with socket.socket() as listener:
    listener.bind(("127.0.0.1", int(sys.argv[sys.argv.index("--port") + 1])))
    listener.listen()
    time.sleep(60)
"""

CARGO = """#!/bin/sh
echo "cargo $*" >> "$CALLS"
case "$1" in
install)
    [ -z "${RACE:-}" ] || echo '// edited mid install' >> src/main.rs
    root="${CARGO_INSTALL_ROOT:-$CARGO_HOME}"
    while [ "$#" -gt 0 ]; do
        if [ "$1" = --root ]; then shift; root="$1"; fi
        shift
    done
    mkdir -p "$root/bin" && cp "$FAKE_APP" "$root/bin/knowmoretabs" ;;
run) while [ "$1" != -- ]; do shift; done; shift; exec "$FAKE_APP" "$@" ;;
esac
"""

UV = """#!/bin/sh
echo "uv $*" >> "$CALLS"
case "$1" in
tool)
    [ -z "${FAIL_UV:-}" ] || exit 1
    mkdir -p "$HOME/.local/bin" && cp "$FAKE_APP" "$HOME/.local/bin/tagger" ;;
run) while [ "$1" != tagger ]; do shift; done; exec "$FAKE_APP" "$@" ;;
esac
"""


def executable(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(0o755)
    return path


def spare_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class Site:
    def __init__(self, base: Path):
        self.checkout = base / "checkout"
        for name, text in SOURCES.items():
            (self.checkout / name).parent.mkdir(parents=True, exist_ok=True)
            (self.checkout / name).write_text(text)
        (self.checkout / "scripts").mkdir()
        shutil.copy(REPO / "justfile", self.checkout)
        shutil.copy(REPO / "scripts/installation.py", self.checkout / "scripts")
        shutil.copy(REPO / "lab/tagger/tagger/app/service_auth.py", self.checkout / "lab/tagger/tagger/app")
        home, tools, self.root, self.calls = base / "home", base / "tools", base / "root", base / "calls"
        executable(tools / "python3", f'#!/bin/sh\nexec "{os.path.realpath(sys.executable)}" "$@"\n')
        executable(tools / "cargo", CARGO)
        executable(tools / "uv", UV)
        self.cargo_bin, self.local_bin = home / ".cargo/bin", home / ".local/bin"
        self.receipt = base / "cache/knowmoretabs/install.json"
        (base / "tmp").mkdir()
        self.env = {
            "HOME": str(home),
            "XDG_CACHE_HOME": str(base / "cache"),
            "CARGO_HOME": str(home / ".cargo"),
            "KMT_ROOT": str(self.root),
            "TMPDIR": str(base / "tmp"),
            "CALLS": str(self.calls),
            "FAKE_APP": str(executable(base / "app", APP)),
            "PATH": os.pathsep.join(
                map(str, [tools, self.cargo_bin, self.local_bin, "/usr/bin", "/bin", "/usr/sbin", "/sbin"])
            ),
        }
        self.ports = [spare_port(), spare_port()]

    def just(self, *args: str, **env: str) -> subprocess.CompletedProcess:
        ports = [f"lib_port={self.ports[0]}", f"tagger_port={self.ports[1]}"]
        return subprocess.run(
            [JUST, *ports, *args],
            cwd=self.checkout,
            env={**self.env, **env},
            capture_output=True,
            text=True,
            timeout=120,
        )

    def install(self, **env: str) -> subprocess.CompletedProcess:
        installed = self.just("install", **env)
        self.calls.unlink(missing_ok=True)
        return installed

    def launches(self) -> list[str]:
        lines = self.calls.read_text().splitlines() if self.calls.exists() else []
        return [line for line in lines if " --port " in line and not line.startswith(("cargo ", "uv "))]

    def refused(self, result: subprocess.CompletedProcess) -> None:
        assert result.returncode != 0, result.stdout + result.stderr
        assert ADVICE in result.stderr
        assert self.launches() == []
        assert not (self.root / "logs").exists()


@pytest.fixture
def site(tmp_path):
    if JUST is None:
        pytest.fail("just is a lab dev dependency; run the tests through `uv run --locked --project lab/tagger`")
    site = Site(tmp_path)
    yield site
    site.just("down")


def test_current_install_launches_the_pair_it_names(site):
    assert site.install().returncode == 0
    up = site.just("up")
    assert up.returncode == 0, up.stdout + up.stderr
    key, kmt = site.root / "logs/session.key", f"{site.cargo_bin}/knowmoretabs"
    assert site.launches() == [
        f"{kmt} --root {site.root} serve --port {site.ports[0]} --model-port {site.ports[1]} --model-secret-file {key}",
        f"{site.local_bin}/tagger app --port {site.ports[1]} --root {site.root} --secret-file {key} --kmt-bin {kmt}",
    ]
    receipt = json.loads(site.receipt.read_text())
    assert receipt["knowmoretabs"]["path"] == kmt
    assert site.just("down").returncode == 0
    assert not key.exists()


@pytest.mark.parametrize(
    "name",
    [
        "src/main.rs",
        "web/index.html",
        "Cargo.lock",
        "lab/tagger/tagger/cli.py",
        "lab/tagger/tagger/app/static/app.js",
        "lab/tagger/uv.lock",
    ],
)
def test_changed_input_with_restored_mtime_refuses(site, name):
    assert site.install().returncode == 0
    path = site.checkout / name
    before = path.stat()
    path.write_text(path.read_text().upper())
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert path.stat().st_size == before.st_size
    site.refused(site.just("up"))


@pytest.mark.parametrize("name", ["src/new.rs", "lab/tagger/tagger/new.py", "lab/tagger/tagger/app/static/new.js"])
def test_untracked_input_refuses(site, name):
    assert site.install().returncode == 0
    (site.checkout / name).write_text("new\n")
    site.refused(site.just("up"))


@pytest.mark.parametrize("name", ["src/extra.rs", "lab/tagger/tagger/extra.py"])
def test_deleted_input_refuses(site, name):
    assert site.install().returncode == 0
    (site.checkout / name).unlink()
    site.refused(site.just("up"))


def test_docs_tests_and_caches_keep_the_install_current(site):
    assert site.install().returncode == 0
    for name in ["README.md", "web/NOTES.md", "lab/tagger/README.md", "src/main_tests.rs", "web/fixtures/case.json"]:
        (site.checkout / name).write_text("edited\n")
    (site.checkout / "lab/tagger/tagger/__pycache__").mkdir()
    (site.checkout / "lab/tagger/tagger/__pycache__/cli.cpython-312.pyc").write_bytes(b"\0")
    up = site.just("up")
    assert up.returncode == 0, up.stdout + up.stderr
    assert len(site.launches()) == 2


def test_changed_binary_refuses(site):
    assert site.install().returncode == 0
    with open(site.cargo_bin / "knowmoretabs", "a") as binary:
        binary.write("# rebuilt elsewhere\n")
    site.refused(site.just("up"))


def test_other_binary_first_on_path_refuses(site):
    assert site.install().returncode == 0
    shadow = site.checkout.parent / "shadow"
    shadow.mkdir()
    shutil.copy(site.cargo_bin / "knowmoretabs", shadow)
    site.refused(site.just("up", PATH=f"{shadow}{os.pathsep}{site.env['PATH']}"))


@pytest.mark.parametrize("custom_root", [False, True])
def test_install_with_shadowed_cargo_binary_writes_no_receipt(site, custom_root):
    assert site.install().returncode == 0
    if custom_root:
        root = site.checkout.parent / "cargo-install"
        site.env["CARGO_INSTALL_ROOT"] = str(root)
        site.cargo_bin = root / "bin"
        site.env["PATH"] = f"{site.cargo_bin}{os.pathsep}{site.env['PATH']}"
    shadow = site.checkout.parent / "shadow"
    old = executable(shadow / "knowmoretabs", APP + "# old installed payload\n")
    site.env["PATH"] = f"{shadow}{os.pathsep}{site.env['PATH']}"
    install = site.install()
    assert install.returncode != 0
    assert "PATH selects a different knowmoretabs" in install.stderr
    assert (site.cargo_bin / "knowmoretabs").read_bytes() != old.read_bytes()
    assert not site.receipt.exists()
    site.refused(site.just("up"))


def test_missing_app_refuses(site):
    assert site.install().returncode == 0
    (site.local_bin / "tagger").unlink()
    site.refused(site.just("up"))


def test_missing_receipt_refuses(site):
    assert site.install().returncode == 0
    site.receipt.unlink()
    site.refused(site.just("up"))


@pytest.mark.parametrize("text", ["", "{", "[]", '{"schema": 1}', "\xff"])
def test_malformed_receipt_refuses(site, text):
    assert site.install().returncode == 0
    site.receipt.write_text(text, encoding="latin-1")
    site.refused(site.just("up"))


def test_failed_install_forgets_the_old_receipt(site):
    assert site.install().returncode == 0
    assert site.install(FAIL_UV="1").returncode != 0
    assert not site.receipt.exists()
    site.refused(site.just("up"))


def test_sources_changed_during_install_write_no_receipt(site):
    install = site.install(RACE="1")
    assert install.returncode != 0
    assert "changed during the install" in install.stderr
    assert not site.receipt.exists()
    site.refused(site.just("up"))


def test_dev_mode_needs_no_install(site):
    up = site.just("up", "dev")
    assert up.returncode == 0, up.stdout + up.stderr
    assert ADVICE not in up.stderr
    calls = site.calls.read_text()
    assert "cargo build --locked\n" in calls
    assert "cargo run --locked -- " in calls and "uv run --locked --project lab/tagger tagger app " in calls
    assert f" --kmt-bin {site.checkout}/target/debug/knowmoretabs" in calls, "Add link and Send run the dev build"
    assert len(site.launches()) == 2


def test_the_launch_key_is_made_once_kept_while_it_exists_and_removed_by_down(site):
    assert site.install().returncode == 0
    key = site.root / "logs/session.key"
    first = site.just("up")
    assert first.returncode == 0, first.stdout + first.stderr
    made = key.read_bytes()
    assert (len(made), oct(key.stat().st_mode & 0o777)) == (65, "0o600")
    assert first.stdout.count("http://") == 1 and f"Library: http://127.0.0.1:{site.ports[0]}/" in first.stdout
    assert made.strip().decode() not in first.stdout + first.stderr + site.calls.read_text()
    assert site.just("up").returncode == 0
    assert key.read_bytes() == made, "a running pair keeps its key"
    assert site.just("down").returncode == 0
    assert not key.exists() and not key.parent.exists()
    key.parent.mkdir(parents=True)
    key.write_bytes(made)
    key.chmod(0o644)
    site.calls.unlink()
    refused = site.just("up")
    assert refused.returncode != 0 and "just down" in refused.stderr
    assert site.launches() == [], "nothing starts on an unsafe key"
