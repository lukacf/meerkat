"""Select one original completed Fresh output for a later Restore action.

The claimed action and original stopped installation own this publication.
Reads never issue native commands, replay Fresh, infer Ready or repair history.
The separate local preparation owner retains the returned exact envelope.
"""
from __future__ import annotations

import base64
from contextlib import asynccontextmanager, nullcontext
from copy import deepcopy
import hashlib
import json
from pathlib import Path

from .connect_application_finalizer import _Current, _write_bytes
from .connect_application_policy import _completed_application_policy_inputs_files
from .connect_application_update_startup import _file_identity, _observe
from .connect_installer_caller import (CustodyError, ExactFile, MAX_INPUT_BYTES,
    MAX_RECEIPT_BYTES, NativeInstallerCaller, ProcessOutcome, _Preparation,
    _artifact_pin, _json, _path, _private_directory, _read, _receipt,
    _same_json_value, _selection, supervisor_executor)
from .deployment import DeploymentService, read_receipt
from .deployment_application_bootstrap import (BootstrapObservation, FRESH_ENTRY_NAME,
    _observe_inputs, _shape, read_original_fresh_admission, require_restore_history_current)
from .deployment_supervisor import pending_custody
from .deployment_updates import require_replacement_history_settled
from .local_application_preparation_inputs import LocalApplicationPreparationOwner
from .remote_product_artifacts import _joined


def _reference(bundle, value):
    """Keep native artifact names and hashes, without interpreting policy."""
    if (not isinstance(value, dict) or set(value) != {"name", "sha256"}
            or not isinstance(value["name"], str) or Path(value["name"]).name != value["name"]
            or value["name"] in {"", ".", ".."}):
        raise CustodyError("The native policy lost its exact bundled artifact reference.")
    return ExactFile(_path(bundle / value["name"]), value["sha256"])


