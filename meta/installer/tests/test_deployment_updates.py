"""Source, ownership and local-state boundaries of the installer update transaction."""
import asyncio
import copy
import io
import json
import os
import stat
import uuid
import zipfile

import pytest

from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit import deployment_updates as updates
from launchkit.deployment_worker import Supervisor, update_runtime
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit


def canonical_update_package(blueprint):
    from launchkit.compiler import generate_package
    generated = generate_package(blueprint)
    files = {item.path: (item.content.encode(), item.mode) for item in generated}
    baseline = {item.path: {"sha256": updates.digest(item.content.encode()), "mode": item.mode} for item in generated}
    fingerprint = updates.digest(json.dumps(baseline, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
    blueprint_hash = updates.digest(json.dumps(json.loads(files["blueprint.json"][0]),
        ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode())
    report = {"framework": {"kind": "host-framework-provenance", "version": 1,
        "blueprint_digest": blueprint_hash, "basis": "current-compiler",
        "baseline_digest": fingerprint, "current_compiler_baseline_digest": fingerprint,
        "retained_template_paths": []}}
    files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    return files


def revised_role_blueprint():
    from launchkit.blueprint import starter_blueprint
    before = starter_blueprint().model_dump(mode="json")
    after = copy.deepcopy(before)
    after["agent_classes"][0]["instructions"] += "\nTreat facts as case-scoped evidence and preserve earlier records."
    if after["flows"]:
        after["flows"][0]["steps"][0]["prompt"] += "\nUse current case permissions."
    return before, after


def test_canonical_role_and_existing_flow_messages_are_narrowly_compatible_with_current_host():
    before, after = revised_role_blueprint()
    previous, candidate = canonical_update_package(before), canonical_update_package(after)
    assert updates.instruction_configuration_update(previous, candidate)
    assert updates.has_host_instruction_carrier(candidate)
    changes, blockers = updates.compatibility(previous, candidate, {})
    assert not blockers
    assert {item["path"] for item in changes} >= {"blueprint.json", "deployment.json", "config/mob.toml"}


def test_formatting_only_native_change_is_not_a_definition_activation():
    before, _ = revised_role_blueprint()
    previous = canonical_update_package(before)
    candidate = dict(previous)
    candidate["config/mob.toml"] = (previous["config/mob.toml"][0] + b"\n# Formatting only\n", 0o644)
    assert not updates.instruction_configuration_update(previous, candidate)
    assert updates.compatibility(previous, candidate, {})[1]


@pytest.mark.parametrize("mutation", ["identity", "model", "provider", "grant", "instances", "member", "topology", "port", "knowledge", "auth", "native-skill", "flow-structure", "mode", "bool-type"])
def test_role_text_never_masks_native_authority_or_projection_changes(mutation):
    before, after = revised_role_blueprint()
    previous = canonical_update_package(before)
    if mutation == "model":
        after["agent_classes"][0]["model"] = "gpt-6-luna"
    elif mutation == "provider":
        after["agent_classes"][0]["provider"] = "anthropic"
        after["agent_classes"][0]["model"] = "claude-sonnet-4-6"
    elif mutation == "grant":
        after["agent_classes"][0]["tools"]["shell"] = not after["agent_classes"][0]["tools"]["shell"]
    elif mutation == "port":
        after["advanced"]["port"] += 1
    candidate = canonical_update_package(after)
    if mutation in {"instances", "knowledge"}:
        value = json.loads(candidate["blueprint.json"][0])
        if mutation == "instances":
            value["agent_classes"][0]["instances"] += 1
        else:
            value["knowledge"]["space_env"] = "OTHER_KNOWLEDGE_SPACE_ID"
        candidate["blueprint.json"] = (json.dumps(value).encode(), 0o644)
    if mutation in {"identity", "member", "topology", "bool-type"}:
        value = json.loads(candidate["deployment.json"][0])
        if mutation == "identity":
            value["agents"][0]["identity"] = "replacement:main"
        elif mutation == "member":
            value["agents"][0]["name"] = "Different person"
        elif mutation == "topology":
            value["connections"] = []
        else:
            value["classes"][0]["tools"]["shell"] = int(value["classes"][0]["tools"]["shell"])
        candidate["deployment.json"] = (json.dumps(value).encode(), 0o644)
    elif mutation == "auth":
        candidate["config/mob.toml"] = (candidate["config/mob.toml"][0] + b'\n[auth]\nmode="changed"\n', 0o644)
    elif mutation == "native-skill":
        native = candidate["config/mob.toml"][0].decode()
        candidate["config/mob.toml"] = (native.replace('[skills.launch_comms]', '[skills.alternate_comms]').encode(), 0o644)
    elif mutation == "flow-structure":
        candidate["config/mob.toml"] = (candidate["config/mob.toml"][0] + b'\n[flows.extra.steps.new]\nrole="lead"\nmessage="extra"\n', 0o644)
    elif mutation == "mode":
        candidate["config/mob.toml"] = (candidate["config/mob.toml"][0], 0o755)
    assert not updates.instruction_configuration_update(previous, candidate)
    assert updates.compatibility(previous, candidate, {})[1]


def test_host_upgrade_requires_current_exact_export_and_accepted_framework_provenance():
    before, after = revised_role_blueprint()
    previous, candidate = canonical_update_package(before), canonical_update_package(after)
    previous["app.py"] = (b"# Older accepted protected host\n", 0o644)
    assert not updates.compatibility(previous, candidate, {})[1]
    candidate["app.py"] = (candidate["app.py"][0] + b"# Authored override\n", 0o644)
    assert updates.compatibility(previous, candidate, {})[1]
    candidate = canonical_update_package(after)
    del previous["implementation-report.json"]
    assert updates.compatibility(previous, candidate, {})[1]


def test_current_compiler_host_survives_ordinary_accepted_baseline_repair_before_role_migration(tmp_path):
    from launchkit.implementation_workspace import ImplementationWorkspace

    before, after = revised_role_blueprint()
    previous = canonical_update_package(before)
    previous["app.py"] = (b"# Older accepted protected host\n", 0o644)
    refreshed = ImplementationWorkspace(tmp_path / "framework-refresh", after)
    refreshed.seed(fresh_reason="explicit-framework-refresh")
    refreshed.write_file("application/feature.py", "VERSION = 'needs intake repair'\n")
    assert refreshed.framework_provenance()["basis"] == "current-compiler"
    with refreshed._locked():
        state = refreshed._state()
        files = refreshed._files(state)
        snapshot = refreshed._framework_snapshot(state, files, build_id=str(uuid.uuid4()),
            accepted_source_digest=refreshed._digest(files))
    repaired = ImplementationWorkspace(tmp_path / "ordinary-repair", after)
    repaired.seed(snapshot)
    repaired.write_file("application/feature.py", "VERSION = 'repaired intake'\n")
    provenance = repaired.framework_provenance()
    assert provenance["basis"] == "accepted-baseline"
    assert provenance["reason"] == "unchanged-specification"
    assert provenance["baseline_digest"] == provenance["current_compiler_baseline_digest"]
    assert provenance["retained_template_paths"] == []
    assert repaired.read_file("app.py") == refreshed.read_file("app.py")
    candidate = {item.path: (item.content.encode(), item.mode)
                 for item in repaired.export_files(require_verified=False)}
    candidate["implementation-report.json"] = (json.dumps({"framework": provenance}).encode(), 0o644)
    assert updates.current_host_framework(candidate)
    assert updates.instruction_configuration_update(previous, candidate)
    changes, blockers = updates.compatibility(previous, candidate, {})
    assert not blockers
    assert {item["path"] for item in changes} >= {"app.py", "config/mob.toml", "application/feature.py"}


@pytest.mark.parametrize("field,value", [
    ("basis", "authored-host"), ("reason", "changed-specification"),
    ("baseline_digest", "a" * 64), ("current_compiler_baseline_digest", "b" * 64),
    ("retained_template_paths", ["app.py"])])
def test_retained_current_host_never_bypasses_exact_compiler_fingerprints(field, value):
    before, after = revised_role_blueprint()
    previous, candidate = canonical_update_package(before), canonical_update_package(after)
    previous["app.py"] = (b"# Older accepted protected host\n", 0o644)
    report = json.loads(candidate["implementation-report.json"][0])
    report["framework"].update(basis="accepted-baseline", reason="unchanged-specification")
    report["framework"][field] = value
    candidate["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    assert not updates.current_host_framework(candidate)
    assert updates.compatibility(previous, candidate, {})[1]


async def prepared(tmp_path, *, console=None, tool_policy=None):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.extra_files = {"application/logic.py": "VERSION = 'old'\n"}
    if tool_policy is not None:
        implementation.extra_files["application/tool-policy.json"] = tool_policy
    if console is not None:
        implementation.extra_files["config/console.toml"] = console
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped")
    (destination / "data").mkdir()
    (destination / "data/ledger.db").write_bytes(b"user application records")
    (destination / "workspace").mkdir()
    (destination / "workspace/user.txt").write_text("user workspace")
    (destination / ".mobkit/state").mkdir()
    (destination / ".mobkit/state/native.db").write_bytes(b"native durable sessions")
    private = (destination / ".env").read_bytes()
    previous = read_receipt(directory)
    implementation.identity = str(uuid.uuid4())
    implementation.digest = "new-accepted-source-digest"
    implementation.extra_files = {"application/logic.py": "VERSION = 'new'\n", "application/new.py": "FEATURE = True\n"}
    if tool_policy is not None:
        implementation.extra_files["application/tool-policy.json"] = tool_policy
    if console is not None:
        implementation.extra_files["config/console.toml"] = console
    return service, directory, destination, previous, private


async def staged(tmp_path):
    service, directory, destination, previous, private = await prepared(tmp_path)
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"],
                                       service.guide.state["version"], service.implementation.identity)
    return service, directory, destination, previous, private, receipt["update"]["update_id"]


POLICY_SOURCE = json.dumps({"provider_id": "application", "policy_id": "observing-tools", "revision": 1,
    "members": [{"member_identity": "lead:main", "grants": []}]})


def policy_guard_image(root):
    return {str(path.relative_to(root)): (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
        for path in root.rglob("*") if path.is_file() and path.name != "launch.lock"}


async def policy_update_stage(tmp_path, change):
    service, directory, destination, previous, private = await prepared(tmp_path,
        tool_policy=None if change == "added" else POLICY_SOURCE)
    original = service.implementation.package

    async def package(*args):
        payload, slug = await original(*args)
        files = updates.package_files(payload)
        name = "application/tool-policy.json"
        if change == "added":
            files[name] = (POLICY_SOURCE.encode(), 0o644)
        elif change == "removed":
            del files[name]
        elif change == "bytes":
            selected = json.loads(POLICY_SOURCE)
            selected["revision"] = 2
            files[name] = (json.dumps(selected).encode(), 0o644)
        elif change == "mode":
            files[name] = (files[name][0], 0o755)
        result = io.BytesIO()
        with zipfile.ZipFile(result, "w") as archive:
            for key, (content, mode) in files.items():
                entry = zipfile.ZipInfo(key)
                entry.external_attr = (stat.S_IFREG | mode) << 16
                archive.writestr(entry, content)
        return result.getvalue(), slug

    service.implementation.package = package
    before = policy_guard_image(destination)
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"],
        service.guide.state["version"], service.implementation.identity)
    assert policy_guard_image(destination) == before
    return service, directory, destination, previous, private, receipt["update"]


@pytest.mark.parametrize("change", ["added", "removed", "bytes", "mode"])
async def test_changed_application_tool_policy_is_staged_for_review_without_publication(tmp_path, change):
    _, directory, destination, previous, private, update = await policy_update_stage(tmp_path, change)
    assert update["status"] == "blocked"
    assert any("application/tool-policy.json" in value and "policy" in value for value in update["blockers"])
    assert read_receipt(directory)["package_sha256"] == previous["package_sha256"]
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == b"user application records"
    assert updates.inspect_transaction(directory, update["update_id"])[2] == update


@pytest.mark.parametrize("action", ["activate", "restore", "recover_candidate", "retry_candidate"])
async def test_policy_guard_blocks_direct_source_actions_after_status_and_blocker_bypass(tmp_path, action):
    _, directory, destination, _, _, update = await policy_update_stage(tmp_path, "bytes")
    _, root, update = updates.transaction(directory, update["update_id"])
    update = updates.checkpoint(directory, root, update,
        status="staged" if action == "activate" else "recovery-needed", blockers=[],
        rollback_available=True, runtime_admission_possible=True)
    before_source, before_journal = policy_guard_image(destination), policy_guard_image(directory)
    with pytest.raises(DraftConflict, match="tool access|permission.update"):
        if action == "restore":
            updates.restore(directory, update["update_id"], restore_data=False)
        else:
            getattr(updates, action)(directory, update["update_id"])
    assert policy_guard_image(destination) == before_source
    assert policy_guard_image(directory) == before_journal
    assert updates.inspect_transaction(directory, update["update_id"])[2] == update


async def test_policy_guard_refuses_worker_before_any_native_or_application_entry(tmp_path):
    _, directory, destination, _, _, update = await policy_update_stage(tmp_path, "bytes")
    _, root, update = updates.transaction(directory, update["update_id"])
    update = updates.checkpoint(directory, root, update, status="staged", blockers=[], native_definition_update=True)
    before_source, before_journal = policy_guard_image(destination), policy_guard_image(directory)
    calls = []

    class NoEntry(Supervisor):
        async def native_definition_revision(self, *args, **kwargs):
            calls.append("native observation")
            raise AssertionError("The unsupported policy update must not enter native observation")

        async def command(self, *args, **kwargs):
            calls.append("command")
            raise AssertionError("The unsupported policy update must not execute application code")

        async def start(self, *args, **kwargs):
            calls.append("start")
            raise AssertionError("The unsupported policy update must not start a runtime")

    with pytest.raises(DraftConflict, match="tool access|permission.update"):
        await update_runtime(NoEntry(directory), "update", update_id=update["update_id"])
    assert calls == []
    assert policy_guard_image(destination) == before_source
    assert policy_guard_image(directory) == before_journal


async def test_unchanged_policy_allows_code_updates_checkpoints_and_original_lease_publication(tmp_path):
    _, directory, destination, previous, private, update = await policy_update_stage(tmp_path, "unchanged")
    assert update["status"] == "staged" and update["blockers"] == []
    identity = update["update_id"]
    _, root, selected = updates.transaction(directory, identity)
    selected = updates.checkpoint(directory, root, selected, detail="Checkpoint progress remains diagnostic")
    assert updates.inspect_transaction(directory, identity)[2] == selected
    updates.activate(directory, identity)
    assert (destination / "application/logic.py").read_text() == "VERSION = 'new'\n"
    assert (destination / "application/tool-policy.json").read_text() == POLICY_SOURCE
    assert (destination / ".env").read_bytes() == private
    updates.restore(directory, identity, restore_data=False)
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert read_receipt(directory)["package_sha256"] == previous["package_sha256"]
    _, root, selected = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, selected, status="recovery-needed", runtime_admission_possible=True)
    updates.retry_candidate(directory, identity)
    assert (destination / "application/tool-policy.json").read_text() == POLICY_SOURCE
    assert (destination / "data/ledger.db").read_bytes() == b"user application records"


async def test_policy_source_publisher_refuses_without_original_project_lease(tmp_path):
    _, directory, _, _, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    before = policy_guard_image(directory)
    with pytest.raises(DraftConflict, match="project lease"):
        updates._publish_source_receipt(directory, candidate, update, detail="Must remain unpublished")
    assert policy_guard_image(directory) == before


@pytest.mark.parametrize("lease_kind", ["closed", "wrong-root"])
async def test_policy_source_publisher_refuses_closed_or_wrong_real_project_lease(tmp_path, lease_kind):
    _, directory, destination, _, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    before = policy_guard_image(directory)
    if lease_kind == "closed":
        with project_lock(destination) as lease:
            pass
        with pytest.raises(RuntimeError, match="project lock.*current"):
            updates._publish_source_receipt(directory, candidate, update, project_lease=lease)
    else:
        unrelated = tmp_path / "another-owned-project"
        unrelated.mkdir(mode=0o700)
        with project_lock(unrelated) as lease:
            with pytest.raises(DraftConflict, match="project lease"):
                updates._publish_source_receipt(directory, candidate, update, project_lease=lease)
    assert policy_guard_image(directory) == before


@pytest.mark.parametrize("change", ["added", "removed", "bytes", "mode"])
async def test_original_lease_publisher_rechecks_policy_archives_after_checkpoint_bypass(tmp_path, change):
    _, directory, destination, _, _, update = await policy_update_stage(tmp_path, change)
    _, root, update = updates.transaction(directory, update["update_id"])
    update = updates.checkpoint(directory, root, update, status="staged", blockers=[])
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    before = policy_guard_image(directory)
    with project_lock(destination) as lease:
        with pytest.raises(DraftConflict, match="tool access|permission.update"):
            updates._publish_source_receipt(directory, candidate, update, project_lease=lease)
    assert policy_guard_image(directory) == before


@pytest.mark.parametrize("substitution", ["target-policy", "saved-archive"])
async def test_original_lease_publisher_refuses_substitution_after_earlier_archive_selection(tmp_path, substitution):
    _, directory, destination, _, _, update = await policy_update_stage(tmp_path, "unchanged")
    _, root, update = updates.transaction(directory, update["update_id"])
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    candidate["application/tool-policy.json"] = (POLICY_SOURCE.replace('"revision": 1', '"revision": 2').encode(), 0o644)
    if substitution == "saved-archive":
        (root / "candidate.zip").write_bytes(b"A changed archive after the earlier checked read")
    before = policy_guard_image(directory)
    with project_lock(destination) as lease:
        with pytest.raises(DraftConflict, match="tool access|permission.update|saved update package changed"):
            updates._publish_source_receipt(directory, candidate, update, project_lease=lease)
    assert policy_guard_image(directory) == before


async def test_original_lease_publisher_refuses_lease_loss_after_source_validation(tmp_path, monkeypatch):
    _, directory, destination, _, _, update = await policy_update_stage(tmp_path, "unchanged")
    _, root, update = updates.transaction(directory, update["update_id"])
    _, candidate = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    before = policy_guard_image(directory)
    held = project_lock(destination)
    lease = held.__enter__()
    original = updates._source_receipt_candidate

    def release_after_validation(*args, **kwargs):
        result = original(*args, **kwargs)
        held.__exit__(None, None, None)
        return result

    monkeypatch.setattr(updates, "_source_receipt_candidate", release_after_validation)
    try:
        with pytest.raises(RuntimeError, match="project lock.*current"):
            updates._publish_source_receipt(directory, candidate, update, project_lease=lease)
    finally:
        held.__exit__(None, None, None)
    assert policy_guard_image(directory) == before


async def test_staging_is_exact_source_bound_and_does_not_change_private_or_installed_data(tmp_path):
    service, directory, destination, previous, private, identity = await staged(tmp_path)
    receipt, root, update = updates.transaction(directory, identity)
    assert update["status"] == "staged" and not update["blockers"]
    assert update["from_build_id"] == previous["build_id"]
    assert update["to_build_id"] == service.implementation.identity
    assert update["source_digest"] == service.implementation.digest
    assert receipt["build_id"] == previous["build_id"]
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions"
    for path in root.glob("*.json"):
        assert "private-api-key-value" not in path.read_text()
    assert not update["rollback_available"]


async def test_authored_application_tests_are_source_only_without_relaxing_native_or_host_boundaries(tmp_path):
    service, directory, _, previous, _ = await prepared(tmp_path)
    service.implementation.extra_files["tests/test_summary.py"] = "def test_summary():\n    assert True\n"
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    assert receipt["update"]["status"] == "staged" and not receipt["update"]["blockers"]
    assert any(item["path"] == "tests/test_summary.py" for item in receipt["update"]["changes"])
    _, before = updates.checked_package(directory, "package.zip", previous["package_sha256"])
    _, after = updates.checked_package(updates.update_directory(directory, receipt["update"]["update_id"]),
                                      "candidate.zip", receipt["update"]["package_sha256"])
    after["config/mob.toml"] = (b"changed native configuration", 0o644)
    _, blockers = updates.compatibility(before, after, receipt)
    assert any("config/mob.toml" in reason for reason in blockers)


CONSOLE = '''[[agents]]
id = "lead:main"
name = "Companion"
[auth]
enabled = true
retries = 1
[permissions]
send = true
[controls.custom]
callback = "original"
[layout]
initial_control = "topology"
initial_agent = "lead:main"
initial_preset = "two_columns"
sidebar_width = 320
'''


def console_compatibility(before, after, *, before_mode=0o644, after_mode=0o644):
    old = {} if before is None else {"config/console.toml": (before.encode() if isinstance(before, str) else before, before_mode)}
    new = {} if after is None else {"config/console.toml": (after.encode() if isinstance(after, str) else after, after_mode)}
    return updates.compatibility(old, new, {})


@pytest.mark.parametrize("before,after", [
    (CONSOLE, CONSOLE.replace('initial_control = "topology"\n', "")),
    (CONSOLE, CONSOLE.replace('initial_agent = "lead:main"', 'initial_agent = "librarian:main"')),
    (CONSOLE, CONSOLE.replace('initial_preset = "two_columns"', 'initial_preset = "one_column"')),
    ('[layout]\ninitial_control="topology"\n', ""),
    ('[auth]\nenabled=true\n', '[auth]\nenabled=true\n[layout]\ninitial_agent="lead:main"\n'),
    (CONSOLE, CONSOLE.replace('initial_control = "topology"', 'initial_control = ""')),
    (CONSOLE, CONSOLE + '# Presentation comment, no setting change.\n'),
])
def test_only_optional_string_console_defaults_are_compatible(before, after):
    changes, blockers = console_compatibility(before, after)
    assert changes == [{"path": "config/console.toml", "change": "changed"}]
    assert blockers == []


@pytest.mark.parametrize("old,new", [
    ('sidebar_width = 320', 'sidebar_width = 400'),
    ('enabled = true', 'enabled = false'),
    ('send = true', 'send = false'),
    ('name = "Companion"', 'name = "Different"'),
    ('id = "lead:main"', 'id = "other:main"'),
    ('callback = "original"', 'callback = "different"'),
    ('retries = 1', 'retries = true'),
    ('retries = 1', 'retries = 1.0'),
    ('retries = 1', 'retries = [1]'),
    ('initial_control = "topology"', 'initial_control = true'),
    ('initial_agent = "lead:main"', 'initial_agent = ["lead:main"]'),
    ('initial_preset = "two_columns"', 'initial_preset = 1'),
])
def test_console_defaults_never_mask_other_config_or_toml_type_changes(old, new):
    after = CONSOLE.replace('initial_control = "topology"\n', "").replace(old, new)
    # When testing that allowed field's type, keep the replacement in place.
    if old.startswith("initial_control"):
        after = CONSOLE.replace(old, new)
    _, blockers = console_compatibility(CONSOLE, after)
    assert len(blockers) == 1 and "config/console.toml" in blockers[0]


def test_nested_types_order_and_keys_outside_layout_stay_exact():
    before = CONSOLE + '[extra]\nvalues=[1,true]\ninitial_agent="lead:main"\n'
    for after in (before.replace('values=[1,true]', 'values=[1,1]'),
                  before.replace('values=[1,true]', 'values=[true,1]'),
                  before.replace('[extra]\nvalues=[1,true]\ninitial_agent="lead:main"',
                                 '[extra]\nvalues=[1,true]\ninitial_agent="other:main"')):
        assert console_compatibility(before, after)[1]


@pytest.mark.parametrize("content", [
    b'not valid TOML', b'[layout]\ninitial_agent="\\uD800"',
    b'[layout]\ninitial_agent="\xff"', b'[layout]\ninitial_agent="\xed\xa0\x80"',
    b'[layout]\ninitial_agent="A"\ninitial_agent="B"',
    b'[layout]\ninitial_agent="A"\n[layout]\ninitial_preset="B"',
    b'[layout]\ninitial_agent="A"\n[extra]\nvalue=nan',
    b'[layout]\ninitial_agent="A"\n[extra]\nvalue=inf',
    b'[layout]\ninitial_agent="A"\nvalue=' + b'[' * 1200 + b'0' + b']' * 1200,
    b' ' * (updates.MAX_CONSOLE_CONFIG_BYTES + 1),
])
def test_invalid_oversized_or_nonordinary_console_input_is_blocked_without_parser_errors(content):
    assert console_compatibility(CONSOLE, content)[1]
    assert console_compatibility(content, CONSOLE)[1]


def test_console_file_presence_and_mode_remain_required_and_byte_bound_is_exact():
    assert console_compatibility(None, CONSOLE)[1]
    assert console_compatibility(CONSOLE, None)[1]
    assert console_compatibility(CONSOLE, CONSOLE, after_mode=0o755)[1]
    base = CONSOLE.replace('initial_control = "topology"\n', "").encode()
    padded = base + b' ' * (updates.MAX_CONSOLE_CONFIG_BYTES - len(base))
    assert console_compatibility(CONSOLE, padded)[1] == []
    assert console_compatibility(CONSOLE, padded + b' ')[1]


async def test_actual_default_control_removal_stages_and_restores_exact_source_without_native_or_data_changes(tmp_path):
    service, directory, destination, previous, private = await prepared(tmp_path, console=CONSOLE)
    candidate = CONSOLE.replace('initial_control = "topology"\n', "")
    service.implementation.extra_files["config/console.toml"] = candidate
    result = await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    identity = result["update"]["update_id"]
    assert result["update"]["status"] == "staged" and result["update"]["blockers"] == []
    assert (destination / "config/console.toml").read_text() == CONSOLE
    updates.activate(directory, identity)
    assert (destination / "config/console.toml").read_text() == candidate
    assert read_receipt(directory)["update"]["status"] == "verifying"
    assert not service.tasks
    updates.restore(directory, identity, restore_data=False)
    assert (destination / "config/console.toml").read_text() == CONSOLE
    assert read_receipt(directory)["build_id"] == previous["build_id"]
    assert read_receipt(directory)["update"]["status"] == "restoring"
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions"
    assert (destination / "data/ledger.db").read_bytes() == b"user application records"


@pytest.mark.parametrize("name", ["config/mob.toml", "application/configuration.json", "application/console.json", "requirements.txt", "app.py"])
async def test_native_host_dependency_and_private_interface_changes_are_staged_but_never_activated(tmp_path, name):
    service, directory, destination, previous, private = await prepared(tmp_path)
    if name == "app.py":
        # Fixture package has a fixed host file; modify the immutable candidate
        # through its controlled exporter rather than changing installed source.
        original = service.implementation.package
        async def changed(*args):
            import io, zipfile
            payload, slug = await original(*args)
            source = updates.package_files(payload)
            source[name] = (b"print('different host')\n", 0o644)
            output = io.BytesIO()
            with zipfile.ZipFile(output, "w") as archive:
                for key, (content, _) in source.items(): archive.writestr(key, content)
            return output.getvalue(), slug
        service.implementation.package = changed
    else:
        service.implementation.extra_files[name] = "{}" if name.endswith(".json") else "new native or dependency content"
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    assert receipt["update"]["status"] == "blocked"
    assert any(name in reason for reason in receipt["update"]["blockers"])
    with pytest.raises(DraftConflict, match="cannot be activated"):
        updates.activate(directory, receipt["update"]["update_id"])
    assert (destination / ".env").read_bytes() == private


async def test_activation_requires_stopped_native_lease_and_never_overwrites_unowned_or_modified_files(tmp_path):
    _, directory, destination, _, _, identity = await staged(tmp_path)
    with project_lock(destination):
        with pytest.raises(RuntimeError, match="already"):
            updates.activate(directory, identity)
    assert read_receipt(directory)["update"]["status"] == "staged"
    added = destination / "application/new.py"
    added.write_text("user custom module")
    with pytest.raises(DraftConflict, match="modified outside|unowned file"):
        updates.activate(directory, identity)
    assert added.read_text() == "user custom module"
    added.unlink()
    original = destination / "application/logic.py"
    original.write_text("user modified existing module")
    with pytest.raises(DraftConflict, match="Installed source differs"):
        updates.activate(directory, identity)
    assert original.read_text() == "user modified existing module"


async def test_hardlinked_state_and_installed_source_are_not_overwritten(tmp_path):
    _, directory, destination, _, _, identity = await staged(tmp_path)
    import os
    old = destination / "application/logic.py"
    os.link(old, tmp_path / "external-source")
    with pytest.raises(DraftConflict, match="owned ordinary"):
        updates.activate(directory, identity)
    (tmp_path / "external-source").unlink()
    private = destination / ".env"
    os.link(private, tmp_path / "external-private")
    with pytest.raises(DraftConflict, match="unowned file or link"):
        updates.activate(directory, identity)
    assert old.read_text() == "VERSION = 'old'\n"


async def test_pre_ready_native_occurrence_and_domain_receipts_survive_verified_previous_source_recovery(tmp_path):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    class FailingCandidate(Supervisor):
        calls = []
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            self.calls.append(read_receipt(directory)["build_id"])
            if len(self.calls) == 1:
                assert read_receipt(directory)["update"]["runtime_admission_possible"] is True
                (destination / "data/ledger.db").write_bytes(b"user application records; pre-health domain action receipt")
                (destination / "workspace/failed.txt").write_text("failed file")
                (destination / ".mobkit/state/native.db").write_bytes(b"native durable sessions; pre-health schedule occurrence receipt")
                raise RuntimeError("candidate fixture startup failed")
            assert (destination / ".env").read_bytes() == private
            assert (destination / "data/ledger.db").read_bytes() == b"user application records; pre-health domain action receipt"
            assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions; pre-health schedule occurrence receipt"
            assert (destination / "workspace/failed.txt").exists()
            assert read_receipt(directory)["update"]["status"] == "restoring"
            on_ready()
    supervisor = FailingCandidate(directory)
    await update_runtime(supervisor, "update")
    receipt = read_receipt(directory)
    assert supervisor.calls == [receipt["update"]["to_build_id"], previous["build_id"]]
    assert receipt["update"]["status"] == "rolled-back"
    assert not receipt["update"]["rollback_available"]
    assert "candidate fixture" in receipt["update"]["activation_error"]
    assert receipt["package_sha256"] == previous["package_sha256"]


async def test_completed_update_and_manual_rollback_preserve_latest_local_data(tmp_path):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    class Ready(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready): on_ready()
    await update_runtime(Ready(directory), "update")
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["update"]["rollback_available"]
    assert receipt["build_id"] != previous["build_id"]
    (destination / "data/ledger.db").write_bytes(b"new records after update")
    (destination / ".mobkit/state/native.db").write_bytes(b"new native conversation after update")
    await update_runtime(Ready(directory), "rollback")
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "rolled-back"
    assert receipt["build_id"] == previous["build_id"]
    assert (destination / "data/ledger.db").read_bytes() == b"new records after update"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"new native conversation after update"
    assert (destination / ".env").read_bytes() == private


@pytest.mark.parametrize("before_mode,after_mode", [(0o644, 0o755), (0o755, 0o644)])
@pytest.mark.parametrize("update_umask", [0o022, 0o077])
async def test_mode_only_application_update_and_rollback_apply_exact_permissions(tmp_path, before_mode, after_mode, update_umask):
    guide = Guide()
    implementation = Implementation(guide)
    helper = "application/helper.sh"
    content = "#!/usr/bin/env bash\nexit 0\n"
    implementation.extra_files = {helper: content}
    helper_mode = before_mode
    original_package = implementation.package

    async def package_with_helper_mode(*args):
        payload, slug = await original_package(*args)
        output = io.BytesIO()
        with zipfile.ZipFile(io.BytesIO(payload)) as source, zipfile.ZipFile(output, "w") as archive:
            for item in source.infolist():
                if item.filename == helper:
                    item.external_attr = (stat.S_IFREG | helper_mode) << 16
                archive.writestr(item, source.read(item))
        return output.getvalue(), slug

    implementation.package = package_with_helper_mode
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    ambient_umask = os.umask(0o022)
    try:
        materialize(directory)
    finally:
        os.umask(ambient_umask)
    await service.shutdown()
    update_receipt(directory, status="stopped")
    previous = read_receipt(directory)
    private = (destination / ".env").read_bytes()
    unchanged = destination / "application/__init__.py"
    unchanged.chmod(0o600)
    helper_mode = after_mode
    implementation.identity = str(uuid.uuid4())
    implementation.digest = "mode-only-accepted-source-digest"
    receipt = await service.stage_update(guide.identity, previous["deployment_id"],
                                        guide.state["version"], implementation.identity)
    identity = receipt["update"]["update_id"]
    assert receipt["update"]["status"] == "staged" and not receipt["update"]["blockers"]
    assert {item["path"] for item in receipt["update"]["changes"]} == {helper, "implementation-report.json"}
    _, root, update = updates.transaction(directory, identity)
    _, before = updates.checked_package(root, "previous.zip", update["previous_package_sha256"])
    _, after = updates.checked_package(root, "candidate.zip", update["package_sha256"])
    assert before[helper] == (content.encode(), before_mode)
    assert after[helper] == (content.encode(), after_mode)

    class Ready(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready): on_ready()

    ambient_umask = os.umask(update_umask)
    try:
        await update_runtime(Ready(directory), "update", update_id=identity)
        assert read_receipt(directory)["update"]["status"] == "completed"
        assert stat.S_IMODE((destination / helper).stat().st_mode) == after_mode
        assert (destination / helper).read_bytes() == content.encode()
        assert stat.S_IMODE(unchanged.stat().st_mode) == 0o600
        await update_runtime(Ready(directory), "rollback", update_id=identity)
        assert read_receipt(directory)["update"]["status"] == "rolled-back"
        assert stat.S_IMODE((destination / helper).stat().st_mode) == before_mode
        assert (destination / helper).read_bytes() == content.encode()
        assert stat.S_IMODE(unchanged.stat().st_mode) == 0o600
        assert (destination / ".env").read_bytes() == private
    finally:
        os.umask(ambient_umask)


async def test_native_instruction_update_uses_observed_revision_and_source_rollback_cas_without_rewinding_sessions(tmp_path, monkeypatch):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, native_definition_update=True)
    monkeypatch.setattr(updates, "has_host_instruction_carrier", lambda files: True)
    observations, activations, starts = [], [], []

    class NativeReady(Supervisor):
        revision = 37
        async def native_definition_revision(self, files):
            receipt = read_receipt(directory)
            observations.append((receipt["build_id"], self.revision))
            return {"definition_revision": self.revision, "package_sha256": receipt["package_sha256"]}
        async def command(self, args, *other, **kwargs):
            if "--activate-revision" in args:
                expected = int(args[-1])
                assert expected == self.revision
                activations.append((read_receipt(directory)["build_id"], expected))
                self.revision += 4  # The worker observes a later epoch, never infers one from guide version.
        async def start(self, definition, *, on_ready):
            starts.append(read_receipt(directory)["build_id"])
            on_ready()

    supervisor = NativeReady(directory)
    await update_runtime(supervisor, "update")
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed"
    assert activations == [(receipt["build_id"], 37)]
    assert receipt["update"]["native_definition_activation"]["definition_revision"] == 41
    (destination / ".mobkit/state/native.db").write_bytes(b"same sessions; new private history")
    (destination / "data/ledger.db").write_bytes(b"withdrawn permission and stale print")
    await update_runtime(supervisor, "rollback")
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "rolled-back"
    assert activations[-1] == (previous["build_id"], 41)
    assert receipt["update"]["native_definition_activation"]["definition_revision"] == 45
    assert observations == [(previous["build_id"], 37), (starts[0], 41), (starts[0], 41), (previous["build_id"], 45)]
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"same sessions; new private history"
    assert (destination / "data/ledger.db").read_bytes() == b"withdrawn permission and stale print"
    assert (destination / ".env").read_bytes() == private


async def test_unsupported_native_carrier_stops_update_before_copy_or_canonical_activation(tmp_path):
    _, directory, destination, previous, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, native_definition_update=True)

    class Unsupported(Supervisor):
        async def native_definition_revision(self, files):
            raise RuntimeError("Installed native carrier is unsupported")
        async def command(self, *args, **kwargs):
            pytest.fail("No activation or authored check is permitted after unknown native capability")
        async def start(self, *args, **kwargs):
            pytest.fail("No updated source was admitted")

    with pytest.raises(RuntimeError, match="unsupported"):
        await update_runtime(Unsupported(directory), "update")
    assert read_receipt(directory)["build_id"] == previous["build_id"]
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions"
    assert not (root / "before-update").exists()


