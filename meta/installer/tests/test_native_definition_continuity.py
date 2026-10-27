"""Native full-definition continuity and carrierless framework refresh boundaries."""
import asyncio
import copy
import hashlib
import importlib.metadata
import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit import native_definition_continuity as continuity
from launchkit import native_models
from launchkit import deployment_updates as updates
from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.deployment_worker import NATIVE_DEFINITION_READ, Supervisor
from test_application_extension import load_host
from test_deployment_updates import canonical_update_package
from test_runtime_registry_transition import native_observer
from test_runtime_dependency_updates import canonical_fixture_packages, registry_fixture_packages


def canonical_definition(mob_id="fixture", *, native=None):
    definition = {"runtime": {}, "id": mob_id, "orchestrator": None,
        "profiles": {"lead": {"model": "gpt-6.1-sol", "provider": "openai",
            "skills": ["role"], "tools": {"builtin": True},
            "provider_params": {"provider_tag": {"provider": "open_ai", "reasoning_effort": "low"}},
            "auto_compact_threshold": 200000}},
        "models": {continuity.LEGACY_MODEL_ID: copy.deepcopy(continuity.LEGACY_MODEL_METADATA)},
        "image_generation_provider": None,
        "wiring": {"auto_wire_orchestrator": False, "role_wiring": []},
        "skills": {"role": {"source": "inline", "content": "Preserve private case facts."}},
        "backend": {"default": "session", "external": None}, "flows": {},
        "topology": None, "supervisor": None, "limits": None,
        "spawn_policy": None, "event_router": None}
    if native is not None:
        definition["profiles"] = copy.deepcopy(native["profiles"])
        definition["skills"] = copy.deepcopy(native["skills"])
    return definition


def structural_page(mob_id="fixture", *, native=None):
    previous = canonical_definition(mob_id, native=native)
    current = continuity.legacy_registry_successor(previous)
    return {"events": [
        {"mob_id": mob_id, "kind": "mob_created", "cursor": 5,
            "data": {"type": "mob_created", "definition": previous}},
        {"mob_id": mob_id, "kind": "mob_definition_updated", "cursor": 91,
            "data": {"type": "mob_definition_updated", "epoch": 2, "definition": current}}],
        "next_after_seq": 91}


def test_full_native_pair_preserves_every_profile_and_uses_shared_metadata():
    page = structural_page()
    before = copy.deepcopy(page)
    proof = continuity.registry_role_continuity(page, "fixture", native_version="0.8.46")
    assert proof["definition_revision"] == 2
    assert proof["creation_cursor"] == 5 and proof["update_cursor"] == 91
    assert continuity.valid_continuity_projection(proof, "fixture", 2)
    assert proof["previous_definition_sha256"] != proof["current_definition_sha256"]
    assert page == before
    assert native_models.legacy_registry_successor is continuity.legacy_registry_successor
    assert native_models.is_legacy_registry_removal is continuity.is_legacy_registry_removal
    assert native_models.supplemental_model_metadata(native_version="0.8.42", core_version=None)[0]["metadata"] == continuity.LEGACY_MODEL_METADATA
    assert native_models.supplemental_model_metadata() == []
    assert continuity.continuity_query("fixture") == {"mob_id": "fixture",
        "event_types": ["mob_created", "mob_definition_updated", "mob_reset"], "after_seq": 0, "limit": 3}


