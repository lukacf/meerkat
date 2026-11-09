"""The ordinary starter composes real preparation, with a controlled installer."""
import builtins
import errno
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
import sys
from pathlib import Path
import tarfile
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import test_bootstrap_host as fixture


SOURCE = fixture.SOURCE
spec = importlib.util.spec_from_file_location("toolkit_starter", SOURCE / "start.py")
starter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(starter)


class BootstrapCommandCaptured(Exception):
    """Stops a product preparation at the archived bootstrap, before it runs."""


def pinned_libraries():
    """The library archive each target's Connect resources pin, with its digest and size."""
    manifest = json.loads((SOURCE / "product/release-manifest.json").read_bytes())
    return {row["upload_name"]: (row["sha256"], row["bytes"]) for row in manifest["assets"]}


class StarterTests(unittest.TestCase):
    publish_release = fixture.BootstrapTests.publish_release
    installer = fixture.BootstrapTests.installer
    official_release = fixture.preparation_tests.PreparationTests.official_release

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

    def test_starter_explicit_github_selection_uses_archived_bindings_without_node(self):
        self.official_release(("github",))
        self.refresh()
        self.options.families = "official-github"
        result = starter.start(self.options)
        self.assertEqual(result["families"], ["official-github"])
        argv = json.loads(Path(result["configuration"]).read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--families") + 1], "official-github")
        self.assertEqual(argv[argv.index("--official-contents-sha256") + 1],
                         fixture.sha(self.files["official/CONTENTS.json"][0]))
        self.assertNotIn("--node", argv)
        self.assertFalse((self.destination / "session").exists())

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

    def product_cli(self, product, root, destination=None, extra=()):
        output, errors = io.StringIO(), io.StringIO()
        with patch.object(product, "__file__", str(root / "prepare.py")), \
                redirect_stdout(output), redirect_stderr(errors):
            with self.assertRaises(SystemExit) as failure:
                product.main(["--destination", str(destination or self.destination), *extra])
        self.assertEqual(failure.exception.code, 1)
        self.assertEqual(output.getvalue(), "")
        lines = errors.getvalue().splitlines()
        self.assertEqual(lines[-1], "Setup preparation unconfirmed. Preserve the setup directory for recovery.")
        self.assertEqual(len(lines), 2)
        return json.loads(lines[0])

    def linux_mount_product(self):
        # Bind every archive/receipt target to Linux while retaining the
        # controlled installer and current test interpreter. This exercises the
        # real preparation path, not native Linux or installed acceptance.
        self.root_name = "toolkit-library-fixture-linux-x86_64"
        name = "selected/provenance/selection.json"
        selected = json.loads(self.files[name][0])
        selected["platform"] = "linux-x86_64"
        self.files[name] = (json.dumps(selected).encode(), self.files[name][1])
        self.publish_release()
        self.name = self.root_name + ".tar.gz"
        self.selected = self.downloads / self.name
        self.selection["assets"][0]["upload_name"] = self.name
        self.refresh()
        product, root = self.product()
        product.verify_resources.return_value[0]["target"] = "linux-x86_64"
        product.HOST_TARGETS = {(product.platform.system(), product.platform.machine()): "linux-x86_64"}
        load = product.exec.side_effect
        def load_linux_fixture(code, namespace):
            load(code, namespace)
            namespace["current_target"] = lambda: "linux-x86_64"
        product.exec.side_effect = load_linux_fixture
        return product, root

    def mount_observation(self, product, flags=0, **changes):
        observe = Mock(return_value=SimpleNamespace(f_flag=flags))
        patch.object(product, "os", SimpleNamespace(**{
            **vars(os), "ST_NOEXEC": 8, "statvfs": observe, **changes})).start()
        return observe

    def test_product_noexec_parent_refuses_before_starter_or_filesystem_effects(self):
        self.recipe_owner_fixture()
        product, root = self.linux_mount_product()
        observe = self.mount_observation(product, flags=8 | 2)
        state = self.base / "state"
        setup = state / ".meerkat-connect" / "test-realm" / "setup-test"
        setup.mkdir(parents=True)
        destination = setup / "prepared"
        before = set(self.base.rglob("*"))
        record = self.product_cli(product, root, destination,
            ("--realm-state-root", str(state), "--realm", "test-realm"))
        self.assertEqual(record, {"schema_version": 1, "kind": "preparation_failure",
            "phase": "destination_filesystem", "code": "destination_noexec"})
        observe.assert_called_once_with(setup)
        product.exec.assert_not_called()
        self.assertEqual(set(self.base.rglob("*")), before)
        self.assertFalse(destination.exists())
        self.assertFalse((setup.parent / "recipes.sqlite").exists())

    def test_product_linux_without_noexec_keeps_actual_preparation_and_protocol(self):
        product, root = self.linux_mount_product()
        observe = self.mount_observation(product, flags=2)
        output, errors = io.StringIO(), io.StringIO()
        with patch.object(product, "__file__", str(root / "prepare.py")), \
                redirect_stdout(output), redirect_stderr(errors):
            self.assertEqual(product.main(["--destination", str(self.destination)]), 0)
        observe.assert_called_once_with(self.destination.parent)
        self.assertEqual(errors.getvalue(), "")
        self.assertEqual(json.loads(output.getvalue()), json.loads((self.destination / "product-mcp.json").read_bytes()))
        self.assertTrue((self.destination / "client/verify-entered").is_file())
        self.assertTrue((self.destination / "product-prepared.json").is_file())

    def test_product_mount_observation_error_is_not_noexec_or_success(self):
        product, root = self.linux_mount_product()
        observe = self.mount_observation(product)
        for number, expected in ((errno.EACCES, "os_permission_denied"), (errno.ENOSYS, "os_error")):
            with self.subTest(errno=number):
                observe.side_effect = OSError(number, "PRIVATE mount error", "/PRIVATE/mount")
                record = self.product_cli(product, root)
                self.assertEqual(record["phase"], "destination_filesystem")
                self.assertEqual(record["code"], expected)
                product.exec.assert_not_called()
                self.assertFalse(self.destination.exists())

    def test_product_mount_api_and_invalid_observations_refuse_before_effects(self):
        product, root = self.linux_mount_product()
        variants = [({"ST_NOEXEC": None}, 0), ({"ST_NOEXEC": True}, 0),
                    ({"ST_NOEXEC": 0}, 0), ({"statvfs": None}, 0),
                    ({}, None), ({}, True), ({}, -1)]
        for changes, flags in variants:
            with self.subTest(changes=changes, flags=flags):
                self.mount_observation(product, flags, **changes)
                record = self.product_cli(product, root)
                self.assertEqual(record["phase"], "destination_filesystem")
                self.assertEqual(record["code"], "destination_mount_unconfirmed")
                product.exec.assert_not_called()
                self.assertFalse(self.destination.exists())

    def test_product_destination_validation_precedes_mount_observation(self):
        product, root = self.linux_mount_product()
        observe = self.mount_observation(product, flags=8)
        alias = self.base / "parent-alias"
        alias.symlink_to(self.base, target_is_directory=True)
        for destination in (alias / "prepared", self.base, Path("relative-prepared")):
            with self.subTest(destination=destination):
                record = self.product_cli(product, root, destination)
                self.assertEqual(record["phase"], "destination")
                self.assertEqual(record["code"], "resource_error")
                observe.assert_not_called()
                product.exec.assert_not_called()
        self.assertFalse((self.base / "prepared").exists())

    def test_product_nonlinux_target_does_not_infer_mount_flag_semantics(self):
        product, root = self.product()
        observe = self.mount_observation(product, flags=8)
        result = product.prepare(root, self.destination, threading.Event())
        observe.assert_not_called()
        self.assertIn("toolkit-setup", result["mcpServers"])
        self.assertTrue((self.destination / "client/verify-entered").is_file())

    def test_product_recipe_spawn_denied_retains_phase_without_private_error(self):
        self.recipe_owner_fixture()
        product, root = self.product()
        state = self.base / "state"
        setup = state / ".meerkat-connect" / "test-realm" / "setup-test"
        setup.mkdir(parents=True)
        destination = setup / "prepared"
        load = product.exec.side_effect
        calls = []
        def deny_spawn(argv, **kwargs):
            calls.append(argv)
            raise PermissionError(errno.EACCES, "PRIVATE credential configuration", "/PRIVATE/executable")
        def load_with_spawn_failure(code, namespace):
            load(code, namespace)
            # Substitute only process creation at the actual runner boundary.
            # Resource admission is the existing controlled product fixture.
            namespace["runner"].subprocess = SimpleNamespace(
                **{**vars(namespace["runner"].subprocess), "Popen": deny_spawn})
        with patch.object(product, "exec", side_effect=load_with_spawn_failure):
            record = self.product_cli(product, root, destination,
                ("--realm-state-root", str(state), "--realm", "test-realm"))
        self.assertEqual(record, {"schema_version": 1, "kind": "preparation_failure",
            "phase": "recipe_initialization", "code": "os_permission_denied"})
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][1], "init")
        self.assertTrue((destination / "distribution" / self.root_name / "selected/bin/toolkit-local-recipes").is_file())
        for name in ("staging", "client", "config", "product-mcp.json", "product-prepared.json"):
            self.assertFalse((destination / name).exists(), name)
        self.assertFalse((setup.parent / "recipes.sqlite").exists())

    def test_product_bootstrap_failure_retains_phase_and_partial_output(self):
        self.installer("partial")
        self.refresh()
        product, root = self.product()
        record = self.product_cli(product, root)
        self.assertEqual(record, {"schema_version": 1, "kind": "preparation_failure",
            "phase": "bootstrap", "code": "owner_refused"})
        self.assertTrue((self.destination / "client/partial").is_file())
        self.assertTrue((self.destination / "staging").is_dir())
        self.assertFalse((self.destination / "product-mcp.json").exists())

    def test_product_success_keeps_stdout_projection_and_stderr_empty(self):
        product, root = self.product()
        output, errors = io.StringIO(), io.StringIO()
        with patch.object(product, "__file__", str(root / "prepare.py")), \
                redirect_stdout(output), redirect_stderr(errors):
            self.assertEqual(product.main(["--destination", str(self.destination)]), 0)
        self.assertEqual(errors.getvalue(), "")
        self.assertEqual(json.loads(output.getvalue()), json.loads((self.destination / "product-mcp.json").read_bytes()))
        self.assertTrue((self.destination / "product-prepared.json").is_file())

    def test_product_explicit_families_reach_actual_preparer_with_exact_official_pins(self):
        self.official_release(("github",))
        self.refresh()
        product, root = self.product()
        manifest = product.verify_resources.return_value[0]
        for index, families in enumerate(("official-github", "agno,services,activepieces,official-github")):
            with self.subTest(families=families):
                manifest["families"] = families
                destination = self.base / ("selected-families-" + str(index))
                product.prepare(root, destination, threading.Event())
                argv = json.loads((destination / "config/mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
                release = destination / "distribution" / self.root_name
                for flag, value in {
                    "--families": families,
                    "--official-bundle": str(release / "official"),
                    "--official-contents-sha256": fixture.sha(self.files["official/CONTENTS.json"][0]),
                    "--official-installer": str(release / "setup/install-library-consumer.py"),
                    "--official-installer-sha256": fixture.sha(self.files["setup/install-library-consumer.py"][0]),
                }.items():
                    self.assertEqual(argv.count(flag), 1)
                    self.assertEqual(argv[argv.index(flag) + 1], value)
                for flag, path in (("--node", root / "node"), ("--npm-cli", root / "npm.js")):
                    if "activepieces" in families.split(","):
                        self.assertEqual(argv[argv.index(flag) + 1], str(path))
                    else:
                        self.assertNotIn(flag, argv)
                self.assertNotIn("--playwright-host-profile", argv)
                self.assertFalse((destination / "session").exists())
                self.assertFalse((destination / "local-session").exists())

    def test_product_official_payload_does_not_enable_unselected_families(self):
        self.official_release()
        self.refresh()
        product, root = self.product()
        product.prepare(root, self.destination, threading.Event())
        argv = json.loads((self.destination / "config/mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--families") + 1], "agno,services,activepieces")
        self.assertFalse(any(arg.startswith("--official-") or arg.startswith("--playwright-") for arg in argv))

    def test_product_family_selection_refuses_unsupported_or_unshipped_families_before_client_install(self):
        product, root = self.product()
        manifest = product.verify_resources.return_value[0]
        for index, families in enumerate(("", "agno,agno", "unknown", "official-github")):
            with self.subTest(families=families):
                manifest["families"] = families
                destination = self.base / ("refused-families-" + str(index))
                with self.assertRaisesRegex(Exception, "owner_refused"):
                    product.prepare(root, destination, threading.Event())
                self.assertFalse((destination / "client").exists())
                self.assertFalse((destination / "product-mcp.json").exists())

    def test_product_refuses_starter_that_ignores_explicit_families(self):
        self.official_release(("github",))
        self.refresh()
        product, root = self.product()
        product.verify_resources.return_value[0]["families"] = "official-github"
        def ignoring_starter(code, namespace):
            builtins.exec(code, namespace)
            namespace["current_target"] = lambda: "macos-arm64"
            start = namespace["start"]
            def ignore_selection(args, **kwargs):
                del args.families
                return start(args, **kwargs)
            namespace["start"] = ignore_selection
        with patch.object(product, "exec", side_effect=ignoring_starter):
            with self.assertRaisesRegex(product.ResourceError, "family_selection_not_prepared"):
                product.prepare(root, self.destination, threading.Event())
        self.assertFalse((self.destination / "product-mcp.json").exists())

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

    def test_product_npm_acquisition_survives_family_selection_without_library_node(self):
        product, root, group = self.npm_product()
        product.verify_resources.return_value[0]["families"] = "agno,services"
        product.prepare(root, self.destination, threading.Event())
        output = self.destination / "config"
        receipt = json.loads((output / "prepared.json").read_bytes())
        args = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(args[args.index("--families") + 1], "agno,services")
        for flag, runtime in (("--node", "node"), ("--npm-cli", "npm_cli")):
            self.assertNotIn(flag, args)
            self.assertNotIn(runtime, receipt["runtimes"])
        for flag, runtime, resource in (("--selected-node", "selected_node", "node"),
                                        ("--selected-npm-cli", "selected_npm_cli", "npm_cli")):
            path = root / group[resource]
            self.assertEqual(args.count(flag), 1)
            self.assertEqual(args[args.index(flag) + 1], str(path))
            self.assertEqual(receipt["runtimes"][runtime]["path"], str(path))
            self.assertEqual(receipt["runtimes"][runtime]["sha256"], fixture.sha(path.read_bytes()))
        archive = root / group["npm_archive"]
        archive_binding = {"path": str(archive), "sha256": fixture.sha(archive.read_bytes())}
        for key, value in archive_binding.items():
            self.assertEqual(receipt["runtimes"]["npm_archive"][key], value)
        profile = json.loads((output / "node-acquisition.json").read_bytes())
        self.assertEqual(profile["npm"], archive_binding)
        self.assertEqual(profile["node_sha256"], fixture.sha((root / group["node"]).read_bytes()))
        self.assertIs(profile["allow_acquisition"], True)
        self.assertIs(receipt["public_npm_enabled"], True)
        self.assertIs(receipt["node_search_enabled"], True)
        self.assertIn("--allow-node-search", args)
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(set(selected["validators"]), {"python", "node"})
        self.assertFalse((self.destination / "session").exists())
        self.assertFalse((self.destination / "local-session").exists())

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

    def product_bootstrap_arguments(self):
        # Actual product preparation with every optional resource it forwards
        # (the npm group, a Node prebuilt policy, a Node build profile and a
        # native realm's recipes), stopped at the archived bootstrap: the
        # exact arguments the actual starter hands it. The bootstrap never runs.
        self.recipe_owner_fixture()
        product, root, _ = self.npm_product()
        entries = []
        for name in ("node-prebuilt-policy.json", "node-build-profile.json"):
            raw = b'{ "scope": "transport_fixture" }\n'
            (root / name).write_bytes(raw)
            entries.append({"path": name, "kind": "file", "mode": 0o644,
                            "size": len(raw), "sha256": fixture.sha(raw)})
        product.verify_resources.return_value[0]["entries"] = entries
        captured = []
        load = product.exec.side_effect
        def stop_at_bootstrap(code, namespace):
            load(code, namespace)
            run_owner = namespace["runner"].run_owner
            def owner(command, *args, **kwargs):
                if len(command) < 4 or Path(command[3]).name != "bootstrap_host.py":
                    return run_owner(command, *args, **kwargs)
                captured.append([str(part) for part in command[4:]])
                raise BootstrapCommandCaptured
            namespace["runner"].run_owner = owner
        product.exec.side_effect = stop_at_bootstrap
        state = self.base / "state"
        attempt = state / ".meerkat-connect/work/setup-one"
        attempt.mkdir(parents=True)
        with self.assertRaises(BootstrapCommandCaptured):
            product.prepare(root, attempt / "prepared", threading.Event(),
                            realm_state_root=str(state), realm="work")
        self.assertEqual(len(captured), 1)
        return captured[0]

    def test_pinned_library_bootstrap_accepts_every_argument_product_preparation_forwards(self):
        # Opt-in: the packaging owner supplies the exact library archive the
        # Connect resources pin for a target. Only its own bootstrap's argument
        # check runs, never its bootstrap or installer.
        selected = os.environ.get("CONNECT_PINNED_LIBRARY_ARCHIVE")
        if selected is None:
            self.skipTest("requires the exact library archive the Connect resources pin")
        archive = Path(selected)
        self.assertTrue(archive.is_absolute())
        pins = pinned_libraries()
        self.assertIn(archive.name, pins, "the supplied archive is not a library the Connect resources pin")
        raw = archive.read_bytes()
        self.assertEqual((fixture.sha(raw), len(raw)), pins[archive.name])
        setup = self.base / "pinned-library-setup"
        setup.mkdir()
        wanted = {archive.name.removesuffix(".tar.gz") + "/setup/" + name: name
                  for name in ("bootstrap_host.py", "prepare_host.py")}
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r|gz") as library:
            for member in library:
                if member.name in wanted:
                    self.assertTrue(member.isfile())
                    (setup / wanted.pop(member.name)).write_bytes(library.extractfile(member).read())
                    if not wanted:
                        break
        self.assertEqual(wanted, {})
        spec = importlib.util.spec_from_file_location("pinned_library_bootstrap", setup / "bootstrap_host.py")
        pinned = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(pinned)
        arguments = self.product_bootstrap_arguments()
        accepted, refused = [], io.StringIO()
        with patch.object(pinned, "bootstrap", side_effect=lambda args, **_: accepted.append(args) or {}), \
                redirect_stdout(io.StringIO()), redirect_stderr(refused):
            try:
                pinned.main(arguments)
            except SystemExit:
                self.fail("the pinned library's bootstrap refuses what product preparation forwards:\n"
                          + refused.getvalue())
        self.assertEqual(len(accepted), 1)
        self.assertIs(accepted[0].enable_public_github_research, True)


class SelectionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="starter-selection-")
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()
        self.source = self.base / "sdk/consumer-setup"
        self.source.mkdir(parents=True)
        for name in ("start.py", "bootstrap_host.py", "prepare_host.py", "current-distribution.json"):
            (self.source / name).write_bytes((SOURCE / name).read_bytes())
        spec = importlib.util.spec_from_file_location("selected_starter", self.source / "start.py")
        self.starter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.starter)

    def test_selected_manifest_resolves_both_platforms_without_optional_host_imports(self):
        selection = {"schema_version": 1, "assets": [
            {"upload_name": "toolkit-library-controlled-" + target + ".tar.gz",
             "sha256": "a" * 64, "bytes": 7} for target in self.starter.host.TARGETS]}
        raw = json.dumps(selection).encode()
        (self.source / "release-manifest.json").write_bytes(raw)
        (self.source / "current-distribution.json").write_text(json.dumps({
            "schema_version": 1, "manifest": "release-manifest.json", "manifest_sha256": fixture.sha(raw)}))
        manifest = self.starter.distribution()
        self.assertEqual(manifest, selection)
        for target in self.starter.host.TARGETS:
            row = self.starter.selected_asset(manifest, target)
            self.assertTrue(row["upload_name"].endswith(target + ".tar.gz"))
        self.assertFalse(any(name.startswith(("meerkat.", "meerkat_mobkit.", "meerkat_toolkit."))
                             for name in __import__("sys").modules))

    def test_missing_default_selection_refuses_before_output_or_owner(self):
        selector = self.source / "current-distribution.json"
        # This copied historical pointer has no manifest in the controlled root.
        # A developer's retained checkout evidence cannot satisfy this selection.
        pointer = json.loads(selector.read_bytes())
        self.assertFalse((self.source / pointer["manifest"]).exists())
        destination = self.base / "new-toolkit"
        options = SimpleNamespace(timeout_seconds=30, destination=str(destination),
                                  archive_dir=str(self.base / "downloads"))
        for missing in ("manifest", "selector"):
            with self.subTest(missing=missing):
                if missing == "selector":
                    selector.unlink()
                with patch.object(self.starter, "current_target", return_value="macos-arm64"), \
                     patch.object(self.starter.runner, "run_owner") as owner, \
                     self.assertRaisesRegex(self.starter.host.PreparationError,
                         "distribution_selection_unavailable_use_trusted_exported_starter"):
                    self.starter.start(options)
                owner.assert_not_called()
                self.assertFalse(destination.exists())

    def test_platform_and_python_mismatch_refuse(self):
        with patch.object(starter.platform, "system", return_value="Windows"):
            with self.assertRaisesRegex(Exception, "unsupported_host_platform"):
                starter.current_target()
        with patch.object(starter.platform, "system", return_value="Darwin"), \
             patch.object(starter.platform, "machine", return_value="arm64"), \
             patch.object(starter.sys, "version_info", (3, 12)):
            with self.assertRaisesRegex(Exception, "native_cpython_3_11"):
                starter.current_target()


class PreparationDiagnosticTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("diagnostic_product", SOURCE / "product/prepare.py")
        self.product = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.product)

    def failure(self, error, phase="resources"):
        output, errors = io.StringIO(), io.StringIO()
        def refuse(*args, **kwargs):
            kwargs["diagnostic"].mark(phase)
            raise error
        with patch.object(self.product, "prepare", side_effect=refuse), \
                redirect_stdout(output), redirect_stderr(errors):
            with self.assertRaises(SystemExit) as failure:
                self.product.main(["--destination", "/PRIVATE/destination"])
        self.assertEqual(failure.exception.code, 1)
        self.assertEqual(output.getvalue(), "")
        lines = errors.getvalue().splitlines()
        self.assertEqual(len(lines), 2)
        self.assertEqual(lines[-1], "Setup preparation unconfirmed. Preserve the setup directory for recovery.")
        self.assertNotIn("PRIVATE", errors.getvalue())
        return json.loads(lines[0])

    def test_exception_payloads_and_unknown_phases_are_never_formatted(self):
        class PrivateError(Exception):
            def __str__(self):
                raise AssertionError("exception formatting is forbidden")
        error = PrivateError("PRIVATE credential")
        error.__cause__ = ValueError("PRIVATE configuration")
        self.assertEqual(self.failure(error, "/PRIVATE/phase"), {
            "schema_version": 1, "kind": "preparation_failure", "phase": "unknown", "code": "unexpected_error"})
        self.assertEqual(self.failure(self.product.ResourceError("PRIVATE input"))["code"], "resource_error")
        self.assertEqual(self.failure(ValueError("owner_refused"))["code"], "unexpected_error")

    def test_os_error_categories_are_closed_and_do_not_export_errno_or_filename(self):
        for number, expected in ((errno.EACCES, "os_permission_denied"), (errno.EPERM, "os_operation_not_permitted"),
                (errno.ENOENT, "os_not_found"), (errno.ENOEXEC, "os_exec_format"), (987654, "os_error")):
            with self.subTest(errno=number):
                record = self.failure(OSError(number, "PRIVATE error", "/PRIVATE/path"))
                self.assertEqual(record["code"], expected)
                self.assertEqual(set(record), {"schema_version", "kind", "phase", "code"})

    def test_known_owner_codes_require_the_selected_exception_type(self):
        diagnostic = self.product.PreparationDiagnostic()
        diagnostic.owner_errors = (starter.host.PreparationError, starter.runner.BootstrapError)
        for code in ("bootstrap_cancelled", "bootstrap_timeout", "owner_refused", "owner_output_limit"):
            self.assertEqual(diagnostic.code(starter.runner.BootstrapError(code)), code)
        self.assertEqual(diagnostic.code(starter.runner.BootstrapError("PRIVATE detail")), "owner_error")
        self.assertEqual(diagnostic.code(starter.runner.BootstrapError("owner_refused", "PRIVATE")), "owner_error")
        self.assertEqual(diagnostic.code(self.product.ResourceError("preparation_cancelled")), "preparation_cancelled")

    def test_diagnostic_is_flushed_before_generic_failure(self):
        events = []
        class RecordingStream(io.StringIO):
            def write(self, text):
                events.append(("write", text))
                return super().write(text)
            def flush(self):
                events.append(("flush", None))
                return super().flush()
        stream = RecordingStream()
        with patch.object(self.product, "verify_resources", side_effect=self.product.ResourceError("PRIVATE")), \
                redirect_stderr(stream):
            with self.assertRaises(SystemExit) as failure:
                self.product.main(["--destination", "/PRIVATE/destination"])
        self.assertEqual(failure.exception.code, 1)
        first_flush = next(i for i, value in enumerate(events) if value[0] == "flush")
        generic = next(i for i, value in enumerate(events) if value[0] == "write" and value[1].startswith("Setup preparation"))
        self.assertLess(first_flush, generic)
        self.assertEqual(json.loads(stream.getvalue().splitlines()[0])["phase"], "resources")

    def test_unavailable_diagnostic_sink_preserves_generic_exit(self):
        class UnavailableOnce(io.StringIO):
            unavailable = True
            def write(self, text):
                if self.unavailable:
                    self.unavailable = False
                    raise OSError(errno.ENOSPC, "PRIVATE sink")
                return super().write(text)
        stream = UnavailableOnce()
        with patch.object(self.product, "verify_resources", side_effect=ValueError("PRIVATE")), redirect_stderr(stream):
            with self.assertRaises(SystemExit) as failure:
                self.product.main(["--destination", "/PRIVATE/destination"])
        self.assertEqual(failure.exception.code, 1)
        self.assertEqual(stream.getvalue(), "Setup preparation unconfirmed. Preserve the setup directory for recovery.\n")

    def test_absent_stderr_never_falls_back_to_protocol_stdout(self):
        output = io.StringIO()
        diagnostic = self.product.PreparationDiagnostic()
        with redirect_stdout(output), redirect_stderr(None):
            diagnostic.emit(ValueError("PRIVATE"))
        self.assertEqual(output.getvalue(), "")

    def test_absent_stderr_full_command_preserves_exit_and_empty_stdout(self):
        output = io.StringIO()
        with patch.object(self.product, "verify_resources", side_effect=ValueError("PRIVATE")), \
                redirect_stdout(output), redirect_stderr(None):
            with self.assertRaises(SystemExit) as failure:
                self.product.main(["--destination", "/PRIVATE/destination"])
        self.assertEqual(failure.exception.code, 1)
        self.assertEqual(output.getvalue(), "")

    def test_diagnostic_formatting_failure_preserves_generic_exit(self):
        for error in (MemoryError("PRIVATE allocation"), ValueError("PRIVATE formatting")):
            output, errors = io.StringIO(), io.StringIO()
            with self.subTest(error=type(error).__name__), \
                    patch.object(self.product, "verify_resources", side_effect=ValueError("PRIVATE owner")), \
                    patch.object(self.product.PreparationDiagnostic, "code", side_effect=error), \
                    redirect_stdout(output), redirect_stderr(errors):
                with self.assertRaises(SystemExit) as failure:
                    self.product.main(["--destination", "/PRIVATE/destination"])
                self.assertEqual(failure.exception.code, 1)
                self.assertEqual(output.getvalue(), "")
                self.assertEqual(errors.getvalue(), "Setup preparation unconfirmed. Preserve the setup directory for recovery.\n")

    def test_other_product_modes_keep_their_existing_generic_failure(self):
        for flag, method in (("--installation-readback", "installation_readback"), ("--local-round-owner", "local_round_owner")):
            output, errors = io.StringIO(), io.StringIO()
            with self.subTest(flag=flag), patch.object(self.product, method, side_effect=ValueError("PRIVATE")), \
                    redirect_stdout(output), redirect_stderr(errors):
                with self.assertRaises(SystemExit) as failure:
                    self.product.main(["--destination", "/PRIVATE/destination", flag, "/PRIVATE/request"])
                self.assertEqual(failure.exception.code, 1)
                self.assertEqual(output.getvalue(), "")
                self.assertEqual(errors.getvalue(), "Setup preparation unconfirmed. Preserve the setup directory for recovery.\n")

    def test_process_control_exceptions_are_not_reclassified(self):
        for error in (KeyboardInterrupt(), SystemExit(7)):
            output, errors = io.StringIO(), io.StringIO()
            with self.subTest(error=type(error).__name__), patch.object(self.product, "prepare", side_effect=error), \
                    redirect_stdout(output), redirect_stderr(errors):
                with self.assertRaises(type(error)) as caught:
                    self.product.main(["--destination", "/PRIVATE/destination"])
                self.assertIs(caught.exception, error)
                self.assertEqual(output.getvalue(), "")
                self.assertEqual(errors.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
