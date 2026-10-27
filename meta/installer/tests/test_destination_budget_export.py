"""Both package exporters must produce an independently loadable helper."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

import pytest

from launchkit import compiler, deployment_deadlines, generator
from launchkit.blueprint import starter_blueprint


def legacy_spec(target):
    return {"project": {"name": "Owned test team", "slug": "owned-test-team",
                        "description": "Coordinate test work with saved context."},
            "agents": [{"id": "lead", "name": "Coordinator", "role": "Coordinate work",
                        "instructions": "Use connected tools and report evidence accurately."}],
            "connections": [], "provider": "openai", "model": "gpt-6.1-sol", "target": target,
            "gcp": {"project": "owned-test-project", "zone": "europe-west1-b"}}


@pytest.mark.parametrize("exporter", ["legacy", "blueprint"])
@pytest.mark.parametrize("target", ["mac", "linux", "gcp"])
def test_both_exporters_load_destination_budget_without_launch_installed(tmp_path, exporter, target):
    if exporter == "legacy":
        files = generator.generate_files(legacy_spec(target))
    else:
        blueprint = starter_blueprint().model_dump()
        blueprint["target"] = target
        blueprint["gcp"] = {"project": "owned-test-project", "zone": "europe-west1-b"}
        files = compiler.generate_files(blueprint)
    helper = tmp_path / "remote_operations.py"
    helper.write_text(files["remote_operations.py"])
    # Actually import the generated destination helper in isolated Python. AST
    # validity alone would miss the string budget's TypeError at module import.
    script = ("import importlib.util,json,sys\n"
        "s=importlib.util.spec_from_file_location('owned_helper',sys.argv[1])\n"
        "m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\n"
        "assert 'launchkit' not in sys.modules\n"
        "print(json.dumps(m.SHUTDOWN_BUDGET,sort_keys=True))\n")
    result = subprocess.run([sys.executable, "-I", "-B", "-c", script, str(helper)],
                            cwd=tmp_path, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    budgets = json.loads(result.stdout)
    assert budgets == {
        "unreported_horizon_ms": deployment_deadlines.UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS,
        "maximum_horizon_ms": deployment_deadlines.MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS,
        "earlier_pinned_horizon_ms": deployment_deadlines.EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS,
        "application_margin": (deployment_deadlines.NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT
                               + deployment_deadlines.APPLICATION_ADAPTER_STOP_MARGIN),
        "force_reap": deployment_deadlines.PROCESS_FORCE_REAP_TIMEOUT,
        "worker_margin": deployment_deadlines.WORKER_STOP_MARGIN,
        "service_margin": deployment_deadlines.USER_SERVICE_STOP_MARGIN,
        "command_margin": deployment_deadlines.SERVICE_STOP_COMMAND_MARGIN,
        "legacy_service": deployment_deadlines.LEGACY_USER_SERVICE_STOP_TIMEOUT}
    assert "__LAUNCH_SHUTDOWN_BUDGET__" not in files["remote_operations.py"]
    sums = dict(row.split("  ", 1)[::-1] for row in files["SHA256SUMS"].splitlines())
    assert sums["remote_operations.py"] == hashlib.sha256(helper.read_bytes()).hexdigest()


@pytest.mark.parametrize("count", [0, 2])
def test_shared_renderer_rejects_missing_or_repeated_budget_slot(count):
    template = "SHUTDOWN_BUDGET = '__LAUNCH_SHUTDOWN_BUDGET__'\n" * count
    with pytest.raises(ValueError, match="shutdown budget template is invalid"):
        deployment_deadlines.render_destination_shutdown_budget(template)
