"""Guide drift checks use inert archives and never execute delivered files."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "installation_guide", Path(__file__).with_name("check-installation-guide.py"))
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


def sha(data):
    return hashlib.sha256(data).hexdigest()


class InstallationGuideTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="guide-check-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.mac = self.root / "dist/agent-install-kit-test-mac"
        self.linux = self.root / "dist/agent-install-kit-test-linux"
        self.mac_files = {
            "bin/install-library-consumer.py": (0o755, b"inert installer\n"),
            "bin/toolkit-python-plan": (0o755, b"inert planner\n"),
            "verification-environment/CONTENTS.json": (0o644, b"{}\n"),
            "README.md": (0o644, b"inert guide\n"),
        }
        manifest = {name: {"sha256": sha(data)}
                    for name, (_, data) in self.mac_files.items()}
        self.mac_files["provenance/kit.json"] = (0o644, json.dumps({"files": manifest}).encode())
        self.mac_sha = self.archive(self.mac, "agent-install-kit-macos-arm64", self.mac_files)
        self.profile = {
            "schema_version": 1,
            "scope": "standing_local_software_installation_only",
            "staging_root": "/host/staging",
            "installer": self.pin("bin/install-library-consumer.py"),
            "validators": {"python": self.pin("bin/toolkit-python-plan")},
        }
        self.guide = self.root / M.GUIDE
        self.guide.parent.mkdir(parents=True)
        for name in M.ENTRY_POINTS:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("See HOST-SETUP.md\n")
        self.write_guide()

    def pin(self, name):
        return {"path": "/host/agent-install-kit-macos-arm64/" + name,
                "sha256": sha(self.mac_files[name][1])}

    def archive(self, directory, root_name, files, extra_member=None):
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / (root_name + ".tar.gz")
        with tarfile.open(path, "w:gz") as archive:
            for name, (mode, data) in files.items():
                info = tarfile.TarInfo(root_name + "/" + name)
                info.mode, info.size = mode, len(data)
                archive.addfile(info, io.BytesIO(data))
            if extra_member is not None:
                archive.addfile(extra_member)
        digest = sha(path.read_bytes())
        (directory / "SHA256SUMS").write_text(digest + "  " + path.name + "\n")
        (directory / "README.md").write_text("Inert archive guide\n")
        return digest

    def write_guide(self, linux=False, suffix=""):
        contents_sha = sha(self.mac_files["verification-environment/CONTENTS.json"][1])
        text = (
            "[current macOS arm64 kit](../../dist/agent-install-kit-test-mac/README.md)\n"
            f'printf "%s  %s\\n" {self.mac_sha} "$KIT_ARCHIVE"\n'
            "```json\n" + json.dumps(self.profile) + "\n```\n"
            f"bootstrap --contents-sha256 {contents_sha}\n"
            "Read $KIT/README.md\n")
        if linux:
            text += "[current Linux x86_64 kit](../../dist/agent-install-kit-test-linux/README.md)\n"
        self.guide.write_text(text + suffix)

    def linux_archive(self, metadata_change=None, payload_change=None, extra_member=None, node=False):
        tools = M.LINUX_NODE_TOOLS if node else M.LINUX_TOOLS
        files = {name: (0o755, b"inert executable\n") for name in tools}
        files.update({"README.md": (0o644, b"Linux Python only\n"),
                      "contracts/NODE-INSTALL.md": (0o644, b"Shared contract, no Node executable\n")})
        metadata = {
            "schema_version": 1, "platform": "linux-x86_64",
            "scope": "selected-python-node-installation-kit" if node else "selected-python-installation-kit",
            "files": {name: {"sha256": sha(data), "bytes": len(data), "mode": mode}
                      for name, (mode, data) in files.items()},
        }
        if metadata_change is not None:
            metadata_change(metadata)
        files["provenance/kit.json"] = (0o644, json.dumps(metadata).encode())
        files["SHA256SUMS"] = (0o644, "".join(
            sha(data) + "  " + name + "\n" for name, (_, data) in sorted(files.items())).encode())
        if payload_change is not None:
            payload_change(files)
        return self.archive(self.linux, "agent-install-kit-linux-x86_64", files, extra_member)

    def test_mac_only_output_and_checks_stay_unchanged(self):
        result = M.check(self.root)
        self.assertEqual(result, {
            "edition": "dist/agent-install-kit-test-mac", "archive_sha256": self.mac_sha,
            "tool_pins_checked": ["bin/install-library-consumer.py", "bin/toolkit-python-plan"],
            "bootstrap_sha256": sha(self.mac_files["verification-environment/CONTENTS.json"][1]),
            "referenced_paths_checked": 1, "entry_points_checked": 3})

    def test_explicit_linux_link_checks_all_files_without_execution(self):
        digest = self.linux_archive()
        self.write_guide(linux=True)
        (self.root / M.ENTRY_POINTS[-1]).write_text(
            "dist/agent-install-kit-test-mac and dist/agent-install-kit-test-linux\n")
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = M.check(self.root)
        self.assertEqual(result["linux"], {
            "edition": "dist/agent-install-kit-test-linux", "archive_sha256": digest,
            "scope": "selected-python-installation-kit", "platform": "linux-x86_64",
            "files_checked": 6, "tools_checked": sorted(M.LINUX_TOOLS)})

    def test_unselected_second_edition_in_each_front_door_is_rejected(self):
        for name in (M.GUIDE, *M.ENTRY_POINTS):
            with self.subTest(name=name):
                path = self.root / name
                original = path.read_text()
                path.write_text(original + "\ndist/agent-install-kit-unchecked/README.md\n")
                with self.assertRaisesRegex(ValueError, "advertised edition differs"):
                    M.check(self.root)
                path.write_text(original)

    def test_linux_cannot_alias_mac_or_be_ambiguous(self):
        link = "[current Linux x86_64 kit](../../dist/agent-install-kit-test-mac/README.md)\n"
        for suffix in (link, link + link):
            with self.subTest(suffix=suffix):
                self.write_guide(suffix=suffix)
                with self.assertRaisesRegex(ValueError, "distinct|only one"):
                    M.check(self.root)

    def test_linux_archive_outer_checksum_and_root_are_bound(self):
        self.linux_archive()
        self.write_guide(linux=True)
        sums = self.linux / "SHA256SUMS"
        sums.write_text("0" * 64 + "  agent-install-kit-linux-x86_64.tar.gz\n")
        with self.assertRaisesRegex(ValueError, "archive checksum mismatch"):
            M.check(self.root)
        wrong_root = tarfile.TarInfo("agent-install-kit-macos-arm64/extra")
        self.linux_archive(extra_member=wrong_root)
        with self.assertRaisesRegex(ValueError, "unexpected Linux archive member"):
            M.check(self.root)

    def test_linux_scope_platform_and_manifest_fields_are_checked(self):
        changes = [
            lambda m: m.update(scope="selected-python-node-mcpb-installation-kit"),
            lambda m: m.update(platform="macos-arm64"),
            lambda m: m.update(schema_version=True),
            lambda m: m["files"].pop("README.md"),
            lambda m: m["files"]["README.md"].update(sha256="0" * 64),
            lambda m: m["files"]["README.md"].update(bytes=1),
            lambda m: m["files"]["README.md"].update(mode=0o755),
        ]
        self.write_guide(linux=True)
        for index, change in enumerate(changes):
            with self.subTest(index=index):
                self.linux_archive(metadata_change=change)
                with self.assertRaisesRegex(ValueError, "scope and platform|manifest file"):
                    M.check(self.root)

    def test_linux_changed_payload_and_internal_checksums_are_rejected(self):
        changes = [
            lambda f: f.update({"README.md": (0o644, b"changed\n")}),
            lambda f: f.update({"unlisted": (0o644, b"extra\n")}),
            lambda f: f.update({"SHA256SUMS": (0o644, b"")}),
            lambda f: f.update({"SHA256SUMS": (0o644, f["SHA256SUMS"][1] * 2)}),
        ]
        self.write_guide(linux=True)
        for index, change in enumerate(changes):
            with self.subTest(index=index):
                self.linux_archive(payload_change=change)
                with self.assertRaisesRegex(ValueError, "manifest file|internal checksum"):
                    M.check(self.root)

    def test_linux_requires_exact_three_tool_set_even_with_consistent_manifest(self):
        self.write_guide(linux=True)
        for tools in (M.LINUX_TOOLS - {"bin/toolkit-python-plan"},
                      M.LINUX_TOOLS | {"bin/toolkit-node-plan"}):
            with self.subTest(tools=tools):
                with patch.object(M, "LINUX_TOOLS", tools):
                    self.linux_archive()
                with self.assertRaisesRegex(ValueError, "exactly its three"):
                    M.check(self.root)

    def test_linux_node_scope_checks_all_four_tools_without_execution(self):
        digest = self.linux_archive(node=True)
        self.write_guide(linux=True)
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = M.check(self.root)["linux"]
        self.assertEqual(result["scope"], "selected-python-node-installation-kit")
        self.assertEqual(result["tools_checked"], sorted(M.LINUX_NODE_TOOLS))
        self.assertEqual(result["archive_sha256"], digest)
        self.assertEqual(result["files_checked"], 7)

    def test_linux_node_scope_requires_exact_four_tool_set(self):
        self.write_guide(linux=True)
        for tools in (M.LINUX_NODE_TOOLS - {"bin/toolkit-node-plan"},
                      M.LINUX_NODE_TOOLS | {"bin/toolkit-mcpb-install"},
                      M.LINUX_NODE_TOOLS | {"bin/toolkit-compose-plan"}):
            with self.subTest(tools=tools):
                with patch.object(M, "LINUX_NODE_TOOLS", tools):
                    self.linux_archive(node=True)
                with self.assertRaisesRegex(ValueError, "exactly its four"):
                    M.check(self.root)
        self.linux_archive(node=True, metadata_change=lambda m: m.update(scope="selected-python-installation-kit"))
        with self.assertRaisesRegex(ValueError, "exactly its three"):
            M.check(self.root)

    def test_linux_link_does_not_weaken_mac_profile_pin_or_bootstrap(self):
        self.linux_archive()
        original = copy.deepcopy(self.profile)
        for change in (
            lambda p: p.update(schema_version=True),
            lambda p: p.update(schema_version=1.0),
            lambda p: p["validators"].update(node=p["validators"]["python"]),
            lambda p: p["installer"].update(sha256="0" * 64),
        ):
            self.profile = copy.deepcopy(original)
            change(self.profile)
            self.write_guide(linux=True)
            with self.assertRaisesRegex(ValueError, "Python profile|tool pin mismatch"):
                M.check(self.root)
        self.profile = original
        self.write_guide(linux=True)
        text = self.guide.read_text()
        self.guide.write_text(text.replace(self.mac_sha, "0" * 64))
        with self.assertRaisesRegex(ValueError, "archive verification"):
            M.check(self.root)
        self.guide.write_text(text.replace(
            sha(self.mac_files["verification-environment/CONTENTS.json"][1]), "0" * 64))
        with self.assertRaisesRegex(ValueError, "bootstrap environment"):
            M.check(self.root)

    def combined_library_links(self, archives=False):
        selected = {
            "macos": "dist/toolkit-library-current-macos-r2/README.md",
            "linux": "dist/toolkit-library-current-linux-r3/README.md",
        }
        if archives:
            selected = {
                target: path.removesuffix("README.md") +
                "toolkit-library-current-" + suffix + ".tar.gz"
                for target, path, suffix in (
                    ("macos", selected["macos"], "macos-arm64"),
                    ("linux", selected["linux"], "linux-x86_64"))
            }
        for value in selected.values():
            path = self.root / value
            path.parent.mkdir(parents=True)
            path.write_text("Inert combined selection, not an executable archive\n")
        (self.root / "README.md").write_text(
            f'[current macOS archive]({selected["macos"]})\n'
            f'[current Linux archive]({selected["linux"]})\nSee HOST-SETUP.md\n')
        with self.guide.open("a") as stream:
            stream.write(f'\n[macOS library](../../{selected["macos"]})\n'
                         f'[Linux library](../../{selected["linux"]})\n')
        for name in ("docs/site/library/local-install.mdx", "docs/site/acquisition/local-install.mdx"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("See HOST-SETUP.md\n" + "\n".join(selected.values()))
        return selected

    def test_combined_frontdoors_follow_readme_without_opening_archives(self):
        selected = self.combined_library_links()
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = M.check(self.root)
        self.assertEqual(result["combined_libraries"], selected)

    def test_combined_archive_frontdoors_preserve_exact_paths_without_opening_archives(self):
        selected = self.combined_library_links(archives=True)
        with patch.object(M.tarfile, "open", side_effect=AssertionError("must not open combined archives")):
            self.assertEqual(M.check_combined_links(self.root), selected)
        self.assertEqual(M.check(self.root)["combined_libraries"], selected)

    def test_stale_combined_archive_or_legacy_guide_is_rejected_in_each_frontdoor(self):
        self.combined_library_links(archives=True)
        stale = (
            "dist/toolkit-library-old-macos/toolkit-library-old-macos-arm64.tar.gz",
            "dist/toolkit-library-current-macos-r2/README.md",
        )
        for name in (M.GUIDE, "agents/setup-installer/README.md",
                     "docs/site/library/local-install.mdx", "docs/site/acquisition/local-install.mdx"):
            path = self.root / name
            original = path.read_text()
            for link in stale:
                with self.subTest(name=name, link=link):
                    path.write_text(original + "\n" + link + "\n")
                    with self.assertRaisesRegex(ValueError, "combined library edition differs"):
                        M.check_combined_links(self.root)
            path.write_text(original)

    def test_combined_archive_selection_requires_existing_file_and_matching_platform(self):
        selected = self.combined_library_links(archives=True)
        readme = self.root / "README.md"
        original = readme.read_text()
        swapped = original.replace(selected["macos"], "SWAP_TARGET")
        readme.write_text(swapped.replace(selected["linux"], selected["macos"])
                          .replace("SWAP_TARGET", selected["linux"]))
        with self.assertRaisesRegex(ValueError, "archive target differs"):
            M.check_combined_links(self.root)
        readme.write_text(original)
        (self.root / selected["linux"]).unlink()
        with self.assertRaisesRegex(ValueError, "missing combined library archive"):
            M.check_combined_links(self.root)

    def test_malformed_or_duplicate_archive_selections_cannot_skip_combined_checks(self):
        selected = self.combined_library_links(archives=True)
        readme = self.root / "README.md"
        original = readme.read_text()
        for text in (original + original,
                     original.replace("[current Linux archive]", "[historical archive]"),
                     original.replace(".tar.gz", ".zip")):
            with self.subTest(text=text):
                readme.write_text(text)
                with self.assertRaisesRegex(ValueError, "exactly one current combined"):
                    M.check_combined_links(self.root)
        readme.write_text(original)
        for name in (M.GUIDE, *M.ENTRY_POINTS, "docs/site/library/local-install.mdx"):
            path = self.root / name
            path.write_text(path.read_text().replace(".tar.gz", ".zip"))
        with self.assertRaisesRegex(ValueError, "exactly one current combined"):
            M.check_combined_links(self.root)

    def test_stale_combined_edition_is_rejected_in_each_frontdoor(self):
        selected = self.combined_library_links()
        for name in (M.GUIDE, "agents/setup-installer/README.md",
                     "docs/site/library/local-install.mdx", "docs/site/acquisition/local-install.mdx"):
            with self.subTest(name=name):
                path = self.root / name
                original = path.read_text()
                path.write_text(original + "\ndist/toolkit-library-old-macos/README.md\n")
                with self.assertRaisesRegex(ValueError, "combined library edition differs"):
                    M.check(self.root)
                path.write_text(original)
        path = self.guide
        path.write_text(path.read_text().replace(selected["macos"], "dist/toolkit-library-old-macos/README.md"))
        with self.assertRaisesRegex(ValueError, "combined library"):
            M.check(self.root)

    def test_combined_selection_missing_duplicate_or_missing_file_is_rejected(self):
        selected = self.combined_library_links()
        path = self.root / "README.md"
        original = path.read_text()
        for contents in ("See HOST-SETUP.md\n", original + original,
                         original.replace("[current Linux archive]", "[historical archive]")):
            with self.subTest(contents=contents):
                path.write_text(contents)
                with self.assertRaisesRegex(ValueError, "exactly one current combined"):
                    M.check(self.root)
        path.write_text(original)
        (self.root / selected["linux"]).unlink()
        with self.assertRaisesRegex(ValueError, "missing combined library guide"):
            M.check(self.root)

    def test_combined_host_platform_swap_and_missing_site_link_are_rejected(self):
        selected = self.combined_library_links()
        original = self.guide.read_text()
        self.guide.write_text(original.replace("[macOS library]", "[temporary]")
                              .replace("[Linux library]", "[macOS library]")
                              .replace("[temporary]", "[Linux library]"))
        with self.assertRaisesRegex(ValueError, "combined library target differs"):
            M.check(self.root)
        self.guide.write_text(original)
        path = self.root / "docs/site/library/local-install.mdx"
        path.write_text(path.read_text().replace(selected["linux"], ""))
        with self.assertRaisesRegex(ValueError, "both combined library targets"):
            M.check(self.root)


if __name__ == "__main__":
    unittest.main()
