"""Reopen original installed product bytes under the actual project owner.

Canonical installation receipts and private bootstrap inputs remain their
existing authorities. Retained intake Python is compared with the trusted
launcher selection and is never executed or used to select its own authority.
"""
from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager, contextmanager
from copy import deepcopy
import hashlib
import os
import re
from pathlib import Path
import stat
import uuid

from .connect_installer_caller import CustodyError, _json, _path, _read, _same_json_value
from .remote_product_artifacts import _joined


def _overlapping_parts(left, right):
    """Lexical ancestor-or-equal relation, including component boundaries."""
    return left[:len(right)] == right or right[:len(left)] == left


class _InstalledSourceView:
    """Installed reference inputs and finite original native output locations.

    This view does not admit native entry or authenticate completed native
    effects. The original Fresh/Restore owner still verifies every selected
    output and owns publication. All accepted application bytes stay closed.
    """

    def __init__(self, directory, inputs, files, *, root):
        self.directory = directory
        self.root = root
        self.inputs = deepcopy(inputs)
        self._original_inputs = deepcopy(inputs)
        self.files = frozenset(files)
        self.reference_root, self._configuration = self._reference_root(files)
        self.knowledge = self._knowledge_role(files)
        self._original_knowledge = deepcopy(self.knowledge)
        self._knowledge_configuration = False
        self._knowledge_checkout_root = None
        self.native_context = self._native_context_paths()
        if self.knowledge is not None:
            from .deployment import private_configuration_snapshot
            self._configuration = private_configuration_snapshot(self.root)[1]
            self._knowledge_configuration = private_configuration_snapshot(self.root / "knowledge")[1]
            self._knowledge_checkout_root = _knowledge_checkout(self.root / "knowledge/.source")
        self.paths = self._paths()

    def _native_context_paths(self):
        paths = (self.root / "shared/data/toolkit-native",
                 self.root / "shared/control/deny.lock", self.root / "shared/control/deny.json")
        # Derive the same lexical paths once per check. Filesystem custody is
        # checked separately; no source, mode or ownership observation is cached.
        accepted = tuple((self.root / name).parts for name in self.files)
        if any(_overlapping_parts(path.parts, source) for path in paths for source in accepted):
            raise CustodyError("Native context outputs overlap the accepted application source.")
        return paths

    def _reference_root(self, files):
        from .deployment import private_configuration_snapshot
        definition = _json(files["deployment.json"][0])
        granted = any(row.get("tools", {}).get("workspace") is True
            for row in definition.get("agents", []))
        declared = any(row.get("key") == "FILES_ROOT" and row.get("secret") is False
            and row.get("required") is False for row in definition.get("environment", []))
        if not granted or not declared or "workspace/START-HERE.txt" not in files:
            return None, False
        _, observed = private_configuration_snapshot(self.root)
        # The accepted scaffold keeps its reference-data role when Configure
        # selects another folder. External connected folders are not inventoried.
        return self.root / "workspace", observed

    def _knowledge_role(self, files):
        """Select generated data locations from this exact accepted local recipe."""
        definition = _json(files["deployment.json"][0])
        choice = definition.get("knowledge") or {}
        if choice.get("mode") != "local":
            return None
        required = {"knowledge/settings.json", "knowledge/manage.py", "knowledge/compose.json",
            "knowledge/Dockerfile", "knowledge/Dockerfile.dockerignore"}
        if not required <= files.keys():
            raise CustodyError("Local knowledge needs its original complete accepted helper source.")
        settings = _json(files["knowledge/settings.json"][0])
        compose = _json(files["knowledge/compose.json"][0])
        if (type(settings) is not dict or settings.get("mode") != "local"
                or settings.get("project") != definition.get("slug")
                or any(settings.get(key) != choice.get(key) for key in ("token_env", "space_env", "url"))
                or type(compose) is not dict or compose.get("name") != settings["project"] + "-knowledge"
                or not re.fullmatch(r"[0-9a-f]{40}", str(settings.get("source_revision", "")))
                or not re.fullmatch(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?",
                    str(settings.get("source_repository", "")))):
            raise CustodyError("Local knowledge generated data differs from its accepted recipe.")
        return {key: settings[key] for key in ("project", "token_env", "space_env",
            "source_repository", "source_revision", "provider", "model", "url")}

    def _paths(self):
        if self.inputs is None:
            return self.root, (), None, None, None
        from .deployment_application_bootstrap import bootstrap_output_paths
        root, outputs, activation = bootstrap_output_paths(self.directory, self.inputs)
        if root != self.root:
            raise CustodyError("Installed source changed its original project root.")
        if activation is None:
            return root, outputs, None, None, None
        current, next_pointer = root / "current", root / (activation.name + ".current.next")
        accepted = tuple((root / name).parts for name in self.files)
        root_parts = root.parts
        for path in (*outputs, current, next_pointer):
            parts = path.parts
            if parts == root_parts or parts[:len(root_parts)] != root_parts or any(
                    _overlapping_parts(parts, source) for source in accepted):
                raise CustodyError("Native lifecycle outputs overlap the accepted application source.")
        return root, tuple(dict.fromkeys(outputs)), activation, current, next_pointer

    def require_current(self):
        from .deployment import private_configuration_snapshot
        if (not _same_json_value(self.inputs, self._original_inputs)
                or not _same_json_value(self.knowledge, self._original_knowledge)
                or self._native_context_paths() != self.native_context
                or self._paths() != self.paths):
            raise CustodyError("Installed source changed its original selected native output paths.")
        if (self._configuration is not False
                and private_configuration_snapshot(self.root)[1] != self._configuration):
            raise CustodyError("Installed connected inputs changed their original private configuration.")
        if (self._knowledge_configuration is not False and private_configuration_snapshot(
                self.root / "knowledge")[1] != self._knowledge_configuration):
            raise CustodyError("Installed local knowledge changed its original private credentials.")
        if (self.knowledge is not None and _knowledge_checkout(self.root / "knowledge/.source")
                != self._knowledge_checkout_root):
            raise CustodyError("Installed local knowledge changed its original checkout directory.")


