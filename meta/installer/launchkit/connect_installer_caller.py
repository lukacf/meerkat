"""Trusted caller for the received native application installer CLI.

This module owns byte custody and a one-shot application attempt record. It does
not compile policy, publish an activation, create a host or grant permission.
The existing stopped-host installer owner supplies its executor and currentness
check. There is deliberately no subprocess launcher or retry implementation.
"""
from __future__ import annotations

import asyncio
import base64
from collections.abc import Sequence
from dataclasses import dataclass, field
import hashlib
import json
import os
from pathlib import Path
import re
import stat
from typing import Awaitable, Callable

from .connect_policy_input import (
    _compiler_input, _text, application_tool_policy_input, application_tool_policy_selection,
)


MAX_INPUT_BYTES = 16 * 1024 * 1024
MAX_RECEIPT_BYTES = 1024 * 1024
MAX_BINARY_BYTES = 512 * 1024 * 1024
MAX_EXECUTABLE_BYTES = 1024 * 1024 * 1024
VERIFY_CHUNK_BYTES = 1024 * 1024
MAX_MEMBER_SURFACE_BYTES = 256 * 1024
MAX_INVENTORY_BYTES = 256 * 1024
INVENTORY_COMMAND = "--lower-application-inventory"
INVENTORY_PHASES = {"arguments", "validation", "receipt"}
APPLICATION_UPDATE_COMMAND = "--validate-application-update"
APPLICATION_UPDATE_PHASES = {"arguments", "validation", "receipt"}
PHASES = {"arguments", "validation", "owner_initialization", "artifact_write", "candidate_write", "receipt"}
SETUP_PHASES = PHASES | {"resource_preparation", "bundle_write"}
CAPABILITY_COMMANDS = {
    "--configure-agent-setup-capability": ("agent_setup_capability_configured", "setup_capability"),
    "--configure-helper-report-capability": ("helper_report_capability_configured", "report_capability"),
}
CAPABILITY_PHASES = PHASES | {"native_parent_issuance"}


HELPERLESS_UPDATE_UNAVAILABLE = (
    "In-place updates are not yet supported for this Connect application without a setup helper. "
    "Keep its installed build and use the original owner's Start to resume it.")


class CustodyError(ValueError):
    """An exact owned file or attempt no longer matches the trusted caller."""


async def _joined_filesystem(function, *args, **kwargs):
    """Join this exact worker before releasing its caller's custody."""
    task = asyncio.create_task(asyncio.to_thread(function, *args, **kwargs))
    cancelled = False
    while True:
        try:
            return await asyncio.shield(task), cancelled
        except asyncio.CancelledError:
            if task.cancelled():
                raise
            cancelled = True
        except BaseException:
            if cancelled:
                raise asyncio.CancelledError
            raise


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise CustodyError("A retained JSON object contains duplicate fields.")
        result[key] = value
    return result


def _json(content):
    try:
        return json.loads(content, object_pairs_hook=_object,
                          parse_constant=lambda _: _invalid_constant())
    except (UnicodeError, json.JSONDecodeError) as error:
        raise CustodyError("Retained JSON bytes are invalid.") from error


def _invalid_constant():
    raise CustodyError("Retained JSON contains a non-JSON constant.")


def _same_json_value(left, right):
    # JSON value custody must not coerce a Boolean into a number. Native
    # serialization may reorder objects or change whitespace, not value types.
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(
            _same_json_value(value, right[key]) for key, value in left.items())
    if isinstance(left, list):
        return len(left) == len(right) and all(
            _same_json_value(a, b) for a, b in zip(left, right))
    return left == right


def _same_member_classes(left, right):
    """Compare only the two defaults omitted by CompiledMemberClass's serde.

    Ordinary purpose and an absent callback account serialize without a field.
    DeclaredNone is a real account declaration and is never equivalent to null.
    Preserve source bytes, declaration order and every other value exactly.
    """
    def canonical(value):
        if not isinstance(value, list) or any(not isinstance(row, dict) for row in value):
            raise CustodyError("Native member classes require their selected declaration list.")
        return [{key: item for key, item in row.items()
            if not (key == "purpose" and item == "ordinary"
                    or key == "callback_account" and item is None)} for row in value]
    return _same_json_value(canonical(left), canonical(right))


def _selected_service_parent_administration(value, *, routes, remote_operator):
    """Check the existing native selection shape, never administrative authority.

    The selected administrative route IDs are a BTreeSet. Compare the IDs as
    a set without changing the pinned draft or inventing a route or operator.
    """
    if value is None:
        return None
    if (type(value) is not dict or set(value) != {"kind", "routes"}
            or value["kind"] != "application_policy_tasks"
            or type(value["routes"]) is not list or not value["routes"]
            or type(routes) is not list or remote_operator is None):
        raise CustodyError("Service-parent administration requires its exact selected native scope and operator.")
    selected = value["routes"]
    available = {row.get("id") for row in routes if isinstance(row, dict) and isinstance(row.get("id"), str)}
    if any(type(name) is not str or len(name) > 192
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/:@-]*", name)
            or any(part in {"", ".", ".."} for part in name.split("/"))
            or name not in available for name in selected):
        raise CustodyError("Service-parent administration requires exact IDs from the original native routes.")
    return {"kind": value["kind"], "routes": sorted(set(selected))}


def _selected_standing_policy(value, *, helper_tools):
    """Validate only the existing finite application-owner policy choices."""
    if value is None:
        return
    if (type(value) is not dict or set(value) != {"installation", "access"}
            or type(value["installation"]) is not str
            or value["installation"] not in {"human_review", "checked_software"}
            or type(value["access"]) is not str
            or value["access"] not in {"human_review", "configured_parent", "provision_task_parent"}
            or helper_tools != "checked_installation"):
        raise CustodyError("Standing acquisition policy requires exact native choices and the selected checked setup surface.")


def _path(path: Path, *, fresh=False):
    path = Path(path)
    if not path.is_absolute() or path.name in {"", ".", ".."}:
        raise CustodyError("A caller path must be absolute.")
    if any(parent.is_symlink() for parent in (path, *path.parents)):
        raise CustodyError("A caller path contains a symlink.")
    if path.parent.resolve(strict=True) / path.name != path:
        raise CustodyError("A caller path must use its canonical parent.")
    if fresh and os.path.lexists(path):
        raise CustodyError("A caller output or attempt must be fresh.")
    return path


def _private_parent(path):
    info = path.parent.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise CustodyError("The caller's existing parent directory is not private and owned.")


def _read(path, limit, *, executable=False):
    path = _path(path)
    # Refuse a FIFO at fstat without waiting for a writer at open. A path can
    # also change to a FIFO after canonical-path validation.
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or before.st_mode & 0o077
                or not 0 < before.st_size <= limit
                or executable and not before.st_mode & stat.S_IXUSR):
            raise CustodyError("The retained caller file is not private, owned and bounded.")
        content = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
        current = path.lstat()
        if (len(content) > limit or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
                or (current.st_dev, current.st_ino) != (after.st_dev, after.st_ino)):
            raise CustodyError("The retained caller file changed during observation.")
        return content


def _verify(path, digest, limit, *, executable=False):
    """Hash an owned executable without retaining its entire debug image."""
    path = _path(path)
    # The file-type check must run even if the selected path became a FIFO.
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or before.st_mode & 0o077
                or not 0 < before.st_size <= limit
                or executable and not before.st_mode & stat.S_IXUSR):
            raise CustodyError("The retained caller file is not private, owned and bounded.")
        observed, size = hashlib.sha256(), 0
        while chunk := stream.read(min(VERIFY_CHUNK_BYTES, limit - size + 1)):
            size += len(chunk)
            if size > limit:
                raise CustodyError("The retained caller file exceeds its verification bound.")
            observed.update(chunk)
        after = os.fstat(stream.fileno())
        current = path.lstat()

        def identity(info):
            return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
                    info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)

        if size != before.st_size or identity(before) != identity(after) or identity(current) != identity(after):
            raise CustodyError("The retained caller file changed during verification.")
        if observed.hexdigest() != digest:
            raise CustodyError("The retained caller file no longer matches its selected bytes.")


@dataclass(frozen=True)
class ExactFile:
    """Trusted existing-owner byte pin, never native semantic qualification."""

    path: Path
    sha256: str

    def read(self, *, executable=False):
        if not re.fullmatch(r"[0-9a-f]{64}", self.sha256):
            raise CustodyError("An exact caller file needs its retained SHA-256.")
        content = _read(self.path, MAX_BINARY_BYTES if executable else MAX_INPUT_BYTES,
                        executable=executable)
        if hashlib.sha256(content).hexdigest() != self.sha256:
            raise CustodyError("The retained caller file no longer matches its selected bytes.")
        return content

    def verify(self, *, executable=False):
        """Check the selected bytes without loading a gateway image into memory.

        Debug product gateways can exceed the legacy materialized read limit.
        Streaming verification retains a finite 1 GiB executable ceiling; input
        artifacts keep their existing bound. This establishes byte custody only.
        """
        if not re.fullmatch(r"[0-9a-f]{64}", self.sha256):
            raise CustodyError("An exact caller file needs its retained SHA-256.")
        _verify(self.path, self.sha256,
                MAX_EXECUTABLE_BYTES if executable else MAX_INPUT_BYTES,
                executable=executable)

    def pin(self):
        return {"path": str(self.path), "sha256": self.sha256}


@dataclass(frozen=True)
class ProductGatewayFile(ExactFile):
    """Existing gateway pin with exact selected product prefix byte custody.

    Its wire pin remains path/hash. The admitted update owner supplies the held
    producer selection; this wrapper creates no native or application authority.
    Producer modes are allowed only inside the original private enclosing root.
    """

    artifacts: object
    enclosing: tuple
    enclosing_mode: int
    entries: tuple

    @classmethod
    def observe(cls, pin, artifacts):
        from .runtime_candidates import MetaToolkitRuntimeArtifacts, recheck_meta_toolkit_artifacts
        from .templates.install_candidate import verify_meta_toolkit_prefix
        if type(artifacts) is not MetaToolkitRuntimeArtifacts or type(pin) is not ExactFile:
            raise CustodyError("Product gateway custody needs its original selected artifacts and exact pin.")
        from .native_prefix_receipt import known_fingerprints
        recheck_meta_toolkit_artifacts(artifacts, artifacts.selection)
        prefix = _path(pin.path).parent.parent
        enclosing = _private_directory(prefix.parent)
        known = known_fingerprints(artifacts, prefix)
        if (pin.path.name != "toolkit_gateway" or pin.path.parent.name != "bin"
                or pin.sha256 != artifacts.candidate["binary"]["sha256"]
                or verify_meta_toolkit_prefix(artifacts, deployment_root=prefix.parent,
                    prefix_path=prefix, **({"known": known} if known is not None else {})) != pin.path):
            raise CustodyError("The original gateway differs from its complete selected native prefix.")
        entries = tuple((prefix.parent / entry.name, cls._identity(prefix.parent / entry.name))
            for entry in artifacts.entries)
        result = cls(pin.path, pin.sha256, artifacts, enclosing, enclosing[0].stat().st_mode, entries)
        result.require_current()
        return result

    @staticmethod
    def _identity(path):
        info = path.lstat()
        return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
            info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)

    def require_current(self):
        from .runtime_candidates import recheck_meta_toolkit_artifacts
        recheck_meta_toolkit_artifacts(self.artifacts, self.artifacts.selection)
        if (_private_directory(self.enclosing[0]) != self.enclosing
                or self.enclosing[0].stat().st_mode != self.enclosing_mode
                or any(self._identity(path) != identity for path, identity in self.entries)):
            raise CustodyError("The original selected product prefix or private enclosing custody changed.")

    def verify(self, *, executable=False):
        from .native_prefix_receipt import known_fingerprints
        from .templates.install_candidate import verify_meta_toolkit_prefix
        if executable is not True:
            raise CustodyError("Product gateway custody is for its exact executable only.")
        self.require_current()
        known = known_fingerprints(self.artifacts, self.path.parent.parent)
        if verify_meta_toolkit_prefix(self.artifacts, deployment_root=self.enclosing[0],
                prefix_path=self.path.parent.parent, **({"known": known} if known is not None else {})) != self.path:
            raise CustodyError("The selected product gateway changed.")
        self.require_current()