@pytest.mark.parametrize("epoch", [1, 8])
async def test_legacy_host_refresh_is_admitted_only_at_observed_initial_epoch_before_source_copy(tmp_path, epoch):
    _, directory, destination, previous, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, host_instruction_update=True)
    slug = json.loads((destination / "deployment.json").read_text())["slug"]

    class LegacyHost(Supervisor):
        started = False
        def native_binary_provenance(self, files):
            return {"native_binary_sha256": "owned artifact fixture"}
        async def command(self, args, *other, **kwargs):
            if kwargs.get("capture_output"):
                assert read_receipt(directory)["package_sha256"] == previous["package_sha256"]
                assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
                return "\n".join(json.dumps({"mob_id": slug, **record}) for record in [
                    {"source": "native runtime capabilities", "host_system_prompt": {"send": False, "dispatch": False}},
                    {"source": "native structural event ledger", "definition_revision": epoch}])
            assert "--activate-revision" not in args
        async def start(self, definition, *, on_ready):
            self.started = True
            on_ready()

    supervisor = LegacyHost(directory)
    if epoch == 1:
        await update_runtime(supervisor, "update")
        assert supervisor.started and read_receipt(directory)["update"]["status"] == "completed"
        assert read_receipt(directory)["update"]["native_definition_observation"]["host_system_prompt"] == {"send": False, "dispatch": False}
    else:
        with pytest.raises(RuntimeError, match="Preserve the observed definition and transaction state"):
            await update_runtime(supervisor, "update")
        assert not supervisor.started and not (root / "before-update").exists()
        assert read_receipt(directory)["package_sha256"] == previous["package_sha256"]
        assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"