def replace_installed_private_configuration(source, values, expected):
    """Original atomic Configure writer with one explicit entered-view transition.

    This file-only lane never clears invalidation or renews closed custody. It
    changes no source, runtime, workspace role or native output observation.
    """
    from .deployment import (private_configuration_snapshot, replace_private_configuration,
        _serialized_private_configuration)
    from .product_runtime_custody import product_source_custody
    source = product_source_custody(source)
    if type(getattr(source, "_installed_view", None)) is not _InstalledSourceView:
        raise CustodyError("Configure requires its actual entered installed source view.")
    source.require_filesystem_current()
    view = source._installed_view
    if (view.root != source._root or view.files != frozenset(source._files)
            or view._configuration is not False and view._configuration != expected):
        raise CustodyError("Configure changed its original installed private-input observation.")
    source._lease.require_current()
    replace_private_configuration(source._root, values, expected)
    actual, observed = private_configuration_snapshot(source._root)
    if (actual != values or observed is None
            or observed[0] != _serialized_private_configuration(values).encode("utf-8")):
        raise CustodyError("The original Configure writer's private output could not be verified.")
    if view._configuration is not False:
        view._configuration = observed
    source.require_filesystem_current()
    source._lease.require_current()


def retain_installed_knowledge_settings(source, project_expected, knowledge_expected,
        project_values, knowledge_values):
    """Advance only the actual checked helper's two private-file replacements.

    The original calling task brackets this file-only completion with its owner.
    No invalidation is cleared and no closed source can be renewed.
    """
    from .deployment import private_configuration_snapshot
    from .product_runtime_custody import product_source_custody
    from .runtime_selection import selected_runtime_artifacts
    import json
    source = product_source_custody(source)
    view = getattr(source, "_installed_view", None)
    if (type(view) is not _InstalledSourceView or view.knowledge is None
            or view.root != source._root or view.files != frozenset(source._files)
            or source._closed or source._invalidated
            or view._configuration != project_expected
            or view._knowledge_configuration != knowledge_expected):
        raise CustodyError("Knowledge setup requires its original current installed private observations.")
    source._lease.require_current()
    # The helper just performed its admitted atomic writes. Recheck every
    # original source/artifact input before advancing these private observations.
    for observed in source._observed:
        observed.finish()
    selected_runtime_artifacts(source.relocated, verified_artifacts=source.artifacts)
    project, project_observed = private_configuration_snapshot(source._root)
    knowledge, knowledge_observed = private_configuration_snapshot(source._root / "knowledge")
    project_bytes = "".join(key + "=" + json.dumps(value) + "\n"
        for key, value in sorted(project_values.items())).encode("utf-8")
    knowledge_bytes = "".join(key + "='" + value + "'\n"
        for key, value in sorted(knowledge_values.items())).encode("utf-8")
    if (project != project_values or knowledge != knowledge_values
            or project_observed is None or knowledge_observed is None
            or project_observed[0] != project_bytes
            or knowledge_observed[0] != knowledge_bytes):
        raise CustodyError("The admitted knowledge helper's exact private outputs changed before retention.")
    view._configuration, view._knowledge_configuration = project_observed, knowledge_observed
    source.require_filesystem_current()
    source._lease.require_current()