@pytest.mark.parametrize("mutation", [
    "missing-creation", "creation-only", "third-update", "reset", "wrong-order", "duplicate-creation",
    "foreign-mob", "foreign-definition", "bool-cursor", "string-cursor", "float-cursor", "zero-cursor",
    "equal-cursor", "reverse-cursor", "bool-epoch", "float-epoch", "string-epoch", "later-epoch",
    "missing-definition", "missing-canonical-field", "unknown-event-field", "wrong-event-type",
    "missing-page-cursor", "bool-page-cursor", "wrong-page-cursor", "unknown-page-field",
])
def test_native_pair_never_infers_missing_or_malformed_history(mutation):
    page = structural_page()
    first, second = page["events"]
    if mutation == "missing-creation":
        page["events"] = [second]
    elif mutation == "creation-only":
        page["events"] = [first]
    elif mutation == "third-update":
        third = copy.deepcopy(second)
        third["cursor"], third["data"]["epoch"] = 99, 3
        page["events"].append(third)
        page["next_after_seq"] = 99
    elif mutation == "reset":
        page["events"].insert(1, {"mob_id": "fixture", "cursor": 41,
            "kind": "mob_reset", "data": {"type": "mob_reset"}})
    elif mutation == "wrong-order":
        page["events"].reverse()
    elif mutation == "duplicate-creation":
        second.update(kind="mob_created", data=copy.deepcopy(first["data"]))
    elif mutation == "foreign-mob":
        second["mob_id"] = "foreign"
    elif mutation == "foreign-definition":
        second["data"]["definition"]["id"] = "foreign"
    elif mutation in {"bool-cursor", "string-cursor", "float-cursor", "zero-cursor"}:
        first["cursor"] = {"bool-cursor": True, "string-cursor": "5", "float-cursor": 5.0, "zero-cursor": 0}[mutation]
    elif mutation in {"equal-cursor", "reverse-cursor"}:
        second["cursor"] = 5 if mutation == "equal-cursor" else 4
        page["next_after_seq"] = second["cursor"]
    elif mutation in {"bool-epoch", "float-epoch", "string-epoch", "later-epoch"}:
        second["data"]["epoch"] = {"bool-epoch": True, "float-epoch": 2.0, "string-epoch": "2", "later-epoch": 3}[mutation]
    elif mutation == "missing-definition":
        second["data"].pop("definition")
    elif mutation == "missing-canonical-field":
        for event in page["events"]:
            event["data"]["definition"].pop("profiles")
    elif mutation == "unknown-event-field":
        second["data"]["claimed_unchanged_roles"] = True
    elif mutation == "wrong-event-type":
        second["data"]["type"] = "mob_created"
    elif mutation == "missing-page-cursor":
        page.pop("next_after_seq")
    elif mutation == "bool-page-cursor":
        page["next_after_seq"] = True
    elif mutation == "wrong-page-cursor":
        page["next_after_seq"] = 92
    else:
        page["claimed_complete"] = True
    with pytest.raises(ValueError):
        continuity.registry_role_continuity(page, "fixture", native_version="0.8.46")


@pytest.mark.parametrize("mutation", ["skills", "model", "provider", "reasoning", "compaction",
    "tools", "wiring", "flows", "source", "unknown-authority", "metadata-float", "metadata-bool",
    "metadata-unknown", "foreign-model"])
def test_model_removal_never_masks_any_other_native_authority_change(mutation):
    page = structural_page()
    previous, current = [item["data"]["definition"] for item in page["events"]]
    profile = current["profiles"]["lead"]
    if mutation == "skills":
        current["skills"]["role"]["content"] += " A different role."
    elif mutation == "model":
        profile["model"] = "gpt-6-luna"
    elif mutation == "provider":
        profile["provider"] = "anthropic"
    elif mutation == "reasoning":
        profile["provider_params"]["provider_tag"]["reasoning_effort"] = "high"
    elif mutation == "compaction":
        profile["auto_compact_threshold"] = None
    elif mutation == "tools":
        profile["tools"]["builtin"] = False
    elif mutation == "wiring":
        current["wiring"]["auto_wire_orchestrator"] = True
    elif mutation == "flows":
        current["flows"] = {"new": {"steps": []}}
    elif mutation == "source":
        current["source_identity"] = {"namespace": "different"}
    elif mutation == "unknown-authority":
        previous["unrecognized"] = current["unrecognized"] = True
    elif mutation == "metadata-float":
        previous["models"][continuity.LEGACY_MODEL_ID]["context_window"] = 1050000.0
    elif mutation == "metadata-bool":
        previous["models"][continuity.LEGACY_MODEL_ID]["vision"] = 1
    elif mutation == "metadata-unknown":
        previous["models"][continuity.LEGACY_MODEL_ID]["unknown"] = True
    else:
        previous["models"]["foreign"] = copy.deepcopy(continuity.LEGACY_MODEL_METADATA)
    with pytest.raises(ValueError):
        continuity.registry_role_continuity(page, "fixture", native_version="0.8.46")


