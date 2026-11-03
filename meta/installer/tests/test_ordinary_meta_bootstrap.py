"""Normal installation admission and child dispatch, with inert native bytes.

The protected source, package, receipt, artifact readers, project lease and
actual MetaBuilderConfig are real. No gateway, provider, compiler, HTTP server
or installed application execution occurs in these controls.
"""
from copy import deepcopy
import asyncio
import sys
from types import SimpleNamespace

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.ordinary_meta_startup import OrdinaryMetaStartup
from launchkit.templates.storage_setup import project_lock
from meerkat_meta.builder import MetaBuilderConfig
from test_deployment_application_bootstrap import write
from test_meta_toolkit_initial_install import (
    initial, implementation_service, product, no_product_execution, no_processes,
)


@pytest.fixture
async def ordinary(initial):
    f = initial
    await f.submit()
    f.directory = f.service._directory(f.guide_id)
    package = deployment.materialize(f.directory)
    receipt = deployment.update_receipt(f.directory, status="stopped", phase="configure")
    private = f.destination / ".mobkit/ordinary-inputs"
    private.mkdir(mode=0o700)
    # These bytes never execute and do not establish gateway qualification.
    f.gateway = write(private / "ordinary-rpc", b"inert ordinary gateway specimen\n", 0o700)
    f.definition = ExactFile(f.destination / "config/mob.toml",
        bootstrap.updates.digest(package["config/mob.toml"][0]))
    f.inputs = {"kind": "ordinary", "gateway": f.gateway.pin(),
        "definition": f.definition.pin(), "compiled_policies": [],
        "runtime_candidate": deepcopy(receipt["runtime_candidate"])}
    f.receipt = receipt
    return f


async def test_real_admission_and_child_select_ordinary_meta_without_helper_preparation(ordinary):
    f = ordinary
    pin, observed = bootstrap.retain_bootstrap_admission(f.directory, f.receipt, f.inputs)
    observed.require_current()
    selected, observation = bootstrap.observe_bootstrap_admission(f.directory, pin)
    assert selected["application_startup"] == f.inputs
    assert observation.current is None and type(observation.gateway) is ExactFile
    assert bootstrap.bootstrap_output_paths(f.directory, f.inputs) == (f.destination, (), None)
    factory = bootstrap.BootstrapFactory(f.directory, pin)
    with project_lock(f.destination) as lease:
        async with factory(lease) as startup:
            assert type(startup) is OrdinaryMetaStartup and startup.lease is lease
            assert startup.runtime is None and startup.compiled_policies == ()
            app = SimpleNamespace(context=SimpleNamespace(root=f.destination, runtime=None),
                dynamic=False, _application_callbacks=None)
            await startup.prepare(app)
            config = startup.builder_config(persistent_state=str(f.destination / ".mobkit/state"),
                roster_provider=app, topology_provider=app, session_builder=object())
            assert type(config) is MetaBuilderConfig
            assert config.application_tool_policies is None
            assert config.prepared_application_bundle is None
            assert config.prepared_application_digest is None
            assert not startup.require_tool_policy_source
            # The generated app reads this at Console wiring: password login, no enrollment.
            assert startup.console_login is False
        assert factory.source._closed
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
    assert not (f.destination / "shared").exists() and not (f.destination / "current").exists()
    assert not f.service.launched[1:]


@pytest.mark.parametrize("kind", [[], {}, 1, None])
def test_invalid_startup_kind_is_refused_as_custody_error(kind):
    with pytest.raises(CustodyError):
        bootstrap._shape({"kind": kind})