def retain_installed_knowledge_checkout(source, expected):
    """Retain this original helper's completed checkout without renewing custody."""
    from .deployment import private_configuration_snapshot
    from .product_runtime_custody import product_source_custody
    from .runtime_selection import selected_runtime_artifacts
    source = product_source_custody(source)
    view = getattr(source, "_installed_view", None)
    if (type(view) is not _InstalledSourceView or view.knowledge is None
            or view.root != source._root or view.files != frozenset(source._files)
            or source._closed or source._invalidated or view._knowledge_checkout_root != expected
            or private_configuration_snapshot(source._root)[1] != view._configuration
            or private_configuration_snapshot(source._root / "knowledge")[1] != view._knowledge_configuration):
        raise CustodyError("Knowledge checkout requires its original current installed private observations.")
    source._lease.require_current()
    for observed in source._observed:
        observed.finish()
    selected_runtime_artifacts(source.relocated, verified_artifacts=source.artifacts)
    observed = _knowledge_checkout(source._root / "knowledge/.source")
    if observed is None or expected is not None and observed != expected:
        raise CustodyError("The original knowledge helper did not retain the same checkout directory.")
    view._knowledge_checkout_root = observed
    source.require_filesystem_current()
    source._lease.require_current()


def _knowledge_checkout(path):
    """Bound a helper-owned Git checkout as data, never application source."""
    if not path.exists() and not path.is_symlink():
        return None
    root = path.lstat()
    if (not stat.S_ISDIR(root.st_mode) or root.st_uid != os.getuid()
            or root.st_mode & 0o077 or path.is_symlink()):
        raise CustodyError("Local knowledge checkout lost its original private directory custody.")
    count, size = 0, 0
    for directory, children, entries in os.walk(path, followlinks=False):
        for name in (*children, *entries):
            item = (Path(directory) / name).lstat()
            count += 1
            size += item.st_size if stat.S_ISREG(item.st_mode) else 0
            # Git normally creates readable 0644 files and 0755 directories
            # below the private checkout root. They must not be writable by others.
            if (count > 50_032 or size > 4 * 1024 * 1024 * 1024
                    or item.st_uid != os.getuid() or item.st_mode & 0o022
                    or not (stat.S_ISREG(item.st_mode) or stat.S_ISDIR(item.st_mode))
                    or stat.S_ISREG(item.st_mode) and item.st_nlink != 1):
                raise CustodyError("Local knowledge checkout contains foreign, linked or unbounded data.")
    return root.st_dev, root.st_ino, root.st_mode, root.st_uid