async def host_fixture(tmp_path, monkeypatch, *, page=None, carrier=False):
    import meerkat_mobkit
    from meerkat_mobkit.runtime import IdentityAgentHandle
    import tomllib

    module = load_host(monkeypatch, tmp_path)
    files = generate_files(starter_blueprint())
    native_path = tmp_path / "config/mob.toml"
    native_path.parent.mkdir()
    native_path.write_text(files["config/mob.toml"])
    deployment = json.loads(files["deployment.json"])
    native = tomllib.loads(files["config/mob.toml"])
    if page is None:
        page = structural_page(deployment["slug"], native=native)
    calls, admissions, verified = [], [], []

    class Runtime:
        def __init__(self, config):
            pass
        async def connect(self):
            pass
        async def shutdown(self):
            calls.append("shutdown")
        def mob_handle(self):
            return self
        def agent(self, identity):
            return IdentityAgentHandle(self, identity)
        async def query_mob_events(self, query):
            assert query == {"event_types": ["mob_created", "mob_definition_updated"], "limit": 1000}
            return [SimpleNamespace(kind="mob_definition_updated", data={"epoch": 2})]
        async def _rpc(self, method, params):
            calls.append((method, copy.deepcopy(params)))
            if method == "mobkit/capabilities":
                return {"host_system_prompt": {"send": carrier, "dispatch": carrier}}
            assert method == "mobkit/mob_events/query"
            assert params == continuity.continuity_query(deployment["slug"])
            return copy.deepcopy(page)
        async def send(self, identity, content, **kwargs):
            admissions.append(("send", identity, content, kwargs))
            return "native send admission"
        async def dispatch(self, identity, dispatch_input, **kwargs):
            admissions.append(("dispatch", identity, dispatch_input.to_dict(), kwargs))
            return "native keyed admission"

    monkeypatch.setattr(meerkat_mobkit, "MobKitRuntime", Runtime)
    monkeypatch.setattr(module, "verify_continuity_runtime",
        lambda *args, **kwargs: verified.append((args, kwargs)))
    application = module.Application(module.ApplicationContext(tmp_path, deployment, asyncio.Event()))
    runtime = await module.connect_application_runtime(SimpleNamespace(
        mob_config_path=str(native_path), gateway_bin=str(tmp_path / ".bin/rpc_gateway")), application)
    return runtime, deployment, page, calls, admissions, verified, native_path


async def test_shipping_sdk_inherits_unchanged_role_for_send_and_keyed_dispatch(tmp_path, monkeypatch):
    from meerkat_mobkit.identity_first_models import DispatchInput
    runtime, deployment, page, calls, admissions, verified, native_path = await host_fixture(tmp_path, monkeypatch)
    before = copy.deepcopy(page)
    native_bytes = native_path.read_bytes()
    identity = deployment["agents"][0]["identity"]
    message = DispatchInput(content="Keep my private corrected preference", origin="system",
        correlation_id="same-private-case", idempotency_key="same-key")
    assert await runtime.agent(identity).dispatch(message) == "native keyed admission"
    assert await runtime.agent(identity).send("Use that correction") == "native send admission"
    assert admissions == [("dispatch", identity, message.to_dict(), {}),
                          ("send", identity, "Use that correction", {})]
    assert len(verified) == 1
    assert [name for name, _ in calls] == ["mobkit/capabilities", "mobkit/mob_events/query", "mobkit/mob_events/query"]
    assert page == before and native_path.read_bytes() == native_bytes
    await runtime.shutdown()


