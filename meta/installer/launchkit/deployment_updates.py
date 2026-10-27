"""Private, source-fenced application updates under the installed project's lease."""
from __future__ import annotations

import ast
import hashlib
import io
import json
import math
import os
import re
import shutil
from pathlib import Path
import stat
import threading
import time
import tomllib
from types import SimpleNamespace
import uuid
import zipfile
from copy import deepcopy

from .deployment import (MAX_PACKAGE_BYTES, OWNER_FILE, now, ownership, package_files, read_receipt,
                         update_receipt, verify_destination, owned_destination)
from .projects import DraftConflict
from .templates.storage_setup import ProjectLockLease, project_lock, write_private
from .artifact_binding import report_brief_hash

MAX_STATE_BYTES = 512 * 1024 * 1024
MAX_STATE_FILES = 20_000
STATE_ROOTS = (".env", ".mobkit", "data", "workspace")
STATE_EXCLUSIONS = {OWNER_FILE, ".mobkit/launch.lock"}
BOUND_KEYS = ("build_id", "guide_version", "blueprint_hash", "brief_hash", "package_sha256", "unresolved")
CURRENT_RECOVERY_STATES = {"recovery-needed", "recovering-current"}
MAX_FAILED_ATTEMPTS = 8
MAX_ATTEMPT_LOG_BYTES = 32 * 1024
MAX_FAILED_ATTEMPT_BYTES = 128 * 1024
MAX_CONSOLE_CONFIG_BYTES = 64_000
MAX_NATIVE_CONFIG_BYTES = 4 * 1024 * 1024
INSTRUCTION_CONFIG_PATHS = {"blueprint.json", "deployment.json", "config/mob.toml"}
CONSOLE_PRESENTATION_DEFAULTS = {"initial_control", "initial_agent", "initial_preset"}
CANDIDATE_PATHS = frozenset({"runtime-candidate.json", "runtime_candidates.py", "install_candidate.py"})
_SDK_WHEEL_HELPER = "sdk_wheel_integrity.py"
_SDK_WHEEL_SUPERVISOR_HELPER = "launchkit/" + _SDK_WHEEL_HELPER
APPLICATION_TOOL_POLICY_PATH = "application/tool-policy.json"
# Meta 0.9.4 keeps conversations and agent identities in Meerkat Core's own
# stores. It refuses native state that an earlier release wrote until a
# one-time, offline `meta_gateway storage-migrate-native` run has converted it.
# Forge does not run that migration, so no update moves an installation across.
NATIVE_STORAGE_META_VERSION = "0.9.4"
NATIVE_STORAGE_UPDATE_BLOCKER = (
    "This update moves the application from Meta {0} to Meta {1}. Meta {1} keeps conversations "
    "and agent identities in a new storage layout and refuses saved state from an earlier release "
    "until a one-time, offline migration (`meta_gateway storage-migrate-native`) has converted it. "
    "Forge does not run that migration, so it cannot apply this update. Nothing was changed: the "
    "installation, its private settings and saved state stay as they are.")
UPDATE_BINDING_KEYS = ("update_id", "from_build_id", "to_build_id", "guide_version", "blueprint_hash",
    "source_digest", "brief_hash", "package_sha256", "previous_package_sha256", "changes", "blockers",
    "native_definition_update", "host_instruction_update", "runtime_dependency_update", "maintained_host_update")
UPDATE_INSTALLATION_KEYS = ("deployment_id", "guide_id", "destination", "execution_mode", "source_kind")
INSPECTION_BINDING_KEYS = ("deployment_id", "guide_id", "build_id", "guide_version", "blueprint_hash",
    "brief_hash", "source_digest", "package_sha256", "destination", "execution_mode", "source_kind",
    "status", "phase", "service_supervisor_pid", "service_instance_id", "service_cleanup_confirmed",
    "runtime_pid", "startup_attempt_id", "interactive_lifecycle_id", "interactive_cleanup", "update_action")


def digest(value):
    return hashlib.sha256(value).hexdigest()


def update_binding(update):
    """Detach source selection from legitimately advancing checkpoints."""
    return deepcopy({key: update.get(key) for key in UPDATE_BINDING_KEYS})


def require_update_binding(update, expected):
    if (not isinstance(update, dict) or not isinstance(expected, dict) or set(expected) != set(UPDATE_BINDING_KEYS)
            or not _same_toml(update_binding(update), expected)):
        raise DraftConflict("The originally admitted update selection changed. No replacement update was admitted.")


def require_update_installation(receipt, expected):
    if (not isinstance(expected, dict) or set(expected) != set(UPDATE_INSTALLATION_KEYS)
            or not _same_toml({key: receipt.get(key) for key in UPDATE_INSTALLATION_KEYS}, expected)):
        raise DraftConflict("The originally admitted installation changed. No replacement destination was admitted.")


def retain_update_admission(directory, update, mode, *, application_startup=None):
    """Retain the existing owner's exact source selection for one worker launch."""
    return _retain_update_admission(directory, update, mode,
        application_startup=application_startup)[0]


def retain_replacement_update_admission(directory, update, mode="update", *, application_startup):
    """Return the private owner admission and its original handoff observation.

    Run through the existing joined file lane. The observation is kept on the
    owner loop until queueing; it is never a browser field or native authority.
    """
    if application_startup is None:
        raise DraftConflict("The application owner must supply its actual replacement inputs.")
    return _retain_update_admission(directory, update, mode,
        application_startup=application_startup)


def _retain_update_admission(directory, update, mode, *, application_startup):
    if mode not in {"update", "rollback", "recover-current", "start", "install"}:
        raise DraftConflict("This update admission has no selected worker mode.")
    binding = update_binding(update)
    parent_files = parent_directories = ()
    if application_startup is not None:
        from .connect_installer_caller import ExactFile, _private_directory, _read
        from .deployment_application_startup import _file_identity
        root = update_directory(directory, update["update_id"])
        parent_directories = tuple((path, _private_directory(path)) for path in (Path(directory), root))
        parent_files = tuple((path, _file_identity(path)) for path in
            (Path(directory) / "receipt.json", root / "update.json", Path(directory) / "package.zip",
             root / "candidate.zip", root / "previous.zip"))
        receipt, root, current = inspect_transaction(directory, update["update_id"], expected_binding=binding)
    else:
        receipt, root, current = transaction(directory, update["update_id"], expected_binding=binding)
    recovery = current["status"] if mode in {"start", "install"} else None
    if mode in {"start", "install"} and recovery not in {"restored-stopped", "candidate-restored-stopped"}:
        raise DraftConflict("Recovery startup requires the original restored source transaction.")
    selected = {"mode": mode, "binding": binding, "recovery_status": recovery,
        "installation": {key: receipt.get(key) for key in UPDATE_INSTALLATION_KEYS}}
    observation = None
    if application_startup is not None:
        if mode != "update":
            raise DraftConflict("A replacement requires its original selected update, not a restart or rollback replay.")
        from .deployment_application_startup import selected_inputs, ATTEMPT_NAME
        original_directory = dict(parent_directories)[root]
        if (root / ATTEMPT_NAME).exists() or (root / ATTEMPT_NAME).is_symlink():
            raise DraftConflict("This replacement already has a retained attempt. Reconcile it before selecting a successor.")
        selected["application_startup"], observation = selected_inputs(application_startup,
            directory=directory, binding=binding, installation=selected["installation"],
            retain_observation=True)
    require_recovery_source(directory, selected)
    if observation is not None:
        # Parent admission custody ends at queue publication. These mutable
        # receipt/package observations are not added to the child transaction.
        # Read and hash under the original metadata values on this file lane.
        parent_pins = tuple((ExactFile(path, digest(_read(path, MAX_PACKAGE_BYTES))), identity)
            for path, identity in parent_files)
        from dataclasses import replace
        observation = replace(observation,
            files=(*observation.files, *parent_pins),
            directories=(*observation.directories, *parent_directories))
        observation.require_current()
    content = json.dumps(selected,
        ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()
    path = root / ("admission-" + str(uuid.uuid4()) + ".json")
    write_archive(path, content)
    admission = {"path": str(path.absolute()), "sha256": digest(content)}
    if observation is not None:
        # Retain the original envelope as well as its selected inputs across
        # the thread-return boundary. No hashing runs on the owner loop.
        original_file = _file_identity(path)
        read_update_admission(directory, mode, admission)
        if _file_identity(path) != original_file or _private_directory(root) != original_directory:
            raise DraftConflict("The replacement admission changed during its original observation.")
        observation = replace(observation,
            files=(*observation.files, (ExactFile(path, admission["sha256"]), original_file)),
            directories=(*observation.directories, (root, original_directory)))
        observation.require_current()
    return admission, observation


def read_update_admission(directory, mode, selection):
    """Read only the original pin passed across the existing worker boundary."""
    if (not isinstance(selection, dict) or set(selection) != {"path", "sha256"}
            or not isinstance(selection["path"], str) or not isinstance(selection["sha256"], str)
            or len(selection["sha256"]) != 64
            or any(char not in "0123456789abcdef" for char in selection["sha256"])):
        raise DraftConflict("The worker has no original retained update admission.")
    path = Path(selection["path"])
    if not path.is_absolute() or any(parent.is_symlink() for parent in (path, *path.parents)):
        raise DraftConflict("The original update admission path changed.")
    try:
        with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as source:
            info, parent = os.fstat(source.fileno()), path.parent.stat()
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                    or info.st_mode & 0o077 or info.st_size > 64 * 1024 * 1024
                    or not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.getuid() or parent.st_mode & 0o077):
                raise DraftConflict("The original update admission is not private owned source.")
            content = source.read(64 * 1024 * 1024 + 1)
            after = os.fstat(source.fileno())
            file_pin = lambda item: (item.st_dev, item.st_ino, item.st_mode, item.st_uid,
                item.st_nlink, item.st_size, item.st_mtime_ns, item.st_ctime_ns)
            parent_pin = lambda item: (item.st_dev, item.st_ino, item.st_mode, item.st_uid)
            if (len(content) > 64 * 1024 * 1024 or file_pin(after) != file_pin(info)
                    or file_pin(path.stat()) != file_pin(info)
                    or parent_pin(path.parent.stat()) != parent_pin(parent)
                    or any(parent.is_symlink() for parent in (path, *path.parents))):
                raise DraftConflict("The original retained update admission changed while reading.")
    except OSError:
        raise DraftConflict("The original retained update admission is unavailable.") from None
    if digest(content) != selection["sha256"]:
        raise DraftConflict("The original retained update admission changed.")
    try:
        value = json.loads(content)
    except (ValueError, UnicodeError):
        raise DraftConflict("The original retained update admission is invalid.") from None
    required = {"mode", "binding", "recovery_status", "installation"}
    if (not isinstance(value, dict) or set(value) not in (required, required | {"application_startup"})
            or value["mode"] != mode or not isinstance(value["binding"], dict)
            or set(value["binding"]) != set(UPDATE_BINDING_KEYS)
            or not isinstance(value["installation"], dict)
            or set(value["installation"]) != set(UPDATE_INSTALLATION_KEYS)
            or mode not in {"update", "rollback", "recover-current", "start", "install"}
            or (not isinstance(value["recovery_status"], str)
                or value["recovery_status"] not in {"restored-stopped", "candidate-restored-stopped"}
                if mode in {"start", "install"} else value["recovery_status"] is not None)):
        raise DraftConflict("The original retained update admission has a different selection.")
    if "application_startup" in value:
        from .deployment_application_startup import SELECTION_KEYS
        startup = value["application_startup"]
        if (mode != "update" or not isinstance(startup, dict) or set(startup) != SELECTION_KEYS
                or startup.get("kind") != "replacement"):
            raise DraftConflict("The retained replacement startup has a different original selection.")
    root = update_directory(directory, value["binding"]["update_id"])
    if path.parent != root.absolute() or not path.name.startswith("admission-"):
        raise DraftConflict("The retained admission belongs to a different update journal.")
    receipt = read_receipt(directory)
    require_update_installation(receipt, value["installation"])
    return value


def require_recovery_source(directory, selected):
    recovery = selected["recovery_status"]
    if recovery is None:
        return
    binding = selected["binding"]
    previous = recovery == "restored-stopped"
    receipt = read_receipt(directory)
    expected = (binding["from_build_id"], binding["previous_package_sha256"]) if previous else (
        binding["to_build_id"], binding["package_sha256"])
    if (receipt["build_id"], receipt["package_sha256"]) != expected:
        raise DraftConflict("The recovery source no longer matches the originally admitted restart.")
    _, source = checked_package(directory, "package.zip", expected[1])
    verify_destination(receipt, source)


def checked_package(directory, name, expected):
    path = Path(directory) / name
    if (path.is_symlink() or not path.is_file() or path.stat().st_size > 64 * 1024 * 1024
            or path.stat().st_uid != os.getuid() or path.stat().st_nlink != 1):
        raise DraftConflict("The saved update package is not an owned ordinary file.")
    payload = path.read_bytes()
    if digest(payload) != expected:
        raise DraftConflict("The saved update package changed. No source was replaced.")
    return payload, package_files(payload)


def _require_unchanged_application_policy(previous, candidate):
    if previous.get(APPLICATION_TOOL_POLICY_PATH) != candidate.get(APPLICATION_TOOL_POLICY_PATH):
        raise DraftConflict("This update changes the agents' tool access and needs a verified permission-update path. Installed source and private data stay intact.")


def require_application_policy_continuity(directory, update):
    """Authenticate policy continuity independently of diagnostic checkpoints."""
    root = update_directory(directory, update["update_id"])
    _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    _, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _require_unchanged_application_policy(previous, candidate)
    return previous, candidate


def require_no_replacement_replay(root):
    """Do not let ordinary source recovery reset an entered native selection."""
    for name in ("application-replacement-attempt.json", "application-replacement-entry.json"):
        path = root / name
        if path.exists() or path.is_symlink():
            raise DraftConflict("This replacement has a retained native attempt. Preserve its source and policy pins; exact native reconciliation is required before restart or rollback.")


def require_replacement_history_settled(directory):
    """Keep an unresolved policy owner visible after a newer update is staged."""
    root = Path(directory) / "updates"
    if not root.exists():
        return
    if root.is_symlink():
        raise DraftConflict("The retained replacement history contains a link.")
    # Existing update roots are bounded private UUID journals. Do not silently
    # skip a retained owner by consulting only the current receipt pointer.
    for index, child in enumerate(root.iterdir()):
        if index >= 2048:
            raise DraftConflict("The retained update history exceeds its bounded inspection. Preserve it for native reconciliation.")
        if child.is_symlink():
            raise DraftConflict("The retained replacement history contains a linked update.")
        if child.is_dir():
            require_no_replacement_replay(child)


def _require_project_lease(receipt, project_lease):
    if (not isinstance(project_lease, ProjectLockLease)
            or project_lease.root != Path(receipt["destination"]).absolute()):
        raise DraftConflict("Source publication requires this installed project's original project lease.")
    project_lease.require_current()


def update_directory(directory, update_id):
    try:
        if str(uuid.UUID(update_id)) != update_id:
            raise ValueError
    except (ValueError, TypeError, AttributeError):
        raise DraftConflict("Choose the exact staged update.") from None
    path = Path(directory) / "updates" / update_id
    if any(parent.is_symlink() for parent in [path, *path.parents]):
        raise DraftConflict("The private update journal contains a link.")
    return path


def _ordinary_toml(value):
    if isinstance(value, dict):
        return all(_ordinary_toml(key) and _ordinary_toml(item) for key, item in value.items())
    if isinstance(value, list):
        return all(_ordinary_toml(item) for item in value)
    if isinstance(value, str):
        value.encode("utf-8")
    return not isinstance(value, float) or math.isfinite(value)


def _same_toml(left, right):
    # Python considers bool/int/float values equal in some cases. Their TOML
    # types are different and must not mask a nonpresentation setting change.
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(_same_toml(value, right[key]) for key, value in left.items())
    if isinstance(left, list):
        return len(left) == len(right) and all(_same_toml(a, b) for a, b in zip(left, right))
    return left == right


def _console_presentation_only(previous, candidate):
    """Permit three UI defaults, not changes to Console authority or topology."""
    if (not isinstance(previous, tuple) or not isinstance(candidate, tuple)
            or len(previous) != 2 or len(candidate) != 2
            or type(previous[1]) is not int or type(candidate[1]) is not int
            or previous[1] != candidate[1]):
        return False
    parsed = []
    try:
        for content, _ in (previous, candidate):
            if not isinstance(content, bytes) or len(content) > MAX_CONSOLE_CONFIG_BYTES:
                return False
            value = tomllib.loads(content.decode("utf-8"))
            if not _ordinary_toml(value):
                return False
            layout = value.get("layout", {})
            if not isinstance(layout, dict):
                return False
            for key in CONSOLE_PRESENTATION_DEFAULTS:
                if key in layout:
                    if not isinstance(layout[key], str):
                        return False
                    del layout[key]
            if "layout" in value and not layout:
                del value["layout"]
            parsed.append(value)
        return _same_toml(*parsed)
    except (ValueError, TypeError, UnicodeError, RecursionError):
        return False


def _parsed_config(files, name):
    entry = files.get(name)
    if (not isinstance(entry, tuple) or len(entry) != 2 or not isinstance(entry[0], bytes)
            or type(entry[1]) is not int or entry[1] != 0o644 or len(entry[0]) > MAX_NATIVE_CONFIG_BYTES):
        raise ValueError("Native configuration must remain an ordinary bounded package file.")
    text = entry[0].decode("utf-8")
    value = tomllib.loads(text) if name.endswith(".toml") else json.loads(text)
    if not isinstance(value, dict) or not _ordinary_toml(value):
        raise ValueError("Native configuration must contain ordinary values.")
    return value


def instruction_configuration_update(previous, candidate):
    """Permit role text and existing flow messages, with native authority exact.

    Both packages must contain the compiler's canonical projections of their
    accepted Blueprint. Stripping only the authored text then compares every
    other field and type, including identity, grants, model and flow structure.
    """
    if not any(previous.get(name) != candidate.get(name) for name in INSTRUCTION_CONFIG_PATHS):
        return False
    try:
        from .blueprint import Blueprint
        from .compiler import _runtime, mob_toml
        if _same_toml(_parsed_config(previous, "config/mob.toml"), _parsed_config(candidate, "config/mob.toml")):
            # Formatting is not an authored native definition transition.
            # Native CAS correctly rejects an already matching definition.
            return False
        projections = []
        for files in (previous, candidate):
            blueprint = _parsed_config(files, "blueprint.json")
            deployment = _parsed_config(files, "deployment.json")
            native = _parsed_config(files, "config/mob.toml")
            spec = Blueprint.model_validate(blueprint)
            if (not _same_toml(blueprint, spec.model_dump(mode="json"))
                    or not _same_toml(deployment, _runtime(spec))
                    or not _same_toml(native, tomllib.loads(mob_toml(spec)))):
                return False
            for item in blueprint["agent_classes"]:
                del item["instructions"]
                del native["skills"][item["id"] + "_role"]["content"]
            for flow in blueprint["flows"]:
                for step in flow["steps"]:
                    del step["prompt"]
                    del native["flows"][flow["id"]]["steps"][step["id"]]["message"]
            for item in deployment["classes"]:
                del item["instructions"]
            projections.append((blueprint, deployment, native))
        return all(_same_toml(a, b) for a, b in zip(*projections))
    except (KeyError, ValueError, TypeError, UnicodeError, RecursionError):
        return False