def _private_output(path, *, activation, pointer=False):
    """Check location custody only; native owns selected output semantics."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if pointer:
        if not stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid():
            raise CustodyError("Installed source found a foreign native activation pointer.")
        target = Path(os.readlink(path))
        if _path(target if target.is_absolute() else path.parent / target) != activation:
            raise CustodyError("Installed source found a foreign native activation pointer.")
        return
    if stat.S_ISREG(info.st_mode):
        if (info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_mode & 0o077
                or info.st_size > 64 * 1024 * 1024):
            raise CustodyError("Selected native output lost its original private file custody.")
        return
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise CustodyError("Selected native output contains a linked or unsafe directory.")
    count = 0
    for directory, children, entries in os.walk(path, followlinks=False):
        for name in (*children, *entries):
            selected = Path(directory) / name
            member = selected.lstat()
            count += 1
            if count > 2048:
                raise CustodyError("Selected native output exceeds its bounded inventory.")
            if stat.S_ISDIR(member.st_mode):
                if member.st_uid != os.getuid() or member.st_mode & 0o077:
                    raise CustodyError("Selected native output contains an unsafe directory.")
            elif (not stat.S_ISREG(member.st_mode) or member.st_uid != os.getuid()
                    or member.st_nlink != 1 or member.st_mode & 0o077
                    or member.st_size > 64 * 1024 * 1024):
                raise CustodyError("Selected native output contains a linked or unsafe file.")


def _native_context_output(path, *, subtree):
    """Observe bounded native-owned output metadata without interpreting it."""
    if not os.path.lexists(path):
        return
    _path(path)
    root = path.parents[2]
    parents = []
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    parents.append(descriptor)
    pending = []
    count = 0
    try:
        for name in path.relative_to(root).parts[:-1]:
            descriptor = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            parents.append(descriptor)
            info = os.fstat(descriptor)
            if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
                raise CustodyError("Native context parents are not owned private directories.")
        if not subtree:
            member = os.stat(path.name, dir_fd=descriptor, follow_symlinks=False)
            if (not stat.S_ISREG(member.st_mode) or member.st_uid != os.getuid()
                    or member.st_nlink != 1 or member.st_mode & 0o022
                    or member.st_size > 64 * 1024 * 1024):
                raise CustodyError("Native context control output is linked or unsafe.")
            return
        pending.append((os.open(path.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
            dir_fd=descriptor), 0))
        while pending:
            selected, depth = pending.pop()
            try:
                info = os.fstat(selected)
                if (info.st_uid != os.getuid() or info.st_mode & (0o077 if depth <= 1 else 0o022)):
                    raise CustodyError("Native context state contains an unsafe directory.")
                for name in os.listdir(selected):
                    member = os.stat(name, dir_fd=selected, follow_symlinks=False)
                    count += 1
                    if count > 2048:
                        raise CustodyError("Native context state exceeds its bounded metadata inventory.")
                    if stat.S_ISDIR(member.st_mode):
                        pending.append((os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=selected), depth + 1))
                    elif (not stat.S_ISREG(member.st_mode) or member.st_uid != os.getuid()
                            or member.st_nlink != 1 or member.st_mode & 0o022
                            or member.st_size > 64 * 1024 * 1024):
                        raise CustodyError("Native context state contains a linked or unsafe file.")
            finally:
                os.close(selected)
    except OSError as error:
        raise CustodyError("Native context output is absent or unsafe during observation.") from error
    finally:
        for descriptor, _ in pending:
            os.close(descriptor)
        for descriptor in reversed(parents):
            os.close(descriptor)


def require_installed_inventory(root, files, view):
    """Keep staged defaults strict; admit only this installed owner's locations."""
    if (type(view) is not _InstalledSourceView or root != view.paths[0]
            or frozenset(files) != view.files):
        raise CustodyError("Installed source needs its original finite bootstrap output view.")
    view.require_current()
    native_state, *native_control = view.native_context
    for path in (root / "shared", root / "shared/data", root / "shared/control"):
        if os.path.lexists(path):
            _path(path)
            info = path.lstat()
            if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                    or stat.S_IMODE(info.st_mode) != 0o700):
                raise CustodyError("Native context parents are not owned private directories.")
    _native_context_output(native_state, subtree=True)
    for path in native_control:
        _native_context_output(path, subtree=False)
    _, outputs, activation, current, next_pointer = view.paths
    for path in outputs:
        _private_output(path, activation=activation)
    for path in (current, next_pointer):
        if path is None:
            continue
        _private_output(path, activation=activation, pointer=True)
    selected_roots = {path for path in (*outputs, current, next_pointer)
        if path is not None and path.parent == root}
    private_roots = {".mobkit", ".venv", ".bin", ".env", "data"}
    knowledge_env = root / "knowledge/.env" if view.knowledge is not None else None
    knowledge_source = root / "knowledge/.source" if view.knowledge is not None else None
    if knowledge_source is not None:
        _knowledge_checkout(knowledge_source)
    actual = set()
    count = 0
    for directory, children, entries in os.walk(root, followlinks=False):
        parent = Path(directory)
        children[:] = [name for name in children if parent / name != knowledge_source
            and parent / name != native_state
            and (parent != root or name not in private_roots and parent / name not in selected_roots)]
        for name in children:
            info = (parent / name).lstat()
            if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_mode & 0o022):
                raise CustodyError("Installed source contains an unsafe application directory.")
        for name in entries:
            path = parent / name
            if parent == root and (name in private_roots or path in selected_roots):
                continue
            if path == knowledge_env:
                continue
            if path in native_control:
                continue
            info = path.lstat()
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1):
                raise CustodyError("Installed source contains a linked or unsafe application file.")
            name = str(path.relative_to(root))
            count += 1
            if count > 2048:
                raise CustodyError("Installed source exceeds its original bounded inventory.")
            if (name not in files and view.reference_root in path.parents
                    and all(not part.startswith(".")
                        for part in path.relative_to(view.reference_root).parts)):
                continue
            actual.add(name)
    if actual != set(files):
        raise CustodyError("Installed source contains missing or unselected application files.")
    view.require_current()