async def test_later_native_role_change_is_refused_before_another_admission(tmp_path, monkeypatch):
    runtime, deployment, page, calls, admissions, _, _ = await host_fixture(tmp_path, monkeypatch)
    identity = deployment["agents"][0]["identity"]
    await runtime.send(identity, "Original unchanged role")
    third = copy.deepcopy(page["events"][-1])
    third["cursor"], third["data"]["epoch"] = 100, 3
    page["events"].append(third)
    page["next_after_seq"] = 100
    with pytest.raises(RuntimeError, match="no turn was sent"):
        await runtime.send(identity, "This must not enter a changed role")
    assert len(admissions) == 1
    assert len(calls) == 3
    await runtime.shutdown()


@pytest.mark.parametrize("failure", ["query", "release", "configuration", "foreign-role"])
async def test_carrierless_refusals_never_admit_or_resend_a_turn(tmp_path, monkeypatch, failure):
    runtime, deployment, page, calls, admissions, _, native_path = await host_fixture(tmp_path, monkeypatch)
    identity = deployment["agents"][0]["identity"]
    if failure == "query":
        async def broken_query(method, params):
            raise RuntimeError("native ledger unavailable")
        monkeypatch.setattr(runtime, "_rpc", broken_query)
    elif failure == "release":
        def unqualified(*args, **kwargs):
            raise RuntimeError("official release refused")
        monkeypatch.setitem(runtime.send.__func__.__globals__, "verify_continuity_runtime", unqualified)
    elif failure == "configuration":
        native_path.write_bytes(native_path.read_bytes() + b"\n# changed source\n")
    else:
        first_skill = next(iter(page["events"][1]["data"]["definition"]["skills"].values()))
        first_skill["content"] += " A different operational mandate."
    with pytest.raises(RuntimeError):
        await runtime.send(identity, "Not admitted")
    assert admissions == []
    await runtime.shutdown()


async def test_supported_native_carrier_needs_no_legacy_continuity_claim(tmp_path, monkeypatch):
    runtime, deployment, page, calls, admissions, verified, _ = await host_fixture(tmp_path, monkeypatch, carrier=True)
    # This stand-in has no explicit public system_prompt parameter, so the
    # existing host uses the raw supported carrier. Do not test that transport
    # here; the existing host-carrier suite covers its exact typed admission.
    supported, epoch = await runtime._observe_instruction_support()
    assert supported == {"send": True, "dispatch": True} and epoch is None
    assert await runtime._uses_instruction_carrier("send") is True
    assert await runtime._uses_instruction_carrier("dispatch") is True
    assert not verified and not admissions
    assert len(calls) == 1
    await runtime.shutdown()


def release_fixture(tmp_path, monkeypatch, version="0.8.46"):
    source = Path(__file__).parents[1] / "launchkit/templates/install_gateway.py"
    (tmp_path / "install_gateway.py").write_bytes(source.read_bytes())
    (tmp_path / "deployment.json").write_text(json.dumps({"mobkitVersion": version}))
    spec = importlib.util.spec_from_file_location("continuity_release_fixture", tmp_path / "install_gateway.py")
    release = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(release)
    directory = tmp_path / ".bin"
    directory.mkdir(mode=0o700)
    binary = directory / "rpc_gateway"
    binary.write_bytes(b"fixture executable bytes, never executed")
    binary.chmod(0o700)
    receipt = directory / "rpc_gateway.receipt.json"
    receipt.write_text(json.dumps({"version": version,
        "archive": f"mobkit-rpc-gateway-{version}-{release.platform_target()}.tar.gz",
        "release_url": release.RELEASE_URL, "archive_sha256": "4" * 64,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}))
    receipt.chmod(0o600)
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    monkeypatch.setattr(importlib.metadata, "version", lambda name: version)
    return binary, receipt


