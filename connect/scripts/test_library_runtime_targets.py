"""Library target admission tests; scripted installs are not Linux runtime proof."""
from __future__ import annotations

import argparse
import copy
from contextlib import ExitStack
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SCRIPT = Path(__file__).with_name("install-library-consumer.py")
SPEC = importlib.util.spec_from_file_location("library_runtime_targets", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TARGET = {
    "python": {"implementation": "cpython", "version": "3.11.16", "platform": "linux",
               "machine": "x86_64", "pointer_bits": 64, "glibc": "glibc 2.39"},
    "node": {"version": "v22.23.2", "platform": "linux", "arch": "x64", "npm_version": "10.9.8"},
}


class RuntimeTargetTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="toolkit-runtime-target-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.counter = 0
        self.calls = []

    def fixture(self, family="services", schema=2, target=TARGET, configured=False):
        self.counter += 1
        bundle = self.root / ("bundle-" + str(self.counter))
        (bundle / family).mkdir(parents=True)
        if family in MODULE.FAMILIES:
            namespace = MODULE.FAMILIES[family][0]
            out = io.BytesIO()
            with zipfile.ZipFile(out, "w") as archive:
                archive.writestr(namespace + "-0.1.0.dist-info/METADATA",
                                 "Metadata-Version: 2.1\nName: " + namespace + "\nVersion: 0.1.0\n")
            raw = out.getvalue()
            files = {namespace + "-0.1.0-py3-none-any.whl": raw,
                     "install-offline.txt": (MODULE.name(namespace) + "==0.1.0 --hash=sha256:" + MODULE.sha(raw) + "\n").encode()}
        else:
            component, filename = "@rkat/toolkit-activepieces", "rkat-toolkit-activepieces-0.1.0.tgz"
            deps = {component: "file:./" + filename}
            files = {
                filename: b"inert component fixture", "dependency.tgz": b"inert dependency fixture",
                "package.json": MODULE.encoded({"dependencies": deps}),
                "package-lock.json": MODULE.encoded({"lockfileVersion": 3, "packages": {
                    "": {"dependencies": deps},
                    "node_modules/" + component: {"version": "0.1.0", "resolved": "file:" + filename},
                    "node_modules/dependency": {"version": "1", "resolved": "https://example.invalid/dependency.tgz", "integrity": "sha512-fixture"},
                }}),
                "offline-cache.json": MODULE.encoded({"schema_version": 1, "tarballs": [{
                    "name": "dependency", "version": "1", "path": "dependency.tgz",
                    "integrity": "sha512-fixture", "sha256": MODULE.sha(b"inert dependency fixture"),
                }]}),
            }
        for key, data in files.items():
            (bundle / family / key).write_bytes(data)
        contents = {"schema_version": schema, "files": {
            family + "/" + key: {"sha256": MODULE.sha(data)} for key, data in files.items()}}
        if target is not None:
            contents["runtime_target"] = copy.deepcopy(target)
        raw = MODULE.encoded(contents)
        (bundle / "CONTENTS.json").write_bytes(raw)
        args = argparse.Namespace(bundle=str(bundle), contents_sha256=MODULE.sha(raw), family=family,
            destination=str(self.root / ("install-" + str(self.counter))), python=sys.executable,
            node=sys.executable, npm_cli=sys.executable, config=None, config_sha256=None, server_name=None)
        if configured:
            config = self.root / ("config-" + str(self.counter) + ".json")
            config.write_bytes(b'{"profile":"fixture","token_env":"PRIVATE_SELECTED_ACCOUNT"}\n')
            args.config, args.config_sha256, args.server_name = str(config), MODULE.sha(config.read_bytes()), "selected"
        return args, contents

    def before_effects(self, args):
        with patch.object(MODULE, "run") as child, self.assertRaises((ValueError, KeyError, TypeError, OSError)):
            MODULE.install(args)
        child.assert_not_called()
        self.assertFalse(Path(args.destination).exists())

    def scripted(self, args, *, python=None, node=None, npm="10.9.8", fail=None):
        """Use real byte/schema preflight, but explicitly fake package-manager work."""
        self.calls = []
        family = args.family

        def child(argv, cwd, env, label, timeout=900, **options):
            self.calls.append({"argv": argv, "label": label, "env": dict(env), "options": options})
            for key in ("HOME", "CODEX_HOME", "GITHUB_TOKEN", "PRIVATE_SELECTED_ACCOUNT", "PYTHONPATH", "NODE_OPTIONS", "HTTP_PROXY"):
                self.assertNotIn(key, env)
            if label == fail:
                raise KeyboardInterrupt
            if label == "python-runtime":
                return MODULE.encoded(TARGET["python"] if python is None else python)
            if label == "node-runtime":
                return MODULE.encoded([TARGET["node"][key] for key in ("version", "platform", "arch")] if node is None else node)
            if label == "npm-runtime":
                self.assertEqual(argv[-3:], ["--prefix", str(cwd), "--version"])
                self.assertNotEqual(env["npm_config_userconfig"], env["npm_config_globalconfig"])
                for key in ("npm_config_userconfig", "npm_config_globalconfig"):
                    self.assertEqual(Path(env[key]).read_bytes(), b"")
                self.assertEqual(json.loads((cwd / "package.json").read_bytes())["dependencies"],
                                 {"@rkat/toolkit-activepieces": "file:./rkat-toolkit-activepieces-0.1.0.tgz"})
                return (npm + "\n").encode()
            if label in ("bootstrap-python", "installed-python"):
                graph = [["pip", "1"], ["setuptools", "2"]]
                if label == "installed-python":
                    graph.append([MODULE.FAMILIES[family][0], "0.1.0"])
                return MODULE.encoded({"distributions": graph, "site": str(cwd / "runtime/lib/python3.11/site-packages")})
            if label == "npm-install":
                for path, name, version in [("@rkat/toolkit-activepieces", "@rkat/toolkit-activepieces", "0.1.0"),
                                            ("dependency", "dependency", "1")]:
                    folder = cwd / "node_modules" / path
                    folder.mkdir(parents=True)
                    (folder / "package.json").write_bytes(MODULE.encoded({"name": name, "version": version}))
            if label == "profiles":
                return b'{"profiles":[{"profile":"fixture"}]}'
            if label == "check-config":
                self.assertEqual(options, {"discard_output": True})
                self.assertFalse((cwd / "ready").exists())
                return b"IGNORED_PRIVATE_VALIDATOR_OUTPUT"
            return b""

        with ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {key: "MUST_NOT_LEAK" for key in
                ("HOME", "CODEX_HOME", "GITHUB_TOKEN", "PRIVATE_SELECTED_ACCOUNT", "PYTHONPATH", "NODE_OPTIONS", "HTTP_PROXY")}))
            stack.enter_context(patch.object(MODULE, "run", side_effect=child))
            stack.enter_context(patch.object(MODULE, "verify_wheel_payload", return_value={"payloads": 1}))
            stack.enter_context(patch.object(MODULE, "verify_npm_payload", return_value=1))
            return MODULE.install(args)

    def test_schema_version_must_be_an_integer_not_boolean_float_or_string(self):
        for version in [True, False, 1.0, 2.0, "1", "2", None, 0, 3]:
            args, _ = self.fixture(schema=version, target=None if version == 1 else TARGET)
            with self.subTest(version=repr(version)):
                self.before_effects(args)

    def test_schema2_requires_exact_target_keys_and_field_kinds(self):
        cases = [None, {}, [], {"python": TARGET["python"]}, {**TARGET, "fallback": "darwin"}]
        for family in TARGET:
            for key in TARGET[family]:
                value = copy.deepcopy(TARGET)
                del value[family][key]
                cases.append(value)
            value = copy.deepcopy(TARGET)
            value[family]["extra"] = "unsupported"
            cases.append(value)
        for family, key, invalid in [
            ("python", "implementation", "pypy"), ("python", "platform", "darwin"),
            ("python", "machine", "aarch64"), ("python", "pointer_bits", True),
            ("python", "pointer_bits", 64.0), ("python", "pointer_bits", 32),
            ("python", "version", "3.12.1"), ("python", "version", [3, 11, 16]),
            ("python", "glibc", "musl 1.2"), ("python", "glibc", None),
            ("node", "platform", "darwin"), ("node", "arch", "arm64"),
            ("node", "version", "22.23.2"), ("node", "version", [22, 23, 2]),
            ("node", "npm_version", "10.9"), ("node", "npm_version", 10),
        ]:
            value = copy.deepcopy(TARGET)
            value[family][key] = invalid
            cases.append(value)
        for index, target in enumerate(cases):
            args, _ = self.fixture(target=target)
            with self.subTest(index=index):
                self.before_effects(args)

    def test_schema1_cannot_silently_ignore_any_runtime_target(self):
        for target in [TARGET, {}, "linux"]:
            args, _ = self.fixture(schema=1, target=target)
            self.before_effects(args)
        with self.assertRaises(ValueError):
            MODULE.library_runtime_target({"schema_version": 1, "runtime_target": None})

    def test_selected_contents_digest_binds_target_without_self_authorization(self):
        args, contents = self.fixture()
        contents["runtime_target"]["python"]["version"] = "3.11.17"
        (Path(args.bundle) / "CONTENTS.json").write_bytes(MODULE.encoded(contents))
        self.before_effects(args)

    def test_duplicate_schema_or_nested_target_key_refuses(self):
        args, contents = self.fixture()
        for key in ["schema_version", "glibc"]:
            original = MODULE.encoded(contents)
            needle = b'"schema_version": 2' if key == "schema_version" else b'"glibc": "glibc 2.39"'
            raw = original.replace(needle, needle + b", " + needle)
            (Path(args.bundle) / "CONTENTS.json").write_bytes(raw)
            args.contents_sha256 = MODULE.sha(raw)
            self.before_effects(args)

    def test_python_target_mismatch_stops_before_venv_and_package_manager(self):
        for key, value in [("version", "3.11.15"), ("implementation", "pypy"),
                           ("platform", "darwin"), ("machine", "aarch64"),
                           ("pointer_bits", 32), ("glibc", "glibc 2.38"), ("glibc", None)]:
            observed = {**TARGET["python"], key: value}
            args, _ = self.fixture()
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.scripted(args, python=observed)
            self.assertEqual([x["label"] for x in self.calls], ["python-runtime"])
            self.assertTrue((Path(args.destination) / "inputs").is_dir())
            self.assertFalse((Path(args.destination) / "runtime").exists())
            self.assertFalse((Path(args.destination) / "installed.json").exists())

    def test_node_target_mismatch_stops_before_npm(self):
        for observed in [["v22.23.1", "linux", "x64"], ["v22.23.2", "darwin", "x64"],
                         ["v22.23.2", "linux", "arm64"], []]:
            args, _ = self.fixture("activepieces")
            with self.subTest(observed=observed), self.assertRaises(ValueError):
                self.scripted(args, node=observed)
            self.assertEqual([x["label"] for x in self.calls], ["node-runtime"])
            self.assertFalse((Path(args.destination) / "package.json").exists())

    def test_npm_target_mismatch_stops_before_cache_or_ci(self):
        for version in ["10.9.7", "11.12.1", "malformed output"]:
            args, _ = self.fixture("activepieces")
            with self.subTest(version=version), self.assertRaises(ValueError):
                self.scripted(args, npm=version)
            self.assertEqual([x["label"] for x in self.calls], ["node-runtime", "npm-runtime"])
            self.assertFalse((Path(args.destination) / "node_modules").exists())
            self.assertFalse((Path(args.destination) / "installed.json").exists())

    def test_absent_selected_runtime_refuses_before_destination(self):
        for family, key in [("services", "python"), ("agno", "python"),
                            ("activepieces", "node"), ("activepieces", "npm_cli")]:
            args, _ = self.fixture(family)
            setattr(args, key, None)
            with self.subTest(family=family, key=key):
                self.before_effects(args)

    def test_all_three_linux_families_publish_only_after_existing_validator(self):
        for family in ["agno", "services", "activepieces"]:
            args, _ = self.fixture(family, configured=True)
            result = self.scripted(args)
            self.assertEqual(result["runtime_target"], TARGET)
            self.assertEqual(result["scope"], "observed_software_installation_only")
            self.assertEqual(self.calls[-1]["label"], "check-config")
            self.assertEqual(result["command"][-3:], ["serve", "--config", str(Path(args.destination) / "configuration/selected.json")])
            self.assertEqual(json.loads((Path(args.destination) / "ready/installed.json").read_bytes()), result)
            self.assertFalse((Path(args.destination) / "installed.json").exists())
            self.assertNotIn("IGNORED_PRIVATE_VALIDATOR_OUTPUT", json.dumps(result))

    def test_legacy_darwin_policy_is_preserved_and_does_not_admit_linux(self):
        for family in ["services", "activepieces"]:
            args, _ = self.fixture(family, schema=1, target=None)
            with self.assertRaises(ValueError):
                self.scripted(args, python=["fixture", [3, 11], "linux", "x86_64", "cpython"])
            self.assertEqual(len(self.calls), 1)
        for family, observed in [("services", ["fixture", [3, 11], "darwin", "arm64", "cpython"]),
                                 ("activepieces", ["v22.19.0", "darwin", "arm64"]),
                                 ("activepieces", ["v24.19.0", "darwin", "arm64"])]:
            args, _ = self.fixture(family, schema=1, target=None)
            result = self.scripted(args, python=observed, node=observed)
            self.assertNotIn("runtime_target", result)
            self.assertNotIn("npm-runtime", [x["label"] for x in self.calls])

    def test_cancelled_linux_preflight_or_validator_does_not_publish(self):
        for family, step in [("services", "python-runtime"), ("activepieces", "npm-runtime"),
                             ("services", "check-config"), ("activepieces", "check-config")]:
            args, _ = self.fixture(family, configured=True)
            with self.subTest(family=family, step=step), self.assertRaises(KeyboardInterrupt):
                self.scripted(args, fail=step)
            self.assertEqual(self.calls[-1]["label"], step)
            self.assertFalse((Path(args.destination) / "ready").exists())
            self.assertFalse((Path(args.destination) / "installed.json").exists())
            with patch.object(MODULE, "run") as child, self.assertRaises(ValueError):
                MODULE.install(args)
            child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