def _framework_provenance(files):
    """Read source provenance already bound to the accepted package journal."""
    try:
        report = json.loads(files["implementation-report.json"][0])
        blueprint = _parsed_config(files, "blueprint.json")
        provenance = report["framework"]
        encoded = json.dumps(blueprint, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()
        return (isinstance(provenance, dict) and provenance.get("kind") == "host-framework-provenance"
            and provenance.get("version") == 1 and provenance.get("blueprint_digest") == digest(encoded)
            and isinstance(provenance.get("baseline_digest"), str) and len(provenance["baseline_digest"]) == 64)
    except (KeyError, TypeError, ValueError, UnicodeError, RecursionError):
        return False


def _candidate_source_selection(files, *, source_custody=None):
    """Read archived dependency identity without accessing its artifact locators."""
    from .runtime_selection import (parse_candidate_selection, selected_runtime_marker,
        selected_runtime_version, selected_runtime_platform)
    deployment = _parsed_config(files, "deployment.json")
    if "runtimeCandidate" not in deployment:
        if CANDIDATE_PATHS & files.keys():
            raise ValueError("Candidate dependency source needs its exact selection.")
        return None
    selection = _parsed_config(files, "runtime-candidate.json")
    target = parse_candidate_selection(selection)
    expected = selected_runtime_marker(target)
    deployment_target = deployment.get("target")
    if deployment_target == "gcp" and source_custody is not None:
        from .product_runtime_custody import gcp_product_deployment_target
        deployment_target = gcp_product_deployment_target(source_custody, target, files=files)
    suffix = {"mac": "apple-darwin", "linux": "unknown-linux-gnu"}.get(deployment_target)
    if (not _same_toml(deployment["runtimeCandidate"], expected)
            or deployment.get("mobkitVersion") != selected_runtime_version(target)
            or suffix is None or not selected_runtime_platform(target).endswith("-" + suffix)
            or not CANDIDATE_PATHS <= files.keys()):
        raise ValueError("The archived deployment does not select this exact local candidate.")
    return selection


def _accepted_ordinary_route(files, blueprint, runtime_target):
    """compiler.accepted_ordinary_route, read from these files' deployment."""
    from .compiler import accepted_ordinary_route
    return accepted_ordinary_route(blueprint, _parsed_config(files, "deployment.json"), runtime_target)


def current_host_framework(files, *, runtime_target=None, verified_artifacts=None, reads=None):
    """Require the exact current compiler export and its protected provenance."""
    if not _framework_provenance(files):
        return False
    try:
        from .compiler import generate_package
        report = json.loads(files["implementation-report.json"][0])
        provenance = report["framework"]
        current_basis = (provenance.get("basis") == "current-compiler" or
                         provenance.get("basis") == "accepted-baseline" and
                         provenance.get("reason") == "unchanged-specification")
        if not current_basis:
            return False
        selection = _candidate_source_selection(files)
        if selection is not None:
            from .runtime_selection import candidate_selection
            if runtime_target is None:
                return False
            if reads is not None and verified_artifacts is None:
                verified_artifacts = reads(runtime_target)
            if not _same_toml(selection, candidate_selection(runtime_target, verified_artifacts=verified_artifacts)):
                return False
        elif runtime_target is not None:
            return False
        blueprint = _parsed_config(files, "blueprint.json")
        generated = (generate_package(blueprint, runtime_target=runtime_target,
                        verified_artifacts=verified_artifacts,
                        legacy_ordinary=_accepted_ordinary_route(files, blueprint, runtime_target))
                     if runtime_target is not None else generate_package(blueprint))
        host = next(item for item in generated if item.path == "app.py")
        continuity = next(item for item in generated if item.path == "native_definition_continuity.py")
        baseline = {item.path: {"sha256": digest(item.content.encode()), "mode": item.mode} for item in generated}
        fingerprint = digest(json.dumps(baseline, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
        from .implementation_workspace import PROTECTED_APPLICATION_PATHS
        protected = provenance.get("protected_application_paths", [])
        if (not isinstance(protected, list) or any(not isinstance(name, str) for name in protected)
                or protected != sorted(set(protected)) or not set(protected) <= PROTECTED_APPLICATION_PATHS
                or any(name in baseline or name not in files for name in protected)):
            return False
        for name in protected:
            content, mode = files[name]
            if not isinstance(content, bytes) or mode not in (0o600, 0o644):
                return False
            content.decode("utf-8")
            if b"\0" in content:
                return False
            baseline[name] = {"sha256": digest(content), "mode": mode}
        if protected:
            manifest = "".join(f"{entry['sha256']}  {name}\n" for name, entry in sorted(baseline.items())
                if name != "SHA256SUMS")
            baseline["SHA256SUMS"]["sha256"] = digest(manifest.encode())
        full_fingerprint = digest(json.dumps(baseline, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
        if ((selection is not None and not _same_toml(provenance.get("runtime_candidate"), selection))
                or selection is None and "runtime_candidate" in provenance):
            return False
        return (files.get("app.py") == (host.content.encode(), host.mode)
                and files.get("native_definition_continuity.py") == (continuity.content.encode(), continuity.mode)
                and current_basis
                and provenance.get("baseline_digest") == full_fingerprint
                and provenance.get("current_compiler_baseline_digest") == fingerprint
                and provenance.get("retained_template_paths") == [])
    except (OSError, KeyError, TypeError, ValueError, UnicodeError, RecursionError, StopIteration):
        return False


def has_host_instruction_carrier(files):
    """Recognize the protected current host, never an authored application claim."""
    from .compiler import TEMPLATES_V2
    return (_framework_provenance(files)
            and files.get("app.py") == ((TEMPLATES_V2 / "app.py").read_bytes(), 0o644)
            and files.get("native_definition_continuity.py") == (
                Path(__file__).with_name("native_definition_continuity.py").read_bytes(), 0o644))


def _reader_uses_sdk_wheel_helper(content):
    """Inspect the retained reader's imports without loading archived code."""
    try:
        tree = ast.parse(content)
    except (SyntaxError, UnicodeError) as error:
        raise ValueError("The retained candidate reader is not valid source.") from error
    return any(isinstance(node, ast.ImportFrom) and node.module == "sdk_wheel_integrity"
               or isinstance(node, ast.Import) and any(
                   alias.name == "sdk_wheel_integrity" for alias in node.names)
               for node in ast.walk(tree))


def _candidate_reader_helper_paths(candidate):
    dependent = _reader_uses_sdk_wheel_helper(candidate["runtime_candidates.py"][0])
    if _SDK_WHEEL_HELPER in candidate:
        return {_SDK_WHEEL_HELPER}
    if dependent:
        raise ValueError("The archived candidate reader lost its SDK wheel dependency.")
    return set()


def _maintained_supervisor_paths(*, sources=None):
    from .compiler import MAINTAINED_HOST_PATHS
    names = ({"launchkit/" + name for name in ("compiler.py", "deployment_updates.py",
        "deployment_service.py", "deployment_worker.py", "deployment_supervisor.py", "runtime_selection.py",
        "runtime_dependencies.py", "runtime_candidates.py", "templates/install_candidate.py")} |
        {"launchkit/templates_v2/" + name for name in MAINTAINED_HOST_PATHS})
    if sources is None or _SDK_WHEEL_SUPERVISOR_HELPER in sources:
        names.add(_SDK_WHEEL_SUPERVISOR_HELPER)
    return sorted(names)


def maintained_host_transition(previous, candidate, receipt, *, runtime_target=None, archived=None, reads=None):
    """Bind seven current host files to the identical already admitted candidate.

    The historical compiler inventory is deliberately bounded. A newer host
    inventory or optional Connect selection needs its own qualified crossing.
    Closed history checks its archived hashes, without admitting a runtime.
    """
    try:
        from .compiler import MAINTAINED_HOST_PATHS, maintained_host_export
        names = set(MAINTAINED_HOST_PATHS)
        if (receipt.get("target", "local") != "local"
                or receipt.get("execution_mode", "interactive") not in {"interactive", "background"}
                or not any(previous.get(name) != candidate.get(name) for name in names)):
            return None
        selection = _candidate_source_selection(previous)
        if (selection is None or not _same_toml(selection, _candidate_source_selection(candidate))
                or not _same_toml(selection, receipt.get("runtime_candidate"))):
            return None
        old_report = _parsed_config(previous, "implementation-report.json")
        report = _parsed_config(candidate, "implementation-report.json")
        framework = report["framework"]
        retained = old_report["framework"].get("retained_template_paths")
        if type(retained) is not list or not all(type(name) is str and len(name) <= 2000
                and "\\" not in name and not any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in name)
                and all(part not in {"", ".", ".."} for part in name.split("/")) for name in retained):
            return None
        old_names = _archived_framework_paths(previous)
        previous_valid = _archived_framework_provenance(previous, old_names)
        if not previous_valid and names <= previous.keys():
            # A completed crossing adds this same fixed Console host inventory.
            # Authenticate its archived baseline rather than expanding the
            # generic historical compiler contract from observed file names.
            maintained_names = sorted(set(old_names) | names)
            # Compiler comparison can list optional templates absent from both
            # archives. These annotations add no files to the fixed inventory.
            absent = frozenset(name for name in retained
                if name not in previous and name not in candidate)
            previous_valid = _archived_framework_provenance(previous, maintained_names,
                absent_template_paths=absent)
            if previous_valid:
                old_names = maintained_names
        binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
        if (not previous_valid or not _framework_provenance(candidate)
                or framework.get("basis") != "runtime-refresh" or framework.get("reason") != "explicit-runtime-refresh"
                or not _same_toml(framework.get("previous"), binding) or not _same_toml(framework.get("origin"), binding)
                or not _same_toml(framework.get("runtime_candidate"), selection)
                or report["build_id"] == old_report["build_id"]
                or not all(_sha256(value) for value in (binding["source_digest"], report["verification"]["digest"],
                    framework.get("current_compiler_baseline_digest")))
                or report["verification"]["digest"] == binding["source_digest"]
                or any(not _same_toml(old_report.get(key), report.get(key))
                       for key in ("guide_version", "blueprint_hash", "brief_hash", "unresolved"))):
            return None
        allowed = names | {"SHA256SUMS", "implementation-report.json", "acceptance-report.json"}
        if any(previous.get(name) != candidate.get(name) for name in previous.keys() | candidate.keys()
               if name not in allowed and not name.startswith("acceptance-tests/")):
            return None
        framework_names = sorted(set(old_names) | names)
        inventory = {name: {"sha256": digest(candidate[name][0]),
            "mode": 0o600 if name == ".env.example" else candidate[name][1]} for name in framework_names}
        if any(candidate[name][1] != (0o755 if name.endswith(".sh") else 0o644) for name in framework_names):
            return None
        sums = "".join(f"{inventory[name]['sha256']}  {name}\n" for name in framework_names).encode()
        inventory["SHA256SUMS"] = {"sha256": digest(sums), "mode": 0o644}
        if framework.get("baseline_digest") != digest(json.dumps(inventory,
                ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()):
            return None
        source_paths = _maintained_supervisor_paths(sources=(
            archived.get("required_supervisor_sources") if archived is not None else None))
        if archived is None:
            from .runtime_selection import candidate_selection
            if runtime_target is None or not _same_toml(selection, candidate_selection(runtime_target,
                    verified_artifacts=None if reads is None else reads(runtime_target))):
                return None
            expected = {item.path: (item.as_bytes(), item.mode) for item in maintained_host_export()}
            sources = {name: digest(Path(__file__).parent.joinpath(*Path(name).parts[1:]).read_bytes())
                       for name in source_paths}
            if any(candidate.get(name) != value for name, value in expected.items()):
                return None
        else:
            sources = archived["required_supervisor_sources"]
            if (type(sources) is not dict or set(sources) != set(source_paths)
                    or not all(_sha256(value) for value in sources.values())):
                return None
            helper_paths = _candidate_reader_helper_paths(candidate)
            if _SDK_WHEEL_SUPERVISOR_HELPER in sources:
                if helper_paths and sources[_SDK_WHEEL_SUPERVISOR_HELPER] != digest(candidate[_SDK_WHEEL_HELPER][0]):
                    return None
            elif helper_paths:
                return None
            # A maintained crossing retains the accepted package reader while
            # its supervisor has a separately pinned source closure. Closed
            # history does not require these original owners to share bytes.
        refreshed = {name: {"sha256": digest(candidate[name][0]), "mode": candidate[name][1]}
                     for name in sorted(names)}
        if any(sources["launchkit/templates_v2/" + name] != refreshed[name]["sha256"] for name in names):
            return None
        return {"kind": "maintained-console-host-update", "version": 1, "runtime_candidate": selection,
            "previous": binding, "candidate": {"build_id": report["build_id"],
                "source_digest": report["verification"]["digest"]},
            "protected_paths": sorted(names), "refreshed_files": refreshed,
            "accepted_baseline_digest": framework["baseline_digest"],
            "compiler_baseline_digest": framework["current_compiler_baseline_digest"],
            "required_supervisor_sources": sources}
    except (OSError, KeyError, ValueError, TypeError, UnicodeError, RecursionError):
        return None


def _require_maintained_host_configuration(receipt, update):
    if update.get("maintained_host_update") is not None:
        from .deployment import private_configuration_snapshot
        values, _ = private_configuration_snapshot(owned_destination(receipt["destination"]))
        if values.get("MOBKIT_CONNECT_BIN"):
            raise DraftConflict("This base Console host refresh does not migrate an already selected Connect frontend. No source or private settings were changed.")


def _completed_maintained_host_transition(previous, candidate, receipt, update, root):
    """Keep closed source history independent of today's compiler and bundle."""
    try:
        descriptor = maintained_host_transition(previous, candidate, receipt, archived=update["maintained_host_update"])
        old = _parsed_config(previous, "implementation-report.json")
        report = _parsed_config(candidate, "implementation-report.json")
        previous_bound = {key: report_brief_hash(old) if key == "brief_hash" else
            update["previous_package_sha256"] if key == "package_sha256" else old[key] for key in BOUND_KEYS}
        bound = {key: report_brief_hash(report) if key == "brief_hash" else
            update["package_sha256"] if key == "package_sha256" else report[key] for key in BOUND_KEYS}
        if (descriptor is None or update.get("status") != "completed" or update.get("blockers") != []
                or update.get("runtime_dependency_update") is not None
                or update.get("native_definition_update") is not False or update.get("host_instruction_update") is not False
                or not _same_toml(json.loads((root / "previous-receipt.json").read_text()), previous_bound)
                or not _same_toml({key: receipt.get(key) for key in BOUND_KEYS}, bound)
                or not _same_toml([update["from_build_id"], update["previous_source_digest"], update["to_build_id"],
                    update["source_digest"], update["guide_version"], update["blueprint_hash"], update["brief_hash"]],
                    [old["build_id"], old["verification"]["digest"], report["build_id"], report["verification"]["digest"],
                     report["guide_version"], report["blueprint_hash"], report_brief_hash(report)])):
            return None
        return descriptor
    except (OSError, KeyError, ValueError, TypeError, UnicodeError, RecursionError):
        return None


def meta_release(version):
    """Return the numeric release of an exact version string, or None."""
    match = (re.match(r"([0-9]{1,6})\.([0-9]{1,6})\.([0-9]{1,6})(?![0-9])", version)
             if type(version) is str else None)
    return tuple(int(part) for part in match.groups()) if match else None


def crosses_native_storage(from_version, to_version):
    """Whether native state written by one release needs the offline
    native-storage migration before the other release can open it."""
    before, after = meta_release(from_version), meta_release(to_version)
    return (before is not None and after is not None
            and before < meta_release(NATIVE_STORAGE_META_VERSION) <= after)


def native_storage_crossing(previous, candidate):
    """Return the (from, to) releases of a source change that would move the
    installed native state across NATIVE_STORAGE_META_VERSION, else None."""
    try:
        versions = tuple(_parsed_config(files, "deployment.json").get("mobkitVersion")
                         for files in (previous, candidate))
    except (KeyError, ValueError, TypeError, UnicodeError, RecursionError):
        return None
    return versions if crosses_native_storage(*versions) else None


def _runtime_dependency_authority(previous, candidate, receipt):
    """Compare the fixed release crossing independently of compiler lifetime."""
    if receipt.get("target", "local") != "local" or not _framework_provenance(previous):
        return None
    mode = receipt.get("execution_mode", "interactive")
    if mode not in {"interactive", "background"} or mode == "interactive" and receipt.get("service"):
        return None
    if mode == "background":
        from .deployment_service import require_update_protocol
        try:
            require_update_protocol(receipt.get("service") or {})
        except (DraftConflict, KeyError, TypeError, ValueError):
            return None
    try:
        from .native_models import is_legacy_registry_removal
        old = _parsed_config(previous, "deployment.json")
        new = _parsed_config(candidate, "deployment.json")
        versions = old.get("mobkitVersion"), new.get("mobkitVersion")
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
        selected = _candidate_source_selection(candidate)
        if selected is not None and type(parse_candidate_selection(selected)) is MetaToolkitRuntimeTarget:
            if mode != "interactive":
                return None
            prior = _candidate_source_selection(previous)
            if prior is not None and type(parse_candidate_selection(prior)) is MetaToolkitRuntimeTarget:
                authority = _product_refresh_authority(previous, candidate, receipt, old, new, prior, selected)
            else:
                authority = _meta_toolkit_dependency_authority(previous, candidate, old, new, selected, versions)
            # Recovery is forward-only once the new runtime may start, and that
            # runtime refuses unmigrated state: never authorize this crossing.
            if authority is not None and crosses_native_storage(authority["from_version"], authority["to_version"]):
                return None
            return authority
        if versions not in {("0.8.42", "0.8.46"), ("0.8.42", "0.8.47"), ("0.8.46", "0.8.47"),
                ("0.8.42", "0.8.48"), ("0.8.46", "0.8.48"), ("0.8.47", "0.8.48")} or (
                mode == "background" and versions[1] not in {"0.8.47", "0.8.48"}):
            return None
        previous_selection = _candidate_source_selection(previous)
        selection = _candidate_source_selection(candidate)
        if previous_selection is not None:
            return None  # A candidate predecessor needs its own qualified crossing.
        if selection is not None:
            if (versions != ("0.8.47", "0.8.48")
                    or not _same_toml(_parsed_config(candidate, "implementation-report.json")["framework"].get("runtime_candidate"), selection)
                    or any(previous.get(name) != candidate.get(name)
                        for name in previous.keys() | candidate.keys() if name.startswith("application/"))):
                return None
            new.pop("runtimeCandidate")
        old.pop("mobkitVersion")
        new.pop("mobkitVersion")
        old_native = _parsed_config(previous, "config/mob.toml")
        new_native = _parsed_config(candidate, "config/mob.toml")
        registry_update = is_legacy_registry_removal(old_native, new_native)
        if selection is not None and registry_update:
            return None
        if (not _same_toml(old, new) or old.get("target") not in {"mac", "linux"}
                or not _same_toml(_parsed_config(previous, "blueprint.json"),
                                  _parsed_config(candidate, "blueprint.json"))
                or (previous.get("config/mob.toml") != candidate.get("config/mob.toml") and not registry_update)
                or previous.get("requirements.application.txt") != candidate.get("requirements.application.txt")):
            return None
        requirements = previous.get("requirements.txt")
        if (not isinstance(requirements, tuple) or requirements[1] != 0o644
                or requirements[0].splitlines().count(f"meerkat-mobkit=={versions[0]}".encode()) != 1
                or requirements[0].count(f"meerkat-mobkit=={versions[0]}".encode()) != 1
                or candidate.get("requirements.txt") != (
                    requirements[0].replace(f"meerkat-mobkit=={versions[0]}".encode(), f"meerkat-mobkit=={versions[1]}".encode()), 0o644)):
            return None
        return {"kind": "qualified-runtime-dependency-update", "version": 1,
                "from_version": versions[0], "to_version": versions[1], "core_version": "0.8.51",
                "definition_unchanged": not registry_update,
                "native_registry_update": registry_update,
                "recovery": "forward-only-after-runtime-admission",
                **({"runtime_candidate": selection} if selection is not None else {})}
    except (KeyError, ValueError, TypeError, UnicodeError, RecursionError):
        return None


def _meta_toolkit_dependency_authority(previous, candidate, old, new, selection, versions):
    """Describe the existing source-owned crossing, without qualifying a product."""
    from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
    target = parse_candidate_selection(selection)
    if (versions[0] not in {"0.8.47", "0.8.48"} or versions[1] != target.meta.version
            or not _same_toml(_parsed_config(candidate, "implementation-report.json")["framework"].get("runtime_candidate"), selection)):
        return None
    previous_selection = _candidate_source_selection(previous)
    if previous_selection is not None and type(parse_candidate_selection(previous_selection)) is MetaToolkitRuntimeTarget:
        return None
    old = dict(old)
    new = dict(new)
    for value in (old, new):
        value.pop("mobkitVersion")
        value.pop("runtimeCandidate", None)
    if (not _same_toml(old, new) or old.get("target") not in {"mac", "linux"}
            or not _same_toml(_parsed_config(previous, "blueprint.json"), _parsed_config(candidate, "blueprint.json"))
            or previous.get("config/mob.toml") != candidate.get("config/mob.toml")
            or previous.get("requirements.application.txt") != candidate.get("requirements.application.txt")
            or any(previous.get(name) != candidate.get(name) for name in previous.keys() | candidate.keys()
                if name.startswith("application/"))):
        return None
    return {"kind": "selected-meta-toolkit-runtime-dependency-update", "version": 4,
        "from_version": versions[0], "to_version": versions[1], "core_version": None,
        "definition_unchanged": True, "native_registry_update": False,
        "recovery": "forward-only-after-runtime-admission", "runtime_candidate": selection,
        **({"previous_runtime_candidate": previous_selection} if previous_selection is not None else {})}


def _candidate_binding_fields(selection):
    binding = selection["accepted"]
    return {"candidate_binding": binding, "candidate_tuple_sha256": binding["tuple_sha256"],
            "sdk_wheel_sha256": binding["sdk_wheel_sha256"], "source_sha": binding["source_sha"],
            "native_run_id": binding["native_run_id"], "native_run_attempt": binding["native_run_attempt"]}


def _product_protected_paths(files):
    from .implementation_workspace import PROTECTED_APPLICATION_PATHS
    paths = _parsed_config(files, "implementation-report.json")["framework"].get("protected_application_paths", [])
    if (type(paths) is not list or any(type(name) is not str for name in paths)
            or paths != sorted(set(paths)) or not set(paths) <= PROTECTED_APPLICATION_PATHS
            or any(name not in files or files[name][1] != 0o644 for name in paths)):
        raise ValueError("The retained protected product inputs changed.")
    return paths


def _product_framework_paths(files, protected):
    # The product successor has its own explicit contract. Historical v1-v4
    # inventories remain fixed and never acquire adopted application inputs.
    compiler_paths = _runtime_framework_paths(files, 4)
    if set(compiler_paths) & set(protected):
        raise ValueError("Protected product inputs collide with the compiler.")
    return sorted(set(compiler_paths) | set(protected))


def _product_framework_inventory(files, names):
    inventory = {}
    for name in names:
        content, mode = files[name]
        if type(content) is not bytes or type(mode) is not int or mode != (0o755 if name.endswith(".sh") else 0o644):
            raise ValueError("The archived product framework mode changed.")
        inventory[name] = {"sha256": digest(content), "mode": 0o600 if name == ".env.example" else mode}
    sums = "".join(f"{inventory[name]['sha256']}  {name}\n" for name in sorted(inventory)).encode()
    inventory["SHA256SUMS"] = {"sha256": digest(sums), "mode": 0o644}
    return inventory


def _product_framework_provenance(files, protected):
    if not _framework_provenance(files) or _product_protected_paths(files) != protected:
        return False
    framework = _parsed_config(files, "implementation-report.json")["framework"]
    selection = _candidate_source_selection(files)
    basis = framework.get("basis")
    if (framework.get("version") != 1 or type(framework.get("version")) is not int
            or selection is None or not _same_toml(framework.get("runtime_candidate"), selection)
            or not _sha256(framework.get("current_compiler_baseline_digest"))
            or not (basis == "current-compiler" or basis == "accepted-baseline"
                and framework.get("reason") == "unchanged-specification" or basis == "runtime-refresh"
                and framework.get("reason") == "explicit-runtime-refresh")):
        return False
    compiler_paths = _runtime_framework_paths(files, 4)
    full = _product_framework_inventory(files, _product_framework_paths(files, protected))
    if framework.get("baseline_digest") != digest(json.dumps(full, sort_keys=True, separators=(",", ":")).encode()):
        return False
    retained = framework.get("retained_template_paths")
    if (type(retained) is not list or any(type(name) is not str for name in retained)
            or retained != sorted(set(retained)) or not set(retained) <= set(compiler_paths)
            or basis != "runtime-refresh" and retained != []):
        return False
    if basis != "runtime-refresh":
        compiler = _product_framework_inventory(files, compiler_paths)
        if framework["current_compiler_baseline_digest"] != digest(json.dumps(compiler, sort_keys=True, separators=(",", ":")).encode()):
            return False
    return True


def _product_refresh_authority(previous, candidate, receipt, old, new, prior, selected):
    from .runtime_selection import parse_candidate_selection, selected_runtime_marker
    before, after = parse_candidate_selection(prior), parse_candidate_selection(selected)
    if (old.get("mobkitVersion") != before.meta.version or new.get("mobkitVersion") != after.meta.version
            or old.get("runtimeCandidate") != selected_runtime_marker(before)
            or new.get("runtimeCandidate") != selected_runtime_marker(after)
            or not any(_same_toml(receipt.get("runtime_candidate"), value) for value in (prior, selected))):
        return None
    protected = _product_protected_paths(previous)
    if (_product_protected_paths(candidate) != protected
            or not _product_framework_provenance(previous, protected)):
        return None
    old, new = dict(old), dict(new)
    for value in (old, new):
        value.pop("mobkitVersion")
        value.pop("runtimeCandidate")
    framework_paths = set(_product_framework_paths(previous, protected))
    ordinary = lambda files: {name: value for name, value in files.items()
        if name not in framework_paths | {"SHA256SUMS", "implementation-report.json", "acceptance-report.json"}}
    if (not _same_toml(old, new) or old.get("target") not in {"mac", "linux"}
            or before.target != after.target
            or previous.get("blueprint.json") != candidate.get("blueprint.json")
            or previous.get("config/mob.toml") != candidate.get("config/mob.toml")
            or previous.get("config/access.toml") != candidate.get("config/access.toml")
            or ordinary(previous) != ordinary(candidate)
            or any(previous[name] != candidate[name] for name in protected)):
        return None
    return {"kind": "selected-meta-toolkit-runtime-dependency-update", "version": 5,
        "from_version": before.meta.version, "to_version": after.meta.version, "core_version": None,
        "definition_unchanged": True, "native_registry_update": False,
        "recovery": "forward-only-after-runtime-admission", "runtime_candidate": selected,
        "previous_runtime_candidate": prior}


def _product_refresh_descriptor(previous, candidate, authority, *, sources, expected, prior_expected):
    old = _parsed_config(previous, "implementation-report.json")["framework"]
    new = _parsed_config(candidate, "implementation-report.json")["framework"]
    protected = _product_protected_paths(previous)
    return {**authority, "protected_paths": _product_framework_paths(candidate, protected),
        "protected_application_paths": protected,
        "protected_application_inventory": {name: {"sha256": digest(previous[name][0]), "mode": previous[name][1]}
            for name in protected},
        "required_supervisor_sources": sources, "runtime_candidate_artifacts": expected,
        "previous_runtime_candidate_artifacts": prior_expected,
        "accepted_baseline_digest": new["baseline_digest"], "compiler_baseline_digest": new["current_compiler_baseline_digest"],
        "previous_baseline_digest": old["baseline_digest"], "previous_compiler_baseline_digest": old["current_compiler_baseline_digest"]}


def _product_runtime_transition(previous, candidate, receipt, authority, runtime_target, *, reads=None):
    from .compiler import generate_package, generate_runtime_refresh
    from .generator import PackageFile
    from .runtime_selection import candidate_selection, parse_candidate_selection, selected_runtime_artifacts
    if runtime_target is None:
        return None
    verified = selected_runtime_artifacts(runtime_target) if reads is None else reads(runtime_target)
    if not _same_toml(candidate_selection(runtime_target, verified_artifacts=verified), authority["runtime_candidate"]):
        return None
    prior_target = parse_candidate_selection(authority["previous_runtime_candidate"])
    prior_verified = None if reads is None else reads(prior_target)
    prior_expected = _candidate_artifact_expectation(prior_target, verified_artifacts=prior_verified)
    expected = _candidate_artifact_expectation(runtime_target, verified_artifacts=verified)
    protected = _product_protected_paths(previous)
    old_report = _parsed_config(previous, "implementation-report.json")
    framework = _parsed_config(candidate, "implementation-report.json")["framework"]
    source = [PackageFile(name, previous[name][0].decode(), 0o600 if name == ".env.example" else previous[name][1])
        for name in _product_framework_paths(previous, protected)]
    generated = generate_runtime_refresh(_parsed_config(candidate, "blueprint.json"), source,
        runtime_target=runtime_target, verified_artifacts=verified, protected_application_paths=tuple(protected),
        predecessor_artifacts=prior_verified)
    blueprint = _parsed_config(candidate, "blueprint.json")
    ordinary = generate_package(blueprint, runtime_target=runtime_target, verified_artifacts=verified,
        legacy_ordinary=_accepted_ordinary_route(previous, blueprint, runtime_target))
    baseline = {item.path: {"sha256": digest(item.as_bytes()), "mode": item.mode} for item in generated}
    compiler = {item.path: {"sha256": digest(item.as_bytes()), "mode": item.mode} for item in ordinary}
    retained = sorted(name for name in set(baseline) | set(compiler)
        if name != "SHA256SUMS" and name not in protected and baseline.get(name) != compiler.get(name))
    binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
    if (framework.get("basis") != "runtime-refresh" or framework.get("reason") != "explicit-runtime-refresh"
            or not _same_toml(framework.get("origin"), binding) or not _same_toml(framework.get("previous"), binding)
            or framework.get("baseline_digest") != digest(json.dumps(baseline, sort_keys=True, separators=(",", ":")).encode())
            or framework.get("current_compiler_baseline_digest") != digest(json.dumps(compiler, sort_keys=True, separators=(",", ":")).encode())
            or framework.get("retained_template_paths") != retained
            or not _product_framework_provenance(candidate, protected)
            or set(baseline) - {"SHA256SUMS"} != set(_product_framework_paths(candidate, protected))
            or any(candidate.get(item.path) != (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
                for item in generated if item.path != "SHA256SUMS")):
        return None
    sources = _runtime_history_supervisor_sources(4)
    if _archived_supervisor_sources(candidate, {"required_supervisor_sources": sources}, 4) is None:
        return None
    selected_runtime_artifacts(runtime_target, verified_artifacts=verified)
    _candidate_artifact_expectation(prior_target, verified_artifacts=prior_verified)
    return _product_refresh_descriptor(previous, candidate, authority, sources=sources,
        expected=expected, prior_expected=prior_expected)


class KeptRuntimeReads:
    """Verified reads of selected native products, kept for this process.

    A kept read is returned only after the selected runtime reader rechecks
    every verified input's file fingerprint, as an explicit verified_artifacts
    argument always does. A missing or changed read is replaced by a new full
    read. Only a few products are kept; other runtimes are always read.
    """

    def __init__(self, limit=4):
        self._lock = threading.Lock()
        self._kept = []
        self._limit = limit

    def __call__(self, target):
        from .runtime_selection import MetaToolkitRuntimeTarget, selected_runtime_artifacts
        if type(target) is not MetaToolkitRuntimeTarget:
            return selected_runtime_artifacts(target)
        with self._lock:
            kept = next((verified for selected, verified in self._kept if selected == target), None)
        if kept is not None:
            try:
                return selected_runtime_artifacts(target, verified_artifacts=kept)
            except (OSError, ValueError):
                pass
        verified = selected_runtime_artifacts(target)
        with self._lock:
            self._kept = [row for row in self._kept if row[0] != target][-(self._limit - 1):] + [(target, verified)]
        return verified


KEPT_RUNTIME_READS = KeptRuntimeReads()


class SelectedRuntimeReads:
    """One check's selected runtime reads: each runtime is read at most once.

    The first use of a runtime reads it, or takes a kept read when one is
    given. A later use in the same check passes the verified artifacts back
    to the selected runtime reader, which rechecks the inputs' fingerprints,
    so an input that changes during the check refuses it.
    """

    def __init__(self, kept=None):
        self._verified = []
        self._kept = kept

    def __call__(self, target):
        from .runtime_selection import selected_runtime_artifacts
        for selected, verified in self._verified:
            if selected == target:
                return selected_runtime_artifacts(target, verified_artifacts=verified)
        verified = selected_runtime_artifacts(target) if self._kept is None else self._kept(target)
        self._verified.append((target, verified))
        return verified


def _candidate_artifact_expectation(runtime_target, *, verified_artifacts=None):
    """Freeze checked member hashes at current admission, never during history."""
    from .runtime_selection import MetaToolkitRuntimeTarget, selected_runtime_artifacts
    if type(runtime_target) is MetaToolkitRuntimeTarget:
        from dataclasses import asdict
        artifacts = selected_runtime_artifacts(runtime_target, verified_artifacts=verified_artifacts)
        from .runtime_selection import _json_paths
        return {"sdk": _json_paths({"meta": asdict(runtime_target.meta), "toolkit": asdict(runtime_target.toolkit)}),
            "native": {"archive_sha256": artifacts.archive.sha256,
                "binary_sha256": artifacts.candidate["binary"]["sha256"],
                "resources_sha256": artifacts.candidate["resources_sha256"],
                "prefix_inventory_sha256": digest(json.dumps([asdict(entry) for entry in artifacts.entries],
                    sort_keys=True, separators=(",", ":")).encode())},
            "catalog_sha256": runtime_target.model_catalog.sha256}
    from .runtime_candidates import read_candidate_artifacts, recheck_candidate_artifacts, _Inputs, MAX_WHEEL_BYTES
    artifacts = (read_candidate_artifacts(runtime_target.tuple_path, accepted=runtime_target.accepted,
        artifact_root=runtime_target.artifact_root) if verified_artifacts is None else
        recheck_candidate_artifacts(verified_artifacts, tuple_path=runtime_target.tuple_path,
            accepted=runtime_target.accepted, artifact_root=runtime_target.artifact_root))
    # The reader's owned, bounded read also rechecks links, owners and races.
    inputs = _Inputs(runtime_target.artifact_root)
    _, wheel = inputs.read(artifacts.sdk.wheel.path, artifacts.sdk.wheel.sha256,
                          MAX_WHEEL_BYTES, size=artifacts.sdk.wheel.size)
    inputs.finish()
    prefix = f"meerkat_mobkit-{artifacts.identity.native_version}.dist-info/"
    with zipfile.ZipFile(io.BytesIO(wheel)) as archive:
        metadata = {name: digest(archive.read(prefix + name)) for name in ("METADATA", "WHEEL", "top_level.txt")}
    from .runtime_selection import candidate_selection
    selection = candidate_selection(runtime_target, verified_artifacts=artifacts)
    common = _candidate_binding_fields(selection)
    native = {}
    for item in artifacts.native:
        if item.binary not in {"rpc_gateway", "mobkit_gateway"}:
            continue
        receipt = {"source": "signed-candidate", "version": artifacts.identity.native_version,
            "platform": artifacts.identity.platform, "archive": item.archive.path.name,
            "archive_sha256": item.archive.sha256, "binary_sha256": item.inner_sha256, **common}
        native[item.binary] = {"archive": item.archive.path.name, "archive_sha256": item.archive.sha256,
            "binary_sha256": item.inner_sha256, "binary_bytes": item.inner_size,
            "receipt_sha256": digest((json.dumps(receipt, sort_keys=True, indent=2) + "\n").encode())}
    if set(native) != {"rpc_gateway", "mobkit_gateway"}:
        raise ValueError("The candidate must bind both installed native tools.")
    return {"sdk": {"payload_sha256": dict(artifacts.sdk.payload), "metadata_sha256": metadata}, "native": native}


def runtime_dependency_transition(previous, candidate, receipt, *, runtime_target=None, reads=None):
    """Recompute every current protected byte, retaining only accepted design.

    reads, when given, holds one Stage's verified selected runtimes, so each is
    read in full once and only rechecked by later calls.
    """
    authority = _runtime_dependency_authority(previous, candidate, receipt)
    if authority is None:
        return None
    try:
        if authority.get("version") == 5:
            return _product_runtime_transition(previous, candidate, receipt, authority, runtime_target, reads=reads)
        from .compiler import generate_package, generate_runtime_refresh
        from .generator import MOBKIT_VERSION, PackageFile
        from .implementation_workspace import _wheel_path
        selection = authority.get("runtime_candidate")
        expected_artifacts = None
        verified_artifacts = None
        previous_artifacts = None
        if authority.get("previous_runtime_candidate") is not None:
            from .runtime_selection import parse_candidate_selection
            prior_target = parse_candidate_selection(authority["previous_runtime_candidate"])
            previous_artifacts = _candidate_artifact_expectation(prior_target,
                verified_artifacts=None if reads is None else reads(prior_target))
        if selection is not None:
            from .runtime_selection import candidate_selection, selected_runtime_artifacts
            if runtime_target is None:
                return None
            verified_artifacts = (selected_runtime_artifacts(runtime_target) if reads is None
                else reads(runtime_target))
            if not _same_toml(selection, candidate_selection(runtime_target,
                    verified_artifacts=verified_artifacts)):
                return None
            expected_artifacts = _candidate_artifact_expectation(runtime_target,
                verified_artifacts=verified_artifacts)
        elif runtime_target is not None or MOBKIT_VERSION != authority["to_version"]:
            return None
        report = _parsed_config(candidate, "implementation-report.json")
        framework = report["framework"]
        blueprint = _parsed_config(candidate, "blueprint.json")
        ordinary = (generate_package(blueprint, runtime_target=runtime_target,
                        verified_artifacts=verified_artifacts,
                        legacy_ordinary=_accepted_ordinary_route(previous, blueprint, runtime_target))
                    if runtime_target is not None else generate_package(blueprint))
        runtime_refresh = framework.get("basis") == "runtime-refresh"
        if runtime_refresh:
            old_report = _parsed_config(previous, "implementation-report.json")
            source = [PackageFile(name, content if _wheel_path(name) else content.decode(),
                                  0o600 if name == ".env.example" else mode)
                      for name, (content, mode) in previous.items()
                      if name != "SHA256SUMS"]
            generated = (generate_runtime_refresh(_parsed_config(candidate, "blueprint.json"), source,
                            runtime_target=runtime_target, verified_artifacts=verified_artifacts)
                         if runtime_target is not None else generate_runtime_refresh(_parsed_config(candidate, "blueprint.json"), source))
            baseline = {item.path: {"sha256": digest(item.as_bytes()), "mode": item.mode} for item in generated}
            current = {item.path: {"sha256": digest(item.as_bytes()), "mode": item.mode} for item in ordinary}
            # Public application additions are not part of the compiler baseline.
            generated = [item for item in generated if item.path in current]
            baseline = {item.path: baseline[item.path] for item in generated}
            retained = sorted(name for name in baseline.keys() | current.keys()
                if name != "SHA256SUMS" and baseline.get(name) != current.get(name))
            previous_binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
            if (framework.get("reason") != "explicit-runtime-refresh"
                    or not _same_toml(framework.get("previous"), previous_binding)
                    or not _same_toml(framework.get("origin"), previous_binding)
                    or framework.get("baseline_digest") != digest(json.dumps(baseline, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
                    or framework.get("current_compiler_baseline_digest") != digest(json.dumps(current, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
                    or framework.get("retained_template_paths") != retained
                    or any(previous.get(name) != candidate.get(name) for name in previous
                           if name.startswith("application/") or name == "requirements.application.txt")
                    or any(name not in previous for name in candidate if name.startswith("application/"))):
                return None
        else:
            if not current_host_framework(candidate, runtime_target=runtime_target,
                    verified_artifacts=verified_artifacts):
                return None
            generated = ordinary
        protected = {item.path: (item.as_bytes(), 0o755 if item.mode & 0o111 else 0o644)
                     for item in generated if item.path != "SHA256SUMS"}
        if any(candidate.get(name) != value for name, value in protected.items()):
            return None
        if verified_artifacts is not None:
            selected_runtime_artifacts(runtime_target, verified_artifacts=verified_artifacts)
        # Admission and closed history must use the same separately fixed
        # inventory. A current export relabeled as historical v1 is refused.
        product = authority["kind"] == "selected-meta-toolkit-runtime-dependency-update"
        version = 4 if product or authority["to_version"] in {"0.8.47", "0.8.48"} else 1
        names = _runtime_framework_paths(candidate, version)
        if set(protected) != set(names) or not _archived_framework_provenance(candidate, names):
            return None
        sources = _runtime_history_supervisor_sources(version) if version in {2, 3, 4} else None
        if version in {2, 3, 4} and _archived_supervisor_sources(candidate,
                {"required_supervisor_sources": sources}, version) is None:
            return None
        return {**authority, "version": version, "protected_paths": names,
            **({"required_supervisor_sources": sources} if version in {2, 3, 4} else {}),
            **({"runtime_candidate_artifacts": expected_artifacts} if selection is not None else {}),
            **({"previous_runtime_candidate_artifacts": previous_artifacts} if previous_artifacts is not None else {}),
            **({"accepted_baseline_digest": framework["baseline_digest"],
                "compiler_baseline_digest": framework["current_compiler_baseline_digest"]}
               if product or authority["to_version"] in {"0.8.47", "0.8.48"} else {})}
    except (OSError, KeyError, ValueError, TypeError, UnicodeError, RecursionError, zipfile.BadZipFile):
        return None


# The two admitted .46 compiler generations share this frozen export contract.
# Its path/mode inventory is historical source provenance, not a new host grant.
_RETAINED_FRAMEWORK_PATHS = frozenset({
    ".env.example", ".gitignore", "README.md", "app.py", "application_configuration.py",
    "blueprint.json", "config/connections.json", "config/console.toml", "config/mob.toml",
    "connectors.py", "console_auth.py", "console_configuration.py", "deploy/deploy-gcp.sh",
    "deploy/install-service.sh", "deploy/tunnel-gcp.sh", "deployment.json", "install.sh",
    "install_gateway.py", "mcp_connections.py", "remote_operations.py", "requirements.txt",
    "setup.py", "start.sh", "storage_setup.py", "workspace/START-HERE.txt",
})
_RETAINED_KNOWLEDGE_PATHS = frozenset({"knowledge/settings.json", "knowledge/manage.py",
    "knowledge/README.md", "knowledge/.gitignore"})
_RETAINED_LOCAL_KNOWLEDGE_PATHS = frozenset({"knowledge/compose.json", "knowledge/Dockerfile",
    "knowledge/Dockerfile.dockerignore"})


def _archived_framework_paths(candidate):
    names = set(_RETAINED_FRAMEWORK_PATHS)
    if _candidate_source_selection(candidate) is not None:
        names.update(CANDIDATE_PATHS)
        names.update(_candidate_reader_helper_paths(candidate))
    if "native_definition_continuity.py" in candidate:
        names.add("native_definition_continuity.py")
    knowledge = _parsed_config(candidate, "deployment.json")["knowledge"]["mode"]
    if knowledge not in {"none", "remote", "local"}:
        raise ValueError
    if knowledge != "none":
        names.update(_RETAINED_KNOWLEDGE_PATHS)
    if knowledge == "local":
        names.update(_RETAINED_LOCAL_KNOWLEDGE_PATHS)
    return sorted(names)


def _archived_framework_provenance(candidate, names, *, absent_template_paths=frozenset()):
    if not _framework_provenance(candidate):
        return False
    report = _parsed_config(candidate, "implementation-report.json")
    framework = report["framework"]
    selection = _candidate_source_selection(candidate)
    if ((selection is not None and not _same_toml(framework.get("runtime_candidate"), selection))
            or selection is None and "runtime_candidate" in framework):
        return False
    basis = (framework.get("basis") == "current-compiler" or
             framework.get("basis") == "accepted-baseline" and
             framework.get("reason") == "unchanged-specification" or
             framework.get("basis") == "runtime-refresh" and framework.get("reason") == "explicit-runtime-refresh")
    runtime_refresh = framework.get("basis") == "runtime-refresh"
    if (type(framework.get("version")) is not int or framework["version"] != 1
            or not basis or (not runtime_refresh and framework.get("retained_template_paths") != [])
            or runtime_refresh and (type(framework.get("retained_template_paths")) is not list
                or any(name not in {"blueprint.json", "config/mob.toml", "config/console.toml", "config/connections.json",
                    ".env.example", "workspace/START-HERE.txt", "mcp_connections.py", "deployment.json", "requirements.txt"}
                    and not name.startswith("knowledge/") and name not in absent_template_paths
                    for name in framework["retained_template_paths"]))):
        return False
    inventory = {}
    for name in names:
        content, mode = candidate[name]
        if type(mode) is not int or mode != (0o755 if name.endswith(".sh") else 0o644):
            return False
        # This exact compiler contract seeded one private example at 0600.
        # Keep the archive loader's 0644 projection for actual source checks.
        inventory[name] = {"sha256": digest(content), "mode": 0o600 if name == ".env.example" else mode}
    sums = "".join(f"{inventory[name]['sha256']}  {name}\n" for name in names).encode()
    inventory["SHA256SUMS"] = {"sha256": digest(sums), "mode": 0o644}
    fingerprint = digest(json.dumps(inventory, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
    return (framework.get("baseline_digest") == fingerprint
            and (framework.get("current_compiler_baseline_digest") == fingerprint
                 or framework.get("basis") == "runtime-refresh" and _sha256(framework.get("current_compiler_baseline_digest"))))


# This historical ordinary host generation retains its fixed export contract.
# A successor does not extend it or the seven-file maintained refresh.
_RUNTIME_FRAMEWORK_V2_PATHS = frozenset({
    '.env.example',
    '.gitignore',
    'README.md',
    'app.py',
    'application_configuration.py',
    'blueprint.json',
    'config/connections.json',
    'config/console.toml',
    'config/mob.toml',
    'connectors.py',
    'console_auth.py',
    'console_binding.py',
    'console_configuration.py',
    'console_connect.py',
    'console_presentation.py',
    'console_setup.css',
    'console_setup.mjs',
    'console_shell.css',
    'console_shell.mjs',
    'console_shell.py',
    'deploy/deploy-gcp.sh',
    'deploy/install-service.sh',
    'deploy/tunnel-gcp.sh',
    'deployment.json',
    'install.sh',
    'install_gateway.py',
    'mcp_connections.py',
    'native_definition_continuity.py',
    'remote_operations.py',
    'requirements.txt',
    'setup.py',
    'start.sh',
    'storage_setup.py',
    'toolkit-ui/LICENSE-APACHE',
    'toolkit-ui/LICENSE-MIT',
    'toolkit-ui/THIRD_PARTY_NOTICES.md',
    'toolkit-ui/index.d.ts',
    'toolkit-ui/index.js',
    'toolkit-ui/model.d.ts',
    'toolkit-ui/model.js',
    'toolkit-ui/package.json',
    'toolkit-ui/projection.d.ts',
    'toolkit-ui/projection.js',
    'toolkit-ui/setup-conversation.d.ts',
    'toolkit-ui/setup-conversation.js',
    'toolkit-ui/setup-markdown.d.ts',
    'toolkit-ui/setup-markdown.js',
    'toolkit-ui/setup-model.d.ts',
    'toolkit-ui/setup-model.js',
    'toolkit-ui/style.css',
    'toolkit-ui/vendor/entities/LICENSE',
    'toolkit-ui/vendor/entities/decode-codepoint.js',
    'toolkit-ui/vendor/entities/decode.js',
    'toolkit-ui/vendor/entities/generated/decode-data-html.js',
    'toolkit-ui/vendor/entities/generated/decode-data-xml.js',
    'toolkit-ui/vendor/entities/internal/bin-trie-flags.js',
    'toolkit-ui/vendor/entities/internal/decode-shared.js',
    'toolkit-ui/vendor/entities/package.json',
    'toolkit-ui/vendor/marked/LICENSE',
    'toolkit-ui/vendor/marked/marked.esm.js',
    'toolkit-ui/vendor/marked/package.json',
    'toolkit-ui/view.d.ts',
    'toolkit-ui/view.js',
    'workspace/START-HERE.txt',
})
_RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS = (
    'launchkit/__init__.py',
    'launchkit/application_access.py',
    'launchkit/application_experiences.py',
    'launchkit/application_first_use.py',
    'launchkit/artifact_binding.py',
    'launchkit/blueprint-limits.json',
    'launchkit/blueprint.py',
    'launchkit/bootstrap.py',
    'launchkit/checker_runtime.py',
    'launchkit/compiler.py',
    'launchkit/deployment.py',
    'launchkit/deployment_cache.py',
    'launchkit/deployment_deadlines.py',
    'launchkit/deployment_evidence.py',
    'launchkit/deployment_experiences.py',
    'launchkit/deployment_ports.py',
    'launchkit/deployment_service.py',
    'launchkit/deployment_sources.py',
    'launchkit/deployment_supervisor.py',
    'launchkit/deployment_updates.py',
    'launchkit/deployment_worker.py',
    'launchkit/design_knowledge.py',
    'launchkit/distribution.py',
    'launchkit/dom_check.py',
    'launchkit/elephant_setup.py',
    'launchkit/gcp_transport.py',
    'launchkit/generator.py',
    'launchkit/guide_models.py',
    'launchkit/implementation_workspace.py',
    'launchkit/installed_feedback.py',
    'launchkit/linux_sandbox.py',
    'launchkit/native_definition_continuity.py',
    'launchkit/native_models.py',
    'launchkit/planning_limits.py',
    'launchkit/projects.py',
    'launchkit/remote_browser_access.py',
    'launchkit/remote_deployment.py',
    'launchkit/remote_destination.py',
    'launchkit/remote_installed_feedback.py',
    'launchkit/remote_interface_bundle.py',
    'launchkit/runtime_candidates.py',
    'launchkit/runtime_dependencies.py',
    'launchkit/runtime_selection.py',
    'launchkit/schema.py',
    'launchkit/ssh_browser_access.py',
    'launchkit/ssh_transport.py',
    'launchkit/templates/app.py',
    'launchkit/templates/candidate_gateway_overlay.py',
    'launchkit/templates/connectors.py',
    'launchkit/templates/deploy-gcp.sh',
    'launchkit/templates/install-service.sh',
    'launchkit/templates/install.sh',
    'launchkit/templates/install_candidate.py',
    'launchkit/templates/install_gateway.py',
    'launchkit/templates/setup.py',
    'launchkit/templates/start.sh',
    'launchkit/templates/storage_setup.py',
    'launchkit/templates_elephant/Dockerfile',
    'launchkit/templates_elephant/README.md',
    'launchkit/templates_elephant/manage.py',
    'launchkit/templates_v2/app.py',
    'launchkit/templates_v2/application_configuration.py',
    'launchkit/templates_v2/console_auth.py',
    'launchkit/templates_v2/console_binding.py',
    'launchkit/templates_v2/console_configuration.py',
    'launchkit/templates_v2/console_connect.py',
    'launchkit/templates_v2/console_presentation.py',
    'launchkit/templates_v2/console_setup.css',
    'launchkit/templates_v2/console_setup.mjs',
    'launchkit/templates_v2/console_shell.css',
    'launchkit/templates_v2/console_shell.mjs',
    'launchkit/templates_v2/console_shell.py',
    'launchkit/templates_v2/mcp_connections.py',
    'launchkit/templates_v2/remote_operations.py',
    'launchkit/templates_v2/setup.py',
    'launchkit/templates_v2/toolkit-ui/LICENSE-APACHE',
    'launchkit/templates_v2/toolkit-ui/LICENSE-MIT',
    'launchkit/templates_v2/toolkit-ui/THIRD_PARTY_NOTICES.md',
    'launchkit/templates_v2/toolkit-ui/index.d.ts',
    'launchkit/templates_v2/toolkit-ui/index.js',
    'launchkit/templates_v2/toolkit-ui/model.d.ts',
    'launchkit/templates_v2/toolkit-ui/model.js',
    'launchkit/templates_v2/toolkit-ui/package.json',
    'launchkit/templates_v2/toolkit-ui/projection.d.ts',
    'launchkit/templates_v2/toolkit-ui/projection.js',
    'launchkit/templates_v2/toolkit-ui/setup-conversation.d.ts',
    'launchkit/templates_v2/toolkit-ui/setup-conversation.js',
    'launchkit/templates_v2/toolkit-ui/setup-markdown.d.ts',
    'launchkit/templates_v2/toolkit-ui/setup-markdown.js',
    'launchkit/templates_v2/toolkit-ui/setup-model.d.ts',
    'launchkit/templates_v2/toolkit-ui/setup-model.js',
    'launchkit/templates_v2/toolkit-ui/style.css',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/LICENSE',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/decode-codepoint.js',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/decode.js',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/generated/decode-data-html.js',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/generated/decode-data-xml.js',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/internal/bin-trie-flags.js',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/internal/decode-shared.js',
    'launchkit/templates_v2/toolkit-ui/vendor/entities/package.json',
    'launchkit/templates_v2/toolkit-ui/vendor/marked/LICENSE',
    'launchkit/templates_v2/toolkit-ui/vendor/marked/marked.esm.js',
    'launchkit/templates_v2/toolkit-ui/vendor/marked/package.json',
    'launchkit/templates_v2/toolkit-ui/view.d.ts',
    'launchkit/templates_v2/toolkit-ui/view.js',
)
_RUNTIME_FRAMEWORK_V2_COPIED_SOURCES = {
    'app.py': 'launchkit/templates_v2/app.py',
    'application_configuration.py': 'launchkit/templates_v2/application_configuration.py',
    'connectors.py': 'launchkit/templates/connectors.py',
    'console_auth.py': 'launchkit/templates_v2/console_auth.py',
    'console_binding.py': 'launchkit/templates_v2/console_binding.py',
    'console_configuration.py': 'launchkit/templates_v2/console_configuration.py',
    'console_connect.py': 'launchkit/templates_v2/console_connect.py',
    'console_presentation.py': 'launchkit/templates_v2/console_presentation.py',
    'console_setup.css': 'launchkit/templates_v2/console_setup.css',
    'console_setup.mjs': 'launchkit/templates_v2/console_setup.mjs',
    'console_shell.css': 'launchkit/templates_v2/console_shell.css',
    'console_shell.mjs': 'launchkit/templates_v2/console_shell.mjs',
    'console_shell.py': 'launchkit/templates_v2/console_shell.py',
    'deploy/install-service.sh': 'launchkit/templates/install-service.sh',
    'native_definition_continuity.py': 'launchkit/native_definition_continuity.py',
    'setup.py': 'launchkit/templates_v2/setup.py',
    'start.sh': 'launchkit/templates/start.sh',
    'storage_setup.py': 'launchkit/templates/storage_setup.py',
    'toolkit-ui/LICENSE-APACHE': 'launchkit/templates_v2/toolkit-ui/LICENSE-APACHE',
    'toolkit-ui/LICENSE-MIT': 'launchkit/templates_v2/toolkit-ui/LICENSE-MIT',
    'toolkit-ui/THIRD_PARTY_NOTICES.md': 'launchkit/templates_v2/toolkit-ui/THIRD_PARTY_NOTICES.md',
    'toolkit-ui/index.d.ts': 'launchkit/templates_v2/toolkit-ui/index.d.ts',
    'toolkit-ui/index.js': 'launchkit/templates_v2/toolkit-ui/index.js',
    'toolkit-ui/model.d.ts': 'launchkit/templates_v2/toolkit-ui/model.d.ts',
    'toolkit-ui/model.js': 'launchkit/templates_v2/toolkit-ui/model.js',
    'toolkit-ui/package.json': 'launchkit/templates_v2/toolkit-ui/package.json',
    'toolkit-ui/projection.d.ts': 'launchkit/templates_v2/toolkit-ui/projection.d.ts',
    'toolkit-ui/projection.js': 'launchkit/templates_v2/toolkit-ui/projection.js',
    'toolkit-ui/setup-conversation.d.ts': 'launchkit/templates_v2/toolkit-ui/setup-conversation.d.ts',
    'toolkit-ui/setup-conversation.js': 'launchkit/templates_v2/toolkit-ui/setup-conversation.js',
    'toolkit-ui/setup-markdown.d.ts': 'launchkit/templates_v2/toolkit-ui/setup-markdown.d.ts',
    'toolkit-ui/setup-markdown.js': 'launchkit/templates_v2/toolkit-ui/setup-markdown.js',
    'toolkit-ui/setup-model.d.ts': 'launchkit/templates_v2/toolkit-ui/setup-model.d.ts',
    'toolkit-ui/setup-model.js': 'launchkit/templates_v2/toolkit-ui/setup-model.js',
    'toolkit-ui/style.css': 'launchkit/templates_v2/toolkit-ui/style.css',
    'toolkit-ui/vendor/entities/LICENSE': 'launchkit/templates_v2/toolkit-ui/vendor/entities/LICENSE',
    'toolkit-ui/vendor/entities/decode-codepoint.js': 'launchkit/templates_v2/toolkit-ui/vendor/entities/decode-codepoint.js',
    'toolkit-ui/vendor/entities/decode.js': 'launchkit/templates_v2/toolkit-ui/vendor/entities/decode.js',
    'toolkit-ui/vendor/entities/generated/decode-data-html.js': 'launchkit/templates_v2/toolkit-ui/vendor/entities/generated/decode-data-html.js',
    'toolkit-ui/vendor/entities/generated/decode-data-xml.js': 'launchkit/templates_v2/toolkit-ui/vendor/entities/generated/decode-data-xml.js',
    'toolkit-ui/vendor/entities/internal/bin-trie-flags.js': 'launchkit/templates_v2/toolkit-ui/vendor/entities/internal/bin-trie-flags.js',
    'toolkit-ui/vendor/entities/internal/decode-shared.js': 'launchkit/templates_v2/toolkit-ui/vendor/entities/internal/decode-shared.js',
    'toolkit-ui/vendor/entities/package.json': 'launchkit/templates_v2/toolkit-ui/vendor/entities/package.json',
    'toolkit-ui/vendor/marked/LICENSE': 'launchkit/templates_v2/toolkit-ui/vendor/marked/LICENSE',
    'toolkit-ui/vendor/marked/marked.esm.js': 'launchkit/templates_v2/toolkit-ui/vendor/marked/marked.esm.js',
    'toolkit-ui/vendor/marked/package.json': 'launchkit/templates_v2/toolkit-ui/vendor/marked/package.json',
    'toolkit-ui/view.d.ts': 'launchkit/templates_v2/toolkit-ui/view.d.ts',
    'toolkit-ui/view.js': 'launchkit/templates_v2/toolkit-ui/view.js',
}


_RUNTIME_FRAMEWORK_V3_REVIEW_PATHS = frozenset({
    "native_controller_review.py", "native_console_login.py",
    "controller_review.mjs", "controller_review.css",
})
_RUNTIME_FRAMEWORK_V3_PATHS = _RUNTIME_FRAMEWORK_V2_PATHS | _RUNTIME_FRAMEWORK_V3_REVIEW_PATHS
_RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS = (*_RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS,
    "launchkit/templates_v2/native_controller_review.py",
    "launchkit/templates_v2/native_console_login.py",
    "launchkit/templates_v2/controller_review.mjs",
    "launchkit/templates_v2/controller_review.css",
)
_RUNTIME_FRAMEWORK_V3_COPIED_SOURCES = {
    **_RUNTIME_FRAMEWORK_V2_COPIED_SOURCES,
    "native_controller_review.py": "launchkit/templates_v2/native_controller_review.py",
    "native_console_login.py": "launchkit/templates_v2/native_console_login.py",
    "controller_review.mjs": "launchkit/templates_v2/controller_review.mjs",
    "controller_review.css": "launchkit/templates_v2/controller_review.css",
}

# This successor ships dormant trusted application-startup helpers. Its source
# closure does not admit native Connect inputs or widen the maintained refresh.
_RUNTIME_FRAMEWORK_V4_HELPER_PATHS = frozenset({
    "launchkit/connect_application_finalizer.py",
    "launchkit/connect_application_policy.py",
    "launchkit/connect_installer_caller.py",
    "launchkit/connect_policy_input.py",
    "launchkit/connect_application_startup.py",
    "launchkit/connect_application_callbacks.py",
})
_RUNTIME_FRAMEWORK_V4_PATHS = (_RUNTIME_FRAMEWORK_V3_PATHS |
    _RUNTIME_FRAMEWORK_V4_HELPER_PATHS | {"launchkit/__init__.py"})
_RUNTIME_FRAMEWORK_V4_SUPERVISOR_PATHS = (*_RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS,
    *sorted(_RUNTIME_FRAMEWORK_V4_HELPER_PATHS))
_RUNTIME_FRAMEWORK_V4_COPIED_SOURCES = {
    **_RUNTIME_FRAMEWORK_V3_COPIED_SOURCES,
    **{name: name for name in _RUNTIME_FRAMEWORK_V4_HELPER_PATHS},
}


def _runtime_framework_paths(candidate, version):
    """Dispatch fixed archived contracts, never infer a generation from files."""
    if type(version) is not int or version not in {1, 2, 3, 4}:
        raise ValueError("Unknown runtime framework history contract.")
    if version == 1:
        return _archived_framework_paths(candidate)
    names = set({2: _RUNTIME_FRAMEWORK_V2_PATHS, 3: _RUNTIME_FRAMEWORK_V3_PATHS,
        4: _RUNTIME_FRAMEWORK_V4_PATHS}[version])
    if _candidate_source_selection(candidate) is not None:
        names.update(CANDIDATE_PATHS)
        names.update(_candidate_reader_helper_paths(candidate))
    knowledge = _parsed_config(candidate, "deployment.json")["knowledge"]["mode"]
    if knowledge not in {"none", "remote", "local"}:
        raise ValueError("Unknown recorded knowledge mode.")
    if knowledge != "none":
        names.update(_RETAINED_KNOWLEDGE_PATHS)
    if knowledge == "local":
        names.update(_RETAINED_LOCAL_KNOWLEDGE_PATHS)
    return sorted(names)


def _runtime_supervisor_paths(version, *, sources=None):
    if type(version) is not int or version not in {2, 3, 4}:
        raise ValueError("Unknown runtime supervisor history contract.")
    names = {2: _RUNTIME_FRAMEWORK_V2_SUPERVISOR_PATHS,
        3: _RUNTIME_FRAMEWORK_V3_SUPERVISOR_PATHS,
        4: _RUNTIME_FRAMEWORK_V4_SUPERVISOR_PATHS}[version]
    if sources is None or _SDK_WHEEL_SUPERVISOR_HELPER in sources:
        return (*names, _SDK_WHEEL_SUPERVISOR_HELPER)
    return names


def _runtime_history_supervisor_sources(version=3):
    """Pin the selected fixed trusted reader and compiler closure."""
    source = Path(__file__).parent
    result = {}
    for name in _runtime_supervisor_paths(version):
        path = source.joinpath(*Path(name).parts[1:])
        if (path.is_symlink() or not path.is_file()
                or path.stat().st_size > 4 * 1024 * 1024):
            raise ValueError("The fixed runtime history reader source is unavailable.")
        result[name] = digest(path.read_bytes())
    return result


def _archived_supervisor_sources(candidate, descriptor, version):
    # Closed history authenticates its archived source qualification. It does
    # not compare with a later compiler or use these hashes as runtime grants.
    sources = descriptor.get("required_supervisor_sources")
    if (type(sources) is not dict or set(sources) != set(_runtime_supervisor_paths(version, sources=sources))
            or not all(_sha256(value) for value in sources.values())):
        return None
    copied = dict({2: _RUNTIME_FRAMEWORK_V2_COPIED_SOURCES,
        3: _RUNTIME_FRAMEWORK_V3_COPIED_SOURCES, 4: _RUNTIME_FRAMEWORK_V4_COPIED_SOURCES}[version])
    # The exported package initializer is generated empty. The real supervisor
    # initializer has its separate source hash and must not replace these bytes.
    if version == 4 and candidate.get("launchkit/__init__.py") != (b"", 0o644):
        return None
    selection = _candidate_source_selection(candidate)
    if selection is not None:
        from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
        if type(parse_candidate_selection(selection)) is MetaToolkitRuntimeTarget:
            # Product entrypoints are generated refusal guards, rather than
            # ordinary shell template copies. Admission compared their exact
            # current compiler bytes; closed history authenticates the retained
            # baseline/source hashes without consulting a later compiler.
            if any(name not in candidate or candidate[name][1] !=
                    (0o755 if name.endswith(".sh") else 0o644)
                    for name in ("install.sh", "start.sh", "install_gateway.py")):
                return None
            copied.pop("start.sh", None)
        elif digest(candidate["start.sh"][0]) != sources["launchkit/templates/start.sh"]:
            # This fixed candidate overlay replaces one ordinary recovery line.
            # Recover its retained template bytes, rather than consulting a
            # later compiler or accepting a changed archived source-map hash.
            overlay = (b'  echo "The installed application interpreter is unavailable." >&2\n'
                b'  echo "Keep this installed project folder, .venv, private settings, agent history and the accepted external artifact bundle in place." >&2\n'
                b'  echo "Correct the reported cause, then return to this saved installation in Forge and use its offered update or recovery controls." >&2\n'
                b'  echo "Do not run this private candidate install.sh manually or start a second installation." >&2\n')
            script, mode = candidate["start.sh"]
            if (mode != 0o755 or script.count(overlay) != 1
                    or digest(script.replace(overlay, b'  echo "Run bash install.sh first." >&2\n'))
                    != sources["launchkit/templates/start.sh"]):
                return None
            copied.pop("start.sh", None)
        copied.update({"runtime_candidates.py": "launchkit/runtime_candidates.py",
                       "install_candidate.py": "launchkit/templates/install_candidate.py"})
        for name in _candidate_reader_helper_paths(candidate):
            if _SDK_WHEEL_SUPERVISOR_HELPER not in sources:
                return None
            copied[name] = _SDK_WHEEL_SUPERVISOR_HELPER
    # Accepted MCP and knowledge source are part of the retained design,
    # authenticated by the fixed baseline, rather than freshly copied host.
    if any(sources[path] != digest(candidate[name][0]) for name, path in copied.items()):
        return None
    return sources


def _archived_v2_supervisor_sources(candidate, descriptor):
    return _archived_supervisor_sources(candidate, descriptor, 2)


def _require_runtime_framework_configuration(receipt, update):
    descriptor = update.get("runtime_dependency_update")
    if (type(descriptor) is dict and type(descriptor.get("version")) is int
            and descriptor["version"] in {2, 3, 4, 5}):
        from .deployment import private_configuration_snapshot
        values, _ = private_configuration_snapshot(owned_destination(receipt["destination"]))
        if values.get("MOBKIT_CONNECT_BIN"):
            raise DraftConflict("The selected optional Connect host needs its own qualified runtime crossing. Preserve its current selection and private settings.")


def _sha256(value):
    return isinstance(value, str) and len(value) == 64 and all(character in "0123456789abcdef" for character in value)


def _retained_native_proof(proof, version):
    if (type(proof) is not dict or proof.get("version") != version
            or proof.get("source") != "official-release"
            or proof.get("platform") not in {"aarch64-apple-darwin", "x86_64-apple-darwin",
                "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"}
            or version in {"0.8.46", "0.8.47", "0.8.48"} and proof.get("core_version") != "0.8.51"
            or type(proof.get("binaries")) is not dict
            or set(proof["binaries"]) != {"rpc_gateway", "mobkit_gateway"}):
        return False
    release = f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{version}"
    for name, prefix in (("rpc_gateway", "mobkit-rpc-gateway"), ("mobkit_gateway", "mobkit-gateway")):
        binary = proof["binaries"][name]
        if (type(binary) is not dict or binary.get("release_url") != release
                or binary.get("archive") != f"{prefix}-{version}-{proof['platform']}.tar.gz"
                or any(not _sha256(binary.get(key)) for key in
                       ("receipt_sha256", "binary_sha256", "archive_sha256"))):
            return False
    return True


def _candidate_proof_binding(proof, selection):
    return all(_same_toml(proof.get(key), value)
               for key, value in _candidate_binding_fields(selection).items())


def _retained_candidate_proof(proof, sdk, selection, expected, destination):
    """Validate frozen installed proof without reading an artifact or process."""
    binding = selection["accepted"]
    if (type(expected) is not dict or set(expected) != {"sdk", "native"}
            or type(expected["sdk"]) is not dict
            or set(expected["sdk"]) != {"payload_sha256", "metadata_sha256"}
            or type(expected["native"]) is not dict
            or set(expected["native"]) != {"rpc_gateway", "mobkit_gateway"}
            or type(proof) is not dict or proof.get("source") != "signed-candidate"
            or proof.get("version") != binding["native_version"]
            or proof.get("core_version") != binding["core_version"]
            or proof.get("platform") != binding["platform"]
            or not _candidate_proof_binding(proof, selection)
            or type(proof.get("binaries")) is not dict
            or set(proof["binaries"]) != {"rpc_gateway", "mobkit_gateway"}
            or type(sdk) is not dict or not _same_toml(proof.get("sdk"), sdk)
            or sdk.get("format") != "forge-installed-candidate-sdk-v1"
            or sdk.get("source") != "local-build-from-exact-frozen-source"
            or sdk.get("signed") is not False or sdk.get("published") is not False
            or sdk.get("version") != binding["native_version"]
            or not _candidate_proof_binding(sdk, selection)):
        return False
    site = sdk.get("site_packages")
    if (not isinstance(site, str) or not site or len(site) > 4096 or "\x00" in site
            or not Path(site).is_absolute() or str(Path(site)) != site or ".." in Path(site).parts
            or not Path(site).is_relative_to(Path(destination) / ".venv")):
        return False
    payload, metadata = expected["sdk"]["payload_sha256"], expected["sdk"]["metadata_sha256"]
    if (type(payload) is not dict or not payload or len(payload) > 4096
            or any(not isinstance(name, str) or not name.startswith("meerkat_mobkit/")
                or ".." in Path(name).parts or not _sha256(value) for name, value in payload.items())
            or type(metadata) is not dict or set(metadata) != {"METADATA", "WHEEL", "top_level.txt"}
            or any(not _sha256(value) for value in metadata.values())
            or not _same_toml(sdk.get("payload_sha256"), payload)
            or not _same_toml(sdk.get("metadata_sha256"), metadata)):
        return False
    for name, prefix in (("rpc_gateway", "mobkit-rpc-gateway"), ("mobkit_gateway", "mobkit-gateway")):
        frozen, actual = expected["native"][name], proof["binaries"][name]
        if (type(frozen) is not dict
                or set(frozen) != {"archive", "archive_sha256", "binary_sha256", "binary_bytes", "receipt_sha256"}
                or type(frozen["binary_bytes"]) is not int or not 0 < frozen["binary_bytes"] <= 512 * 1024 * 1024
                or frozen["archive"] != f"{prefix}-{binding['native_version']}-{binding['platform']}.tar.gz"
                or any(not _sha256(frozen[key]) for key in ("archive_sha256", "binary_sha256", "receipt_sha256"))
                or type(actual) is not dict or set(actual) != {"archive", "archive_sha256", "binary_sha256", "receipt_sha256"}
                or any(not _same_toml(actual.get(key), frozen[key]) for key in actual)):
            return False
    return True


def _retained_product_proof(proof, sdk, selection, expected, destination):
    """Read the existing closed transaction's selected byte facts, not admission."""
    from dataclasses import asdict
    from .runtime_selection import (MetaToolkitRuntimeTarget, parse_candidate_selection,
        selected_runtime_platform, _json_paths)
    target = parse_candidate_selection(selection)
    if (type(target) is not MetaToolkitRuntimeTarget or type(expected) is not dict
            or set(expected) != {"sdk", "native", "catalog_sha256"}
            or not _same_toml(expected["sdk"], _json_paths({"meta": asdict(target.meta), "toolkit": asdict(target.toolkit)}))
            or expected["catalog_sha256"] != target.model_catalog.sha256
            or type(expected["native"]) is not dict or set(expected["native"]) != {
                "archive_sha256", "binary_sha256", "resources_sha256", "prefix_inventory_sha256"}
            or any(not _sha256(value) for value in expected["native"].values())
            or type(proof) is not dict or proof.get("source") != "meta-toolkit-product"
            or proof.get("version") != target.meta.version or proof.get("platform") != selected_runtime_platform(target)
            or proof.get("assembly_sha256") != target.assembly.sha256
            or proof.get("catalog_sha256") != expected["catalog_sha256"]
            or not _same_toml(proof.get("sdk"), sdk) or not _same_toml(sdk, expected["sdk"])
            or not _same_toml(proof.get("binaries"), {"toolkit_gateway": {
                "binary_sha256": expected["native"]["binary_sha256"]}})):
        return False
    return all(proof.get(key) == expected["native"][key] for key in
        ("archive_sha256", "resources_sha256", "prefix_inventory_sha256"))


def _completed_product_runtime_transition(previous, candidate, receipt, update, root):
    """Read product v5 history solely from the original archived source/proofs."""
    authority = _runtime_dependency_authority(previous, candidate, receipt)
    descriptor = update["runtime_dependency_update"]
    if (authority is None or authority.get("version") != 5 or descriptor.get("version") != 5
            or update.get("status") != "completed" or update.get("blockers") != []
            or update.get("rollback_available") is not False or update.get("runtime_admission_possible") is not True):
        return None
    protected = _product_protected_paths(previous)
    if not _product_framework_provenance(candidate, protected):
        return None
    sources = _archived_supervisor_sources(candidate, descriptor, 4)
    if sources is None:
        return None
    old = _parsed_config(previous, "implementation-report.json")
    report = _parsed_config(candidate, "implementation-report.json")
    binding = {"build_id": old["build_id"], "source_digest": old["verification"]["digest"]}
    framework = report["framework"]
    previous_bound = {key: report_brief_hash(old) if key == "brief_hash" else
        update["previous_package_sha256"] if key == "package_sha256" else old[key] for key in BOUND_KEYS}
    bound = {key: report_brief_hash(report) if key == "brief_hash" else
        update["package_sha256"] if key == "package_sha256" else report[key] for key in BOUND_KEYS}
    if (not _same_toml(json.loads((root / "previous-receipt.json").read_text()), previous_bound)
            or not _same_toml({key: receipt.get(key) for key in BOUND_KEYS}, bound)
            or not _same_toml([update["from_build_id"], update["previous_source_digest"], update["to_build_id"],
                update["source_digest"], update["guide_version"], update["blueprint_hash"], update["brief_hash"]],
                [old["build_id"], old["verification"]["digest"], report["build_id"], report["verification"]["digest"],
                 report["guide_version"], report["blueprint_hash"], report_brief_hash(report)])
            or framework.get("basis") != "runtime-refresh"
            or not _same_toml(framework.get("previous"), binding) or not _same_toml(framework.get("origin"), binding)
            or update.get("native_definition_update") is not False
            or type(update.get("host_instruction_update")) is not bool
            or any(update.get(key) is not None for key in ("runtime_storage_upgrade", "native_definition_observation",
                "native_activation_expected_revision", "native_activation_package_sha256",
                "native_definition_activation", "native_definition_recovery_observation"))):
        return None
    retained = update["runtime_files_retained"]
    artifacts = update["runtime_artifacts"]
    before, after = retained["previous_native"], artifacts["native"]
    prior_expected = descriptor["previous_runtime_candidate_artifacts"]
    expected = descriptor["runtime_candidate_artifacts"]
    if (retained.get("format") != "mobkit-launch-runtime-retention-v1" or retained.get("retained") is not True
            or not _sha256(retained.get("inventory_sha256"))
            or retained.get("destination") != receipt["destination"] or retained.get("backup") != str(root / "runtime-before")
            or not _retained_product_proof(before, before.get("sdk"), authority["previous_runtime_candidate"], prior_expected, receipt["destination"])
            or not _retained_product_proof(after, artifacts["sdk"], authority["runtime_candidate"], expected, receipt["destination"])
            or before.get("platform") != after.get("platform")
            or not _same_toml(receipt.get("runtime_candidate"), authority["runtime_candidate"])):
        return None
    return _product_refresh_descriptor(previous, candidate, authority, sources=sources,
        expected=expected, prior_expected=prior_expected)


def _completed_runtime_dependency_transition(previous, candidate, receipt, update, root):
    """Read a closed crossing through its exact archived qualification proof.

    This proves historical package/activation provenance. It does not admit
    a worker, requalify old host source or infer current runtime readiness.
    """
    try:
        if (update.get("runtime_dependency_update") or {}).get("version") == 5:
            return _completed_product_runtime_transition(previous, candidate, receipt, update, root)
        authority = _runtime_dependency_authority(previous, candidate, receipt)
        if (authority is None or update.get("status") != "completed" or update.get("blockers") != []
                or update.get("rollback_available") is not False
                or update.get("runtime_admission_possible") is not True or authority.get("version") == 5):
            return None
        descriptor = update["runtime_dependency_update"]
        history_version = descriptor["version"]
        product = authority["kind"] == "selected-meta-toolkit-runtime-dependency-update"
        names = _runtime_framework_paths(candidate, history_version)
        if (history_version in {2, 3, 4} and not product and authority["to_version"] not in {"0.8.47", "0.8.48"}
                or not _archived_framework_provenance(candidate, names)):
            return None
        sources = (_archived_supervisor_sources(candidate, descriptor, history_version)
                   if history_version in {2, 3, 4} else None)
        if history_version in {2, 3, 4} and sources is None:
            return None
        old_report = _parsed_config(previous, "implementation-report.json")
        report = _parsed_config(candidate, "implementation-report.json")
        if (type(report.get("guide_version")) is not int or report["guide_version"] < 1
                or type(old_report.get("guide_version")) is not int or old_report["guide_version"] < 1
                or type(old_report.get("framework", {}).get("version")) is not int
                or not _sha256(report_brief_hash(report)) or not _sha256(report_brief_hash(old_report))
                or any(not isinstance(update.get(key), str) or not update[key] or len(update[key]) > 128
                       for key in ("source_digest", "previous_source_digest"))):
            return None
        bound = {key: report_brief_hash(report) if key == "brief_hash" else
                 update["package_sha256"] if key == "package_sha256" else report[key] for key in BOUND_KEYS}
        if (not _same_toml({key: receipt.get(key) for key in BOUND_KEYS}, bound)
                or not _same_toml([report["build_id"], report["guide_version"], report["blueprint_hash"],
                                  report_brief_hash(report), report["verification"]["digest"]],
                                 [update["to_build_id"], update["guide_version"], update["blueprint_hash"],
                                  update["brief_hash"], update["source_digest"]])
                or old_report["build_id"] != update["from_build_id"]
                or old_report["verification"]["digest"] != update["previous_source_digest"]
                or update.get("native_definition_update") is not authority["native_registry_update"]
                or type(update.get("host_instruction_update")) is not bool):
            return None
        previous_bound = {key: report_brief_hash(old_report) if key == "brief_hash" else
            update["previous_package_sha256"] if key == "package_sha256" else old_report[key] for key in BOUND_KEYS}
        if not _same_toml(json.loads((root / "previous-receipt.json").read_text()), previous_bound):
            return None
        if report["framework"].get("basis") == "runtime-refresh":
            binding = {"build_id": old_report["build_id"], "source_digest": old_report["verification"]["digest"]}
            if (not _same_toml(report["framework"].get("previous"), binding)
                    or not _same_toml(report["framework"].get("origin"), binding)
                    or any(previous.get(name) != candidate.get(name) for name in previous if name.startswith("application/"))
                    or any(name not in previous for name in candidate if name.startswith("application/"))):
                return None
        retained, artifacts = update["runtime_files_retained"], update["runtime_artifacts"]
        old_native, new_native = retained["previous_native"], artifacts["native"]
        from_version, to_version = authority["from_version"], authority["to_version"]
        prior_selection = authority.get("previous_runtime_candidate")
        prior_artifacts = descriptor.get("previous_runtime_candidate_artifacts")
        prior_valid = (_retained_candidate_proof(old_native, old_native.get("sdk"), prior_selection,
                prior_artifacts, receipt["destination"]) if product and prior_selection is not None
            else _retained_native_proof(old_native, from_version))
        if (retained.get("format") != "mobkit-launch-runtime-retention-v1"
                or retained.get("retained") is not True or not _sha256(retained.get("inventory_sha256"))
                or retained.get("destination") != receipt.get("destination")
                or retained.get("backup") != str(root / "runtime-before")
                or not prior_valid
                or old_native["platform"] != new_native["platform"]
                or (authority.get("runtime_candidate") is None and (
                    not _retained_native_proof(new_native, to_version)
                    or not _same_toml(artifacts["sdk"], {"package": "meerkat-mobkit", "version": to_version})))):
            return None
        selection = authority.get("runtime_candidate")
        expected_artifacts = None
        if selection is not None:
            descriptor = update["runtime_dependency_update"]
            expected_artifacts = descriptor.get("runtime_candidate_artifacts")
            if (not _same_toml(receipt.get("runtime_candidate"), selection)
                    or not _same_toml(descriptor.get("runtime_candidate"), selection)
                    or not (_retained_product_proof if product else _retained_candidate_proof)(
                        new_native, artifacts["sdk"], selection, expected_artifacts, receipt["destination"])):
                return None
        storage = update.get("runtime_storage_upgrade")
        if product and (storage is not None or update.get("native_definition_observation") is not None):
            return None
        if storage is not None and (type(storage) is not dict or type(storage.get("applied")) is not bool
                or to_version == "0.8.46" and storage.get("applied") is not True
                or (storage.get("from_version"), storage.get("to_version")) != (from_version, to_version)
                or "marker" in storage and not _same_toml(storage["marker"], {"format": 1,
                    "project": _parsed_config(candidate, "deployment.json")["slug"], "version": to_version, "phase": "ready"})):
            return None
        if authority["native_registry_update"]:
            pin = update["native_activation_expected_revision"]
            original = update["native_definition_observation"]
            final = update.get("native_definition_activation") or update.get("native_definition_recovery_observation")
            mob = _parsed_config(candidate, "deployment.json")["slug"]
            if (type(pin) is not int or pin < 1 or update["native_activation_package_sha256"] != update["package_sha256"]
                    or type(original) is not dict or type(final) is not dict):
                return None
            for observed, version, revision, package, native in (
                    (original, from_version, pin, update["previous_package_sha256"], old_native),
                    (final, to_version, pin + 1, update["package_sha256"], new_native)):
                if (not _same_toml(observed.get("definition_revision"), revision)
                        or observed.get("mob_id") != mob or observed.get("package_sha256") != package
                        or observed.get("native_binary_source") != "official-release"
                        or observed.get("native_binary_version") != version
                        or observed.get("native_binary_sha256") != native["binaries"]["rpc_gateway"]["binary_sha256"]):
                    return None
            if update.get("native_definition_activation") is not None and (
                    not _same_toml(final.get("expected_revision"), pin) or final.get("restored_source") is not False):
                return None
        elif selection is not None and any(update.get(key) is not None for key in (
                "native_activation_expected_revision", "native_activation_package_sha256",
                "native_definition_activation", "native_definition_recovery_observation")):
            return None  # This exact .47 crossing does not change a native definition.
        if selection is not None and update.get("native_definition_observation") is not None:
            observed = update["native_definition_observation"]
            package = observed.get("package_sha256") if type(observed) is dict else None
            native = old_native if package == update["previous_package_sha256"] else new_native
            if (type(observed) is not dict or type(observed.get("definition_revision")) is not int
                    or observed["definition_revision"] < 1
                    or package not in {update["previous_package_sha256"], update["package_sha256"]}
                    or observed.get("mob_id") != _parsed_config(candidate, "deployment.json")["slug"]
                    or observed.get("native_binary_source") != native["source"]
                    or observed.get("native_binary_version") != native["version"]
                    or observed.get("native_binary_sha256") != native["binaries"]["rpc_gateway"]["binary_sha256"]
                    or native["source"] == "signed-candidate" and (
                        not _same_toml(observed.get("candidate_binding"), selection["accepted"])
                        or observed.get("candidate_tuple_sha256") != selection["accepted"]["tuple_sha256"])):
                return None
        return {**authority, "version": history_version, "protected_paths": names,
            **({"required_supervisor_sources": sources} if history_version in {2, 3, 4} else {}),
            **({"runtime_candidate_artifacts": expected_artifacts} if selection is not None else {}),
            **({"previous_runtime_candidate_artifacts": prior_artifacts} if prior_selection is not None else {}),
            **({"accepted_baseline_digest": report["framework"]["baseline_digest"],
                "compiler_baseline_digest": report["framework"]["current_compiler_baseline_digest"]}
               if product or authority["to_version"] in {"0.8.47", "0.8.48"} else {})}
    except (OSError, KeyError, ValueError, TypeError, UnicodeError, RecursionError):
        return None


def compatibility(previous, candidate, receipt, *, runtime_target=None, reads=None):
    changes, blockers = [], []
    from .application_inventory_source import selects_application_inventory
    from .connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
    if selects_application_inventory({name: row[0] for name, row in previous.items()}):
        blockers.append(HELPERLESS_UPDATE_UNAVAILABLE)
    crossing = native_storage_crossing(previous, candidate)
    if crossing is not None:
        blockers.append(NATIVE_STORAGE_UPDATE_BLOCKER.format(*crossing))
    instruction_update = instruction_configuration_update(previous, candidate)
    current_host = (current_host_framework(candidate, runtime_target=runtime_target, reads=reads)
                    if runtime_target is not None else current_host_framework(candidate))
    dependency_update = runtime_dependency_transition(previous, candidate, receipt, runtime_target=runtime_target,
        reads=reads)
    maintained_update = maintained_host_transition(previous, candidate, receipt, runtime_target=runtime_target,
        reads=reads)
    for name in sorted(previous.keys() | candidate.keys()):
        if previous.get(name) == candidate.get(name):
            continue
        changes.append({"path": name, "change": "added" if name not in previous else "removed" if name not in candidate else "changed"})
        if name == APPLICATION_TOOL_POLICY_PATH:
            blockers.append(f"{name}: this update changes the agents' tool access and needs a verified permission-update path. Installed source and private data stay intact.")
            continue
        allowed = (name.startswith("application/") and name not in {
            "application/configuration.json", "application/console.json"}) or name.startswith(("acceptance-tests/", "tests/")) or name in {
            "README.md", "acceptance-report.json", "implementation-report.json", "SHA256SUMS"}
        if name == "config/console.toml":
            allowed = _console_presentation_only(previous.get(name), candidate.get(name))
        elif name in INSTRUCTION_CONFIG_PATHS:
            allowed = instruction_update and current_host
        elif name == "app.py":
            allowed = current_host and _framework_provenance(previous) and previous.get(name, (None, None))[1] == candidate[name][1]
        elif name == "native_definition_continuity.py":
            allowed = (current_host and _framework_provenance(previous)
                and candidate.get(name) == (Path(__file__).with_name(name).read_bytes(), 0o644)
                and (name not in previous or previous[name][1] == 0o644))
        if dependency_update and name in dependency_update["protected_paths"]:
            allowed = True
        if maintained_update and name in maintained_update["protected_paths"]:
            allowed = True
        if not allowed:
            blockers.append(f"{name} changes native configuration, dependencies or host source. Its migration needs a separate verified upgrade path.")
    return changes, blockers


def write_archive(path, payload):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


SETUP_REPLACEMENT_BLOCKER = ("This update changes the application's native definition, instructions, tool "
    "policy or runtime. After setup, Forge applies only updates that keep those unchanged. Keep the installed "
    "build, or install the new build into a new folder.")
SETUP_OWNER_BLOCKER = ("This update is not built from the installed application's own source, or it changes "
    "its plan, definition, protected settings or runtime. After setup, Forge applies only same-source updates "
    "that keep them. Keep the installed build, or install the new build into a new folder.")
SETUP_OWNER_RECONNECT = ("Forge has not reconnected this application's completed setup since it restarted. "
    "Use Continue setup once, then stage or apply the update again.")
ORDINARY_REPLACEMENT_BLOCKER = ("This update changes the application's native definition, instructions or "
    "runtime. Forge applies only updates that keep those unchanged to an application without a setup helper. "
    "Keep the installed build, or install the new build into a new folder.")
ORDINARY_OWNER_BLOCKER = ("This update is not built from the installed application's own source, or it changes "
    "its plan, definition, protected settings or runtime. Forge applies only same-source updates that keep them "
    "to an application without a setup helper. Keep the installed build, or install the new build into a new folder.")
FIRST_SETUP_UPDATE = ("This application has not finished its setup, so it cannot take a staged update in "
    "place yet. Its installed application is unchanged. Finish its setup, then stage the accepted update again.")
OWNER_RECONNECT_UPDATE = ("Forge has not reconnected this application since it restarted. Its installed "
    "application is unchanged. Use Continue setup once, then apply the update again.")


SETUP_RESTORE_NAME = "setup-restore.json"


def retain_setup_restore(root, restore_selection):
    """Keep the owner's exact Restore pin private to one update journal.

    The journal, and the receipt and status projections built from it, carry
    only the selected file's digest, never its private path.
    """
    pin = {"path": restore_selection["path"], "sha256": restore_selection["sha256"]}
    content = json.dumps(pin, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()
    write_archive(Path(root) / SETUP_RESTORE_NAME, content)
    return pin["sha256"]


def setup_restore_pin(directory, update):
    """The exact Restore pin recorded when this update was staged, or None."""
    if not isinstance(update, dict) or "setup_restore_sha256" not in update:
        return None
    from .deployment_application_bootstrap import SETUP_RESTORE_CHANGED
    from .connect_installer_caller import CustodyError, _json, _read
    try:
        pin = _json(_read(update_directory(directory, update["update_id"]) / SETUP_RESTORE_NAME, 64 * 1024))
    except (CustodyError, OSError, DraftConflict) as error:
        raise CustodyError(SETUP_RESTORE_CHANGED) from error
    if (not isinstance(pin, dict) or set(pin) != {"path", "sha256"}
            or pin["sha256"] != update["setup_restore_sha256"]):
        raise CustodyError(SETUP_RESTORE_CHANGED)
    return pin


def stage(directory, payload, report, *, runtime_target=None, restore_selection=None, setup_blockers=(),
        ordinary_owner=False):
    """Stage one accepted package. After setup, only through the owner's completed Restore.

    restore_selection is the pin the deployment service read from the trusted
    owner. An update that keeps the Restore's native inputs is staged with that
    pin; any other change after setup is blocked here. setup_blockers are the
    owner's own refusals of this candidate, which the deployment service read.
    ordinary_owner marks an application without a setup helper: its owner
    starts it, so only an update that keeps its native inputs is staged.
    """
    return publish_stage(directory, prepare_stage(directory, payload, report, runtime_target=runtime_target,
        restore_selection=restore_selection, setup_blockers=setup_blockers, ordinary_owner=ordinary_owner))


STAGE_CHANGED = "The installation changed while its update was being prepared. No update was staged; stage it again."


def _stage_binding(receipt):
    """The receipt facts a prepared Stage relies on: source, owner, runtime state and the staged update.

    Health, Console and log observations stay out, so a status read during the
    preparation does not refuse it; a changed runtime state or owner does.
    """
    return {**inspection_binding(receipt), **{key: receipt.get(key) for key in (*BOUND_KEYS, "target")},
        "update": receipt.get("update")}


def prepare_stage(directory, payload, report, *, runtime_target=None, restore_selection=None, setup_blockers=(),
        ordinary_owner=False):
    """Read and check one accepted package for staging, without writing anything.

    This is the long part of a Stage: it reads both packages, the installed
    source and every selected runtime, which can be a full native product. The
    deployment service runs it in a worker thread. Each selected runtime is
    read in full once. publish_stage rechecks the result and writes it.
    restore_selection, setup_blockers and ordinary_owner are as for stage().
    """
    require_replacement_history_settled(directory)
    from .deployment_application_bootstrap import require_update_bootstrap_history, setup_keeps_candidate
    restore = require_update_bootstrap_history(directory, restore_selection)
    receipt = read_receipt(directory)
    if "supervisor_refresh" in receipt:
        from .deployment_supervisor import require_complete
        require_complete(directory, receipt)
    if receipt.get("source_kind") == "native-team":
        raise DraftConflict("Agent-team source updates are not supported. No installed source was changed.")
    from .runtime_selection import MetaToolkitRuntimeTarget
    if type(runtime_target) is MetaToolkitRuntimeTarget:
        # The install worker refuses this too, but only after the update has
        # retained the current runtime. Nothing is staged here.
        from .deployment import selected_update_refusal
        refusal = selected_update_refusal(runtime_target)
        if refusal is not None:
            raise DraftConflict(refusal)
    previous_payload, previous = checked_package(directory, "package.zip", receipt["package_sha256"])
    candidate = package_files(payload)
    previous_report = json.loads(previous["implementation-report.json"][0])
    if any(previous_report.get(key) != receipt.get(key) for key in ("build_id", "guide_version", "blueprint_hash", "unresolved")):
        raise DraftConflict("The installed metadata no longer matches its exact previous package.")
    verify_destination(receipt, previous)
    reads = SelectedRuntimeReads(kept=KEPT_RUNTIME_READS)
    dependency = runtime_dependency_transition(previous, candidate, receipt, runtime_target=runtime_target, reads=reads)
    maintained = maintained_host_transition(previous, candidate, receipt, runtime_target=runtime_target, reads=reads)
    if maintained:
        if not _same_toml(report, _parsed_config(candidate, "implementation-report.json")):
            raise DraftConflict("The maintained host report differs from its exact accepted source package.")
        _require_maintained_host_configuration(receipt, {"maintained_host_update": maintained})
    if dependency:
        _require_runtime_framework_configuration(receipt, {"runtime_dependency_update": dependency})
    if dependency and receipt.get("execution_mode") == "background":
        from .deployment_service import verify_service, require_update_protocol, require_candidate_update_protocol
        service = verify_service(directory, receipt)
        require_update_protocol(service)
        require_candidate_update_protocol(service, {"runtime_dependency_update": dependency})
        if receipt.get("status") != "stopped" or receipt.get("service_cleanup_confirmed") is not True:
            raise DraftConflict("Stop this exact owned background service and confirm its native cleanup before staging a runtime refresh.")
    changes, blockers = compatibility(previous, candidate, receipt, runtime_target=runtime_target, reads=reads)
    dependency_update = runtime_dependency_transition(previous, candidate, receipt, runtime_target=runtime_target,
        reads=reads)
    native_definition_update = bool(instruction_configuration_update(previous, candidate)
        or dependency_update and dependency_update["native_registry_update"])
    host_instruction_update = bool(not maintained and has_host_instruction_carrier(candidate)
        and not has_host_instruction_carrier(previous))
    if restore is not None:
        blockers.extend(setup_blockers)
        if (native_definition_update or host_instruction_update or dependency_update
                or maintained or not setup_keeps_candidate(directory, restore, candidate)):
            blockers.append(SETUP_REPLACEMENT_BLOCKER)
    elif ordinary_owner:
        blockers.extend(setup_blockers)
        if native_definition_update or host_instruction_update or dependency_update or maintained:
            blockers.append(ORDINARY_REPLACEMENT_BLOCKER)
    transaction = {"update_id": str(uuid.uuid4()), "status": "blocked" if blockers else "staged",
        "from_build_id": receipt["build_id"], "to_build_id": report["build_id"],
        "guide_version": report["guide_version"], "blueprint_hash": report["blueprint_hash"],
        "brief_hash": report_brief_hash(report),
        "source_digest": report["verification"]["digest"], "package_sha256": digest(payload),
        "previous_package_sha256": receipt["package_sha256"],
        "previous_source_digest": previous_report["verification"]["digest"], "changes": changes,
        "blockers": blockers, "rollback_available": False, "runtime_admission_possible": False,
        "native_definition_update": native_definition_update,
        "runtime_dependency_update": dependency_update,
        "maintained_host_update": maintained,
        "host_instruction_update": host_instruction_update,
        "host_instruction_scope": "Canonical profile skills on ordinary trusted-host send and keyed dispatch turns only; Console, native flows, schedules and internal peer turns are outside this carrier.",
        "detail": ("Update prepared for review, but cannot be applied yet. Installed source and private data have not changed."
                   if blockers else "The exact accepted update is staged. Installed source and private data have not changed.")}
    return SimpleNamespace(receipt=deepcopy(receipt), restore_selection=deepcopy(restore_selection),
        restore=deepcopy(restore), previous_payload=previous_payload, payload=payload,
        previous_receipt={key: report_brief_hash(previous_report) if key == "brief_hash" else receipt[key]
            for key in BOUND_KEYS},
        transaction=transaction)


def publish_stage(directory, prepared):
    """Publish one prepared Stage after rechecking what it read.

    Only cheap reads and the update's own files happen here, so the deployment
    service runs it on the event loop with no await after its final source
    and design fences. A change since the preparation writes nothing.
    """
    require_replacement_history_settled(directory)
    from .deployment_application_bootstrap import require_update_bootstrap_history
    restore = require_update_bootstrap_history(directory, prepared.restore_selection)
    if not _same_toml(restore, prepared.restore) or not _same_toml(
            _stage_binding(read_receipt(directory)), _stage_binding(prepared.receipt)):
        raise DraftConflict(STAGE_CHANGED)
    transaction = deepcopy(prepared.transaction)
    root = update_directory(directory, transaction["update_id"])
    root.mkdir(mode=0o700, parents=True)
    write_archive(root / "previous.zip", prepared.previous_payload)
    write_archive(root / "candidate.zip", prepared.payload)
    write_private(root / "previous-receipt.json", prepared.previous_receipt)
    if restore is not None:
        # Apply and the worker re-verify this exact pin; the owner starts it.
        transaction["setup_restore_sha256"] = retain_setup_restore(root, prepared.restore_selection)
    write_private(root / "update.json", transaction)
    return update_receipt(directory, update=transaction)


def transaction(directory, expected_id, *, cleanup_update=None, expected_binding=None, expected_installation=None):
    """Check selected update authority, or the running owner's frozen cleanup source.

    Only an actual transaction worker supplies its previously checked update
    for final teardown. That read cannot select an old update or rewrite the
    receipt's newer staged pointer.
    """
    return _transaction(directory, expected_id, cleanup_update=cleanup_update,
                        expected_binding=expected_binding, expected_installation=expected_installation, reconcile=True)


def inspection_binding(receipt):
    """Small source/owner snapshot; service observations are presentation only."""
    from .deployment_service import service_binding
    service = receipt.get("service")
    if service is not None and not isinstance(service, dict):
        raise DraftConflict("The installed service binding could not be inspected.")
    update = receipt.get("update") or {}
    return {**{key: receipt.get(key) for key in INSPECTION_BINDING_KEYS},
        "service": service_binding(service) if service else None,
        "update": {key: update.get(key) for key in UPDATE_BINDING_KEYS},
        "source_carriers": {key: {"present": key in receipt, "value": receipt.get(key)}
            for key in ("runtime_candidate", "supervisor_refresh")}}


def require_inspection_current(directory, receipt, update=None):
    """Recheck selected authority after a read thread, without publishing it."""
    current = require_inspection_binding_current(directory, receipt, update)
    expected = update if update is not None else receipt.get("update") or {}
    _require_live_package(directory, {expected["package_sha256"], expected["previous_package_sha256"]}
        if expected else {current["package_sha256"]})
    return current


def require_inspection_binding_current(directory, receipt, update=None):
    """Recheck the small original receipt/journal join without archive reads.

    A caller using this alone retains its original full byte observation until
    the next effect. Ordinary inspection still uses require_inspection_current.
    """
    current = read_receipt(directory)
    if not _same_toml(inspection_binding(current), inspection_binding(receipt)):
        raise DraftConflict("The installed source or runtime owner changed during inspection. Check its current state again.")
    selected = current.get("update") or {}
    expected = update if update is not None else receipt.get("update") or {}
    if any(not _same_toml(selected.get(key), expected.get(key)) for key in UPDATE_BINDING_KEYS):
        raise DraftConflict("A different update is now selected. Review its staged package first.")
    if expected.get("update_id"):
        root = update_directory(directory, expected["update_id"])
        if not _same_toml(json.loads((root / "update.json").read_text()), expected):
            raise DraftConflict("The update checkpoint changed during inspection. Check its current state again.")
    return {**current, "update": expected} if expected else current


def inspect_transaction(directory, expected_id, *, expected_binding=None, expected_installation=None):
    """Check the selected transaction and return a detached journal projection."""
    receipt, root, update = _transaction(directory, expected_id,
                                        expected_binding=expected_binding, expected_installation=expected_installation, reconcile=False)
    return require_inspection_current(directory, receipt, update), root, update


def _transaction(directory, expected_id, *, cleanup_update=None, expected_binding=None, expected_installation=None, reconcile):
    receipt = read_receipt(directory)
    if expected_installation is not None:
        require_update_installation(receipt, expected_installation)
    if receipt.get("source_kind") == "native-team":
        raise DraftConflict("Agent-team source updates are not supported here. Keep this installation or choose a new empty destination.")
    update = cleanup_update if cleanup_update is not None else receipt.get("update") or {}
    if update.get("update_id") != expected_id:
        raise DraftConflict("A different update is now selected. Review its staged package first.")
    root = update_directory(directory, expected_id)
    actual = json.loads((root / "update.json").read_text())
    if expected_binding is not None:
        require_update_binding(update, expected_binding)
        require_update_binding(actual, expected_binding)
    if any(not _same_toml(actual.get(key), update.get(key)) for key in UPDATE_BINDING_KEYS):
        raise DraftConflict("The staged update journal changed. Preserve its files before continuing.")
    if actual.get("previous_source_digest") != update.get("previous_source_digest"):
        # A legacy archive checkpoint can add authenticated derived metadata
        # before its receipt projection is written. Only that one-way change
        # may reconcile, and only against the exact previously saved package.
        source = actual.get("previous_source_digest")
        if update.get("previous_source_digest") is not None or not isinstance(source, str) or not source or len(source) > 128:
            raise DraftConflict("The staged update's previous source provenance changed. Preserve its files before continuing.")
        _, previous = checked_package(root, "previous.zip", actual["previous_package_sha256"])
        report = json.loads(previous["implementation-report.json"][0])
        if report.get("build_id") != actual["from_build_id"] or report.get("verification", {}).get("digest") != source:
            raise DraftConflict("The staged update's derived previous source provenance does not match its exact saved package.")
    # update.json is the transaction authority. A host interruption may happen
    # before its newest checkpoint reaches the public receipt projection.
    if actual != update:
        if cleanup_update is None:
            receipt = update_receipt(directory, update=actual) if reconcile else {**receipt, "update": actual}
        update = actual
    _, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    if update.get("runtime_dependency_update") is not None:
        runtime_target = None
        descriptor = update["runtime_dependency_update"]
        if update.get("status") != "completed" and descriptor.get("runtime_candidate") is not None:
            from .runtime_selection import parse_candidate_selection
            try:
                runtime_target = parse_candidate_selection(descriptor["runtime_candidate"])
            except (ValueError, TypeError, KeyError):
                raise DraftConflict("The checked update's candidate target is invalid.") from None
        # Every inspection, including each status poll of a staged update,
        # re-derives the transition from the products' kept verified reads.
        transition = (_completed_runtime_dependency_transition(previous, candidate, receipt, update, root)
            if update.get("status") == "completed" else runtime_dependency_transition(previous, candidate, receipt,
                runtime_target=runtime_target, reads=SelectedRuntimeReads(kept=KEPT_RUNTIME_READS)))
        if not _same_toml(update["runtime_dependency_update"], transition):
            raise DraftConflict("The runtime dependency transition no longer matches its exact accepted packages and qualified host.")
    if update.get("maintained_host_update") is not None:
        from .runtime_selection import parse_candidate_selection
        try:
            runtime_target = parse_candidate_selection(update["maintained_host_update"]["runtime_candidate"])
        except (ValueError, TypeError, KeyError):
            raise DraftConflict("The checked maintained host target is invalid.") from None
        transition = (_completed_maintained_host_transition(previous, candidate, receipt, update, root)
            if update.get("status") == "completed" else
            maintained_host_transition(previous, candidate, receipt, runtime_target=runtime_target))
        if (not _same_toml(update["maintained_host_update"], transition)
                or update.get("runtime_dependency_update") is not None
                or update.get("native_definition_update") is not False or update.get("host_instruction_update") is not False
                or not _same_toml(transition and transition["previous"],
                    {"build_id": update["from_build_id"], "source_digest": update["previous_source_digest"]})
                or not _same_toml(transition and transition["candidate"],
                    {"build_id": update["to_build_id"], "source_digest": update["source_digest"]})):
            raise DraftConflict("The maintained host transition no longer matches its exact accepted packages and qualified host.")
        if update.get("status") != "completed":
            _require_maintained_host_configuration(receipt, update)
    _require_live_package(directory, {update["package_sha256"], update["previous_package_sha256"]})
    if expected_binding is not None:
        # An already running original worker can finish owned teardown after
        # the next update is staged. Its frozen cleanup selection authenticates
        # only this original journal/source and cannot select or rewrite the
        # newer receipt pointer.
        if cleanup_update is None:
            require_update_binding(read_receipt(directory).get("update") or {}, expected_binding)
        require_update_binding(json.loads((root / "update.json").read_text()), expected_binding)
    if expected_installation is not None:
        require_update_installation(read_receipt(directory), expected_installation)
    return receipt, root, update


def _require_live_package(directory, accepted_hashes):
    live = Path(directory) / "package.zip"
    if (live.is_symlink() or not live.is_file() or live.stat().st_nlink != 1 or live.stat().st_uid != os.getuid()
            or live.stat().st_size > 64 * 1024 * 1024
            or digest(live.read_bytes()) not in accepted_hashes):
        raise DraftConflict("The installed package journal changed outside this update. Preserve it before recovery.")


def checkpoint(directory, root, update, *, expected_binding=None, expected_installation=None, **changes):
    from .deployment import receipt_lock
    update = {**update, **changes}
    with receipt_lock(directory):
        receipt = read_receipt(directory)
        if expected_installation is not None:
            require_update_installation(receipt, expected_installation)
        if expected_binding is not None:
            require_update_binding(update, expected_binding)
            require_update_binding(receipt.get("update") or {}, expected_binding)
            require_update_binding(json.loads((root / "update.json").read_text()), expected_binding)
        write_private(root / "update.json", update)
        receipt.update(update=update, updated_at=now())
        write_private(Path(directory) / "receipt.json", receipt)
    return update


def retain_failed_service_attempt(directory, expected_id, context, *, error, logs, redact,
                                  service_instance_id=None, startup_attempt_id=None):
    """Retain historical supervisor output before another service resets logs.

    Only the update controller calls this. Output is diagnostic text, not a
    typed native failure, current health or completed application work.
    """
    receipt, root, update = transaction(directory, expected_id)
    if (context.get("build_id"), context.get("package_sha256")) != (
            receipt["build_id"], receipt["package_sha256"]):
        raise DraftConflict("The failed service start no longer matches this update's source. No diagnostic was relabelled.")
    for index, identity in enumerate((context.get("attempt_id"), service_instance_id, startup_attempt_id)):
        if index and identity is None:
            continue
        try:
            if str(uuid.UUID(identity)) != identity:
                raise ValueError
        except (ValueError, TypeError, AttributeError):
            raise DraftConflict("The failed service diagnostic lacks its exact owned attempt identity.") from None
    if ((service_instance_id is not None and service_instance_id != receipt.get("service_instance_id"))
            or (startup_attempt_id is not None and startup_attempt_id != receipt.get("startup_attempt_id"))):
        raise DraftConflict("The observed service attempt changed before retaining its diagnostics.")
    _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
    verify_destination(receipt, files)
    report = json.loads(files["implementation-report.json"][0])
    if report.get("build_id") != receipt["build_id"]:
        raise DraftConflict("The failed service diagnostic does not match its checked package.")
    source = report.get("verification", {}).get("digest")
    if not isinstance(source, str) or not source or len(source) > 128:
        raise DraftConflict("The failed service diagnostic lacks checked source provenance.")
    previous_source = update.get("previous_source_digest")
    if previous_source is None:
        # Older staged journals may lack this projection. Derive it from their
        # exact saved package, never from a path or a new implementation.
        _, previous_files = checked_package(root, "previous.zip", update["previous_package_sha256"])
        previous_report = json.loads(previous_files["implementation-report.json"][0])
        previous_source = previous_report.get("verification", {}).get("digest")
        if (previous_report.get("build_id") != update["from_build_id"]
                or not isinstance(previous_source, str) or not previous_source or len(previous_source) > 128):
            raise DraftConflict("The previous failed service source lacks its exact checked package provenance.")
    expected_source = update["source_digest"] if receipt["package_sha256"] == update["package_sha256"] else previous_source
    if source != expected_source:
        raise DraftConflict("The failed service source digest no longer matches this transaction.")
    output = []
    for item in logs[-120:]:
        if not isinstance(item, dict) or not isinstance(item.get("text"), str):
            continue
        text = redact(item["text"])[:4000]
        if text.strip():
            output.append({"at": redact(str(item.get("at", "")))[:64],
                           "phase": redact(str(item.get("phase", "")))[:80], "text": text})
    truncated = len(logs) > 120
    while output and len(json.dumps(output, indent=2).encode()) > MAX_ATTEMPT_LOG_BYTES:
        output.pop(0)
        truncated = True
    failure = {"attempt_id": context["attempt_id"], "update_id": expected_id,
               "build_id": receipt["build_id"], "package_sha256": receipt["package_sha256"],
               "source_digest": source,
               "source": "candidate" if receipt["package_sha256"] == update["package_sha256"] else "previous",
               "service_instance_id": service_instance_id, "startup_attempt_id": startup_attempt_id,
               "service_cleanup_confirmed": receipt.get("service_cleanup_confirmed") if service_instance_id else None,
               "captured_at": now(), "error_type": redact(type(error).__name__)[:80],
               "error": redact(str(error))[:1500] if isinstance(error, (ValueError, RuntimeError, TimeoutError))
                        else redact(type(error).__name__)[:80],
               "logs": output, "logs_truncated": truncated,
               "scope": "Historical redacted supervisor output during this failed source-bound service start. It does not establish current health or a typed native or application cause."}
    previous = update.get("failed_attempts", [])
    if not isinstance(previous, list) or any(not isinstance(item, dict) for item in previous):
        raise DraftConflict("This update's saved failure diagnostics need inspection before continuing.")
    bound_keys = ("update_id", "build_id", "package_sha256", "source_digest", "service_instance_id", "startup_attempt_id")
    if any(item.get("attempt_id") == context["attempt_id"]
           and any(item.get(key) != failure[key] for key in bound_keys) for item in previous):
        raise DraftConflict("This saved failed attempt belongs to another exact source or service instance. Its diagnostics were preserved.")
    records = [item for item in previous if item.get("attempt_id") != context["attempt_id"]]
    records.append(failure)
    removed = 0
    while (len(records) > MAX_FAILED_ATTEMPTS
           or len(json.dumps({"failed_attempts": records}, indent=2).encode()) > MAX_FAILED_ATTEMPT_BYTES):
        records.pop(0)
        removed += 1
    return checkpoint(directory, root, update, previous_source_digest=previous_source, failed_attempts=records,
                      failed_attempts_truncated=update.get("failed_attempts_truncated", 0) + removed)


def state_files(destination):
    result, total = {}, 0
    for name in STATE_ROOTS:
        root = destination / name
        if not root.exists() and not root.is_symlink():
            continue
        paths = [root, *root.rglob("*")] if root.is_dir() and not root.is_symlink() else [root]
        for path in paths:
            relative = str(path.relative_to(destination))
            if relative in STATE_EXCLUSIONS:
                continue
            info = path.lstat()
            if stat.S_ISDIR(info.st_mode):
                if info.st_uid != os.getuid():
                    raise DraftConflict("Local state contains an unowned directory. No update data was overwritten.")
                continue
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
                raise DraftConflict("Local state contains an unowned file or link. No update data was overwritten.")
            total += info.st_size
            if total > MAX_STATE_BYTES or len(result) >= MAX_STATE_FILES:
                raise DraftConflict("Local state exceeds the bounded update backup. Keep this deployment and use a separately reviewed migration.")
            result[relative] = path
    return result


def snapshot(destination, root, name):
    backup = root / name
    pending = root / (name + ".pending")
    # No installed bytes change until the complete private backup is published.
    # Interrupted copies are safe to replace only inside this owned journal.
    for path in (pending, backup):
        if path.exists() or path.is_symlink():
            state_files(path)
            if path.is_symlink() or path.stat().st_uid != os.getuid():
                raise DraftConflict("The private state backup is no longer owned.")
            for entry in path.rglob("*"):
                if entry.is_symlink() or entry.stat().st_uid != os.getuid() or (entry.is_file() and entry.stat().st_nlink != 1):
                    raise DraftConflict("The private state backup contains an unowned path.")
            shutil.rmtree(path)
    pending.mkdir(mode=0o700)
    manifest = {}
    for relative, source in state_files(destination).items():
        content = source.read_bytes()
        target = pending / relative
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        write_archive(target, content)
        manifest[relative] = {"sha256": digest(content), "mode": stat.S_IMODE(source.stat().st_mode)}
    write_private(pending / "snapshot.json", manifest)
    pending.replace(backup)


def restore_state(destination, root, name):
    manifest = json.loads((root / name / "snapshot.json").read_text())
    existing = state_files(destination)
    saved = state_files(root / name)
    if saved.keys() != manifest.keys():
        raise DraftConflict("The private state backup inventory changed. No restore was performed.")
    values = {}
    for relative, row in manifest.items():
        content = saved[relative].read_bytes()
        if digest(content) != row["sha256"]:
            raise DraftConflict("The private state backup changed. No restore was performed.")
        values[relative] = (content, row["mode"])
    for relative in existing.keys() - values.keys():
        existing[relative].unlink()
    for relative, (content, mode) in values.items():
        replace_file(destination / relative, content, mode)


def replace_file(path, content, mode):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if any(parent.is_symlink() for parent in [path, *path.parents]):
        raise DraftConflict("An update path was replaced by a link.")
    before = path.stat().st_mtime if path.exists() else 0
    pending = path.with_name(path.name + ".launch-update-" + uuid.uuid4().hex)
    descriptor = os.open(pending, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)
        os.fchmod(stream.fileno(), mode)
        stream.flush()
        os.fsync(stream.fileno())
    # A changed Python module must invalidate a same-size cached bytecode file.
    modified = max(time.time(), int(before) + 1)
    os.utime(pending, (modified, modified))
    pending.replace(path)


def verify_transition(receipt, previous, candidate):
    destination = owned_destination(receipt["destination"])
    marker = destination / OWNER_FILE
    report = json.loads(candidate["implementation-report.json"][0])
    old_report = json.loads(previous["implementation-report.json"][0])
    expected = [ownership({**receipt, "build_id": old_report["build_id"],
                           "package_sha256": receipt["update"]["previous_package_sha256"]}),
                ownership({**receipt, "build_id": report["build_id"],
                           "package_sha256": receipt["update"]["package_sha256"]})]
    if (marker.is_symlink() or marker.parent.is_symlink() or not marker.is_file()
            or marker.stat().st_uid != os.getuid() or marker.stat().st_nlink != 1
            or json.loads(marker.read_text()) not in expected):
        raise DraftConflict("The installed ownership receipt changed. No update source was overwritten.")
    for name in previous.keys() | candidate.keys():
        path = destination / name
        if any(parent.is_symlink() for parent in [path, *path.parents]):
            raise DraftConflict("An update source path was replaced by a link.")
        if not path.exists():
            if name in previous and name in candidate:
                raise DraftConflict("Owned update source is missing. Preserve this folder before recovery.")
            continue
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise DraftConflict("An update source file is not an owned ordinary file.")
        value = path.read_bytes()
        if value not in {entry[0] for entry in [previous.get(name), candidate.get(name)] if entry is not None}:
            raise DraftConflict("Installed source was modified outside this update. No modified file was overwritten.")
    return destination


def _require_policy_source_selection(directory, receipt, files, update):
    if update.get("update_id"):
        selected = receipt.get("update") or {}
        root = update_directory(directory, update["update_id"])
        actual = json.loads((root / "update.json").read_text())
        if any(not _same_toml(value.get(key), update.get(key))
                for value in (selected, actual) for key in UPDATE_BINDING_KEYS):
            raise DraftConflict("The selected update changed before source publication.")
        previous, _ = require_application_policy_continuity(directory, update)
    else:
        _, previous = checked_package(directory, "package.zip", receipt["package_sha256"])
    _require_unchanged_application_policy(previous, files)


def _source_receipt_candidate(directory, receipt, files, update, *, project_lease):
    """Validate source selection before replacement and at atomic publication."""
    from .runtime_selection import parse_candidate_selection
    _require_project_lease(receipt, project_lease)
    _require_policy_source_selection(directory, receipt, files, update)
    _require_maintained_host_configuration(receipt, update)
    _require_runtime_framework_configuration(receipt, update)
    selection = _candidate_source_selection(files)
    descriptor = update.get("runtime_dependency_update") or {}
    staged = descriptor.get("runtime_candidate")
    if staged is not None:
        if (receipt.get("package_sha256") == update.get("previous_package_sha256")
                and descriptor.get("previous_runtime_candidate") is not None
                and "runtime_candidate" not in receipt):
            raise DraftConflict("The predecessor lost its original admitted candidate receipt selection.")
        if "runtime_candidate" in receipt:
            if receipt.get("package_sha256") == update.get("previous_package_sha256"):
                prior = descriptor.get("previous_runtime_candidate")
                if (descriptor.get("kind") != "selected-meta-toolkit-runtime-dependency-update"
                        or prior is None):
                    raise DraftConflict("The ordinary predecessor has an unexpected candidate receipt selection.")
                if not _same_toml(receipt["runtime_candidate"], prior):
                    raise DraftConflict("The predecessor has no matching admitted candidate receipt selection.")
                root = update_directory(directory, update["update_id"])
                _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
                if not _same_toml(_candidate_source_selection(previous), prior):
                    raise DraftConflict("The predecessor candidate differs from its original admitted source.")
            elif not _same_toml(receipt["runtime_candidate"], staged):
                raise DraftConflict("The candidate source has no matching admitted host selection.")
        elif receipt.get("package_sha256") == update.get("package_sha256"):
            raise DraftConflict("The candidate source has no matching admitted host selection.")
    admitted = staged if staged is not None else receipt.get("runtime_candidate")
    receipt = dict(receipt)
    if selection is not None:
        if admitted is None or not _same_toml(admitted, selection):
            raise DraftConflict("The candidate source has no matching admitted host selection.")
        try:
            parse_candidate_selection(admitted)
        except (ValueError, KeyError, TypeError):
            raise DraftConflict("The admitted candidate source target is invalid.") from None
        receipt["runtime_candidate"] = admitted
    elif "runtime_candidate" in receipt:
        if (descriptor.get("runtime_candidate") is None
                or update.get("runtime_admission_possible") is not False
                or _parsed_config(files, "deployment.json").get("mobkitVersion") != descriptor.get("from_version")):
            raise DraftConflict("The admitted candidate runtime has no qualified downgrade path.")
        del receipt["runtime_candidate"]
    _require_project_lease(receipt, project_lease)
    return receipt


def _publish_source_receipt(directory, files, update, *, project_lease=None, expected_installation=None,
        require_owner_current=None, **changes):
    """Persist the admitted source target from checked host authority only."""
    from .deployment import receipt_lock
    with receipt_lock(directory):
        receipt = read_receipt(directory)
        if expected_installation is not None:
            require_update_installation(receipt, expected_installation)
        receipt = _source_receipt_candidate(directory, receipt, files, update, project_lease=project_lease)
        receipt.update(changes, updated_at=now())
        report = json.loads(files["implementation-report.json"][0])
        if "organization_hash" in report:
            receipt["organization_hash"] = report["organization_hash"]
        else:
            receipt.pop("organization_hash", None)
        project_lease.require_current()
        if require_owner_current is not None:
            require_owner_current()
        write_private(Path(directory) / "receipt.json", receipt)
        return receipt


def activate(directory, expected_id, *, expected_binding=None, expected_installation=None,
        require_owner_current=None):
    require_replacement_history_settled(directory)
    receipt, root, update = transaction(directory, expected_id, expected_binding=expected_binding, expected_installation=expected_installation)
    require_no_replacement_replay(root)
    if update["status"] not in {"staged", "snapshotting"} or update["blockers"]:
        raise DraftConflict("This staged package cannot be activated. Review its update requirements.")
    _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    _, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _require_unchanged_application_policy(previous, candidate)
    destination = verify_destination(receipt, previous)
    verify_transition(receipt, previous, candidate)
    # This is the same nonblocking OS lease held by the generated native host.
    with project_lock(destination) as lease:
        if require_owner_current is not None:
            require_owner_current()
        verify_destination(receipt, previous)
        verify_transition(receipt, previous, candidate)
        _source_receipt_candidate(directory, read_receipt(directory), candidate, update, project_lease=lease)
        for name in candidate.keys() - previous.keys():
            if (destination / name).exists():
                raise DraftConflict("New update source would overwrite an unowned file. Preserve it before updating.")
        update = checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="snapshotting", detail="Saving private settings and local native/application state before replacing any source.")
        snapshot(destination, root, "before-update")
        if require_owner_current is not None:
            require_owner_current()
        update = checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="applying", rollback_available=True)
        if expected_installation is not None:
            require_update_installation(read_receipt(directory), expected_installation)
        if expected_binding is not None:
            require_update_binding(read_receipt(directory).get("update") or {}, expected_binding)
            require_update_binding(json.loads((root / "update.json").read_text()), expected_binding)
        _source_receipt_candidate(directory, read_receipt(directory), candidate, update, project_lease=lease)
        if require_owner_current is not None:
            require_owner_current()
        apply_files(destination, previous, candidate)
        payload, _ = checked_package(root, "candidate.zip", update["package_sha256"])
        report = json.loads(candidate["implementation-report.json"][0])
        if require_owner_current is not None:
            require_owner_current()
        replace_file(Path(directory) / "package.zip", payload, 0o600)
        receipt = _publish_source_receipt(directory, candidate, update, **{key: report[key] for key in ("build_id", "guide_version", "blueprint_hash", "unresolved")},
                                 project_lease=lease, expected_installation=expected_installation,
                                 require_owner_current=require_owner_current,
                                 brief_hash=report_brief_hash(report), package_sha256=update["package_sha256"], bootstrap=None, console_url=None, runtime_pid=None)
        if require_owner_current is not None:
            require_owner_current()
        write_private(destination / OWNER_FILE, ownership(receipt))
        verify_destination(receipt, candidate)
        checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="verifying", detail="Updated bytes are installed. Native startup and actual Console health are not yet verified.")
    return candidate


def apply_files(destination, previous, candidate):
    for name in sorted(previous.keys() | candidate.keys()):
        path = destination / name
        if any(parent.is_symlink() for parent in [path, *path.parents]):
            raise DraftConflict("An update source path changed to a link during replacement.")
        if path.exists():
            info = path.stat()
            expected = {entry[0] for entry in [previous.get(name), candidate.get(name)] if entry is not None}
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                    or path.read_bytes() not in expected):
                raise DraftConflict("An update source file changed during replacement. Modified source was preserved.")
        elif name in previous and name in candidate:
            raise DraftConflict("Owned source disappeared during replacement. No missing source was silently recreated.")
        if name not in candidate:
            if path.exists():
                path.unlink()
        else:
            content, mode = candidate[name]
            # Apply declared mode changes without resetting unchanged local modes.
            if (not path.exists() or path.read_bytes() != content or
                    previous.get(name) != candidate[name] and stat.S_IMODE(path.stat().st_mode) != mode):
                replace_file(path, content, mode)


def restore(directory, expected_id, *, restore_data, rollback_kind=None, expected_binding=None, expected_installation=None):
    require_replacement_history_settled(directory)
    receipt, root, update = transaction(directory, expected_id, expected_binding=expected_binding, expected_installation=expected_installation)
    require_no_replacement_replay(root)
    if update.get("runtime_dependency_update") and update.get("runtime_admission_possible") is not False:
        raise DraftConflict("The upgraded runtime may have used retained native storage. Previous runtime files are preserved; recover the current accepted runtime instead of downgrading.")
    if restore_data and update.get("runtime_admission_possible") is not False:
        raise DraftConflict("Native startup may have admitted work. Preserve current native and application state; restore verified source only.")
    payload, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    _, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _require_unchanged_application_policy(previous, candidate)
    old_report = json.loads(previous["implementation-report.json"][0])
    bound = {key: old_report[key] for key in ("build_id", "guide_version", "blueprint_hash", "unresolved")}
    bound["brief_hash"] = report_brief_hash(old_report)
    bound["package_sha256"] = update["previous_package_sha256"]
    saved_bound = json.loads((root / "previous-receipt.json").read_text())
    if "brief_hash" not in saved_bound:
        saved_bound = {**saved_bound, "brief_hash": report_brief_hash(old_report)}
    if saved_bound != bound:
        raise DraftConflict("Previous deployment metadata changed. Its immutable package and private backup are retained.")
    destination = verify_transition(receipt, previous, candidate)
    with project_lock(destination) as lease:
        verify_transition(receipt, previous, candidate)
        _source_receipt_candidate(directory, read_receipt(directory), previous, update, project_lease=lease)
        update = checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="restoring", rollback_kind=rollback_kind or ("failed-update" if restore_data else "manual"), detail="Restoring the previous verified source. Its runtime health is not yet verified.")
        if restore_data:
            restore_state(destination, root, "before-update")
        elif not (root / "before-rollback").exists():
            snapshot(destination, root, "before-rollback")
        if expected_installation is not None:
            require_update_installation(read_receipt(directory), expected_installation)
        _source_receipt_candidate(directory, read_receipt(directory), previous, update, project_lease=lease)
        apply_files(destination, candidate, previous)
        replace_file(Path(directory) / "package.zip", payload, 0o600)
        receipt = _publish_source_receipt(directory, previous, update, project_lease=lease, expected_installation=expected_installation, **bound, bootstrap=None, console_url=None, runtime_pid=None)
        write_private(destination / OWNER_FILE, ownership(receipt))
        verify_destination(receipt, previous)
    return previous


def recover_candidate(directory, expected_id, *, expected_binding=None, expected_installation=None):
    require_replacement_history_settled(directory)
    receipt, root, update = transaction(directory, expected_id, expected_binding=expected_binding, expected_installation=expected_installation)
    require_no_replacement_replay(root)
    payload, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    _require_unchanged_application_policy(previous, candidate)
    destination = verify_transition(receipt, previous, candidate)
    with project_lock(destination) as lease:
        verify_transition(receipt, previous, candidate)
        _source_receipt_candidate(directory, read_receipt(directory), candidate, update, project_lease=lease)
        if update.get("runtime_admission_possible") is False:
            restore_state(destination, root, "before-rollback")
        if expected_installation is not None:
            require_update_installation(read_receipt(directory), expected_installation)
        _source_receipt_candidate(directory, read_receipt(directory), candidate, update, project_lease=lease)
        apply_files(destination, previous, candidate)
        report = json.loads(candidate["implementation-report.json"][0])
        replace_file(Path(directory) / "package.zip", payload, 0o600)
        receipt = _publish_source_receipt(directory, candidate, update, **{key: report[key] for key in ("build_id", "guide_version", "blueprint_hash", "unresolved")},
                                 project_lease=lease, expected_installation=expected_installation, brief_hash=report_brief_hash(report), package_sha256=update["package_sha256"], bootstrap=None, console_url=None, runtime_pid=None)
        write_private(destination / OWNER_FILE, ownership(receipt))
        verify_destination(receipt, candidate)
        checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="recovering-candidate", rollback_available=True,
                   detail="The attempted rollback failed. Updated source was restored while retaining current native and application state; runtime health still needs verification.")
    return candidate


def retry_candidate(directory, expected_id, *, expected_binding=None, expected_installation=None):
    require_replacement_history_settled(directory)
    """Retry the exact candidate against retained state without restoring data."""
    receipt, root, update = transaction(directory, expected_id, expected_binding=expected_binding, expected_installation=expected_installation)
    require_no_replacement_replay(root)
    if update["status"] not in CURRENT_RECOVERY_STATES or update["blockers"]:
        raise DraftConflict("This transaction is not eligible for current-application recovery.")
    payload, candidate = checked_package(root, "candidate.zip", update["package_sha256"])
    _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
    _require_unchanged_application_policy(previous, candidate)
    runtime_target = None
    if (update.get("runtime_dependency_update") or {}).get("runtime_candidate") is not None:
        from .runtime_selection import parse_candidate_selection
        runtime_target = parse_candidate_selection(update["runtime_dependency_update"]["runtime_candidate"])
    elif update.get("maintained_host_update") is not None:
        from .runtime_selection import parse_candidate_selection
        runtime_target = parse_candidate_selection(update["maintained_host_update"]["runtime_candidate"])
    _, blockers = compatibility(previous, candidate, receipt, runtime_target=runtime_target)
    if blockers:
        raise DraftConflict("This recovery needs a native, host, dependency or private-configuration migration.")
    report = json.loads(candidate["implementation-report.json"][0])
    if (report.get("build_id") != update["to_build_id"] or report.get("guide_version") != update["guide_version"]
            or report.get("blueprint_hash") != update["blueprint_hash"]
            or report.get("verification", {}).get("digest") != update["source_digest"]
            or update.get("brief_hash") is not None and report_brief_hash(report) != update["brief_hash"]):
        raise DraftConflict("The candidate metadata no longer matches the exact recovery transaction.")
    destination = verify_transition(receipt, previous, candidate)
    with project_lock(destination) as lease:
        verify_transition(receipt, previous, candidate)
        _source_receipt_candidate(directory, read_receipt(directory), candidate, update, project_lease=lease)
        state_files(destination)  # Refuse unowned/linked private state; never copy or rewind it.
        update = checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="recovering-current", runtime_admission_possible=True,
            detail="Retrying the exact accepted application using retained private settings and latest native and application records. Runtime health is not yet verified.")
        if expected_installation is not None:
            require_update_installation(read_receipt(directory), expected_installation)
        _source_receipt_candidate(directory, read_receipt(directory), candidate, update, project_lease=lease)
        apply_files(destination, previous, candidate)
        replace_file(Path(directory) / "package.zip", payload, 0o600)
        receipt = _publish_source_receipt(directory, candidate, update, **{key: report[key] for key in ("build_id", "guide_version", "blueprint_hash", "unresolved")},
                                 project_lease=lease, expected_installation=expected_installation, brief_hash=report_brief_hash(report), package_sha256=update["package_sha256"], bootstrap=None, console_url=None, runtime_pid=None,
                                 health="unknown", process_live=False)
        write_private(destination / OWNER_FILE, ownership(receipt))
        verify_destination(receipt, candidate)
    return candidate


def committed(directory, expected_id, *, restored=False, started=True, setup=True, expected_binding=None,
        expected_installation=None):
    """Settle a transaction. started=False: the owner's next Start runs this source.

    setup=False marks an application without a setup helper: its next Start
    runs the source with its owner's ordinary startup inputs.
    """
    _, root, update = transaction(directory, expected_id, expected_binding=expected_binding, expected_installation=expected_installation)
    rollback_available = not restored
    if started:
        detail = ("The previous verified source restarted and passed native bootstrap and live Console health." if restored
                  else "The accepted update passed native bootstrap and live Console health. External workflows still need their own verification.")
    else:
        detail = (("The previous source is restored and passed its application contract check." if restored
                   else "The accepted update is installed and passed its application contract check.")
                  + (" The application is stopped. Start restores its completed setup with this source." if setup
                     else " The application is stopped. Its next Start runs this source."))
    if (root / "application-replacement-attempt.json").exists():
        rollback_available = False
        detail += " The original replacement selection is retained. Native policy acceptance, definition commit and member adoption require their own exact receipts before a restart or rollback."
    if rollback_available and update.get("runtime_dependency_update"):
        rollback_available = False
        detail += " Previous runtime files and packages are retained. This storage crossing supports recovery with the current accepted runtime; a downgrade has not been qualified."
    if rollback_available and update.get("native_definition_update") is True and update.get("runtime_admission_possible") is not False:
        _, previous = checked_package(root, "previous.zip", update["previous_package_sha256"])
        if not has_host_instruction_carrier(previous):
            rollback_available = False
            detail += " Previous source and history are retained, but its legacy host cannot restore continuing role instructions. Forward recovery remains available."
    if not started and rollback_available:
        detail += " If that Start fails, Restore previous application restores the previous source."
    return checkpoint(directory, root, update, expected_binding=expected_binding, expected_installation=expected_installation, status="rolled-back" if restored else "completed",
        rollback_available=rollback_available, detail=detail)
