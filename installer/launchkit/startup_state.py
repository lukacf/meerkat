"""Select the original Forge saved-state directory before importing its app.

This trusted launcher selection only preserves the directory's identity. It
does not grant access, transfer native custody or provide a protected runtime.
"""
from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import stat
import sys


@dataclass(frozen=True)
class _SavedStateSelection:
    root: Path
    device: int
    inode: int


_selected = None


def _directory(root):
    if not root.is_absolute() or root.resolve(strict=True) != root:
        raise ValueError("Select the original absolute Forge state directory without links.")
    info = root.stat(follow_symlinks=False)
    if not stat.S_ISDIR(info.st_mode):
        raise ValueError("The original Forge state directory is unavailable.")
    return info


def _environment_matches(root):
    for name in ("META_FORGE_HOME", "MOBKIT_LAUNCH_HOME"):
        value = os.environ.get(name)
        if value and Path(value).expanduser().resolve() != root:
            raise ValueError("The Forge state environment differs from its original startup selection.")


def configure_saved_state_root(root):
    """Trusted launcher only; never create, relocate or replace saved state.

    Call before either the package factory or application-host factory imports
    launchkit.server. Missing or changed originals refuse startup, including
    when the ordinary environment/default directory would be writable.
    """
    global _selected
    if "launchkit.server" in sys.modules:
        raise RuntimeError("Select original Forge state before importing the application.")
    root = Path(root)
    info = _directory(root)
    _environment_matches(root)
    candidate = _SavedStateSelection(root, info.st_dev, info.st_ino)
    if _selected is not None and _selected != candidate:
        raise ValueError("The original Forge state selection cannot be replaced.")
    _selected = candidate


def saved_state_root(default):
    """Resolve ordinary startup as before, or recheck the selected original."""
    if _selected is None:
        home = os.environ.get("META_FORGE_HOME") or os.environ.get("MOBKIT_LAUNCH_HOME", str(default))
        return Path(home).expanduser().resolve()
    info = _directory(_selected.root)
    if (info.st_dev, info.st_ino) != (_selected.device, _selected.inode):
        raise ValueError("The original Forge state directory changed before application startup.")
    _environment_matches(_selected.root)
    return _selected.root