def _verified_process(attempt, completion, *, argv, pins, command, kind, selection):
    if (not isinstance(attempt, dict) or set(attempt) != {"status", "argv", "input_files", "output_path"}
            or attempt["status"] != "entered" or attempt["argv"] != list(argv)
            or attempt["input_files"] != [pin.pin() for pin in pins] or attempt["output_path"] is not None):
        raise CustodyError("The original finalization command or exact input closure changed.")
    expected = {"status", "receipt", "artifact", "process", "error_type"}
    if command == "--prepare-application-policy":
        expected.add("selection")
    if (not isinstance(completion, dict) or set(completion) != expected
            or completion["status"] != "receipt_verified" or completion["error_type"] is not None
            or completion["artifact"] is not None or not isinstance(completion["process"], dict)
            or set(completion["process"]) != {"returncode", "stdout_base64", "stderr_base64"}):
        raise CustodyError("The original native finalization has no verified exited outcome.")
    process = completion["process"]
    try:
        streams = []
        for name in ("stdout_base64", "stderr_base64"):
            if not isinstance(process[name], str) or len(process[name]) > 4 * ((MAX_RECEIPT_BYTES + 2) // 3):
                raise ValueError("The retained process stream exceeds its bound.")
            streams.append(base64.b64decode(process[name], validate=True))
        outcome = ProcessOutcome(process["returncode"], *streams)
    except (TypeError, ValueError) as error:
        raise CustodyError("The original finalization lost its exact exited process streams.") from error
    status, receipt = _receipt(outcome, command, kind,
        selection if command == "--validate-application-policy" else None)
    if (status != "receipt_verified" or not _same_json_value(receipt, completion["receipt"])
            or not _same_json_value(receipt["selection"], selection)
            or command == "--prepare-application-policy"
                and not _same_json_value(completion["selection"], selection)):
        raise CustodyError("The retained finalization outcome differs from its original native receipt.")


def _completed_restore(owner, directory, receipt, fresh, lease, source_custody):
    """Authenticate the original finalizer in a joined filesystem-only lane."""
    original_receipt = _file_identity(directory / "receipt.json")
    snapshots = {}

    def files_current():
        lease.require_current()
        if source_custody is not None:
            source_custody.require_filesystem_current()
        if (_file_identity(directory / "receipt.json") != original_receipt
                or not _same_json_value(read_receipt(directory), receipt)):
            raise CustodyError("The stopped installation changed during Restore selection.")
        if pending_custody(directory, receipt):
            raise CustodyError("The original installation support custody remains unsettled.")
        require_replacement_history_settled(directory)

    def observe(pin):
        content, identity = _observe(pin)
        previous = snapshots.setdefault(pin.path, identity)
        if previous != identity:
            raise CustodyError("The original finalization bytes changed during observation.")
        return content

    def record(path, limit=MAX_INPUT_BYTES):
        raw = _read(_path(path), limit)
        pin = ExactFile(path, hashlib.sha256(raw).hexdigest())
        if observe(pin) != raw:
            raise CustodyError("The original finalization record changed during observation.")
        return pin, _json(raw)

    files_current()
    entry_path = directory / FRESH_ENTRY_NAME
    _, entry = record(entry_path, 16 * 1024)
    if (not isinstance(entry, dict) or set(entry) != {"kind", "admission", "provision_attempt_path"}
            or entry["kind"] != "application-bootstrap-entry"
            or entry["provision_attempt_path"] != fresh["provision_attempt_path"]):
        raise CustodyError("Restore lost its original entered Fresh selection.")
    admission = _artifact_pin(entry["admission"])
    observe(admission)
    original = read_original_fresh_admission(directory, entry["admission"])
    if not _same_json_value(original["application_startup"], fresh):
        raise CustodyError("Restore cannot replace the original local Fresh input selection.")
    fresh_observation = _observe_inputs(directory, receipt, fresh, entered=True,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    paths = {name: _path(fresh[name]) for name in (
        "working_directory", "bundle_path", "activation_path", "provision_attempt_path")}
    work, root = paths["working_directory"], lease.root
    _private_directory(work)
    snapshots[work] = _file_identity(work)
    plan, policy, template = (_artifact_pin(fresh[name]) for name in (
        "preparation_plan", "source_policy", "activation_template"))
    for pin in (plan, policy, template):
        observe(pin)
    _, finalization = record(work / "finalization.json")
    expected_sources = [policy.pin(), template.pin()]
    if (not isinstance(finalization, dict) or set(finalization) != {"status", "preparation_plan",
            "completed_manifest", "source_files", "original_current", "bundle", "activation"}
            or finalization["status"] != "entered" or finalization["preparation_plan"] != plan.pin()
            or finalization["source_files"] != expected_sources
            or finalization["bundle"] != str(paths["bundle_path"])
            or finalization["activation"] != str(paths["activation_path"])):
        raise CustodyError("The original finalizer lost its exact local Fresh inputs and output paths.")
    prior = finalization["original_current"]
    if (not isinstance(prior, dict) or set(prior) != {"target", "identity", "activation"}
            or prior["activation"] is None and prior != {"target": None, "identity": None, "activation": None}):
        raise CustodyError("The original finalizer lost its historical current pointer observation.")
    if prior["activation"] is not None:
        prior_pin = _artifact_pin(prior["activation"])
        if prior_pin.path.parent != root:
            raise CustodyError("The original activation history is outside its installation.")
        observe(prior_pin)
    manifest = _artifact_pin(finalization["completed_manifest"])
    observe(manifest)
    # This existing caller is used only for its file-only completed-bundle
    # reader. Its executor remains the one actual inactive Install Supervisor.
    caller = NativeInstallerCaller(fresh_observation.gateway,
        execute=supervisor_executor(owner._inputs.supervisor, timeout=owner._inputs.command_timeout),
        require_current=files_current, require_filesystem_current=files_current,
        provision_attempt_path=paths["provision_attempt_path"])
    bundle = caller._retain_completed_application_preparation_files(manifest,
        preparation_plan=plan, source_files=(policy, template))
    projected = _completed_application_policy_inputs_files(bundle, policy)
    policy_pin = ExactFile(work / "final-local-policy.json",
        hashlib.sha256(projected["local_policy_input"]).hexdigest())
    observe(policy_pin)
    artifacts_root = work / "artifacts"
    _private_directory(artifacts_root)
    snapshots[artifacts_root] = _file_identity(artifacts_root)
    acquisition = projected["acquisition"]
    artifacts = tuple(ExactFile(artifacts_root / name, pin.sha256)
        for pin, name in zip(projected["source_artifacts"], projected["source_artifact_names"], strict=True))
    artifacts += (ExactFile(artifacts_root / acquisition.path,
        hashlib.sha256(acquisition.content.encode()).hexdigest()),)
    if {path.name for path in artifacts_root.iterdir()} != {pin.path.name for pin in artifacts}:
        raise CustodyError("The original policy staging artifacts contain missing or unselected bytes.")
    native_pins = (bundle.manifest, bundle.setup, bundle.application_tool_policy,
        *((bundle.resource_handoff,) if bundle.resource_handoff is not None else ()),
        *(pin for pair in bundle.member_surfaces for pin in pair))
    for pin in (*native_pins, *artifacts):
        observe(pin)
    selection_file, selected = record(work / "selection.json", 64 * 1024)
    selected, output_pins = _selection(selection_file.read())
    preparation = _Preparation(_private_directory(root), _private_directory(artifacts_root),
        paths["bundle_path"], paths["activation_path"], artifacts, template)
    for path in (bundle.directory[0], bundle.artifact_directory[0], paths["bundle_path"]):
        _private_directory(path)
        snapshots[path] = _file_identity(path)
    preparation.checked_selection(selected)
    for pin in (*output_pins, *(ExactFile(paths["bundle_path"] / pin.path.name, pin.sha256)
            for pin in artifacts)):
        observe(pin)
    source_files = (plan, bundle.manifest, policy, template)
    _, prepare = record(work / "prepare.json")
    _, prepared = record(work / "prepare.json.outcome.json")
    argv = (str(fresh_observation.gateway.path), "--prepare-application-policy",
        "--policy", str(policy_pin.path), "--artifacts", str(artifacts_root),
        "--deployment-root", str(root), "--bundle", str(paths["bundle_path"]),
        "--activation-template", str(template.path), "--activation", str(paths["activation_path"]))
    _verified_process(prepare, prepared, argv=argv,
        pins=(fresh_observation.gateway, policy_pin, template, *artifacts, *source_files),
        command="--prepare-application-policy", kind="application_policy_prepared", selection=selected)
    _, validate = record(work / "validate.json")
    _, validated = record(work / "validate.json.outcome.json")
    _verified_process(validate, validated,
        argv=(str(fresh_observation.gateway.path), "--validate-application-policy", "--selection", str(selection_file.path)),
        pins=(fresh_observation.gateway, selection_file, *output_pins, *source_files, *artifacts),
        command="--validate-application-policy", kind="application_policy_validated", selection=selected)
    _, publication = record(work / "publication.json")
    _, published = record(work / "finalization.outcome.json")
    if (not isinstance(publication, dict) or set(publication) != {"status", "selection", "original_current"}
            or publication["status"] != "entered" or not _same_json_value(publication["selection"], selected)
            or not _same_json_value(publication["original_current"], prior)
            or not isinstance(published, dict) or set(published) != {
                "status", "error_type", "publication_entered", "selection"}
            or published["status"] != "published" or published["error_type"] is not None
            or published["publication_entered"] is not True
            or not _same_json_value(published["selection"], selected)
            or _Current.observe(root).activation != output_pins[0]):
        raise CustodyError("The original Fresh finalization did not publish this exact current activation.")
    local_policy = _json(observe(output_pins[2]))
    native = local_policy.get("native_host") if isinstance(local_policy, dict) else None
    if not isinstance(native, dict):
        raise CustodyError("The actual native local policy lacks its original setup reference.")
    acquisition_pin = _reference(paths["bundle_path"], native.get("setup"))
    acquired_bytes = observe(acquisition_pin)
    acquired = _json(acquired_bytes)
    if acquired_bytes != acquisition.as_bytes():
        raise CustodyError("The selected native acquisition artifact lost its original shape.")
    setup = _reference(paths["bundle_path"], acquired.get("prepared_application_setup"))
    if setup.sha256 != bundle.setup.sha256 or setup.path.name != bundle.setup.path.name:
        raise CustodyError("Restore selected a different completed native application setup.")
    observe(setup)
    value = {"kind": "restore", "gateway": fresh["gateway"], "selection_file": selection_file.pin(),
        "preparation_plan": fresh["preparation_plan"], "completed_setup": setup.pin()}
    if "model_configuration" in fresh:
        value["model_configuration"] = deepcopy(fresh["model_configuration"])
    restored = _observe_inputs(directory, receipt, value,
        **({"source_custody": source_custody} if source_custody is not None else {}))
    require_restore_history_current(directory, value)
    for path in (work, artifacts_root, bundle.directory[0], bundle.artifact_directory[0], paths["bundle_path"]):
        _private_directory(path)
        snapshots.setdefault(path, _file_identity(path))
    observed = BootstrapObservation(restored.directories,
        (*fresh_observation.files, *restored.files, *snapshots.items()),
        restored.current, root, gateway=restored.gateway,
        model_configuration=restored.model_configuration)
    bundle._authenticate_files()
    preparation.checked_selection(selected)
    files_current()
    observed.require_current()
    return value, observed


@asynccontextmanager
async def _restore_source(directory, lease, current, fresh, source):
    """Use the real destination operation's source, or the local own scope."""
    if source is None:
        from .product_installed_source import open_installed_product_source
        async with open_installed_product_source(directory, project_lease=lease,
                require_current=current, application_startup=fresh) as selected:
            yield selected
        return
    from .product_runtime_custody import (await_product_validation,
        product_source_custody, require_product_installation)
    source = product_source_custody(source)
    source.require_owner_current()
    if source._lease is not lease:
        raise CustodyError("Restore must retain its same actual destination project lease.")
    await await_product_validation(source, require_product_installation, source, read_receipt(directory))
    current()
    yield source
    current()
    source.require_owner_current()


async def retain_local_restore(owner, deployment, guide_id, deployment_id):
    """Action-owned read/publication, never a GET or native startup supplier."""
    if (type(owner) is not LocalApplicationPreparationOwner or not isinstance(deployment, DeploymentService)
            or not deployment.lock.locked() or owner.factory.preparation_owner is not owner
            or owner.factory.host is not owner.host or guide_id != owner.factory.guide_id):
        raise CustodyError("Restore selection needs its original local owner and claimed deployment action.")
    directory = deployment._directory(guide_id, deployment_id)
    if not await _joined(lambda: (directory / FRESH_ENTRY_NAME).exists()
            or (directory / FRESH_ENTRY_NAME).is_symlink(), _owner_current=owner.host.require_current):
        return None
    owner._require(guide_id, deployment_id)
    if (owner._fresh_pin is None or owner._fresh_value is None
            or owner._inputs is not owner.host._retained_preparation_inputs
            or _shape(owner._fresh_value) != "fresh"
            or not _same_json_value(_json(owner._fresh_pin.read()), owner._fresh_value)):
        raise CustodyError("Restore needs this owner's original retained Fresh input pin.")
    if deployment_id in deployment._application_preparations:
        raise CustodyError("Settle the original preparation cleanup before retaining Restore.")
    def inactive():
        selected = owner.host._selected_preparation(deployment, guide_id, deployment_id,
            restore_inputs=owner._inputs)
        if (owner.host._retained_preparation_inputs is not owner._inputs
                or selected["supervisor"] is not owner._inputs.supervisor):
            raise CustodyError("Restore lost its same original inactive preparation Supervisor.")
    inactive()
    directory, receipt, stopped = deployment._stopped_application_owner(guide_id, deployment_id)
    if directory != owner._inputs.supervisor.directory:
        raise CustodyError("Restore cannot adopt another installed source owner.")
    source_custody = owner.host._product_source_custody
    if source_custody is not None:
        from .destination_application_inputs import DestinationApplicationInputs
        from .product_runtime_custody import product_source_custody
        source_custody = product_source_custody(source_custody)
        if (type(owner.factory) is not DestinationApplicationInputs
                or owner.factory.factory._source is not source_custody
                or owner.factory.factory.host is not owner.host):
            raise CustodyError("Restore needs its actual destination input owner and entered source operation.")
        source_custody.require_owner_current()
    fresh = deepcopy(owner._fresh_value)
    from .templates.storage_setup import project_lock
    context = (project_lock(owner._inputs.supervisor.destination) if source_custody is None
        else nullcontext(source_custody._lease))
    with context as lease:
        def current():
            stopped()
            lease.require_current()
            owner._require(guide_id, deployment_id)
            inactive()
            if (deployment_id in deployment._application_preparations
                    or not _same_json_value(_json(owner._fresh_pin.read()), fresh)):
                raise CustodyError("The original local Fresh or preparation owner changed.")
        current()
        async with _restore_source(directory, lease, current, fresh, source_custody) as source:
            current()
            value, observed = await _joined(_completed_restore, owner, directory, deepcopy(receipt),
                fresh, lease, source, _owner_current=current)
            current()
            observed.require_current()
            path = owner._inputs.startup_selection_path.parent / "restore-selection.json"
            content = (json.dumps(value, ensure_ascii=True, allow_nan=False,
                sort_keys=True, separators=(",", ":")) + "\n").encode()
            def publish():
                lease.require_current()
                if source is not None:
                    source.require_filesystem_current()
                observed.require_current()
                require_restore_history_current(directory, value)
                # Cancellation may have joined a successful exclusive write
                # before its original action could retain the returned pin.
                # Reentry may adopt only these fully authenticated exact bytes
                # at this same selected path. Partial or foreign bytes refuse.
                if path.exists() or path.is_symlink():
                    pin = ExactFile(path, hashlib.sha256(content).hexdigest())
                else:
                    pin = _write_bytes(path, content)
                actual, identity = _observe(pin)
                if actual != content:
                    raise CustodyError("The original Restore handoff contains different bytes.")
                observed.require_current()
                if _file_identity(path) != identity:
                    raise CustodyError("The original Restore handoff changed during retention.")
                lease.require_current()
                return pin, identity
            pin, identity = await _joined(publish, _owner_current=current)
            current()
            observed.require_current()
            require_restore_history_current(directory, value)
            if _file_identity(path) != identity:
                raise CustodyError("The original Restore handoff changed before its action retained it.")
            pin.read()
            return pin
