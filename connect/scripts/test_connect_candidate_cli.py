"""Offline CLI producer composition controls. No native build or acceptance."""
from pathlib import Path
import copy
import importlib.util
import io
import json
import os
import shutil
import struct
import tarfile

import pytest


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


p = load("candidate_cli", Path(__file__).with_name("connect-candidate.py"))
f = load("inert_assembler_cli", Path(__file__).with_name("test_assemble_connect_product.py"))
# Release mode scans the assembled product; these rules are synthetic test data (a made-up marker).
PRIVACY = Path(__file__).with_name("fixtures") / "release-payload-privacy-scan"
SCAN = {"rules": PRIVACY / "synthetic-rules.json", "exemptions": PRIVACY / "synthetic-exemptions.json"}


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
    # A versioned assembly also requires the monorepo source identity of the
    # checkout it runs in (the assembler's own tests cover that check). This
    # synthetic producer has no Git checkout, so a stand-in records the call.
    released = []
    monkeypatch.setattr(f.ASSEMBLER, "checked_release_source", lambda raw: released.append(raw) or {
        "repository": "lukacf/meerkat", "revision": "a" * 40, "tree": "b" * 40,
        "source_manifest_sha256": p.digest(original.source)})

    def run(self, name, argv, **kwargs):
        calls.append([str(v) for v in argv])
        assert name == "cli-assemble"
        assert Path(argv[1]).name == "assemble-connect-product.py"
        original.record = Path(argv[argv.index("--build-record") + 1])
        destination = Path(argv[argv.index("--destination") + 1])
        original.assemble(destination=destination, release_version="1.2.3")
        assert released == [original.record.read_bytes()]
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
    p.assemble_cli(work, original.shutdown, p.digest(original.shutdown), output, **SCAN)
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
        p.assemble_cli(work, review, selected, output, **SCAN)
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


# ---- The complete macOS arm64 installation: `connect-candidate.py macos <step>` ---------------------
# Everything below is simulated on any host: Cargo, codesign, the other owners' scripts and every product
# executable are recorded stand-ins. No file written here can run. What only a macOS runner shows is not
# claimed by these tests.
CONNECT = Path(__file__).resolve().parents[1]
REVISION, TREE = "a" * 40, "b" * 40
RESOURCES = "c" * 64
LINKER_SIGNED, ADHOC = 0x20002, 0x2


def macho(*, cpu=0x0100000C, filetype=2, minimum=(11, 0), libraries=("/usr/lib/libSystem.B.dylib",),
          flags=LINKER_SIGNED, payload=b"", magic=0xFEEDFACF):
    """A synthetic thin Mach-O file: a header, load commands, a payload and a code signature blob."""
    commands = [struct.pack("<IIIIII", 0x32, 24, 1, (minimum[0] << 16) | (minimum[1] << 8), 15 << 16, 0)]
    for name in libraries:
        raw = name.encode() + b"\0"
        raw += b"\0" * (-len(raw) % 8)
        commands.append(struct.pack("<IIIIII", 0x0C, 24 + len(raw), 24, 0, 0, 0) + raw)
    blob = b""
    if flags is not None:
        directory = struct.pack(">IIII", 0xFADE0C02, 16, 0x20400, flags)
        blob = struct.pack(">III", 0xFADE0CC0, 20 + len(directory), 1) + struct.pack(">II", 0, 20) + directory
        commands.append(struct.pack("<IIII", 0x1D, 16, 0, len(blob)))
    body = b"".join(commands)
    data = struct.pack("<8I", magic, cpu, 0, filetype, len(commands), len(body), 0, 0) + body
    data += b"INERT - NEVER EXECUTE\n" + payload
    if blob:
        position = 32 + len(body) - 8
        data = data[:position] + struct.pack("<I", len(data)) + data[position + 4:] + blob
    return data


def resign(data, flags):
    """What the codesign stand-in does: the same file with another code directory flag word."""
    position = data.rindex(struct.pack(">I", 0xFADE0C02)) + 12
    return data[:position] + struct.pack(">I", flags) + data[position + 4:]


def option(argv, name):
    return Path(argv[argv.index(name) + 1])


