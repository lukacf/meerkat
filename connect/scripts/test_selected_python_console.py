"""Explicit console metadata/launch checks with inert wheels, no package imports."""
from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("console_readback_fixtures", Path(__file__).with_name("test_selected_reconciliation.py"))
R = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(R)

P, M = R.P, R.M


def configure(fixture, *, target="outside_server : Application.run", group="console_scripts", name="serve-it", extra=None):
    metadata = f"[{group}]\n{name} = {target}\n".encode()
    payloads = {"outside_server-1.0.dist-info/entry_points.txt": metadata,
                "outside_server/__init__.py": b"raise RuntimeError('never import fixture payload')\n"}
    payloads.update(extra or {})
    fixture.filename, raw, _ = P.wheel(extra=payloads)
    fixture.files["artifact.whl"] = raw
    fixture.plan.update(module="outside_server", entrypoint={
        "executable": "outside_server/__init__.py", "python_console_script": name,
        "arguments": ["literal ${HOME}", "x; __import__('os')", "--database", "/literal path"]})
    fixture.bind()


class PythonConsoleTests(unittest.TestCase):
    def fixture(self, **kwargs):
        fixture = P.SelectedPythonTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        configure(fixture, **kwargs)
        return fixture

    def closure(self, fixture):
        return M.selected_python_closure(fixture.files, fixture.plan, set(sys.stdlib_module_names), P.EXTENSIONS)

    def test_explicit_primary_console_uses_fixed_code_and_literal_arguments(self):
        fixture = self.fixture()
        closure = self.closure(fixture)
        self.assertEqual(closure["console_script"], {"name": "serve-it", "module": "outside_server", "attributes": "Application.run"})
        launch = M.selected_python_launch("/selected/runtime/bin/python", fixture.plan, closure)
        self.assertEqual(launch[:4], ["/selected/runtime/bin/python", "-I", "-B", "-c"])
        self.assertEqual(launch[4], M.PYTHON_CONSOLE_LAUNCH)
        self.assertEqual(launch[5:8], ["outside_server", "Application.run", "/selected/runtime/bin/serve-it"])
        self.assertEqual(launch[8:], fixture.plan["entrypoint"]["arguments"])
        called = []
        def run():
            called.append(list(sys.argv))
            return 7
        with patch("importlib.import_module", return_value=SimpleNamespace(Application=SimpleNamespace(run=run))) as imported:
            with patch.object(sys, "argv", ["-c", *launch[5:]]), self.assertRaises(SystemExit) as ended:
                exec(M.PYTHON_CONSOLE_LAUNCH, {})
        self.assertEqual(ended.exception.code, 7)
        imported.assert_called_once_with("outside_server")
        self.assertEqual(called, [["/selected/runtime/bin/serve-it", *fixture.plan["entrypoint"]["arguments"]]])

    def test_install_publishes_same_fixed_launch_without_executing_selected_code(self):
        fixture = self.fixture()
        fixture.software()
        closure = self.closure(fixture)
        def controlled(argv, cwd, env, label, timeout=900):
            if label in ("validate-selection", "python-runtime"):
                return fixture.fake_run(argv, cwd, env, label, timeout)
            fixture.calls.append(label)
            venv, site = cwd / "runtime", cwd / "runtime/lib/python3.11/site-packages"
            if label == "create-venv":
                (venv / "bin").mkdir(parents=True)
                (venv / "bin/python").write_bytes(b"inert interpreter, never executed")
                site.mkdir(parents=True)
            elif label in ("bootstrap-python", "installed-python"):
                rows = [["pip", "fixture"]]
                if label == "installed-python":
                    rows += [[name, version] for name, version in closure["versions"].items()]
                return M.encoded({"site": str(site), "distributions": rows})
            elif label == "pip-install":
                for wheel in closure["wheels"]:
                    for path, raw in wheel["files"].items():
                        output = site / path
                        output.parent.mkdir(parents=True, exist_ok=True)
                        output.write_bytes(raw)
                    for name, raw in (("RECORD", b"generated"), ("INSTALLER", b"pip\n"), ("REQUESTED", b"")):
                        (site / wheel["dist_info"] / name).write_bytes(raw)
                    for script in wheel["scripts"]:
                        (venv / "bin" / script).write_bytes(b"untrusted generated body")
            elif label != "pip-check":
                self.fail("unexpected installer effect " + label)
            return b""
        with patch.object(M, "run", side_effect=controlled):
            result = M.install_selected_python(fixture.args)
        self.assertEqual(result["command"][3:8], ["-c", M.PYTHON_CONSOLE_LAUNCH, "outside_server", "Application.run", str(Path(fixture.args.destination) / "runtime/bin/serve-it")])
        self.assertEqual(result["command"][8:], fixture.plan["entrypoint"]["arguments"])
        self.assertEqual(json.loads((Path(fixture.args.destination) / "ready/installed.json").read_bytes()), result)
        self.assertEqual(fixture.calls, ["validate-selection", "python-runtime", "create-venv", "bootstrap-python", "pip-install", "pip-check", "installed-python"])

    def test_module_mode_stays_exact_and_does_not_select_a_script(self):
        fixture = self.fixture()
        fixture.plan.update(module="outside_server.__main__", entrypoint={"executable": "outside_server/__main__.py", "arguments": ["literal"]})
        closure = self.closure(fixture)
        self.assertIsNone(closure["console_script"])
        self.assertEqual(M.selected_python_launch("/python", fixture.plan, closure), ["/python", "-I", "-B", "-m", "outside_server.__main__", "literal"])

    def test_selected_metadata_is_case_sensitive_console_only_and_has_no_extras(self):
        for kwargs in ({"group": "gui_scripts"}, {"target": "outside_server:run [feature]"},
                       {"target": "outside_server:run[feature]"}, {"target": "elsewhere:run"}):
            with self.subTest(kwargs=kwargs), self.assertRaises(M.InstallError):
                self.closure(self.fixture(**kwargs))
        fixture = self.fixture()
        for name in ("missing", "SERVE-IT", "", "../serve-it", "x/y", "x;run", "a\x00b", True):
            fixture.plan["entrypoint"]["python_console_script"] = name
            with self.subTest(name=name), self.assertRaises(M.InstallError):
                self.closure(fixture)
        # Dotted callable identifiers are not restricted to a function named main.
        self.assertEqual(M.python_script_reference("outside_server : API.caf\u00e9"), ("outside_server", "API.caf\u00e9", False))

    def test_ambiguous_or_injected_metadata_cannot_be_selected(self):
        for metadata in (b"[console_scripts]\nserve-it = outside_server:run\nserve-it = outside_server:other\n",
                         b"[console_scripts]\nserve-it = outside_server:run\n[gui_scripts]\nserve-it = outside_server:run\n",
                         b"[DEFAULT]\nserve-it = outside_server:run\n[console_scripts]\n",
                         b"[console_scripts]\nserve-it: outside_server:run\n"):
            with self.subTest(metadata=metadata), self.assertRaises((M.InstallError, M.configparser.Error)):
                self.closure(self.fixture(extra={"outside_server-1.0.dist-info/entry_points.txt": metadata}))
        for target in ("outside_server:run()", "outside_server:run;exit", "outside_server:__import__('os')", "../outside_server:run", "outside_server:run.other()", "outside_server:run [feature] trailing"):
            with self.subTest(target=target), self.assertRaises(M.InstallError):
                self.closure(self.fixture(target=target))

    def test_primary_target_and_namespace_cannot_be_supplied_or_shadowed_by_dependency(self):
        fixture = self.fixture()
        filename, raw, _ = P.wheel("dependency", extra={"dependency-1.0.dist-info/entry_points.txt": b"[console_scripts]\nonly-dependency = dependency:main\n"})
        fixture.files["closure/" + filename] = raw
        fixture.bind()
        fixture.plan["entrypoint"]["python_console_script"] = "only-dependency"
        with self.assertRaises(M.InstallError):
            self.closure(fixture)
        fixture.plan.update(module="dependency", entrypoint={"executable": "dependency/__init__.py", "python_console_script": "only-dependency", "arguments": []})
        with self.assertRaises(M.InstallError):
            self.closure(fixture)
        for path in ("outside_server.py", "outside_server.abi3.so", "outside_server/another.py"):
            fixture = self.fixture()
            filename, raw, _ = P.wheel("dependency", extra={path: b"inert"})
            fixture.files["closure/" + filename] = raw
            fixture.bind()
            with self.subTest(path=path), self.assertRaises(M.InstallError):
                self.closure(fixture)
        for path in ("outside_server.py", "outside_server.abi3.so", "outside_server/__init__.abi3.so"):
            with self.subTest(path=path), self.assertRaises(M.InstallError):
                self.closure(self.fixture(extra={path: b"inert"}))

    def test_readback_reconstructs_console_launch_without_trusting_generated_wrapper_or_receipt(self):
        helper = R.SelectedReadbackTests()
        self.addCleanup(helper.doCleanups)
        fixture, receipt = helper.fixture(True, software=True, configure=configure)
        result = helper.check(fixture)
        self.assertEqual(result["command"][3:5], ["-c", M.PYTHON_CONSOLE_LAUNCH])
        destination = Path(fixture.args.destination)
        # Generated wrapper hashes remain historical observations. Even a changed
        # wrapper plus a self-consistent receipt cannot replace canonical launch.
        wrapper = destination / "runtime/bin/serve-it"
        wrapper.write_bytes(b"changed but never invoked")
        receipt["verification"]["installer_generated"]["bin/serve-it"] = M.sha(wrapper.read_bytes())
        (destination / "ready/installed.json").write_bytes(M.encoded(receipt))
        helper.check(fixture)
        original = copy.deepcopy(receipt)
        for index, value in ((4, "raise RuntimeError('injected')"), (5, "dependency"), (6, "other")):
            changed = copy.deepcopy(original)
            changed["command"][index] = value
            changed["mcp_configuration"]["mcpServers"][fixture.plan["mcp_name"]]["args"] = changed["command"][1:]
            (destination / "ready/installed.json").write_bytes(M.encoded(changed))
            (destination / "ready/mcp.json").write_bytes(M.encoded(changed["mcp_configuration"]))
            helper.check(fixture, False)
        (destination / "ready/installed.json").write_bytes(M.encoded(original))
        (destination / "ready/mcp.json").write_bytes(M.encoded(original["mcp_configuration"]))
        helper.check(fixture)
        # Independent source bytes remain authoritative even if both retained
        # copies of wheel metadata and the mutable receipt were edited together.
        primary = destination / "runtime/lib/python3.11/site-packages/outside_server-1.0.dist-info/entry_points.txt"
        primary.write_bytes(b"[console_scripts]\nserve-it = outside_server:other\n")
        helper.check(fixture, False)


if __name__ == "__main__":
    unittest.main()
