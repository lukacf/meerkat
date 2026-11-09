"""Exact model-only setup repair and native readiness, using fixture stores."""
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import tomllib
from types import SimpleNamespace

import pytest

from launchkit import architect as module
from launchkit import native_definition_continuity, native_models
from launchkit.native_models import (is_legacy_registry_removal, legacy_registry_successor,
    model_catalog_context, model_registry_lines, supplemental_model_metadata)
from test_architect_startup_recovery import FakeRuntime, startup
from test_meta_guide_runtime import selected as selected_guide_runtime


def old_metadata():
    return supplemental_model_metadata(native_version="0.8.42", core_version=None)[0]["metadata"]


def definition():
    profiles = {}
    for name in ("architect", "connectors", "reviewer", "visualizer"):
        profiles[name] = {"model": "gpt-6.1-sol", "provider": "openai",
            "self_hosted_server_id": None, "image_generation_provider": None,
            "auto_compact_threshold": 321987 if name in {"architect", "visualizer"} else None,
            "resume_overrides": ["model", "provider", "provider_params"], "skills": [],
            "tools": {"builtins": False, "shell": False, "comms": True, "memory": name != "visualizer",
                "workgraph": False, "mob": False, "schedule": False, "image_generation": False,
                "read_only": False, "mcp": [], "mcp_servers": [], "rust_bundles": []},
            "peer_description": "", "external_addressable": True, "backend": None,
            "runtime_mode": "turn_driven", "max_inline_peer_notifications": None, "output_schema": None,
            "provider_params": {"provider_tag": {"provider": "open_ai",
                "reasoning_effort": "medium" if name == "visualizer" else "low",
                "supports_reasoning_override": True}}}
    return {"runtime": {}, "id": "launch-architect", "orchestrator": None, "profiles": profiles,
        "models": {"gpt-6.1-sol": old_metadata()}, "image_generation_provider": None,
        "wiring": {"auto_wire_orchestrator": False, "role_wiring": []}, "skills": {},
        "backend": {"default": "session", "external": None}, "flows": {}, "topology": None,
        "supervisor": None, "limits": None, "spawn_policy": None, "event_router": None}


def write_spec(root, spec, revision=7, *, raw=None, blob=False):
    state = root / ".mobkit/state"
    state.mkdir(parents=True, mode=0o700, exist_ok=True)
    root.chmod(0o700)
    (root / ".mobkit").chmod(0o700)
    state.chmod(0o700)
    database = state / "event_log.sqlite3"
    content = raw if raw is not None else json.dumps({"revision": revision, "definition": spec}, sort_keys=True)
    encoded = content if type(content) is bytes else content.encode("utf-8")
    with sqlite3.connect(database) as fixture_store:
        fixture_store.execute("CREATE TABLE IF NOT EXISTS mob_specs (mob_id TEXT PRIMARY KEY, spec_json TEXT NOT NULL)")
        fixture_store.execute("INSERT OR REPLACE INTO mob_specs VALUES (?, ?)",
                              ("launch-architect", encoded if blob else content))
    database.chmod(0o600)
    return encoded


@pytest.fixture
def registry(tmp_path):
    root = tmp_path / "setup"
    original = definition()
    raw = write_spec(root, original)
    qualification = {"native": {"rpc_gateway": {"binary_sha256": "1" * 64},
                                "mobkit_gateway": {"binary_sha256": "2" * 64}}}
    emitted = module._definition_toml(legacy_registry_successor(original))
    return SimpleNamespace(root=root, original=original, raw=raw,
        qualification=qualification, emitted=emitted, receipt=root / ".mobkit" / module.SETUP_REGISTRY_RECEIPT,
        completed=root / ".mobkit" / module.SETUP_REGISTRY_COMPLETED)


def prepare(fixture, **kwargs):
    return module._prepare_setup_registry(fixture.root, fixture.qualification, fixture.emitted, **kwargs)


def test_current_release_has_no_custom_builtin_declaration_and_old_support_is_exact():
    selected = [("openai", "gpt-6.1-sol"), ("openai", "gpt-6.1-sol")]
    assert model_registry_lines(selected) == []
    assert supplemental_model_metadata() == []
    context = model_catalog_context()
    assert context["native_version"] == "0.8.47" and context["core_version"] == "0.8.51"
    assert context["status"] == "unknown" and context["models"] == []
    older = tomllib.loads("\n".join(model_registry_lines(selected, native_version="0.8.42", core_version=None)))
    assert older["models"] == {"gpt-6.1-sol": old_metadata()}


