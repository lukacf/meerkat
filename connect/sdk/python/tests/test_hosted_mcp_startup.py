"""Trusted startup argument transport only, without URL/auth acceptance claims."""
import hashlib
from types import SimpleNamespace

import pytest

from meerkat_meta._transport import PersistentTransport
from meerkat_toolkit.host_control import (
    ApplicationPreparation, ApplicationUpdateStartup, HostControlError,
    SelectedHostArtifact, ToolkitMobKitRuntime, ToolkitPersistentTransport,
)
from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime
from meerkat_toolkit.application_update import ToolkitApplicationUpdateRuntime
from test_application_preparation import selection as restored_selection


def artifact(tmp_path):
    path = tmp_path / "selected.json"; data = b'{"fixture":"startup input"}\n'; path.write_bytes(data)
    return SelectedHostArtifact(str(path), hashlib.sha256(data).hexdigest())


def selected(kind, tmp_path):
    if kind == "restore": return restored_selection()
    if kind == "prepare": return ApplicationPreparation(artifact(tmp_path), timeout=1)
    return ApplicationUpdateStartup(artifact(tmp_path), SelectedHostArtifact("/prior", "b" * 64), "/associations")


@pytest.mark.parametrize("kind", ["restore", "prepare", "update"])
@pytest.mark.parametrize("base", [None, "https://host.example/app/mcp", "native-validates-url"])
def test_actual_transport_start_has_exact_optional_argument_only(kind, base, tmp_path, monkeypatch):
    observed = []
    def before_spawn(self):
        observed.append(self._extra_args)
        raise RuntimeError("inert pre-spawn stop")
    monkeypatch.setattr(PersistentTransport, "start", before_spawn)
    transport = ToolkitPersistentTransport("/not-started", selected(kind, tmp_path), hosted_mcp_redirect_base=base)
    with pytest.raises(RuntimeError, match="pre-spawn"): transport.start()
    args = observed[0]
    assert args.count("--hosted-mcp-redirect-base") == int(base is not None)
    if base is not None: assert args[args.index("--hosted-mcp-redirect-base") + 1] == base
    assert not transport._extra_args
    assert transport._selection == selected(kind, tmp_path)


@pytest.mark.parametrize("kind", ["restore", "prepare", "update"])
@pytest.mark.parametrize("base", [None, "https://host.example/mount"])
def test_public_runtime_constructors_forward_without_modifying_model_config(kind, base, tmp_path):
    config = SimpleNamespace(gateway_bin="/not-started")
    source = selected(kind, tmp_path)
    if kind == "prepare": runtime = ToolkitApplicationRuntime(config, source, lambda _: None, hosted_mcp_redirect_base=base)
    elif kind == "update": runtime = ToolkitApplicationUpdateRuntime(config, source, hosted_mcp_redirect_base=base)
    else: runtime = ToolkitMobKitRuntime(config, source, hosted_mcp_redirect_base=base)
    transport = runtime._make_gateway_transport()
    assert transport._hosted_mcp_redirect_base == base
    assert vars(config) == {"gateway_bin": "/not-started"}


@pytest.mark.parametrize("base", ["", "   ", True, 7, [], "https://host/\nnext", "https://host/\0", "https://host/\x7f", "https://host/\x85"])
@pytest.mark.parametrize("kind", ["transport", "restore", "prepare", "update"])
def test_invalid_shape_refuses_before_file_or_process_access(base, kind):
    config = SimpleNamespace(gateway_bin="/not-started")
    absent = SelectedHostArtifact("/must-not-open", "a" * 64)
    with pytest.raises(HostControlError, match="hosted MCP redirect base"):
        if kind == "transport": ToolkitPersistentTransport("/not-started", restored_selection(), hosted_mcp_redirect_base=base)
        elif kind == "restore": ToolkitMobKitRuntime(config, restored_selection(), hosted_mcp_redirect_base=base)
        elif kind == "prepare": ToolkitApplicationRuntime(config, ApplicationPreparation(absent, timeout=1), lambda _: None, hosted_mcp_redirect_base=base)
        else: ToolkitApplicationUpdateRuntime(config, ApplicationUpdateStartup(absent, absent, "/associations"), hosted_mcp_redirect_base=base)
