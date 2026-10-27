"""Trusted Forge selection and SDK argument controls, without native OAuth."""
import pytest
import subprocess
import sys

from launchkit.connect_application_startup import FreshApplicationStartup
from launchkit.connect_installer_caller import CustodyError
from test_application_startup import Fixture as FreshFixture
from test_application_restart import RestartFixture as RestoredFixture
from test_application_update_startup import fixture as replacement_fixture

BASE = "https://console.example.test/native-auth"


def fresh_owner(f, **options):
    return FreshApplicationStartup(project_lease=f.lease, accepted_source=f.source,
        gateway=f.gateway, preparation_plan=f.plan, source_policy=f.policy,
        activation_template=f.template, command=lambda *a, **kw: pytest.fail("unexpected command"),
        command_timeout=5, startup_timeout=60, provision_attempt_path=f.root / "provision.json",
        working_directory=f.workspace, bundle_path=f.root / "bundle",
        activation_path=f.root / "activation.json", **options)


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_trusted_redirect_reaches_exact_runtime_not_builder(tmp_path, monkeypatch, mode):
    f = (FreshFixture if mode == "fresh" else RestoredFixture)(tmp_path, monkeypatch)
    f.owner = (fresh_owner(f, hosted_mcp_redirect_base=BASE) if mode == "fresh"
               else f.make_owner(hosted_mcp_redirect_base=BASE))
    assert f.owner.console_public_origin == "https://console.example.test"
    name = "meerkat_toolkit.application_runtime" if mode == "fresh" else "meerkat_toolkit.host_control"
    cls = "ToolkitApplicationRuntime" if mode == "fresh" else "ToolkitMobKitRuntime"
    original = getattr(f.modules[name], cls)
    captured = []

    def runtime(*args, **kwargs):
        captured.append(kwargs)
        return original(*args)

    monkeypatch.setattr(f.modules[name], cls, runtime)
    await f.owner.prepare(f.app)
    config = f.config()
    assert not hasattr(config, "hosted_mcp_redirect_base")
    result = await f.owner.connect(config, f.app)
    assert captured == [{"hosted_mcp_redirect_base": BASE}]
    assert result.config is config
    await f.owner.shutdown(result)


@pytest.mark.parametrize("base", ["", False, {}, "http://console.test", "https://u:p@console.test",
    "https://console.test/?q=1", "https://console.test/#fragment", "https://console.test/\n"])
def test_invalid_console_binding_refuses_before_native_effects(tmp_path, monkeypatch, base):
    f = FreshFixture(tmp_path, monkeypatch)
    with pytest.raises(CustodyError, match="hosted MCP"):
        fresh_owner(f, hosted_mcp_redirect_base=base)
    assert f.events == []


@pytest.mark.asyncio
async def test_changed_base_refuses_before_preparation(tmp_path, monkeypatch):
    f = FreshFixture(tmp_path, monkeypatch)
    f.owner = fresh_owner(f, hosted_mcp_redirect_base=BASE)
    f.owner.hosted_mcp_redirect_base = "https://different.example.test"
    with pytest.raises(CustodyError, match="hosted MCP"):
        await f.owner.prepare(f.app)
    assert f.events == []


@pytest.mark.asyncio
async def test_replacement_keeps_original_base_outside_model_config(replacement_fixture, monkeypatch):
    f = replacement_fixture
    f.owner = f.make_owner(hosted_mcp_redirect_base=BASE)
    module = f.modules["meerkat_toolkit.application_update"]
    original = module.ToolkitApplicationUpdateRuntime
    captured = []

    def runtime(*args, **kwargs):
        captured.append(kwargs)
        return original(*args)

    monkeypatch.setattr(module, "ToolkitApplicationUpdateRuntime", runtime)
    await f.owner.prepare(f.app)
    config = f.config()
    assert not hasattr(config, "hosted_mcp_redirect_base")
    runtime = await f.owner.connect(config, f.app)
    assert captured == [{"hosted_mcp_redirect_base": BASE}]
    assert runtime is f.owner.runtime is f.app.context.runtime
    await f.owner.shutdown(runtime)


def test_generated_package_resolves_its_own_protected_console_module(tmp_path):
    from launchkit.compiler import generate_package
    from launchkit.blueprint import starter_blueprint
    for item in generate_package(starter_blueprint()):
        if not item.path.endswith(".py"):
            continue
        path = tmp_path / item.path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(item.as_bytes())
    assert not (tmp_path / "launchkit/templates_v2/console_auth.py").exists()
    result = subprocess.run([sys.executable, "-I", "-c",
        "import sys; sys.path.insert(0, sys.argv[1]); "
        "from launchkit.connect_application_startup import _hosted_mcp_origin; "
        "assert _hosted_mcp_origin(sys.argv[2]) == 'https://console.example.test'",
        str(tmp_path), BASE], capture_output=True, text=True, timeout=15)
    assert result.returncode == 0, result.stderr


@pytest.mark.asyncio
async def test_generated_host_requires_authenticated_console_for_hosted_mcp(tmp_path, monkeypatch):
    import json
    from types import SimpleNamespace
    from launchkit.compiler import generate_files
    from launchkit.blueprint import starter_blueprint
    from test_application_extension import load_host
    module = load_host(monkeypatch, tmp_path)
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    gateway = tmp_path / "gateway"
    gateway.write_bytes(b"inert test path")
    startup = SimpleNamespace(gateway=SimpleNamespace(path=gateway),
        console_public_origin="https://console.example.test", hosted_mcp_targets=(),
        require_tool_policy_source=False)
    monkeypatch.setattr(module, "console_configuration", lambda *args: False)
    monkeypatch.setattr(module, "validate_configuration_values", lambda *args: None)
    with pytest.raises(ValueError, match="requires the configured authenticated Console"):
        await module.run(spec, SimpleNamespace(), application_startup=startup)
