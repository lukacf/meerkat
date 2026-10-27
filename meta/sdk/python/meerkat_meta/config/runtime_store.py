"""Runtime-store durability declaration for Meta runtime.

The gateway's runtime store (``runtime.sqlite`` — session resume, archive,
retire) is persistent SQLite by default and needs no configuration. Since
the storage-unification arc (M4) a failed open is a **startup error**, not
a silent fall-back to an in-memory twin; the only way to run in-memory on
a persistent launch is the explicit declaration this module produces:

    Meta.builder().runtime_store(runtime_store.memory())

which serializes to the ``runtime_options.runtime_store =
{"storage": "memory"}`` wire form. Sessions then do not survive gateway
restart, and the choice is visible in the storage census
(``meta/status`` → ``storage.slots``).
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class EphemeralRuntimeStoreConfig:
    """Explicit declaration of an in-memory runtime store.

    There is deliberately no persistent variant: persistent SQLite is the
    default and only alternative, so the sole declarable choice is the
    ephemeral one.
    """

    def to_dict(self) -> dict[str, Any]:
        return {"storage": "memory"}


def memory() -> EphemeralRuntimeStoreConfig:
    """Declare the runtime store in-memory (sessions do not survive restart)."""
    return EphemeralRuntimeStoreConfig()
