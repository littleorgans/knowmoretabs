"""Prototype tagger for knowmoretabs: local embeddings, per tag heads, owner tags as labels.

Privacy contract: page text is read only inside `embed`; no step prints or logs it.
Everything derived from the archive is written under the data directory, never the repo.
"""