def test_legacy_exact_export_keeps_setup_serializer_type_sensitive():
    assert native_models._exact is native_definition_continuity._exact
    assert len({native_models._exact({"value": value}) for value in (True, 1, 1.0)}) == 3
    successor = legacy_registry_successor(definition())
    transported = tomllib.loads(module._definition_toml(successor))
    assert transported["models"] == {}
    for role in ("architect", "connectors", "reviewer", "visualizer"):
        expected, actual = successor["profiles"][role], transported["profiles"][role]
        assert actual["model"] == expected["model"]
        assert actual["provider"] == expected["provider"]
        assert actual["provider_params"] == expected["provider_params"]
        assert actual.get("auto_compact_threshold") == expected["auto_compact_threshold"]


@pytest.mark.parametrize("native_version,core_version", [
    ("0.8.46", None), ("0.8.46", "0.8.50"), ("0.8.42", "0.8.51"),
    ("0.8.48", None), ("0.8.48", "0.8.50"), ("0.8.48", "0.8.52"), ("0.8.49", "0.8.51")])
def test_unknown_release_pair_never_guesses_model_declarations(native_version, core_version):
    with pytest.raises(ValueError, match="exact supported"):
        model_registry_lines([("openai", "gpt-6.1-sol")], native_version=native_version, core_version=core_version)


def test_explicit_048_core051_uses_native_model_without_changing_default_release():
    selected = [("openai", "gpt-6.1-sol"), ("anthropic", "claude-sonnet-4.5")]
    assert native_models.VERSION == "0.8.47"
    assert model_registry_lines(selected, native_version="0.8.48", core_version="0.8.51") == []
    assert supplemental_model_metadata(native_version="0.8.48") == []
    context = model_catalog_context(native_version="0.8.48")
    assert context["native_version"] == "0.8.48" and context["core_version"] == "0.8.51"
    assert context["status"] == "unknown" and context["supplemental_models"] == []
    previous = definition()
    before = copy.deepcopy(previous)
    successor = legacy_registry_successor(previous, native_version="0.8.48", core_version="0.8.51")
    assert previous == before and successor == {**before, "models": {}}
    assert is_legacy_registry_removal(previous, successor)
    for field in ("profiles", "skills", "wiring", "flows"):
        assert successor[field] == before[field]


@pytest.mark.parametrize("core_version", [None, "0.8.50", "0.8.52", True])
def test_048_registry_repair_refuses_unqualified_core_tuple(core_version):
    previous = definition()
    before = copy.deepcopy(previous)
    with pytest.raises(ValueError, match="Core 0.8.51"):
        legacy_registry_successor(previous, native_version="0.8.48", core_version=core_version)
    assert previous == before


def test_pure_model_successor_preserves_every_other_native_field_and_original():
    previous = definition()
    before = copy.deepcopy(previous)
    current = legacy_registry_successor(previous)
    assert previous == before
    assert current["models"] == {} and current["profiles"] == previous["profiles"]
    assert is_legacy_registry_removal(previous, current)
    omitted = copy.deepcopy(current)
    del omitted["models"]
    assert is_legacy_registry_removal(previous, omitted)
    for name in previous:
        if name != "models":
            assert current[name] == previous[name]


@pytest.mark.parametrize("mutate", [
    lambda value: value["models"]["gpt-6.1-sol"].update(vision=1),
    lambda value: value["models"]["gpt-6.1-sol"].update(context_window=1050000.0),
    lambda value: value["models"]["gpt-6.1-sol"].update(call_timeout_secs=599),
    lambda value: value["models"].update({"private-custom-model": {"provider": "openai"}}),
    lambda value: value["models"]["gpt-6.1-sol"].update(extra=True),
])
def test_unknown_or_changed_custom_model_is_preserved_and_refused(mutate):
    previous = definition()
    mutate(previous)
    before = copy.deepcopy(previous)
    with pytest.raises(ValueError, match="exact legacy"):
        legacy_registry_successor(previous)
    assert not is_legacy_registry_removal(previous, {**previous, "models": {}})
    assert previous == before


