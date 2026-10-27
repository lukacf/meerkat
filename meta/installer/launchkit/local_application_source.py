"""Select protected local application source during one deliberate initial Build.

The trusted Build caller supplies its original store, current guide version,
qualified product target and fresh private output directory. These files are
application declarations in existing source formats. They do not authenticate
a Console user, issue native grants, prepare an installation or qualify a target.
Importing this module and calling an ordinary read supplier produce no files.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import tomllib

from . import compiler
from .blueprint import Blueprint
from .connect_application_callbacks import _source_definition
from .connect_application_finalizer import _sync_directory, _write_bytes
from .connect_installer_caller import (
    CustodyError, ExactFile, MAX_INPUT_BYTES, _inventory_input, _path, _private_directory,
    _private_parent,
)
from .connect_policy_input import application_tool_policy_selection
from .guide_store import GuideStore
from .local_application_factory import LocalApplicationFactory, _replacement_supplier
from .runtime_selection import (
    MetaToolkitRuntimeTarget, implementation_candidate_selection, selected_runtime_artifacts,
    selected_runtime_version,
)
from .templates_v2.application_configuration import validate_application_fields
from .templates_v2.console_configuration import console_authentication


def _json_bytes(value):
    return (json.dumps(value, ensure_ascii=True, allow_nan=False,
        separators=(",", ":")) + "\n").encode("utf-8")


def _application_acquisition_selection(pins, compiler_sources):
    """Use the existing private source format for a trusted original owner."""
    return {"blueprint": pins["blueprint.json"].pin(), "definition": pins["mob.toml"].pin(),
        "original_inventory": pins["application-inventory-input.json"].pin(),
        "source_policy": pins["tool-policy.json"].pin(), "source_compiler": list(compiler_sources)}


def _application_viewer_selection(pins, acquisition, operator):
    return {"base_source_selection": acquisition.pin(), "operator": operator,
        "source_inputs": {"application/configuration.json": pins["configuration.json"].pin(),
            "application/console.json": pins["console.json"].pin(), "config/access.toml": pins["access.toml"].pin()}}


def _store_identity(store):
    """Check presence before GuideStore's creating database read helper."""
    try:
        root = _private_directory(store.root)
        path = _path(store.root / "guided-setups.sqlite3")
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            info, current = os.fstat(descriptor), path.lstat()
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_nlink != 1 or info.st_mode & 0o077
                    or (info.st_dev, info.st_ino) != (current.st_dev, current.st_ino)):
                raise CustodyError("The original guide database must be private, owned and ordinary.")
            return root, (info.st_dev, info.st_ino)
        finally:
            os.close(descriptor)
    except OSError as error:
        raise CustodyError("The original guide storage is missing or changed.") from error


def _guide(store, guide_id, version, storage):
    def current():
        if _store_identity(store) != storage:
            raise CustodyError("The original guide storage changed during source selection.")

    current()
    state = store.get(guide_id)
    current()
    operation = store.operation(guide_id)
    current()
    if (not isinstance(state, dict) or state.get("id") != guide_id
            or type(state.get("version")) is not int or state["version"] != version
            or operation is not None and (not isinstance(operation, dict)
                or operation.get("status") == "running")):
        raise CustodyError("Initial Build requires this guide's exact current completed version.")
    selected = state.get("proposal") if state.get("proposal") is not None else state.get("blueprint")
    return Blueprint.model_validate(selected)


def _compiler_sources():
    """Observe the maintained renderer and its source/model declarations."""
    result = []
    for name in ("compiler.py", "blueprint.py", "native_models.py", "native_definition_continuity.py",
            "application_inventory_source.py"):
        path = _path(Path(compiler.__file__).parent / name)
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            before = os.fstat(stream.fileno())
            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                    or before.st_nlink != 1 or before.st_mode & 0o022
                    or not 0 < before.st_size <= MAX_INPUT_BYTES):
                raise CustodyError("The selected compiler source must remain owned, ordinary and bounded.")
            content = stream.read(MAX_INPUT_BYTES + 1)
            after, current = os.fstat(stream.fileno()), path.lstat()
            def identity(info):
                return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
                    info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
            if (len(content) != before.st_size or identity(before) != identity(after)
                    or identity(current) != identity(after)):
                raise CustodyError("The selected compiler source changed during observation.")
        result.append(({"path": str(path), "sha256": hashlib.sha256(content).hexdigest(),
            "bytes": len(content)}, identity(after)))
    return tuple(result)


_CHECKED_SETUP_CONSEQUENCES = {
    # Exact selected Connect setup protocol operations. Consequences describe
    # effects; native catalog/MCP witnesses and protected reviews own authority.
    "tool_catalog_search": "r0", "tool_catalog_load": "r0",
    "setup_state": "r0", "setup_jobs": "r0", "setup_job": "r0",
    "setup_catalog": "r0", "setup_catalog_entry": "r0", "setup_catalog_route": "r0",
    "setup_help": "r0", "setup_recovery_manifest": "r0", "setup_guide": "r0",
    "setup_inspect_package": "r0", "setup_inspect_schema": "r0", "setup_inspect_acquired": "r0",
    "setup_recipes": "r0", "setup_recipe": "r0",
    "setup_prepare": "r1", "setup_retry_prepare": "r1", "setup_check": "r1",
    "setup_stop": "r1", "setup_reconcile": "r1", "setup_acquire_python": "r1",
    "setup_search_node": "r1", "setup_acquire_node": "r1", "setup_remember_remote": "r1",
    "setup_acquire_node_prebuilt": "r1",
    "setup_export_recipe": "r1", "setup_selected_check": "r1",
    "setup_install": "r2", "setup_selected_install": "r2",
}