def _trusted_owner_source(owner_source):
    if owner_source is None:
        from .remote_interface_bundle import product_destination_owner_bytes
        owner_source = product_destination_owner_bytes()
    if type(owner_source) is not bytes or not 0 < len(owner_source) <= 4 * 1024 * 1024:
        raise CustodyError("Installed source needs the trusted launcher's exact original owner bytes.")
    return owner_source


def _selected_owner_source(directory, owner_source, owner_source_sha256):
    """A trusted child literal may select historical bytes only as data."""
    if owner_source_sha256 is None:
        return _trusted_owner_source(owner_source)
    if (type(owner_source_sha256) is not str or len(owner_source_sha256) != 64
            or any(char not in "0123456789abcdef" for char in owner_source_sha256)):
        raise CustodyError("Installed source needs its independently selected trusted owner digest.")
    if owner_source is None:
        directory = _path(directory)
        receipt = _json(_read(directory / "receipt.json", 1024 * 1024))
        deployment_id = receipt.get("deployment_id")
        try:
            valid = str(uuid.UUID(deployment_id)) == deployment_id and directory.name == deployment_id
        except (ValueError, TypeError, AttributeError):
            valid = False
        if not valid:
            raise CustodyError("Installed source lost its original canonical intake identity.")
        path = Path.home() / ".local/state/meta-forge/product-sources" / deployment_id / "source-owner.py"
        owner_source = _read(_path(path), 4 * 1024 * 1024)
    owner_source = _trusted_owner_source(owner_source)
    if hashlib.sha256(owner_source).hexdigest() != owner_source_sha256:
        raise CustodyError("Retained owner data differs from the trusted launcher's original selection.")
    return owner_source