class MacosLane:
    """One simulated macOS run. `sign` selects what the codesign stand-in leaves in the file."""

    ORDER = ("preflight", "library", "resources", "build", "stage", "check-gateway", "check-cli",
             "check-consumer", "check-conversation", "check-official-install")
    LAST = "check-official-install"

    def __init__(self, root, monkeypatch, *, sign=ADHOC):
        self.root, self.sign, self.calls = root, sign, []
        self.work = root / "cw"
        self.target = root / "cargo-target"
        self.target.mkdir()
        self.versions = p.macos_versions()
        self.gateway_status = "passed"
        self.after = {}
        self.chrome = root / "Inert Chrome"
        self.chrome.write_bytes(b"inert browser - never started")
        self.chrome.chmod(0o755)
        policy = CONNECT / "release/node-prebuilt-policy-macos-arm64.json"
        value = p.read_json(CONNECT / "release/connect-release-inputs-macos-arm64.json")
        # The committed record is pending. This completed copy stands for the integrated tree.
        value["library"] = {"state": "selected", "reason": "test selection",
            "pins": {"path": "release/node-prebuilt-policy-macos-arm64.json", "sha256": p.digest(policy)}}
        self.inputs = root / "selected-inputs.json"
        dump(self.inputs, value)
        self.selection = value
        self.pinned = root / "pins"
        rows = []
        for name, version in value["sdk_build_tools"].items():
            wheel = self.pinned / "pypi-wheel" / f"{name}-{version}-py3-none-any.whl"
            wheel.parent.mkdir(parents=True, exist_ok=True)
            wheel.write_bytes(("inert build tool " + name).encode())
            rows.append({"kind": "pypi-wheel", "name": name.replace("_", "-"), "version": version,
                         "file": wheel.name, "sha256": p.digest(wheel), "sha512": None, "use": ["component build tool"]})
        node = self.pinned / "release-download/node-v24.19.0-darwin-arm64.tar.gz"
        node.parent.mkdir()
        with tarfile.open(node, "w:gz") as archive:
            for leaf in ("bin/node", "lib/node_modules/npm/bin/npm-cli.js"):
                data = b"inert " + leaf.encode()
                info = tarfile.TarInfo("node-v24.19.0-darwin-arm64/" + leaf)
                info.size, info.mode = len(data), 0o755
                archive.addfile(info, io.BytesIO(data))
        rows.append({"kind": "release-download", "name": "node", "version": "v24.19.0", "file": node.name,
                     "sha256": p.digest(node), "sha512": None, "use": ["acquisition node", "runtime node"]})
        # The resolved pin file and the fetch step's record that binds it to the committed pins and the lock.
        self.pins = self.pinned / "library-pins-macos-arm64.resolved.json"
        dump(self.pins, {"schema": "connect-library-pins/1", "target": "macos-arm64", "pins": rows})
        self.fetched = {"schema": "connect-pinned-inputs/1", "pins_sha256": value["library"]["pins"]["sha256"],
            "lock_sha256": p.digest(CONNECT.parent / "Cargo.lock"), "files": [],
            "resolved_pins": {"file": self.pins.name, "rows": len(rows), "sha256": p.digest(self.pins)}}
        dump(self.pinned / "FETCHED.json", self.fetched)
        monkeypatch.setattr(p, "macos_input_file", lambda: self.inputs)
        monkeypatch.setattr(p, "require_native_host", lambda target: None)
        monkeypatch.setattr(p, "macos_checkout", lambda: (REVISION, TREE))
        monkeypatch.setattr(p, "macos_git", lambda *args: "1790000000")
        monkeypatch.setattr(p, "MACOS_WORK_ROOT_LIMIT", 4096)
        monkeypatch.setattr(p.shutil, "which", lambda name: "/opt/tools/bin/" + name)
        monkeypatch.setattr(p, "macos_conversation", self.conversation)
        lane = self

        def run(recorder, name, argv, *, cwd=None, env=None, seconds=600, inherit=True):
            return lane.command(recorder, name, [str(item) for item in argv], cwd, env or {}, inherit)

        monkeypatch.setattr(p.Recorder, "run", run)

    def command(self, recorder, name, argv, cwd, env, inherit):
        self.calls.append((name, argv, env, inherit))
        log = recorder.records / (name + ".log")
        assert not log.exists(), "a command name was used twice: " + name
        log.write_text(self.effect(name, argv, cwd, env))
        record = {"argv": argv, "cwd": str(cwd), "env": dict(env), "exit_code": 0, "timed_out": False,
                  "log": p.binding(log)}
        dump(recorder.records / (name + ".json"), record)
        if name in self.after:
            self.after[name]()
        return record, log

    def artifact(self, name, *, test=False, payload=b""):
        path = self.target / (name + ("-test" if test else ""))
        path.write_bytes(macho(payload=payload + name.encode()))
        return json.dumps({"reason": "compiler-artifact", "target": {"name": name},
                           "profile": {"test": test}, "executable": str(path)}) + "\n"

    def effect(self, name, argv, cwd, env):
        version = self.versions["connect"]
        if name.startswith("sign-verify-"):
            return ""
        if name.startswith("sign-"):
            assert argv[:4] == ["/usr/bin/codesign", "--force", "--sign", "-"]
            path = Path(argv[-1])
            if self.sign is not None:
                path.write_bytes(resign(path.read_bytes(), self.sign))
            return ""
        if name.startswith("version-"):
            return name.removeprefix("version-") + " " + version + " (inert)\n"
        if name == "rust-version":
            return "rustc 1.97.0 (inert)\nrelease: " + self.selection["rust_toolchain"] + "\nhost: aarch64-apple-darwin\n"
        if name == "source-record":
            dump(option(argv, "--output"), {"synthetic": True})
            return ""
        if name == "tool-dependencies":
            return ("    Updating crates.io index\nserde v1.0.228\nsyn v2.0.117 (proc-macro)\n"
                    "toolkit-package v" + version + " (/inert/connect/crates/toolkit-package)\nwarning: inert\n")
        if name == "rust-sysroot":
            return "/inert/toolchain\n"
        if name.startswith("tool-"):
            return self.artifact(name.removeprefix("tool-"))
        if name == "library-components":
            out = option(argv, "--output")
            out.mkdir()
            (out / "logs").mkdir()
            (out / "logs/build-1-activepieces-npm-ci.stdout").write_text("fetched https://registry.npmjs.org/esbuild\n")
            names = {"agno": "meerkat_toolkit_agno-0.1.5-py3-none-any.whl",
                     "services": "meerkat_toolkit_services-0.1.23-py3-none-any.whl",
                     "activepieces": "rkat-toolkit-activepieces-0.1.0.tgz"}
            rows = {}
            for label, leaf in names.items():
                (out / leaf).write_bytes(("inert component " + leaf).encode())
                rows[label] = {"file": leaf, "sha256": p.digest(out / leaf), "bytes": (out / leaf).stat().st_size}
            dump(out / "components.json", {"schema": "connect-library-components/1", "components": rows,
                 "reproducible": {label: {"equal": label != "activepieces"} for label in names}})
            return ""
        if name == "library-consumers":
            dump(option(argv, "--output") / "CONTENTS.json", {"schema_version": 1})
            dump(option(argv, "--closure-record"), {"schema_version": 1})
            return ""
        if name == "library-kit":
            (option(argv, "--output")).mkdir()
            (option(argv, "--output") / "agent-install-kit-macos-arm64.tar.gz").write_bytes(b"inert kit")
            return ""
        if name == "library-package":
            out = option(argv, "--output")
            out.mkdir()
            edition = argv[argv.index("--edition") + 1]
            (out / f"toolkit-library-{edition}-macos-arm64.tar.gz").write_bytes(b"inert library")
            dump(out / "build.json", {"schema_version": 2})
            return ""
        if name == "starter-export":
            dump(option(argv, "--destination") / "release-manifest.json", {"schema_version": 1})
            return ""
        if name == "resources":
            out = option(argv, "--destination")
            out.mkdir()
            archive = out / "meerkat-connect-resources-macos-arm64.tar.gz"
            archive.write_bytes(b"inert resource closure")
            dump(out / "build.json", {"schema_version": 1, "target": "macos-arm64", "resources_sha256": RESOURCES,
                                      "archive_sha256": p.digest(archive)})
            return ""
        if name == "native-build":
            assert env["MEERKAT_CONNECT_RESOURCES_SHA256"] == RESOURCES
            names = [argv[index + 1] for index, item in enumerate(argv) if item == "--bin"]
            return "".join(self.artifact(item, payload=RESOURCES.encode()) for item in names)
        if name == "test-build":
            assert env["MEERKAT_CONNECT_RESOURCES_SHA256"] == RESOURCES
            # Cargo may replace its ordinary output during a test build: the production copies must not follow.
            for item in ("toolkit_gateway", "meerkat-connect"):
                (self.target / item).write_bytes(b"replaced by the test build")
            names = [argv[index + 1] for index, item in enumerate(argv) if item == "--test"]
            return "".join(self.artifact(item, test=True, payload=RESOURCES.encode()) for item in names)
        if name == "rkat-build":
            return self.artifact("rkat")
        if name in ("sdk-build-venv", "verifier-venv"):
            (Path(argv[-1]) / "bin").mkdir(parents=True)
            (Path(argv[-1]) / "bin/python").write_text("inert\n")
            return ""
        if name.startswith("sdk-wheel-"):
            label = name.removeprefix("sdk-wheel-")
            distribution = {"toolkit": "meerkat-toolkit", "meta": "meerkat-meta"}[label]
            self.wheel(option(argv, "--outdir"), distribution, self.versions[label + "_sdk"])
            return ""
        if name == "verifier-site":
            return str(self.work / "verifier-venv/lib/site") + "\n"
        if name == "gateway-verify":
            out = option(argv, "--output")
            out.mkdir()
            dump(out / "manifest.json", {"status": self.gateway_status, "cases": []})
            (out / "trace.txt").write_text("read https://news.example.org/v0/item/1.json\n")
            return ""
        if name in (p.CLI_COMPOSITION, p.CLI_SHUTDOWN):
            prefix = "connect-product-acceptance-" if name == p.CLI_COMPOSITION else "connect-active-worker-acceptance-"
            retained = Path(env["TMPDIR"]) / (prefix + "inert")
            retained.mkdir()
            (retained / "native-dispatch.jsonl").write_text('{"inert":true}\n')
            return "running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out;\n"
        if name == p.CLI_CONSUMER:
            return "read https://news.example.org/top\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;\n"
        if name in ("gateway-stage", "cli-stage", "gateway-final", "cli-final"):
            out, kind = option(argv, "--destination"), argv[argv.index("--binary-kind") + 1]
            binary = option(argv, "--binary")
            assert p.digest(binary) == argv[argv.index("--binary-sha256") + 1]
            if name.endswith("-stage"):
                assert "--stage-only" in argv
                staged = out / "meerkat-connect-macos-arm64/bin" / kind
                staged.parent.mkdir(parents=True)
                shutil.copyfile(binary, staged)
                (staged.parent / "linked").symlink_to(kind)
                self.closure(out / "meerkat-connect-macos-arm64/libexec/meerkat-connect")
                dump(out / "staging.json", {"candidate": {"target": "macos-arm64", "binary_kind": kind,
                    "resources_sha256": RESOURCES, "binary": {"sha256": p.digest(binary), "size": binary.stat().st_size}},
                    "build_record_sha256": p.digest(option(argv, "--build-record"))})
            else:
                assert "--stage-only" not in argv
                leaf = ("meerkat-connect-gateway-" if kind == "toolkit_gateway" else "meerkat-connect-") + version \
                    + "-macos-arm64.tar.gz"
                out.mkdir()
                (out / leaf).write_bytes(("inert product " + kind).encode())
                (out / (leaf + ".sha256")).write_text(p.digest(out / leaf) + "  " + leaf + "\n")
                dump(out / "assembly.json", {"archive": leaf, "release_version": version,
                                             "binary_sha256": p.digest(binary)})
            return ""
        if name == "official-playwright-install":
            out = option(argv, "--destination")
            (out / "ready").mkdir(parents=True)
            dump(out / "ready/installed.json", {"schema_version": 1, "scope": self.official_scope,
                                                "family": "official-playwright"})
            dump(out / "ready/mcp.json", {"mcpServers": {}})
            (out / "npm-runtime.stdout").write_text("11.17.0\n")
            return ""
        assert name in ("cargo-version", "cargo-paths", "python-version", "system", "system-version", "system-sdk",
                        "zstd-version", "identity-check-before-build", "identity-check-after-build",
                        "sdk-build-tools", "sdk-build-inventory", "verifier-install", "verifier-inventory"), name
        return "inert\n"

    official_scope = "observed_software_installation_only"
    LIBRARY = "toolkit-library-20260921-connect-aaaaaaaa-macos-arm64"

    def closure(self, resources):
        """The staged resource closure, as far as the official install check reads it."""
        names = {"python": "runtime/python/bin/python3.11", "node": "runtime/node/bin/node",
                 "npm_cli": "runtime/node/lib/node_modules/npm/bin/npm-cli.js",
                 "archive": "archives/" + self.LIBRARY + ".tar.gz"}
        for key in ("python", "node", "npm_cli"):
            (resources / names[key]).parent.mkdir(parents=True, exist_ok=True)
            (resources / names[key]).write_bytes(("inert " + key).encode())
        (resources / "archives").mkdir()
        with tarfile.open(resources / names["archive"], "w:gz") as archive:
            for leaf, data in (("official/CONTENTS.json", b'{"format": 1}\n'),
                               ("setup/install-library-consumer.py", b"# inert installer\n")):
                info = tarfile.TarInfo(self.LIBRARY + "/" + leaf)
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        dump(resources / "resources.json", {"schema_version": 1, **names})

    def wheel(self, directory, distribution, version, requires=None):
        import zipfile
        leaf = distribution.replace("-", "_") + "-" + version + "-py3-none-any.whl"
        requires = requires if requires is not None else (
            ['meerkat-meta==' + self.versions["meta_sdk"] + '; extra == "native-host"']
            if distribution == "meerkat-toolkit" else [])
        with zipfile.ZipFile(directory / leaf, "w") as archive:
            archive.writestr(distribution.replace("-", "_") + "-" + version + ".dist-info/METADATA",
                "Metadata-Version: 2.4\nName: " + distribution + "\nVersion: " + version + "\n"
                + "".join("Requires-Dist: " + item + "\n" for item in requires))
        return directory / leaf

    def conversation(self, argv, cwd, environment, **bounds):
        self.calls.append(("conversation", [str(item) for item in argv], dict(environment), False))
        return {"timed_out": False, "closed_by": "terminal_interrupt", "alive_at_prompt": True,
                "quiet_at_prompt": True, "exit_code": 0, "transcript": self.transcript}

    transcript = ("Realm: conversation-check (/inert/state)\r\n" + p.CONVERSATION_PREPARING
                  + " You will review its access before the setup conversation opens.\r\n"
                  "The supplied setup helper asks for these permissions:\r\nContinue? ")

    def rkat_archive(self):
        archive = self.root / ("rkat-" + self.versions["core"] + "-aarch64-apple-darwin.tar.gz")
        data = macho(payload=b"rkat")
        with tarfile.open(archive, "w:gz") as output:
            info = tarfile.TarInfo("rkat")
            info.size, info.mode = len(data), 0o755
            output.addfile(info, io.BytesIO(data))
        return archive

    def step(self, name, **options):
        if name == "preflight":
            return p.macos_preflight(self.work, REVISION)
        if name in ("library", "resources"):
            return getattr(p, "macos_" + name)(self.work, self.pins, self.pinned)
        if name == "stage":
            return p.macos_stage(self.work, self.pins, self.pinned, **options)
        if name == "check-consumer":
            if not options:
                archive = self.rkat_archive()
                options = {"archive": archive, "archive_sha256": p.digest(archive)}
            return p.macos_check_consumer(self.work, **options)
        if name == "check-official-install":
            return p.macos_check_official_install(self.work, options.get("chrome", self.chrome))
        return getattr(p, "macos_" + name.replace("-", "_"))(self.work)

    def through(self, last, *, skip=()):
        for name in self.ORDER[:self.ORDER.index(last) + 1]:
            if name not in skip:
                self.step(name)
        return self

    def names(self):
        return [call[0] for call in self.calls]

    def stage_record(self, **options):
        return p.macos_stage_record(self.work, self.root / "stage", **options)

    def accepted_review(self):
        value = p.read_json(self.work / "cli-qualification/native-shutdown-review.pending.json")
        value["disposition"] = "accepted"
        review = self.root / "native-shutdown-review.accepted.json"
        dump(review, value)
        return review

    def privacy_report(self, *, status="clean", leave_out=None):
        """What the private-text scan hands back: counts and the digests of the files it read. No rule."""
        final = self.root / "final"
        paths = sorted([*final.glob("cli/*.tar.gz*"), *final.glob("gateway/*.tar.gz*")])
        report = self.root / "privacy-report.json"
        dump(report, {"schema": "inert", "status": status, "payload": [
            {"name": path.name, "bytes": path.stat().st_size, "sha256": p.digest(path)}
            for path in paths if leave_out is None or not path.name.startswith(leave_out)]})
        return report

    def assemble(self, **changes):
        review = changes.pop("review", None) or self.accepted_review()
        values = {"work": self.work, "stage_directory": self.root / "stage", "review": review,
                  "review_sha256": p.digest(review), "output": self.root / "final", **changes}
        return p.macos_assemble(**values)


