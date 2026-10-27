"""Exercise the actual shell entry with an inert Python protocol peer.

The peer simulates verified package inputs. It cannot install dependencies,
import an SDK, start a server or execute native code. Artifact qualification
and installed application acceptance are separate from these wiring checks.
"""
import json
from pathlib import Path
import subprocess
import sys

import pytest


@pytest.fixture
def product_entry(tmp_path):
    root = tmp_path / "Forge folder with spaces"
    root.mkdir()
    (root / "start.sh").write_bytes(Path(__file__).parents[1].joinpath("start.sh").read_bytes())
    (root / "standalone-manifest.json").write_text("{}")
    python = root / ".venv/bin/python"
    python.parent.mkdir(parents=True)
    python.write_text("#!" + sys.executable + "\n" + '''import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with Path(os.environ["ENTRY_LOG"]).open("a") as log:
    log.write(json.dumps(args) + "\\n")
failure = os.environ.get("ENTRY_FAILURE")
if args[:3] == ["-B", "-m", "launchkit.distribution"]:
    if failure == "verification":
        sys.exit(31)
    if args[3:] == ["--bootstrap-inputs", "."]:
        print("meta-toolkit\\nvendor/guide/Meta client.whl\\nvendor/product/Toolkit runtime.whl")
        sys.exit(0)
    if args[3:] == ["--verify", "."]:
        print("legacy-single.whl")
        sys.exit(0)
elif args[:2] == ["-m", "pip"]:
    sys.exit(32 if failure == "dependencies" and args[2:3] == ["install"] else 0)
elif args[:2] == ["-m", "uvicorn"]:
    sys.exit(0)
elif args[:2] == ["-B", "-c"] and args[2] == "from launchkit.packaged_runtime import open_selection; open_selection()":
    sys.exit(33 if failure == "product" else 0)
elif args[:1] == ["-c"] and args[1].startswith("from meerkat_meta import Meta;"):
    sys.exit(34 if failure == "sdk" else 0)
elif args[:1] == ["-c"] and args[1].startswith("import sys; sys.exit(0 if sys.version_info >= (3, 11)"):
    sys.exit(0)
sys.exit(91)
''')
    python.chmod(0o755)
    retained = [root / ".studio/keep.json", root / ".env", root / ".venv/keep.txt"]
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Retained synthetic setup.\n")
    before = {path: path.read_bytes() for path in retained}
    log = tmp_path / "entry.jsonl"

    def run(*, prepare_only=False, failure="", gateway=False):
        env = {"PATH": "/usr/bin:/bin", "HOME": str(tmp_path), "ENTRY_LOG": str(log),
            "ENTRY_FAILURE": failure, "META_FORGE_PORT": "8791"}
        if gateway:
            stub = root / "unrelated_gateway"
            stub.write_text("#!/bin/sh\nexit 97\n"); stub.chmod(0o755)
            env["MOBKIT_GATEWAY_BIN"] = str(stub)
        result = subprocess.run(["/bin/bash", "start.sh", *(["--prepare-only"] if prepare_only else [])],
            cwd=root, env=env, capture_output=True, text=True, timeout=10)
        assert all(path.read_bytes() == value for path, value in before.items())
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        return result, calls
    return run


@pytest.mark.parametrize("prepare_only", [False, True])
def test_product_entry_keeps_both_sdks_and_uses_the_selected_runtime(product_entry, prepare_only):
    result, calls = product_entry(prepare_only=prepare_only)
    assert result.returncode == 0, result.stderr
    assert ["-B", "-m", "launchkit.distribution", "--bootstrap-inputs", "."] in calls
    assert ["-m", "pip", "install", "--quiet", "--force-reinstall", "--no-deps",
        "vendor/guide/Meta client.whl", "vendor/product/Toolkit runtime.whl"] in calls
    assert ["-B", "-c", "from launchkit.packaged_runtime import open_selection; open_selection()"] in calls
    assert not any("launchkit.bootstrap" in args or any("meerkat_mobkit" in part for part in args) for args in calls)
    serving = [args for args in calls if args[:2] == ["-m", "uvicorn"]]
    assert serving == ([] if prepare_only else [["-m", "uvicorn", "launchkit.packaged_runtime:create_app",
        "--factory", "--host", "127.0.0.1", "--port", "8791", "--timeout-graceful-shutdown", "20"]])


@pytest.mark.parametrize("failure", ["verification", "dependencies", "sdk", "product"])
def test_product_preparation_failure_cannot_fall_back_to_the_legacy_runtime(product_entry, failure):
    result, calls = product_entry(failure=failure)
    assert result.returncode != 0
    assert not any(args[:2] in (["-m", "uvicorn"], ["-m", "launchkit.bootstrap"]) for args in calls)
    if failure == "verification":
        assert not any(args[:2] == ["-m", "pip"] for args in calls)
    elif failure in {"dependencies", "product"}:
        assert "Forge dependency preparation did not finish" in result.stderr


def test_product_bundle_refuses_an_unrelated_gateway_before_dependency_preparation(product_entry):
    result, calls = product_entry(gateway=True)
    assert result.returncode != 0
    assert "Remove MOBKIT_GATEWAY_BIN" in result.stderr
    assert not any(args[:2] in (["-m", "pip"], ["-m", "uvicorn"]) for args in calls)
