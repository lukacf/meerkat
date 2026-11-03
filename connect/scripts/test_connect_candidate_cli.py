"""Offline CLI producer composition controls. No native build or acceptance."""
from pathlib import Path
import copy
import importlib.util
import json
import shutil

import pytest


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


p = load("candidate_cli", Path(__file__).with_name("connect-candidate.py"))
f = load("inert_assembler_cli", Path(__file__).with_name("test_assemble_connect_product.py"))


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value) + "\n")


@pytest.fixture(params=tuple(p.TARGETS))
def ready(tmp_path, monkeypatch, request):
    # Existing inert assembly fixtures exercise the real owner validators and
    # archive writer. None of their synthetic binaries can execute.
    original = f.InertInputs(tmp_path, request.param)
    original.select_explicit()
    work = tmp_path / "producer"
    cli = work / "cli"
    cli.mkdir(parents=True)
    resources = work / "resources/build.json"
    resources.parent.mkdir()
    shutil.copyfile(original.resource_record, resources)
    shutil.copyfile(original.component, resources.parent / ("meerkat-connect-resources-" + request.param + ".tar.gz"))
    original.resource_record = resources
    binary = cli / "binary/meerkat-connect"
    binary.parent.mkdir()
    shutil.copyfile(original.binary, binary)
    original.binary = binary
    original.record_value["binary"] = p.binding(binary)
    build = cli / "build-record.json"
    dump(build, original.record_value)
    candidate = original.composition_value["candidate"]
    dump(cli / "stage/staging.json", {"candidate": candidate,
        "build_record_sha256": p.digest(build), "resource_build_record_sha256": p.digest(resources)})
    shutil.copyfile(original.composition, cli / "installed-composition.json")
    pending = copy.deepcopy(original.shutdown_value)
    pending["disposition"] = "pending"
    dump(cli / "native-shutdown-review.pending.json", pending)
    value = {"target": request.param, "connect_version": "1.2.3"}
    dump(work / "inputs-selection.json", value)
    monkeypatch.setattr(p, "cli_producer_inputs", lambda supplied: (value, original.source, resources)
                        if supplied == work else pytest.fail("wrong retained producer"))
    calls = []

    def run(self, name, argv, **kwargs):
        calls.append([str(v) for v in argv])
        assert name == "cli-assemble"
        assert Path(argv[1]).name == "assemble-connect-product.py"
        original.record = Path(argv[argv.index("--build-record") + 1])
        destination = Path(argv[argv.index("--destination") + 1])
        original.assemble(destination=destination, release_version="1.2.3")
        return {}, None

    monkeypatch.setattr(p.Recorder, "run", run)
    return original, work, cli, calls, tmp_path / "final"


def test_final_cli_reuses_original_records_and_owner_without_building(ready):
    original, work, cli, calls, output = ready
    if original.target == "linux-x86_64":
        try:
            f.PACKAGER["require_case_sensitive"](work)
        except ValueError:
            pytest.skip("actual Linux archive owner requires a case-sensitive test filesystem")
    before = (cli / "build-record.json").read_bytes()
    p.assemble_cli(work, original.shutdown, p.digest(original.shutdown), output)
    assert len(calls) == 1
    assert (cli / "build-record.json").read_bytes() == before
    final = p.read_json(output / "assembled/assembly.json")
    assert final["scope"] == "conversation_catalog_local_preparation_and_help_preview"
    assert final["qualification"]["cli_tty_qualified"] is False
    assert "cli_terminal_interaction" in final["not_qualified"]
    assert final["binary_sha256"] == p.digest(original.binary)
    assert final["archive"].endswith(original.target + ".tar.gz")
    assert not (output / "RUNTIME-SELECTION.json").exists()


@pytest.mark.parametrize("change", ("pending", "rejected", "missing", "hash", "candidate", "evidence",
    "omitted_observation", "staged_build", "composition", "resource", "binary", "test_executable"))
