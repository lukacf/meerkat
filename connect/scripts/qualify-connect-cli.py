#!/usr/bin/env python3
"""Build and exercise the two supplied-prefix CLI scenarios on a local Apple Silicon Mac.

This developer command does not assemble a distribution or accept native worker
shutdown. Its compiled harness remains dependent on its original Cargo paths.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import runpy
import shutil
import signal
import stat
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
CANDIDATE_MANIFEST = Path("native-candidate/Cargo.toml")
TARGET = "macos-arm64"
SCENARIOS = (
    ("deferred", "actual_cli_lazily_prepares_supplied_library_then_frontend", False),
    ("prebound", "actual_cli_prebound_host_retains_supplied_library_in_original_session", True),
)
BUILD_BOUND = 1800
SCENARIO_BOUND = 600
JOIN_BOUND = 15
KILL_BOUND = 5


class QualificationError(ValueError):
    """A fixed, public validation message, never child/provider prose."""


def require(condition, message):
    if not condition:
        raise QualificationError(message)


def failure(error):
    return str(error) if isinstance(error, QualificationError) else type(error).__name__


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def reference(path):
    require(path.is_absolute() and not path.is_symlink() and path.is_file(),
            "regular absolute evidence or artifact required")
    return {"path": str(path), "sha256": digest(path), "size": path.stat().st_size}


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


def status(label, state):
    # Observation output must never interrupt cleanup if its consumer closes.
    try:
        print(json.dumps({"stage": label, "status": state}), flush=True)
    except OSError:
        pass


def source_inputs():
    paths = {ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "scripts/repo-cargo",
             ROOT / CANDIDATE_MANIFEST, ROOT / CANDIDATE_MANIFEST.with_name("Cargo.lock"),
             ROOT / "scripts/assemble-connect-product.py", Path(__file__).resolve(),
             ROOT / "sdk/consumer-setup/product/prepare.py"}
    for name in ("rust-toolchain", "rust-toolchain.toml"):
        if (ROOT / name).exists():
            paths.add(ROOT / name)
    # Bound the Toolkit inputs of the selected candidate, including bundled
    # integration records and catalogs. External native source identity belongs
    # to the existing lane evidence and is not established by this inventory.
    for name in ("crates/meerkat-connect", "crates/toolkit-package", "crates/toolkit-store",
                 "crates/toolkit-catalog", "crates/meerkat-toolkit", "crates/toolkit-mobkit", ".cargo"):
        directory = ROOT / name
        if not directory.exists():
            continue
        for path in directory.rglob("*"):
            require(not path.is_symlink(), "source symlinks are not supported")
            if path.is_file():
                paths.add(path)
    require(len(paths) <= 4096 and sum(path.stat().st_size for path in paths) <= 128 * 1024 * 1024,
            "active source tree exceeds qualification bound")
    return {str(path.relative_to(ROOT)): {"sha256": digest(path), "size": path.stat().st_size}
            for path in sorted(paths)}


def process_rows():
    result = subprocess.run(["/bin/ps", "-axo", "pid=,ppid=,pgid=,lstart=,command="],
                            capture_output=True, check=True, timeout=5)
    require(len(result.stdout) <= 8 * 1024 * 1024, "process observation exceeds bound")
    rows = []
    for line in result.stdout.decode("utf-8", errors="strict").splitlines():
        fields = line.strip().split(None, 8)
        require(len(fields) == 9 and all(field.isdecimal() for field in fields[:3]),
                "unrecognized process observation")
        rows.append({"pid": int(fields[0]), "ppid": int(fields[1]), "pgid": int(fields[2]),
                     "started": " ".join(fields[3:8]), "command": fields[8]})
    return rows


def group_absent(pgid):
    try:
        os.killpg(pgid, 0)
        return False
    except ProcessLookupError:
        return True


def run_command(output, label, argv, cwd, environment, bound, marker=None):
    """One entered process owner, with bounded cleanup and value-free failures."""
    record = {"argv": list(map(str, argv)), "started_at": utc(), "bound_seconds": bound,
              "operator_signals": [], "errors": [], "exit_code": None}
    child = None
    owned = {}
    remaining = []
    interrupted = False
    started = time.monotonic()

    def interruption(_number, _frame):
        nonlocal interrupted
        interrupted = True
        if "operator_interrupt" not in record["errors"]:
            record["errors"].append("operator_interrupt")

    def observe():
        nonlocal remaining
        rows = process_rows()
        by_pid = {row["pid"]: row for row in rows}
        ancestors = {os.getpid()}
        parent = os.getpid()
        while parent in by_pid and by_pid[parent]["ppid"] not in ancestors:
            parent = by_pid[parent]["ppid"]
            ancestors.add(parent)
        pids = {child.pid} if child is not None and child.poll() is None else set()
        pids.update(pid for pid, start in owned if pid in by_pid and by_pid[pid]["started"] == start)
        while True:
            expanded = pids | {row["pid"] for row in rows if row["ppid"] in pids}
            if expanded == pids:
                break
            pids = expanded
        for row in rows:
            if row["pid"] in pids and row["pid"] not in ancestors:
                public = {key: value for key, value in row.items() if key != "command"}
                public["command_sha256"] = hashlib.sha256(row["command"].encode()).hexdigest()
                owned[(row["pid"], row["started"])] = public
        remaining = [row for row in rows if row["pid"] not in ancestors and
                     ((row["pid"], row["started"]) in owned or (marker and marker in row["command"]))]
        return rows

    def safe_observe():
        try:
            return observe()
        except Exception as error:
            record["errors"].append("process_observation:" + type(error).__name__)
            return None

    def signal_child_group(sig):
        rows = safe_observe()
        if child is None:
            return
        # Fixture-created CLI/worker groups are observed only. This command may
        # signal the entered harness/Cargo group, never adopt another supervisor.
        if child.poll() is not None and not any(
                row["pgid"] == child.pid and (row["pid"], row["started"]) in owned for row in rows or []):
            return
        try:
            os.killpg(child.pid, sig)
            record["operator_signals"].append({"pgid": child.pid, "signal": sig.name})
        except ProcessLookupError:
            pass
        except OSError as error:
            record["errors"].append("signal:" + type(error).__name__)

    def child_group_absent():
        try:
            return child is not None and group_absent(child.pid)
        except OSError as error:
            record["errors"].append("group_observation:" + type(error).__name__)
            return False

    def settled():
        observed = safe_observe()
        return (child is None or (child.poll() is not None and child_group_absent())) and observed is not None and not remaining

    def wait_settled(seconds):
        deadline = time.monotonic() + seconds
        while True:
            if settled():
                return True
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.1)

    status(label, "running")
    previous_interrupt = signal.signal(signal.SIGINT, interruption)
    try:
        with (output / (label + ".stdout")).open("xb") as stdout, (output / (label + ".stderr")).open("xb") as stderr:
            try:
                # Popen belongs inside the same try/finally as every wait.
                require(not interrupted, "operator interrupted command")
                child = subprocess.Popen(record["argv"], cwd=cwd, env=environment,
                                         stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                         start_new_session=True)
                record.update(pid=child.pid, pgid=child.pid)
                while child.poll() is None:
                    require(not interrupted, "operator interrupted command")
                    observe()
                    require(time.monotonic() - started < bound, "observation_deadline")
                    time.sleep(0.2)
            except BaseException as error:
                record["errors"].append("execution:" + failure(error))
            finally:
                if not wait_settled(1):
                    signal_child_group(signal.SIGINT)
                    if not wait_settled(JOIN_BOUND):
                        signal_child_group(signal.SIGKILL)
                        wait_settled(KILL_BOUND)
                if child is not None:
                    try:
                        record["exit_code"] = child.wait(timeout=KILL_BOUND)
                    except subprocess.TimeoutExpired:
                        record["errors"].append("direct_child_not_reaped")
    except BaseException as error:
        record["errors"].append("output_or_cleanup:" + type(error).__name__)
    finally:
        # Repeated ordinary Ctrl-C records failure but cannot skip cleanup.
        signal.signal(signal.SIGINT, previous_interrupt)
    # Independent final observations cannot turn a failed cleanup into success.
    safe_observe()
    record.update(finished_at=utc(), seconds=time.monotonic() - started,
                  direct_child_reaped=child is not None and child.poll() is not None,
                  process_group_absent=child_group_absent(),
                  observed_processes=list(owned.values()),
                  remaining_processes=[{key: value for key, value in row.items() if key != "command"}
                                       for row in remaining])
    record["logs"] = [reference(output / (label + suffix)) for suffix in (".stdout", ".stderr")
                      if (output / (label + suffix)).is_file()]
    record["passed"] = (record["exit_code"] == 0 and record["direct_child_reaped"]
                        and record["process_group_absent"] and not record["remaining_processes"]
                        and not record["operator_signals"] and not record["errors"])
    save(output / (label + ".json"), record)
    status(label, "passed" if record["passed"] else "failed")
    return record


def cargo_artifacts(path):
    expected = {"meerkat-connect": ("bin", ROOT / "crates/meerkat-connect/src/main.rs"),
                "cli_paired_host": ("test", ROOT / "crates/meerkat-connect/tests/cli_paired_host.rs"),
                "packaged_setup": ("test", ROOT / "crates/meerkat-connect/tests/packaged_setup.rs")}
    found = {}
    finished = []
    with path.open(encoding="utf-8") as stream:
        for line in stream:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("reason") == "build-finished":
                finished.append(message.get("success"))
            if message.get("reason") != "compiler-artifact" or not message.get("executable"):
                continue
            target = message["target"]
            name = target["name"]
            if name not in expected:
                continue
            kind, source = expected[name]
            require(target["kind"] == [kind] and Path(target["src_path"]).resolve(strict=True) == source
                    and message["profile"]["test"] == (kind == "test"), "unexpected Cargo target identity")
            require(name not in found, "ambiguous Cargo artifact")
            artifact = Path(message["executable"])
            require(artifact.is_absolute() and not artifact.is_relative_to(ROOT), "artifact must be outside checkout")
            found[name] = {**reference(artifact), "cargo_artifact": message}
    require(set(found) == set(expected) and finished == [True], "missing successful Cargo artifacts")
    return found


def preflight(destination):
    require(platform.system() == "Darwin" and platform.machine() == "arm64", "local Apple Silicon Mac required")
    version = tuple(int(part) for part in platform.mac_ver()[0].split(".")[:2])
    require(version >= (13, 5), "supplied runtimes require macOS 13.5 or later")
    require(destination.is_absolute() and not destination.exists() and not destination.is_symlink()
            and destination.parent.resolve(strict=True) == destination.parent
            and not destination.is_relative_to(ROOT), "fresh canonical destination outside checkout required")
    before = source_inputs()
    assembly = runpy.run_path(str(ROOT / "scripts/assemble-connect-product.py"))
    selected = assembly["TARGETS"][TARGET]
    assembly["pin"](selected["component"], selected["component_sha256"], selected["component_size"])
    assembly["pin"](assembly["PREPARE"], assembly["PREPARE_SHA"])
    verifier = runpy.run_path(str(assembly["PREPARE"]))
    require(source_inputs() == before, "qualification inputs changed during preflight")
    return assembly, selected, verifier, before


def qualify(destination):
    assembly, selected, verifier, before = preflight(destination)
    destination.mkdir(mode=0o700)
    output = destination / "evidence"
    output.mkdir(mode=0o700)
    result = {"schema_version": 1, "kind": "meerkat_connect_supplied_prefix_cli_qualification",
              "scope": "two_supplied_prefix_terminal_scenarios_with_synthetic_provider_and_paired_host",
              "source_scope": "toolkit_inputs_only_external_native_identity_requires_lane_evidence",
              "cargo_manifest": str(ROOT / CANDIDATE_MANIFEST),
              "status": "failed", "target": TARGET, "started_at": utc(), "executions": []}
    try:
        save(output / "source-inputs.json", before)
        result["source_manifest"] = reference(output / "source-inputs.json")
        result["resource_component"] = reference(selected["component"])
        result["resources_sha256"] = selected["resources_sha256"]
        build_env = {key: value for key, value in os.environ.items() if key in (
                     "PATH", "HOME", "USER", "LOGNAME", "SHELL", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN",
                     "SDKROOT", "DEVELOPER_DIR", "RUST_LANE_ID", "CARGO_BUILD_JOBS", "RUSTFLAGS",
                     "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
                     or key.startswith("CARGO_PROFILE_")}
        build_env.update(CARGO_NET_OFFLINE="true",
                         MEERKAT_CONNECT_RESOURCES_SHA256=selected["resources_sha256"])
        result["cargo_env"] = {key: build_env[key] for key in
                              ("CARGO_NET_OFFLINE", "MEERKAT_CONNECT_RESOURCES_SHA256")}
        result["inherited_build_options"] = sorted(key for key in build_env if
                                                  key.startswith(("CARGO_PROFILE_", "RUST")) or key == "CARGO_BUILD_JOBS")
        cargo = run_command(output, "build", [ROOT / "scripts/repo-cargo", "test", "--locked", "--offline",
                            "--manifest-path", ROOT / CANDIDATE_MANIFEST,
                            "-p", "meerkat-connect", "--test", "cli_paired_host", "--test", "packaged_setup",
                            "--no-run", "--message-format=json"], ROOT, build_env, BUILD_BOUND)
        result["cargo"] = reference(output / "build.json")
        require(cargo["passed"], "resource-bound build did not pass")
        artifacts = cargo_artifacts(output / "build.stdout")
        for artifact in artifacts.values():
            assembly["check_binary"](Path(artifact["path"]), artifact["sha256"], target=TARGET)
        save(output / "artifacts.json", artifacts)
        result["artifacts"] = reference(output / "artifacts.json")
        result["candidate"] = {"target": TARGET, "source_manifest_sha256": result["source_manifest"]["sha256"],
                               "resources_sha256": selected["resources_sha256"],
                               "companion_cli_binary": {key: artifacts["meerkat-connect"][key] for key in ("sha256", "size")},
                               "test_executable": {key: artifacts["packaged_setup"][key] for key in ("sha256", "size")}}
        result["cli_test_executable"] = {key: artifacts["cli_paired_host"][key] for key in ("path", "sha256", "size")}
        require(source_inputs() == before, "build inputs changed")
        for name in ("prefix", "cwd", "home", "tmp", "empty-path", "user-config"):
            (destination / name).mkdir(mode=0o700)
        test_env = {"HOME": str(destination / "home"), "TMPDIR": str(destination / "tmp"),
                    "PATH": str(destination / "empty-path"), "XDG_CONFIG_HOME": str(destination / "user-config"),
                    "PYTHONNOUSERSITE": "1", "PIP_NO_INDEX": "1", "LC_ALL": "C", "TZ": "UTC"}
        assembly["pin"](selected["component"], selected["component_sha256"], selected["component_size"])
        extracted = run_command(output, "extract", ["/usr/bin/tar", "--no-same-owner", "-xpf",
                                selected["component"], "-C", destination / "prefix"],
                                destination / "cwd", test_env, 120, str(destination))
        result["resource_extraction"] = reference(output / "extract.json")
        require(extracted["passed"], "resource extraction did not pass")
        prefix = destination / "prefix" / ("meerkat-connect-" + TARGET)
        require({path.name for path in (destination / "prefix").iterdir()} == {prefix.name}
                and {path.name for path in prefix.iterdir()} == {"bin", "libexec"}
                and not list((prefix / "bin").iterdir()), "unexpected supplied prefix shape")
        resources = prefix / "libexec/meerkat-connect"
        manifest, pin = verifier["verify_resources"](resources)
        require(pin == selected["resources_sha256"] and manifest["target"] == TARGET
                and stat.S_IMODE(resources.stat().st_mode) == 0o555
                and stat.S_IMODE((resources / "resources.json").stat().st_mode) == 0o444,
                "resource manifest or root mode mismatch")
        save(output / "resources-before.json", {"resources_sha256": pin, "entries": manifest["entries"]})
        copied = prefix / "bin/meerkat-connect"
        shutil.copyfile(artifacts["meerkat-connect"]["path"], copied)
        copied.chmod(0o755)
        assembly["pin"](copied, artifacts["meerkat-connect"]["sha256"], artifacts["meerkat-connect"]["size"])
        for label, name, prebound in SCENARIOS:
            require(source_inputs() == before, "source inputs changed before scenario")
            for artifact in artifacts.values():
                assembly["pin"](Path(artifact["path"]), artifact["sha256"], artifact["size"])
            assembly["pin"](copied, artifacts["meerkat-connect"]["sha256"], artifacts["meerkat-connect"]["size"])
            scenario_path = output / (label + "-scenario.json")
            execution = run_command(output, label, [artifacts["cli_paired_host"]["path"], "--ignored", "--exact", name, "--nocapture"],
                                    destination / "cwd", test_env | {"CONNECT_REAL_LIBRARY_PREFIX": str(prefix),
                                    "CONNECT_REAL_LIBRARY_EVIDENCE": str(scenario_path)}, SCENARIO_BOUND, str(destination))
            result["executions"].append(reference(output / (label + ".json")))
            require(execution["passed"], "supplied-prefix test did not pass")
            require(scenario_path.stat().st_size <= 1024 * 1024, "scenario record exceeds bound")
            scenario = verifier["decode"](scenario_path.read_bytes())
            require(scenario["schema_version"] == 1 and scenario["kind"] == "meerkat_connect_supplied_cli_scenario"
                    and scenario["scenario"] == name and scenario["stage"] == "passed"
                    and scenario["platform"] == {"os": "macos", "arch": "aarch64"}
                    and scenario["prebound_host"] == prebound and scenario["supplied_prefix"] == str(prefix),
                    "scenario identity mismatch")
            for field, artifact in (("cli_artifact", "meerkat-connect"), ("test_artifact", "cli_paired_host")):
                require(scenario[field] == {key: artifacts[artifact][key] for key in ("sha256", "size")},
                        "scenario artifact mismatch")
            require(scenario["cli_exit_code"] == 0 and scenario["cli_group_absent"] is True
                    and scenario["terminal_restored"] is True and scenario["fixture_servers_joined"] is True
                    and scenario["both_turns_persisted"] is True and scenario["native_session_count"] == 1
                    and scenario["frontend_submissions"] == 1 and scenario["pairings"] == 1,
                    "scenario terminal or original-session evidence incomplete")
            library = scenario["library"]
            require(library["resources_sha256"] == pin and library["catalog_scope"] == "catalog_metadata_only"
                    and library["complete_entry_matches_pin"] is True and library["preparation_count"] == 1
                    and library["job_count"] == 0 and library["all_families_unprepared"] is True
                    and library["request_and_install_directories_empty"] is True
                    and library["browser_companion_requested"] is False and library["native_install_denied"] is True,
                    "supplied library scope mismatch")
            after, after_pin = verifier["verify_resources"](resources)
            require(after == manifest and after_pin == pin
                    and stat.S_IMODE(resources.stat().st_mode) == 0o555
                    and stat.S_IMODE((resources / "resources.json").stat().st_mode) == 0o444,
                    "supplied resource closure or root modes changed")
            for artifact in artifacts.values():
                assembly["pin"](Path(artifact["path"]), artifact["sha256"], artifact["size"])
            assembly["pin"](copied, artifacts["meerkat-connect"]["sha256"], artifacts["meerkat-connect"]["size"])
            require(stat.S_IMODE(copied.stat().st_mode) == 0o755, "installed CLI mode changed")
            save(output / (label + "-closure.json"), {"resources_sha256": pin,
                 "entries": len(after["entries"]), "matches_before": True,
                 "resource_root_mode": 0o555, "manifest_mode": 0o444, "cli_mode": 0o755})
        result["scenarios"] = [reference(output / (label + "-scenario.json")) for label, _, _ in SCENARIOS]
        result["resource_closure_unchanged"] = True
        result["status"] = "passed"
    except BaseException as error:
        # Only our constant validation messages are public; child prose stays private.
        result["failure"] = failure(error)
    finally:
        try:
            after = source_inputs()
            save(output / "source-inputs-after.json", after)
            result["sources_unchanged"] = before is not None and before == after
        except Exception as error:
            result["source_observation_error"] = type(error).__name__
            result["sources_unchanged"] = False
        if not result["sources_unchanged"]:
            result["status"] = "failed"
        result["finished_at"] = utc()
        result["not_qualified"] = ["installed_composition", "active_setup_worker_shutdown", "live_accounts",
                                   "service_installation", "frontend_activation", "distribution", "other_targets"]
        save(output / "result.json", result)
        status("qualification", result["status"])
    return 0 if result["status"] == "passed" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        return qualify(args.destination)
    except (OSError, ValueError) as error:
        status("preflight_or_evidence", failure(error))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