@pytest.mark.parametrize("mutation", [None, "package", "sdk", "override", "gateway", "binary",
    "receipt-source", "receipt-version", "receipt-url", "receipt-archive", "receipt-mode"])
def test_inherited_roles_require_existing_qualified_official_release_proof(tmp_path, monkeypatch, mutation):
    binary, receipt = release_fixture(tmp_path, monkeypatch)
    version, selected = "0.8.46", binary
    if mutation == "package":
        version = "0.8.42"
    elif mutation == "sdk":
        monkeypatch.setattr(importlib.metadata, "version", lambda name: "0.8.42")
    elif mutation == "override":
        monkeypatch.setenv("MOBKIT_RPC_GATEWAY_BIN", str(binary))
    elif mutation == "gateway":
        selected = tmp_path / "foreign_gateway"
    elif mutation == "binary":
        binary.write_bytes(b"changed bytes")
    elif mutation == "receipt-mode":
        receipt.chmod(0o644)
    elif mutation:
        record = json.loads(receipt.read_text())
        key, value = {"receipt-source": ("source", "explicit_override"),
            "receipt-version": ("version", "0.8.42"), "receipt-url": ("release_url", "https://example.test/release"),
            "receipt-archive": ("archive", "foreign-platform.tar.gz")}[mutation]
        record[key] = value
        receipt.write_text(json.dumps(record))
    if mutation is None:
        continuity.verify_continuity_runtime(tmp_path, binary, native_version=version)
    else:
        with pytest.raises(RuntimeError, match="no turn was sent"):
            continuity.verify_continuity_runtime(tmp_path, selected, native_version=version)


@pytest.mark.parametrize("mutation", [None, "wrong-mob", "bool-revision", "wrong-core", "bool-cursor",
    "unknown-proof", "duplicate-proof", "wrong-binary", "override-binary", "missing-proof"])
async def test_host_only_framework_observation_uses_native_pair_without_weakening_role_updates(
        tmp_path, monkeypatch, registry_fixture_packages, mutation):
    observer, files, commands = await native_observer(tmp_path, current=True, revision=2)
    definition = json.loads(files["deployment.json"][0])
    mob_id = definition["slug"]
    selected_version = observer.native_binary_provenance(files)["native_binary_version"]
    assert selected_version == definition["mobkitVersion"]
    proof = continuity.registry_role_continuity(structural_page(mob_id), mob_id, native_version=selected_version)
    if mutation == "wrong-mob":
        proof["mob_id"] = "foreign"
    elif mutation == "bool-revision":
        proof["definition_revision"] = True
    elif mutation == "wrong-core":
        proof["core_version"] = "0.8.50"
    elif mutation == "bool-cursor":
        proof["creation_cursor"] = True
    elif mutation == "unknown-proof":
        proof["claimed_role_adoption"] = True
    original_command = observer.command
    original_provenance = observer.native_binary_provenance

    async def command(*args, **kwargs):
        assert args[0][-1] == NATIVE_DEFINITION_READ
        text = await original_command(*args, **kwargs)
        if mutation == "missing-proof":
            return text
        record = json.dumps({"mob_id": mob_id, "source": "native role continuity observation", "continuity": proof})
        return text + "\n" + record + ("\n" + record if mutation == "duplicate-proof" else "")

    def provenance(checked):
        return {**original_provenance(checked),
            "native_binary_version": "0.8.42" if mutation == "wrong-binary" else selected_version,
            "native_binary_source": "explicit_override" if mutation == "override-binary" else "official-release"}

    monkeypatch.setattr(observer, "command", command)
    monkeypatch.setattr(observer, "native_binary_provenance", provenance)
    if mutation is None:
        result = await observer.native_definition_revision(files, require_carrier=False)
        assert result["definition_revision"] == 2 and result["native_role_continuity"] == proof
        assert result["native_binary_version"] == proof["native_version"] == selected_version
    else:
        with pytest.raises(RuntimeError, match="cannot author current role instructions"):
            await observer.native_definition_revision(files, require_carrier=False)
    with pytest.raises(RuntimeError, match="cannot author current role instructions"):
        await observer.native_definition_revision(files, require_carrier=True)
    assert commands == ["Native definition revision", "Native definition revision"]


