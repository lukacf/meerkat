"""Adversarial wheel tests. These fixtures do not prove native activation."""
from __future__ import annotations

import argparse
import base64
import copy
import csv
import importlib.util
import io
import json
from pathlib import Path
import stat
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("python_installer", Path(__file__).with_name("install-library-consumer.py"))
EXTENSIONS = [".cpython-311-darwin.so", ".abi3.so", ".so"]
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


def wheel(package="outside_server", version="1.0", extra=None, mutate_record=None, tags="py3-none-any"):
    dist = package + "-" + version + ".dist-info"
    files = {package + "/__init__.py": b"# inert fixture\n",
             package + "/__main__.py": b"raise RuntimeError('installation must not execute this')\n",
             dist + "/METADATA": ("Metadata-Version: 2.1\nName: " + package + "\nVersion: " + version + "\n").encode(),
             dist + "/WHEEL": ("Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: " + tags + "\n").encode()}
    files.update(extra or {})
    rows = [[path, "sha256=" + base64.urlsafe_b64encode(M.hashlib.sha256(raw).digest()).rstrip(b"=").decode(), str(len(raw))]
            for path, raw in files.items()]
    rows.append([dist + "/RECORD", "", ""])
    if mutate_record:
        mutate_record(rows)
    output = io.StringIO(newline="")
    csv.writer(output).writerows(rows)
    files[dist + "/RECORD"] = output.getvalue().encode()
    return package + "-" + version + "-" + tags + ".whl", archive(files), files


def archive(files, extra=()):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as target:
        for path, raw in files.items():
            target.writestr(path, raw)
        for entry, raw in extra:
            target.writestr(entry, raw)
    return output.getvalue()


class SelectedPythonTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="toolkit-selected-python-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.filename, raw, self.payload = wheel()
        self.files = {"artifact.whl": raw, "package.json": b"{}\n", "activation.json": b"{}\n"}
        self.plan = {"schema_version": 1, "component": "server", "manifest_digest": "d" * 64,
                     "artifact_filename": self.filename, "module": "outside_server.__main__",
                     "entrypoint": {"executable": "outside_server/__main__.py", "arguments": ["literal ${HOME}"]},
                     "mcp_name": "outside/server"}
        self.bind()
        self.calls = []

    def bind(self):
        rows = []
        for path, raw in self.files.items():
            if path == "artifact.whl" or path.endswith(".whl") and path.startswith("closure/"):
                filename = self.filename if path == "artifact.whl" else path.removeprefix("closure/")
                pieces = filename.split("-")
                rows.append(M.name(pieces[0]) + "==" + pieces[1] + " --hash=sha256:" + M.sha(raw))
        self.files["closure/requirements"] = ("\n".join(rows) + "\n").encode()
        self.plan["artifact_digest"] = M.sha(self.files["artifact.whl"])
        self.plan["closure"] = {path.removeprefix("closure/"): M.sha(raw) for path, raw in self.files.items()
                                if path.startswith("closure/")}
        for path, raw in self.files.items():
            output = self.bundle / path
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(raw)
        self.args = argparse.Namespace(selected_python=str(self.bundle), destination=str(self.root / "new"),
                                       python=sys.executable, validator=sys.executable, component="server",
                                       selection_sha256=M.sha(self.files["declaration.json" if "document_kind" in self.plan else "activation.json"]))
        if "document_kind" in self.plan:
            self.args.document_kind = "local_software_declaration"

    def software(self):
        # Metadata validation is controlled; preserve the real artifact closure.
        self.plan["document_kind"] = "local_software_declaration"
        self.plan["declaration_digest"] = self.plan.pop("manifest_digest")
        self.files["declaration.json"] = self.files.pop("activation.json")
        (self.bundle / "activation.json").unlink()
        self.bind()

    def inspect(self, raw=None):
        return M.selected_wheel(self.filename, self.files["artifact.whl"] if raw is None else raw, {"json", "sys"})

    def fake_run(self, argv, cwd, env, label, timeout=900):
        self.calls.append(label)
        self.assertNotIn("HOME", env)
        self.assertNotIn("CODEX_HOME", env)
        if label == "validate-selection":
            software = "document_kind" in self.plan
            filename = "declaration.json" if software else "activation.json"
            flag = "--declaration" if software else "--selection"
            self.assertEqual(argv[argv.index(flag) + 1], str(cwd / "inputs" / filename))
            self.assertNotIn("--selection" if software else "--declaration", argv)
            self.assertEqual((cwd / "inputs" / filename).read_bytes(), self.files[filename])
            return M.encoded(self.plan)
        if label == "python-runtime":
            self.assertIn("-S", argv)
            return M.encoded({"runtime": ["fixture", [3, 11], "darwin", "arm64", "cpython"], "stdlib": ["json", "sys"], "extensions": EXTENSIONS})
        self.fail("an invalid wheel reached effectful step " + label)

    def test_software_declaration_install_emits_exact_receipt_with_controlled_runtime(self):
        self.software()
        closure = M.selected_python_closure(self.files, self.plan, {"json", "sys"}, EXTENSIONS)
        def controlled(argv, cwd, env, label, timeout=900):
            if label in ("validate-selection", "python-runtime"):
                return self.fake_run(argv, cwd, env, label, timeout)
            self.calls.append(label)
            venv = cwd / "runtime"
            site = venv / "lib/python3.11/site-packages"
            if label == "create-venv":
                (venv / "bin").mkdir(parents=True)
                (venv / "bin/python").write_bytes(b"inert interpreter fixture; never executed")
                site.mkdir(parents=True)
            elif label in ("bootstrap-python", "installed-python"):
                rows = [["pip", "fixture"]]
                if label == "installed-python":
                    rows += [[name, version] for name, version in closure["versions"].items()]
                return M.encoded({"site": str(site), "distributions": rows})
            elif label == "pip-install":
                for wheel in closure["wheels"]:
                    for path, raw in wheel["files"].items():
                        target = site / path
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(raw)
                    for name, raw in (("RECORD", b"fixture pip generated record"), ("INSTALLER", b"pip\n"), ("REQUESTED", b"")):
                        (site / wheel["dist_info"] / name).write_bytes(raw)
            elif label != "pip-check":
                self.fail("unexpected controlled runtime call " + label)
            return b""
        with patch.object(M, "run", side_effect=controlled):
            result = M.install_selected_python(self.args)
        self.assertEqual(result["document_kind"], "local_software_declaration")
        self.assertEqual(result["declaration_digest"], "d" * 64)
        self.assertNotIn("manifest_digest", result)
        self.assertIn("declaration.json", result["inputs"])
        self.assertNotIn("activation.json", result["inputs"])
        self.assertEqual(json.loads((Path(self.args.destination) / "ready/installed.json").read_bytes()), result)
        self.assertGreater(result["verification"]["verified_payload_files"], 0)
        self.assertEqual(self.calls[-1], "installed-python")

    def test_software_dual_identity_refuses_before_runtime_or_pip(self):
        self.software()
        self.plan["manifest_digest"] = "d" * 64
        with patch.object(M, "run", side_effect=self.fake_run), self.assertRaises(M.InstallError):
            M.install_selected_python(self.args)
        self.assertEqual(self.calls, ["validate-selection"])
        self.assertFalse((Path(self.args.destination) / "ready").exists())

    def test_valid_closure_keeps_module_and_literal_argv(self):
        closure = M.selected_python_closure(self.files, self.plan, {"json"}, EXTENSIONS)
        self.assertEqual(closure["module"], "outside_server.__main__")
        self.assertEqual(closure["versions"], {"outside-server": "1.0"})
        self.assertEqual(self.plan["entrypoint"]["arguments"], ["literal ${HOME}"])
        self.assertEqual(closure["wheels"][0]["files"], self.payload)

    def test_primary_payload_is_required_not_a_dependency_entrypoint(self):
        filename, raw, _ = wheel("another_server")
        self.files["closure/" + filename] = raw
        self.bind()
        self.plan.update(module="another_server.__main__", entrypoint={"executable": "another_server/__main__.py", "arguments": []})
        with self.assertRaises(ValueError):
            M.selected_python_closure(self.files, self.plan, set(), EXTENSIONS)

    def test_startup_hooks_and_stdlib_bootstrap_shadowing_are_refused(self):
        for path in ("sneaky.pth", "pkg/sitecustomize.py", "usercustomize.py", "json/__init__.py",
                     "pip/extra.py", "_distutils_hack/extra.py", "setuptools/extra.py", "SiteCustomize.py",
                     "USERcustomize.py", "sitecustomize/__init__.py", "USERCUSTOMIZE/__init__.py", "PIP/extra.py"):
            _, raw, _ = wheel(extra={path: b"raise RuntimeError('untrusted')\n"})
            with self.subTest(path=path), self.assertRaises(ValueError):
                self.inspect(raw)

    def test_unsupported_compression_is_a_controlled_refusal(self):
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_BZIP2) as target:
            for path, raw in self.payload.items():
                target.writestr(path, raw)
        with self.assertRaises(M.InstallError):
            self.inspect(output.getvalue())

    def test_data_relocation_and_installer_owned_metadata_are_refused(self):
        for path in ("outside_server-1.0.data/scripts/escape", "outside_server-1.0.dist-info/INSTALLER",
                     "outside_server-1.0.dist-info/REQUESTED", "outside_server-1.0.dist-info/direct_url.json"):
            _, raw, _ = wheel(extra={path: b"not installer evidence"})
            with self.subTest(path=path), self.assertRaises(ValueError):
                self.inspect(raw)

    def test_record_missing_changed_duplicate_and_weak_hash_are_refused(self):
        def missing(rows): rows.pop(0)
        def changed(rows): rows[0][1] = "sha256=incorrect"
        def duplicate(rows): rows.append(rows[0])
        def weak(rows): rows[0][1] = "md5=incorrect"
        def self_hash(rows): rows[-1][1] = "sha256=incorrect"
        for mutation in (missing, changed, duplicate, weak, self_hash):
            _, raw, _ = wheel(mutate_record=mutation)
            with self.subTest(mutation=mutation.__name__), self.assertRaises(ValueError):
                self.inspect(raw)

    def test_wheel_name_version_tags_and_distribution_cannot_differ(self):
        for filename in ("another-1.0-py3-none-any.whl", "outside_server-2.0-py3-none-any.whl",
                         "outside_server-1.0-py2-none-any.whl", "artifact.whl", "../" + self.filename):
            with self.subTest(filename=filename), self.assertRaises(ValueError):
                M.selected_wheel(filename, self.files["artifact.whl"], set())

    def test_compressed_wheel_tags_and_boolean_case_preserve_exact_payload(self):
        tags = "cp311-cp311-manylinux_2_17_x86_64.manylinux2014_x86_64"
        expanded = "cp311-cp311-manylinux_2_17_x86_64\nTag: cp311-cp311-manylinux2014_x86_64"
        for tag_headers in (tags, expanded):
            for purelib in ("true", "True", "FALSE"):
                metadata = ("Wheel-Version: 1.0\nRoot-Is-Purelib: " + purelib
                            + "\nTag: " + tag_headers + "\n").encode()
                filename, raw, payload = wheel(tags=tags, extra={"outside_server-1.0.dist-info/WHEEL": metadata})
                with self.subTest(tags=tag_headers, purelib=purelib):
                    result = M.selected_wheel(filename, raw, set())
                    self.assertEqual(result["sha256"], M.sha(raw))
                    self.assertEqual(result["files"], payload)

    def test_wheel_tag_and_boolean_normalization_keeps_refusals(self):
        tags = "cp311-cp311-manylinux_2_17_x86_64.manylinux2014_x86_64"
        valid = "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: " + tags + "\n"
        malformed = (valid.replace("Root-Is-Purelib: true", "Root-Is-Purelib: yes"),
                     valid.replace("Root-Is-Purelib: true\n", ""),
                     valid + "Root-Is-Purelib: False\n",
                     valid.replace(tags, "cp311-cp311-manylinux_2_17_x86_64"),
                     valid.replace(tags, tags + ".win_amd64"),
                     valid.replace(tags, tags.replace(".manylinux", "..manylinux")),
                     valid.replace(tags, tags + "-extra"),
                     valid.replace("Tag: " + tags + "\n", ""))
        for metadata in malformed:
            filename, raw, _ = wheel(tags=tags, extra={"outside_server-1.0.dist-info/WHEEL": metadata.encode()})
            with self.subTest(metadata=metadata), self.assertRaises(M.InstallError):
                M.selected_wheel(filename, raw, set())
        # Equivalent metadata still must satisfy the unchanged RECORD hashes.
        filename, _, payload = wheel(tags=tags)
        payload["outside_server-1.0.dist-info/WHEEL"] = valid.replace("true", "True").encode()
        with self.assertRaisesRegex(M.InstallError, "RECORD payload mismatch"):
            M.selected_wheel(filename, archive(payload), set())

    def test_zip_symlink_duplicate_escape_and_case_alias_are_refused(self):
        link = zipfile.ZipInfo("link")
        link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        cases = [[(link, b"/outside")], [("../outside", b"x")], [("outside_server/__init__.py", b"changed")],
                 [("Outside_Server/extra.py", b"x")], [("OUTSIDE_SERVER/", b"")], [("outside_server", b"file")]]
        for extra in cases:
            with self.subTest(paths=[str(item[0]) for item in extra]), self.assertRaises(ValueError):
                self.inspect(archive(self.payload, extra))

    def test_member_and_expansion_limits_apply_before_installation(self):
        with patch.object(M, "MAX_NPM_MEMBERS", 1), self.assertRaises(ValueError):
            self.inspect()
        with patch.object(M, "MAX_NPM_EXPANDED", 1), self.assertRaises(ValueError):
            self.inspect()
        with patch.object(M, "MAX_TOTAL", 1), self.assertRaises(ValueError):
            M.selected_python_closure(self.files, self.plan, set(), EXTENSIONS)

    def test_dependency_url_and_ambiguous_metadata_are_refused(self):
        metadata = self.payload["outside_server-1.0.dist-info/METADATA"]
        for addition in (b"Requires-Dist: thing @ https://example.invalid/wheel\n", b"Name: another\n", b"Version: 2\n"):
            _, raw, _ = wheel(extra={"outside_server-1.0.dist-info/METADATA": metadata + addition})
            with self.subTest(addition=addition), self.assertRaises(ValueError):
                self.inspect(raw)

    def test_missing_extra_duplicate_and_changed_pins_are_refused(self):
        original = self.files["closure/requirements"]
        for pins in (b"", original + original, original + b"extra==1 --hash=sha256:" + b"0" * 64 + b"\n",
                     original.replace(b"==1.0", b"==2.0")):
            self.files["closure/requirements"] = pins
            with self.subTest(pins=pins), self.assertRaises(ValueError):
                M.selected_python_closure(self.files, self.plan, set(), EXTENSIONS)

    def test_cross_wheel_namespace_payload_and_script_collisions_are_refused(self):
        script = {"outside_server-1.0.dist-info/entry_points.txt": b"[console_scripts]\nrunner = outside_server:main\n"}
        self.filename, self.files["artifact.whl"], _ = wheel(extra=script)
        for extra in ({"outside_server/another.py": b"x"}, {"outside_server/__init__.py": b"x"},
                      {"another-1.0.dist-info/entry_points.txt": b"[console_scripts]\nRUNNER = another:main\n"}):
            filename, raw, _ = wheel("another", extra=extra)
            self.files["closure/" + filename] = raw
            self.bind()
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                M.selected_python_closure(self.files, self.plan, set(), EXTENSIONS)

    def test_entrypoint_cannot_be_init_missing_escaping_or_inconsistent_module(self):
        for path, module in (("outside_server/__init__.py", "outside_server.__init__"),
                             ("outside_server/missing.py", "outside_server.missing"),
                             ("../escape.py", "escape"), ("outside_server/__main__.py", "elsewhere")):
            plan = copy.deepcopy(self.plan)
            plan.update(module=module, entrypoint={"executable": path, "arguments": []})
            with self.subTest(path=path, module=module), self.assertRaises(ValueError):
                M.selected_python_closure(self.files, plan, set(), EXTENSIONS)

    def test_ambiguous_parent_module_is_refused(self):
        self.filename, self.files["artifact.whl"], _ = wheel(extra={"outside_server.py": b"x"})
        self.bind()
        with self.assertRaises(ValueError):
            M.selected_python_closure(self.files, self.plan, set(), EXTENSIONS)

    def test_binary_or_package_import_cannot_shadow_selected_source(self):
        for path in ("outside_server/__init__.cpython-311-darwin.so", "outside_server/__main__.abi3.so",
                     "outside_server/__main__/__init__.py", "outside_server/__main__/__init__.so"):
            self.filename, self.files["artifact.whl"], _ = wheel(extra={path: b"alternate import target"})
            self.bind()
            with self.subTest(path=path), self.assertRaises(ValueError):
                M.selected_python_closure(self.files, self.plan, set(), EXTENSIONS)

    def test_retained_bytecode_cannot_substitute_for_source(self):
        for path in ("outside_server/__main__.pyc", "outside_server/__pycache__/__main__.cpython-311.pyc"):
            _, raw, _ = wheel(extra={path: b"attacker supplied bytecode"})
            with self.subTest(path=path), self.assertRaises(ValueError):
                self.inspect(raw)

    def test_forged_startup_code_never_reaches_venv_or_pip(self):
        self.filename, self.files["artifact.whl"], _ = wheel(extra={"exploit.pth": b"import malicious\n"})
        self.bind()
        with patch.object(M, "run", side_effect=self.fake_run), self.assertRaises(ValueError):
            M.install_selected_python(self.args)
        self.assertEqual(self.calls, ["validate-selection", "python-runtime"])
        self.assertFalse((Path(self.args.destination) / "runtime").exists())
        self.assertFalse((Path(self.args.destination) / "ready").exists())

    def test_input_mutation_is_refused_before_any_process(self):
        (self.bundle / "activation.json").write_bytes(b"changed")
        with patch.object(M, "run") as child, self.assertRaises(ValueError):
            M.install_selected_python(self.args)
        child.assert_not_called()
        self.assertFalse(Path(self.args.destination).exists())

    def test_wheel_mutation_is_refused_before_runtime(self):
        (self.bundle / "artifact.whl").write_bytes(b"changed")
        with patch.object(M, "run", side_effect=self.fake_run), self.assertRaises(ValueError):
            M.install_selected_python(self.args)
        self.assertEqual(self.calls, ["validate-selection"])

    def test_bootstrap_script_name_collision_is_refused(self):
        with self.assertRaises(ValueError):
            M.checked_paths(["bin/python", "bin/PYTHON"])

    def test_optional_entrypoint_extras_remain_inert_metadata(self):
        _, raw, _ = wheel(extra={"outside_server-1.0.dist-info/entry_points.txt":
                                 b"[console_scripts]\noptional = outside_server:main [cli, more]\n"})
        self.assertEqual(self.inspect(raw)["scripts"], {"optional": "outside_server:main [cli, more]"})

    def test_bootstrap_data_filename_with_parentheses_is_literal(self):
        root = self.root / "bootstrap"
        root.mkdir()
        (root / "script (dev).tmpl").write_bytes(b"trusted bootstrap bytes")
        self.assertEqual(M.snapshot_tree(root), {"script (dev).tmpl": ("file", M.sha(b"trusted bootstrap bytes"))})
        with self.assertRaises(ValueError):
            M.relative("../script (dev).tmpl")

    def test_existing_directory_is_preserved(self):
        destination = Path(self.args.destination)
        destination.mkdir()
        (destination / "owned").write_bytes(b"untouched")
        with patch.object(M, "run") as child, self.assertRaises(ValueError):
            M.install_selected_python(self.args)
        child.assert_not_called()
        self.assertEqual((destination / "owned").read_bytes(), b"untouched")


if __name__ == "__main__":
    unittest.main()
