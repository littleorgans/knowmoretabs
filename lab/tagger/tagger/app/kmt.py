"""How the app runs knowmoretabs, for Add link: `App.binary` (`tagger app --kmt-bin`, the binary `just up` serves
the Library with; else this name, found on PATH) on the app's own archive, and never on a snapshot copy inside the
app's data directory.
"""

BINARY = "knowmoretabs"


def snapshot(app) -> bool:
    """The app's archive lies inside its data directory (a snapshot copy), so nothing may write it. Both paths are
    resolved so an alias cannot write into a snapshot copy."""
    return app.lib.root.resolve().is_relative_to(app.lib.paths.data.resolve())


def argv(app, args: list[str]) -> list[str]:
    return [app.binary, "--root", str(app.lib.root), *args]