@pytest.mark.parametrize("loss", ["source-check", "before-admission", "after-admission"])
async def test_exact_activation_recovery_replays_original_native_cas_after_interruption(tmp_path, monkeypatch, loss):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, native_definition_update=True)
    monkeypatch.setattr(updates, "has_host_instruction_carrier", lambda files: True)
    candidate = update["package_sha256"]
    artifact = {"native_binary_sha256": "a" * 64, "native_binary_version": "0.8.42", "native_binary_source": "explicit_override"}

    class InterruptedNative(Supervisor):
        revision = 12
        canonical = previous["package_sha256"]
        interrupt = True
        attempts = []
        observations = []
        def native_binary_provenance(self, files):
            return artifact
        async def native_definition_revision(self, files):
            source = read_receipt(directory)["package_sha256"]
            self.observations.append((source, self.revision))
            if source != self.canonical:
                raise RuntimeError("Ordinary candidate connection cannot bypass the native pinned definition")
            return {"definition_revision": self.revision, "package_sha256": source,
                    "host_system_prompt": {"send": True, "dispatch": True}, **artifact}
        async def command(self, args, *other, **kwargs):
            if "--activate-revision" not in args:
                if loss == "source-check" and self.interrupt:
                    self.interrupt = False
                    raise RuntimeError("Owned source check was interrupted before activation")
                return
            expected = int(args[-1])
            source = read_receipt(directory)["package_sha256"]
            self.attempts.append((source, expected))
            if self.canonical == source:
                assert self.revision == expected + 1, "Native AlreadyMatching rejects a fresh pin at the current revision"
            else:
                assert self.revision == expected
                if not self.interrupt or loss == "after-admission":
                    self.revision += 1
                    self.canonical = source
            if self.interrupt:
                self.interrupt = False
                raise RuntimeError("Native activation response was lost")
        async def before_source_restore(self):
            raise RuntimeError("Owned cleanup could not be confirmed; preserve the exact candidate and current state")
        async def start(self, definition, *, on_ready):
            assert self.canonical == candidate
            on_ready()

    supervisor = InterruptedNative(directory)
    with pytest.raises(RuntimeError, match="No source fallback was performed"):
        await update_runtime(supervisor, "update")
    assert read_receipt(directory)["update"]["status"] == "recovery-needed"
    assert read_receipt(directory)["package_sha256"] == candidate
    (destination / ".mobkit/state/native.db").write_bytes(b"same session identities and latest admissions")
    (destination / "data/ledger.db").write_bytes(b"latest changed facts and withdrawn consent")
    await update_runtime(supervisor, "recover-current")
    assert supervisor.attempts == [(candidate, 12)] * (1 if loss == "source-check" else 2)
    assert supervisor.revision == 13
    assert supervisor.observations == [(previous["package_sha256"], 12), (candidate, 13)]
    assert read_receipt(directory)["update"]["status"] == "completed"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"same session identities and latest admissions"
    assert (destination / "data/ledger.db").read_bytes() == b"latest changed facts and withdrawn consent"
    assert (destination / ".env").read_bytes() == private


