"""Actual server selection with private custody specimens and inert artifacts.

The guide remains the original service. No native, provider, credentials,
Console or deployment process executes. Linux substitution tests file/root
custody only and does not qualify the fixture's native artifact.
"""
import ast
from pathlib import Path

import pytest

from launchkit.connect_application_startup import _ModelConfiguration
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.implementation import ImplementationService
from launchkit.projects import DraftConflict
from test_ordinary_application_registry import ordinary_registry, registry, product, no_product_execution, no_processes
from test_managed_model_factory_selection import model_source


CALLBACKS = (("application_setup_selection", "_selected_startup"),
    ("application_setup_preparation", "prepare_application"),
    ("application_setup_retention", "retain_installation_inputs"),
    ("application_setup_continuation", "continue_application_startup"),
    ("application_replacement_selection", "_selected_replacement"),
    ("application_host_selection", "selected_host"))


def server_selection(f):
    source = Path(__file__).parents[1] / "launchkit/server.py"
    tree = ast.parse(source.read_text())
    node = next(n for n in tree.body if isinstance(n, ast.FunctionDef)
        and n.name == "select_local_application_runtime")
    namespace = {"__name__": "launchkit.server", "__package__": "launchkit",
        "application_host": None, "implementation": f.implementation,
        "deployment": f.service, "guide": f.guide,
        "ImplementationService": ImplementationService,
        "LocalApplicationRegistry": LocalApplicationRegistry, "DraftConflict": DraftConflict}
    exec(compile(ast.Module(body=[node], type_ignores=[]), str(source), "exec"), namespace)
    f.implementation.prepare_workspace = ImplementationService.prepare_workspace.__get__(f.implementation)
    for name, _ in CALLBACKS:
        setattr(f.service, name, None)
    return namespace


async def test_explicit_application_model_owner_binds_original_registry_not_guide(ordinary_registry, monkeypatch):
    f = ordinary_registry
    value = model_source(f.root, monkeypatch)
    monkeypatch.setenv("XDG_CONFIG_HOME", value["credential_config_root"])
    models = _ModelConfiguration.observe(value)
    namespace = server_selection(f)
    select = namespace["select_local_application_runtime"]
    original_guide = f.guide
    host = select(f.target, ordinary_meta_gateway=f.gateway, model_configuration=models)
    assert type(host) is LocalApplicationRegistry
    assert host._selected_model_configuration() == value
    assert namespace["guide"] is original_guide and original_guide.architect is None
    assert select(f.target, ordinary_meta_gateway=f.gateway, model_configuration=models) is host
    with pytest.raises(DraftConflict, match="original application model configuration"):
        select(f.target, ordinary_meta_gateway=f.gateway)
    assert not f.service.tasks and not f.service.processes and not f.implementation.tasks


async def test_raw_model_configuration_cannot_select_application_credentials(ordinary_registry):
    f = ordinary_registry
    namespace = server_selection(f)
    with pytest.raises(TypeError, match="original application model configuration owner"):
        namespace["select_local_application_runtime"](f.target, ordinary_meta_gateway=f.gateway,
            model_configuration={"host_config": "/caller-file"})
    assert namespace["application_host"] is None
    assert not f.service.tasks and not f.service.processes