@contextmanager
def _open_files(directory, lease, callback, owner_source, application_startup, retained=None):
    from .deployment import OWNER_FILE, ownership, read_receipt, verify_destination
    from .deployment_application_bootstrap import bootstrap_installation_binding
    from .deployment_sources import read_package_source
    from .deployment_updates import checked_package
    from .product_runtime_custody import require_product_installation
    from .remote_product_destination_owner import (
        SOURCE_DIRECTORY, StoppedProductSource, _context, _intake, _result, _source_owner)
    from . import remote_product_artifact_receiver as receiver
    directory = _path(directory)
    receipt = _json(_read(directory / "receipt.json", 1024 * 1024))
    if type(receipt) is not dict or read_receipt(directory) != receipt:
        raise CustodyError("Installed source lost its real canonical installation receipt.")
    root = _path(receipt["destination"])
    lease.require_current()
    if lease.root != root:
        raise CustodyError("Installed source must use this actual project's original lease.")
    marker = root / SOURCE_DIRECTORY / "owner.json"
    intake = Path.home() / ".local/state/meta-forge/product-sources" / receipt["deployment_id"]
    marker_present, intake_present = os.path.lexists(marker), os.path.lexists(intake)
    if not marker_present and not intake_present:
        if owner_source is not None:
            raise CustodyError("The original selected destination source intake disappeared during admission.")
        yield None
        return
    if not marker_present or not intake_present:
        raise CustodyError("Installed source lost part of its original destination intake ownership.")
    selected = _json(_read(marker, 64 * 1024))
    if type(selected) is not dict or set(selected) != {
            "schema", "scope", "context", "origin", "relocated_selection_sha256"}:
        raise CustodyError("Installed source has no exact original intake marker.")
    context = _context(selected["context"], owner_source)
    if (not _same_json_value(selected, _source_owner(context))
            or context["host"]["destination"] != str(root)
            or any(receipt.get(name) != context[name] for name in
                ("deployment_id", "guide_id", "build_id", "package_sha256"))
            or directory != Path(context["host"]["home"]) / ".local/state/meta-forge/deployments"
                / context["guide_id"] / context["deployment_id"]):
        raise CustodyError("Installed source differs from its original canonical installation owner.")
    _, package = checked_package(directory, "package.zip", receipt["package_sha256"])
    provenance = read_package_source(package, receipt)
    if (provenance["source_kind"] != "application"
            or provenance["source_digest"] != context["source_digest"]
            or provenance["guide_version"] != context["guide_version"]
            or verify_destination(receipt, package) != root
            or not _same_json_value(_json(_read(root / OWNER_FILE, 64 * 1024)), ownership(receipt))):
        raise CustodyError("Installed source changed its original application package or ownership.")
    binding = bootstrap_installation_binding(receipt)
    view = _InstalledSourceView(directory, application_startup, package, root=root)
    with _intake(context, owner_source) as (store, current):
        receiver._read(store / "staged.json", receiver._json(_result(context)), keep=False)[1].finish()
        source = StoppedProductSource(context, store, owner_source, lease, callback,
            installed_view=view, file_lane=True, retained=retained)
        try:
            require_product_installation(source, receipt)
            current()
            yield source
            if bootstrap_installation_binding(read_receipt(directory)) != binding:
                raise CustodyError("Installed source changed its original retained installation binding.")
            if not source._invalidated:
                source.require_filesystem_current()
            current()
        finally:
            source.close()


@asynccontextmanager
async def open_installed_product_source(directory, *, project_lease, require_current,
        owner_source=None, owner_source_sha256=None, application_startup=None, retained=None):
    """Join file work while the original owner callback stays on its caller task.

    Only a product Start passes ``retained``: the inventory receipt.json names
    for its installed prefix, used in place of re-inflating the archive.
    """
    from .remote_product_artifacts import _current
    def current():
        project_lease.require_current()
        _current(require_current)
        project_lease.require_current()
    current()
    def relocated():
        receipt = _json(_read(_path(directory) / "receipt.json", 1024 * 1024))
        root = _path(receipt["destination"])
        from .remote_product_destination_owner import SOURCE_DIRECTORY
        return (os.path.lexists(root / SOURCE_DIRECTORY / "owner.json")
            or os.path.lexists(Path.home() / ".local/state/meta-forge/product-sources" / receipt["deployment_id"]))
    has_intake = await _joined(relocated, _owner_current=current)
    if not has_intake and (owner_source is not None or owner_source_sha256 is not None):
        raise CustodyError("The original selected destination source intake is missing.")
    expected = (await _joined(_selected_owner_source, directory, owner_source,
        owner_source_sha256, _owner_current=current)
        if has_intake else None)
    current()
    scope = _open_files(directory, project_lease, current, expected, application_startup, retained)
    entered = {}
    def enter():
        entered["source"] = scope.__enter__()
        return entered["source"]
    try:
        source = await _joined(enter, _owner_current=current)
    except BaseException:
        if "source" in entered:
            await _joined(scope.__exit__, None, None, None)
        raise
    try:
        current()
        if source is not None:
            source.require_owner_current()
        yield source
        current()
        if source is not None:
            source.require_owner_current()
    finally:
        # Closing exact descriptors is cleanup, never renewed source admission.
        # A lost original owner must not discard its already entered scope.
        await _joined(scope.__exit__, None, None, None)