@pytest.mark.parametrize("advanced", [False, True])
async def test_forward_recovery_converges_an_interrupted_previous_source_cas_before_candidate_activation(tmp_path, monkeypatch, advanced):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, native_definition_update=True)
    monkeypatch.setattr(updates, "has_host_instruction_carrier", lambda files: False)
    candidate = update["package_sha256"]
    artifact = {"native_binary_sha256": "b" * 64, "native_binary_version": "0.8.42", "native_binary_source": "explicit_override"}

    class InterruptedFallback(Supervisor):
        canonical = previous["package_sha256"]
        revision = 20
        interrupt_fallback = True
        starts = 0
        attempts = []
        def native_binary_provenance(self, files):
            return artifact
        async def native_definition_revision(self, files):
            source = read_receipt(directory)["package_sha256"]
            assert source == self.canonical, "Ordinary observation cannot bootstrap mismatched previous source"
            return {"definition_revision": self.revision, "package_sha256": source,
                    "host_system_prompt": {"send": True, "dispatch": True}, **artifact}
        async def command(self, args, *other, **kwargs):
            if "--activate-revision" not in args:
                return
            expected = int(args[-1])
            source = read_receipt(directory)["package_sha256"]
            self.attempts.append((source, expected))
            interrupted = source == previous["package_sha256"] and self.interrupt_fallback
            if source == self.canonical:
                assert self.revision == expected + 1
            else:
                assert self.revision == expected
                if not interrupted or advanced:
                    self.canonical, self.revision = source, self.revision + 1
            if interrupted:
                self.interrupt_fallback = False
                raise RuntimeError("Previous-source canonical activation response was lost")
        async def start(self, definition, *, on_ready):
            assert read_receipt(directory)["package_sha256"] == candidate, "Never start the previous legacy application"
            self.starts += 1
            if self.starts == 1:
                (destination / ".mobkit/state/native.db").write_bytes(b"same identities; candidate startup admission")
                (destination / "data/ledger.db").write_bytes(b"latest case facts; withdrawn consent")
                raise RuntimeError("Candidate actual health failed after canonical activation")
            on_ready()

    supervisor = InterruptedFallback(directory)
    with pytest.raises(RuntimeError, match="needs recovery"):
        await update_runtime(supervisor, "update")
    assert read_receipt(directory)["package_sha256"] == previous["package_sha256"]
    assert read_receipt(directory)["update"]["native_activation_package_sha256"] == previous["package_sha256"]
    await update_runtime(supervisor, "recover-current")
    assert supervisor.attempts == [(candidate, 20), (previous["package_sha256"], 21),
                                   (previous["package_sha256"], 21), (candidate, 22)]
    assert supervisor.revision == 23 and supervisor.starts == 2
    assert read_receipt(directory)["update"]["status"] == "completed"
    assert read_receipt(directory)["update"]["rollback_available"] is False
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"same identities; candidate startup admission"
    assert (destination / "data/ledger.db").read_bytes() == b"latest case facts; withdrawn consent"
    assert (destination / ".env").read_bytes() == private