def test_missing_failed_or_different_review_never_assembles(ready, change):
    original, work, cli, calls, output = ready
    review = original.shutdown
    accepted = p.read_json(review)
    if change in ("pending", "rejected"):
        accepted["disposition"] = change
    elif change == "candidate":
        accepted["candidate"]["binary"]["sha256"] = "1" * 64
    elif change == "evidence":
        original.evidence.write_text("changed raw observation")
    elif change == "omitted_observation":
        # A different valid file cannot stand in for the producer's raw result.
        other = cli / "unrelated.txt"
        other.write_text("unrelated")
        accepted["evidence"] = [p.binding(other)]
    elif change == "staged_build":
        dump(cli / "build-record.json", {"changed": True})
    elif change == "composition":
        value = p.read_json(cli / "installed-composition.json")
        value["execution"]["exit_code"] = 1
        dump(cli / "installed-composition.json", value)
    elif change == "resource":
        original.resource_record.write_text("changed")
    elif change == "binary":
        original.binary.write_bytes(b"changed binary")
    elif change == "test_executable":
        original.installed_test.write_bytes(b"changed fixture")
    dump(review, accepted)
    selected = "0" * 64 if change == "hash" else p.digest(review)
    if change == "missing":
        review.unlink()
    with pytest.raises((ValueError, FileNotFoundError)):
        p.assemble_cli(work, review, selected, output)
    assert calls == []
    assert not output.exists()


def test_compiler_selection_separates_production_and_test_profiles(tmp_path):
    log = tmp_path / "cargo.log"
    rows = [{"reason": "compiler-artifact", "target": {"name": "meerkat-connect"},
             "profile": {"test": test}, "executable": "/owned/" + name}
            for test, name in ((False, "production"), (True, "fixture"))]
    log.write_text("progress\n" + "\n".join(json.dumps(row) for row in rows))
    assert p.emitted_executable(log, "meerkat-connect", test=False) == Path("/owned/production")
    assert p.emitted_executable(log, "meerkat-connect", test=True) == Path("/owned/fixture")
    log.write_text(json.dumps(rows[0]) + "\n" + json.dumps(rows[0]))
    with pytest.raises(ValueError, match="one exact"):
        p.emitted_executable(log, "meerkat-connect", test=False)


@pytest.mark.parametrize("outcome", ("pass", "zero_tests", "failed", "missing_trace"))
def test_installed_test_outcome_is_execution_evidence_only(tmp_path, monkeypatch, outcome):
    record = p.Recorder(tmp_path)
    installed = tmp_path / "packaged_setup"
    installed.write_text("inert controlled test")

    def run(name, argv, *, cwd, env, seconds):
        assert argv == [installed, "--exact", "--ignored", p.CLI_SHUTDOWN, "--nocapture"]
        assert env["PATH"] == str(cwd / "empty-path")
        assert list((cwd / "empty-path").iterdir()) == []
        root = Path(env["TMPDIR"]) / "connect-active-worker-acceptance-inert"
        root.mkdir()
        if outcome != "missing_trace":
            (root / "native-dispatch.jsonl").write_text('{"inert":true}\n')
        log = record.records / (name + ".log")
        log.write_text("test result: ok. " + ("0" if outcome == "zero_tests" else "1")
                       + " passed; 0 failed; 0 ignored; 1 filtered out;\n")
        execution = {"exit_code": int(outcome == "failed"), "argv": list(map(str, argv)),
                     "log": p.binding(log)}
        dump(record.records / (name + ".json"), execution)
        if outcome == "failed":
            raise ValueError("command did not succeed")
        return execution, log

    monkeypatch.setattr(record, "run", run)
    if outcome == "pass":
        execution, evidence = p.cli_test_evidence(record, installed, p.CLI_SHUTDOWN,
            tmp_path / "run", "connect-active-worker-acceptance-")
        assert execution["exit_code"] == 0 and len(evidence) == 3
    else:
        with pytest.raises(ValueError):
            p.cli_test_evidence(record, installed, p.CLI_SHUTDOWN,
                tmp_path / "run", "connect-active-worker-acceptance-")
    assert not list(tmp_path.glob("*review*"))


