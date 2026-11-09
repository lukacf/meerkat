#!/usr/bin/env python3
"""Real Driver child-test acceptance using the existing private SDK harness.

Builds nothing. The only forced ordering is a cfg(test) call to Driver's actual
admission fence after held completion and before final application selection.
Native Ready, configured acquisition, recovery and joined shutdown stay real.
This does not qualify SIGTERM, nonzero native drain or a shipped product binary.
"""
from __future__ import annotations

import argparse
import asyncio
import importlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import sys
import traceback
from types import SimpleNamespace


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def wrapper(python, binary, selector, report, fence):
    # Exec preserves the SDK's actual child PID and private descriptors. The
    # selected test restores the protocol pipe after libtest's own header, so
    # neither native JSON output nor Ready is parsed or rewritten by a relay.
    return "#!" + str(python) + "\n" + """import json, os, sys
binary, selector, report, fence = CONFIG
args = sys.argv[1:]
env = dict(os.environ)
# The gateway harness deliberately isolates its environment. This wrapper runs
# a libtest child, which needs the native control lane's explicit 32 MiB stack.
env['RUST_MIN_STACK'] = '33554432'
protocol = os.dup(1)
os.set_inheritable(protocol, True)
env.update(CONNECT_LATE_ADOPTION_TEST_CHILD=json.dumps(args),
           CONNECT_LATE_ADOPTION_TEST_REPORT=report,
           CONNECT_LATE_ADOPTION_TEST_FENCE=fence,
           CONNECT_LATE_ADOPTION_TEST_STDOUT=str(protocol))
command = [binary, '--exact', selector, '--ignored', '--quiet', '--nocapture', '--test-threads=1']
with open(os.devnull, 'wb') as harness_output:
    os.dup2(harness_output.fileno(), 1)
os.execve(binary, command, env)
""".replace("CONFIG", repr((str(binary), selector, str(report), fence)))