async def test_legacy_role_rollback_after_admission_is_rejected_without_source_or_canonical_mutation(tmp_path, monkeypatch):
    _, directory, destination, _, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, native_definition_update=True)
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, runtime_admission_possible=True)
    before = read_receipt(directory)["package_sha256"]
    monkeypatch.setattr(updates, "has_host_instruction_carrier", lambda files: False)
    updates.committed(directory, identity)
    assert read_receipt(directory)["update"]["rollback_available"] is False
    assert "legacy host cannot restore continuing role" in read_receipt(directory)["update"]["detail"]
    assert (root / "previous.zip").is_file()

    class NoRollback(Supervisor):
        async def native_definition_revision(self, *args):
            pytest.fail("Legacy role rollback must be rejected before even cold native observation")
        async def command(self, *args, **kwargs):
            pytest.fail("No role rollback may be attempted")

    with pytest.raises(DraftConflict, match="legacy host cannot restore continuing role"):
        await update_runtime(NoRollback(directory), "rollback")
    assert read_receipt(directory)["package_sha256"] == before
    assert (destination / "application/logic.py").read_text() == "VERSION = 'new'\n"


async def test_exact_current_canonical_recovery_never_redeclares_an_already_matching_definition(tmp_path):
    _, directory, _, _, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, native_definition_update=True)
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status="recovery-needed", runtime_admission_possible=True)

    class AlreadyCanonical(Supervisor):
        async def native_definition_revision(self, files):
            return {"definition_revision": 19, "package_sha256": read_receipt(directory)["package_sha256"],
                    "host_system_prompt": {"send": True, "dispatch": True}}
        async def command(self, args, *other, **kwargs):
            assert "--activate-revision" not in args, "Native AlreadyMatching must not be requested at epoch 19"
        async def start(self, definition, *, on_ready):
            on_ready()

    await update_runtime(AlreadyCanonical(directory), "recover-current")
    assert read_receipt(directory)["update"]["status"] == "completed"
    assert read_receipt(directory)["update"]["native_definition_recovery_observation"]["definition_revision"] == 19


@pytest.mark.parametrize("records", [[], [{"revision": 4}], [
    {"source": "native structural event ledger", "definition_revision": True}], [
    {"source": "native structural event ledger", "definition_revision": 4},
    {"source": "native structural event ledger", "definition_revision": 4}], [
    {"source": "native structural event ledger", "definition_revision": 4},
    {"source": "native runtime capabilities", "host_system_prompt": {"send": True, "dispatch": "true"}}]])
async def test_native_observation_requires_one_typed_epoch_and_both_actual_carrier_capabilities(tmp_path, records):
    _, directory, _, previous, _ = await prepared(tmp_path)
    _, files = updates.checked_package(directory, "package.zip", previous["package_sha256"])
    slug = json.loads(files["deployment.json"][0])["slug"]

    class Observation(Supervisor):
        def native_binary_provenance(self, files):
            return {"native_binary_sha256": "exact owned artifact stand-in"}
        async def command(self, *args, **kwargs):
            return "\n".join(json.dumps({"mob_id": slug, **record}) for record in records)

    with pytest.raises(RuntimeError, match="unambiguous|cannot author"):
        await Observation(directory).native_definition_revision(files)


async def test_same_accepted_candidate_can_be_restaged_as_new_transaction_after_explicit_verified_rollback(tmp_path):
    service, directory, destination, previous, private, identity = await staged(tmp_path)
    class Ready(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready): on_ready()
    await update_runtime(Ready(directory), "update", update_id=identity)
    (destination / "data/ledger.db").write_bytes(b"latest domain records before explicit rollback")
    await update_runtime(Ready(directory), "rollback", update_id=identity)
    update_receipt(directory, status="stopped", phase="stop")
    old_root = updates.update_directory(directory, identity)
    result = await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    assert result["update"]["status"] == "staged"
    assert result["update"]["update_id"] != identity
    assert result["update"]["to_build_id"] == service.implementation.identity
    assert result["build_id"] == previous["build_id"]
    assert not service.tasks and old_root.is_dir()
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert (destination / "data/ledger.db").read_bytes() == b"latest domain records before explicit rollback"
    assert (destination / ".env").read_bytes() == private


async def test_partial_source_replacement_and_lost_receipt_checkpoint_can_be_recovered_without_unowned_writes(tmp_path, monkeypatch):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    actual = updates.apply_files
    def interrupted(target, before, after):
        content, mode = after["application/logic.py"]
        updates.replace_file(target / "application/logic.py", content, mode)
        raise OSError("fixture host lost after first source write")
    monkeypatch.setattr(updates, "apply_files", interrupted)
    with pytest.raises(OSError): updates.activate(directory, identity)
    assert read_receipt(directory)["update"]["status"] == "applying"
    receipt, root, update = updates.transaction(directory, identity)
    # Crash between authority checkpoint and its receipt projection is repaired
    # from the journal, never inferred from copied files or a vanished process.
    checkpoint = {**update, "detail": "new durable checkpoint"}
    updates.write_private(root / "update.json", checkpoint)
    assert updates.transaction(directory, identity)[2]["detail"] == "new durable checkpoint"
    monkeypatch.setattr(updates, "apply_files", actual)
    updates.restore(directory, identity, restore_data=True)
    assert read_receipt(directory)["build_id"] == previous["build_id"]
    assert read_receipt(directory)["update"]["status"] == "restoring"
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    assert (destination / ".env").read_bytes() == private
    assert not (destination / "application/new.py").exists()


async def test_interrupted_snapshot_is_retryable_before_any_source_writes(tmp_path, monkeypatch):
    _, directory, destination, _, private, identity = await staged(tmp_path)
    actual = updates.snapshot
    def interrupted(target, root, name):
        pending = root / (name + ".pending")
        pending.mkdir()
        (pending / ".env").write_bytes(b"partial private backup")
        raise OSError("fixture backup interrupted")
    monkeypatch.setattr(updates, "snapshot", interrupted)
    with pytest.raises(OSError): updates.activate(directory, identity)
    assert read_receipt(directory)["update"]["status"] == "snapshotting"
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"
    monkeypatch.setattr(updates, "snapshot", actual)
    updates.activate(directory, identity)
    assert read_receipt(directory)["update"]["status"] == "verifying"
    assert (destination / ".env").read_bytes() == private