@pytest.mark.parametrize("mutate", [
    lambda value: value["profiles"]["architect"].update(model="gpt-6-luna"),
    lambda value: value["profiles"]["architect"].update(auto_compact_threshold=200000),
    lambda value: value["profiles"]["visualizer"]["provider_params"]["provider_tag"].update(reasoning_effort="low"),
    lambda value: value["wiring"].update(auto_wire_orchestrator=True),
])
def test_pure_comparison_refuses_any_other_definition_change(mutate):
    previous = definition()
    current = legacy_registry_successor(previous)
    mutate(current)
    assert not is_legacy_registry_removal(previous, current)


def test_setup_model_update_retains_immutable_cas_authority_without_changing_native_row(registry):
    database = registry.root / ".mobkit/state/event_log.sqlite3"
    before = database.read_bytes(), database.stat().st_ino
    mob, revision, receipt = prepare(registry)
    assert revision == 7
    assert tomllib.loads(mob) == tomllib.loads(registry.emitted)
    assert tomllib.loads(mob).get("models", {}) == {}
    assert base64.b64decode(receipt["previous_spec"]) == registry.raw
    assert receipt["previous_spec_sha256"] == hashlib.sha256(registry.raw).hexdigest()
    assert receipt["successor_definition"] == legacy_registry_successor(registry.original)
    saved_receipt = registry.receipt.read_bytes(), registry.receipt.stat().st_ino
    assert registry.receipt.stat().st_mode & 0o777 == 0o600
    assert prepare(registry) == (mob, revision, receipt)
    assert saved_receipt == (registry.receipt.read_bytes(), registry.receipt.stat().st_ino)
    assert before == (database.read_bytes(), database.stat().st_ino)


@pytest.mark.parametrize("blob", [False, True])
def test_canonical_text_and_blob_preserve_exact_utf8_authority_bytes(registry, blob):
    registry.original["profiles"]["architect"]["peer_description"] = "A shared caf\u00e9 context"
    content = json.dumps({"revision": 7, "definition": registry.original}, indent=3, ensure_ascii=False) + "\n"
    registry.raw = write_spec(registry.root, registry.original, raw=content, blob=blob)
    registry.emitted = module._definition_toml(legacy_registry_successor(registry.original))
    database = registry.root / ".mobkit/state/event_log.sqlite3"
    before = database.read_bytes(), database.stat().st_ino
    _, revision, receipt = prepare(registry)
    assert revision == 7 and module._read_setup_spec(registry.root)[0] == content.encode("utf-8")
    assert base64.b64decode(receipt["previous_spec"]) == content.encode("utf-8")
    assert receipt["previous_spec_sha256"] == hashlib.sha256(content.encode("utf-8")).hexdigest()
    assert before == (database.read_bytes(), database.stat().st_ino)


@pytest.mark.parametrize(
    "raw", [b"\xffnot-utf8", b"x" * 2_000_001], ids=["invalid-utf8", "oversized"]
)
def test_invalid_or_oversized_canonical_blob_refuses_before_receipt(registry, raw):
    write_spec(registry.root, registry.original, raw=raw, blob=True)
    with pytest.raises(RuntimeError, match="could not be verified"):
        prepare(registry)
    assert not registry.receipt.exists() and not registry.completed.exists()


def test_setup_forward_recovery_accepts_only_exact_native_successor_without_second_update(registry):
    mob, _, receipt = prepare(registry)
    saved_receipt = registry.receipt.read_bytes(), registry.receipt.stat().st_ino
    write_spec(registry.root, receipt["successor_definition"], revision=8)
    assert prepare(registry) == (mob, None, receipt)
    module._verify_setup_registry(registry.root, receipt)
    assert saved_receipt == (registry.receipt.read_bytes(), registry.receipt.stat().st_ino)


def complete(registry):
    mob, _, transition = prepare(registry)
    write_spec(registry.root, transition["successor_definition"], revision=8, blob=True)
    module._verify_setup_registry(registry.root, transition)
    return mob, transition


