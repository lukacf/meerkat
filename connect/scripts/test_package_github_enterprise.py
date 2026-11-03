"""Offline assembly controls use synthetic bytes, never execute a provider binary."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("ghe_package", Path(__file__).with_name("package-github-enterprise.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class EnterprisePackageTests(unittest.TestCase):
    def test_inert_assembly_binds_inventory_and_refuses_archive_drift_or_overwrite(self):
        files = {"LICENSE": b"fixture license", "README.md": b"fixture readme", "github-mcp-server": b"inert fixture"}
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            for name, data in files.items():
                info = tarfile.TarInfo(name); info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        raw = buffer.getvalue()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            source = root / "release.tgz"; source.write_bytes(raw)
            target = root / "bundle"
            with (patch.object(package, "ARCHIVE_SHA256", package.owner.sha(raw)),
                  patch.object(package, "ARCHIVE_BYTES", len(raw)),
                  patch.object(package, "MEMBERS", {name: len(data) for name, data in files.items()}),
                  patch.object(package.owner, "GHE_ARTIFACT_SHA256", package.owner.sha(files["github-mcp-server"])),
                  patch.object(package.owner, "GHE_LICENSE_SHA256", package.owner.sha(files["LICENSE"])),
                  patch.object(package.owner, "check_official_executable") as header,
                  patch.object(package.owner.subprocess, "Popen", side_effect=AssertionError("unexpected execution"))):
                result = package.assemble(source, target)
                self.assertEqual(result["scope"], "offline_input_bundle_only")
                inputs, component = package.owner.official_inputs(target, result["contents_sha256"], "github")
                self.assertEqual(component["profile"], package.owner.GHE_PROFILE)
                self.assertEqual(inputs["github/github-mcp-server"], files["github-mcp-server"])
                self.assertEqual((target / "github/github-mcp-server").stat().st_mode & 0o777, 0o600)
                self.assertEqual(header.call_count, 2)
                with self.assertRaisesRegex(package.owner.InstallError, "fresh destination"):
                    package.assemble(source, target)
                source.write_bytes(raw[:-1] + bytes([raw[-1] ^ 1]))
                with self.assertRaisesRegex(package.owner.InstallError, "archive differs"):
                    package.assemble(source, root / "wrong")
                self.assertFalse((root / "wrong").exists())


if __name__ == "__main__":
    unittest.main()