async def test_stale_build_acceptance_and_replaced_update_id_are_rejected_before_worker_admission(tmp_path):
    service, directory, _, previous, _, identity = await staged(tmp_path)
    service.implementation.digest = "source changed since stage"
    with pytest.raises(DraftConflict, match="no longer"):
        await service.action(service.guide.identity, previous["deployment_id"], "apply-update", update_id=identity)
    assert not service.tasks
    service.implementation.digest = "new-accepted-source-digest"
    service.implementation.identity = str(uuid.uuid4())
    next_receipt = await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    assert next_receipt["update"]["update_id"] != identity
    with pytest.raises(DraftConflict, match="different update"):
        await service.action(service.guide.identity, previous["deployment_id"], "apply-update", update_id=identity)
    assert not service.tasks


async def test_cancelled_start_restores_verified_source_retaining_recorded_state_without_claiming_runtime_recovery(tmp_path):
    _, directory, destination, previous, _, identity = await staged(tmp_path)
    class Cancelled(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            (destination / "data/ledger.db").write_bytes(b"unpublished candidate state")
            self.stop.set()
            raise asyncio.CancelledError
    with pytest.raises(asyncio.CancelledError): await update_runtime(Cancelled(directory), "update")
    receipt = read_receipt(directory)
    assert receipt["build_id"] == previous["build_id"]
    assert receipt["update"]["status"] == "restored-stopped"
    assert "No restart" in receipt["update"]["detail"]
    assert (destination / "data/ledger.db").read_bytes() == b"unpublished candidate state"


async def test_both_startups_failing_keeps_recovery_visible_and_late_operational_failure_does_not_rewind_committed_data(tmp_path):
    _, directory, destination, previous, _, _ = await staged(tmp_path)
    class Broken(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready): raise RuntimeError("fixture native startup failed")
    with pytest.raises(RuntimeError, match="needs recovery"): await update_runtime(Broken(directory), "update")
    receipt = read_receipt(directory)
    assert receipt["build_id"] == previous["build_id"]
    assert receipt["update"]["status"] == "recovery-needed"
    # A verified recovery gets a new ordinary source stage, not a stale guess.
    updates.committed(directory, receipt["update"]["update_id"], restored=True)
    service, directory2, destination2, previous2, _, _ = await staged(tmp_path / "separate")
    class LateFailure(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            on_ready()
            (destination2 / "data/ledger.db").write_bytes(b"published live workflow records")
            raise RuntimeError("late operational fixture failure")
    with pytest.raises(RuntimeError, match="late operational"): await update_runtime(LateFailure(directory2), "update")
    receipt = read_receipt(directory2)
    assert receipt["build_id"] != previous2["build_id"]
    assert receipt["update"]["status"] == "completed"
    assert (destination2 / "data/ledger.db").read_bytes() == b"published live workflow records"


async def test_restored_source_doctor_failure_does_not_inherit_candidate_startup_cause(tmp_path):
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    candidate = read_receipt(directory)["update"]["to_build_id"]
    update_receipt(directory, health="ready", health_checked_at="2026-09-30T23:00:00+00:00")
    attempt = "12345678-1234-1234-1234-123456789abc"
    starts = []
    class CandidateFailed(Supervisor):
        async def command(self, args, *other, **kwargs):
            if read_receipt(directory)["build_id"] == previous["build_id"]:
                assert args[-1] == "--doctor"
                assert read_receipt(directory)["startup_attempt_id"] is None
                assert read_receipt(directory)["startup_console_health"] is None
                assert read_receipt(directory)["health"] == "unknown"
                assert read_receipt(directory)["health_checked_at"] is None
                raise RuntimeError("restored source private configuration check failed")
        async def start(self, definition, *, on_ready):
            starts.append(read_receipt(directory)["build_id"])
            update_receipt(directory, phase="start", startup_attempt_id=attempt,
                health="ready", health_checked_at="2026-09-30T23:01:00+00:00",
                startup_console_health={"attempt_id": attempt, "status": "fail", "category": "authentication", "http_status": 401, "attempts": 1})
            (destination / "data/ledger.db").write_bytes(b"latest candidate domain receipt")
            (destination / ".mobkit/state/native.db").write_bytes(b"latest candidate native occurrence")
            raise RuntimeError("candidate Console authorization failed")
    with pytest.raises(RuntimeError, match="needs recovery"):
        await update_runtime(CandidateFailed(directory), "update", update_id=identity)
    receipt = read_receipt(directory)
    assert starts == [candidate]
    assert receipt["build_id"] == previous["build_id"] and receipt["phase"] == "update"
    assert receipt["startup_attempt_id"] is None and receipt["startup_console_health"] is None
    assert receipt["health"] == "unknown" and receipt["health_checked_at"] is None
    assert receipt["update"]["status"] == "recovery-needed"
    assert receipt["update"]["activation_error"] == "candidate Console authorization failed"
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == b"latest candidate domain receipt"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"latest candidate native occurrence"


async def test_failed_apply_and_fallback_cannot_publish_previous_runtime_ready_health(tmp_path, monkeypatch):
    from launchkit import deployment_worker as worker
    _, directory, destination, previous, private, identity = await staged(tmp_path)
    original = updates.retain_update_admission(directory, read_receipt(directory)["update"], "update")
    # The retained real failed apply had these prior healthy-runtime fields,
    # an EOF before native readiness, and the same failure on restored source.
    update_receipt(directory, health="ready", health_checked_at="2026-09-30T23:00:00+00:00",
                   update_action={"action": "apply-update", "update_id": identity})
    attempts = []
    error = "The application closed its startup verification channel before reporting readiness. Inspect the runtime logs for the native startup failure."
    class StartupEof(Supervisor):
        async def command(self, *args, **kwargs):
            assert read_receipt(directory)["health"] == "unknown"
            assert read_receipt(directory)["health_checked_at"] is None
        async def start(self, definition, *, on_ready):
            attempts.append(read_receipt(directory)["build_id"])
            raise RuntimeError(error)
    async def owned_input(supervisor):
        await supervisor.stop.wait()
        return "stopped"
    monkeypatch.setattr(worker, "Supervisor", StartupEof)
    monkeypatch.setattr(worker, "owner_input", owned_input)
    assert await worker.run(directory, "update", update_selection=original) == 1
    receipt = read_receipt(directory)
    assert len(attempts) == 2 and attempts[-1] == previous["build_id"]
    assert receipt["status"] == "failed" and receipt["update"]["status"] == "recovery-needed"
    assert receipt["update"]["activation_error"] == error
    assert receipt["health"] == "failed" and receipt["health_checked_at"]
    assert receipt["runtime_pid"] is None and receipt["console_url"] is None and not receipt["process_live"]
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == b"user application records"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"native durable sessions"


async def test_owner_loss_before_first_worker_instruction_does_not_replace_any_source(tmp_path):
    _, directory, destination, previous, private, _ = await staged(tmp_path)
    supervisor = Supervisor(directory)
    supervisor.stop.set()
    with pytest.raises(asyncio.CancelledError): await update_runtime(supervisor, "update")
    assert read_receipt(directory)["update"]["status"] == "staged"
    assert read_receipt(directory)["build_id"] == previous["build_id"]
    assert (destination / ".env").read_bytes() == private


async def test_native_readiness_pipe_without_actual_http_health_never_publishes_update_success(tmp_path, monkeypatch):
    service, directory, destination, _, _, _ = await staged(tmp_path)
    import os, sys, httpx
    monkeypatch.setattr("launchkit.deployment_worker.CONSOLE_START_TIMEOUT", 0.05)
    interpreter = destination / ".venv/bin/python"
    interpreter.parent.mkdir(parents=True)
    interpreter.symlink_to(sys.executable)
    bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0}, "identities": {"lead:main": {}}}
    (destination / "app.py").write_text(
        "import json,os,time\n"
        f"payload={{'pid':os.getpid(),'bootstrap':{bootstrap!r},'console_url':'http://127.0.0.1:18088/console'}}\n"
        "fd=int(os.environ['MOBKIT_LAUNCH_READY_FD'])\nos.write(fd,(json.dumps(payload)+'\\n').encode())\nos.close(fd)\ntime.sleep(60)\n")
    class Unavailable:
        async def __aenter__(self): return self
        async def __aexit__(self, *args): pass
        async def get(self, *args, **kwargs): raise httpx.ConnectError("fixture unavailable Console")
    monkeypatch.setattr(httpx, "AsyncClient", lambda **kwargs: Unavailable())
    published = []
    supervisor = Supervisor(directory)
    with pytest.raises(RuntimeError, match="actual Console identities health check failed"):
        await asyncio.wait_for(supervisor.start(service.implementation.definition, on_ready=lambda: published.append(True)), 5)
    assert not published and supervisor.child is None
    receipt = read_receipt(directory)
    assert receipt["status"] == "starting" and receipt["health"] == "unknown" and receipt["console_url"] is None


async def test_failed_manual_rollback_recovers_current_source_retaining_pre_health_domain_and_native_receipts(tmp_path):
    _, directory, destination, previous, _, identity = await staged(tmp_path)
    updates.activate(directory, identity)
    updates.committed(directory, identity)
    candidate = read_receipt(directory)["build_id"]
    (destination / "data/ledger.db").write_bytes(b"latest user records")
    (destination / ".mobkit/state/native.db").write_bytes(b"latest native sessions")
    class BrokenPrevious(Supervisor):
        calls = []
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            self.calls.append(read_receipt(directory)["build_id"])
            if len(self.calls) == 1:
                (destination / "data/ledger.db").write_bytes(b"latest user records; old startup domain receipt")
                (destination / ".mobkit/state/native.db").write_bytes(b"latest native sessions; old startup native occurrence")
                raise RuntimeError("old application rejected newer local schema")
            assert read_receipt(directory)["update"]["status"] == "recovering-candidate"
            assert (destination / "data/ledger.db").read_bytes() == b"latest user records; old startup domain receipt"
            assert (destination / ".mobkit/state/native.db").read_bytes() == b"latest native sessions; old startup native occurrence"
            on_ready()
    supervisor = BrokenPrevious(directory)
    await update_runtime(supervisor, "rollback")
    receipt = read_receipt(directory)
    assert supervisor.calls == [previous["build_id"], candidate]
    assert receipt["update"]["status"] == "completed"
    assert receipt["build_id"] == candidate
    assert "old application rejected" in receipt["update"]["activation_error"]


