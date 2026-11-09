"""Host preparation uses real profile readers and inert executable inputs."""
from contextlib import redirect_stdout
import asyncio
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import venv


SOURCE = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("prepare_host", SOURCE / "prepare_host.py")
host = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(host)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


class PreparationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="toolkit-host-preparation-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.release = self.base / "release with spaces"
        self.release.mkdir(mode=0o755)
        self.staging = self.base / "staging"
        self.staging.mkdir()
        self.archive = self.base / "release.tar.gz"
        self.root_name = "toolkit-library-fixture-macos-arm64"
        self.files = {
            "setup/backend.py": ((SOURCE / "backend.py").read_bytes(), 0o644),
            "setup/python_acquisition.py": ((SOURCE / "python_acquisition.py").read_bytes(), 0o644),
            "setup/mcp_adapter.py": (b"import json,sys; print(json.dumps({'prefix':sys.prefix}))\n", 0o644),
            "consumers/CONTENTS.json": (b'{"files":{}}\n', 0o644),
            "consumers/install-library-consumer.py": (b"# inert library installer\n", 0o644),
            "selected/bin/install-library-consumer.py": (b"# inert selected installer\n", 0o755),
            "selected/bin/toolkit-python-plan": (b"# inert planner, never executed\n", 0o755),
            "selected/provenance/selection.json": (json.dumps({"platform": "macos-arm64", "scope": "retained_python_tools_only"}).encode(), 0o644),
            "licenses/notice.txt": (b"retained notice\n", 0o644),
        }
        self.publish_release()
        self.runtime = self.base / "host-python"
        self.runtime.write_bytes(b"inert host executable, must never run\n")
        self.runtime.chmod(0o755)
        self.pip = self.base / "pip.pyz"
        self.pip.write_bytes(b"inert trusted pip, must never run\n")
        self.args = SimpleNamespace(release_archive=str(self.archive), release_sha256=sha(self.archive.read_bytes()),
            release_dir=str(self.release), target="macos-arm64", output=str(self.base / "host-config"),
            workspace=str(self.base / "future-workspace"), staging_root=str(self.staging),
            python=str(self.runtime), python_sha256=sha(self.runtime.read_bytes()),
            mcp_python=str(self.runtime), mcp_python_sha256=sha(self.runtime.read_bytes()),
            families="agno,services,activepieces", node=str(self.runtime), node_sha256=sha(self.runtime.read_bytes()),
            npm_cli=str(self.pip), npm_cli_sha256=sha(self.pip.read_bytes()),
            allow_public_pypi=False, pip_zipapp=None, pip_sha256=None,
            allow_public_npm=False, npm_archive=None, npm_archive_sha256=None,
            selected_node=None, selected_node_sha256=None,
            selected_npm_cli=None, selected_npm_cli_sha256=None,
            playwright_host_profile=None, playwright_host_profile_sha256=None,
            recipes_database=None, recipes_realm=None, recipes_owner=None, recipes_scope=None,
            allow_remote_recipes=False)

    def publish_release(self, extra=None):
        for name, (raw, mode) in self.files.items():
            path = self.release / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
            path.chmod(mode)
        directories = {str(parent) for name in self.files for parent in Path(name).parents if str(parent) != "."}
        for name in directories:
            (self.release / name).chmod(0o755)
        with tarfile.open(self.archive, "w:gz") as archive:
            for name in sorted({""} | directories | set(self.files)):
                info = tarfile.TarInfo(self.root_name + ("/" + name if name else ""))
                if name in self.files:
                    raw, info.mode = self.files[name]
                    info.size = len(raw)
                    archive.addfile(info, io.BytesIO(raw))
                else:
                    info.type, info.mode = tarfile.DIRTYPE, 0o755
                    archive.addfile(info)
            if extra is not None:
                archive.addfile(extra)
        if hasattr(self, "args"):
            self.args.release_sha256 = sha(self.archive.read_bytes())

    def acquire(self):
        self.args.allow_public_pypi = True
        self.args.pip_zipapp, self.args.pip_sha256 = str(self.pip), sha(self.pip.read_bytes())

    def refuse(self, code):
        with self.assertRaisesRegex(host.PreparationError, code):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_existing_profiles_and_mcp_arguments_without_launch(self):
        self.acquire()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            result = host.prepare(self.args)
        output = Path(result["directory"])
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(receipt["scope"], "host_setup_configuration_only")
        self.assertEqual(receipt["target"], "macos-arm64")
        self.assertEqual(sha((output / "prepared.json").read_bytes()), result["receipt_sha256"])
        for name, digest in receipt["files"].items():
            self.assertEqual(sha((output / name).read_bytes()), digest)
            self.assertEqual((output / name).stat().st_mode & 0o777, 0o600)
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertNotIn("python_target", selected)
        self.assertEqual(selected["validators"]["python"]["path"], str(self.release / "selected/bin/toolkit-python-plan"))
        mcp = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        args = mcp["args"]
        self.assertEqual(mcp["command"], "/usr/bin/env")
        self.assertEqual(args[0], "-i")
        self.assertEqual(args[args.index("--families") + 1], "agno,services,activepieces")
        self.assertEqual(args[args.index("--selected-install-profile") + 1], str(output / "selected-install.json"))
        self.assertEqual(args[args.index("--python-acquisition-profile-sha256") + 1], receipt["files"]["python-acquisition.json"])
        self.assertFalse(Path(self.args.workspace).exists())
        self.assertFalse(list(self.release.rglob("__pycache__")))
        self.assertNotIn("--prior-request", args)
        self.assertNotIn("prior-request.json", receipt["files"])

    def prior_request(self, record=None):
        from test_successor import historical
        path = self.base / "historical-request.json"
        # Deliberately retain non-canonical whitespace as part of the source.
        path.write_text(json.dumps(record or historical(), indent=2) + "\n")
        self.args.prior_request = str(path)
        self.args.prior_request_sha256 = sha(path.read_bytes())
        return path

    def test_prior_request_pair_is_explicit(self):
        path = self.prior_request()
        expected = sha(path.read_bytes())
        for source, digest in ((str(path), None), (None, expected), ("", expected),
                               (str(path), ""), (False, False)):
            self.args.prior_request, self.args.prior_request_sha256 = source, digest
            self.refuse("prior_request_pair_required")
        self.args.prior_request, self.args.prior_request_sha256 = path.name, expected
        self.refuse("invalid_input_path")
        self.args.prior_request = str(path)
        self.args.prior_request_sha256 = "not-a-digest"
        self.refuse("invalid_sha256")

    def test_prior_request_cli_preserves_exact_bytes_and_all_supported_versions(self):
        from test_successor import historical
        for version, family in ((1, "agno"), (2, "official-github"), (3, "official-playwright")):
            with self.subTest(version=version):
                selection = {"family": family, "profile": "example", "operations": ["read"], "values": {}}
                record = historical(selection)
                record["schema_version"] = version
                if version > 1:
                    record["official_component"] = family.removeprefix("official-")
                if version == 3:
                    record["playwright_host_profile_sha256"] = "e" * 64
                path = self.prior_request(record)
                original = path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_mode
                self.args.output = str(self.base / ("prepared-v" + str(version)))
                arguments = []
                for key, value in vars(self.args).items():
                    if value is not None and value is not False:
                        arguments.append("--" + key.replace("_", "-"))
                        if value is not True:
                            arguments.append(str(value))
                with redirect_stdout(io.StringIO()), patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
                    self.assertEqual(host.main(arguments), 0)
                output = Path(self.args.output)
                frozen = output / "prior-request.json"
                self.assertEqual(frozen.read_bytes(), original[0])
                self.assertEqual(frozen.stat().st_mode & 0o777, 0o600)
                self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns, path.stat().st_mode), original)
                receipt = json.loads((output / "prepared.json").read_bytes())
                self.assertEqual(receipt["files"]["prior-request.json"], self.args.prior_request_sha256)
                argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
                self.assertEqual(argv[argv.index("--prior-request") + 1], str(frozen))
                self.assertEqual(argv[argv.index("--prior-request-sha256") + 1], self.args.prior_request_sha256)
                self.assertEqual(argv[argv.index("--families") + 1], self.args.families)
                self.assertFalse(Path(self.args.workspace).exists())

    def test_prior_request_bad_hash_schema_duplicate_keys_and_bounds_refuse(self):
        path = self.prior_request()
        valid = path.read_bytes()
        self.args.prior_request_sha256 = "0" * 64
        self.refuse("prior_request_changed")
        for raw in (b"{}", valid.replace(b'"schema_version": 1', b'"schema_version": true'),
                    valid.replace(b'"schema_version": 1', b'"schema_version": 1, "schema_version": 1'),
                    b"x" * (128 * 1024 + 1)):
            path.write_bytes(raw)
            self.args.prior_request_sha256 = sha(raw)
            with self.assertRaisesRegex(Exception, "invalid_prior_request|invalid_file"):
                host.prepare(self.args)
            self.assertFalse(Path(self.args.output).exists())
            self.assertFalse(Path(self.args.workspace).exists())

    def test_prior_request_aliases_overlaps_and_original_write_paths_refuse(self):
        path = self.prior_request()
        alias = self.base / "source-alias.json"
        alias.symlink_to(path)
        self.args.prior_request = str(alias)
        with self.assertRaises(OSError):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        parent_alias = self.base / "parent-alias"
        parent_alias.symlink_to(self.base, target_is_directory=True)
        self.args.prior_request = str(parent_alias / path.name)
        with self.assertRaisesRegex(Exception, "invalid_recovery_path"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        original = path.read_bytes()
        staged = self.staging / path.name
        staged.write_bytes(original)
        self.args.prior_request = str(staged)
        self.refuse("prior_request_overlaps_host_paths")
        self.args.prior_request = str(path)
        self.recipes("retain")
        for suffix in ("", "-wal", "-shm", "-journal"):
            protected = Path(self.args.recipes_database + suffix)
            protected.write_bytes(original)
            self.args.prior_request = str(protected)
            self.refuse("recipes_database_overlaps_host_paths")
            self.assertEqual(protected.read_bytes(), original)

    def test_prior_request_prepublication_mutation_and_existing_output_are_preserved(self):
        path = self.prior_request()
        original = path.read_bytes()
        recheck = host.recheck_pins
        def changed(pins):
            recheck(pins)
            path.write_bytes(b"changed after validation")
        with patch.object(host, "recheck_pins", changed):
            self.refuse("prior_request_changed")
        path.write_bytes(original)
        self.args.output = str(path)
        with self.assertRaisesRegex(host.PreparationError, "output_or_workspace_exists"):
            host.prepare(self.args)
        self.assertEqual(path.read_bytes(), original)

    def test_generated_mcp_reads_frozen_prior_and_reviews_current_incompatible_fields(self):
        from test_backend import help_source, profiles
        from test_successor import historical
        selection = historical()["selection"]
        selection["operations"].append("removed_operation")
        selection["values"].update(timeout="old-text-type", removed_field="WITHHELD_OLD_VALUE")
        path = self.prior_request(historical(selection))
        original = path.read_bytes()
        # A process-level package-owner fixture supplies current metadata only.
        # The generated MCP launch and the Controller remain the real sources.
        metadata_script = ("import json,sys\nprint(json.dumps(" + repr(help_source("agno"))
                           + " if 'config-help' in sys.argv else " + repr(profiles("agno")) + "))\n")
        installer = """import json, pathlib, sys
def arg(flag): return sys.argv[sys.argv.index(flag) + 1]
destination = pathlib.Path(arg('--destination'))
runtime = destination / 'runtime/bin/python'
runtime.parent.mkdir(parents=True)
runtime.write_text('#!' + sys.executable + '\\n' + METADATA_SCRIPT)
runtime.chmod(0o755)
command = [str(runtime), '-I', '-m', 'meerkat_toolkit_agno.agno_mcp']
(destination / 'installed.json').write_text(json.dumps({
    'family': arg('--family'), 'destination': str(destination),
    'scope': 'observed_software_installation_only',
    'contents_sha256': arg('--contents-sha256'), 'command': command}))
""".replace("METADATA_SCRIPT", repr(metadata_script)).encode()
        self.files["setup/mcp_adapter.py"] = (SOURCE.joinpath("mcp_adapter.py").read_bytes(), 0o644)
        self.files["consumers/install-library-consumer.py"] = (installer, 0o644)
        self.files["consumers/agno/PROFILE-GUIDE.md"] = (b"Current fixture guide\n", 0o644)
        manifest = {name.removeprefix("consumers/"): {"sha256": sha(raw)}
                    for name, (raw, _) in self.files.items()
                    if name.startswith("consumers/") and name != "consumers/CONTENTS.json"}
        self.files["consumers/CONTENTS.json"] = (json.dumps({"files": manifest}).encode(), 0o644)
        self.publish_release()
        self.args.families = "agno"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.args.python = self.args.mcp_python = sys.executable
        self.args.python_sha256 = self.args.mcp_python_sha256 = sha(Path(sys.executable).resolve().read_bytes())
        host.prepare(self.args)
        mcp = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        path.unlink()

        async def exercise():
            from mcp import ClientSession, StdioServerParameters
            from mcp.client.stdio import stdio_client
            async with asyncio.timeout(30):
                async with stdio_client(StdioServerParameters(**mcp)) as (reader, writer):
                    async with ClientSession(reader, writer) as client:
                        await client.initialize()
                        self.assertIn("setup_successor", [t.name for t in (await client.list_tools()).tools])
                        async def call(name, args):
                            result = await client.call_tool(name, args)
                            self.assertFalse(result.is_error, result)
                            return result.structured_content
                        state = await call("setup_state", {})
                        self.assertEqual(state["prior_selection"], {"family": "agno", "profile": "searxng",
                            "request_sha256": self.args.prior_request_sha256})
                        params = {"prior_request_sha256": self.args.prior_request_sha256}
                        self.assertEqual((await call("setup_successor", params))["status"], "preparation_required")
                        job = (await call("setup_prepare", {"family": "agno"}))["job"]
                        while True:
                            state = await call("setup_state", {})
                            current = state["job"]
                            self.assertEqual(current["id"], job["id"])
                            if current["status"] != "running":
                                self.assertEqual(current["status"], "succeeded", current)
                                break
                            await asyncio.sleep(0.02)
                        proposal = await call("setup_successor", params)
                        self.assertEqual(proposal["status"], "review_required")
                        self.assertEqual(proposal["values"], {"host": selection["values"]["host"], "optional_flag": False})
                        self.assertEqual({(item["name"], item["reason"]) for item in proposal["unapplied"]}, {
                            ("timeout", "current_type_differs"), ("removed_field", "not_in_current_profile"),
                            ("removed_operation", "not_in_current_profile")})
                        self.assertNotIn("WITHHELD_OLD_VALUE", json.dumps(proposal))
                        self.assertEqual(proposal["help"]["guide_text"], "Current fixture guide\n")
                        self.assertEqual(state["job"]["kind"], "prepare")
        asyncio.run(exercise())
        self.assertFalse(path.exists())
        self.assertEqual((Path(self.args.workspace) / "prior-request.json").read_bytes(), original)

    def test_plugin_inspection_requires_release_capability_and_explicit_opt_in(self):
        self.args.enable_plugin_inspection = True
        self.refuse("plugin_inspection_not_in_release")
        for malformed in ("agent-plugin-files", {"agent-plugin-files": True}, [1]):
            self.files["selected/provenance/selection.json"] = (json.dumps({
                "platform": "macos-arm64", "scope": "retained_selected_tools",
                "capabilities": malformed,
            }).encode(), 0o644)
            self.publish_release()
            self.refuse("plugin_inspection_not_in_release")
        name = "selected/bin/toolkit-package-inspect"
        self.files[name] = (b"# inert inspector\n", 0o755)
        self.files["selected/provenance/selection.json"] = (json.dumps({
            "platform": "macos-arm64", "scope": "retained_selected_tools",
            "capabilities": ["agent-plugin-files"],
        }).encode(), 0o644)
        self.publish_release()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            result = host.prepare(self.args)
        profile = json.loads((Path(result["directory"]) / "selected-install.json").read_bytes())
        self.assertIs(profile["plugin_inspection"], True)
        self.assertEqual(profile["package_inspector"]["sha256"], sha(self.files[name][0]))
        self.args.output = str(self.base / "without-plugin-inspection")
        self.args.enable_plugin_inspection = False
        result = host.prepare(self.args)
        profile = json.loads((Path(result["directory"]) / "selected-install.json").read_bytes())
        self.assertNotIn("plugin_inspection", profile)

    def test_package_inspector_requires_explicit_opt_in_and_pins_archive_tool(self):
        name = "selected/bin/toolkit-package-inspect"
        self.files[name] = (b"# inert package inspector, never executed\n", 0o755)
        self.publish_release()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        profile = json.loads((Path(self.args.output) / "selected-install.json").read_bytes())
        self.assertNotIn("package_inspector", profile)
        self.args.output = str(self.base / "with-inspector")
        self.args.enable_package_inspection = True
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        output = Path(self.args.output)
        profile = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(profile["package_inspector"], {
            "path": str(self.release / name), "sha256": sha(self.files[name][0])})
        self.assertNotIn("package_inspector", profile["validators"])
        self.assertFalse((output / "python-acquisition.json").exists())
        self.assertFalse((output / "node-acquisition.json").exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_missing_or_nonexecutable_package_inspector_refuses_before_output(self):
        self.args.enable_package_inspection = True
        self.refuse("package_inspector_not_in_release")
        self.files["selected/bin/toolkit-package-inspect"] = (b"# inert inspector\n", 0o644)
        self.publish_release()
        with self.assertRaisesRegex(Exception, "invalid_selected_host_profile"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_acquired_inspection_is_explicit_and_does_not_start_a_worker(self):
        self.acquire()
        self.files["setup/acquired_inspection.py"] = (b"# trusted inert worker, not executed\n", 0o644)
        self.publish_release()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        self.assertNotIn("--allow-acquired-inspection", (Path(self.args.output) / "mcp.json").read_text())
        self.args.output = str(self.base / "with-acquired-inspection")
        self.args.enable_acquired_inspection = True
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        entry = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        self.assertEqual(entry["args"].count("--allow-acquired-inspection"), 1)
        self.assertFalse(Path(self.args.workspace).exists())
        self.assertFalse(list(self.release.rglob("__pycache__")))

    def test_acquired_inspection_requires_acquisition_and_shipped_worker(self):
        self.args.enable_acquired_inspection = True
        self.refuse("acquired_inspection_requires_acquisition")
        self.acquire()
        self.refuse("acquired_inspection_not_in_release")

    def test_no_implicit_network_scope_or_family_removal(self):
        host.prepare(self.args)
        output = Path(self.args.output)
        self.assertFalse((output / "python-acquisition.json").exists())
        self.assertNotIn("--python-acquisition-profile", (output / "mcp.json").read_text())
        self.args.output = str(self.base / "missing-node")
        self.args.node = None
        self.refuse("activepieces_runtime_required")

    def official_release(self, components=("github", "playwright")):
        delivery = {"format": "meerkat-toolkit.official-consumers.v1",
                    "catalog_ids": list(components),
                    "components": [{"catalog_id": name, "target": self.args.target}
                                   for name in components]}
        self.files.update({
            "official/CONTENTS.json": (b'{"format":1,"files":{}}\n', 0o644),
            "official/delivery.json": (json.dumps(delivery).encode(), 0o644),
            "setup/install-library-consumer.py": (b"# inert official installer\n", 0o644),
        })
        self.publish_release()

    def playwright_profile(self):
        path = self.base / "host playwright.json"
        # Preparation preserves the host choice; the installer checks these
        # paths and browser/runtime compatibility when explicitly invoked.
        path.write_text(json.dumps({"schema_version": 1,
            "chrome_executable": "/host-selected/chrome", "chrome_sha256": "a" * 64,
            "process_parent": "/host-selected/processes"}, indent=2) + "\n")
        self.args.playwright_host_profile = str(path)
        self.args.playwright_host_profile_sha256 = sha(path.read_bytes())
        return path

    def prepared_arguments(self):
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        return json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]

    def recipes(self, scope="read"):
        self.files["selected/bin/toolkit-local-recipes"] = (b"# inert recipes owner, never executed\n", 0o755)
        self.publish_release()
        database = self.base / "recipes.sqlite3"
        # The helper checks paths, not SQLite contents or initialization.
        database.write_bytes(b"host-owned database; helper must not decode this\n")
        self.args.recipes_database = str(database)
        self.args.recipes_realm, self.args.recipes_owner = "host-realm", "host-owner"
        self.args.recipes_scope = scope
        return database

    def test_recipes_are_omitted_by_default_even_with_shipped_tool(self):
        self.recipes()
        self.args.recipes_database = self.args.recipes_realm = self.args.recipes_owner = self.args.recipes_scope = None
        argv = self.prepared_arguments()
        self.assertFalse(any("recipes" in arg for arg in argv))
        self.assertFalse((Path(self.args.output) / "local-recipes.json").exists())

    def test_recipe_scope_profile_and_remote_flags_without_database_effects(self):
        database = self.recipes()
        sidecars = [Path(str(database) + suffix) for suffix in ("-wal", "-shm", "-journal")]
        for path in sidecars:
            path.write_bytes(b"host-owned sidecar\n")
        original = {path: (path.read_bytes(), path.stat().st_mode, path.stat().st_mtime_ns)
                    for path in [database, *sidecars]}
        for scope, remote, expected in (
            ("read", False, "disclose_nonsecret_local_recipe_metadata"),
            ("retain", False, "retain_and_disclose_nonsecret_local_recipe_metadata"),
            ("retain", True, "retain_and_disclose_nonsecret_local_recipe_metadata"),
        ):
            with self.subTest(scope=scope, remote=remote):
                self.args.output = str(self.base / (scope + str(remote)))
                self.args.recipes_scope, self.args.allow_remote_recipes = scope, remote
                with patch("sqlite3.connect", side_effect=AssertionError("no database open allowed")):
                    argv = self.prepared_arguments()
                output = Path(self.args.output)
                raw = (output / "local-recipes.json").read_bytes()
                self.assertEqual(json.loads(raw), {"schema_version": 1, "scope": expected,
                    "database": str(database), "realm": "host-realm", "owner": "host-owner",
                    "tool": {"path": str(self.release / "selected/bin/toolkit-local-recipes"),
                             "sha256": sha(self.files["selected/bin/toolkit-local-recipes"][0])}})
                self.assertEqual(argv.count("--local-recipes-profile"), 1)
                self.assertEqual(argv[argv.index("--local-recipes-profile") + 1], str(output / "local-recipes.json"))
                self.assertEqual(argv[argv.index("--local-recipes-profile-sha256") + 1], sha(raw))
                self.assertEqual("--allow-remote-recipes" in argv, remote)
                self.assertEqual((output / "local-recipes.json").stat().st_mode & 0o777, 0o600)
                self.assertEqual(json.loads((output / "prepared.json").read_bytes())["files"]["local-recipes.json"], sha(raw))
                self.assertFalse(Path(self.args.workspace).exists())
                self.assertFalse(list(self.release.rglob("__pycache__")))
                for path, value in original.items():
                    self.assertEqual((path.read_bytes(), path.stat().st_mode, path.stat().st_mtime_ns), value)

    def test_recipe_inputs_and_remote_retention_are_explicit(self):
        self.recipes()
        for name in ("recipes_database", "recipes_realm", "recipes_owner", "recipes_scope"):
            with self.subTest(missing=name):
                original = getattr(self.args, name)
                setattr(self.args, name, None)
                self.refuse("complete_recipes_profile_required")
                setattr(self.args, name, original)
        self.args.recipes_scope = "write"
        self.refuse("invalid_recipes_scope")
        self.args.recipes_scope, self.args.allow_remote_recipes = "read", True
        self.refuse("remote_recipes_require_retention")
        self.args.recipes_database = self.args.recipes_realm = self.args.recipes_owner = self.args.recipes_scope = None
        self.refuse("remote_recipes_require_retention")

    def test_recipe_profile_owner_rejects_invalid_principal_or_database(self):
        database = self.recipes()
        for name, value in (("recipes_realm", ""), ("recipes_owner", "x" * 257),
                            ("recipes_owner", "host\x00owner"), ("recipes_database", "relative.sqlite3"),
                            ("recipes_database", str(self.base / "missing.sqlite3")),
                            ("recipes_database", str(self.base))):
            with self.subTest(name=name, value=value):
                original = getattr(self.args, name)
                setattr(self.args, name, value)
                self.refuse("invalid_local_recipes_profile")
                setattr(self.args, name, original)
        alias = self.base / "database-alias"
        alias.symlink_to(database)
        self.args.recipes_database = str(alias)
        self.refuse("invalid_local_recipes_profile")
        alias.unlink()
        alias.symlink_to(self.base, target_is_directory=True)
        self.args.recipes_database = str(alias / database.name)
        self.refuse("invalid_local_recipes_profile")

    def test_recipe_tool_requires_shipped_executable_and_exact_archive_bytes(self):
        self.recipes()
        name = "selected/bin/toolkit-local-recipes"
        raw, mode = self.files.pop(name)
        (self.release / name).unlink()
        for target in host.TARGETS:
            with self.subTest(target=target):
                self.args.target, self.root_name = target, "toolkit-library-fixture-" + target
                self.files["selected/provenance/selection.json"] = (json.dumps({
                    "platform": target, "scope": "retained_python_tools_only"}).encode(), 0o644)
                self.publish_release()
                self.refuse("local_recipes_not_in_release")
        self.files[name] = (raw, 0o644)
        self.publish_release()
        self.refuse("invalid_local_recipes_profile")
        self.files[name] = (raw, mode)
        self.publish_release()
        (self.release / name).write_bytes(b"different owner")
        self.refuse("extracted_release_changed")
        self.publish_release()
        (self.release / name).chmod(0o644)
        self.refuse("extracted_release_changed")

    def test_recipe_database_and_future_sidecars_cannot_overlap_host_directories(self):
        database = self.recipes()
        for directory in (self.release, self.staging):
            with self.subTest(directory=directory):
                # Use an existing release member, so archive validation itself stays valid.
                self.args.recipes_database = str(directory / ("licenses/notice.txt" if directory == self.release else "db"))
                if directory == self.staging:
                    Path(self.args.recipes_database).write_bytes(b"db")
                self.refuse("recipes_database_overlaps_host_paths")
        self.args.recipes_database = str(database)
        for name in ("output", "workspace"):
            original = getattr(self.args, name)
            for suffix in ("-wal", "-shm", "-journal"):
                with self.subTest(name=name, suffix=suffix):
                    setattr(self.args, name, str(database) + suffix)
                    self.refuse("recipes_database_overlaps_host_paths")
            setattr(self.args, name, original)

    def test_recipe_sidecar_case_and_unicode_aliases_refuse_before_publication(self):
        original = self.recipes("retain").read_bytes()
        for spelling, alias in (("Recipes.sqlite3", "recipes.sqlite3"),
                                ("caf\u00e9.sqlite3", "cafe\u0301.sqlite3")):
            for name in ("output", "workspace"):
                for suffix in ("-wal", "-shm", "-journal"):
                    with self.subTest(spelling=spelling, name=name, suffix=suffix):
                        directory = self.base / (spelling + name + suffix)
                        directory.mkdir()
                        database = directory / spelling
                        database.write_bytes(original)
                        self.args.recipes_database = str(database)
                        self.args.output = str(directory / "host-config")
                        self.args.workspace = str(directory / "future-workspace")
                        setattr(self.args, name, str(directory / (alias + suffix)))
                        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
                            self.refuse("recipes_database_overlaps_host_paths")
                        self.assertEqual(database.read_bytes(), original)
                        for reserved in ("-wal", "-shm", "-journal"):
                            self.assertFalse(Path(str(database) + reserved).exists())

    def test_recipe_sidecar_similar_sibling_names_remain_allowed(self):
        original = self.recipes("retain").read_bytes()
        for spelling, alias in (("Recipes.sqlite3", "recipes.sqlite3"),
                                ("caf\u00e9.sqlite3", "cafe\u0301.sqlite3")):
            with self.subTest(spelling=spelling):
                directory = self.base / ("sibling-" + spelling)
                directory.mkdir()
                database = directory / spelling
                database.write_bytes(original)
                self.args.recipes_database = str(database)
                self.args.output = str(directory / (alias + "-wal-backup"))
                self.args.workspace = str(directory / (alias + "-shm-backup"))
                self.prepared_arguments()
                self.assertTrue((Path(self.args.output) / "prepared.json").is_file())
                self.assertFalse(Path(self.args.workspace).exists())
                self.assertEqual(database.read_bytes(), original)
                for suffix in ("-wal", "-shm", "-journal"):
                    self.assertFalse(Path(str(database) + suffix).exists())

    def test_recipe_sidecar_aliases_and_late_database_change_refuse(self):
        database = self.recipes()
        sidecar = Path(str(database) + "-wal")
        sidecar.symlink_to(self.release / "licenses/notice.txt")
        self.refuse("invalid_recipes_database_sidecar")
        sidecar.unlink()
        sidecar.mkdir()
        self.refuse("invalid_recipes_database_sidecar")
        sidecar.rmdir()
        recheck = host.recheck_pins
        def replace_database(pins):
            recheck(pins)
            database.unlink()
            database.symlink_to(self.pip)
        with patch.object(host, "recheck_pins", replace_database):
            self.refuse("invalid_local_recipes_profile")

    def test_recipes_compose_with_all_families_and_acquisition_via_cli(self):
        self.recipes("retain")
        self.args.allow_remote_recipes = True
        self.official_release()
        self.playwright_profile()
        self.npm_acquisition()
        self.acquire()
        self.args.families = "agno,services,activepieces,official-github,official-playwright"
        arguments = []
        for key, value in vars(self.args).items():
            if value is not None and value is not False:
                arguments.append("--" + key.replace("_", "-"))
                if value is not True:
                    arguments.append(str(value))
        with redirect_stdout(io.StringIO()), patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            self.assertEqual(host.main(arguments), 0)
        argv = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        for flag, value in (("--families", self.args.families), ("--node", self.args.node),
                            ("--selected-node", self.args.selected_node),
                            ("--official-installer", str(self.release / "setup/install-library-consumer.py"))):
            self.assertEqual(argv[argv.index(flag) + 1], value)
        for name in ("selected-install", "local-recipes", "python-acquisition", "node-acquisition", "playwright-host"):
            self.assertTrue((Path(self.args.output) / (name + ".json")).is_file())

    def test_official_payload_does_not_enable_families_by_default(self):
        self.official_release()
        argv = self.prepared_arguments()
        self.assertEqual(argv[argv.index("--families") + 1], "agno,services,activepieces")
        self.assertFalse(any(arg.startswith("--official-") or arg.startswith("--playwright-") for arg in argv))
        self.assertEqual(argv[argv.index("--verifier") + 1], str(self.release / "consumers/install-library-consumer.py"))
        self.assertEqual(argv[argv.index("--verifier-sha256") + 1], sha(self.files["consumers/install-library-consumer.py"][0]))

    def test_github_uses_archived_official_bindings_on_both_targets(self):
        self.args.families = "official-github"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        for target in host.TARGETS:
            with self.subTest(target=target):
                self.args.target = target
                self.args.output = str(self.base / ("github-" + target))
                self.root_name = "toolkit-library-fixture-" + target
                self.files["selected/provenance/selection.json"] = (json.dumps({
                    "platform": target, "scope": "retained_python_tools_only"}).encode(), 0o644)
                self.official_release(("github",))
                argv = self.prepared_arguments()
                for flag, value in {
                    "--families": "official-github", "--official-bundle": str(self.release / "official"),
                    "--official-contents-sha256": sha(self.files["official/CONTENTS.json"][0]),
                    "--official-installer": str(self.release / "setup/install-library-consumer.py"),
                    "--official-installer-sha256": sha(self.files["setup/install-library-consumer.py"][0]),
                    "--verifier": str(self.release / "setup/install-library-consumer.py"),
                    "--verifier-sha256": sha(self.files["setup/install-library-consumer.py"][0]),
                }.items():
                    self.assertEqual(argv.count(flag), 1)
                    self.assertEqual(argv[argv.index(flag) + 1], value)
                self.assertNotIn("--node", argv)
                self.assertNotIn("--playwright-host-profile", argv)
                self.assertFalse(Path(self.args.workspace).exists())

    def test_playwright_preserves_profile_bytes_and_uses_library_node_pair(self):
        self.official_release()
        self.selected_node()
        self.args.families = "agno,services,activepieces,official-github,official-playwright"
        original = self.playwright_profile().read_bytes()
        argv = self.prepared_arguments()
        output = Path(self.args.output)
        self.assertEqual((output / "playwright-host.json").read_bytes(), original)
        self.assertEqual(argv[argv.index("--playwright-host-profile") + 1], str(output / "playwright-host.json"))
        self.assertEqual(argv[argv.index("--playwright-host-profile-sha256") + 1], sha(original))
        self.assertEqual(argv[argv.index("--node") + 1], self.args.node)
        self.assertEqual(argv[argv.index("--npm-cli") + 1], self.args.npm_cli)
        self.assertEqual(argv[argv.index("--selected-node") + 1], self.args.selected_node)
        self.assertNotEqual(self.args.node, self.args.selected_node)
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(receipt["files"]["playwright-host.json"], sha(original))
        self.assertEqual((output / "playwright-host.json").stat().st_mode & 0o777, 0o600)
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(selected["installer"]["path"], str(self.release / "selected/bin/install-library-consumer.py"))
        self.assertFalse(Path(self.args.workspace).exists())

    def test_historical_macos_target_and_cli_official_options(self):
        self.official_release()
        delivery = json.loads(self.files["official/delivery.json"][0])
        for row in delivery["components"]:
            del row["target"]
        self.files["official/delivery.json"] = (json.dumps(delivery).encode(), 0o644)
        self.publish_release()
        self.args.families = "official-playwright"
        self.playwright_profile()
        arguments = []
        for key, value in vars(self.args).items():
            if value is not None and value is not False:
                arguments.extend(["--" + key.replace("_", "-"), str(value)])
        with redirect_stdout(io.StringIO()), patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            self.assertEqual(host.main(arguments), 0)
        output = io.StringIO()
        with redirect_stdout(output), self.assertRaises(SystemExit) as caught:
            host.main(["--help"])
        self.assertEqual(caught.exception.code, 0)
        for family in ("agno", "services", "activepieces", "official-github", "official-playwright"):
            self.assertIn(family, output.getvalue())

    def test_each_official_binding_is_required_and_unknown_family_refuses(self):
        self.official_release()
        self.args.families += ",official-github"
        for name in host.OFFICIAL_FILES:
            with self.subTest(name=name):
                original = self.files.pop(name)
                (self.release / name).unlink()
                self.publish_release()
                self.refuse("official_setup_not_in_release")
                self.files[name] = original
                self.publish_release()
        self.args.families = "official-unknown"
        self.refuse("invalid_library_families")

    def test_official_selection_requires_shipped_component_and_supported_target(self):
        self.args.families += ",official-github"
        self.refuse("official_setup_not_in_release")
        self.official_release(("playwright",))
        self.refuse("official_family_not_in_release")
        self.official_release()
        delivery = json.loads(self.files["official/delivery.json"][0])
        delivery["components"][0]["target"] = "linux-x86_64"
        self.files["official/delivery.json"] = (json.dumps(delivery).encode(), 0o644)
        self.publish_release()
        self.refuse("official_target_mismatch")
        self.args.families = "official-playwright"
        self.args.target = "linux-x86_64"
        self.root_name = "toolkit-library-fixture-linux-x86_64"
        self.files["selected/provenance/selection.json"] = (json.dumps({
            "platform": "linux-x86_64", "scope": "retained_python_tools_only"}).encode(), 0o644)
        self.official_release(("github",))
        self.refuse("official_family_not_in_release")

    def test_playwright_profile_and_runtime_are_explicit(self):
        self.official_release()
        self.args.families = "official-playwright"
        self.refuse("playwright_host_profile_required")
        profile = self.playwright_profile()
        self.args.playwright_host_profile_sha256 = None
        self.refuse("playwright_host_profile_required")
        self.args.playwright_host_profile_sha256 = sha(profile.read_bytes())
        self.args.node = None
        self.refuse("playwright_runtime_required")
        self.args.node = str(self.runtime)
        self.args.families = "agno,services,activepieces"
        self.refuse("unused_playwright_host_profile")

    def test_playwright_profile_hash_shape_and_bounds_refuse_before_output(self):
        self.official_release()
        self.args.families = "official-playwright"
        profile = self.playwright_profile()
        self.args.playwright_host_profile_sha256 = "0" * 64
        self.refuse("playwright_host_profile_changed")
        original = json.loads(profile.read_bytes())
        wrong_version = {**original, "schema_version": True}
        for payload, error in (
            (b'{"schema_version":1}', "invalid_request"),
            (b'{"schema_version":1,"schema_version":1}', "invalid_json"),
            (json.dumps(wrong_version).encode(), "invalid_playwright_host_profile"),
            (b"x" * (64 * 1024 + 1), "invalid_input_file"),
        ):
            with self.subTest(size=len(payload)):
                profile.write_bytes(payload)
                self.args.playwright_host_profile_sha256 = sha(payload)
                with self.assertRaisesRegex(Exception, error):
                    host.prepare(self.args)
                self.assertFalse(Path(self.args.output).exists())
                self.assertFalse(Path(self.args.workspace).exists())

    def test_official_archive_and_late_profile_mutations_refuse(self):
        self.official_release()
        self.args.families = "official-playwright"
        profile = self.playwright_profile()
        for name in ("official/CONTENTS.json", "official/delivery.json", "setup/install-library-consumer.py"):
            with self.subTest(name=name):
                (self.release / name).write_bytes(b"changed")
                self.refuse("extracted_release_changed")
                self.publish_release()
        original_recheck = host.recheck_pins
        def change_profile(pins):
            original_recheck(pins)
            profile.write_bytes(b"changed after validation")
        with patch.object(host, "recheck_pins", change_profile):
            self.refuse("playwright_host_profile_changed")

    def selected_node(self):
        self.files["setup/node_acquisition.py"] = ((SOURCE / "node_acquisition.py").read_bytes(), 0o644)
        self.files["selected/bin/toolkit-node-plan"] = (b"# inert selected Node planner\n", 0o755)
        self.files["selected/provenance/selection.json"] = (json.dumps({"platform": self.args.target,
            "scope": "retained_selected_tools"}).encode(), 0o644)
        self.publish_release()
        node = self.base / "selected-node"
        node.write_bytes(b"inert distinct selected Node\n")
        node.chmod(0o755)
        npm = self.base / "selected-npm.js"
        npm.write_bytes(b"// inert distinct selected npm\n")
        npm.chmod(0o755)
        self.args.selected_node, self.args.selected_node_sha256 = str(node), sha(node.read_bytes())
        self.args.selected_npm_cli, self.args.selected_npm_cli_sha256 = str(npm), sha(npm.read_bytes())

    def test_node_build_transport_options_are_absent_or_exact_bounded_regular_bytes(self):
        self.assertIsNone(host.node_build_options(self.args))
        profile = self.base / "build-profile.json"
        raw = b'{ "external_tool": "/host/selected/tool" }\n'
        profile.write_bytes(raw)
        expected = sha(raw)
        self.args.node_build_profile = str(profile)
        self.args.node_build_profile_sha256 = expected
        self.assertEqual(host.node_build_options(self.args), (profile, expected, raw))
        for path, digest in ((str(profile), None), (None, expected),
                             (str(profile), "0" * 64), ("relative.json", expected)):
            with self.subTest(path=path, digest=digest):
                self.args.node_build_profile, self.args.node_build_profile_sha256 = path, digest
                with self.assertRaises((host.PreparationError, OSError)):
                    host.node_build_options(self.args)
        link = self.base / "build-profile-link.json"
        link.symlink_to(profile)
        self.args.node_build_profile, self.args.node_build_profile_sha256 = str(link), expected
        with self.assertRaises(OSError): host.node_build_options(self.args)
        self.args.node_build_profile = str(profile)
        for raw in (b"", b"x" * 65537):
            profile.write_bytes(raw)
            self.args.node_build_profile_sha256 = sha(raw)
            with self.assertRaises(host.PreparationError): host.node_build_options(self.args)

    def test_node_build_profile_cannot_enable_node_or_network_implicitly(self):
        profile = self.base / "build-profile.json"
        profile.write_bytes(b'{}\n')
        self.args.node_build_profile, self.args.node_build_profile_sha256 = str(profile), sha(profile.read_bytes())
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            self.refuse("node_build_requires_selected_node_validator")
        self.assertFalse(self.args.allow_public_npm)
        self.assertFalse(self.args.allow_public_pypi)

    def node_build_profile(self):
        # Transport-only JSON. Installer schema validation is a separate
        # owner boundary before a selected build can be reviewed or executed.
        path = self.base / "build-profile.json"
        path.write_bytes(b'{ "scope": "transport_fixture", "external_tool": "/host/selected/tool" }\n')
        self.args.node_build_profile = str(path)
        self.args.node_build_profile_sha256 = sha(path.read_bytes())
        return path

    def test_node_build_snapshot_preserves_bytes_and_does_not_enable_acquisition(self):
        self.selected_node()
        source = self.node_build_profile()
        original = source.read_bytes()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            result = host.prepare(self.args)
        output = Path(result["directory"])
        receipt = json.loads((output / "prepared.json").read_bytes())
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual((output / "node-build-profile.json").read_bytes(), original)
        self.assertEqual(selected["node_build"], {"path": str(output / "node-build-profile.json"), "sha256": sha(original)})
        self.assertEqual(receipt["files"]["node-build-profile.json"], sha(original))
        self.assertEqual(receipt["runtimes"]["node_build_profile"]["sha256"], sha(original))
        self.assertEqual(set(selected["validators"]), {"python", "node"})
        self.assertFalse(receipt["public_npm_enabled"])
        self.assertFalse(receipt["public_pypi_enabled"])
        self.assertFalse(Path(self.args.workspace).exists())
        source.write_bytes(b"later external mutation")
        self.assertEqual((output / "node-build-profile.json").read_bytes(), original)

    def test_node_build_mutation_before_publication_retains_no_success(self):
        self.selected_node()
        source = self.node_build_profile()
        recheck = host.recheck_pins
        def mutate(pins):
            recheck(pins)
            source.write_bytes(b"changed after input-pin recheck")
        with patch.object(host, "recheck_pins", side_effect=mutate):
            self.refuse("node_build_profile_changed")

    def test_node_build_profile_overlap_refuses_even_through_parent_alias(self):
        profile = self.staging / "build-profile.json"
        profile.write_bytes(b'{}\n')
        alias = self.base / "staging-alias"
        alias.symlink_to(self.staging, target_is_directory=True)
        self.args.node_build_profile = str(alias / profile.name)
        self.args.node_build_profile_sha256 = sha(profile.read_bytes())
        self.refuse("node_build_profile_overlaps_host_paths")
        self.assertEqual(profile.read_bytes(), b'{}\n')

    def mcpb(self, *, inspection=False):
        self.files[host.MCPB_TOOL] = (b"# inert MCPB owner, never executed\n", 0o755)
        selection = {"platform": self.args.target, "scope": "retained_selected_tools"}
        if inspection:
            selection["capabilities"] = ["mcpb-archive-inspection"]
            self.files["selected/bin/toolkit-package-inspect"] = (b"# inert inspector\n", 0o755)
        self.files["selected/provenance/selection.json"] = (json.dumps(selection).encode(), 0o644)
        self.args.allow_mcpb = True
        self.args.enable_mcpb_inspection = inspection
        if self.args.selected_node is None:
            self.args.selected_node = str(self.runtime)
            self.args.selected_node_sha256 = sha(self.runtime.read_bytes())
        self.publish_release()

    def test_mcpb_node_only_profile_and_launch_are_explicit(self):
        self.mcpb()
        self.args.families = "services"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        argv = self.prepared_arguments()
        output = Path(self.args.output)
        profile = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(set(profile["validators"]), {"python", "mcpb"})
        self.assertEqual(profile["validators"]["mcpb"], {
            "path": str(self.release / host.MCPB_TOOL), "sha256": sha(self.files[host.MCPB_TOOL][0])})
        self.assertEqual(argv[argv.index("--selected-node") + 1], self.args.selected_node)
        for absent in ("--node", "--npm-cli", "--selected-npm-cli", "--node-acquisition-profile",
                       "--python-acquisition-profile", "--allow-mcpb"):
            self.assertNotIn(absent, argv)
        self.assertNotIn("mcpb_inspection", profile)
        self.assertNotIn("package_inspector", profile)
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(set(receipt["runtimes"]), {"python", "mcp_python", "selected_node"})
        self.assertEqual(receipt["runtimes"]["selected_node"]["sha256"], self.args.selected_node_sha256)
        self.assertFalse(receipt["public_npm_enabled"])
        self.assertFalse(receipt["public_pypi_enabled"])
        self.assertFalse(Path(self.args.workspace).exists())

    def test_generated_mcp_starts_with_explicit_node_only_runtime(self):
        self.mcpb(inspection=True)
        self.files["setup/mcp_adapter.py"] = ((SOURCE / "mcp_adapter.py").read_bytes(), 0o644)
        self.files["consumers/services/PROFILE-GUIDE.md"] = (b"Inert services guide\n", 0o644)
        files = {name.removeprefix("consumers/"): {"sha256": sha(raw)}
                 for name, (raw, _) in self.files.items()
                 if name.startswith("consumers/") and name != "consumers/CONTENTS.json"}
        self.files["consumers/CONTENTS.json"] = (json.dumps({"files": files}).encode(), 0o644)
        self.publish_release()
        self.args.families = "services"
        self.args.node = self.args.node_sha256 = self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.args.python = self.args.mcp_python = sys.executable
        self.args.python_sha256 = self.args.mcp_python_sha256 = sha(Path(sys.executable).resolve().read_bytes())
        host.prepare(self.args)
        entry = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        self.assertIn("--selected-node", entry["args"])
        self.assertNotIn("--selected-npm-cli", entry["args"])

        async def exercise():
            from mcp import ClientSession, StdioServerParameters
            from mcp.client.stdio import stdio_client
            async with asyncio.timeout(20):
                async with stdio_client(StdioServerParameters(**entry)) as (reader, writer):
                    async with ClientSession(reader, writer) as client:
                        await client.initialize()
                        tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                        self.assertIn("setup_inspect_mcpb", tools)
                        schema = tools["setup_selected_check"].input_schema
                        self.assertEqual(set(schema["properties"]["profile"]["enum"]), {"python", "mcpb"})
                        result = await client.call_tool("setup_state", {})
                        self.assertFalse(result.is_error, result)
                        state = result.structured_content
                        self.assertEqual(set(state["selected_installation"]["profiles"]), {"python", "mcpb"})
                        self.assertTrue(state["mcpb_inspection"]["available"])
                        self.assertIsNone(state["job"])
                        self.assertEqual(state["job_history"]["count"], 0)
        asyncio.run(exercise())
        self.assertEqual((Path(self.args.workspace) / "selected-host-profile.json").read_bytes(),
                         (Path(self.args.output) / "selected-install.json").read_bytes())
        self.assertEqual(list((Path(self.args.workspace) / "selections").iterdir()), [])

    def test_shipped_mcpb_does_not_change_default_profile(self):
        self.mcpb(inspection=True)
        self.args.allow_mcpb = self.args.enable_mcpb_inspection = False
        self.args.selected_node = self.args.selected_node_sha256 = None
        argv = self.prepared_arguments()
        profile = json.loads((Path(self.args.output) / "selected-install.json").read_bytes())
        self.assertEqual(set(profile), {"schema_version", "scope", "staging_root", "installer", "validators"})
        self.assertEqual(set(profile["validators"]), {"python"})
        self.assertNotIn("--selected-node", argv)

    def test_mcpb_requires_explicit_complete_runtime_not_library_fallback(self):
        self.mcpb()
        node, expected = self.args.selected_node, self.args.selected_node_sha256
        for value, digest, code in ((None, None, "mcpb_runtime_required"),
                                   (node, None, "selected_node_runtime_pair_required"),
                                   (None, expected, "selected_node_runtime_pair_required"),
                                   ("", expected, "selected_node_runtime_pair_required"),
                                   (node, "", "selected_node_runtime_pair_required")):
            with self.subTest(value=value, digest=digest):
                self.args.selected_node, self.args.selected_node_sha256 = value, digest
                self.refuse(code)
        self.args.selected_node, self.args.selected_node_sha256 = node, expected
        self.args.allow_mcpb = False
        self.refuse("selected_node_runtime_pair_required")

    def test_mcpb_requires_shipped_executable_and_matching_runtime(self):
        self.mcpb()
        payload = self.files.pop(host.MCPB_TOOL)
        (self.release / host.MCPB_TOOL).unlink()
        self.publish_release()
        self.refuse("mcpb_not_in_release")
        self.files[host.MCPB_TOOL] = (payload[0], 0o644)
        self.publish_release()
        with self.assertRaisesRegex(Exception, "invalid_selected_host_profile"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        self.files[host.MCPB_TOOL] = payload
        self.publish_release()
        (self.release / host.MCPB_TOOL).write_bytes(b"changed owner")
        self.refuse("extracted_release_changed")
        self.publish_release()
        self.args.selected_node_sha256 = "0" * 64
        self.refuse("host_input_changed")

    def test_mcpb_does_not_relax_selected_npm_or_public_acquisition(self):
        self.mcpb()
        for cli, expected in ((str(self.pip), None), (None, sha(self.pip.read_bytes())),
                              ("", sha(self.pip.read_bytes()))):
            with self.subTest(cli=cli):
                self.args.selected_npm_cli, self.args.selected_npm_cli_sha256 = cli, expected
                self.refuse("selected_node_runtime_pair_required")
        self.args.selected_npm_cli = self.args.selected_npm_cli_sha256 = None
        self.args.allow_public_npm = True
        self.args.npm_archive, self.args.npm_archive_sha256 = str(self.pip), sha(self.pip.read_bytes())
        self.refuse("explicit_npm_scope_required")

    def test_mcpb_and_selected_node_acquisition_coexist(self):
        self.npm_acquisition()
        self.mcpb()
        argv = self.prepared_arguments()
        output = Path(self.args.output)
        profile = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(set(profile["validators"]), {"python", "node", "mcpb"})
        self.assertEqual(argv.count("--selected-node"), 1)
        self.assertEqual(argv[argv.index("--selected-npm-cli") + 1], self.args.selected_npm_cli)
        acquisition = json.loads((output / "node-acquisition.json").read_bytes())
        self.assertEqual(acquisition["node_sha256"], self.args.selected_node_sha256)

    def test_mcpb_inspection_cli_pins_capable_inspector_without_plugin_scope(self):
        self.mcpb(inspection=True)
        arguments = []
        for key, value in vars(self.args).items():
            if value is not None and value is not False:
                arguments.append("--" + key.replace("_", "-"))
                if value is not True:
                    arguments.append(str(value))
        with redirect_stdout(io.StringIO()), patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            self.assertEqual(host.main(arguments), 0)
        profile = json.loads((Path(self.args.output) / "selected-install.json").read_bytes())
        self.assertIs(profile["mcpb_inspection"], True)
        inspector = "selected/bin/toolkit-package-inspect"
        self.assertEqual(profile["package_inspector"], {
            "path": str(self.release / inspector), "sha256": sha(self.files[inspector][0])})
        self.assertNotIn("plugin_inspection", profile)
        self.assertNotIn("node", profile["validators"])
        self.assertFalse(Path(self.args.workspace).exists())

    def test_mcpb_inspection_requires_install_scope_and_release_capability(self):
        self.args.enable_mcpb_inspection = True
        self.refuse("mcpb_inspection_requires_mcpb")
        self.mcpb(inspection=True)
        for capabilities in ([], "mcpb-archive-inspection", {"mcpb-archive-inspection": True}, [1]):
            with self.subTest(capabilities=capabilities):
                self.files["selected/provenance/selection.json"] = (json.dumps({
                    "platform": self.args.target, "scope": "retained_selected_tools",
                    "capabilities": capabilities}).encode(), 0o644)
                self.publish_release()
                self.refuse("mcpb_inspection_not_in_release")
        self.mcpb(inspection=True)
        del self.files["selected/bin/toolkit-package-inspect"]
        (self.release / "selected/bin/toolkit-package-inspect").unlink()
        self.publish_release()
        self.refuse("package_inspector_not_in_release")

    def test_mcpb_flags_reject_non_boolean_values(self):
        for name, diagnostic in (("allow_mcpb", "invalid_mcpb_choice"),
                                 ("enable_mcpb_inspection", "invalid_mcpb_inspection_choice")):
            for value in (1, "true", None):
                with self.subTest(name=name, value=value):
                    setattr(self.args, name, value)
                    self.refuse(diagnostic)
            setattr(self.args, name, False)

    def npm_acquisition(self):
        self.selected_node()
        npm = self.base / "npm.tgz"
        with tarfile.open(npm, "w:gz") as archive:
            for name, raw in {"package.json": b'{"name":"npm","version":"12.2.0"}',
                              "bin/npm-cli.js": b"// inert trusted archive entry\n"}.items():
                info = tarfile.TarInfo("package/" + name)
                info.size, info.mode = len(raw), 0o644
                archive.addfile(info, io.BytesIO(raw))
        self.args.allow_public_npm = True
        self.args.npm_archive, self.args.npm_archive_sha256 = str(npm), sha(npm.read_bytes())

    def test_distinct_selected_pair_keeps_library_pair_and_network_disabled(self):
        self.selected_node()
        host.prepare(self.args)
        output = Path(self.args.output)
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--node") + 1], self.args.node)
        self.assertEqual(argv[argv.index("--selected-node") + 1], self.args.selected_node)
        self.assertEqual(argv[argv.index("--npm-cli") + 1], self.args.npm_cli)
        self.assertEqual(argv[argv.index("--selected-npm-cli") + 1], self.args.selected_npm_cli)
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertEqual(set(selected["validators"]), {"python", "node"})
        self.assertFalse((output / "node-acquisition.json").exists())
        self.assertFalse((output / "python-acquisition.json").exists())

    def test_public_npm_opt_in_uses_selected_node_and_owner_validation(self):
        self.npm_acquisition()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no child allowed")):
            host.prepare(self.args)
        output = Path(self.args.output)
        acquisition = json.loads((output / "node-acquisition.json").read_bytes())
        self.assertEqual(acquisition["node_sha256"], self.args.selected_node_sha256)
        self.assertNotEqual(acquisition["node_sha256"], self.args.node_sha256)
        self.assertTrue(json.loads((output / "prepared.json").read_bytes())["public_npm_enabled"])
        self.assertFalse(Path(self.args.workspace).exists())
        self.args.output = str(self.base / "bad-npm")
        Path(self.args.npm_archive).write_bytes(b"not an npm distribution")
        self.args.npm_archive_sha256 = sha(Path(self.args.npm_archive).read_bytes())
        with self.assertRaisesRegex(Exception, "invalid_node_acquisition_profile"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())

    def test_node_search_opt_in_keeps_acquisition_and_node_install_disabled(self):
        self.npm_acquisition()
        self.args.allow_public_npm = False
        self.args.enable_node_search = True
        self.args.selected_npm_cli = self.args.selected_npm_cli_sha256 = None
        with patch.object(subprocess, "Popen", side_effect=AssertionError("preparation must be offline")):
            host.prepare(self.args)
        output = Path(self.args.output)
        profile = json.loads((output / "node-acquisition.json").read_bytes())
        self.assertIs(profile["allow_acquisition"], False)
        self.assertEqual(profile["node_sha256"], self.args.selected_node_sha256)
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-node-search", argv)
        self.assertNotIn("--selected-npm-cli", argv)
        selected = json.loads((output / "selected-install.json").read_bytes())
        self.assertNotIn("node", selected["validators"])
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertTrue(receipt["node_search_enabled"])
        self.assertFalse(receipt["public_npm_enabled"])
        self.assertFalse(Path(self.args.workspace).exists())

    def test_node_search_requires_explicit_choice_and_full_runtime_archive_binding(self):
        self.args.enable_node_search = "true"
        self.refuse("invalid_node_search_choice")
        self.args.enable_node_search = True
        self.refuse("explicit_npm_scope_required")

    def test_node_search_and_acquisition_are_independent_choices(self):
        self.npm_acquisition()
        self.args.enable_node_search = True
        host.prepare(self.args)
        output = Path(self.args.output)
        self.assertIs(json.loads((output / "node-acquisition.json").read_bytes())["allow_acquisition"], True)
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-node-search", argv)

    def test_selected_pair_and_npm_scope_must_be_complete(self):
        self.args.selected_node = str(self.runtime)
        self.refuse("selected_node_runtime_pair_required")
        self.args.selected_node = None
        self.args.allow_public_npm = True
        self.refuse("explicit_npm_scope_required")
        self.args.allow_public_npm = False
        self.args.npm_archive = str(self.pip)
        self.args.npm_archive_sha256 = sha(self.pip.read_bytes())
        self.refuse("explicit_npm_scope_required")

    def test_legacy_python_release_refuses_selected_node_without_breaking_default(self):
        self.args.selected_node, self.args.selected_node_sha256 = str(self.runtime), sha(self.runtime.read_bytes())
        self.args.selected_npm_cli, self.args.selected_npm_cli_sha256 = str(self.runtime), sha(self.runtime.read_bytes())
        self.refuse("selected_node_not_in_release")
        self.args.selected_node = self.args.selected_node_sha256 = None
        self.args.selected_npm_cli = self.args.selected_npm_cli_sha256 = None
        self.assertEqual(host.prepare(self.args)["status"], "prepared")

    def test_linux_npm_target_and_python_acquisition_coexist(self):
        self.args.target, self.root_name = "linux-x86_64", "toolkit-library-fixture-linux-x86_64"
        self.npm_acquisition()
        self.acquire()
        host.prepare(self.args)
        output = Path(self.args.output)
        for name in ("node-acquisition.json", "python-acquisition.json"):
            self.assertEqual(json.loads((output / name).read_bytes())["target"], "linux-x86_64")
        self.assertEqual(json.loads((output / "selected-install.json").read_bytes())["python_target"], "linux-x86_64")

    def test_pip_requires_explicit_scope_and_complete_pin(self):
        for enabled, path, digest in [(False, str(self.pip), sha(self.pip.read_bytes())), (True, None, None),
                                      (True, str(self.pip), None)]:
            self.args.allow_public_pypi, self.args.pip_zipapp, self.args.pip_sha256 = enabled, path, digest
            self.refuse("explicit_pypi_scope_required")

    def test_linux_profiles_require_archive_and_provenance_target(self):
        self.args.target = "linux-x86_64"
        self.refuse("unsupported_combined_release")
        self.root_name = "toolkit-library-fixture-linux-x86_64"
        self.publish_release()
        self.refuse("release_target_mismatch")
        self.files["selected/provenance/selection.json"] = (json.dumps({"platform": "linux-x86_64", "scope": "retained_python_tools_only"}).encode(), 0o644)
        self.publish_release()
        self.acquire()
        host.prepare(self.args)
        output = Path(self.args.output)
        self.assertEqual(json.loads((output / "selected-install.json").read_bytes())["python_target"], self.args.target)
        self.assertEqual(json.loads((output / "python-acquisition.json").read_bytes())["target"], self.args.target)

    def test_wrong_archive_member_or_runtime_fails_before_output(self):
        self.args.release_sha256 = "0" * 64
        self.refuse("release_archive_changed")
        self.args.release_sha256 = sha(self.archive.read_bytes())
        (self.release / "licenses/notice.txt").write_bytes(b"changed")
        self.refuse("extracted_release_changed")
        self.publish_release()
        self.args.python_sha256 = "0" * 64
        self.refuse("host_input_changed")

    def test_added_file_symlink_and_mode_change_refuse(self):
        extra = self.release / "extra.txt"
        extra.write_text("unexpected")
        self.refuse("extracted_release_changed")
        extra.unlink()
        notice = self.release / "licenses/notice.txt"
        notice.unlink()
        notice.symlink_to(self.pip)
        self.refuse("invalid_release_member")
        notice.unlink()
        self.publish_release()
        notice.chmod(0o755)
        self.refuse("extracted_release_changed")

    def test_archive_duplicate_link_and_bounds_refuse(self):
        for kind in (tarfile.REGTYPE, tarfile.SYMTYPE, tarfile.LNKTYPE):
            extra = tarfile.TarInfo(self.root_name + "/licenses/notice.txt")
            extra.type, extra.mode, extra.linkname = kind, 0o644, "outside"
            self.publish_release(extra)
            self.refuse("invalid_archive_member")
        self.publish_release()
        with patch.object(host, "ENTRY_LIMIT", 2):
            self.refuse("release_too_large")
        with patch.object(host, "ARCHIVE_LIMIT", 3):
            with self.assertRaisesRegex(host.PreparationError, "invalid_release_archive"):
                host.archive_hash(io.BytesIO(b"1234"))
            self.assertEqual(host.archive_hash(io.BytesIO(b"123")), sha(b"123"))

    def test_case_and_unicode_destination_aliases_refuse_before_publication(self):
        for first, second in (("NewConfig", "newconfig"), ("caf\u00e9", "cafe\u0301")):
            with self.subTest(first=first):
                self.args.output = str(self.base / first)
                self.args.workspace = str(self.base / second)
                self.refuse("overlapping_directories")

    def test_overlaps_and_existing_destination_are_preserved(self):
        self.args.workspace = str(self.staging / "nested")
        self.refuse("overlapping_directories")
        self.args.workspace = str(self.base / "workspace")
        output = Path(self.args.output)
        output.mkdir()
        marker = output / "existing"
        marker.write_text("keep")
        with self.assertRaisesRegex(host.PreparationError, "output_or_workspace_exists"):
            host.prepare(self.args)
        self.assertEqual(marker.read_text(), "keep")

    def test_recheck_detects_mutation_and_publication_error_has_no_receipt(self):
        original = host.recheck_pins
        def changed(pins):
            self.runtime.write_bytes(b"changed after profile validation")
            original(pins)
        with patch.object(host, "recheck_pins", changed):
            self.refuse("host_input_changed")
        self.runtime.write_bytes(b"inert host executable, must never run\n")
        write = host.write_new
        def interrupted(path, raw):
            if path == Path(self.args.output) / "mcp.json":
                raise OSError("injected disk error")
            write(path, raw)
        with patch.object(host, "write_new", interrupted), self.assertRaises(OSError):
            host.prepare(self.args)
        self.assertTrue(Path(self.args.output).is_dir())
        self.assertFalse((Path(self.args.output) / "prepared.json").exists())

    def test_real_venv_entrypoint_survives_generated_command(self):
        environment = self.base / "mcp environment"
        venv.EnvBuilder(with_pip=False).create(environment)
        entry = environment / "bin/python"
        self.args.mcp_python = str(entry)
        self.args.mcp_python_sha256 = sha(entry.resolve().read_bytes())
        host.prepare(self.args)
        mcp = json.loads((Path(self.args.output) / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]
        self.assertIn(str(entry), mcp["args"])
        result = subprocess.run([mcp["command"], *mcp["args"]], capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["prefix"], str(environment))

    def test_existing_selected_owner_refuses_nonexecutable_planner(self):
        raw, _ = self.files["selected/bin/toolkit-python-plan"]
        self.files["selected/bin/toolkit-python-plan"] = (raw, 0o644)
        self.publish_release()
        with self.assertRaisesRegex(Exception, "invalid_selected_host_profile"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())

    def test_private_extracted_directories_preserve_exact_file_binding(self):
        for path in self.release.rglob("*"):
            if path.is_dir():
                path.chmod(0o700)
            else:
                path.chmod(0o700 if path.stat().st_mode & 0o111 else 0o600)
        result = host.prepare(self.args)
        self.assertEqual(result["status"], "prepared")

    def test_actual_tar_extraction_under_private_umask(self):
        destination = self.base / "private-extraction"
        destination.mkdir()
        subprocess.run(["/usr/bin/tar", "-xzf", str(self.archive), "-C", str(destination)],
                       check=True, capture_output=True, timeout=15, umask=0o077)
        self.args.release_dir = str(destination / self.root_name)
        result = host.prepare(self.args)
        self.assertEqual(result["status"], "prepared")

    def test_late_release_change_and_competing_output_are_not_overwritten(self):
        recheck = host.recheck_release
        calls = []
        def changed(root, snapshot):
            calls.append(True)
            if len(calls) == 2:
                (root / "licenses/notice.txt").write_bytes(b"late change")
            recheck(root, snapshot)
        with patch.object(host, "recheck_release", changed):
            self.refuse("extracted_release_changed")
        self.publish_release()
        pins = host.recheck_pins
        def competing(values):
            pins(values)
            output = Path(self.args.output)
            output.mkdir()
            (output / "owned-by-other-attempt").write_text("retain")
        with patch.object(host, "recheck_pins", competing), self.assertRaises(FileExistsError):
            host.prepare(self.args)
        self.assertEqual([p.name for p in Path(self.args.output).iterdir()], ["owned-by-other-attempt"])


if __name__ == "__main__":
    unittest.main()
