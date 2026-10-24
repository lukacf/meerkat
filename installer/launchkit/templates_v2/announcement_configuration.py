"""Packaged native announcement declarations, without a collector or cursor store."""
from __future__ import annotations

import json
from pathlib import Path


DECLARATIONS_PATH = "application/announcement-declarations.json"
MAX_DECLARATIONS_BYTES = 16 * 1024 * 1024
SOURCES_CAPABILITY = "mob_announcements.sources.v1"


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Announcement declarations contain a duplicate JSON field.")
        result[key] = value
    return result


def _constant(value):
    raise ValueError("Announcement declarations require finite JSON values.")


def announcement_options(root, *, meta):
    """Capture the optional packaged sidecar once, as native inline options.

    Native owns its schema, revision/digest checks and reconciliation. Passing
    the captured value avoids a later path read changing what this start uses.
    The file contains declarations already authored for an actual native
    binding; organizational prose and guide intent are never interpreted here.
    """
    root = Path(root).resolve()
    path = root / DECLARATIONS_PATH
    if not path.exists() and not path.is_symlink():
        return {}
    if not meta:
        raise ValueError("Incoming source declarations require a selected Meta runtime.")
    if (path.parent.is_symlink() or path.is_symlink() or not path.is_file()
            or path.resolve().parent != root / "application"):
        raise ValueError("Announcement declarations must be an ordinary packaged file.")
    with path.open("rb") as stream:
        data = stream.read(MAX_DECLARATIONS_BYTES + 1)
    if len(data) > MAX_DECLARATIONS_BYTES:
        raise ValueError("Announcement declarations exceed the package file limit.")
    declarations = json.loads(data, object_pairs_hook=_object, parse_constant=_constant)
    if type(declarations) is not dict:
        raise ValueError("Announcement declarations must be a native JSON object.")
    return {"mob_announcements": {"enabled": True, "declarations": declarations}}


async def require_announcement_sources(runtime, config):
    """Check native support before starting application adapters.

    Capability and successful declaration reconciliation do not prove that a
    collector is running, a pending subscription is ready, or an item arrived.
    Applications observe those states through the native announcement methods.
    """
    options = getattr(config, "mob_announcements", None)
    if options is None or "declarations" not in options:
        return
    handle = runtime.mob_handle()
    capabilities = await handle.capabilities()
    if SOURCES_CAPABILITY not in capabilities.feature_capabilities:
        raise RuntimeError("The selected Meta runtime does not support incoming source declarations.")
    view = await handle.announcement_declarations_get()
    if view.revision != options["declarations"]["revision"]:
        raise RuntimeError("The native runtime did not apply the packaged announcement declaration revision.")