async def test_new_guide_operation_during_package_export_prevents_stage_publication(tmp_path):
    service, directory, _, previous, _ = await prepared(tmp_path)
    original = service.implementation.package
    async def racing(*args):
        payload = await original(*args)
        service.guide.state["operation"] = {"status": "running"}
        return payload
    service.implementation.package = racing
    with pytest.raises(DraftConflict, match="changed while preparing"):
        await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    assert "update" not in read_receipt(directory)
    assert not (directory / "updates").exists()


async def test_package_backup_or_live_journal_tampering_blocks_recovery(tmp_path):
    _, directory, _, _, _, identity = await staged(tmp_path)
    _, root, update = updates.transaction(directory, identity)
    previous = (root / "previous.zip").read_bytes()
    (root / "previous.zip").write_bytes(b"different historic source")
    with pytest.raises(DraftConflict, match="saved update package changed"):
        updates.transaction(directory, identity)
    (root / "previous.zip").write_bytes(previous)
    (directory / "package.zip").write_bytes(b"unknown installed journal")
    with pytest.raises(DraftConflict, match="journal changed outside"):
        updates.transaction(directory, identity)


async def test_queued_update_cannot_be_replaced_and_worker_uses_exact_admitted_stage_identity(tmp_path):
    service, directory, destination, previous, _, identity = await staged(tmp_path)
    attempt = "12345678-1234-1234-1234-123456789abc"
    update_receipt(directory, startup_attempt_id=attempt,
                   startup_console_health={"attempt_id": attempt, "status": "fail", "category": "authentication", "attempts": 1})
    await service.action(service.guide.identity, previous["deployment_id"], "apply-update", update_id=identity)
    assert read_receipt(directory)["update_action"] == {"action": "apply-update", "update_id": identity}
    assert read_receipt(directory)["startup_attempt_id"] is None
    assert read_receipt(directory)["startup_console_health"] is None
    service.implementation.identity = str(uuid.uuid4())
    with pytest.raises(DraftConflict, match="Finish or recover"):
        await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    await service.shutdown()
    update_receipt(directory, status="stopped", phase="stop")
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"], 4, service.implementation.identity)
    assert receipt["update"]["update_id"] != identity
    with pytest.raises(DraftConflict, match="different update"):
        await update_runtime(Supervisor(directory), "update", update_id=identity)
    assert (destination / "application/logic.py").read_text() == "VERSION = 'old'\n"


async def test_apply_checks_guide_again_after_awaited_source_inspection(tmp_path):
    service, directory, _, previous, _, identity = await staged(tmp_path)
    original = service.implementation.status
    async def racing(*args):
        result = await original(*args)
        service.guide.state["operation"] = {"status": "running"}
        return result
    service.implementation.status = racing
    with pytest.raises(DraftConflict, match="no longer"):
        await service.action(service.guide.identity, previous["deployment_id"], "apply-update", update_id=identity)
    assert not service.tasks
    assert read_receipt(directory)["update"]["status"] == "staged"