def _initial_policy(spec, inventory, guide_id):
    """Declare only the existing setup entrypoints selected by this plan.

    Every durable member has an explicit row. The dedicated helper can request
    public-service onboarding, optionally request protected account onboarding,
    and use its selected finite
    setup catalog/MCP operations. Other callbacks and member scopes stay absent.
    Native policy compilation and exact tool association remain mandatory.
    """
    bundled = {identity for item in spec.agent_classes
        if "meerkat_toolkit.setup" in item.tools.rust_bundles for identity in item.identities()}
    helper_identity = inventory.get("helper_identity")
    members = []
    for row in inventory["roster"]:
        identity = row["identity"]
        names = ("setup_report", "toolkit_request_public_service", "toolkit_request_service_account") if identity == helper_identity else (
            ("find_and_install",) if identity in bundled else ())
        if identity == helper_identity and not spec.acquisition.account_requests:
            names = tuple(name for name in names if name != "toolkit_request_service_account")
        grants = [{"tool_name": name, "action": "invoke", "consequence": "r1"} for name in names]
        if identity == helper_identity:
            grants += [{"tool_name": name, "action": "invoke", "consequence": consequence}
                for name, consequence in _CHECKED_SETUP_CONSEQUENCES.items()]
            if spec.acquisition.public_research:
                # Invocation authority stays with the selected native owner.
                # Existing setup operations retain their exact names/effects.
                grants += [{"tool_name": name, "action": "invoke", "consequence": "r1"}
                    for name in ("setup_search_github", "setup_read_github")]
        members.append({"member_identity": identity, "grants": grants})
    value = {"provider_id": "meta-forge:" + guide_id,
        "policy_id": "application-tools:" + guide_id, "revision": 1, "members": members}
    content = _json_bytes(value)
    application_tool_policy_selection(content,
        selected_member_identities=tuple(row["identity"] for row in inventory["roster"]))
    return content


ORDINARY_SOURCE_UNAVAILABLE = (
    "This Forge was started without the Meta RPC gateway that an application without a "
    "connection setup helper needs. Ask the package owner for a Forge package containing "
    "the gateway matching its application Meta SDK. Your plan is preserved.")
ORDINARY_TARGET_UNSUPPORTED = (
    "An application without a connection setup helper can be built here only for a Mac "
    "or Linux destination. Change the plan's destination, or add the connection setup "
    "helper. Your plan is preserved.")


def _missing_source_input(spec):
    """Name the actual missing input; only a helper plan is pending setup support."""
    if spec.acquisition is None:
        if any(row.tools.rust_bundles for row in spec.agent_classes):
            return compiler.NATIVE_BUNDLE_PENDING
        if spec.target not in {"mac", "linux"}:
            return ORDINARY_TARGET_UNSUPPORTED
        return ORDINARY_SOURCE_UNAVAILABLE
    return compiler.CONNECTION_SETUP_PENDING


