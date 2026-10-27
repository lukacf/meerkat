"""The ordinary starter composes real preparation, with a controlled installer."""
import builtins
import importlib.util
import io
import json
import os
import sys
from pathlib import Path
import tarfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import test_bootstrap_host as fixture


SOURCE = fixture.SOURCE
spec = importlib.util.spec_from_file_location("toolkit_starter", SOURCE / "start.py")
starter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(starter)


class StarterTests(unittest.TestCase):
    publish_release = fixture.BootstrapTests.publish_release
    installer = fixture.BootstrapTests.installer

    def setUp(self):
        fixture.BootstrapTests.setUp(self)
        self.files["setup/bootstrap_host.py"] = ((SOURCE / "bootstrap_host.py").read_bytes(), 0o644)
        self.files["setup/acquired_inspection.py"] = ((SOURCE / "acquired_inspection.py").read_bytes(), 0o644)
        self.files["setup/public_github.py"] = ((SOURCE / "public_github.py").read_bytes(), 0o644)
        self.files["setup/local_session.py"] = ((SOURCE / "local_session.py").read_bytes(), 0o644)
        self.files["selected/bin/toolkit-package-inspect"] = (b"# not executed\n", 0o755)
        self.publish_release()
        self.downloads = self.base / "downloads"
        self.downloads.mkdir()
        self.name = self.root_name + ".tar.gz"
        self.selected = self.downloads / self.name
        self.selected.write_bytes(self.archive.read_bytes())
        self.selection = {"schema_version": 1, "assets": [{"upload_name": self.name,
            "sha256": fixture.sha(self.selected.read_bytes()), "bytes": self.selected.stat().st_size}]}
        self.destination = self.base / "new-toolkit"
        self.options = SimpleNamespace(archive_dir=str(self.downloads), destination=str(self.destination),
            allow_public_pypi=False, pip_zipapp=None, node=None, npm_cli=None, timeout_seconds=30)
        self.addCleanup(patch.stopall)
        patch.object(starter, "distribution", return_value=self.selection).start()
        patch.object(starter, "current_target", return_value="macos-arm64").start()

    def refresh(self):
        self.selected.write_bytes(self.archive.read_bytes())
        self.selection["assets"][0].update(sha256=fixture.sha(self.selected.read_bytes()),
                                           bytes=self.selected.stat().st_size)

    def test_real_archived_bootstrap_with_derived_paths_and_no_environment_leak(self):
        with patch.dict(os.environ, {"TOOLKIT_STARTER_TEST_SECRET": "private"}):
            result = starter.start(self.options)
        self.assertEqual(result["status"], "prepared")
        self.assertEqual(result["scope"], "offline_setup_client_and_host_configuration_only")
        self.assertEqual(result["families"], ["agno", "services"])
        self.assertFalse(result["public_pypi_enabled"])
        self.assertTrue(Path(result["configuration"]).is_file())
        self.assertFalse((self.destination / "session").exists())
        entered = json.loads((self.destination / "client/entered.json").read_text())
        self.assertNotIn("TOOLKIT_STARTER_TEST_SECRET", entered["environment"])
        receipt = json.loads((self.destination / "config/prepared.json").read_text())
        self.assertFalse(receipt["public_npm_enabled"])
        self.assertFalse(receipt["node_search_enabled"])
        with self.assertRaisesRegex(Exception, "exists"):
            starter.start(self.options)
        self.assertTrue(Path(result["configuration"]).is_file())

    def recipe_owner_fixture(self):
        # Controlled initialization protocol only. Durable SQLite semantics belong
        # to the actual local-recipes tests, not this preparation transport fixture.
        self.files["selected/bin/toolkit-local-recipes"] = (("#!" + sys.executable + "\n" +
            "import json,pathlib,sys\n" +
            "assert sys.argv[1] == 'init'\n" +
            "database=pathlib.Path(sys.argv[sys.argv.index('--database')+1])\n" +
            "database.write_bytes(b'inert store transport fixture')\n" +
            "database.with_suffix('.argv').write_text(json.dumps(sys.argv[1:]))\n" +
            "print(json.dumps({'schema_version':1,'scope':'retained_recipe_metadata','status':'initialized'}))\n"
        ).encode(), 0o755)
        self.publish_release(); self.refresh()

    def test_recipe_initialization_joins_pinned_owner_before_bootstrap(self):
        self.recipe_owner_fixture()
        database = self.base / "recipes.sqlite"
        self.options.recipes_database = str(database)
        self.options.recipes_realm = "selected-realm"
        self.options.recipes_owner = "selected-owner"
        self.options.recipes_scope = "retain"
        self.options.allow_remote_recipes = True
        result = starter.start(self.options)
        self.assertEqual(json.loads(database.with_suffix(".argv").read_bytes()),
            ["init", "--database", str(database), "--realm", "selected-realm", "--owner", "selected-owner"])
        profile = json.loads((self.destination / "config/local-recipes.json").read_bytes())
        self.assertEqual(profile["database"], str(database))
        self.assertEqual(profile["realm"], "selected-realm")
        self.assertEqual(profile["owner"], "selected-owner")
        self.assertEqual(profile["tool"]["sha256"], fixture.sha(self.files["selected/bin/toolkit-local-recipes"][0]))
        command = json.loads(Path(result["configuration"]).read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-remote-recipes", command)
        self.assertIn("--local-recipes-profile-sha256", command)
        self.assertFalse((self.destination / "session").exists())

    def test_partial_recipe_scope_refuses_before_any_owner_or_destination(self):
        for values in ({"recipes_realm":"realm"}, {"allow_remote_recipes":True},
                       {"recipes_database":str(self.base / "recipes.sqlite"), "recipes_realm":"realm",
                        "recipes_owner":"owner", "recipes_scope":"read", "allow_remote_recipes":True}):
            with self.subTest(values=values), patch.object(starter.runner,"run_owner") as run:
                with self.assertRaises(Exception):
                    starter.start(SimpleNamespace(**{**vars(self.options),**values}))
                run.assert_not_called()
                self.assertFalse(self.destination.exists())

    def test_uncertain_recipe_init_never_enters_bootstrap(self):
        self.recipe_owner_fixture()
        options = SimpleNamespace(**{**vars(self.options), "recipes_database":str(self.base / "recipes.sqlite"),
            "recipes_realm":"realm", "recipes_owner":"owner", "recipes_scope":"retain", "allow_remote_recipes":True})
        with patch.object(starter.runner,"run_owner",return_value={"status":"unconfirmed"}) as run:
            with self.assertRaisesRegex(Exception,"local_recipes_initialization_unconfirmed"):
                starter.start(options)
            self.assertEqual(run.call_count,1)
            self.assertEqual(run.call_args.args[0][1],"init")
        self.assertFalse((self.destination / "config").exists())
        self.assertFalse((self.destination / "client").exists())

    def test_bad_archive_never_creates_output_or_calls_owner(self):
        for alteration in ("hash", "missing", "symlink"):
            with self.subTest(alteration=alteration):
                self.selected.unlink(missing_ok=True)
                if alteration == "hash":
                    self.selected.write_bytes(b"wrong bytes")
                elif alteration == "symlink":
                    self.selected.symlink_to(self.archive)
                with patch.object(starter.runner, "run_owner") as run:
                    with self.assertRaises(Exception):
                        starter.start(self.options)
                    run.assert_not_called()
                self.assertFalse(self.destination.exists())

    def test_manifest_not_taken_from_download_directory(self):
        (self.downloads / "current-distribution.json").write_text("malicious local selection")
        (self.downloads / "release-manifest.json").write_text("malicious local manifest")
        self.selected.write_bytes(b"wrong bytes")
        with self.assertRaisesRegex(Exception, "selected_archive_changed"):
            starter.start(self.options)
        self.assertFalse(self.destination.exists())

    def test_unsafe_archive_shapes_refuse_before_output_or_execution(self):
        for name, kind in (("../escape", tarfile.REGTYPE), (self.root_name + "/link", tarfile.SYMTYPE),
                           (self.root_name + "/device", tarfile.FIFOTYPE),
                           (self.root_name + "/licenses/notice.txt", tarfile.REGTYPE),
                           (self.root_name + "/LICENSES/NOTICE.txt", tarfile.REGTYPE)):
            with self.subTest(name=name):
                extra = tarfile.TarInfo(name)
                extra.type, extra.mode = kind, 0o644
                extra.linkname = "/tmp"
                self.publish_release(extra)
                self.refresh()
                with patch.object(starter.runner, "run_owner") as run:
                    with self.assertRaises(Exception):
                        starter.start(self.options)
                    run.assert_not_called()
                self.assertFalse(self.destination.exists())

    def test_occupied_case_alias_and_overlapping_paths_preserve_user_data(self):
        occupied = self.base / "NEW-TOOLKIT"
        occupied.mkdir()
        (occupied / "mine").write_text("keep")
        with self.assertRaisesRegex(Exception, "exists"):
            starter.start(self.options)
        self.assertEqual((occupied / "mine").read_text(), "keep")
        self.options.destination = str(self.downloads / "nested")
        with self.assertRaisesRegex(Exception, "destination_overlaps_inputs"):
            starter.start(self.options)

    def test_selected_node_and_discovery_require_complete_explicit_choices(self):
        for values in ({"node": str(self.runtime)}, {"npm_cli": str(self.pip)},
                       {"allow_public_pypi": True}, {"pip_zipapp": str(self.pip)}):
            with self.subTest(values=values):
                options = SimpleNamespace(**{**vars(self.options), **values})
                with self.assertRaisesRegex(Exception, "required_together|explicit_choice"):
                    starter.start(options)
                self.assertFalse(self.destination.exists())

    def test_explicit_discovery_prepares_only_that_scope(self):
        self.options.allow_public_pypi = True
        self.options.pip_zipapp = str(self.pip)
        result = starter.start(self.options)
        self.assertTrue(result["public_pypi_enabled"])
        receipt = json.loads((self.destination / "config/prepared.json").read_text())
        self.assertTrue(receipt["public_pypi_enabled"])
        self.assertFalse(receipt["public_npm_enabled"])
        self.assertEqual(receipt["runtimes"]["pip"]["sha256"], fixture.sha(self.pip.read_bytes()))
        command = json.loads(Path(result["configuration"]).read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-acquired-inspection", command)
        self.assertNotIn("--node-acquisition-profile", command)

    def product(self):
        # Use the actual product -> starter -> archived bootstrap path. Native
        # resource verification is outside this fixture: its interpreter is the
        # test process, and the accepted bootstrap installer is controlled.
        spec = importlib.util.spec_from_file_location("product_prepare", SOURCE / "product/prepare.py")
        product = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(product)
        # This archive deliberately uses a synthetic macOS target. Admit that
        # fixture in this product module only, without changing global platform
        # functions or the real starter/bootstrap/preparation implementation.
        patch.object(product, "HOST_TARGETS", {
            (product.platform.system(), product.platform.machine()): "macos-arm64"
        }).start()
        def load_fixture_starter(code, namespace):
            builtins.exec(code, namespace)
            # The existing test interpreter need not be the bundled CPython
            # 3.11. Substitute only this fixture's target admission, preserving
            # the actual selected start/bootstrap/preparation functions.
            namespace["current_target"] = lambda: "macos-arm64"
        patch.object(product, "exec", side_effect=load_fixture_starter, create=True).start()
        root = self.base / "product"
        root.mkdir()
        (root / "starter").mkdir()
        for name in ("start.py", "bootstrap_host.py", "prepare_host.py"):
            (root / "starter" / name).write_bytes((SOURCE / name).read_bytes())
        raw = json.dumps(self.selection).encode()
        (root / "starter/release-manifest.json").write_bytes(raw)
        (root / "starter/current-distribution.json").write_text(json.dumps({
            "schema_version": 1, "manifest": "release-manifest.json", "manifest_sha256": fixture.sha(raw)}))
        (root / "runtime").mkdir()
        (root / "runtime/pip.pyz").write_bytes(self.pip.read_bytes())
        (root / "python").symlink_to(Path(sys.executable).resolve())
        (root / "node").write_bytes(self.runtime.read_bytes())
        (root / "node").chmod(0o755)
        (root / "npm.js").write_bytes(self.pip.read_bytes())
        (root / self.name).write_bytes(self.selected.read_bytes())
        manifest = {"target": "macos-arm64", "python": "python", "node": "node", "npm_cli": "npm.js",
                    "starter": "starter/start.py", "archive": self.name}
        patch.object(product, "verify_resources", return_value=(manifest, "a" * 64)).start()
        return product, root

    def npm_product(self):
        self.files["setup/node_acquisition.py"] = ((SOURCE / "node_acquisition.py").read_bytes(), 0o644)
        self.files["selected/bin/toolkit-node-plan"] = (b"# inert Node planner, never executed\n", 0o755)
        self.files["selected/provenance/selection.json"] = (json.dumps({
            "platform": "macos-arm64", "scope": "retained_selected_tools"}).encode(), 0o644)
        self.publish_release()
        self.refresh()
        product, root = self.product()
        group = {"node": "runtime/acquisition-node/bin/node",
                 "npm_cli": "runtime/acquisition-node/lib/node_modules/npm/bin/npm-cli.js",
                 "npm_archive": "runtime/npm-acquisition.tgz"}
        for name in ("node", "npm_cli"):
            path = root / group[name]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("inert distinct acquisition " + name).encode())
            path.chmod(0o755)
        with tarfile.open(root / group["npm_archive"], "w:gz") as archive:
            for name, raw in {"package.json": b'{"name":"npm","version":"12.2.0"}',
                              "bin/npm-cli.js": b"// inert archive entry\n"}.items():
                info = tarfile.TarInfo("package/" + name)
                info.size, info.mode = len(raw), 0o644
                archive.addfile(info, io.BytesIO(raw))
        product.verify_resources.return_value[0]["npm_acquisition"] = group
        return product, root, group

    def test_product_supplied_npm_group_reaches_actual_preparer_without_install_or_runtime_substitution(self):
        product, root, group = self.npm_product()
        with patch.dict(os.environ, {"NPM_PRODUCT_SECRET_TEST": "must not reach child"}):
            product.prepare(root, self.destination, threading.Event())
        output = self.destination / "config"
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertIs(receipt["public_npm_enabled"], True)
        self.assertIs(receipt["node_search_enabled"], True)
        profile = json.loads((output / "node-acquisition.json").read_bytes())
        self.assertEqual(profile["scope"], "trusted_npm_registry_acquisition")
        self.assertIs(profile["allow_acquisition"], True)
        self.assertEqual(profile["npm"], {"path": str(root / group["npm_archive"]),
            "sha256": fixture.sha((root / group["npm_archive"]).read_bytes())})
        self.assertEqual(profile["node_sha256"], fixture.sha((root / group["node"]).read_bytes()))
        self.assertNotEqual(profile["node_sha256"], receipt["runtimes"]["node"]["sha256"])
        args = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-node-search", args)
        self.assertEqual(args[args.index("--node") + 1], str(root / "node"))
        self.assertEqual(args[args.index("--npm-cli") + 1], str(root / "npm.js"))
        self.assertEqual(args[args.index("--selected-node") + 1], str(root / group["node"]))
        self.assertEqual(args[args.index("--selected-npm-cli") + 1], str(root / group["npm_cli"]))
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(set(selected["validators"]), {"python", "node"})
        entered = json.loads((self.destination / "client/entered.json").read_bytes())
        self.assertNotIn("NPM_PRODUCT_SECRET_TEST", entered["environment"])
        self.assertFalse((self.destination / "session").exists(), "no package acquisition or install started")
        self.assertFalse((self.destination / "local-session").exists())

        checked_receipt = product.checked_receipt
        for key in ("public_npm_enabled", "node_search_enabled"):
            for supplied in (None, False):
                destination = self.base / ("refused-" + key + "-" + str(supplied))
                def unavailable(host, path, expected):
                    value = checked_receipt(host, path, expected)
                    if path.name == "prepared.json":
                        if supplied is None: value.pop(key, None)
                        else: value[key] = supplied
                    return value
                with self.subTest(key=key, supplied=supplied), patch.object(
                        product, "checked_receipt", side_effect=unavailable):
                    with self.assertRaisesRegex(product.ResourceError, "npm_acquisition_scope_mismatch"):
                        product.prepare(root, destination, threading.Event())
                    self.assertFalse((destination / "product-mcp.json").exists())

    def test_product_node_build_profile_reaches_archived_preparer_as_exact_snapshot(self):
        product, root, _ = self.npm_product()
        # Profile meaning is checked by the selected installer at build review.
        # This control exercises the actual product/start/bootstrap/profile path
        # with a controlled setup-client installer and no selected build.
        raw = b'{ "scope": "transport_fixture", "external_tool": "/host/selected/tool" }\n'
        path = root / "node-build-profile.json"
        path.write_bytes(raw)
        manifest = product.verify_resources.return_value[0]
        manifest["entries"] = [{"path": path.name, "kind": "file", "mode": 0o644,
                                "size": len(raw), "sha256": fixture.sha(raw)}]
        product.prepare(root, self.destination, threading.Event())
        output = self.destination / "config"
        receipt = json.loads((output / "prepared.json").read_bytes())
        profile = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual((output / path.name).read_bytes(), raw)
        self.assertEqual(profile["node_build"], {"path": str(output / path.name), "sha256": fixture.sha(raw)})
        self.assertEqual(receipt["files"][path.name], fixture.sha(raw))
        self.assertEqual(receipt["runtimes"]["node_build_profile"]["sha256"], fixture.sha(raw))
        self.assertIn("node", profile["validators"])
        self.assertTrue((self.destination / "client/verify-entered").is_file())
        self.assertFalse((self.destination / "session").exists())
        self.assertFalse((self.destination / "local-session").exists())
        self.assertTrue((self.destination / "product-mcp.json").is_file())
        path.write_bytes(b"source changed after preparation")
        self.assertEqual((output / path.name).read_bytes(), raw)

    def test_product_node_build_changed_inventory_refuses_before_starter(self):
        product, root, _ = self.npm_product()
        path = root / "node-build-profile.json"
        path.write_bytes(b'{}\n')
        product.verify_resources.return_value[0]["entries"] = [{"path": path.name,
            "kind": "file", "mode": 0o644, "size": path.stat().st_size, "sha256": "0" * 64}]
        with self.assertRaisesRegex(product.ResourceError, "bundled_node_build_profile_unavailable"):
            product.prepare(root, self.destination, threading.Event())
        self.assertFalse(self.destination.exists())

    def test_product_npm_group_null_partial_escape_missing_or_linked_refuses_before_starter(self):
        product, root, group = self.npm_product()
        manifest = product.verify_resources.return_value[0]
        missing = root / group["npm_archive"]
        raw = missing.read_bytes()
        variants = [None, {"node": group["node"]}, {**group, "fallback": True},
                    {**group, "npm_archive": "../outside"}, {**group, "npm_archive": "absent.tgz"}]
        for value in variants:
            with self.subTest(value=value), patch.object(product, "ModuleType") as load:
                manifest["npm_acquisition"] = value
                with self.assertRaises(product.ResourceError):
                    product.prepare(root, self.destination, threading.Event())
                load.assert_not_called()
                self.assertFalse(self.destination.exists())
        manifest["npm_acquisition"] = group
        missing.unlink(); missing.symlink_to(root / "npm.js")
        with patch.object(product, "ModuleType") as load:
            with self.assertRaisesRegex(product.ResourceError, "bundled_npm_acquisition_unavailable"):
                product.prepare(root, self.destination, threading.Event())
            load.assert_not_called()
        missing.unlink(); missing.write_bytes(raw)

    def test_product_wrong_npm_archive_refuses_before_client_install_and_retains_preparation(self):
        product, root, group = self.npm_product()
        (root / group["npm_archive"]).write_bytes(b"not the declared npm distribution")
        # This fixture deliberately bypasses immutable closure verification so
        # the actual archived preparer's independent npm owner must refuse.
        with self.assertRaisesRegex(Exception, "owner_refused"):
            product.prepare(root, self.destination, threading.Event())
        self.assertFalse((self.destination / "product-mcp.json").exists())
        self.assertFalse((self.destination / "config").exists())
        self.assertFalse((self.destination / "client").exists())
        self.assertTrue((self.destination / "distribution").is_dir(), "partial preparation is retained")
        self.assertFalse((self.destination / "session").exists())

    def test_starter_npm_scope_requires_complete_explicit_pair_and_archive_before_owner(self):
        for values in ({"allow_public_npm": True}, {"enable_node_search": True}, {"selected_node": ""},
                       {"allow_public_npm": 1}, {"enable_node_search": "yes"},
                       {"selected_node": str(self.runtime), "selected_npm_cli": str(self.pip),
                        "npm_archive": str(self.pip)}):
            with self.subTest(values=values), patch.object(starter.runner, "run_owner") as run:
                options = SimpleNamespace(**{**vars(self.options), **values})
                with self.assertRaisesRegex(Exception, "complete_explicit_npm_resources_and_scope_required"):
                    starter.start(options)
                run.assert_not_called()
                self.assertFalse(self.destination.exists())

    def test_product_default_prepares_acquisition_and_inspection_without_manual_paths(self):
        product, root = self.product()
        checked_receipt = product.checked_receipt
        def legacy_receipt(host, path, expected):
            value = checked_receipt(host, path, expected)
            if path.name == "prepared.json":
                value.pop("public_npm_enabled", None)
                value.pop("node_search_enabled", None)
            return value
        # A supported older resource receipt can omit capabilities it never
        # enabled. This controlled projection changes no retained owner files.
        with patch.object(product, "checked_receipt", side_effect=legacy_receipt):
            config = product.prepare(root, self.destination, threading.Event())
        args = config["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(Path(args[6]).name, "local_session.py")
        self.assertEqual(args[args.index("--state-root") + 1], str(self.destination / "local-session"))
        prepared_args = json.loads((self.destination / "config/mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--python-acquisition-profile", prepared_args)
        self.assertIn("--allow-acquired-inspection", prepared_args)
        selected = json.loads((self.destination / "config/selected-install.json").read_bytes())
        self.assertIn("package_inspector", selected)
        self.assertNotIn("--node-acquisition-profile", prepared_args)
        receipt = json.loads((self.destination / "config/prepared.json").read_bytes())
        profile = json.loads((self.destination / "config/python-acquisition.json").read_bytes())
        self.assertEqual(profile["scope"], "trusted_pypi_wheel_acquisition")
        self.assertEqual(profile["pip"]["path"], str(root / "runtime/pip.pyz"))
        self.assertEqual(profile["pip"]["sha256"], fixture.sha(self.pip.read_bytes()))
        self.assertTrue(receipt["public_pypi_enabled"])
        self.assertFalse(receipt["public_npm_enabled"])
        self.assertFalse((self.destination / "session").exists(), "preparation must not start an acquisition")
        self.assertFalse((self.destination / "local-session").exists())
        self.assertEqual(json.loads((self.destination / "product-mcp.json").read_bytes()), config)
        retained = (self.destination / "product-prepared.json").read_bytes()
        with self.assertRaisesRegex(product.ResourceError, "fresh_disjoint_destination_required"):
            product.prepare(root, self.destination, threading.Event())
        self.assertEqual((self.destination / "product-prepared.json").read_bytes(), retained)

    def test_native_product_realm_selects_fixed_recipe_owner_without_agent_paths(self):
        self.recipe_owner_fixture()
        product, root = self.product()
        state = self.base / "state"
        attempt = state / ".meerkat-connect/work/setup-one"
        attempt.mkdir(parents=True)
        destination = attempt / "prepared"
        config = product.prepare(root,destination,threading.Event(),realm_state_root=str(state),realm="work")
        profile = json.loads((destination / "config/local-recipes.json").read_bytes())
        database = state / ".meerkat-connect/work/recipes.sqlite"
        self.assertEqual((profile["database"],profile["realm"],profile["owner"]),
            (str(database),"work","meerkat-connect"))
        self.assertEqual(profile["scope"],"retain_and_disclose_nonsecret_local_recipe_metadata")
        command = json.loads((destination / "config/mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-remote-recipes",command)
        self.assertIn("--local-recipes-profile-sha256",command)
        self.assertEqual(Path(config["mcpServers"]["toolkit-setup"]["args"][6]).name,"local_session.py")
        self.assertFalse((destination / "local-session").exists())
        self.assertFalse((destination / "session").exists())
        self.assertEqual(json.loads(database.with_suffix(".argv").read_bytes())[-4:],
            ["--realm","work","--owner","meerkat-connect"])

    def test_product_new_attempt_has_distinct_descriptor_and_retains_old_attempt(self):
        product, root = self.product()
        first = product.prepare(root, self.destination, threading.Event())
        old = {name: (self.destination / name).read_bytes()
               for name in ("product-prepared.json", "product-mcp.json", "config/mcp.json")}
        state = self.destination / "local-session"
        state.mkdir(mode=0o700)
        (state / "session.json").write_bytes(b"retain interrupted attempt\n")
        second_destination = self.base / "next-toolkit"
        second = product.prepare(root, second_destination, threading.Event())
        self.assertNotEqual(first, second)
        for config, directory in ((first, self.destination), (second, second_destination)):
            args = config["mcpServers"]["toolkit-setup"]["args"]
            self.assertEqual(args[args.index("--state-root") + 1], str(directory / "local-session"))
        self.assertEqual((state / "session.json").read_bytes(), b"retain interrupted attempt\n")
        self.assertFalse((second_destination / "local-session").exists())
        self.assertEqual({name: (self.destination / name).read_bytes() for name in old}, old)

    def test_product_requires_launcher_from_the_exact_selected_archive(self):
        del self.files["setup/local_session.py"]
        (self.release / "setup/local_session.py").unlink()
        self.publish_release()
        self.refresh()
        product, root = self.product()
        with self.assertRaisesRegex(product.ResourceError, "local_setup_launcher_not_selected"):
            product.prepare(root, self.destination, threading.Event())
        self.assertTrue((self.destination / "config/prepared.json").is_file())
        self.assertFalse((self.destination / "product-prepared.json").exists())
        self.assertFalse((self.destination / "local-session").exists())

    def test_product_missing_or_linked_pip_refuses_before_starter_or_destination(self):
        product, root = self.product()
        pip = root / "runtime/pip.pyz"
        pip.unlink()
        for linked in (False, True):
            if linked:
                pip.symlink_to(self.pip)
            with self.subTest(linked=linked), self.assertRaisesRegex(
                    product.ResourceError, "bundled_python_acquisition_unavailable"):
                product.prepare(root, self.destination, threading.Event())
            self.assertFalse(self.destination.exists())

    def test_acquisition_requires_inspector_in_exact_archived_release(self):
        del self.files["setup/acquired_inspection.py"]
        (self.release / "setup/acquired_inspection.py").unlink()
        self.publish_release()
        self.refresh()
        self.options.allow_public_pypi = True
        self.options.pip_zipapp = str(self.pip)
        with self.assertRaisesRegex(Exception, "owner_refused"):
            starter.start(self.options)
        self.assertFalse((self.destination / "config").exists())

    def test_partial_bootstrap_is_preserved_and_not_reported_prepared(self):
        self.installer("partial")
        self.refresh()
        with self.assertRaisesRegex(Exception, "owner_refused"):
            starter.start(self.options)
        partial = self.destination / "client/partial"
        self.assertEqual(partial.read_text(), "keep this failed installation")
        self.assertFalse((self.destination / "config").exists())
        with self.assertRaisesRegex(Exception, "exists"):
            starter.start(self.options)
        self.assertTrue(partial.exists())

    def test_cancelled_before_start_has_no_effects(self):
        stopped = threading.Event()
        stopped.set()
        with self.assertRaisesRegex(Exception, "bootstrap_cancelled"):
            starter.start(self.options, stopped=stopped)
        self.assertFalse(self.destination.exists())


class SelectionTests(unittest.TestCase):
    def test_current_selection_resolves_both_platforms_without_optional_host_imports(self):
        manifest = starter.distribution()
        for target in starter.host.TARGETS:
            row = starter.selected_asset(manifest, target)
            self.assertTrue(row["upload_name"].endswith(target + ".tar.gz"))
        self.assertFalse(any(name.startswith(("meerkat.", "meerkat_mobkit.", "meerkat_toolkit."))
                             for name in __import__("sys").modules))

    def test_platform_and_python_mismatch_refuse(self):
        with patch.object(starter.platform, "system", return_value="Windows"):
            with self.assertRaisesRegex(Exception, "unsupported_host_platform"):
                starter.current_target()
        with patch.object(starter.platform, "system", return_value="Darwin"), \
             patch.object(starter.platform, "machine", return_value="arm64"), \
             patch.object(starter.sys, "version_info", (3, 12)):
            with self.assertRaisesRegex(Exception, "native_cpython_3_11"):
                starter.current_target()


if __name__ == "__main__":
    unittest.main()