async def test_admission_boundary_survives_host_loss_and_explicit_recovery_never_rewinds_occurrence_receipts(tmp_path):
    service, directory, destination, previous, _, identity = await staged(tmp_path)
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, runtime_admission_possible=True)
    native = destination / ".mobkit/state/native.db"
    domain = destination / "data/ledger.db"
    native.write_bytes(b"native sessions plus acknowledged pre-health schedule occurrence")
    domain.write_bytes(b"domain ledger plus corresponding external-action receipt")
    update_receipt(directory, status="starting", phase="start")
    fresh = Service(service.root.parent, service.guide, service.implementation)
    receipt = await fresh.status(service.guide.identity)
    assert receipt["status"] == "interrupted"
    assert receipt["update"]["runtime_admission_possible"] is True
    with pytest.raises(DraftConflict, match="Native startup may have admitted"):
        updates.restore(directory, identity, restore_data=True)
    class ReadyPrevious(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            assert native.read_bytes() == b"native sessions plus acknowledged pre-health schedule occurrence"
            assert domain.read_bytes() == b"domain ledger plus corresponding external-action receipt"
            on_ready()
    await update_runtime(ReadyPrevious(directory), "rollback", update_id=identity)
    assert read_receipt(directory)["build_id"] == previous["build_id"]
    assert read_receipt(directory)["update"]["status"] == "rolled-back"
    assert native.read_bytes() == b"native sessions plus acknowledged pre-health schedule occurrence"


async def test_incompatible_old_source_cannot_claim_recovery_or_erase_pre_health_native_domain_work(tmp_path):
    _, directory, destination, _, _, _ = await staged(tmp_path)
    native = destination / ".mobkit/state/native.db"
    domain = destination / "data/ledger.db"
    class Incompatible(Supervisor):
        calls = 0
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            self.calls += 1
            if self.calls == 1:
                native.write_bytes(b"acknowledged native occurrence before HTTP health")
                domain.write_bytes(b"acknowledged domain receipt before HTTP health")
                raise RuntimeError("HTTP health unavailable after native work")
            assert native.read_bytes() == b"acknowledged native occurrence before HTTP health"
            assert domain.read_bytes() == b"acknowledged domain receipt before HTTP health"
            raise RuntimeError("old source cannot use newer application state")
    with pytest.raises(RuntimeError, match="needs recovery"):
        await update_runtime(Incompatible(directory), "update")
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed"
    assert "Current native and application state" in receipt["update"]["detail"]
    assert native.read_bytes() == b"acknowledged native occurrence before HTTP health"
    assert domain.read_bytes() == b"acknowledged domain receipt before HTTP health"


async def test_legacy_interrupted_journal_without_explicit_admission_boundary_preserves_latest_occurrences(tmp_path):
    _, directory, destination, _, _, identity = await staged(tmp_path)
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    legacy = {key: value for key, value in update.items() if key != "runtime_admission_possible"}
    updates.write_private(root / "update.json", legacy)
    update_receipt(directory, update=legacy, status="interrupted", phase="start")
    native = destination / ".mobkit/state/native.db"
    domain = destination / "data/ledger.db"
    native.write_bytes(b"legacy native occurrence already acknowledged")
    domain.write_bytes(b"legacy domain receipt already recorded")
    with pytest.raises(DraftConflict, match="may have admitted"):
        updates.restore(directory, identity, restore_data=True)
    class Previous(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            assert native.read_bytes() == b"legacy native occurrence already acknowledged"
            assert domain.read_bytes() == b"legacy domain receipt already recorded"
            on_ready()
    await update_runtime(Previous(directory), "rollback", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "rolled-back"
    assert native.read_bytes() == b"legacy native occurrence already acknowledged"


async def test_application_contract_load_failure_retains_domain_records_before_native_start(tmp_path):
    _, directory, destination, _, _, _ = await staged(tmp_path)
    domain = destination / "data/ledger.db"
    class CheckFailure(Supervisor):
        calls = 0
        async def command(self, args, *other, **kwargs):
            assert read_receipt(directory)["update"]["runtime_admission_possible"] is True
            if args[-1] == "--application-check" and self.calls == 0:
                self.calls += 1
                domain.write_bytes(b"application factory recorded domain event")
                raise RuntimeError("candidate application contract rejected")
        async def start(self, definition, *, on_ready):
            assert domain.read_bytes() == b"application factory recorded domain event"
            on_ready()
    await update_runtime(CheckFailure(directory), "update")
    assert read_receipt(directory)["update"]["status"] == "rolled-back"
    assert domain.read_bytes() == b"application factory recorded domain event"


async def recovery_needed(tmp_path):
    service, directory, destination, previous, private, identity = await staged(tmp_path)
    updates.activate(directory, identity)
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, runtime_admission_possible=True)
    (destination / "data/ledger.db").write_bytes(b"latest retained domain receipt")
    (destination / ".mobkit/state/native.db").write_bytes(b"latest retained native occurrence")
    updates.restore(directory, identity, restore_data=False, rollback_kind="failed-update")
    _, root, update = updates.transaction(directory, identity)
    updates.checkpoint(directory, root, update, status="recovery-needed", rollback_available=True,
                       activation_error="previous source cannot read the retained current state")
    update_receipt(directory, status="failed", phase="recovery", health="unknown", process_live=False)
    return service, directory, destination, previous, private, identity


async def test_current_recovery_admission_requires_exact_transaction_current_accepted_source_and_idle_guide(tmp_path):
    service, directory, destination, previous, private, identity = await recovery_needed(tmp_path)
    for change in ["wrong-id", "source", "acceptance", "guide"]:
        old_digest, old_accepted = service.implementation.digest, service.implementation.accepted
        selected = str(uuid.uuid4()) if change == "wrong-id" else identity
        if change == "source": service.implementation.digest = "a replacement source"
        if change == "acceptance": service.implementation.accepted = False
        if change == "guide": service.guide.state["operation"] = {"status": "running"}
        with pytest.raises((DraftConflict, ValueError)):
            await service.action(service.guide.identity, previous["deployment_id"], "recover-current", update_id=selected)
        assert not service.tasks
        assert read_receipt(directory)["update"]["status"] == "recovery-needed"
        assert (destination / ".env").read_bytes() == private
        service.implementation.digest, service.implementation.accepted = old_digest, old_accepted
        service.guide.state.pop("operation", None)
    receipt = await service.action(service.guide.identity, previous["deployment_id"], "recover-current", update_id=identity)
    assert receipt["status"] == "queued"
    assert receipt["update_action"] == {"action": "recover-current", "update_id": identity}
    await service.shutdown()


async def test_current_recovery_keeps_latest_records_and_requires_configuration_checks_before_verified_publication(tmp_path):
    service, directory, destination, previous, private, identity = await recovery_needed(tmp_path)
    expected_records = {"data/ledger.db": (destination / "data/ledger.db").read_bytes(),
                        ".mobkit/state/native.db": (destination / ".mobkit/state/native.db").read_bytes()}
    class Ready(Supervisor):
        checks = []
        async def command(self, args, *other, **kwargs):
            self.checks.append(args[-1])
            with pytest.raises(RuntimeError, match="already"):
                with project_lock(destination): pass
        async def start(self, definition, *, on_ready):
            assert self.checks == ["--doctor", "--application-check"]
            receipt = read_receipt(directory)
            assert receipt["build_id"] == service.implementation.identity
            assert receipt["update"]["status"] == "recovering-current" and receipt["health"] == "unknown"
            assert (destination / ".env").read_bytes() == private
            for name, content in expected_records.items(): assert (destination / name).read_bytes() == content
            on_ready()
    await update_runtime(Ready(directory), "recover-current", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed"
    assert receipt["build_id"] == service.implementation.identity != previous["build_id"]
    assert receipt["update"]["rollback_available"]
    assert "previous source cannot read" in receipt["update"]["activation_error"]
    for name, content in expected_records.items(): assert (destination / name).read_bytes() == content


async def test_failed_current_recovery_does_not_fallback_or_rewind_new_native_domain_records(tmp_path):
    service, directory, destination, _, private, identity = await recovery_needed(tmp_path)
    class Failed(Supervisor):
        calls = 0
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            self.calls += 1
            (destination / "data/ledger.db").write_bytes(b"forward-recovery domain receipt")
            (destination / ".mobkit/state/native.db").write_bytes(b"forward-recovery native occurrence")
            raise RuntimeError("the actual current Console did not become healthy")
    supervisor = Failed(directory)
    with pytest.raises(RuntimeError, match="Console did not become healthy"):
        await update_runtime(supervisor, "recover-current", update_id=identity)
    receipt = read_receipt(directory)
    assert supervisor.calls == 1
    assert receipt["build_id"] == service.implementation.identity
    assert receipt["update"]["status"] == "recovery-needed" and receipt["health"] == "unknown"
    assert "Console did not become healthy" in receipt["update"]["recovery_error"]
    assert (destination / "data/ledger.db").read_bytes() == b"forward-recovery domain receipt"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"forward-recovery native occurrence"
    assert (destination / ".env").read_bytes() == private


async def test_current_recovery_rejects_private_configuration_failure_before_native_start_without_echoing_secrets(tmp_path):
    service, directory, destination, previous, private, identity = await recovery_needed(tmp_path)
    attempt = "12345678-1234-1234-1234-123456789abc"
    update_receipt(directory, startup_attempt_id=attempt,
                   startup_console_health={"attempt_id": attempt, "status": "fail", "category": "authentication", "attempts": 1})
    await service.action(service.guide.identity, previous["deployment_id"], "recover-current", update_id=identity)
    await service.shutdown()
    before = (destination / ".mobkit/state/native.db").read_bytes()
    class MissingConfiguration(Supervisor):
        async def command(self, args, *other, **kwargs):
            assert args[-1] == "--doctor"
            raise RuntimeError("private-api-key-value is not valid for this configured connection")
        async def start(self, *args, **kwargs): raise AssertionError("invalid private configuration must not start native agents")
    with pytest.raises(RuntimeError):
        await update_runtime(MissingConfiguration(directory), "recover-current", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed" and receipt["health"] == "unknown"
    assert receipt["startup_attempt_id"] is None and receipt["startup_console_health"] is None
    assert "private-api-key-value" not in json.dumps(receipt)
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes() == before


async def test_worker_entry_runs_exact_admitted_current_recovery_through_owned_transaction(tmp_path, monkeypatch):
    from launchkit import deployment_worker as worker
    service, directory, destination, previous, private, identity = await recovery_needed(tmp_path)
    original = updates.retain_update_admission(directory, read_receipt(directory)["update"], "recover-current")
    await service.action(service.guide.identity, previous["deployment_id"], "recover-current", update_id=identity)
    await service.shutdown()
    observed = []
    class Ready(Supervisor):
        async def command(self, args, *other, **kwargs):
            observed.append(args[-1])
        async def start(self, definition, *, on_ready):
            observed.append("start")
            assert definition == service.implementation.definition
            assert (destination / ".mobkit/state/native.db").read_bytes() == b"latest retained native occurrence"
            assert (destination / "data/ledger.db").read_bytes() == b"latest retained domain receipt"
            on_ready()
    async def owned_input(supervisor):
        await supervisor.stop.wait()
        return "stopped"
    monkeypatch.setattr(worker, "Supervisor", Ready)
    monkeypatch.setattr(worker, "owner_input", owned_input)
    await worker.run(directory, "recover-current", update_selection=original)
    receipt = read_receipt(directory)
    assert observed == ["--doctor", "--application-check", "start"]
    assert receipt["update"]["status"] == "completed"
    assert receipt["build_id"] == service.implementation.identity != previous["build_id"]
    assert (destination / ".env").read_bytes() == private


@pytest.mark.parametrize("admission", [{}, {"action": "rollback"}, {"action": "recover-current"},
                                    {"action": "recover-current", "update_id": "different-transaction"}])
async def test_worker_current_recovery_entry_rejects_missing_or_different_admission(tmp_path, monkeypatch, admission):
    from launchkit import deployment_worker as worker
    _, directory, destination, previous, private, identity = await recovery_needed(tmp_path)
    original = updates.retain_update_admission(directory, read_receipt(directory)["update"], "recover-current")
    selected = {"update_id": identity, **admission} if admission.get("action") == "rollback" else admission
    update_receipt(directory, update_action=selected)
    before_receipt = read_receipt(directory)
    before = (destination / "application/logic.py").read_bytes()
    class Forbidden(Supervisor):
        async def command(self, *args, **kwargs):
            raise AssertionError("Unadmitted recovery must not execute authored source.")
        async def start(self, *args, **kwargs):
            raise AssertionError("Unadmitted recovery must not start the native runtime.")
    async def owned_input(supervisor):
        await supervisor.stop.wait()
        return "stopped"
    monkeypatch.setattr(worker, "Supervisor", Forbidden)
    monkeypatch.setattr(worker, "owner_input", owned_input)
    with pytest.raises(ValueError, match="The original user-admitted update action changed"):
        await worker.run(directory, "recover-current", update_selection=original)
    receipt = read_receipt(directory)
    assert receipt == before_receipt and receipt["update"]["status"] == "recovery-needed"
    assert receipt["build_id"] == previous["build_id"]
    assert (destination / "application/logic.py").read_bytes() == before
    assert (destination / ".env").read_bytes() == private
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"latest retained native occurrence"
    assert (destination / "data/ledger.db").read_bytes() == b"latest retained domain receipt"


async def test_interrupted_current_recovery_is_retriable_using_same_source_and_retained_records(tmp_path):
    service, directory, destination, _, private, identity = await recovery_needed(tmp_path)
    class Interrupted(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            (destination / "data/ledger.db").write_bytes(b"interrupted recovery domain receipt")
            (destination / ".mobkit/state/native.db").write_bytes(b"interrupted recovery native occurrence")
            self.stop.set()
            raise asyncio.CancelledError
    with pytest.raises(asyncio.CancelledError):
        await update_runtime(Interrupted(directory), "recover-current", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "recovery-needed"
    class Ready(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready):
            assert (destination / "data/ledger.db").read_bytes() == b"interrupted recovery domain receipt"
            assert (destination / ".mobkit/state/native.db").read_bytes() == b"interrupted recovery native occurrence"
            on_ready()
    await update_runtime(Ready(directory), "recover-current", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "completed"
    assert (destination / ".env").read_bytes() == private


async def test_current_recovery_hard_interruption_and_partial_source_copy_have_durable_exact_recovery(tmp_path, monkeypatch):
    service, directory, destination, previous, private, identity = await recovery_needed(tmp_path)
    apply = updates.apply_files
    def lost_owner(target, before, after):
        content, mode = after["application/logic.py"]
        updates.replace_file(target / "application/logic.py", content, mode)
        raise OSError("fixture owner disappeared during source copy")
    monkeypatch.setattr(updates, "apply_files", lost_owner)
    with pytest.raises(OSError): updates.retry_candidate(directory, identity)
    assert read_receipt(directory)["update"]["status"] == "recovering-current"
    update_receipt(directory, status="checking", phase="recovery")
    fresh = Service(service.root.parent, service.guide, service.implementation)
    receipt = await fresh.status(service.guide.identity)
    assert receipt["status"] == "interrupted"
    with pytest.raises(DraftConflict, match="different update"):
        await update_runtime(Supervisor(directory), "recover-current", update_id=str(uuid.uuid4()))
    monkeypatch.setattr(updates, "apply_files", apply)
    class Ready(Supervisor):
        async def command(self, *args, **kwargs): pass
        async def start(self, definition, *, on_ready): on_ready()
    await update_runtime(Ready(directory), "recover-current", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "completed"
    assert (destination / "data/ledger.db").read_bytes() == b"latest retained domain receipt"
    assert (destination / ".mobkit/state/native.db").read_bytes() == b"latest retained native occurrence"
    assert (destination / ".env").read_bytes() == private


async def test_current_recovery_refuses_busy_lease_modified_source_and_linked_private_state(tmp_path):
    _, directory, destination, _, _, identity = await recovery_needed(tmp_path)
    with project_lock(destination):
        with pytest.raises(RuntimeError, match="already"):
            updates.retry_candidate(directory, identity)
    original = destination / "application/logic.py"
    before = original.read_bytes()
    original.write_text("user-edited source")
    with pytest.raises(DraftConflict, match="modified outside"):
        updates.retry_candidate(directory, identity)
    assert original.read_text() == "user-edited source"
    original.write_bytes(before)
    import os
    os.link(destination / ".env", tmp_path / "unowned-private-copy")
    with pytest.raises(DraftConflict, match="unowned file or link"):
        updates.retry_candidate(directory, identity)
    assert read_receipt(directory)["update"]["status"] == "recovery-needed"