async def test_selected_toolkit_gateway_cannot_be_used_as_ordinary_rpc(ordinary):
    f = ordinary
    from launchkit import runtime_selection
    artifacts = runtime_selection.selected_runtime_artifacts(f.target)
    value = deepcopy(f.inputs)
    value["gateway"]["path"] = str(f.destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway")
    with pytest.raises(CustodyError, match="separately selected ordinary RPC"):
        bootstrap.retain_bootstrap_admission(f.directory, f.receipt, value)
    assert not list(f.directory.glob("bootstrap-admission-*"))


@pytest.mark.parametrize("change", ["runtime", "definition", "gateway", "duplicate-policy", "extra-plan"])
async def test_ordinary_admission_refuses_changed_original_selections_before_native(ordinary, change):
    f = ordinary
    value = deepcopy(f.inputs)
    if change == "runtime":
        value["runtime_candidate"]["assembly"]["sha256"] = "f" * 64
    elif change == "definition":
        value["definition"] = f.gateway.pin()
    elif change == "gateway":
        f.gateway.path.write_bytes(b"changed selected gateway\n")
    elif change == "duplicate-policy":
        policy = write(f.gateway.path.parent / "compiled-policy", b"inert policy specimen\n")
        value["compiled_policies"] = [policy.pin(), policy.pin()]
    else:
        value["preparation_plan"] = f.gateway.pin()
    with pytest.raises((CustodyError, ValueError)):
        bootstrap.retain_bootstrap_admission(f.directory, f.receipt, value)
    assert not list(f.directory.glob("bootstrap-admission-*"))
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


async def test_entered_fresh_cannot_be_bypassed_with_ordinary_startup(ordinary):
    f = ordinary
    original = write(f.directory / bootstrap.FRESH_ENTRY_NAME, b"retained uncertain Fresh specimen\n")
    pin, _ = bootstrap.retain_bootstrap_admission(f.directory, f.receipt, f.inputs)
    with pytest.raises(CustodyError, match="completed Restore"):
        bootstrap.observe_bootstrap_admission(f.directory, pin)
    with pytest.raises(CustodyError, match="completed Restore"):
        bootstrap.BootstrapFactory(f.directory, pin)
    assert original.read() == b"retained uncertain Fresh specimen\n"


async def test_admitted_definition_change_refuses_child_before_source_entry(ordinary):
    f = ordinary
    pin, _ = bootstrap.retain_bootstrap_admission(f.directory, f.receipt, f.inputs)
    factory = bootstrap.BootstrapFactory(f.directory, pin)
    f.definition.path.write_bytes(b"changed accepted definition\n")
    with project_lock(f.destination) as lease:
        with pytest.raises((CustodyError, ValueError)):
            async with factory(lease):
                pytest.fail("Changed accepted source reached native startup")
    assert factory.startup is None and factory.source is None


async def test_observation_retains_ordinary_gateway_and_compiler_policy_bytes(ordinary):
    f = ordinary
    policy = write(f.gateway.path.parent / "compiled-policy", b'{"inert_compiler_specimen":true}\n')
    f.inputs["compiled_policies"] = [policy.pin()]
    pin, observed = bootstrap.retain_bootstrap_admission(f.directory, f.receipt, f.inputs)
    policy.path.write_bytes(b"changed compiled policy\n")
    with pytest.raises(CustodyError):
        observed.require_current()
    with pytest.raises(CustodyError):
        bootstrap.observe_bootstrap_admission(f.directory, pin)


async def test_normal_start_action_queues_existing_bootstrap_child_with_original_selection(ordinary):
    f = ordinary
    await f.service.start_application(f.guide_id, f.receipt["deployment_id"], application_startup=f.inputs)
    assert len(f.service.launched) == 2
    directory, mode, arguments = f.service.launched[-1]
    assert directory == f.directory and mode == "start"
    assert set(arguments) == {"bootstrap_selection"}
    selected = bootstrap.read_bootstrap_admission(directory, arguments["bootstrap_selection"])
    assert selected["application_startup"] == f.inputs
    assert selected["installation"]["destination"] == str(f.destination)
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
    assert not (f.destination / "current").exists()
    # Capturing the worker launch is not native readiness or useful execution.
    assert deployment.read_receipt(directory)["console_url"] is None


async def test_normal_start_refuses_uncertain_fresh_without_replacing_history(ordinary):
    f = ordinary
    entry = write(f.directory / bootstrap.FRESH_ENTRY_NAME, b"uncertain Fresh retained\n")
    with pytest.raises((CustodyError, ValueError), match="Fresh|Restore"):
        await f.service.start_application(f.guide_id, f.receipt["deployment_id"], application_startup=f.inputs)
    assert len(f.service.launched) == 1
    assert entry.read() == b"uncertain Fresh retained\n"


async def test_actual_child_dispatch_preimports_meta_and_retains_original_lease(ordinary, monkeypatch):
    f = ordinary
    pin, _ = bootstrap.retain_bootstrap_admission(f.directory, f.receipt, f.inputs)
    imports, owners = [], []
    original = bootstrap.importlib.import_module
    def main(*, application_startup_factory):
        async def selected():
            with project_lock(f.destination) as lease:
                async with application_startup_factory(lease) as startup:
                    assert type(startup) is OrdinaryMetaStartup and startup.lease is lease
                    assert startup.gateway == f.gateway and startup.compiled_policies == ()
                    owners.append(startup)
        asyncio.run(selected())
    app = SimpleNamespace(__file__=str(f.destination / "app.py"), main=main)
    def record(name):
        imports.append(name)
        return app if name == "app" else original(name)
    monkeypatch.setattr(bootstrap.importlib, "import_module", record)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    await asyncio.to_thread(bootstrap.run_child, str(f.directory), pin["path"], pin["sha256"])
    assert imports == ["meerkat_meta.builder", "meerkat_meta.runtime", "app"]
    assert len(owners) == 1 and owners[0].runtime is None
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