@pytest.fixture
def lane(tmp_path, monkeypatch):
    return MacosLane(tmp_path, monkeypatch)


def test_committed_macos_selection_is_pending_and_refuses_every_step(tmp_path, monkeypatch):
    committed = p.read_json(CONNECT / "release/connect-release-inputs-macos-arm64.json")
    state = committed["library"]["state"]
    assert state in ("pending", "selected")
    if state == "pending":
        assert committed["library"]["pins"]["sha256"] is None
        assert committed["library"]["reason"].startswith("PENDING")
    else:
        # Completed at integration: the committed pin file is exactly the one the selection names.
        assert p.checked_macos_inputs(copy.deepcopy(committed)) == committed
        assert committed["library"]["pins"]["sha256"] == p.digest(CONNECT / committed["library"]["pins"]["path"])
    # A pending record refuses everything, whatever the committed file says today.
    pending = copy.deepcopy(committed)
    pending["library"]["state"] = "pending"
    pending["library"]["pins"]["sha256"] = None
    with pytest.raises(ValueError, match="unbound macOS producer inputs"):
        p.checked_macos_inputs(pending)
    # The legacy candidate preflight reads the same file and refuses it as unbound, as it always did.
    assert p.unbound(pending)
    selection = tmp_path / "pending-selection.json"
    dump(selection, pending)
    monkeypatch.setattr(p, "macos_input_file", lambda: selection)
    monkeypatch.setattr(p, "require_native_host", lambda target: None)
    monkeypatch.setattr(p, "macos_checkout", lambda: (REVISION, TREE))
    work = tmp_path / "cw"
    assert p.macos_main(["preflight", "--work", str(work), "--source-sha", REVISION]) == 1
    assert not (work / "records").exists() and not (work / "inputs-selection.json").exists()
    for step in p.MACOS_STEPS[1:]:
        assert p.macos_main([step, "--work", str(work)]) == 1
    assert sorted(path.name for path in work.iterdir()) == []


@pytest.mark.parametrize("change", ("state", "pins", "policy", "export", "extra", "toolchain"))
def test_macos_selection_refuses_any_unbound_or_different_value(lane, change):
    value = p.read_json(lane.inputs)
    assert p.checked_macos_inputs(copy.deepcopy(value)) == value
    if change == "state":
        value["library"]["state"] = "pending"
    elif change == "pins":
        value["library"]["pins"]["sha256"] = "0" * 64
    elif change == "policy":
        value["node_prebuilt_policy"]["sha256"] = None
    elif change == "export":
        value["source"]["export_commit"] = "main"
    elif change == "extra":
        value["library_url"] = "https://example.invalid/library.tar.gz"
    else:
        value["rust_toolchain"] = "stable"
    with pytest.raises(ValueError):
        p.checked_macos_inputs(value)


def test_linux_candidate_inputs_are_unchanged():
    assert p.digest(CONNECT / "release/connect-release-inputs.json") == \
        "fd761d77aebbda5c0659d1dbcc619ecf6e53f01e9038fff7780e142cbbdca518"
    assert p.input_path("linux-x86_64").name == "connect-release-inputs.json"


def test_darwin_node_prebuilt_policy_binds_the_pinned_node():
    policy = p.read_json(CONNECT / "release/node-prebuilt-policy-macos-arm64.json")
    assert set(policy) == {"schema_version", "scope", "target", "allowed_origins", "allowed_redirect_origins"}
    assert policy["schema_version"] == 1 and policy["scope"] == "trusted_public_node_prebuilt_resources"
    assert policy["target"] == {"platform": "darwin", "arch": "arm64", "libc": None, "node_version": "v24.19.0",
        "node_abi": "137", "node_sha256": "27db838bb204ef7c21df2931f5656e4c8fb32e6e947f363a402b49714d32b5b1"}
    assert policy["allowed_origins"] == ["https://github.com"]
    assert policy["allowed_redirect_origins"] == ["https://release-assets.githubusercontent.com"]
    selected = p.read_json(CONNECT / "release/connect-release-inputs-macos-arm64.json")["node_prebuilt_policy"]
    assert selected == {"path": "release/node-prebuilt-policy-macos-arm64.json",
                        "sha256": p.digest(CONNECT / "release/node-prebuilt-policy-macos-arm64.json")}


def test_macho_facts_are_read_without_any_tool(tmp_path):
    path = tmp_path / "inert"
    path.write_bytes(macho(minimum=(11, 0), libraries=("/usr/lib/libSystem.B.dylib",
        "/System/Library/Frameworks/Security.framework/Versions/A/Security"), payload=b"x" * 4096))
    facts = p.checked_macos_executable(path, "13.5")
    assert facts["minimum_os"] == "11.0" and facts["platform"] == 1 and len(facts["libraries"]) == 2
    assert facts["code_signature"] == {"flags": LINKER_SIGNED, "adhoc": True, "linker_signed": True}
    path.write_bytes(resign(path.read_bytes(), ADHOC))
    assert p.macho_facts(path)["code_signature"] == {"flags": ADHOC, "adhoc": True, "linker_signed": False}


@pytest.mark.parametrize("change,message", (
    ({"cpu": 0x01000007}, "thin arm64"), ({"filetype": 6}, "thin arm64"), ({"magic": 0xCAFEBABE}, "thin 64-bit"),
    ({"libraries": ("/usr/lib/libSystem.B.dylib", "/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib")}, "does not have"),
    ({"minimum": (14, 0)}, "above the selected")))
def test_macos_executable_facts_refuse_what_a_users_mac_cannot_run(tmp_path, change, message):
    path = tmp_path / "inert"
    path.write_bytes(macho(**change))
    with pytest.raises(ValueError, match=message):
        p.checked_macos_executable(path, "13.5")