@pytest.mark.parametrize("mutation", [None, "changed-helper", "helper-mode", "missing-helper", "foreign-provenance"])
def test_future_framework_refresh_adds_only_exact_protected_shared_helper(mutation):
    candidate = canonical_update_package(starter_blueprint())
    previous = copy.deepcopy(candidate)
    previous.pop("native_definition_continuity.py")
    previous["app.py"] = (b"# Earlier accepted protected host fixture\n", 0o644)
    old_baseline = {name: {"sha256": updates.digest(content), "mode": mode}
        for name, (content, mode) in previous.items() if name != "implementation-report.json"}
    old_fingerprint = updates.digest(json.dumps(old_baseline, ensure_ascii=True,
        sort_keys=True, separators=(",", ":")).encode())
    old_report = json.loads(previous["implementation-report.json"][0])
    old_report["framework"].update(basis="accepted-baseline", reason="unchanged-specification",
        baseline_digest=old_fingerprint, current_compiler_baseline_digest=old_fingerprint)
    previous["implementation-report.json"] = (json.dumps(old_report).encode(), 0o644)
    if mutation == "changed-helper":
        raw, mode = candidate["native_definition_continuity.py"]
        candidate["native_definition_continuity.py"] = (raw + b"# authored replacement\n", mode)
    elif mutation == "helper-mode":
        candidate["native_definition_continuity.py"] = (candidate["native_definition_continuity.py"][0], 0o755)
    elif mutation == "missing-helper":
        candidate.pop("native_definition_continuity.py")
    elif mutation == "foreign-provenance":
        report = json.loads(previous["implementation-report.json"][0])
        report["framework"]["blueprint_digest"] = "f" * 64
        previous["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
    changes, blockers = updates.compatibility(previous, candidate, {})
    assert previous["config/mob.toml"] == candidate["config/mob.toml"]
    if mutation is None:
        assert not blockers
        assert {entry["path"] for entry in changes} == {"app.py", "native_definition_continuity.py", "implementation-report.json"}
        assert updates.current_host_framework(candidate) and updates.has_host_instruction_carrier(candidate)
        exported = generate_files(starter_blueprint())["native_definition_continuity.py"]
        assert exported.encode() == Path(continuity.__file__).read_bytes()
        assert exported in NATIVE_DEFINITION_READ
    else:
        assert blockers


@pytest.mark.parametrize("carrier,changed_role", [(False, False), (False, True), (True, False)])
def test_owned_cold_reader_projects_native_pair_for_an_old_host_without_new_helper(
        tmp_path, monkeypatch, capsys, carrier, changed_role):
    # This invokes only the owned reader with an inert Python app/runtime stand-in.
    # No SDK process, native executable, provider request or admission is invoked.
    binary, _ = release_fixture(tmp_path, monkeypatch)
    page = structural_page("fixture")
    if changed_role:
        page["events"][1]["data"]["definition"]["skills"]["role"]["content"] += " Changed role."
    calls = []
    class Runtime:
        async def _rpc(self, method, params):
            calls.append((method, params))
            if method == "mobkit/capabilities":
                return {"host_system_prompt": {"send": carrier, "dispatch": carrier}}
            assert method == "mobkit/mob_events/query"
            assert params == continuity.continuity_query("fixture")
            return page
    application = SimpleNamespace(deployment={"slug": "fixture", "mobkitVersion": "0.8.46"},
        context=SimpleNamespace(root=tmp_path))
    config = SimpleNamespace(gateway_bin=str(binary))
    async def original_connect(config, application):
        return Runtime()
    app = SimpleNamespace(connect_application_runtime=original_connect)
    async def observe():
        await app.connect_application_runtime(config, application)
        print(json.dumps({"mob_id": "fixture", "definition_revision": 2,
            "source": "native structural event ledger"}))
    app.main = lambda: asyncio.run(observe())
    monkeypatch.setitem(sys.modules, "app", app)
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    exec(NATIVE_DEFINITION_READ, {"__name__": "__main__"})
    records = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert records[0]["host_system_prompt"] == {"send": carrier, "dispatch": carrier}
    assert records[-1]["definition_revision"] == 2
    if carrier:
        assert len(records) == 2 and len(calls) == 1
    elif changed_role:
        assert records[1]["continuity"] is None and len(calls) == 2
    else:
        assert records[1]["continuity"] == continuity.registry_role_continuity(page, "fixture", native_version="0.8.46")
        assert len(records) == 3 and len(calls) == 2


@pytest.mark.parametrize("version", ["0.8.47", "0.8.48"])
def test_same_core_continuity_projection_and_runtime_proof_are_bound_to_exact_release(tmp_path, monkeypatch, version):
    binary, receipt = release_fixture(tmp_path, monkeypatch, version)
    continuity.verify_continuity_runtime(tmp_path, binary, native_version=version)
    page = structural_page()
    projection = continuity.registry_role_continuity(page, "fixture", native_version=version)
    assert projection["native_version"] == version and projection["core_version"] == "0.8.51"
    assert continuity.valid_continuity_projection(projection, "fixture", 2)
    with pytest.raises(RuntimeError):
        continuity.verify_continuity_runtime(tmp_path, binary, native_version="0.8.46")
    with pytest.raises(ValueError):
        continuity.registry_role_continuity(page, "fixture", native_version="0.8.49")


@pytest.mark.parametrize("change", ["instructions", "tools", "epoch", "later-update", "core-projection"])
def test_048_role_continuity_still_requires_exact_registry_only_native_history(change):
    page = structural_page()
    current = page["events"][1]
    if change == "instructions":
        current["data"]["definition"]["skills"]["role"]["content"] += " Changed role instructions."
    elif change == "tools":
        current["data"]["definition"]["profiles"]["lead"]["tools"]["builtin"] = False
    elif change == "epoch":
        current["data"]["epoch"] = 3
    elif change == "later-update":
        later = copy.deepcopy(current)
        later["cursor"], later["data"]["epoch"] = 101, 3
        page["events"].append(later)
        page["next_after_seq"] = 101
    else:
        projection = continuity.registry_role_continuity(page, "fixture", native_version="0.8.48")
        projection["core_version"] = "0.8.52"
        assert not continuity.valid_continuity_projection(projection, "fixture", 2)
        return
    with pytest.raises(ValueError):
        continuity.registry_role_continuity(page, "fixture", native_version="0.8.48")


@pytest.mark.parametrize("change", ["sdk", "override", "gateway", "candidate-receipt", "changed-binary"])
def test_048_inherited_roles_keep_matching_sdk_and_official_runtime_admission(tmp_path, monkeypatch, change):
    binary, receipt = release_fixture(tmp_path, monkeypatch, "0.8.48")
    selected = binary
    if change == "sdk":
        monkeypatch.setattr(importlib.metadata, "version", lambda name: "0.8.47")
    elif change == "override":
        monkeypatch.setenv("MOBKIT_RPC_GATEWAY_BIN", str(binary))
    elif change == "gateway":
        selected = tmp_path / "candidate/rpc_gateway"
    elif change == "candidate-receipt":
        value = json.loads(receipt.read_bytes())
        value["source"] = "accepted-candidate"
        receipt.write_text(json.dumps(value))
    else:
        binary.write_bytes(b"Other candidate bytes, not the receipt's executable.")
    with pytest.raises(RuntimeError, match="no turn was sent"):
        continuity.verify_continuity_runtime(tmp_path, selected, native_version="0.8.48")