@dataclass(frozen=True)
class ProcessOutcome:
    """Raw exited process boundary supplied by the existing installer owner."""

    returncode: int
    stdout: bytes
    stderr: bytes


def supervisor_executor(supervisor, *, timeout: float,
                        phase: str = "native application installer"):
    """Use the existing process owner without changing caller authority.

    The trusted install/update owner supplies its same Supervisor and command
    bound. Supervisor alone owns launch, private streams, stop and joined
    teardown. Exceptions remain unknown to the caller; this adapter never
    retries, selects a binary, checks a lease or interprets a receipt.
    """

    async def execute(argv: tuple[str, ...]) -> ProcessOutcome:
        raw = await supervisor.command(argv, phase, timeout, raw_outcome=True)
        return ProcessOutcome(**raw)

    return execute


@dataclass(frozen=True)
class InstallerOutcome:
    """Historical CLI observation; verified bytes do not represent readiness."""

    status: str
    receipt: dict | None
    artifact: ExactFile | None
    attempt_path: Path
    selection: dict | None = None
    setup_bundle: PreparedSetupBundle | None = None
    inventory: NativeApplicationInventory | None = None


@dataclass(frozen=True)
class NativeApplicationInventory:
    """Retained same-owner inventory observation, without native authority.

    The inventory caller attaches this carrier only to its successful result.
    It rechecks the original command, exited process, exact sources and owner;
    it never reruns inventory or confers routes, capabilities or permission.
    """

    _caller: NativeInstallerCaller = field(repr=False, compare=False)
    gateway: ExactFile
    compiler_input: ExactFile
    source_files: tuple[ExactFile, ...]
    attempt: ExactFile
    completion: ExactFile

    @property
    def originals(self):
        return (self.gateway, self.compiler_input, *self.source_files, self.attempt, self.completion)

    def authenticate(self):
        self._caller.require_current()
        result = self._authenticate_files()
        self._caller.require_current()
        return result

    def _authenticate_files(self):
        """Only filesystem/projection work; the caller owns loop-side checks."""
        if self.gateway != self._caller.gateway:
            raise CustodyError("The native inventory changed its selected gateway.")
        contents = []
        for index, pin in enumerate(self.originals):
            if index != 0 or type(pin) is not ProductGatewayFile:
                _private_parent(_path(pin.path))
            if index == 0:
                pin.verify(executable=True)
            else:
                contents.append(pin.read())
        source = _inventory_input(contents[0])
        entered, completed = _json(contents[-2]), _json(contents[-1])
        argv = [str(self.gateway.path), INVENTORY_COMMAND, "--input", str(self.compiler_input.path)]
        if not _same_json_value(entered, {"status": "entered", "argv": argv,
                "input_files": [pin.pin() for pin in (self.gateway, self.compiler_input, *self.source_files)],
                "output_path": None}):
            raise CustodyError("The retained native inventory changed its original command or input pins.")
        if (not isinstance(completed, dict) or set(completed) != {
                "status", "receipt", "artifact", "process", "error_type"}
                or completed["status"] != "receipt_verified" or completed["artifact"] is not None
                or completed["error_type"] is not None or not isinstance(completed["process"], dict)
                or set(completed["process"]) != {"returncode", "stdout_base64", "stderr_base64"}):
            raise CustodyError("The retained inventory is not its successful original exited observation.")
        raw = completed["process"]
        try:
            process = ProcessOutcome(raw["returncode"],
                base64.b64decode(raw["stdout_base64"], validate=True),
                base64.b64decode(raw["stderr_base64"], validate=True))
        except (ValueError, TypeError) as error:
            raise CustodyError("The retained inventory lost its exact process bytes.") from error
        status, receipt = _receipt(process, INVENTORY_COMMAND, None, inventory_input=source)
        if status != "receipt_verified" or not _same_json_value(receipt, completed["receipt"]):
            raise CustodyError("The native inventory no longer matches its original process receipt.")
        # Recheck after the process/row join, before returning these copied facts.
        for index, pin in enumerate(self.originals):
            if index != 0 or type(pin) is not ProductGatewayFile:
                _private_parent(_path(pin.path))
            pin.verify(executable=index == 0)
        return source, receipt


def _selection(content):
    return _host_selection(_json(content))


def _host_selection(selected):
    if (not isinstance(selected, dict) or set(selected) != {
            "deployment_root", "activation", "policy_manifest", "local_policy"}
            or not isinstance(selected["deployment_root"], str)):
        raise CustodyError("The retained selection does not have the received HostBootstrapSelection fields.")
    artifacts = []
    for name in ("activation", "policy_manifest", "local_policy"):
        value = selected[name]
        if (not isinstance(value, dict) or set(value) != {"path", "sha256"}
                or not isinstance(value["path"], str) or not isinstance(value["sha256"], str)):
            raise CustodyError("The retained selection lacks an exact selected artifact pin.")
        artifacts.append(ExactFile(Path(value["path"]), value["sha256"]))
    # This is only byte custody for the received typed projection. The native
    # owner still validates manifest membership, policy semantics and roots.
    return selected, tuple(artifacts)


def _application_update_selection(content):
    selected = _json(content)
    if (not isinstance(selected, dict) or set(selected) != {
            "update_id", "predecessor", "compilation_input", "application_tool_policy"}
            or not isinstance(selected["update_id"], str) or not selected["update_id"]):
        raise CustodyError("The retained update selection lacks its exact four fields and correlation.")
    _, predecessor = _host_selection(selected["predecessor"])
    artifacts = tuple(_artifact_pin(selected[name])
                      for name in ("compilation_input", "application_tool_policy"))
    return selected, (*predecessor, *artifacts)


def _private_directory(path):
    path = _path(Path(path))
    info = path.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise CustodyError("A preparation directory must be private and owned.")
    return path, (info.st_dev, info.st_ino)


def _artifact_pin(value):
    if (not isinstance(value, dict) or set(value) != {"path", "sha256"}
            or not isinstance(value["path"], str) or not isinstance(value["sha256"], str)):
        raise CustodyError("The setup receipt lacks an exact artifact pin.")
    return ExactFile(Path(value["path"]), value["sha256"])