def test_build_signs_each_executable_before_any_record_binds_its_digest(lane):
    lane.through("build")
    names = lane.names()
    for name in p.MACOS_SHIPPED:
        binary = lane.work / "binary" / name
        signature = p.read_json(binary.with_name(name + ".signature.json"))
        # Signing changed the bytes; every record names the signed ones.
        assert signature["unsigned"]["sha256"] != signature["signed"]["sha256"] == p.digest(binary)
        assert signature["identity"] == "-" and signature["code_directory"]["linker_signed"] is False
        assert names.index("native-build") < names.index("sign-" + name) < names.index("sign-verify-" + name) \
            < names.index("version-" + name)
        assert p.read_json(binary.with_name(name + ".facts.json"))["binary"]["sha256"] == p.digest(binary)
    record = p.read_json(lane.work / "records/gateway-build-record.json")
    gateway = lane.work / "binary/toolkit_gateway"
    assert record["binary"] == p.binding(gateway) and record["signature"] == p.binding(p.signature_path(gateway))
    assert record["binary"]["sha256"] != p.digest(lane.target / "toolkit_gateway")
    assert record["cargo"]["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] == RESOURCES == record["resources_sha256"]
    argv = record["cargo"]["argv"]
    assert argv[1:4] == ["build", "--locked", "--release"] and "--offline" not in argv
    assert argv[argv.index("--manifest-path") + 1] == str(CONNECT.parent / "Cargo.toml")
    assert not any(item.startswith("+") for item in argv), "no toolchain override: rust-toolchain.toml selects it"
    assert [argv[index + 1] for index, item in enumerate(argv) if item == "--bin"] == ["toolkit_gateway", "meerkat-connect"]
    # The real owner's validator accepts this record for staging.
    helpers = f.PREPARE
    _, observed = f.ASSEMBLER.build_record(lane.work / "records/gateway-build-record.json", gateway,
        p.digest(gateway), helpers, target="macos-arm64", resources_sha=RESOURCES, binary_kind="toolkit_gateway",
        resource_record_sha=p.digest(lane.work / "resources/build.json"), stage_only=True)
    assert observed["candidate"]["binary"] == {"sha256": p.digest(gateway), "size": gateway.stat().st_size}


@pytest.mark.parametrize("fault", ("unsigned", "linker_signed_only", "changed_after_signing", "wrong_version"))
def test_build_refuses_an_executable_that_is_not_the_signed_bytes(tmp_path, monkeypatch, fault):
    lane = MacosLane(tmp_path, monkeypatch)
    lane.through("resources")
    if fault == "linker_signed_only":
        lane.sign = LINKER_SIGNED  # codesign left the linker's own signature in place
    elif fault == "unsigned":
        monkeypatch.setattr(p, "macos_sign", lambda record, path, label: None)
    elif fault == "changed_after_signing":
        binary = lane.work / "binary/toolkit_gateway"
        lane.after["version-toolkit_gateway"] = lambda: binary.write_bytes(binary.read_bytes() + b"changed")
    elif fault == "wrong_version":
        lane.versions = {**lane.versions, "connect": "0.0.1"}
    with pytest.raises(ValueError):
        lane.step("build")
    assert not (lane.work / "records/gateway-build-record.json").exists()
    assert not p.step_path(lane.work, "build").exists()


def test_library_step_builds_signs_and_records_six_tools_then_calls_the_owners(lane):
    lane.through("library")
    work = lane.work
    for name, feature in p.MACOS_TOOLS:
        record = p.read_json(work / "tools" / (name + ".build.json"))
        tool = work / "tools" / name
        assert record["binary"] == "tools/" + name and record["mode"] == "0755"
        assert record["sha256"] == p.digest(tool) and record["bytes"] == tool.stat().st_size
        assert record["target"] == "aarch64-apple-darwin" and record["features"] == ([feature] if feature else [])
        command = record["command"]
        assert command.count("--bin") == 1 and command[command.index("--bin") + 1] == name
        assert "--locked" in command and "--release" in command
        assert p.read_json(p.signature_path(tool))["signed"]["sha256"] == record["sha256"]
        assert tool.stat().st_mode & 0o777 == 0o755
    assert dict(p.MACOS_TOOLS)["toolkit-package-inspect"] == "local-artifacts"
    assert (work / "tools/dependencies.txt").read_text() == (
        "serde v1.0.228\nsyn v2.0.117 (proc-macro)\ntoolkit-package v" + lane.versions["connect"]
        + " (/inert/connect/crates/toolkit-package)\n")
    owners = {call[0]: call for call in lane.calls if call[0].startswith("library-")}
    assert list(owners) == ["library-components", "library-consumers", "library-kit", "library-package"]
    scripts, pinned = str(CONNECT / "scripts"), ["--pins", str(lane.pins), "--pinned-inputs", str(lane.pinned)]
    python = [p.sys.executable, "-I", "-B"]
    components = work / "components"
    agno, services = components / "meerkat_toolkit_agno-0.1.5-py3-none-any.whl", \
        components / "meerkat_toolkit_services-0.1.23-py3-none-any.whl"
    pieces = components / "rkat-toolkit-activepieces-0.1.0.tgz"
    closure = str(work / "consumers-closure/dependency-closure-macos-arm64.json")
    edition = "20260921-connect-aaaaaaaa"
    assert p.read_json(p.step_path(work, "preflight"))["library_edition"] == edition
    node = work / "node/node-v24.19.0-darwin-arm64"
    assert owners["library-components"][1] == [*python, scripts + "/build-library-components.py", *pinned,
        "--python", p.sys.executable, "--node", str(node / "bin/node"),
        "--npm-cli", str(node / "lib/node_modules/npm/bin/npm-cli.js"), "--source-date-epoch", "1790000000",
        "--output", str(components), "--verify-reproducible"]
    recorded = p.read_json(p.step_path(work, "library"))["outputs"]
    assert recorded["components_reproducible"]["activepieces"] == {"equal": False}, "measured and stated, not assumed"
    assert owners["library-consumers"][1] == [*python, scripts + "/export-library-consumers.py",
        "--target", "darwin-arm64-cpython311", *pinned,
        "--agno-wheel", str(agno), "--agno-sha256", p.digest(agno),
        "--services-wheel", str(services), "--services-sha256", p.digest(services),
        "--activepieces-archive", str(pieces), "--activepieces-sha256", p.digest(pieces),
        "--closure-record", closure, "--output", str(work / "consumers")]
    assert owners["library-kit"][1] == [*python, scripts + "/package-agent-install-kit.py", "--target", "macos-arm64",
        *pinned, "--tools", str(work / "tools"), "--tool-dependencies", str(work / "tools/dependencies.txt"),
        "--rust-license", "/inert/toolchain/share/doc/rust/COPYRIGHT-library.html", "--output", str(work / "kit")]
    kit = work / "kit/agent-install-kit-macos-arm64.tar.gz"
    assert owners["library-package"][1] == [*python, scripts + "/package-library-release.py", "--target", "macos-arm64",
        "--edition", edition, *pinned, "--consumers", str(work / "consumers"),
        "--consumers-contents-sha256", p.digest(work / "consumers/CONTENTS.json"), "--consumer-closure", closure,
        "--kit", str(kit), "--kit-sha256", p.digest(kit), "--output", str(work / "library")]
    for call in owners.values():
        assert call[3] is False and set(call[2]) == {"HOME", "TMPDIR", "PATH", "LC_ALL", "PIP_CONFIG_FILE",
            "PIP_DISABLE_PIP_VERSION_CHECK", "PIP_NO_CACHE_DIR", "PYTHONDONTWRITEBYTECODE"}
        assert "/home/" not in call[2]["HOME"].replace(str(lane.root), "")


def test_the_tool_dependency_listing_is_the_fetch_steps_closure_command_and_forces_plain_text(lane):
    # The listing is read line by line, and a hosted runner's Rust setup asks Cargo for colour
    # (CARGO_TERM_COLOR=always), which colours the repeat mark even into a file. The command line itself
    # says `--color never`; the environment says it too, for Cargo's status lines in the same log.
    lane.through("library")
    fetch = load("connect_fetch_pinned_inputs_for_the_producer", CONNECT / "scripts/fetch-pinned-inputs.py")
    call = next(call for call in lane.calls if call[0] == "tool-dependencies")
    assert call[1] == [str(CONNECT / "scripts/repo-cargo"), "tree", "--locked", "--manifest-path",
                       str(CONNECT.parent / "Cargo.toml"), *fetch.TOOL_CLOSURE_ARGUMENTS]
    assert call[1][-2:] == ["--color", "never"]
    assert call[3] is True and call[2] == {"RUST_LANE_ID": "connect-macos-arm64", "CARGO_TERM_COLOR": "never"}


@pytest.mark.parametrize("line", (
    "syn v2.0.117 \x1b[33m\x1b[2m(*)\x1b[39m\x1b[22m",       # a coloured repeat mark, as Cargo prints it
    "\x1b[1m\x1b[32m    Updating\x1b[0m crates.io index",  # a coloured status line
    "serde v1.0.228\x1b[0m"))
def test_a_tool_dependency_listing_with_a_terminal_escape_sequence_is_refused(lane, line):
    # Such a line is not a package line, and it is not skipped as a status line either: a listing that
    # carries terminal control text is not the plain listing the command asks for.
    lane.through("preflight")
    original = lane.effect

    def effect(name, argv, cwd, env):
        text = original(name, argv, cwd, env)
        return text + line + "\n" if name == "tool-dependencies" else text

    lane.effect = effect
    with pytest.raises(ValueError, match="escape"):
        lane.step("library")
    assert not (lane.work / "tools/dependencies.txt").exists()
    assert not p.step_path(lane.work, "library").exists()


def test_resources_step_calls_the_resource_packager_with_the_pinned_pair_only(lane):
    lane.through("resources")
    work = lane.work
    library = work / "library/toolkit-library-20260921-connect-aaaaaaaa-macos-arm64.tar.gz"
    selection = p.read_json(work / "library-selection.json")
    assert selection == {"schema_version": 1, "status": "candidate_input_not_published", "assets": [
        {"upload_name": library.name, "bytes": library.stat().st_size, "sha256": p.digest(library)}]}
    call = next(call for call in lane.calls if call[0] == "resources")
    policy = CONNECT / "release/node-prebuilt-policy-macos-arm64.json"
    assert call[1] == [p.sys.executable, "-I", "-B", str(CONNECT / "scripts/package-connect-resources.py"),
        "--destination", str(work / "resources"), "--zstd", "/opt/tools/bin/zstd", "--target", "macos-arm64",
        "--pins", str(lane.pins), "--pinned-inputs", str(lane.pinned),
        "--library", str(library), "--library-sha256", p.digest(library), "--library-bytes", str(library.stat().st_size),
        "--starter", str(work / "starter"),
        "--release-manifest-sha256", p.digest(work / "starter/release-manifest.json"),
        "--node-prebuilt-policy", str(policy), "--node-prebuilt-policy-sha256", p.digest(policy)]
    assert not {"--upstream", "--acquisition-node-archive", "--npm-archive"} & set(call[1])
    assert p.read_json(p.step_path(work, "resources"))["outputs"]["resources_sha256"] == RESOURCES


def test_stage_builds_both_test_executables_once_and_keeps_the_production_bytes(lane):
    lane.through("stage")
    work, names = lane.work, lane.names()
    assert names.count("test-build") == 1
    argv = next(call[1] for call in lane.calls if call[0] == "test-build")
    assert argv[1] == "test" and argv[-1] == "--message-format=json-render-diagnostics" and "--no-run" in argv
    assert [argv[index + 1] for index, item in enumerate(argv) if item == "--test"] == list(p.MACOS_TESTS)
    for name in p.MACOS_SHIPPED:
        assert p.signed_binding(work / "binary" / name)["sha256"] != p.digest(lane.target / name)
    cli = p.read_json(work / "records/cli-build-record.json")
    tested = work / "binary/packaged_setup"
    assert cli["binary"] == p.binding(work / "binary/meerkat-connect") and cli["test_executable"] == p.binding(tested)
    assert cli["cargo"] == p.read_json(work / "records/native-build.json")
    assert p.read_json(p.signature_path(tested))["signed"] == p.binding(tested)
    _, observed = f.ASSEMBLER.build_record(work / "records/cli-build-record.json", work / "binary/meerkat-connect",
        p.digest(work / "binary/meerkat-connect"), f.PREPARE, target="macos-arm64", resources_sha=RESOURCES,
        binary_kind="meerkat-connect", resource_record_sha=p.digest(work / "resources/build.json"), stage_only=True)
    assert observed["candidate"]["test_executable"] == {"sha256": p.digest(tested), "size": tested.stat().st_size}
    installed = work / "cli-stage/meerkat-connect-macos-arm64/bin/packaged_setup"
    assert installed.read_bytes() == tested.read_bytes() and installed.stat().st_mode & 0o777 == 0o755
    wheels = p.read_json(work / "sdk/SDK-WHEELS.json")
    assert {label: wheels[label]["source"] for label in ("meta", "toolkit")} == {
        "meta": "built_from_checkout", "toolkit": "built_from_checkout"}
    for name in ("gateway-stage", "cli-stage"):
        call = next(call for call in lane.calls if call[0] == name)
        assert "--release-version" in call[1] and call[3] is False


def test_stage_takes_a_supplied_published_meta_wheel_by_digest_only(lane):
    lane.through("build")
    supplied = lane.wheel(lane.root, "meerkat-meta", lane.versions["meta_sdk"])
    with pytest.raises(ValueError, match="stated digest"):
        lane.step("stage", meta_wheel=supplied, meta_wheel_sha256="0" * 64)


def test_supplied_meta_wheel_is_recorded_as_supplied(lane):
    lane.through("build")
    supplied = lane.wheel(lane.root, "meerkat-meta", lane.versions["meta_sdk"])
    lane.step("stage", meta_wheel=supplied, meta_wheel_sha256=p.digest(supplied))
    wheels = p.read_json(lane.work / "sdk/SDK-WHEELS.json")
    assert wheels["meta"]["source"] == "supplied" and wheels["meta"]["sha256"] == p.digest(supplied)
    assert "sdk-wheel-meta" not in lane.names() and "sdk-wheel-toolkit" in lane.names()


def test_installed_checks_run_products_with_a_complete_environment_only(lane):
    lane.through(lane.LAST)
    work = lane.work
    products = [call for call in lane.calls if call[0] in (p.CLI_COMPOSITION, p.CLI_SHUTDOWN, p.CLI_CONSUMER,
                "gateway-verify", "conversation", "version-meerkat-connect", "version-toolkit_gateway")]
    assert len(products) == 7
    for name, argv, env, inherit in products:
        assert inherit is False and "HOME" in env and "TMPDIR" in env
        assert not [key for key in env if key.endswith(("_API_KEY", "_TOKEN", "_SECRET"))]
        assert all(str(work) in env[key] for key in ("HOME", "TMPDIR"))
        assert "/home/" not in env["HOME"].replace(str(lane.root), ""), "no directory named home in the work root"
    for name in (p.CLI_COMPOSITION, p.CLI_SHUTDOWN):
        env = next(call[2] for call in lane.calls if call[0] == name)
        assert env["PATH"].endswith("/empty-path") and list(Path(env["PATH"]).iterdir()) == []
    consumer = next(call for call in lane.calls if call[0] == p.CLI_CONSUMER)
    assert consumer[2]["CONNECT_REAL_LIBRARY_PREFIX"] == str(work / "cli-stage/meerkat-connect-macos-arm64")
    assert consumer[2]["CONNECT_SUPPLIED_CLI_SHA256"] == p.digest(work / "binary/meerkat-connect")
    assert consumer[2]["CONNECT_FINAL_RKAT_SHA256"] == p.digest(work / "rkat/rkat")
    pending = p.read_json(work / "cli-qualification/native-shutdown-review.pending.json")
    assert pending["disposition"] == "pending" and pending["scope"] == "active_setup_worker_shutdown"
    composition = p.read_json(work / "cli-qualification/installed-composition.json")
    assert composition["status"] == "passed" and composition["candidate"] == pending["candidate"]
    assert p.read_json(work / "cli-qualification/consumer.json")["rkat"]["source"] == "published_archive"
    conversation = p.read_json(work / "cli-qualification/conversation-start.json")
    assert conversation["status"] == "passed" and conversation["closed_by"] == "terminal_interrupt"
    assert conversation["argv"][0] == str(work / "cli-stage/meerkat-connect-macos-arm64/bin/meerkat-connect")
    # Cargo keeps the job's toolchain environment and gets the resource digest and the lane name.
    for name in ("native-build", "test-build"):
        call = next(call for call in lane.calls if call[0] == name)
        assert call[3] is True and call[2] == {"RUST_LANE_ID": "connect-macos-arm64", "CARGO_TERM_COLOR": "never",
                                                "MEERKAT_CONNECT_RESOURCES_SHA256": RESOURCES}


@pytest.mark.parametrize("outcome", ("zero_tests", "no_trace", "gateway_failed"))
def test_a_check_that_did_not_really_pass_leaves_no_passing_record(lane, outcome):
    lane.through("stage")
    original = lane.effect

    def effect(name, argv, cwd, env):
        text = original(name, argv, cwd, env)
        if outcome == "zero_tests" and name == p.CLI_COMPOSITION:
            return "test result: ok. 0 passed; 0 failed; 0 ignored; 10 filtered out;\n"
        if outcome == "no_trace" and name == p.CLI_SHUTDOWN:
            next(Path(env["TMPDIR"]).glob("connect-active-worker-acceptance-*/native-dispatch.jsonl")).unlink()
        return text

    lane.effect = effect
    lane.gateway_status = "failed" if outcome == "gateway_failed" else "passed"
    with pytest.raises(ValueError):
        lane.step("check-gateway" if outcome == "gateway_failed" else "check-cli")
    assert not (lane.work / "cli-qualification/native-shutdown-review.pending.json").exists()
    assert not p.step_path(lane.work, "check-gateway" if outcome == "gateway_failed" else "check-cli").exists()


@pytest.mark.parametrize("transcript,alive,quiet,accepted", (
    (MacosLane.transcript, True, True, True),
    (MacosLane.transcript, False, True, False),
    (MacosLane.transcript, True, False, False),
    ("Realm: x (/inert)\r\n" + p.CONVERSATION_PREPARING + " You will review its access.\r\n", True, True, False),
    ("Realm: x (/inert)\r\n" + p.CONVERSATION_PREPARING + " ...\r\nError: " + p.CONVERSATION_UNAVAILABLE
     + "; reinstall the complete product. No setup command was started.\r\n", False, True, False),
    ("meerkat-connect requires an interactive terminal.\r\n", False, True, False)))
def test_conversation_start_means_past_the_resource_step_and_waiting(transcript, alive, quiet, accepted):
    if accepted:
        assert p.conversation_verdict(transcript, alive=alive, quiet=quiet).endswith("Continue?")
    else:
        with pytest.raises(ValueError):
            p.conversation_verdict(transcript, alive=alive, quiet=quiet)


def test_conversation_markers_are_the_strings_the_cli_prints():
    crate = CONNECT / "crates/meerkat-connect/src"
    main = (crate / "main.rs").read_text()
    assert '"Realm: {} ({})"' in main and p.CONVERSATION_REALM == "Realm: "
    assert '"' + p.CONVERSATION_PREPARING + " " in main
    assert main.index(p.CONVERSATION_PREPARING) < main.index("InstalledResources::locate()?")
    assert '"' + p.CONVERSATION_UNAVAILABLE + ";" in (crate / "resources.rs").read_text()


def test_conversation_runs_on_a_real_terminal_and_closes_the_program(tmp_path):
    # A stand-in program (this interpreter), not the product: it prints the two lines, a prompt, and waits.
    program = tmp_path / "inert_cli.py"
    program.write_text("import sys, time\nassert sys.stdin.isatty() and sys.stdout.isatty()\n"
        "print('Realm: inert (/inert)')\nprint(" + repr(p.CONVERSATION_PREPARING + " inert") + ")\n"
        "print('Review access'); sys.stdout.write('Continue? '); sys.stdout.flush()\ntime.sleep(600)\n")
    observed = p.macos_conversation([p.sys.executable, "-I", str(program)], tmp_path, {"PATH": "/usr/bin:/bin"},
                                    seconds=60, quiet_seconds=1, close_seconds=10)
    assert observed["alive_at_prompt"] and observed["quiet_at_prompt"] and not observed["timed_out"]
    assert observed["closed_by"] in ("terminal_interrupt", "sigint") and observed["exit_code"] != 0
    assert p.conversation_verdict(observed["transcript"], alive=True, quiet=True).endswith("Continue?")


def test_stage_record_holds_the_gateway_product_and_a_pending_review_never_the_cli_product(lane):
    lane.through(lane.LAST)
    # A check may leave a socket or a pipe behind: it holds no bytes and is listed, not archived.
    os.mkfifo(lane.work / "gateway-verification/control.pipe")
    stage = lane.stage_record()
    directory = lane.root / "stage"
    assert stage["special_files_left_out"] == ["gateway-verification/control.pipe"]
    assert p.checked_stage(directory) == stage == p.read_json(directory / "STAGE.json")
    assert stage["gates"]["shutdown_review"] == {"status": "pending"}
    assert stage["gates"]["consumer"] == {"status": "passed", "rkat": "published_archive"}
    assert {name: gate["status"] for name, gate in stage["gates"].items() if name != "shutdown_review"} == {
        "gateway_verification": "passed", "cli_composition": "passed", "consumer": "passed", "conversation_start": "passed",
        "official_playwright_install": "passed"}
    assert stage["revision"] == REVISION and stage["work_root"] == str(lane.work)
    assert stage["checkout_root"] == str(CONNECT.parent) and stage["resources_sha256"] == RESOURCES
    # The fetch binding travels with the records: committed pins, resolved pin file, Cargo.lock.
    assert stage["pins_sha256"] == lane.selection["library"]["pins"]["sha256"]
    assert stage["resolved_pins_sha256"] == p.digest(lane.pins) != stage["pins_sha256"]
    assert stage["lock_sha256"] == p.digest(CONNECT.parent / "Cargo.lock")
    paths = [row["path"] for row in stage["files"]]
    assert paths == sorted(paths) and len(paths) == len(set(paths))
    version = lane.versions["connect"]
    # The gateway product is complete in stage one: the exact archive is there to be scanned.
    gateway = f"products/gateway/meerkat-connect-gateway-{version}-macos-arm64.tar.gz"
    assert gateway in paths and gateway + ".sha256" in paths and "products/gateway/assembly.json" in paths
    product = stage["products"]["gateway"]
    assert product["state"] == "assembled_not_scanned_not_published"
    assert product["archive"]["sha256"] == p.digest(directory / gateway)
    assert lane.names().index("gateway-verify") < lane.names().index("gateway-final")
    # The CLI product is not: its assembly needs the accepted review, which stage one never has.
    assert stage["products"]["cli"] == {"state": "awaits_shutdown_review"}
    assert "cli-final" not in lane.names()
    assert not [path for path in paths if path.endswith(f"meerkat-connect-{version}-macos-arm64.tar.gz")]
    for needed in ("inputs-selection.json", "source-manifest.json", "library-selection.json", "library/build.json",
                   "library/toolkit-library-20260921-connect-aaaaaaaa-macos-arm64.tar.gz", "resources/build.json",
                   "resources/meerkat-connect-resources-macos-arm64.tar.gz", "binary/meerkat-connect",
                   "binary/toolkit_gateway", "binary/packaged_setup", "binary/cli_paired_host",
                   "binary/meerkat-connect.signature.json", "binary/toolkit_gateway.facts.json",
                   "tools/toolkit-node-plan.build.json", "tools/dependencies.txt", "components/components.json",
                   "records/native-build.json", "records/native-build.log", "records/pinned-inputs/FETCHED.json",
                   "records/pinned-inputs/library-pins-macos-arm64.resolved.json",
                   "records/gateway-build-record.json", "records/cli-build-record.json", "sdk/SDK-WHEELS.json",
                   "cli-qualification/installed-composition.json", "cli-qualification/consumer.json",
                   "cli-qualification/native-shutdown-review.pending.json",
                   "cli-qualification/conversation-start.json", "cli-qualification/conversation-run/transcript.txt",
                   "cli-qualification/shutdown-run/tmp/connect-active-worker-acceptance-inert/native-dispatch.jsonl",
                   "gateway-stage.tar", "cli-stage.tar", "gateway-verification.tar"):
        assert needed in paths, needed
    assert not [path for path in paths if path.startswith(("pins/", "tools/toolkit-", "node/"))
                and not path.endswith(".json")]
    # The network section says what was read from the records and that nothing was captured.
    network = stage["network"]
    assert set(network) == {"library", "check-gateway", "check-cli", "check-consumer", "check-conversation",
                            "check-official-install"}
    assert "--offline" in network["check-official-install"]["expected"]
    assert all(entry["captured"] is False for entry in network.values())
    assert network["library"]["hosts_named_in_records"] == ["registry.npmjs.org"]
    assert "registry.npmjs.org" in network["library"]["expected"]
    assert network["check-gateway"]["hosts_named_in_records"] == ["news.example.org"]
    assert network["check-consumer"]["hosts_named_in_records"] == ["news.example.org"]
    assert network["check-cli"]["hosts_named_in_records"] == []
    assert network["check-conversation"]["hosts_named_in_records"] == []
    with pytest.raises(ValueError, match="already ran"):
        lane.stage_record()


def test_stage_record_states_a_consumer_check_that_did_not_run(lane):
    lane.through(lane.LAST, skip=("check-consumer",))
    with pytest.raises(ValueError, match="consumer check either ran"):
        lane.stage_record()
    stage = lane.stage_record(consumer_not_run="Core's archive of this version is not published")
    assert stage["gates"]["consumer"] == {"status": "not_run",
                                          "reason": "Core's archive of this version is not published"}
    with pytest.raises(ValueError, match="stage gates did not pass: consumer"):
        lane.assemble()
    assert not (lane.root / "final").exists()


def test_consumer_check_needs_exactly_one_rkat_and_records_which(lane):
    lane.through("check-cli")
    with pytest.raises(ValueError, match="exactly one of"):
        lane.step("check-consumer", archive=None, archive_sha256=None, from_checkout=False)


def test_a_checkout_built_rkat_proves_the_check_but_never_assembles_a_release(lane):
    lane.through("check-cli")
    lane.step("check-consumer", from_checkout=True)
    lane.step("check-conversation")
    lane.step("check-official-install")
    assert "rkat-build" in lane.names()
    assert p.read_json(lane.work / "cli-qualification/consumer.json")["rkat"]["source"] == "built_from_checkout"
    assert lane.stage_record()["gates"]["consumer"] == {"status": "passed", "rkat": "built_from_checkout"}
    with pytest.raises(ValueError, match="published rkat"):
        lane.assemble()


def test_assemble_restores_the_stage_and_runs_only_the_cli_assembly(lane):
    lane.through(lane.LAST)
    stage = lane.stage_record()
    work = lane.work
    review = lane.accepted_review()
    before = len(lane.calls)
    # A fresh stage-two host: nothing but the stage record is there, and a downloaded workflow artifact
    # keeps no file mode: every plain file of the stage record is 0644 (the trees travel as tar members).
    shutil.rmtree(work)
    plain = [path for path in (lane.root / "stage").rglob("*") if path.is_file()]
    for path in plain:
        path.chmod(0o644)
    assert len(plain) > 40 and not [path for path in plain if os.access(path, os.X_OK)]
    value = lane.assemble(review=review)
    for name in (*p.MACOS_SHIPPED, *p.MACOS_TESTS):
        assert (work / "binary" / name).stat().st_mode & 0o777 == 0o755
    assert (work / "cli-stage/meerkat-connect-macos-arm64/bin/packaged_setup").stat().st_mode & 0o777 == 0o755
    assert (work / "gateway-stage/meerkat-connect-macos-arm64/bin/toolkit_gateway").is_file()
    assert [call[0] for call in lane.calls[before:]] == ["cli-final"]
    assert value["native_execution"] is False and value["publication"] == "not_performed"
    assert value["private_text_scan"] == "required_before_publication"
    version = lane.versions["connect"]
    final = lane.root / "final"
    assert value["products"]["gateway"]["archive"]["name"] == f"meerkat-connect-gateway-{version}-macos-arm64.tar.gz"
    assert value["products"]["gateway"]["archive"] == stage["products"]["gateway"]["archive"], "the stage's own bytes"
    assert value["products"]["cli"]["archive"]["name"] == f"meerkat-connect-{version}-macos-arm64.tar.gz"
    assert value == p.read_json(final / "ASSEMBLY.json")
    assert sorted(path.name for path in (final / "gateway").iterdir()) == sorted(
        [f"meerkat-connect-gateway-{version}-macos-arm64.tar.gz",
         f"meerkat-connect-gateway-{version}-macos-arm64.tar.gz.sha256", "assembly.json"])
    assert (work / "gateway-stage/meerkat-connect-macos-arm64/bin/linked").is_symlink()
    assert (work / "binary/meerkat-connect").stat().st_mode & 0o777 == 0o755
    qualified = p.read_json(final / "qualified-build-record.json")
    assert qualified["native_shutdown_review"]["sha256"] == value["shutdown_review_sha256"]
    assert qualified["installed_composition"] == p.binding(work / "cli-qualification/installed-composition.json")
    cli = next(call[1] for call in lane.calls if call[0] == "cli-final")
    assert cli[cli.index("--build-record") + 1] == str(final / "qualified-build-record.json")
    gateway = next(call[1] for call in lane.calls if call[0] == "gateway-final")
    assert gateway[gateway.index("--gateway-verification") + 1] == str(work / "gateway-verification/manifest.json")


@pytest.mark.parametrize("fault", (None, "refused", "cli_not_scanned", "gateway_not_scanned", "digest",
                                   "changed_archive", "twice"))
def test_nothing_is_publishable_without_a_clean_scan_of_both_exact_archives(lane, fault):
    lane.through(lane.LAST)
    lane.stage_record()
    lane.assemble()
    final = lane.root / "final"
    report = lane.privacy_report(status="refused" if fault == "refused" else "clean", leave_out={
        "cli_not_scanned": "meerkat-connect-" + lane.versions["connect"],
        "gateway_not_scanned": "meerkat-connect-gateway-"}.get(fault))
    stated = "0" * 64 if fault == "digest" else p.digest(report)
    if fault == "changed_archive":
        next((final / "cli").glob("*.tar.gz")).write_bytes(b"other bytes")
    if fault in (None, "twice"):
        value = p.macos_release_check(final, report, stated)
        assert value == p.read_json(final / "PUBLISHABLE.json") and value["publication"] == "not_performed"
        assert sorted(row["name"] for row in value["assets"]) == sorted(
            path.name for path in [*final.glob("cli/*.tar.gz*"), *final.glob("gateway/*.tar.gz*")])
        assert value["privacy_report_sha256"] == p.digest(report)
        if fault == "twice":
            with pytest.raises(ValueError, match="already checked"):
                p.macos_release_check(final, report, stated)
    else:
        with pytest.raises(ValueError):
            p.macos_release_check(final, report, stated)
        assert not (final / "PUBLISHABLE.json").exists()
    assert p.macos_main(["release-check", "--assembly", str(final), "--privacy-report", str(report),
                         "--privacy-report-sha256", stated]) == 1, "a second or refused check never passes"


@pytest.mark.parametrize("fault", ("pending_review", "review_digest", "review_candidate", "review_evidence",
    "other_work_root", "changed_file", "changed_executable", "changed_gateway_product", "pending_selection",
    "cli_product_in_stage"))
def test_assemble_refuses_before_any_assembly(lane, fault):
    lane.through(lane.LAST)
    lane.stage_record()
    stage_path = lane.root / "stage/STAGE.json"
    changes = {}
    review = lane.accepted_review()
    accepted = p.read_json(review)
    if fault == "pending_review":
        accepted["disposition"] = "pending"
    elif fault == "review_candidate":
        accepted["candidate"]["resources_sha256"] = "1" * 64
    elif fault == "review_evidence":
        accepted["evidence"] = accepted["evidence"][1:]
    if fault.startswith(("pending_review", "review_")):
        other = lane.root / "other-review.json"
        dump(other, accepted)
        changes = {"review": other, "review_sha256": "0" * 64 if fault == "review_digest" else p.digest(other)}
    elif fault == "other_work_root":
        changes = {"work": lane.root / "elsewhere"}
    elif fault == "changed_file":
        (lane.root / "stage/resources/build.json").write_text("{}\n")
    elif fault == "changed_executable":
        (lane.work / "binary/meerkat-connect").write_bytes(b"other bytes")
    elif fault == "changed_gateway_product":
        next((lane.work / "products/gateway").glob("*.tar.gz")).write_bytes(b"other bytes")
    elif fault == "pending_selection":
        value = p.read_json(lane.inputs)
        value["library"]["state"] = "pending"
        dump(lane.inputs, value)
    elif fault == "cli_product_in_stage":
        stage = p.read_json(stage_path)
        stage["products"]["cli"] = {"state": "assembled_not_scanned_not_published"}
        dump(stage_path, stage)
    before = len(lane.calls)
    with pytest.raises(ValueError):
        lane.assemble(**changes)
    assert len(lane.calls) == before
    assert not (lane.root / "final/ASSEMBLY.json").exists() and not (lane.root / "final/cli").exists()


def test_steps_run_once_in_order_and_only_on_the_recorded_checkout(lane, monkeypatch):
    with pytest.raises((ValueError, FileNotFoundError)):
        lane.step("build")
    lane.step("preflight")
    with pytest.raises(ValueError, match="already ran"):
        lane.step("preflight")
    with pytest.raises(ValueError, match="earlier step has no record: library"):
        lane.step("resources")
    lane.step("library")
    with pytest.raises(ValueError, match="already ran"):
        lane.step("library")
    monkeypatch.setattr(p, "macos_checkout", lambda: ("d" * 40, TREE))
    with pytest.raises(ValueError, match="not the one preflight recorded"):
        lane.step("resources")


def test_preflight_refuses_another_commit_host_or_toolchain(lane, monkeypatch):
    with pytest.raises(ValueError, match="not the selected commit"):
        p.macos_preflight(lane.work, "e" * 40)
    lane.selection["rust_toolchain"] = "1.96.0"
    with pytest.raises(ValueError):
        lane.step("preflight")
    monkeypatch.undo()
    if (p.platform.system(), p.platform.machine()) != p.TARGETS["macos-arm64"]:
        assert p.macos_main(["preflight", "--work", str(lane.root / "host"), "--source-sha", REVISION]) == 1
        assert p.macos_main(["build", "--work", str(lane.root / "host")]) == 1


def test_macos_is_a_word_of_its_own_and_the_other_commands_keep_their_parser(monkeypatch, capsys):
    seen = []
    monkeypatch.setattr(p, "macos_main", lambda arguments: seen.append(arguments) or 7)
    monkeypatch.setattr(p.sys, "argv", ["connect-candidate.py", "macos", "build", "--work", "/inert"])
    assert p.main() == 7 and seen == [["build", "--work", "/inert"]]
    monkeypatch.setattr(p.sys, "argv", ["connect-candidate.py", "macos-build"])
    with pytest.raises(SystemExit):
        p.main()
    assert "invalid choice" in capsys.readouterr().err and len(seen) == 1


def test_recorder_runs_a_child_with_exactly_the_given_environment(tmp_path, monkeypatch):
    monkeypatch.setenv("CONNECT_INERT_SECRET_API_KEY", "must-not-reach-the-child")
    record = p.Recorder(tmp_path)
    selected, log = record.run("complete", [p.sys.executable, "-I", "-c",
        "import os; print(sorted(os.environ))"], cwd=tmp_path, env={"INERT_ONLY": "1"}, inherit=False)
    assert "CONNECT_INERT_SECRET_API_KEY" not in log.read_text() and "INERT_ONLY" in log.read_text()
    assert selected["env"] == {"INERT_ONLY": "1"}
    inherited, log = record.run("inherited", [p.sys.executable, "-I", "-c",
        "import os; print('CONNECT_INERT_SECRET_API_KEY' in os.environ)"], cwd=tmp_path)
    assert log.read_text().strip() == "True" and "CONNECT_INERT_SECRET_API_KEY" not in inherited["env"]


@pytest.mark.parametrize("fault", ("committed_pins", "resolved_file", "lock", "no_record", "another_fetch_later"))
def test_pinned_inputs_are_bound_to_the_committed_pins_the_resolved_file_and_the_lock(lane, fault):
    lane.step("preflight")
    record = lane.pinned / "FETCHED.json"
    changed = copy.deepcopy(lane.fetched)
    if fault == "committed_pins":
        changed["pins_sha256"] = "0" * 64
    elif fault == "resolved_file":
        changed["resolved_pins"]["sha256"] = "0" * 64
    elif fault == "lock":
        changed["lock_sha256"] = "0" * 64
    if fault == "no_record":
        record.unlink()
    elif fault != "another_fetch_later":
        dump(record, changed)
    if fault == "another_fetch_later":
        lane.step("library")
        kept = lane.work / "records/pinned-inputs"
        assert (kept / "FETCHED.json").read_bytes() == record.read_bytes()
        assert (kept / lane.pins.name).read_bytes() == lane.pins.read_bytes()
        changed["files"] = [{"kind": "crate", "file": "inert-1.0.0.crate", "bytes": 1, "sha256": "1" * 64}]
        dump(record, changed)
        with pytest.raises(ValueError, match="another fetch"):
            lane.step("resources")
    else:
        with pytest.raises((ValueError, FileNotFoundError)):
            lane.step("library")
        assert "tool-toolkit-local-recipes" not in lane.names(), "refused before anything was built"


def test_official_playwright_install_is_the_backends_command_with_the_closures_node_and_npm(lane):
    lane.through(lane.LAST)
    work = lane.work
    closure = work / "cli-stage/meerkat-connect-macos-arm64/libexec/meerkat-connect"
    run = work / "cli-qualification/official-run"
    library = run / "library" / lane.LIBRARY
    call = next(call for call in lane.calls if call[0] == "official-playwright-install")
    profile, selected = run / "playwright-host.json", run / "selected.json"
    assert call[1] == [str(closure / "runtime/python/bin/python3.11"), "-I", "-B",
        str(library / "setup/install-library-consumer.py"), "--official-bundle", str(library / "official"),
        "--contents-sha256", p.digest(library / "official/CONTENTS.json"), "--component", "playwright",
        "--playwright-host-profile", str(profile), "--playwright-host-profile-sha256", p.digest(profile),
        "--node", str(closure / "runtime/node/bin/node"),
        "--npm-cli", str(closure / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"),
        "--destination", str(run / "install"), "--config", str(selected), "--config-sha256", p.digest(selected),
        "--server-name", "playwright-isolated"]
    assert call[3] is False and set(call[2]) == {"HOME", "TMPDIR", "PATH", "LC_ALL"}
    assert p.read_json(profile) == {"schema_version": 1, "chrome_executable": str(lane.chrome),
        "chrome_sha256": p.digest(lane.chrome), "process_parent": str(run / "process")}
    assert p.read_json(selected) == p.PLAYWRIGHT_CONFIGURATION
    record = p.read_json(work / "cli-qualification/official-playwright.json")
    assert record["status"] == "passed" and "a browser start" in record["not_claimed"]
    assert record["route"] == "official-playwright" and record["npm_version"] == "11.17.0"
    assert p.binding(run / "install/npm-runtime.stdout") in record["evidence"]
    # The profile, the operations and the server name are the installer's own fixed values.
    installer = (CONNECT / "scripts/install-library-consumer.py").read_text()
    assert 'PLAYWRIGHT_PROFILE = "' + p.PLAYWRIGHT_CONFIGURATION["profile"] + '"' in installer
    assert "PLAYWRIGHT_OPERATIONS = " + json.dumps(p.PLAYWRIGHT_CONFIGURATION["operations"]) in installer
    assert '"mcpServers": {"' + p.PLAYWRIGHT_SERVER + '"' in installer


@pytest.mark.parametrize("fault", ("install_failed", "no_receipt"))
def test_a_failed_official_install_fails_the_stage(lane, fault):
    lane.through("check-conversation")
    if fault == "no_receipt":
        lane.official_scope = "nothing_was_installed"
    else:
        original = lane.effect

        def effect(name, argv, cwd, env):
            if name == "official-playwright-install":
                raise ValueError("command did not succeed: official-playwright-install")
            return original(name, argv, cwd, env)

        lane.effect = effect
    with pytest.raises(ValueError, match="the official-playwright install route failed"):
        lane.step("check-official-install")
    assert not (lane.work / "cli-qualification/official-playwright.json").exists()
    # The failure names the route and keeps what the installer said about its npm.
    refused = p.read_json(lane.work / "cli-qualification/official-playwright.refused.json")
    assert refused["status"] == "refused" and refused["route"] == "official-playwright" and refused["reason"]
    assert refused["npm_version"] == ("11.17.0" if fault == "no_receipt" else None)
    assert p.macos_main(["check-official-install", "--work", str(lane.work)]) == 1
    with pytest.raises(ValueError, match="earlier step has no record: check-official-install"):
        lane.stage_record()


def test_an_official_install_that_could_not_run_is_stated_and_never_assembles(lane):
    lane.through("check-conversation")
    lane.step("check-official-install", chrome=lane.root / "no-such-browser")
    assert "official-playwright-install" not in lane.names()
    stage = lane.stage_record()
    gate = stage["gates"]["official_playwright_install"]
    assert gate["status"] == "not_run" and gate["reason"].startswith("no Chrome executable at ")
    with pytest.raises(ValueError, match="stage gates did not pass: official_playwright_install"):
        lane.assemble()


def test_accept_review_records_the_acceptance_of_one_exact_stage(lane, capsys):
    lane.through(lane.LAST)
    lane.stage_record()
    stage, review = lane.root / "stage", lane.root / "accepted.json"
    assert p.macos_main(["accept-review", "--stage", str(stage), "--output", str(review)]) == 0
    assert capsys.readouterr().out.strip().endswith("accepted shutdown review sha256 " + p.digest(review))
    accepted = p.read_json(review)
    pending = p.read_json(stage / "cli-qualification/native-shutdown-review.pending.json")
    assert accepted["disposition"] == "accepted" and accepted["evidence"] == pending["evidence"]
    assert accepted["pending_record_sha256"] == p.digest(stage / "cli-qualification/native-shutdown-review.pending.json")
    assert accepted["stage_sha256"] == p.digest(stage / "STAGE.json")
    value = lane.assemble(review=review)
    assert value["shutdown_review_sha256"] == p.digest(review)
    # The record is a function of the stage: a second run (the hosted stage two) with the digest the reader
    # stated writes the same bytes, and refuses any other stated digest before it writes anything.
    again = lane.root / "accepted-again.json"
    assert p.macos_main(["accept-review", "--stage", str(stage), "--review-sha256", p.digest(review),
                         "--output", str(again)]) == 0
    assert again.read_bytes() == review.read_bytes()
    assert p.macos_main(["accept-review", "--stage", str(stage), "--review-sha256", "0" * 64,
                         "--output", str(lane.root / "not-stated.json")]) == 1
    assert not (lane.root / "not-stated.json").exists()
    # The same command refuses a stage whose bound observation is missing or changed, and an existing output.
    assert p.macos_main(["accept-review", "--stage", str(stage), "--output", str(review)]) == 1
    (stage / "records" / (p.CLI_SHUTDOWN + ".log")).write_text("another observation\n")
    assert p.macos_main(["accept-review", "--stage", str(stage), "--output", str(lane.root / "second.json")]) == 1
    assert not (lane.root / "second.json").exists()
    # Even a stage record that agrees with itself is refused when an observation is not the one the
    # pending record binds: the record and its row were changed together here.
    changed = stage / "records" / (p.CLI_SHUTDOWN + ".log")
    value = p.read_json(stage / "STAGE.json")
    for row in value["files"]:
        if row["path"] == "records/" + changed.name:
            row.update(bytes=changed.stat().st_size, sha256=p.digest(changed))
    dump(stage / "STAGE.json", value)
    assert p.checked_stage(stage)["revision"] == REVISION
    with pytest.raises(ValueError, match="an observation the record binds is missing or changed"):
        p.macos_accept_review(stage, lane.root / "third.json")
    assert not (lane.root / "third.json").exists()


def test_the_repository_scanners_own_report_is_an_accepted_report(tmp_path):
    # The real payload scanner with the synthetic rules: its report.json is the form release-check takes.
    files = []
    for name in ("meerkat-connect-1.2.3-macos-arm64.tar.gz.sha256", "meerkat-connect-gateway-1.2.3-macos-arm64.tar.gz.sha256"):
        (tmp_path / name).write_text("0" * 64 + "  " + name.removesuffix(".sha256") + "\n")
        files.append(tmp_path / name)
    report = p.privacy_scan(files, (SCAN["rules"], SCAN["exemptions"], None), tmp_path / "scan")
    assets = [p.checked_file(path) for path in files]
    assert p.checked_scan_report(report, assets) == report and report["status"] == "clean"
    assert report["schema"] == "release-payload-privacy-scan/report/1" and p.HEX.fullmatch(report["rules_sha256"])


@pytest.mark.parametrize("change,accepted", (
    ({}, True),
    ({"rules_sha256": "d" * 64, "engine": {"sha256": "e" * 64, "exit_code": 0, "result": "clean"},
      "hits": {"accepted": {}, "refused": {}}, "coverage": {"files": 4, "bytes": 123}, "tool_version": "8.30.1",
      "schema": "release-payload-privacy-scan/report/1"}, True),
    ({"matched": ["a private name"]}, False),
    ({"hits": {"refused": {"names": 1}}, "detail": "the token text"}, False),
    ({"engine": {"result": "found a private name"}}, False),
    ({"note": "reviewed by hand"}, False),
    ({"a key with spaces": 1}, False),
    ({"tool_version": "version one"}, False),
    ({"status": "refused"}, False)))
def test_scan_report_may_hold_counts_and_digests_but_no_free_text(change, accepted):
    assets = [{"name": "meerkat-connect-1.2.3-macos-arm64.tar.gz", "size": 10, "sha256": "a" * 64},
              {"name": "meerkat-connect-1.2.3-macos-arm64.tar.gz.sha256", "size": 99, "sha256": "b" * 64}]
    report = {"status": "clean", "payload": [{"name": row["name"], "bytes": row["size"], "sha256": row["sha256"]}
                                             for row in assets], **change}
    if accepted:
        assert p.checked_scan_report(report, assets) == report
    else:
        with pytest.raises(ValueError):
            p.checked_scan_report(report, assets)
    # A file that is not one of the product files is never named by an accepted report.
    report["payload"].append({"name": "notes.txt", "bytes": 1, "sha256": "c" * 64})
    with pytest.raises(ValueError, match="not one of the product files"):
        p.checked_scan_report(report, assets)
