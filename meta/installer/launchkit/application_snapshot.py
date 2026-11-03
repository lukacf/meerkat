"""Freeze checked application source for an isolated, non-acceptance trial."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

from .implementation_workspace import ImplementationWorkspace
from .projects import DraftConflict, fingerprint
from .artifact_binding import current_version


def trial_build_current(status, state, build_id=None):
    return bool(status and status.get("build_id") and
        (build_id is None or status["build_id"] == build_id) and
        status.get("guide_id") == state["id"] and
        current_version(status, state) and
        status.get("status") in {"completed", "failed", "interrupted"} and
        status.get("source_verified") is True and status.get("source_digest"))


async def checked_trial_source(implementation, guide_id, state, build_id):
    if implementation is None or not build_id:
        raise ValueError("Choose a checked application build before trying its callbacks.")
    status = await implementation.status(guide_id, state=state)
    if status.get("cleanup_pending") is True:
        if status.get("cleanup_in_progress") is True or status.get("stop_requested") is True and status.get("status") in {"queued", "running", "checking"}:
            raise DraftConflict("The build runtime is closing. Follow cleanup in the builder before starting an application trial.")
        if status.get("status") in {"completed", "failed", "interrupted"}:
            raise DraftConflict("The build runtime still needs cleanup. Review cleanup in the builder before starting an application trial.")
        raise DraftConflict("The builder is still working. Finish the build and its runtime cleanup before starting an application trial.")
    if not trial_build_current(status, state, build_id):
        raise DraftConflict("The application build changed or has no current passing source checks. Finish its checks before trying it.")
    directory = implementation._directory(guide_id, build_id)
    frozen = json.loads((directory / "input.json").read_text())
    if fingerprint(frozen.get("brief", {})) != fingerprint(state.get("brief", {})):
        raise DraftConflict("The application brief changed. Build the current specification before trying its callbacks.")
    workspace = ImplementationWorkspace(directory / "workspace", frozen["blueprint"])
    files = await asyncio.to_thread(workspace.export_files, require_verified=True)
    if workspace._digest(files) != status["source_digest"]:
        raise DraftConflict("The checked source changed while preparing this application trial.")
    deployment = next((json.loads(item.content) for item in files if item.path == "deployment.json"), None)
    if not isinstance(deployment, dict):
        raise ValueError("The checked application is missing its deployment contract.")
    return {"build_id": build_id, "source_digest": status["source_digest"],
            "brief_hash": fingerprint(frozen.get("brief", {})), "deployment": deployment,
            "acceptance_status": (status.get("acceptance") or {}).get("status")}, files


def write_trial_source(directory, files):
    root = Path(directory) / "application-source"
    root.mkdir(mode=0o700)
    for item in files:
        path = root / item.path
        path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        path.write_bytes(item.as_bytes())
        # Original modes participate in the checked source digest. The private
        # parent and offline sandbox enforce isolation, not altered file modes.
        os.chmod(path, item.mode)