@dataclass(frozen=True)
class PreparedSetupBundle:
    """Pinned native compiler input, without interpreting or projecting it.

    The application owner retains these pins in its activation/source closure
    and authenticates them before every use. Native helper_projection and
    project_roster remain the sole owners of definition and live roster meaning.
    This historical bundle is not a readiness receipt or execution permission.
    """

    directory: tuple
    resources_directory: tuple
    manifest: ExactFile
    setup: ExactFile
    application_tool_policy: ExactFile
    resource_handoff: ExactFile
    setup_input: ExactFile
    helper_source_surface: ExactFile
    helper_initial_surface: ExactFile

    def pins(self):
        return {"directory": {"path": str(self.directory[0]),
                "device": self.directory[1][0], "inode": self.directory[1][1]},
            "resources_directory": {"path": str(self.resources_directory[0]),
                "device": self.resources_directory[1][0], "inode": self.resources_directory[1][1]},
            "manifest": self.manifest.pin(), "setup": self.setup.pin(),
            "application_tool_policy": self.application_tool_policy.pin(),
            "resource_handoff": self.resource_handoff.pin(),
            "setup_input": self.setup_input.pin(),
            "helper_source_surface": self.helper_source_surface.pin(),
            "helper_initial_surface": self.helper_initial_surface.pin()}

    def authenticate(self):
        """Return exact setup/policy bytes after whole-bundle custody checks."""
        if (_private_directory(self.directory[0]) != self.directory
                or _private_directory(self.resources_directory[0]) != self.resources_directory):
            raise CustodyError("The retained setup or resource directory changed.")
        expected = {"manifest.json", "application-setup.json", "application-tool-policy.json",
                    "helper-initial-surface.json"}
        if {path.name for path in self.directory[0].iterdir()} != expected:
            raise CustodyError("The setup bundle does not contain its exact artifact closure.")
        manifest_bytes = self.manifest.read()
        setup_bytes = self.setup.read()
        policy_bytes = self.application_tool_policy.read()
        self.helper_source_surface.read()
        self.helper_initial_surface.read()
        self.resource_handoff.read()
        input_bytes = self.setup_input.read()
        manifest, setup, source = _json(manifest_bytes), _json(setup_bytes), _json(input_bytes)
        if (not isinstance(manifest, dict) or set(manifest) != {"schema", "kind", "artifacts"}
                or type(manifest["schema"]) is not int or manifest["schema"] != 1
                or manifest["kind"] != "application_setup_preparation"
                or not isinstance(manifest["artifacts"], list)):
            raise CustodyError("The setup manifest has an invalid custody shape.")
        observed = {}
        for row in manifest["artifacts"]:
            if (not isinstance(row, dict) or set(row) != {"path", "sha256"}
                    or not isinstance(row["path"], str) or row["path"] in observed):
                raise CustodyError("The setup manifest contains ambiguous artifacts.")
            observed[row["path"]] = row["sha256"]
        if observed != {self.setup.path.name: self.setup.sha256,
                        self.application_tool_policy.path.name: self.application_tool_policy.sha256,
                        self.helper_initial_surface.path.name: self.helper_initial_surface.sha256}:
            raise CustodyError("The setup manifest differs from the selected output artifacts.")
        if (not isinstance(setup, dict) or type(setup.get("schema")) is not int
                or setup["schema"] != 3 or not isinstance(setup.get("input_sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", setup["input_sha256"])
                or setup.get("application_tool_policy") != {
                    "name": self.application_tool_policy.path.name,
                    "sha256": self.application_tool_policy.sha256}
                or setup.get("resources") != {
                    "artifact_directory": str(self.resources_directory[0]),
                    "handoff": self.resource_handoff.pin()}):
            raise CustodyError("The setup bundle lacks its native input hash, policy or resource handoff.")
        # Native validate() hashes canonical_json(&ApplicationSetupInput) after
        # typed deserialization/default normalization, not the raw input file.
        # setup_input above independently pins exact original bytes. Python
        # must not rebuild native serialization or equate these two digests.
        if not isinstance(source, dict):
            raise CustodyError("The retained setup input is not an object.")
        if (type(source.get("schema")) is not int or source["schema"] != 3
                or not _same_json_value(source.get("helper_source_surface"), self.helper_source_surface.pin())
                or not _same_json_value(setup.get("helper_source_surface"), self.helper_source_surface.pin())):
            raise CustodyError("The setup bundle changed its selected source surface.")
        helper = setup.get("helper")
        members = setup.get("native_members")
        if (not isinstance(helper, dict) or not isinstance(helper.get("member"), dict)
                or not isinstance(members, list)):
            raise CustodyError("The prepared setup lacks its emitted surface binding.")
        references = [row.get("initial_surface") for row in members
                      if isinstance(row, dict) and _same_json_value(row.get("member"), helper["member"])]
        # This checks only an opaque reference association. Native owns member
        # selection, ToolIdentity reconstruction, visible subsets and grants.
        if (len(references) != 1 or not _same_json_value(references[0], {
                "name": self.helper_initial_surface.path.name,
                "sha256": self.helper_initial_surface.sha256})):
            raise CustodyError("The emitted surface differs from its prepared member reference.")
        definition_source = source.get("definition")
        if (isinstance(definition_source, dict) and definition_source.get("format") == "toml"
                and not _same_json_value(setup.get("definition_source"), definition_source)):
            raise CustodyError("The prepared setup changed its exact raw TOML source.")
        return setup_bytes, policy_bytes


def _completed_manifest(manifest):
    value = _json(manifest.read())
    if (not isinstance(value, dict) or set(value) != {"schema", "kind", "artifacts"}
            or type(value["schema"]) is not int or value["schema"] != 1
            or value["kind"] != "application_setup_preparation"
            or not isinstance(value["artifacts"], list)):
        raise CustodyError("The completed setup manifest has a different custody shape.")
    artifacts = {}
    for row in value["artifacts"]:
        if (not isinstance(row, dict) or set(row) != {"path", "sha256"}
                or not isinstance(row["path"], str) or not isinstance(row["sha256"], str)
                or row["path"] in artifacts
                or not re.fullmatch(r"[0-9a-f]{64}", row["sha256"])
                or row["path"] not in {"application-setup.json", "application-tool-policy.json"}
                and not re.fullmatch(r"member-surface-[0-9a-f]{64}\.json", row["path"])):
            raise CustodyError("The completed setup manifest has ambiguous artifact names or pins.")
        artifacts[row["path"]] = ExactFile(manifest.path.parent / row["path"], row["sha256"])
    if not {"application-setup.json", "application-tool-policy.json"} <= artifacts.keys():
        raise CustodyError("The completed setup manifest lacks setup or policy bytes.")
    return artifacts


def _completed_surface(pin):
    # Preserve full native ToolIdentity bytes, including provenance/catalog IDs.
    # Python neither decodes identities nor recreates native canonical hashes.
    content = _read(pin.path, MAX_MEMBER_SURFACE_BYTES)
    if (not isinstance(pin.sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", pin.sha256)
            or hashlib.sha256(content).hexdigest() != pin.sha256):
        raise CustodyError("The completed member surface changed its exact bytes.")
    return content


def _preparation_setup_targets(value, *, compiled=False):
    """Preserve finite explicit target data; native validates its typed scope."""
    if not isinstance(value, list):
        raise CustodyError("Explicit helper targets require a retained list.")
    fields = {"target", "parent", "restrictions"} if compiled else {"target", "issuance_id"}
    targets = []
    for row in value:
        if not isinstance(row, dict) or set(row) != fields:
            raise CustodyError("An explicit helper target changed its native fields.")
        target = row["target"]
        if target is not None and (not isinstance(target, dict)
                or not isinstance(target.get("kind"), str) or not target["kind"]
                or target["kind"].strip() != target["kind"]):
            raise CustodyError("An explicit helper target requires its exact native tag or null.")
        if any(_same_json_value(target, previous) for previous in targets):
            raise CustodyError("Explicit helper targets repeat a selected target.")
        if compiled and (row["parent"] is None or not isinstance(row["restrictions"], dict)):
            raise CustodyError("A completed helper target lacks its native parent or restrictions.")
        targets.append(target)
    return value


def _preparation_helper_catalog_request(request):
    """Read explicit host intent, never infer successor permission from reset."""
    if (not isinstance(request, dict) or not {"issuance_id"} <= request.keys()
            or request.keys() - {"issuance_id", "scope"}
            or not isinstance(request["issuance_id"], str) or not request["issuance_id"]
            or request["issuance_id"].strip() != request["issuance_id"]
            or request.get("scope", "exact_session") not in ("exact_session", "current_helper_successors")):
        raise CustodyError("The retained helper catalog request changed its native fields or scope.")
    return request.get("scope", "exact_session")


def _preparation_helper_catalog_basis(basis):
    if (not isinstance(basis, dict) or basis.get("kind") != "targeted_grant_lineages"
            or not {"kind", "targets"} <= basis.keys() or basis.keys() - {"kind", "base", "targets"}
            or basis.get("base") is not None or not isinstance(basis["targets"], list)):
        raise CustodyError("The helper catalog parent must stay separate from executable authority.")


def _preparation_helper_catalog_capability(value, basis):
    """Check the separate container; native owns parent bounds and coordinates.

    The successor parent has unrestricted resource domains. Only the native
    binder's child is pinned to the application/current SID. This container
    neither makes the parent executable nor promises revocation on retirement.
    """
    if value is None:
        return None
    if (not isinstance(value, dict) or set(value) != {"parent"}
            or not isinstance(value["parent"], list) or not value["parent"]
            or any(not isinstance(ref, dict) or not {"grant_id", "root_authority"} <= ref.keys()
                or not isinstance(ref["grant_id"], str) or not ref["grant_id"]
                or ref["grant_id"].strip() != ref["grant_id"] or ref["root_authority"] is None
                for ref in value["parent"])):
        raise CustodyError("The helper catalog parent must stay separate from executable authority.")
    _preparation_helper_catalog_basis(basis)
    return value


def _prepared_issuance_reference(reference, issuance_id, operator):
    # This checks an association only. All other returned grant coordinates are
    # opaque; canonical decoding, issuance and currentness belong to native.
    if (operator is None or not isinstance(reference, dict)
            or not {"grant_id", "root_authority"} <= reference.keys()
            or not _same_json_value(reference["grant_id"], issuance_id)
            or not _same_json_value(reference["root_authority"], operator)):
        raise CustodyError("The completed helper addition differs from its retained issuance request.")


def _prepared_helper_access_basis(early, completed, authorities, operator, server):
    """Join only requested native additions, without issuing or resolving grants.

    The authenticated preparation exchange owns the reference bytes and actual
    helper session. Existing bases and targets remain exact; the caller never
    infers a session, server, controller key or grant from configuration.
    """
    mcp, catalog = authorities.get("helper_mcp"), authorities.get("helper_catalog")
    if mcp is None and catalog is None and _same_json_value(early, completed):
        return early
    for basis in (early, completed):
        if (not isinstance(basis, dict) or not {"kind", "targets"} <= basis.keys()
                or set(basis) - {"kind", "base", "targets"}
                or basis["kind"] != "targeted_grant_lineages"
                or not isinstance(basis["targets"], list)):
            raise CustodyError("The completed helper changed its operation authority selection kind.")

    def requested_lineage(lineage, request):
        # Check only this reference's retained request association. Native owns
        # qualified principals, canonical coordinates, issuance and current use.
        if not isinstance(lineage, list) or len(lineage) != 1:
            raise CustodyError("The completed helper addition differs from its retained issuance request.")
        _prepared_issuance_reference(lineage[0], request["issuance_id"], operator)

    old_base, new_base = early.get("base"), completed.get("base")
    catalog_scope = _preparation_helper_catalog_request(catalog) if catalog is not None else None
    if catalog_scope == "current_helper_successors":
        if old_base is not None or new_base is not None:
            raise CustodyError("A helper successor catalog parent is not an executable base.")
    elif catalog is not None:
        requested_lineage(new_base, catalog)
    if not _same_json_value(old_base, new_base):
        if catalog is None or old_base is not None:
            raise CustodyError("The completed helper replaced or widened its selected base.")

    old, new = early["targets"], completed["targets"]
    if mcp is not None:
        name = server.get("name") if isinstance(server, dict) else None
        if not isinstance(name, str) or not name or name.strip() != name:
            raise CustodyError("The completed helper lacks its exact bundled server selection.")
        target = {"kind": "mcp_server", "server": name}
        selected = [row for row in new if isinstance(row, dict)
                    and _same_json_value(row.get("target"), target)]
        if len(selected) != 1 or set(selected[0]) != {"target", "lineage"}:
            raise CustodyError("The completed helper lacks its one requested bundled operation target.")
        requested_lineage(selected[0]["lineage"], mcp)
    if not _same_json_value(old, new):
        if (mcp is None or len(new) != len(old) + 1
                or any(isinstance(row, dict) and _same_json_value(row.get("target"), target) for row in old)
                or not _same_json_value([row for row in new if row is not selected[0]], old)):
            raise CustodyError("The completed helper replaced or widened an existing operation target.")
    return completed


def _prepared_plan_source(plan, setup):
    """Join opaque driver configuration, never normalize native authority.

    The same live authenticated exchange supplies this manifest. Its resolved
    input_sha256 remains a native fingerprint, not a hash of the original plan.
    Native projection, when a helper is selected, and final adoption must still
    validate these facts.
    """
    fields = {"schema", "deployment_root", "owner", "startup", "authorities",
              "application", "acquisition", "output", "hold_timeout_ms"}
    if (not isinstance(plan, dict) or not fields - {"acquisition"} <= plan.keys()
            or plan.keys() - fields
            or type(plan["schema"]) is not int or plan["schema"] != 1
            or not isinstance(plan["application"], dict) or not isinstance(setup, dict)):
        raise CustodyError("Completed preparation requires its original schema1 driver plan.")
    source = plan["application"]
    helper_fields = {"helper", "helper_tools", "helper_profile"}
    selected_helper_fields = helper_fields & source.keys()
    if selected_helper_fields and selected_helper_fields != helper_fields:
        raise CustodyError("Completed preparation requires all selected helper fields or none.")
    has_helper = bool(selected_helper_fields)
    if (("acquisition" in plan) != has_helper
            or has_helper and not isinstance(plan["acquisition"], dict)):
        raise CustodyError("Completed preparation requires acquisition only for its selected helper.")
    if not has_helper and setup.keys() & (helper_fields | {
            "source_helper_member", "source_helper_roster", "server", "resources", "helper_source_surface"}):
        raise CustodyError("Helperless completion cannot introduce helper output fields.")
    original, resolved = plan["owner"], setup.get("native_owner")
    owner_fields = {"deployment", "components", "channels", "native_host"}
    native_fields = {"realm", "grant_administrator", "remote_operator", "ingress_domain", "members", "routes"}
    native_optional = {"service_parent_administration", "member_classes"}
    if (not isinstance(original, dict) or set(original) != owner_fields
            or not isinstance(resolved, dict) or set(resolved) != owner_fields
            or not isinstance(original["native_host"], dict) or not native_fields <= original["native_host"].keys()
            or original["native_host"].keys() - native_fields - native_optional
            or not isinstance(resolved["native_host"], dict) or not native_fields <= resolved["native_host"].keys()
            or resolved["native_host"].keys() - native_fields - native_optional
            or not _same_json_value(original["native_host"]["members"], source.get("members"))):
        raise CustodyError("The completed preparation requires its exact resolved native owner.")
    before, after = original["native_host"], resolved["native_host"]
    classes = source.get("member_classes", [])
    if (not isinstance(classes, list) or any(not isinstance(row, dict) for row in classes)
            or not _same_member_classes(before.get("member_classes", []), classes)
            or not _same_member_classes(after.get("member_classes", []), classes)):
        raise CustodyError("The completed native owner changed its selected member classes.")
    administration = [_selected_service_parent_administration(value.get("service_parent_administration"),
        routes=value["routes"], remote_operator=value["remote_operator"]) for value in (before, after)]
    if not _same_json_value(*administration):
        raise CustodyError("The completed native owner changed its selected service-parent administration.")
    acquisition = plan.get("acquisition")
    if isinstance(acquisition, dict):
        _selected_standing_policy(acquisition.get("standing_policy"), helper_tools=source.get("helper_tools"))
    members = setup.get("native_members")
    if (not isinstance(members, list) or any(not isinstance(row, dict) for row in members)
            or not isinstance(setup.get("realm"), dict)
            or not _same_json_value(after["members"], [
                {key: value for key, value in row.items() if key != "initial_surface"} for row in members])
            or not _same_json_value(after["realm"], setup.get("realm", {}).get("realm_id"))
            or any(not _same_json_value(original[key], resolved[key]) for key in ("deployment", "components"))
            or any(not _same_json_value(before[key], after[key]) for key in native_fields - {"members", "routes"})):
        raise CustodyError("The completed native owner changed its selected configuration or finished members.")
    source_helper, helper = source.get("helper"), setup.get("helper")
    if has_helper and (not isinstance(source_helper, dict) or not isinstance(source_helper.get("member"), dict)
            or not isinstance(helper, dict) or not isinstance(helper.get("member"), dict)):
        raise CustodyError("The completed preparation lacks its source and projected helper.")
    def project(binding):
        return helper["member"] if has_helper and _same_json_value(binding, source_helper["member"]) else binding
    channels = original["channels"]
    if (not isinstance(channels, list) or any(not isinstance(row, dict) for row in channels)
            or not _same_json_value(resolved["channels"], [
                {**row, "member": project(row.get("member"))} for row in channels])):
        raise CustodyError("The completed preparation changed an original channel source fact.")
    authorities = plan["authorities"]
    allowed_authorities = ({"controllers", "agent_setup", "helper_report", "helper_mcp", "helper_catalog"}
                           if has_helper else {"controllers"})
    if (not isinstance(authorities, dict) or set(authorities) - allowed_authorities
            or not isinstance(authorities.get("controllers", []), list)
            or not isinstance(authorities.get("agent_setup", []), list)):
        raise CustodyError("The retained preparation has ambiguous authority request custody.")
    issuance_ids, controller_targets = set(), set()
    def issuance(value):
        if (not isinstance(value, str) or not value or value.strip() != value
                or value in issuance_ids):
            raise CustodyError("The retained preparation repeats or changes an issuance ID.")
        issuance_ids.add(value)
    for name in ("helper_mcp", "helper_catalog"):
        request = authorities.get(name)
        if request is None:
            continue
        if name == "helper_catalog":
            _preparation_helper_catalog_request(request)
        elif not isinstance(request, dict) or set(request) != {"issuance_id"}:
            raise CustodyError("The retained helper access request changed its native fields.")
        issuance(request["issuance_id"])
    for request in authorities.get("controllers", []):
        target = request.get("target") if isinstance(request, dict) else None
        if (not isinstance(request, dict) or not {"target", "issuance_id"} <= request.keys()
                or set(request) - {"target", "issuance_id", "controller_route"}
                or not isinstance(target, dict)):
            raise CustodyError("The retained controller request lacks its exact target.")
        issuance(request["issuance_id"])
        constraint = request.get("controller_route")
        if constraint is not None and (not isinstance(constraint, str) or not constraint or constraint.strip() != constraint):
            raise CustodyError("The retained controller request changed its explicit key constraint.")
        if target == {"kind": "helper"}:
            if not has_helper:
                raise CustodyError("A completed helper controller requires a selected helper.")
            key = ("helper",)
        elif set(target) == {"kind", "route"} and target["kind"] == "original_route" and isinstance(target["route"], str):
            key = ("original_route", target["route"])
        else:
            raise CustodyError("The retained controller request changed its native target.")
        if key in controller_targets:
            raise CustodyError("The retained controller request repeats a target.")
        controller_targets.add(key)
    routes, completed_routes = before["routes"], after["routes"]
    if (not isinstance(routes, list) or not isinstance(completed_routes, list)
            or len(routes) != len(completed_routes)):
        raise CustodyError("The completed preparation changed its original route inventory.")
    route_ids = set()
    for selected, completed in zip(routes, completed_routes):
        if (not isinstance(selected, dict) or not isinstance(completed, dict)
                or not isinstance(selected.get("id"), str) or selected["id"] in route_ids):
            raise CustodyError("The completed preparation has ambiguous original route custody.")
        route_ids.add(selected["id"])
        expected = dict(selected)
        origin = selected.get("origin")
        if isinstance(origin, dict) and origin.get("kind") == "local_console":
            expected["origin"] = {**origin, "member": project(origin.get("member"))}
        if (("original_route", selected["id"]) in controller_targets
                and selected.get("controller_parent") is None):
            expected["controller_parent"] = completed.get("controller_parent")
            if not expected["controller_parent"]:
                raise CustodyError("The completed original controller request lacks its native parent.")
        if not _same_json_value(expected, completed):
            raise CustodyError("The completed preparation changed an original route or source fact.")
    if any(key[0] == "original_route" and key[1] not in route_ids for key in controller_targets):
        raise CustodyError("The retained controller request selects an absent original route.")
    selected_members = source.get("members")
    if (not isinstance(selected_members, list) or len(selected_members) != len(members)
            or any(not isinstance(row, dict) for row in selected_members)):
        raise CustodyError("The completed preparation changed its selected member inventory.")
    if not selected_members:
        classes = source.get("member_classes")
        policy = source.get("tool_policy")
        if (has_helper or not isinstance(classes, list) or not classes
                or any(not isinstance(row, dict) for row in classes)
                or source.get("roster") != [] or not isinstance(policy, dict) or policy.get("members") != []):
            raise CustodyError("Empty completed preparation requires helperless classes and no invented initial inventory.")
    setup_requests = authorities.get("agent_setup", [])
    for request in setup_requests:
        if (not isinstance(request, dict) or not {"member", "route", "issuance_id"} <= request.keys()
                or set(request) - {"member", "route", "issuance_id", "helper_targets"}
                or not isinstance(request.get("member"), dict)
                or not isinstance(request.get("route"), str)
                or sum(_same_json_value(row.get("member"), request["member"]) for row in selected_members) != 1):
            raise CustodyError("The retained setup request lacks its exact selected member.")
        issuance(request.get("issuance_id"))
        for selected_target in _preparation_setup_targets(request.get("helper_targets", [])):
            issuance(selected_target["issuance_id"])
    for selected, completed in zip(selected_members, members):
        if not has_helper and selected.get("setup_capability") is not None:
            raise CustodyError("A retained setup capability requires a selected helper.")
        expected = {**selected, "member": project(selected.get("member"))}
        # Native's optional setup reference omits None on serialization.
        if expected.get("setup_capability") is None:
            expected.pop("setup_capability", None)
        requests = [request for request in setup_requests if _same_json_value(request["member"], selected.get("member"))]
        if len(requests) > 1:
            raise CustodyError("The retained setup request repeats a member.")
        if requests and selected.get("setup_capability") is None:
            capability = completed.get("setup_capability")
            if (not isinstance(capability, dict) or not {"route", "parent"} <= capability.keys()
                    or set(capability) - {"route", "parent", "helper_targets"}
                    or not _same_json_value(capability["route"], requests[0]["route"])
                    or capability["parent"] is None):
                raise CustodyError("The completed setup request lacks its selected native capability.")
            expected["setup_capability"] = capability
        if requests:
            capability = completed.get("setup_capability")
            if not isinstance(capability, dict):
                raise CustodyError("The completed setup request lacks its selected native capability.")
            targets = _preparation_setup_targets(capability.get("helper_targets", []), compiled=True)
            requested = requests[0].get("helper_targets", [])
            if len(targets) != len(requested) or any(not _same_json_value(row["target"], intent["target"])
                    for row, intent in zip(targets, requested)):
                raise CustodyError("The completed setup request changed its explicit helper targets.")
            if requested:
                _prepared_issuance_reference(capability.get("parent"), requests[0]["issuance_id"], after["remote_operator"])
            for row, intent in zip(targets, requested):
                _prepared_issuance_reference(row["parent"], intent["issuance_id"], after["remote_operator"])
        if not _same_json_value(expected, {key: value for key, value in completed.items() if key != "initial_surface"}):
            raise CustodyError("The completed preparation changed a member destination or setup selection.")
    if not has_helper:
        return source
    expected_helper = {"represented_subject": None, **source_helper, "member": helper["member"]}
    if expected_helper.get("report_capability") is None:
        expected_helper.pop("report_capability", None)
    early_catalog = _preparation_helper_catalog_capability(source_helper.get("catalog_capability"),
        source_helper.get("operation_basis"))
    completed_catalog = _preparation_helper_catalog_capability(helper.get("catalog_capability"),
        helper.get("operation_basis"))
    catalog_request = authorities.get("helper_catalog")
    catalog_scope = (_preparation_helper_catalog_request(catalog_request)
        if catalog_request is not None else None)
    if early_catalog is None:
        expected_helper.pop("catalog_capability", None)
        if completed_catalog is not None:
            if (catalog_scope != "current_helper_successors"
                    or len(completed_catalog["parent"]) != 1):
                raise CustodyError("The completed helper has an unrequested catalog successor parent.")
            # Both source and completed operation bases must remain unoccupied.
            _preparation_helper_catalog_capability(completed_catalog, source_helper.get("operation_basis"))
            expected_helper["catalog_capability"] = completed_catalog
    if catalog_scope == "current_helper_successors":
        if completed_catalog is None or len(completed_catalog["parent"]) != 1:
            raise CustodyError("The completed helper lacks its requested catalog successor parent.")
        _prepared_issuance_reference(completed_catalog["parent"][0],
            catalog_request["issuance_id"], after["remote_operator"])
    elif catalog_request is not None and completed_catalog is not None:
        raise CustodyError("An exact-session catalog request cannot select a successor parent.")
    if ("helper",) in controller_targets and not source_helper.get("controller_parent"):
        expected_helper["controller_parent"] = helper.get("controller_parent")
        if not expected_helper["controller_parent"]:
            raise CustodyError("The completed helper controller request lacks its native parent.")
    report = authorities.get("helper_report")
    if report is not None:
        if not isinstance(report, dict) or not _same_json_value(report.get("member"), source_helper["member"]):
            raise CustodyError("The retained report request changed its exact helper.")
        issuance(report.get("issuance_id"))
        for name in ("account_issuance_id", "public_issuance_id"):
            if report.get(name) is not None:
                issuance(report[name])
        if source_helper.get("report_capability") is None:
            expected_helper["report_capability"] = helper.get("report_capability")
            if not expected_helper["report_capability"]:
                raise CustodyError("The completed report request lacks its native capability.")
    expected_helper["operation_basis"] = _prepared_helper_access_basis(
        source_helper.get("operation_basis"), helper.get("operation_basis"), authorities,
        after["remote_operator"], setup.get("server"))
    if not _same_json_value(expected_helper, helper):
        raise CustodyError("The completed preparation changed an explicit helper choice.")
    return source


@dataclass(frozen=True)
class CompletedApplicationSetupBundle:
    """Byte custody for the existing producer's completed schema4 output.

    The trusted owner must supply the genuine native manifest after successful
    same-preparation finish with publication withheld. This reader cannot prove
    that precondition, qualify native semantics or select an SDK carrier. It
    retains the original input and all native output fields as exact bytes.
    """

    directory: tuple
    resources_directory: tuple | None
    manifest: ExactFile
    setup: ExactFile
    application_tool_policy: ExactFile
    resource_handoff: ExactFile | None
    setup_input: ExactFile | None
    member_surfaces: tuple[tuple[ExactFile, ExactFile], ...]
    source_files: tuple[ExactFile, ...]
    _require_current: Callable[[], None] = field(repr=False, compare=False)
    preparation_plan: ExactFile | None = None
    _require_filesystem_current: Callable[[], None] | None = field(default=None, repr=False, compare=False)

    @property
    def artifact_directory(self):
        """Actual native artifact root; helperless output has no resource copy."""
        return self.resources_directory if self.resources_directory is not None else self.directory

    def pins(self):
        return {"directory": {"path": str(self.directory[0]),
                    "device": self.directory[1][0], "inode": self.directory[1][1]},
                **({"resources_directory": {"path": str(self.resources_directory[0]),
                    "device": self.resources_directory[1][0], "inode": self.resources_directory[1][1]}}
                   if self.resources_directory is not None else {}),
                **{name: getattr(self, name).pin() for name in (
                    "manifest", "setup", "application_tool_policy", "resource_handoff")
                    if getattr(self, name) is not None},
                **({"preparation_plan": self.preparation_plan.pin()} if self.preparation_plan is not None
                   else {"setup_input": self.setup_input.pin()}),
                "member_surfaces": [{"original": original.pin(), "output": output.pin()}
                    for original, output in self.member_surfaces],
                "source_files": [pin.pin() for pin in self.source_files]}

    def authenticate(self):
        """Synchronous owner-fenced reader for existing trusted callers."""
        return self._authenticate_files(_owner_fences=True)

    def _authenticate_files(self, *, _owner_fences=False):
        """File-only reader for joined workers, preserving the real owner."""
        def require_current():
            if _owner_fences:
                self._require_current()
            if self._require_filesystem_current is not None:
                self._require_filesystem_current()

        require_current()
        if (_private_directory(self.directory[0]) != self.directory
                or _private_directory(self.artifact_directory[0]) != self.artifact_directory):
            raise CustodyError("The retained completed setup directories changed.")
        input_pin = self.preparation_plan if self.preparation_plan is not None else self.setup_input
        source_bytes = input_pin.read()
        if self.preparation_plan is not None and len(source_bytes) > 256 * 1024:
            raise CustodyError("The retained preparation plan exceeds its native artifact limit.")
        if self.preparation_plan is not None:
            _private_parent(_path(input_pin.path))
        source = _json(source_bytes)
        for pin in self.source_files:
            pin.read()
        artifacts = _completed_manifest(self.manifest)
        setup_bytes, policy_bytes = self.setup.read(), self.application_tool_policy.read()
        setup = _json(setup_bytes)
        if self.preparation_plan is not None:
            if self.setup_input is not None or not isinstance(setup, dict):
                raise CustodyError("Driver plan custody cannot substitute a distinct resolved input.")
            source = _prepared_plan_source(source, setup)
        if (not isinstance(source, dict) or type(source.get("schema")) is not int or source["schema"] != 4
                or not isinstance(setup, dict) or type(setup.get("schema")) is not int or setup["schema"] != 4
                or "helper_source_surface" in setup
                or not isinstance(setup.get("input_sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", setup["input_sha256"])):
            raise CustodyError("The completed setup requires its distinct schema4 input and output.")
        if (artifacts.get("application-setup.json") != self.setup
                or artifacts.get("application-tool-policy.json") != self.application_tool_policy
                or not _same_json_value(setup.get("application_tool_policy"), {
                    "name": self.application_tool_policy.path.name,
                    "sha256": self.application_tool_policy.sha256})):
            raise CustodyError("The completed setup changed its selected policy references.")
        has_helper = "helper" in source
        if has_helper:
            if (self.resources_directory is None or self.resource_handoff is None
                    or "artifact_directory" in setup
                    or not _same_json_value(setup.get("resources"), {
                        "artifact_directory": str(self.resources_directory[0]),
                        "handoff": self.resource_handoff.pin()})):
                raise CustodyError("The completed setup changed its selected resource references.")
        elif (self.preparation_plan is None or self.resources_directory is not None
                or self.resource_handoff is not None
                or set(setup) != {"schema", "native_owner", "input_sha256", "realm", "definition_source",
                    "definition", "native_members", "artifact_directory", "application_tool_policy"}
                or setup["artifact_directory"] != str(self.directory[0])):
            raise CustodyError("Helperless completion requires its exact compiler-owned artifact directory.")
        source_helper, helper = source.get("helper"), setup.get("helper")
        selections, members = source.get("members"), setup.get("native_members")
        if (not isinstance(selections, list) or not isinstance(members, list) or len(members) != len(selections)):
            raise CustodyError("The completed setup changed its selected member inventory.")
        if has_helper and (not isinstance(source_helper, dict) or not isinstance(source_helper.get("member"), dict)
                or not isinstance(helper, dict) or not isinstance(helper.get("member"), dict)
                or not selections
                or not _same_json_value(setup.get("source_helper_member"), source_helper["member"])
                or not _same_json_value(
                    {key: value for key, value in source_helper["member"].items() if key != "role"},
                    {key: value for key, value in helper["member"].items() if key != "role"})):
            raise CustodyError("The completed setup changed its source and final helper associations.")
        helper_indices = [index for index, row in enumerate(selections)
            if has_helper and isinstance(row, dict) and _same_json_value(row.get("member"), source_helper["member"])]
        if has_helper and len(helper_indices) != 1:
            raise CustodyError("The retained schema4 input has an ambiguous source helper association.")
        references = []
        for index, (selected, member) in enumerate(zip(selections, members)):
            expected_member = helper["member"] if index in helper_indices else selected.get("member") if isinstance(selected, dict) else None
            reference = member.get("initial_surface") if isinstance(member, dict) else None
            if (not isinstance(selected, dict) or not isinstance(member, dict)
                    or not isinstance(selected.get("member"), dict) or not isinstance(member.get("member"), dict)
                    or set(member) - ({"member", "logical_owner", "context_generation", "audience", "initial_surface"}
                        | ({"setup_capability"} if self.preparation_plan is not None else set()))
                    or not {"member", "logical_owner", "context_generation", "audience", "initial_surface"} <= member.keys()
                    or not _same_json_value(member["member"], expected_member)
                    or not _same_json_value(member["context_generation"], selected.get("context_generation"))
                    or not isinstance(reference, dict) or set(reference) != {"name", "sha256"}
                    or not isinstance(reference["name"], str) or not isinstance(reference["sha256"], str)
                    or not re.fullmatch(r"member-surface-[0-9a-f]{64}\.json", reference["name"])
                    or not re.fullmatch(r"[0-9a-f]{64}", reference["sha256"])):
                raise CustodyError("The completed setup changed its exact member reference inventory.")
            references.append((ExactFile(self.artifact_directory[0] / reference["name"], reference["sha256"]),
                               ExactFile(self.directory[0] / reference["name"], reference["sha256"])))
        if (len({output.path.name for _, output in references}) != len(references)
                or tuple(references) != self.member_surfaces
                or artifacts != {pin.path.name: pin for pin in (
                    self.setup, self.application_tool_policy, *(output for _, output in references))}
                or {path.name for path in self.directory[0].iterdir()} != set(artifacts) | {"manifest.json"}):
            raise CustodyError("The completed setup lacks its exact member and bundle file closure.")
        definition = source.get("definition")
        if (isinstance(definition, dict) and definition.get("format") == "toml"
                and not _same_json_value(setup.get("definition_source"), definition)):
            raise CustodyError("The completed setup changed its exact raw TOML source.")
        for name in ("helper_tools", "helper_profile"):
            if not _same_json_value(setup.get(name), source.get(name)):
                raise CustodyError("The completed setup changed an explicit helper configuration choice.")
        # Serde's missing Option<PathBuf> is None; no other realm defaults exist.
        realm = source.get("realm")
        if (not isinstance(realm, dict) or not _same_json_value(setup.get("realm"),
                {"user_config_root": None, **realm})):
            raise CustodyError("The completed setup changed its explicit resource realm inputs.")
        # These literal identity/profile strings associate the retained row.
        # Native alone resolves roster member IDs and all other typed row values.
        roster, retained_row = source.get("roster"), setup.get("source_helper_roster")
        if has_helper and (not isinstance(roster, list) or not isinstance(retained_row, dict)
                or not isinstance(retained_row.get("identity"), str)
                or not _same_json_value(retained_row.get("profile"), source_helper["member"].get("role"))
                or sum(isinstance(row, dict)
                    and _same_json_value(row.get("identity"), retained_row.get("identity"))
                    and _same_json_value(row.get("profile"), retained_row.get("profile"))
                    for row in roster) != 1):
            raise CustodyError("The completed setup changed its retained source helper roster association.")
        if self.resource_handoff is not None:
            self.resource_handoff.read()
        for original, output in references:
            if _completed_surface(original) != _completed_surface(output):
                raise CustodyError("The completed member surface differs from its original resource bytes.")
        require_current()
        # Reauthenticate the entire supplied source closure and output pins after
        # observation. Currentness remains the existing application owner's duty.
        for pin in (input_pin, *self.source_files, self.manifest, self.setup,
                    self.application_tool_policy,
                    *((self.resource_handoff,) if self.resource_handoff is not None else ())):
            pin.read()
        if self.preparation_plan is not None:
            _private_parent(_path(input_pin.path))
        for original, output in references:
            _completed_surface(original)
            _completed_surface(output)
        if (_private_directory(self.directory[0]) != self.directory
                or _private_directory(self.artifact_directory[0]) != self.artifact_directory
                or {path.name for path in self.directory[0].iterdir()} != set(artifacts) | {"manifest.json"}):
            raise CustodyError("The completed setup closure changed during observation.")
        require_current()
        return setup_bytes, policy_bytes


@dataclass(frozen=True)
class _SetupPreparation:
    parent: tuple
    output_path: Path
    setup_input: ExactFile
    helper_source_surface: ExactFile

    def require_inputs(self, *, fresh=False):
        if _private_directory(self.parent[0]) != self.parent:
            raise CustodyError("The selected setup output parent changed.")
        if fresh:
            _path(self.output_path, fresh=True)

    def checked_bundle(self, receipt):
        pins = tuple(_artifact_pin(receipt[name]) for name in (
            "manifest", "setup", "application_tool_policy", "helper_initial_surface"))
        if tuple(pin.path for pin in pins) != tuple(self.output_path / name for name in (
                "manifest.json", "application-setup.json", "application-tool-policy.json",
                "helper-initial-surface.json")):
            raise CustodyError("The native setup selected different bundle paths.")
        directory = _private_directory(self.output_path)
        setup = _json(pins[1].read())
        if not isinstance(setup, dict) or not isinstance(setup.get("resources"), dict):
            raise CustodyError("The prepared setup lacks its resource custody.")
        resources = setup["resources"]
        resource_path = receipt["prepared_artifact_directory"]
        if (not isinstance(resource_path, str) or resources.get("artifact_directory") != resource_path
                or not isinstance(setup.get("helper"), dict)
                or not _same_json_value(setup["helper"].get("member"), receipt["helper"])):
            raise CustodyError("The setup receipt and bundle disagree on helper or resources.")
        resource_directory = _private_directory(Path(resource_path))
        if (resource_directory[0] == self.output_path
                or self.output_path in resource_directory[0].parents
                or resource_directory[0] in self.output_path.parents):
            raise CustodyError("The native setup bundle overlaps its prepared resources.")
        handoff = _artifact_pin(resources.get("handoff"))
        if handoff.path != resource_directory[0] / "prepare.stdout":
            raise CustodyError("The prepared resource handoff has a different path.")
        bundle = PreparedSetupBundle(directory, resource_directory, *pins[:3], handoff,
            self.setup_input, self.helper_source_surface, pins[3])
        bundle.authenticate()
        return bundle


@dataclass(frozen=True)
class _Preparation:
    root: tuple
    artifacts_directory: tuple
    bundle_path: Path
    activation_path: Path
    artifacts: tuple[ExactFile, ...]
    activation_template: ExactFile

    def require_inputs(self, *, fresh=False):
        if (_private_directory(self.root[0]) != self.root
                or _private_directory(self.artifacts_directory[0]) != self.artifacts_directory):
            raise CustodyError("The selected preparation directories changed.")
        if fresh:
            _path(self.bundle_path, fresh=True)
            _path(self.activation_path, fresh=True)

    def checked_selection(self, value):
        selected, outputs = _selection(json.dumps(value, allow_nan=False).encode())
        expected = (self.activation_path, self.bundle_path / "manifest.json",
                    self.bundle_path / "toolkit-local-policy.json")
        if (selected["deployment_root"] != str(self.root[0])
                or tuple(pin.path for pin in outputs) != expected):
            raise CustodyError("The native preparation selected different output paths.")
        _private_directory(self.bundle_path)
        contents = [pin.read() for pin in outputs]
        copied = {pin.path.name: pin.sha256 for pin in self.artifacts}
        files = {**copied, "toolkit-local-policy.json": outputs[2].sha256}
        if {path.name for path in self.bundle_path.iterdir()} != set(files) | {"manifest.json"}:
            raise CustodyError("The prepared bundle does not contain the exact selected artifact closure.")
        manifest = _json(contents[1])
        if not isinstance(manifest, dict) or set(manifest) != {"artifacts"} or not isinstance(manifest["artifacts"], list):
            raise CustodyError("The native preparation manifest has an invalid custody shape.")
        observed = {}
        for row in manifest["artifacts"]:
            if (not isinstance(row, dict) or set(row) != {"path", "sha256"}
                    or not isinstance(row["path"], str) or row["path"] in observed):
                raise CustodyError("The native preparation manifest contains ambiguous artifacts.")
            observed[row["path"]] = row["sha256"]
        if observed != files:
            raise CustodyError("The native manifest differs from the selected copied artifacts.")
        for name, digest in copied.items():
            ExactFile(self.bundle_path / name, digest).read()
        template, activation = _json(self.activation_template.read()), _json(contents[0])
        if not isinstance(template, dict) or not isinstance(activation, dict):
            raise CustodyError("The activation template and output must be JSON objects.")
        if (not _same_json_value(
                {key: item for key, item in template.items() if key != "policy_bundle"},
                {key: item for key, item in activation.items() if key != "policy_bundle"})
                or activation.get("policy_bundle") != {"path": self.bundle_path.name,
                    "policy_digest": "hc-policy-v1:sha256:" + outputs[1].sha256}):
            raise CustodyError("The prepared activation changed its selected template metadata or bundle binding.")
        return selected


def _write_new(path, value):
    path = _path(path, fresh=True)
    _private_parent(path)
    content = (json.dumps(value, ensure_ascii=True, allow_nan=False, separators=(",", ":")) + "\n").encode()
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)


def _capability_request(content, *, agent):
    request = _json(content)
    fields = {"member", "issuance_id", *({"route"} if agent else set())}
    optional = set() if agent else {"account_issuance_id", "public_issuance_id"}
    if (not isinstance(request, dict) or not fields <= request.keys()
            or request.keys() - fields - optional
            or not isinstance(request["member"], dict)
            or set(request["member"]) != {"mob_id", "role", "member"}
            or any(not isinstance(value, str) or not value
                   for value in request["member"].values())
            or not isinstance(request["issuance_id"], str) or not request["issuance_id"]
            or agent and (not isinstance(request["route"], str) or not request["route"])):
        raise CustodyError("Capability configuration requires the exact native request fields.")
    if not agent:
        identifiers = [request["issuance_id"]]
        for name in ("account_issuance_id", "public_issuance_id"):
            value = request.get(name)
            if value is None:
                continue
            if not isinstance(value, str) or not value or value.strip() != value:
                raise CustodyError("Optional helper issuance IDs require exact nonempty text or null.")
            identifiers.append(value)
        if len(set(identifiers)) != len(identifiers):
            raise CustodyError("Helper capability requests require distinct issuance IDs.")
    # Native decodes the identifiers and owns all permission semantics. This
    # only retains the selected wire shape for matching its exited receipt.
    return request


def _inventory_input(content):
    if len(content) > MAX_INVENTORY_BYTES:
        raise CustodyError("The inventory input exceeds its native artifact limit.")
    source = _json(content)
    required = {"schema", "mob_id", "roster"}
    if (not isinstance(source, dict) or not required <= source.keys()
            or source.keys() - required - {"helper_identity", "member_classes"}
            or type(source["schema"]) is not int or source["schema"] != 1
            or not isinstance(source["mob_id"], str) or not source["mob_id"]
            or ("helper_identity" in source and (type(source["helper_identity"]) is not str
                or not source["helper_identity"]))
            or not isinstance(source["roster"], list)):
        raise CustodyError("Inventory requires the exact schema 1 compiler input.")
    declarations = source.get("member_classes", [])
    if type(declarations) is not list or any(type(item) is not dict for item in declarations):
        raise CustodyError("Inventory requires an exact native member-class declaration list.")
    if not source["roster"] and not declarations:
        raise CustodyError("An empty inventory requires explicit native member-class declarations.")
    identities = set()
    for row in source["roster"]:
        if (not isinstance(row, dict)
                or any(not isinstance(row.get(name), str) or not row[name]
                       for name in ("identity", "profile"))
                or row["identity"] in identities):
            raise CustodyError("The inventory roster has missing or repeated identity/profile facts.")
        identities.add(row["identity"])
    if "helper_identity" in source and source["helper_identity"] not in identities:
        raise CustodyError("The selected inventory helper is absent from its exact roster.")
    # DurableAgentSpec and identifier decoding remain native-owned. Preserve
    # every roster field and class declaration as source bytes; only these two
    # facts join rows. Native validates the CompiledMemberClass contents.
    return source


def _inventory_receipt(receipt, source):
    has_helper = "helper_identity" in source
    fields = {"schema", "mob_id", "realm_id", "members"}
    if has_helper:
        fields.add("helper_identity")
    if (not isinstance(receipt, dict)
            or set(receipt) != fields
            or type(receipt["schema"]) is not int or receipt["schema"] != 1
            or not _same_json_value(receipt["mob_id"], source["mob_id"])
            or (has_helper and not _same_json_value(receipt["helper_identity"], source["helper_identity"]))
            or not isinstance(receipt["realm_id"], str) or not receipt["realm_id"]
            or not isinstance(receipt["members"], list)
            or len(receipt["members"]) != len(source["roster"])):
        raise CustodyError("The native inventory changed its selected schema, mob, helper or rows.")
    members = set()
    for selected, row in zip(source["roster"], receipt["members"]):
        member = row.get("member") if isinstance(row, dict) else None
        if (not isinstance(row, dict) or set(row) != {"identity", "member"}
                or not _same_json_value(row["identity"], selected["identity"])
                or not isinstance(member, dict) or set(member) != {"mob_id", "role", "member"}
                or any(not isinstance(value, str) or not value for value in member.values())
                or not _same_json_value(member["mob_id"], source["mob_id"])
                or not _same_json_value(member["role"], selected["profile"])
                or member["member"] in members):
            raise CustodyError("The native inventory changed its same-order member bindings.")
        members.add(member["member"])
    # Native alone spells realm and member IDs. The explicit helper identity
    # stays separate from original-route selection; no row confers authority.


def _receipt(process, command, expected_kind, selection=None, capability_request=None,
             inventory_input=None):
    if (not isinstance(process, ProcessOutcome) or type(process.returncode) is not int
            or not isinstance(process.stdout, bytes) or not isinstance(process.stderr, bytes)
            or len(process.stdout) > MAX_RECEIPT_BYTES or len(process.stderr) > MAX_RECEIPT_BYTES
            or not process.stdout.endswith(b"\n") or process.stdout.count(b"\n") != 1
            or b"\r" in process.stdout):
        raise CustodyError("The native process did not retain one bounded exact receipt line.")
    receipt = _json(process.stdout)
    if not isinstance(receipt, dict):
        raise CustodyError("The native receipt is not an object.")
    if process.returncode == 1 and set(receipt) == {"kind", "command", "phase", "effects_may_have_occurred", "detail"}:
        if (receipt["kind"] == "application_installer_failed" and receipt["command"] == command
                and receipt["phase"] in (SETUP_PHASES if command == "--prepare-application-setup"
                    else CAPABILITY_PHASES if command in CAPABILITY_COMMANDS
                    else INVENTORY_PHASES if command == INVENTORY_COMMAND
                    else APPLICATION_UPDATE_PHASES if command == APPLICATION_UPDATE_COMMAND else PHASES)
                and type(receipt["effects_may_have_occurred"]) is bool
                and (command not in (INVENTORY_COMMAND, APPLICATION_UPDATE_COMMAND)
                     or receipt["effects_may_have_occurred"] is False)
                and isinstance(receipt["detail"], str)):
            return "native_failure", receipt
    if command == INVENTORY_COMMAND:
        if process.returncode != 0 or inventory_input is None:
            raise CustodyError("The native inventory and process exit do not match this command.")
        _inventory_receipt(receipt, inventory_input)
        return "receipt_verified", receipt
    if process.returncode != 0 or receipt.get("kind") != expected_kind:
        raise CustodyError("The native receipt and process exit do not match this command.")
    if command in CAPABILITY_COMMANDS:
        kind, field = CAPABILITY_COMMANDS[command]
        if (expected_kind != kind or capability_request is None
                or set(receipt) != {"kind", "member", field}
                or not _same_json_value(receipt["member"], capability_request["member"])
                or not isinstance(receipt[field], dict)):
            raise CustodyError("The native capability receipt changed its exact selected member or fields.")
        capability = receipt[field]
        if field == "setup_capability":
            valid = (set(capability) == {"route", "parent"}
                and _same_json_value(capability["route"], capability_request["route"])
                and isinstance(capability["parent"], dict) and bool(capability["parent"]))
        else:
            fields = {"parent"}
            for request_field, parent_field in (("account_issuance_id", "account_parent"),
                                                ("public_issuance_id", "public_parent")):
                if capability_request.get(request_field) is not None:
                    fields.add(parent_field)
            valid = set(capability) == fields and all(
                isinstance(capability[name], list) and bool(capability[name])
                and all(isinstance(parent, dict) and bool(parent) for parent in capability[name])
                for name in fields)
            if valid:
                parents = [capability[name] for name in sorted(fields)]
                valid = not any(_same_json_value(parent, earlier)
                    for index, parent in enumerate(parents) for earlier in parents[:index])
        if not valid:
            raise CustodyError("The native capability receipt lacks its selected route or lineage reference shape.")
        # Opaque native lineage references are returned unchanged. Schema4 and
        # the native issuer, not this byte-custody caller, qualify their bounds.
    elif expected_kind == "application_setup_prepared":
        if (set(receipt) != {"kind", "manifest", "setup", "application_tool_policy", "helper",
                            "prepared_artifact_directory", "helper_initial_surface"}
                or not isinstance(receipt["helper"], dict)
                or not isinstance(receipt["prepared_artifact_directory"], str)):
            raise CustodyError("The native setup receipt lacks its exact bundle and helper selection.")
        for name in ("manifest", "setup", "application_tool_policy", "helper_initial_surface"):
            _artifact_pin(receipt[name])
    elif selection is not None:
        if set(receipt) != {"kind", "selection"} or not _same_json_value(receipt["selection"], selection):
            raise CustodyError("The native receipt does not identify the exact supplied selection.")
    elif expected_kind == "application_policy_prepared":
        if set(receipt) != {"kind", "selection"}:
            raise CustodyError("The native preparation receipt lacks its exact selection.")
    elif (set(receipt) != {"kind", "artifact"} or not isinstance(receipt["artifact"], dict)
          or set(receipt["artifact"]) != {"path", "sha256"}
          or not isinstance(receipt["artifact"]["path"], str)
          or not isinstance(receipt["artifact"]["sha256"], str)):
        raise CustodyError("The native compiler receipt lacks its exact artifact pin.")
    return "receipt_verified", receipt


class NativeInstallerCaller:
    """One existing install/update owner; no new runtime or native store owner.

    provision_attempt_path is fixed by that owner for the selected native pair,
    outside cloned application state, and reused on reconstruction/restart. It
    must not vary with candidate, generation, failure, login or retry. An entered
    record bars automatic replay through this caller. Exact native phase and
    effects evidence remains available to the existing trusted owner, including
    a known validation failure with no effect. Any explicit reconciled fresh
    provisioning attempt belongs to that owner with actual absent-pair/exclusive-
    fence facts. Capability reconciliation preserves the exact request bytes and
    issuance ID for native issued_reference readback. Both preserve the entered
    predecessor. This class has no automatic replay, readback or recovery.

    require_current must raise when the actual selected source or stopped-host
    lease changes. The existing owner holds that lease across the entire await.
    require_filesystem_current optionally carries that owner's full source
    closure into file readers without calling its loop-owned selection callback.
    Public synchronous retained-bundle readers remain synchronous; live hosts
    must use the file-only branch inside their joined owner-fenced workers.
    execute must return exact separate stdout/stderr bytes and the actual exit,
    join entered process teardown before raising, and never retry a command.
    """

    def __init__(self, gateway: ExactFile, *,
                 execute: Callable[[tuple[str, ...]], Awaitable[ProcessOutcome]],
                 require_current: Callable[[], None], provision_attempt_path: Path,
                 require_filesystem_current: Callable[[], None] | None = None):
        if require_filesystem_current is not None and not callable(require_filesystem_current):
            raise CustodyError("Filesystem source currentness requires its trusted callable.")
        self.gateway = gateway
        self.execute = execute
        self.require_current = require_current
        self.require_filesystem_current = require_filesystem_current
        self.provision_attempt_path = Path(provision_attempt_path)

    def _require_filesystem_current(self):
        if self.require_filesystem_current is not None:
            self.require_filesystem_current()

    async def _current_owner(self, owner_call):
        if owner_call is None:
            self.require_current()
        else:
            # A shielded finalizer retains its operation Task, while the
            # awaiting original owner performs this exact synchronous fence.
            await owner_call(self.require_current)

    async def _current_filesystem(self, function, *args, _owner_call=None, **kwargs):
        # The actual owner callback stays on its event loop, including the
        # check after every new filesystem await and before native admission.
        await self._current_owner(_owner_call)
        def checked():
            self._require_filesystem_current()
            result = function(*args, **kwargs)
            self._require_filesystem_current()
            return result
        result, cancelled = await _joined_filesystem(checked)
        if cancelled:
            raise asyncio.CancelledError
        await self._current_owner(_owner_call)
        return result

    def retain_completed_application_setup(self, manifest: ExactFile, *, setup_input: ExactFile,
            source_files: tuple[ExactFile, ...]):
        """Retain genuine complete_bundle outputs, without executing a command.

        This separate dormant path receives the existing trusted producer's
        manifest pin. It cannot turn schema4 into a standalone CLI command or
        attest to native finish, readiness, publication or SDK availability.
        """
        return self._retain_completed_application_setup(manifest, setup_input=setup_input,
            preparation_plan=None, source_files=source_files)

    def retain_completed_application_preparation(self, manifest: ExactFile, *,
            preparation_plan: ExactFile, source_files: tuple[ExactFile, ...]):
        """Retain one same-live-attempt manifest with its original driver plan.

        The existing owner must obtain manifest from this exact retained
        ToolkitApplicationRuntime preparation attempt after native finish. A
        standalone file pin cannot establish that live exchange precondition.
        This byte reader keeps input_sha256 opaque and never authors a resolved
        input, issues a parent, reopens the leased native pair or proves Ready.
        Native helper_projection and final candidate validation/adoption remain
        mandatory under the same existing transaction and native owner.
        """
        return self._retain_completed_application_setup(manifest, setup_input=None,
            preparation_plan=preparation_plan, source_files=source_files)

    def _retain_completed_application_preparation_files(self, manifest, *, preparation_plan, source_files):
        return self._retain_completed_application_setup(manifest, setup_input=None,
            preparation_plan=preparation_plan, source_files=source_files, _owner_fences=False)

    def _retain_completed_application_setup(self, manifest, *, setup_input, preparation_plan,
            source_files, _owner_fences=True):
        if _owner_fences:
            self.require_current()
        self._require_filesystem_current()
        input_pin = preparation_plan if preparation_plan is not None else setup_input
        if (not isinstance(manifest, ExactFile) or not isinstance(input_pin, ExactFile)
                or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Completed setup custody requires exact manifest, input and source pins.")
        if preparation_plan is not None:
            _private_parent(_path(preparation_plan.path))
            plan_bytes = preparation_plan.read()
            if len(plan_bytes) > 256 * 1024:
                raise CustodyError("The retained preparation plan exceeds its native artifact limit.")
            plan = _json(plan_bytes)
            if not isinstance(plan, dict) or plan.get("output") != str(manifest.path.parent):
                raise CustodyError("The completed manifest changed its original driver output path.")
            _private_directory(Path(plan.get("deployment_root", "")))
        if manifest.path.name != "manifest.json":
            raise CustodyError("The completed setup must retain its native manifest path.")
        artifacts = _completed_manifest(manifest)
        directory = _private_directory(manifest.path.parent)
        setup = artifacts["application-setup.json"]
        value = _json(setup.read())
        if not isinstance(value, dict) or not isinstance(value.get("native_members"), list):
            raise CustodyError("The completed setup lacks its retained member references.")
        resource_directory, handoff = None, None
        if "helper" in value:
            resources = value.get("resources")
            if ("artifact_directory" in value or not isinstance(resources, dict)
                    or not isinstance(resources.get("artifact_directory"), str)):
                raise CustodyError("The completed setup lacks its retained resource references.")
            resource_directory = _private_directory(Path(resources["artifact_directory"]))
            if (directory[0] == resource_directory[0] or directory[0] in resource_directory[0].parents
                    or resource_directory[0] in directory[0].parents):
                raise CustodyError("The completed setup output overlaps its original resources.")
            handoff = _artifact_pin(resources.get("handoff"))
            if handoff.path != resource_directory[0] / "prepare.stdout":
                raise CustodyError("The completed setup changed its original resource handoff path.")
        elif (preparation_plan is None or "resources" in value
                or value.get("artifact_directory") != str(directory[0])):
            raise CustodyError("Helperless completion requires its exact compiler-owned artifact directory.")
        artifact_directory = resource_directory if resource_directory is not None else directory
        source_paths = [input_pin.path, *(pin.path for pin in source_files)]
        if len(set(source_paths)) != len(source_paths) or any(
                path == owner or owner in path.parents
                for path in source_paths for owner in (directory[0], artifact_directory[0])):
            raise CustodyError("The completed setup must preserve separate original source custody.")
        surfaces = []
        for member in value["native_members"]:
            reference = member.get("initial_surface") if isinstance(member, dict) else None
            if (not isinstance(reference, dict) or set(reference) != {"name", "sha256"}
                    or not isinstance(reference["name"], str) or not isinstance(reference["sha256"], str)
                    or not re.fullmatch(r"member-surface-[0-9a-f]{64}\.json", reference["name"])):
                raise CustodyError("The completed setup lacks flat native member surface references.")
            surfaces.append((ExactFile(artifact_directory[0] / reference["name"], reference["sha256"]),
                             ExactFile(directory[0] / reference["name"], reference["sha256"])))
        bundle = CompletedApplicationSetupBundle(directory, resource_directory, manifest, setup,
            artifacts["application-tool-policy.json"], handoff, setup_input, tuple(surfaces),
            source_files, self.require_current, preparation_plan, self.require_filesystem_current)
        if _owner_fences:
            bundle.authenticate()
        else:
            bundle._authenticate_files()
        return bundle

    async def validate_application_policy(self, selection: ExactFile, *, attempt_path: Path,
                                          source_files: tuple[ExactFile, ...] = (), _owner_call=None):
        selected, artifacts = await self._current_filesystem(lambda: _selection(selection.read()),
            _owner_call=_owner_call)
        return await self._run("--validate-application-policy", "application_policy_validated",
            ("--selection", str(selection.path)), selection, (*artifacts, *source_files),
            Path(attempt_path), selected=selected, _owner_call=_owner_call)

    async def validate_application_update(self, selection: ExactFile, *, attempt_path: Path,
                                          source_files: tuple[ExactFile, ...] = ()):
        """Observe native update-input validation under the existing owner.

        The owner supplies the retained predecessor, full source closure and
        held project lease through its currentness checks. An exact receipt is
        input validation only, without publication, member adoption or Ready.
        """
        if (not isinstance(selection, ExactFile) or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Update validation needs exact retained selection and source pins.")
        selected, artifacts = await self._current_filesystem(
            lambda: _application_update_selection(selection.read()))
        return await self._run(APPLICATION_UPDATE_COMMAND, "application_update_validated",
            ("--selection", str(selection.path)), selection, (*artifacts, *source_files),
            Path(attempt_path), selected=selected)

    async def lower_application_inventory(self, compiler_input: ExactFile, *,
            attempt_path: Path, source_files: tuple[ExactFile, ...] = ()):
        """Observe native roster bindings from the selected compiler input.

        The schema 1 result stays in InstallerOutcome.receipt with its exact
        helper identity and same-order rows. Keep this original roster as the
        later compiler input. Inventory does not select original routes, grant
        acquisition, create native owners or establish application readiness.
        """
        self.require_current()
        if (not isinstance(compiler_input, ExactFile) or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Inventory needs exact retained compiler input and source pins.")
        source = await self._current_filesystem(lambda: _inventory_input(compiler_input.read()))
        attempt_path = Path(attempt_path)
        if {attempt_path, Path(str(attempt_path) + ".outcome.json")} & {
                self.provision_attempt_path, Path(str(self.provision_attempt_path) + ".outcome.json")}:
            raise CustodyError("Inventory attempts must preserve separate native provisioning custody.")
        result = await self._run(INVENTORY_COMMAND, None,
            ("--input", str(compiler_input.path)), compiler_input, source_files,
            attempt_path, inventory_input=source)
        if result.status != "receipt_verified":
            return result

        def retain_inventory():
            pins = []
            for path in (attempt_path, Path(str(attempt_path) + ".outcome.json")):
                content = _read(path, MAX_INPUT_BYTES)
                pins.append(ExactFile(path, hashlib.sha256(content).hexdigest()))
            inventory = NativeApplicationInventory(self, self.gateway, compiler_input,
                source_files, *pins)
            _, observed = inventory._authenticate_files()
            if not _same_json_value(observed, result.receipt):
                raise CustodyError("The original inventory result changed before retention.")
            return inventory

        inventory = await self._current_filesystem(retain_inventory)
        return InstallerOutcome(result.status, result.receipt, result.artifact,
            result.attempt_path, result.selection, result.setup_bundle, inventory)

    async def configure_agent_setup_capability(self, selection: ExactFile, request: ExactFile, *,
            attempt_path: Path, source_files: tuple[ExactFile, ...] = ()):
        """Invoke one explicit trusted member/route request, without activation."""
        return await self._configure_capability("--configure-agent-setup-capability",
            selection, request, attempt_path=attempt_path, source_files=source_files)

    async def configure_helper_report_capability(self, selection: ExactFile, request: ExactFile, *,
            attempt_path: Path, source_files: tuple[ExactFile, ...] = ()):
        """Invoke one selected helper request; native derives its report bounds."""
        return await self._configure_capability("--configure-helper-report-capability",
            selection, request, attempt_path=attempt_path, source_files=source_files)

    async def _configure_capability(self, command, selection, request, *, attempt_path, source_files):
        self.require_current()
        if (not isinstance(selection, ExactFile) or not isinstance(request, ExactFile)
                or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Capability configuration needs exact retained selection, request and source pins.")
        def read_request():
            _, artifacts = _selection(selection.read())
            requested = _capability_request(request.read(), agent=command == "--configure-agent-setup-capability")
            return artifacts, requested
        artifacts, requested = await self._current_filesystem(read_request)
        attempt_path = Path(attempt_path)
        if {attempt_path, Path(str(attempt_path) + ".outcome.json")} & {
                self.provision_attempt_path, Path(str(self.provision_attempt_path) + ".outcome.json")}:
            raise CustodyError("Capability attempts must preserve separate native provisioning custody.")
        kind, _ = CAPABILITY_COMMANDS[command]
        return await self._run(command, kind,
            ("--selection", str(selection.path), "--request", str(request.path)),
            selection, (request, *artifacts, *source_files), attempt_path,
            capability_request=requested)

    async def provision_application_owner(self, selection: ExactFile, *, namespace: str,
                                          generation: int, source_files: tuple[ExactFile, ...] = ()):
        if not isinstance(namespace, str) or not namespace or namespace.strip() != namespace or "\x00" in namespace:
            raise CustodyError("The native namespace must be exact nonempty text.")
        if type(generation) is not int or not 1 <= generation <= 2**64 - 1:
            raise CustodyError("The native owner generation must be a positive u64.")
        selected, artifacts = await self._current_filesystem(lambda: _selection(selection.read()))
        return await self._run("--provision-application-owner", "application_owner_provisioned",
            ("--selection", str(selection.path), "--namespace", namespace, "--generation", str(generation)),
            selection, (*artifacts, *source_files), self.provision_attempt_path, selected=selected)

    async def compile_selected_application_tool_policy(self, selection: ExactFile, *,
            source_id: str, selected_member_identities: Sequence[str] | None = None,
            compiler_input_path: Path, output_path: Path, attempt_path: Path,
            source_files: tuple[ExactFile, ...] = (),
            inventory: NativeApplicationInventory | None = None):
        """Retain uncompiled input from one exact trusted application selection.

        With inventory, exact durable source rows join the retained native
        receipt. Without it, the source must already contain actual native
        member identities supplied by the trusted owner. There is no heuristic
        conversion. Source provenance always hashes the original selected bytes.
        Native code still writes the policy artifact and canonical receipt. The
        fresh private input remains retained after failure or unknown entry.
        """
        if (not isinstance(selection, ExactFile) or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Policy selection must be a retained ExactFile.")
        if inventory is not None and (not isinstance(inventory, NativeApplicationInventory)
                or inventory._caller is not self or inventory.gateway != self.gateway
                or selected_member_identities is not None):
            raise CustodyError("Policy compilation needs this same caller's inventory, without competing identities.")
        if inventory is None and selected_member_identities is None:
            raise CustodyError("Policy compilation needs explicit native member identities or retained inventory.")
        self.require_current()
        def retain_input():
            raw = selection.read()
            if inventory is None:
                content = application_tool_policy_input(raw, source_id=source_id,
                    selected_member_identities=selected_member_identities)
            else:
                original, observed = inventory._authenticate_files()
                selected = application_tool_policy_selection(raw,
                    selected_member_identities=tuple(row["identity"] for row in original["roster"]))
                bindings = {row["identity"]: row["member"]["member"] for row in observed["members"]}
                for row in selected["members"]:
                    row["member_identity"] = bindings[row["member_identity"]]
                content = _compiler_input(selected, raw, _text(source_id, "source_id"))
            # Reuse the private exclusive JSON writer, then verify byte-equivalence
            # to the pure adapter rather than defining another input serializer.
            _write_new(compiler_input_path, _json(content))
            compiler_input = ExactFile(Path(compiler_input_path), hashlib.sha256(content).hexdigest())
            compiler_input.read()
            return compiler_input
        compiler_input = await self._current_filesystem(retain_input)
        if inventory is not None:
            await self._current_filesystem(inventory._authenticate_files)
        return await self.compile_application_tool_policy(compiler_input,
            output_path=output_path, attempt_path=attempt_path,
            # _run already pins/verifies this exact gateway as its executable.
            # Do not reread a large gateway as a bounded ordinary source file.
            source_files=(selection, *(inventory.originals[1:] if inventory is not None else ()), *source_files),
            expected_gateway=inventory.gateway if inventory is not None else None)

    async def compile_application_tool_policy(self, compiler_input: ExactFile, *, output_path: Path,
            attempt_path: Path, source_files: tuple[ExactFile, ...] = (),
            expected_gateway: ExactFile | None = None):
        def require_output():
            path = _path(output_path, fresh=True)
            _private_parent(path)
            return path
        output_path = await self._current_filesystem(require_output)
        return await self._run("--compile-application-tool-policy", "application_tool_policy_compiled",
            ("--input", str(compiler_input.path), "--output", str(output_path)),
            compiler_input, tuple(source_files), Path(attempt_path), output_path=output_path,
            expected_gateway=expected_gateway)

    async def prepare_application_policy(self, compiled_local_policy: ExactFile, *,
            artifacts_directory: Path, artifacts: tuple[ExactFile, ...], deployment_root: Path,
            bundle_path: Path, activation_template: ExactFile, activation_path: Path,
            attempt_path: Path, source_files: tuple[ExactFile, ...] = (), _owner_call=None):
        """Retain the native-produced immutable bundle, without publishing it.

        The trusted owner supplies the exact copied artifact closure and already
        compiled local policy. Native code validates and serializes the policy,
        manifest and activation; Python checks only selected byte/path custody.
        """
        await self._current_owner(_owner_call)
        if (not isinstance(compiled_local_policy, ExactFile)
                or not isinstance(activation_template, ExactFile)
                or not isinstance(artifacts, tuple)
                or any(not isinstance(pin, ExactFile) for pin in artifacts)):
            raise CustodyError("Preparation needs exact retained policy, template and artifact pins.")
        def require_preparation():
            root, directory = _private_directory(deployment_root), _private_directory(artifacts_directory)
            bundle, activation = _path(Path(bundle_path), fresh=True), _path(Path(activation_path), fresh=True)
            names = [pin.path.name for pin in artifacts]
            if (any(pin.path.parent != directory[0] for pin in artifacts)
                    or len(set(names)) != len(names) or "policy.json" not in names
                    or set(names) & {"manifest.json", "toolkit-local-policy.json"}):
                raise CustodyError("Preparation requires one exact direct-child source artifact closure.")
            attempt = Path(attempt_path)
            claimed = {attempt, Path(str(attempt) + ".outcome.json"), self.provision_attempt_path,
                       Path(str(self.provision_attempt_path) + ".outcome.json")}
            if (bundle.parent != root[0] or activation.parent != root[0]
                    or bundle == activation or {bundle, activation} & claimed):
                raise CustodyError("Preparation outputs must be distinct fresh direct children, separate from attempt custody.")
            return _Preparation(root, directory, bundle, activation, artifacts, activation_template)
        preparation = await self._current_filesystem(require_preparation, _owner_call=_owner_call)
        root, directory = preparation.root, preparation.artifacts_directory
        bundle_path, activation_path = preparation.bundle_path, preparation.activation_path
        attempt_path = Path(attempt_path)
        return await self._run("--prepare-application-policy", "application_policy_prepared",
            ("--policy", str(compiled_local_policy.path), "--artifacts", str(directory[0]),
             "--deployment-root", str(root[0]), "--bundle", str(bundle_path),
             "--activation-template", str(activation_template.path), "--activation", str(activation_path)),
            compiled_local_policy, (activation_template, *artifacts, *source_files), attempt_path,
            preparation=preparation, _owner_call=_owner_call)

    async def prepare_application_setup(self, setup_input: ExactFile, *, output_directory: Path,
            attempt_path: Path, source_files: tuple[ExactFile, ...] = ()):
        """Prepare resources before policy/native bootstrap from exact app input.

        The trusted application owner chooses every schema 3 input, including
        its helper and profile. This method only invokes and authenticates the
        native-produced bundle; it does not select, compile or project topology.
        It never re-prepares on restart or repairs a partial entered attempt.
        """
        self.require_current()
        if (not isinstance(setup_input, ExactFile) or not isinstance(source_files, tuple)
                or any(not isinstance(pin, ExactFile) for pin in source_files)):
            raise CustodyError("Setup preparation needs exact retained input and source pins.")
        def require_setup():
            source = _json(setup_input.read())
            if not isinstance(source, dict) or type(source.get("schema")) is not int or source["schema"] != 3:
                raise CustodyError("Setup preparation requires the selected schema 3 input.")
            helper_source_surface = _artifact_pin(source.get("helper_source_surface"))
            # The existing private ExactFile owner retains this original path on
            # restart. Native validates its association and compiles the surface.
            helper_source_surface.read()
            output = _path(Path(output_directory), fresh=True)
            parent = _private_directory(output.parent)
            attempt = _path(Path(attempt_path), fresh=True)
            attempt_custody = {attempt, Path(str(attempt) + ".outcome.json"), self.provision_attempt_path,
                              Path(str(self.provision_attempt_path) + ".outcome.json")}
            if helper_source_surface.path in attempt_custody:
                raise CustodyError("The selected source surface must be separate from attempt custody.")
            claimed = {*attempt_custody,
                       self.gateway.path, setup_input.path, helper_source_surface.path,
                       *(pin.path for pin in source_files)}
            if output in claimed:
                raise CustodyError("The setup output must be separate from selected sources and attempt custody.")
            if {attempt, Path(str(attempt) + ".outcome.json")} & {
                    self.provision_attempt_path, Path(str(self.provision_attempt_path) + ".outcome.json")}:
                raise CustodyError("The setup attempt must preserve separate native provisioning custody.")
            return _SetupPreparation(parent, output, setup_input, helper_source_surface)
        preparation = await self._current_filesystem(require_setup)
        output, attempt = preparation.output_path, Path(attempt_path)
        helper_source_surface = preparation.helper_source_surface
        return await self._run("--prepare-application-setup", "application_setup_prepared",
            ("--input", str(setup_input.path), "--output", str(output)), setup_input,
            (helper_source_surface, *source_files), attempt, setup_preparation=preparation)

    async def _run(self, command, kind, arguments, input_file, source_files, attempt_path, *,
                   selected=None, output_path=None, preparation=None, setup_preparation=None,
                   capability_request=None, inventory_input=None, expected_gateway=None, _owner_call=None):
        await self._current_owner(_owner_call)
        if expected_gateway is not None and self.gateway != expected_gateway:
            raise CustodyError("The native compiler changed its retained inventory gateway.")
        pins = (self.gateway, input_file, *source_files)
        argv = (str(self.gateway.path), command, *arguments)

        def require_inputs(*, fresh=False):
            self._require_filesystem_current()
            if expected_gateway is not None and self.gateway != expected_gateway:
                raise CustodyError("The native compiler changed its retained inventory gateway.")
            for index, pin in enumerate(pins):
                if (command in CAPABILITY_COMMANDS or command in (INVENTORY_COMMAND, APPLICATION_UPDATE_COMMAND)):
                    if index != 0 or type(pin) is not ProductGatewayFile:
                        _private_parent(_path(pin.path))
                if index == 0:
                    pin.verify(executable=True)
                else:
                    pin.read()
            if preparation is not None:
                preparation.require_inputs(fresh=fresh)
            if setup_preparation is not None:
                setup_preparation.require_inputs(fresh=fresh)
            self._require_filesystem_current()

        def claim_attempt():
            require_inputs(fresh=True)
            completion = _path(Path(str(attempt_path) + ".outcome.json"), fresh=True)
            if output_path is not None and output_path in {Path(attempt_path), completion}:
                raise CustodyError("The native output cannot overwrite application attempt custody.")
            # Exclusive claim and its preceding pin checks share one worker.
            # A crash or cancelled waiter retains this exact entered attempt.
            _write_new(attempt_path, {"status": "entered", "argv": list(argv),
                "input_files": [pin.pin() for pin in pins],
                "output_path": str(output_path) if output_path else None,
                **({"output_directory": str(setup_preparation.output_path)} if setup_preparation else {})})
            return completion

        # A successful claim is joined even under cancellation. Its exact result
        # enters the completion path before cancellation can release custody.
        completion_path, cancelled = await _joined_filesystem(claim_attempt)
        process = None
        receipt = None
        artifact = None
        output_selection = None
        setup_bundle = None
        status = "outcome_unknown"
        error_type = None
        try:
            if cancelled:
                raise asyncio.CancelledError
            await self._current_owner(_owner_call)
            if expected_gateway is not None and self.gateway != expected_gateway:
                raise CustodyError("The native compiler changed its retained inventory gateway.")
            process = await self.execute(argv)
            (status, receipt), interrupted = await _joined_filesystem(
                _receipt, process, command, kind, selected, capability_request, inventory_input)
            if interrupted:
                raise asyncio.CancelledError
            # Native effects and any receipt remain historical if selection or
            # the caller lease changed. Never publish from that stale result.
            try:
                await self._current_owner(_owner_call)
                await self._current_filesystem(require_inputs, _owner_call=_owner_call)
            except Exception:
                status = "source_changed"
                raise

            def checked_outputs():
                artifact, selection, bundle = None, None, None
                if status == "receipt_verified" and output_path is not None:
                    if receipt["artifact"]["path"] != str(output_path):
                        raise CustodyError("The native compiler wrote a different output path.")
                    artifact = ExactFile(output_path, receipt["artifact"]["sha256"])
                    artifact.read()
                if status == "receipt_verified" and preparation is not None:
                    selection = preparation.checked_selection(receipt["selection"])
                if status == "receipt_verified" and setup_preparation is not None:
                    bundle = setup_preparation.checked_bundle(receipt)
                return artifact, selection, bundle

            artifact, output_selection, setup_bundle = await self._current_filesystem(checked_outputs,
                _owner_call=_owner_call)
        except asyncio.CancelledError:
            cancelled = True
            status, artifact, error_type = "outcome_unknown", None, "CancelledError"
            output_selection = None
            setup_bundle = None
        except Exception as error:
            error_type = type(error).__name__
            if status != "source_changed":
                status = "outcome_unknown"
            artifact = None
            output_selection = None
            setup_bundle = None
        finally:
            def retain_completion():
                retained_process = None
                if (isinstance(process, ProcessOutcome) and type(process.returncode) is int
                        and isinstance(process.stdout, bytes) and isinstance(process.stderr, bytes)
                        and len(process.stdout) <= MAX_RECEIPT_BYTES and len(process.stderr) <= MAX_RECEIPT_BYTES):
                    retained_process = {"returncode": process.returncode,
                        "stdout_base64": base64.b64encode(process.stdout).decode("ascii"),
                        "stderr_base64": base64.b64encode(process.stderr).decode("ascii")}
                # This separate immutable completion can be missing after a crash.
                # Never remove the entered record or replace a completion writer.
                _write_new(completion_path, {"status": status,
                    "receipt": receipt, "artifact": artifact.pin() if artifact else None,
                    "process": retained_process, "error_type": error_type,
                    **({"selection": output_selection} if preparation is not None else {}),
                    **({"setup_bundle": setup_bundle.pins() if setup_bundle else None}
                       if setup_preparation is not None else {})})
            _, interrupted = await _joined_filesystem(retain_completion)
            cancelled = cancelled or interrupted
        if cancelled:
            raise asyncio.CancelledError
        # The completion is historical. A lease change during its fsync cannot
        # make the returned artifact/selection current or rewrite that receipt.
        try:
            await self._current_owner(_owner_call)
            if expected_gateway is not None and self.gateway != expected_gateway:
                raise CustodyError("The native compiler changed its retained inventory gateway.")
            # Every supported source path retains its original pins, including
            # explicit native identities without an inventory carrier. The
            # immutable completion is historical if any input changed during
            # its awaited fsync; withhold the current artifact or selection.
            await self._current_filesystem(require_inputs, _owner_call=_owner_call)
            if self.gateway != pins[0]:
                raise CustodyError("The native caller changed its selected gateway.")
        except Exception:
            return InstallerOutcome("source_changed", receipt, None, attempt_path)
        return InstallerOutcome(status, receipt, artifact, attempt_path, output_selection, setup_bundle)
