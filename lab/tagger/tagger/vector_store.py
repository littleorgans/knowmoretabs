"""Page keyed vectors, serialized writers and atomic generation publication.

Legacy positional files are read once and preserved for rollback. Only the manifest
selects a generation; unpublished arrays cannot change what a reader sees.
"""

import fcntl
import json
import os
import uuid
from contextlib import contextmanager
from pathlib import Path

import numpy as np

VERSION = 1
COUNT_KEYS = ("reused", "embedded", "stale", "dropped", "migrated", "cold_migration", "failed")


class VectorStore:
    def __init__(self, folder: Path, name: str):
        self.folder = folder / name
        self.legacy = folder / f"{name}.npy"
        self.name = name

    @property
    def exists(self) -> bool:
        return (self.folder / "manifest.json").exists() or self.legacy.exists()

    @contextmanager
    def _lock(self):
        self.folder.mkdir(parents=True, exist_ok=True)
        with (self.folder / "lock").open("a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield

    def _read(self):
        path = self.folder / "manifest.json"
        if not path.exists():
            return None
        manifest = json.loads(path.read_text())
        if manifest["version"] != VERSION:
            raise ValueError("unsupported vector manifest version")
        arrays = {}
        for key, filename in manifest["arrays"].items():
            if Path(filename).name != filename:
                raise ValueError("invalid vector array filename")
            arrays[key] = np.load(self.folder / filename, allow_pickle=False)
        self._validate(manifest["rows"], arrays, manifest["model"]["dim"])
        return manifest, arrays

    @staticmethod
    def _validate(rows, arrays, dim):
        vectors = arrays["vectors"]
        if vectors.shape != (len(rows), dim) or not np.isfinite(vectors).all():
            raise ValueError("invalid vector array shape or values")
        if len({r["id"] for r in rows}) != len(rows):
            raise ValueError("duplicate vector row id")
        if "mask" in arrays and (arrays["mask"].shape != (len(rows),) or arrays["mask"].dtype != bool):
            raise ValueError("invalid image mask")

    def read(self):
        """Load one intact generation under the same lock used by cleanup."""
        with self._lock():
            return self._read()

    def _stage(self, filename, write):
        target = self.folder / filename
        part = self.folder / f"{filename}.{uuid.uuid4().hex}.tmp"
        with part.open("xb") as stream:
            write(stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(part, target)

    def _sync_dir(self):
        fd = os.open(self.folder, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

    def _publish(self, rows, model, arrays):
        self._validate(rows, arrays, model["dim"])
        generation = uuid.uuid4().hex
        files = {key: f"{generation}-{key}.npy" for key in arrays}
        for key, filename in files.items():
            self._stage(filename, lambda stream, key=key: np.save(stream, arrays[key], allow_pickle=False))
        self._sync_dir()
        manifest = {"version": VERSION, "model": model, "arrays": files, "rows": rows}
        self._stage("manifest.json", lambda stream: stream.write(json.dumps(manifest).encode()))
        self._sync_dir()
        # This directory belongs exclusively to this store. Never remove legacy files.
        keep = {*files.values(), "manifest.json", "lock"}
        for path in self.folder.iterdir():
            if path.name not in keep and (path.suffix in (".npy", ".tmp")):
                path.unlink()
        self._sync_dir()

    def _migrate(self, rows, model, mask):
        if not self.legacy.exists():
            return None
        meta_path = self.legacy.with_suffix(".json")
        if not meta_path.exists():
            return None
        meta = json.loads(meta_path.read_text())
        # Legacy config was fixed by embed.py. Validate every setting it recorded.
        for key in ("model", "prompt", "dim", "max_tokens"):
            if key in model and meta.get(key) != model[key]:
                return None
        arrays = {"vectors": np.load(self.legacy, allow_pickle=False)}
        if mask is not None:
            path = self.legacy.with_name(f"{self.name}-mask.npy")
            if not path.exists():
                return None
            arrays["mask"] = np.load(path, allow_pickle=False)
        try:
            self._validate(rows, arrays, model["dim"])
        except ValueError:
            return None
        return {"model": model, "rows": rows}, arrays

    def reconcile(self, rows, model, embed, *, mask=None, legacy_rows=None, retain=False, allow_missing=False):
        """Return vectors in requested id order; embed only missing/stale eligible rows.

        `embed` receives requested row indices. Full startup drops absent ids; a
        partial sync retains all other ids. The legacy row provider describes the
        dataset order and is called under the lock only when migration is needed.
        Optional masked embeddings may fail; those rows stay absent and retry later.
        """
        if allow_missing and mask is None:
            raise ValueError("optional embeddings require a mask")
        if len({row["id"] for row in rows}) != len(rows):
            raise ValueError("duplicate vector row id")
        if mask is not None:
            mask = np.asarray(mask, dtype=bool)
            if mask.shape != (len(rows),):
                raise ValueError("invalid requested mask")
        counts = dict.fromkeys(COUNT_KEYS, 0)
        with self._lock():
            old = self._read()
            migrating = old is None and self.legacy.exists()
            if migrating:
                old = self._migrate(legacy_rows() if legacy_rows is not None else rows, model, mask)
                counts["migrated" if old else "cold_migration"] = len(old[0]["rows"]) if old else 1
            previous, arrays = old if old else ({"model": model, "rows": []}, {})
            same_model = previous["model"] == model
            indices = {r["id"]: i for i, r in enumerate(previous["rows"])}
            vectors = np.zeros((len(rows), model["dim"]), np.float32)
            missing = []
            for i, row in enumerate(rows):
                j = indices.get(row["id"])
                fresh = j is not None and same_model and previous["rows"][j] == row
                if fresh and mask is not None:
                    fresh = "mask" in arrays and bool(arrays["mask"][j]) == bool(mask[i])
                if fresh:
                    vectors[i] = arrays["vectors"][j]
                    counts["reused"] += 1
                else:
                    counts["stale"] += j is not None
                    if mask is None or mask[i]:
                        missing.append(i)
            failed = []
            if missing:
                try:
                    encoded = np.asarray(embed(missing), dtype=np.float32)
                    if encoded.shape != (len(missing), model["dim"]) or not np.isfinite(encoded).all():
                        raise ValueError("embedder returned invalid vectors")
                except Exception:
                    if not allow_missing:
                        raise
                    failed = missing
                    mask[failed] = False
                else:
                    vectors[missing] = encoded
            counts["embedded"] = len(missing) - len(failed)
            counts["failed"] = len(failed)
            output = {"vectors": vectors}
            if mask is not None:
                output["mask"] = mask
            requested = {r["id"] for r in rows}
            extra = [i for i, r in enumerate(previous["rows"]) if r["id"] not in requested]
            counts["dropped"] = 0 if retain and same_model else len(extra)
            published_rows = rows
            published = output
            if failed:
                keep = np.ones(len(rows), bool)
                keep[failed] = False
                published_rows = [row for i, row in enumerate(rows) if keep[i]]
                published = {key: value[keep] for key, value in output.items()}
            if retain and extra and same_model:
                published_rows = [*published_rows, *(previous["rows"][i] for i in extra)]
                published = {key: np.concatenate([value, arrays[key][extra]]) for key, value in published.items()}
            failed_cached = any(rows[i]["id"] in indices for i in failed)
            changed = migrating or old is None or previous["model"] != model or counts["dropped"] > 0 or failed_cached
            if changed or counts["reused"] + counts["failed"] != len(rows):
                self._publish(published_rows, model, published)
            return output, counts