def main(*, drain_factory=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-executable", required=True, type=Path)
    parser.add_argument("--selector", required=True)
    parser.add_argument("--resource-executable", required=True, type=Path)
    parser.add_argument("--resources-sha256", required=True)
    parser.add_argument("--toolkit-python", required=True, type=Path)
    parser.add_argument("--meta-python", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    for name in ("test_executable", "resource_executable", "toolkit_python", "meta_python", "output"):
        path = getattr(args, name)
        if not path.is_absolute():
            parser.error("all selected paths must be absolute")
        setattr(args, name, path.resolve())
    if args.output.exists():
        parser.error("output must be fresh; previous evidence is retained")
    expected_selector = "application_driver::late_adoption_tests::" + (
        "nonzero_drain_refuses_new_helper_and_completes_entered_console_turn" if drain_factory else
        "late_adoption_fences_actual_acquisition_before_native_ready_and_joins")
    if args.selector != expected_selector:
        parser.error("only the selected real Driver child control is supported")
    scripts = Path(__file__).resolve().parent.parent
    v = load("late_adoption_native_verifier", scripts / "verify_native_gateway_subprocess.py")
    c = load("late_adoption_console_fixture", scripts / "verify_installed_console.py")
    args.output.mkdir(mode=0o700, parents=True)
    report = {"schema": 1, "status": "running", "cases": [],
              "scope": "native lib-test Driver/private channel/real Ready; no product or SIGTERM qualification",
              "nonzero_drain": drain_factory is not None}
    try:
        sys.path[0:0] = [str(args.toolkit_python), str(args.meta_python)]
        v.SDK = importlib.import_module("meerkat_toolkit.host_control")
        v.APP_RUNTIME = importlib.import_module("meerkat_toolkit.application_runtime")
        v.META_BUILDER = importlib.import_module("meerkat_meta.builder")
        v.META_IDENTITY = importlib.import_module("meerkat_meta.identity_first_models")
        selected = [(v.SDK, args.toolkit_python), (v.APP_RUNTIME, args.toolkit_python)]
        selected.extend((importlib.import_module(name), args.meta_python) for name in (
            "meerkat_meta", "meerkat_meta.builder", "meerkat_meta.runtime", "meerkat_meta._transport",
            "meerkat_meta.agent_builder", "meerkat_meta.identity_first_models",
            "meerkat_meta.identity_first_providers", "meerkat_meta.types"))
        for module, root in selected:
            path = Path(module.__file__).resolve()
            v.require(path.is_relative_to(root), "SDK import escaped its explicit selection")
            report.setdefault("sdk_sources", []).append(v.artifact_pin(path))
        v.require(args.resource_executable.parent.name == "bin", "selected resource package layout")
        resource_root = args.resource_executable.parent.parent / "libexec/meerkat-connect"
        resource_raw = v.exact_bytes(resource_root / "resources.json", args.resources_sha256,
                                     limit=32 * 1024 * 1024)
        manifest = v.unique_json(resource_raw)
        v.require(manifest["kind"] == "meerkat_connect_resources", "selected resource manifest kind")
        package = args.output / "native-test-package"
        (package / "bin").mkdir(mode=0o700, parents=True)
        # No hard links: the acceptance child cannot modify the retained donor.
        shutil.copytree(resource_root, package / "libexec/meerkat-connect", symlinks=True)
        binary = package / "bin/connect-lib-test"
        shutil.copy2(args.test_executable, binary)
        v.require(v.digest(binary) == v.digest(args.test_executable), "emitted test binary copy changed")
        v.require(v.exact_bytes(package / "libexec/meerkat-connect/resources.json") == resource_raw,
                  "resource copy changed selection")
        candidate = {"kind": "emitted_native_test_executable", "executable": v.artifact_pin(binary),
                     "emitted": v.artifact_pin(args.test_executable),
                     "resources_sha256": args.resources_sha256,
                     "resource_origin": v.artifact_pin(resource_root / "resources.json")}
        report["candidate"] = candidate
        report["fixture_sources"] = [v.artifact_pin(path) for path in (
            Path(__file__).resolve(), scripts / "verify_native_gateway_subprocess.py",
            scripts / "verify_installed_console.py")]
        case_root = args.output / "cases"
        case_root.mkdir(mode=0o700)

        class Scenario(c.ConsoleScenario):
            scope = "real late Driver adoption with configured acquisition; Ready and joined recovery observation"

            def __init__(self, origin, fenced, observations):
                super().__init__(v, origin)
                self.fenced, self.observations = fenced, observations
                self.name = "late-adoption-" + ("fenced" if fenced else "unfenced")

            def observed(self, name):
                return v.unique_json(v.exact_bytes(self.observations / (name + ".json")))

            async def run(self, private, provider, directory, events, process, base_url):
                v.require(process.poll() is None and base_url.rstrip("/") == self.origin,
                          "same native Ready child is not serving the selected listener")
                v.require(self.observed("before-selection") == {
                    "native_held_completion": True, "installed": False, "closed": False,
                    "admission_stopped": self.fenced}, "fence was not before late retention")
                for phase in ("retained", "before_ready", "after_ready"):
                    expected = {"same_installed_owner": True, "recovery": {
                        "stopped": self.fenced, "selected_owner_control": phase == "after_ready",
                        "task": "Running" if phase == "after_ready" and not self.fenced else "Absent"}}
                    v.require(self.observed(phase) == expected, "actual recovery owner/state mismatch at " + phase)
                v.require(not provider.requests, "lifecycle control unexpectedly admitted a model turn")
                return {"status": "passed", "fenced": self.fenced,
                        "real_ready_observed_by_existing_harness": True}

            def check_final_artifacts(self, directory, result):
                v.require(self.observed("joined") == {"same_installed_owner": True,
                    "host_source_local_shared_joined": True, "recovery": {
                        "stopped": True, "selected_owner_control": True,
                        "task": "Absent" if self.fenced else "Finished"}}, "actual recovery was not joined")
                v.require(self.observed("driver-return") == {"returned_ok": True, "error": None},
                          "Driver/private/native cleanup did not return successfully")
                v.require(result.get("model_request_count") == 0 and result.get("provider_joined") is True,
                          "unexpected model work or unjoined provider")
                v.require(len(result.get("cleanup_observation", {})) == 9
                          and all(result["cleanup_observation"].values()), "incomplete actual cleanup")
                result["owner_observations"] = [v.artifact_pin(self.observations / (name + ".json")) for name in (
                    "before-selection", "retained", "before_ready", "after_ready", "joined", "driver-return")]

        for fence in (("drain",) if drain_factory else ("fenced", "unfenced")):
            observations = args.output / (fence + "-owner")
            observations.mkdir(mode=0o700)
            gateway = package / "bin" / ("toolkit_gateway_" + fence)
            gateway.write_text(wrapper(Path(sys.executable).resolve(), binary, args.selector, observations, fence))
            gateway.chmod(0o700)
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                origin = "http://127.0.0.1:" + str(listener.getsockname()[1])
            scenario = (drain_factory(Scenario, v, c, origin, observations) if drain_factory else
                        Scenario(origin, fence == "fenced", observations))
            result = asyncio.run(v._run_installed_product(
                SimpleNamespace(output=case_root, toolkit_gateway=gateway), candidate, package,
                manifest, corrupt=False, console=scenario))
            case = {"case": scenario.name, "result": v.artifact_pin(
                case_root / scenario.name / "result.json"), "status": result["status"],
                "wrapper": v.artifact_pin(gateway)}
            if drain_factory and result["status"] == "passed":
                case["drain_capture"] = result["nonzero_drain"]["capture"]
            report["cases"].append(case)
            v.require(result["status"] == "passed", "real late-adoption case failed")
        v.require(v.digest(binary) == candidate["emitted"]["sha256"], "native test executable changed")
        v.require(v.exact_bytes(resource_root / "resources.json") == resource_raw,
                  "retained resource selection changed")
        report["status"] = "passed"
    except BaseException as error:
        report.update(status="failed", error=repr(error), traceback=traceback.format_exc())
    finally:
        v.save_json(args.output / "result.json", report)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