def test_completed_model_repair_preserves_receipts_and_allows_later_explicit_update(registry):
    _, transition = complete(registry)
    retained = registry.receipt.read_bytes(), registry.receipt.stat().st_ino
    completed = registry.completed.read_bytes(), registry.completed.stat().st_ino
    proof = json.loads(completed[0])
    assert proof["native_revision"] == 8
    assert proof["registry_receipt_sha256"] == hashlib.sha256(retained[0]).hexdigest()
    assert proof["successor_definition_sha256"] == hashlib.sha256(
        json.dumps(transition["successor_definition"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    next_definition = copy.deepcopy(transition["successor_definition"])
    next_definition["profiles"]["architect"]["model"] = "gpt-6-luna"
    registry.emitted = module._definition_toml(next_definition)
    assert prepare(registry, explicit_revision=8) == (registry.emitted, None, None)
    assert retained == (registry.receipt.read_bytes(), registry.receipt.stat().st_ino)
    assert completed == (registry.completed.read_bytes(), registry.completed.stat().st_ino)


def test_completed_repair_returns_later_native_revision_to_ordinary_pinning(registry):
    _, transition = complete(registry)
    retained = registry.receipt.read_bytes(), registry.completed.read_bytes()
    later = copy.deepcopy(transition["successor_definition"])
    later["profiles"]["architect"]["auto_compact_threshold"] = 456789
    write_spec(registry.root, later, revision=9, blob=True)
    registry.emitted = module._definition_toml(later)
    assert prepare(registry) == (registry.emitted, None, None)
    assert retained == (registry.receipt.read_bytes(), registry.completed.read_bytes())


@pytest.mark.parametrize("key,value", [("format", True), ("native_revision", 8.0),
    ("registry_receipt_sha256", "f" * 64), ("phase", "ready")])
def test_changed_completion_never_discharges_retained_authority(registry, key, value):
    complete(registry)
    completed = json.loads(registry.completed.read_text())
    completed[key] = value
    registry.completed.write_text(json.dumps(completed))
    with pytest.raises(RuntimeError, match="completion changed"):
        prepare(registry, explicit_revision=8)


def test_orphaned_completion_never_authorizes_a_definition(registry):
    complete(registry)
    registry.receipt.unlink()
    with pytest.raises(RuntimeError, match="no retained update authority"):
        prepare(registry)


@pytest.mark.parametrize("revision,legacy", [(7, False), (8, True), (9, True)])
def test_completed_repair_cannot_cover_rollback_or_new_custom_registry(registry, revision, legacy):
    _, transition = complete(registry)
    current = copy.deepcopy(transition["successor_definition"])
    if legacy:
        current["models"] = {"gpt-6.1-sol": old_metadata()}
    write_spec(registry.root, current, revision=revision)
    with pytest.raises(RuntimeError, match="cannot authorize"):
        prepare(registry)


def test_interrupted_completion_publication_converges_without_replacing_original_receipt(registry, monkeypatch):
    from launchkit import runtime_dependencies
    _, _, transition = prepare(registry)
    write_spec(registry.root, transition["successor_definition"], revision=8, blob=True)
    retained = registry.receipt.read_bytes(), registry.receipt.stat().st_ino
    unlink = runtime_dependencies._unlink_publication
    def interrupt(path, identity):
        raise OSError("Fixture completion interruption")
    monkeypatch.setattr(runtime_dependencies, "_unlink_publication", interrupt)
    with pytest.raises(OSError, match="completion interruption"):
        module._verify_setup_registry(registry.root, transition)
    assert registry.completed.stat().st_nlink == 2
    monkeypatch.setattr(runtime_dependencies, "_unlink_publication", unlink)
    assert prepare(registry) == (registry.emitted, None, None)
    assert registry.completed.stat().st_nlink == 1
    assert retained == (registry.receipt.read_bytes(), registry.receipt.stat().st_ino)


def test_completion_never_labels_a_replaced_native_store_as_the_retained_store(registry):
    _, _, transition = prepare(registry)
    state = registry.root / ".mobkit/state"
    original = registry.root / ".mobkit/retained-state"
    state.rename(original)
    write_spec(registry.root, transition["successor_definition"], revision=8, blob=True)
    with pytest.raises(RuntimeError, match="native store changed"):
        module._verify_setup_registry(registry.root, transition)
    assert not registry.completed.exists() and (original / "event_log.sqlite3").exists()


@pytest.mark.parametrize("revision,change", [(7, False), (9, False), (8, True)])
def test_setup_recovery_refuses_changed_or_unbound_native_successor(registry, revision, change):
    _, _, receipt = prepare(registry)
    successor = copy.deepcopy(receipt["successor_definition"])
    if change:
        successor["profiles"]["architect"]["auto_compact_threshold"] += 1
    write_spec(registry.root, successor, revision=revision)
    before = registry.receipt.read_bytes()
    with pytest.raises(RuntimeError, match="neither"):
        prepare(registry)
    assert registry.receipt.read_bytes() == before


@pytest.mark.parametrize("key,value", [("format", True), ("expected_revision", 7.0),
    ("previous_spec_sha256", "f" * 64), ("phase", "ready"), ("core_version", "0.8.50")])
def test_changed_retained_model_authority_refuses(registry, key, value):
    _, _, receipt = prepare(registry)
    receipt[key] = value
    registry.receipt.write_text(json.dumps(receipt))
    with pytest.raises(RuntimeError, match="retained setup model update changed"):
        prepare(registry)


def test_retained_previous_duplicate_keys_never_satisfy_canonical_authority(registry):
    _, _, receipt = prepare(registry)
    forged = registry.raw.decode().replace('"revision": 7', '"revision": 7, "revision": 7')
    receipt["previous_spec"] = base64.b64encode(forged.encode()).decode()
    registry.receipt.write_text(json.dumps(receipt))
    with pytest.raises(RuntimeError, match="invalid"):
        prepare(registry)


def test_setup_model_update_reconciles_only_exact_interrupted_publication(registry, monkeypatch):
    from launchkit import runtime_dependencies
    unlink = runtime_dependencies._unlink_publication
    def interrupt(path, identity):
        raise OSError("Fixture interruption after exclusive publication")
    monkeypatch.setattr(runtime_dependencies, "_unlink_publication", interrupt)
    with pytest.raises(OSError, match="Fixture interruption"):
        prepare(registry)
    assert registry.receipt.stat().st_nlink == 2
    monkeypatch.setattr(runtime_dependencies, "_unlink_publication", unlink)
    assert prepare(registry)[1] == 7
    assert registry.receipt.stat().st_nlink == 1
    assert not list(registry.receipt.parent.glob("." + registry.receipt.name + ".publish-*"))


def test_setup_model_update_refuses_unrelated_hardlink(registry):
    prepare(registry)
    os.link(registry.receipt, registry.receipt.parent / "unrelated-hardlink")
    with pytest.raises(RuntimeError, match="unrelated hardlink"):
        prepare(registry)
    assert registry.receipt.stat().st_nlink == 2


def test_setup_model_update_does_not_combine_with_other_revision_activation(registry):
    with pytest.raises(RuntimeError, match="before another"):
        prepare(registry, explicit_revision=7)
    assert not registry.receipt.exists()


def test_setup_model_update_refuses_changed_model_selection_before_retaining_receipt(registry):
    registry.emitted = registry.emitted.replace('"gpt-6.1-sol"', '"gpt-6-luna"')
    with pytest.raises(RuntimeError, match="profile choices changed"):
        prepare(registry)
    assert not registry.receipt.exists()


@pytest.mark.parametrize("mutate", [
    lambda value: value.update(source_identity={"artifact_sha256": "a" * 64}),
    lambda value: value["profiles"]["architect"].update(unknown_native_key=True),
    lambda value: value["profiles"]["architect"].update(skills=[None]),
    lambda value: value["runtime"].update(dynamic_value=None),
    lambda value: value["profiles"]["architect"]["provider_params"]["provider_tag"].update(dynamic_value=None),
])
def test_unknown_or_unrepresentable_definition_is_never_dropped(registry, mutate):
    original = copy.deepcopy(registry.original)
    mutate(original)
    write_spec(registry.root, original)
    with pytest.raises(RuntimeError, match="definition"):
        prepare(registry)
    assert not registry.receipt.exists()


def test_builder_visual_reviewer_and_application_use_same_collision_free_builtin(selected_guide_runtime):
    from launchkit.implementation_worker import implementation_mob_toml
    from launchkit.visual_review_runtime import visual_review_mob_toml
    from launchkit.blueprint import starter_blueprint
    from launchkit.compiler import generate_files
    from launchkit.rehearsal_worker import rehearsal_mob_toml
    spec = starter_blueprint()
    for profile in spec.agent_classes:
        profile.provider, profile.model = "openai", "gpt-6.1-sol"
    for content in (implementation_mob_toml("openai", "gpt-6.1-sol"), visual_review_mob_toml(),
                    generate_files(spec)["config/mob.toml"], rehearsal_mob_toml(spec)):
        native = tomllib.loads(content)
        assert "models" not in native
        assert all(profile["model"] == "gpt-6.1-sol" and profile["provider"] == "openai"
                   for profile in native["profiles"].values())
    builder = tomllib.loads(implementation_mob_toml("openai", "gpt-6.1-sol"))
    assert set(builder["profiles"]) == {"implementation", "acceptance"}
    assert all(profile["provider_params"]["provider_tag"]["reasoning_effort"] == "low"
               for profile in builder["profiles"].values())


@pytest.mark.parametrize("surface", ["compiler", "generator"])
def test_historical_package_pin_retains_exact_selected_model_declaration(monkeypatch, surface):
    from launchkit import compiler, generator
    from launchkit.blueprint import starter_blueprint
    if surface == "compiler":
        source = compiler
        spec = starter_blueprint()
        for profile in spec.agent_classes:
            profile.provider, profile.model = "openai", "gpt-6.1-sol"
    else:
        source = generator
        spec = {"project": {"name": "Historical team", "slug": "historical-team",
                    "description": "Retain the original selected model and native release."},
                "agents": [{"id": "lead", "name": "Lead", "role": "Coordinate the work",
                            "instructions": "Answer the owner using the supplied workspace context."}],
                "connections": ["workspace"], "provider": "openai", "model": "gpt-6.1-sol", "target": "mac"}
    monkeypatch.setattr(source, "MOBKIT_VERSION", "0.8.42")
    files = source.generate_files(spec)
    native = tomllib.loads(files["config/mob.toml"])
    assert native["models"] == {"gpt-6.1-sol": old_metadata()}
    assert all(profile["model"] == "gpt-6.1-sol" and profile["provider"] == "openai"
               for profile in native["profiles"].values())
    assert "meerkat-mobkit==0.8.42\n" in files["requirements.txt"]
    assert json.loads(files["deployment.json"])["mobkitVersion"] == "0.8.42"


@pytest.mark.asyncio
async def test_setup_emission_uses_native_builtin_and_checks_each_logical_member(startup):
    service, controller = startup
    runtime = FakeRuntime()
    controller.outcomes = [runtime]
    assert (await service.start())["status"] == "ready"
    native = tomllib.loads(controller.mob_configs[0])
    assert "models" not in native
    assert all(profile["model"] == "gpt-6.1-sol" for profile in native["profiles"].values())
    assert runtime.member_calls == list(module._SETUP_IDENTITIES)
    assert runtime.identity_calls == list(module._SETUP_IDENTITIES)
    assert native["profiles"]["visualizer"]["provider_params"]["provider_tag"]["reasoning_effort"] == "medium"
    assert all(native["profiles"][name]["provider_params"]["provider_tag"]["reasoning_effort"] == "low"
               for name in ("architect", "connectors", "reviewer"))
    await service.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("status,error,sid", [
    ("broken", "Mock registry validation failure", "saved-native-sid"),
    ("unknown", None, "saved-native-sid"), ("retiring", None, "saved-native-sid"),
    ("completed", None, "saved-native-sid"), ("active", "Mock error", "saved-native-sid"),
    ("active", None, ""), ("active", None, None),
])
async def test_bootstrap_ready_never_hides_unavailable_native_member(startup, status, error, sid):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.members["architect:main"] = SimpleNamespace(status=status, error=error, current_session_id=sid)
    controller.outcomes = [runtime]
    with pytest.raises(RuntimeError, match="unavailable native member"):
        await service.start()
    assert service.runtime is None and service.state == "failed"
    assert runtime.shutdown_calls == 1 and controller.lock_events == ["acquired", "released"]
    assert runtime.member_calls == ["architect:main"]


@pytest.mark.asyncio
async def test_native_member_observation_failure_releases_only_after_owned_cleanup(startup):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.member_error = RuntimeError("Mock native observation failed")
    controller.outcomes = [runtime]
    with pytest.raises(RuntimeError, match="Mock native observation failed"):
        await service.start()
    assert runtime.shutdown_calls == 1 and controller.lock_events == ["acquired", "released"]
    assert service.runtime is None and service.state == "failed"


@pytest.mark.asyncio
async def test_active_member_must_belong_to_current_identity_session(startup):
    service, controller = startup
    runtime = FakeRuntime()
    runtime.identity_sessions["architect:main"] = "different-continuity-session"
    controller.outcomes = [runtime]
    with pytest.raises(RuntimeError, match="unavailable native member"):
        await service.start()
    assert runtime.shutdown_calls == 1 and controller.lock_events == ["acquired", "released"]
    assert runtime.member_calls == runtime.identity_calls == ["architect:main"]


@pytest.mark.asyncio
async def test_current_meta_guide_leaves_legacy_fixture_and_model_cas_to_native_owner(startup):
    service, controller = startup
    root = module.setup_runtime_root(service.root)
    write_spec(root, definition(), revision=7, blob=True)
    database = root / ".mobkit/state/event_log.sqlite3"
    before = database.read_bytes(), database.stat().st_ino
    runtime = FakeRuntime()
    controller.outcomes = [runtime]
    assert (await service.start())["status"] == "ready"
    # The inert native owner accepts startup. Host code neither interprets nor
    # rewrites this predecessor-format store and cannot qualify its migration.
    assert controller.spec_updates == []
    assert before == (database.read_bytes(), database.stat().st_ino)
    assert not (root / ".mobkit" / module.SETUP_REGISTRY_RECEIPT).exists()
    assert not (root / ".mobkit" / module.SETUP_REGISTRY_COMPLETED).exists()
    assert runtime.wait_calls
    await service.shutdown()


@pytest.mark.asyncio
async def test_current_meta_explicit_revision_uses_native_cas_without_legacy_host_repair(startup):
    service, controller = startup
    root = module.setup_runtime_root(service.root)
    write_spec(root, definition(), revision=7)
    database = root / ".mobkit/state/event_log.sqlite3"
    before = database.read_bytes(), database.stat().st_ino
    async def original_revision(builder):
        assert controller.lock_events == ["acquired"]
        assert controller.spec_updates == [7]
        assert before == (database.read_bytes(), database.stat().st_ino)
    controller.callback_runner = original_revision
    runtime = FakeRuntime()
    controller.outcomes = [runtime]
    assert (await service.start(expected_revision=7))["status"] == "ready"
    assert controller.spec_updates == [7] and controller.builds == 1
    assert before == (database.read_bytes(), database.stat().st_ino)
    assert not (root / ".mobkit" / module.SETUP_REGISTRY_RECEIPT).exists()
    assert runtime.wait_calls
    await service.shutdown()


@pytest.mark.asyncio
async def test_native_rejection_keeps_legacy_fixture_without_claiming_ready(startup):
    service, controller = startup
    root = module.setup_runtime_root(service.root)
    write_spec(root, definition(), revision=7)
    database = root / ".mobkit/state/event_log.sqlite3"
    before = database.read_bytes(), database.stat().st_ino
    runtime = FakeRuntime(ready=False, broken=1)
    controller.outcomes = [runtime]
    with pytest.raises(RuntimeError, match="did not become ready"):
        await service.start()
    assert controller.spec_updates == []
    assert before == (database.read_bytes(), database.stat().st_ino)
    assert runtime.shutdown_calls == 1 and service.runtime is None
    assert service.state == "failed"
    assert not (root / ".mobkit" / module.SETUP_REGISTRY_COMPLETED).exists()


def test_completed_046_setup_model_authority_survives_047_pair_without_another_cas(registry, monkeypatch):
    from launchkit import bootstrap
    fixture = registry
    monkeypatch.setattr(bootstrap, "VERSION", "0.8.46")
    _, pin, expected = prepare(fixture)
    assert pin == 7 and expected["version"] == "0.8.46"
    write_spec(fixture.root, expected["successor_definition"], revision=8)
    module._verify_setup_registry(fixture.root, expected)
    old_receipt, old_completion = fixture.receipt.read_bytes(), fixture.completed.read_bytes()
    native = (fixture.root / ".mobkit/state/event_log.sqlite3").read_bytes()
    monkeypatch.setattr(bootstrap, "VERSION", "0.8.47")
    current_pair = {"native": {"rpc_gateway": {"binary_sha256": "3" * 64},
                               "mobkit_gateway": {"binary_sha256": "4" * 64}}}
    assert module._prepare_setup_registry(fixture.root, current_pair, fixture.emitted) == (fixture.emitted, None, None)
    assert fixture.receipt.read_bytes() == old_receipt and fixture.completed.read_bytes() == old_completion
    assert (fixture.root / ".mobkit/state/event_log.sqlite3").read_bytes() == native