@pytest.mark.parametrize("shutdown_ok", (True, False))
def test_stage_preserves_production_binary_and_never_accepts_review(tmp_path, monkeypatch, shutdown_ok):
    work = tmp_path / "producer"
    work.mkdir()
    source = work / "artifacts/source-manifest.json"
    dump(source, {"revision": "a" * 40})
    resources = work / "resources/build.json"
    dump(resources, {"resources_sha256": "b" * 64})
    selected = {"target": "macos-arm64", "connect_version": "1.2.3"}
    dump(work / "inputs-selection.json", selected)
    monkeypatch.setattr(p, "preflight", lambda **kwargs: (selected, "a" * 40, "c" * 40))
    monkeypatch.setattr(p, "cli_producer_inputs", lambda supplied: (selected, source, resources))
    emitted = tmp_path / "cargo-output"
    emitted.write_bytes(b"inert production executable")
    tested = tmp_path / "test-output"
    tested.write_bytes(b"inert fixture executable")
    calls = []

    def run(self, name, argv, **kwargs):
        calls.append(name)
        log = self.records / (name + ".log")
        if name == "cli-build":
            kind, path, is_test = "meerkat-connect", emitted, False
        elif name == "cli-test-build":
            emitted.write_bytes(b"dev-feature executable overwritten by test build")
            kind, path, is_test = "packaged_setup", tested, True
        else:
            assert name == "cli-stage"
            native = p.read_json(work / "cli/build-record.json")
            assert Path(native["binary"]["path"]).read_bytes() == b"inert production executable"
            candidate = {"target": selected["target"], "resources_sha256": "b" * 64,
                "binary": {key: native["binary"][key] for key in ("sha256", "size")}}
            dump(work / "cli/stage/staging.json", {"candidate": candidate})
            return {}, None
        log.write_text(json.dumps({"reason": "compiler-artifact", "target": {"name": kind},
            "profile": {"test": is_test}, "executable": str(path)}))
        actual = {"exit_code": 0, "argv": list(map(str, argv)), "env": kwargs["env"], "log": p.binding(log)}
        dump(self.records / (name + ".json"), actual)
        return actual, log

    def evidence(record, installed, name, directory, prefix):
        assert installed.read_bytes() == b"inert fixture executable"
        if name == p.CLI_SHUTDOWN and not shutdown_ok:
            raise ValueError("owned shutdown test failed")
        directory.mkdir()
        raw = directory / "raw-observation.json"
        dump(raw, {"inert": True, "test": name})
        return {"exit_code": 0, "argv": [str(installed), name]}, [p.binding(raw)]

    monkeypatch.setattr(p.Recorder, "run", run)
    monkeypatch.setattr(p, "cli_test_evidence", evidence)
    if shutdown_ok:
        p.stage_cli(work)
        pending = p.read_json(work / "cli/native-shutdown-review.pending.json")
        assert pending["disposition"] == "pending"
        assert pending["scope"] == "active_setup_worker_shutdown"
        assert pending["evidence"]
    else:
        with pytest.raises(ValueError, match="shutdown test failed"):
            p.stage_cli(work)
        assert not (work / "cli/native-shutdown-review.pending.json").exists()
    assert calls == ["cli-build", "cli-test-build", "cli-stage"]
    assert not (work / "cli/assembled").exists()


@pytest.mark.parametrize("change", (None, "source", "selection", "metadata", "native_revision",
                                    "resource", "source_reference"))
def test_cli_uses_exact_retained_source_graph_and_resource_selection(tmp_path, monkeypatch, change):
    root = tmp_path / "source"
    root.mkdir()
    leaf = root / "tracked.py"
    leaf.write_text("selected source bytes\n")
    work = tmp_path / "work"
    work.mkdir()
    metadata = work / "metadata.log"
    metadata.write_text('{"selected_graph":true}\n')
    value = {"native_release": {"core": "selected", "meta": "selected"}}
    selection = work / "inputs-selection.json"
    dump(selection, value)
    resources = work / "resources/build.json"
    dump(resources, {"resources_sha256": "b" * 64})
    source = work / "artifacts/source-manifest.json"
    dump(source, {"revision": "a" * 40, "input_selection": p.binding(selection),
        "files": [{"path": "tracked.py", "sha256": p.digest(leaf), "size": leaf.stat().st_size}],
        "metadata": p.binding(metadata), "native_release": value["native_release"]})
    native = {"source_manifest": p.binding(source), "resource_build_record_sha256": p.digest(resources),
              "resources_sha256": "b" * 64}
    monkeypatch.setattr(p, "ROOT", root)
    monkeypatch.setattr(p.subprocess, "check_output", lambda *args, **kwargs: "a" * 40 + "\n")
    if change == "source":
        leaf.write_text("unselected source")
    elif change == "selection":
        selection.write_text("changed")
    elif change == "metadata":
        metadata.write_text("changed graph")
    elif change == "native_revision":
        s = p.read_json(source)
        s["native_release"] = {"core": "different", "meta": "selected"}
        dump(source, s)
        native["source_manifest"] = p.binding(source)
    elif change == "resource":
        dump(resources, {"resources_sha256": "c" * 64})
    elif change == "source_reference":
        native["source_manifest"]["sha256"] = "c" * 64
    dump(work / "build-record.json", native)
    if change:
        with pytest.raises(ValueError):
            p.cli_producer_inputs(work)
    else:
        assert p.cli_producer_inputs(work) == (value, source, resources)