def create_local_application_source(*, guide_store, guide_id, guide_version,
        runtime_target, output_directory, model_configuration=None, replacement_selection=None,
        configuration_source=None):
    """Return one real source factory after publishing and checking its source.

    The existing initial Build owner selects this fresh private directory. No
    HTTP or saved package value may select runtime_target. A changed proposal
    needs a new source owner; an interrupted output is preserved and not reused.
    This function has no Install, inventory, Fresh or runtime side effects.
    A trusted Linux product may construct an unchanged GCP source plan; the
    local source factory does not supply its destination installation owner.
    """
    _replacement_supplier(replacement_selection)
    if configuration_source is not None and type(configuration_source) is not ExactFile:
        raise CustodyError("Application configuration needs an original exact declaration file.")
    configuration_bytes = None
    if configuration_source is not None:
        from .connect_application_update_startup import _observe, _file_identity
        _private_parent(configuration_source.path)
        configuration_bytes, configuration_identity = _observe(configuration_source)
        configuration_pin = configuration_source.pin()
    if (type(guide_store) is not GuideStore or type(runtime_target) is not MetaToolkitRuntimeTarget
            or type(guide_version) is not int or guide_version < 1):
        raise CustodyError("Initial source selection needs its actual guide store, version and trusted product.")
    guide_store.identity(guide_id)
    storage = _store_identity(guide_store)
    spec = _guide(guide_store, guide_id, guide_version, storage)
    selected_plan = spec.model_dump(mode="json")
    if not compiler._product_application_supported(spec, runtime_target=runtime_target):
        raise CustodyError(_missing_source_input(spec))
    sources = _compiler_sources()
    artifacts = selected_runtime_artifacts(runtime_target)
    availability = compiler.package_availability(spec, runtime_target=runtime_target,
        verified_artifacts=artifacts)
    if not availability["available"]:
        raise CustodyError(availability["reason"])
    authored_inventory = compiler.requires_authored_inventory(spec)
    inventory_bytes = compiler.application_inventory_input(spec,
        **({"member_classes": []} if authored_inventory else {}))
    inventory = _inventory_input(inventory_bytes)
    definition = compiler.mob_toml(spec, native_version=selected_runtime_version(runtime_target),
        runtime_target=runtime_target, verified_artifacts=artifacts)
    _source_definition({"format": "toml", "text": definition})
    policy = _initial_policy(spec, inventory, guide_id)
    # This is a new logical application source identity, not an existing user
    # discovered from the machine or an authenticated principal.
    subject = "forge-" + guide_id
    console = {"authentication": "local-password"}
    configuration = {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user",
            "description": "Use " + subject + ", the user selected for this application. Enter it in protected Install settings.",
            "secret": False, "required": True},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password",
            "description": "Choose a password for this application in protected Install settings. Keep it out of the conversation.",
            "secret": True, "required": True, "min_length": 12, "max_length": 4096}]}
    if configuration_bytes is not None:
        # Keep the owner's complete declaration bytes, including its Console
        # fields. Existing validators reject secret defaults and host overrides.
        configuration = json.loads(configuration_bytes.decode("utf-8"))
    console_authentication(console, validate_application_fields(configuration,
        existing_keys=tuple(row["key"] for row in compiler.environment_fields(spec))))
    access = "enabled = true\nadmins = [" + json.dumps(subject) + "]\nrules = []\n\n[groups]\n"
    if tomllib.loads(access) != {"enabled": True, "admins": [subject], "rules": [], "groups": {}}:
        raise CustodyError("The selected protected Console source changed its operator association.")
    contents = {"blueprint.json": _json_bytes(selected_plan), "mob.toml": definition.encode("utf-8"),
        "application-inventory-input.json": inventory_bytes, "tool-policy.json": policy,
        "configuration.json": _json_bytes(configuration), "console.json": _json_bytes(console),
        "access.toml": access.encode("utf-8")}
    if configuration_source is not None:
        del contents["configuration.json"]
    if any(not 0 < len(content) <= MAX_INPUT_BYTES for content in contents.values()):
        raise CustodyError("The selected application source exceeds the existing private input bound.")
    output = _path(Path(output_directory), fresh=True)
    _private_parent(output)
    parent = _private_directory(output.parent)

    def current():
        if configuration_source is not None:
            _private_parent(configuration_source.path)
            if (configuration_source.pin() != configuration_pin
                    or _file_identity(configuration_source.path) != configuration_identity
                    or configuration_source.read() != configuration_bytes
                    or _file_identity(configuration_source.path) != configuration_identity):
                raise CustodyError("The original application configuration declarations changed during Build.")
        if (_guide(guide_store, guide_id, guide_version, storage).model_dump(mode="json") != selected_plan
                or _compiler_sources() != sources or _private_directory(output.parent) != parent):
            raise CustodyError("The original guide, compiler or private output owner changed during Build.")

    current()
    implementation_candidate_selection(runtime_target, deployment_target=spec.target, verified_artifacts=artifacts)
    output.mkdir(mode=0o700)
    _sync_directory(output.parent)
    directory = _private_directory(output)

    def write(name, content):
        current()
        if _private_directory(output) != directory:
            raise CustodyError("The original protected source output directory was replaced.")
        pin = _write_bytes(output / name, content)
        current()
        if _private_directory(output) != directory:
            raise CustodyError("The original protected source output directory changed during publication.")
        return pin

    pins = {name: write(name, content) for name, content in contents.items()}
    if configuration_source is not None:
        pins["configuration.json"] = configuration_source
    current()
    acquisition = _application_acquisition_selection(pins, [item[0] for item in sources])
    if authored_inventory:
        from .application_inventory_source import INVENTORY_PATH
        acquisition["accepted_inventory_path"] = INVENTORY_PATH
    acquisition_pin = write("source-selection.json", _json_bytes(acquisition))
    viewer = _application_viewer_selection(pins, acquisition_pin,
        {"subject": subject, "private_configuration_key": "CONSOLE_USER_ID"})
    viewer_pin = write("APPLICATION-SELECTION.json", _json_bytes(viewer))
    factory = LocalApplicationFactory(guide_store=guide_store, guide_id=guide_id, runtime_target=runtime_target,
        acquisition_selection=acquisition_pin, viewer_selection=viewer_pin,
        model_configuration=model_configuration, replacement_selection=replacement_selection)
    factory()  # Retain the same host only after all original source readers pass.
    current()
    implementation_candidate_selection(runtime_target, deployment_target=spec.target, verified_artifacts=artifacts)
    if _private_directory(output) != directory:
        raise CustodyError("The original protected source output directory changed before handoff.")
    factory.host.require_current()
    return factory
