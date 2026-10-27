"""Real extracted GET handlers and query validation, without server-global stores."""
import ast
from copy import deepcopy
import logging
from pathlib import Path

from fastapi import FastAPI, HTTPException, Query
from fastapi.testclient import TestClient
import pytest

from launchkit.projects import DraftConflict


@pytest.fixture
def api():
    calls = []
    class Service:
        async def options(self, *args, **kwargs):
            calls.append(("options", args, deepcopy(kwargs)))
            return {"current": True, "source_digest": "a" * 64}

        async def settings_options(self, *args, **kwargs):
            calls.append(("settings", args, deepcopy(kwargs)))
            return {"scope": "verified-installed-private-configuration"}

    # Compile these exact shipping handlers; do not import server.py's global
    # stores, signing authority or native services into this offline fixture.
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    tree = ast.parse(source.read_text())
    selected = {"guide_result", "deployment_options", "installed_deployment_settings"}
    body = [node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and node.name in selected]
    assert len(body) == 3
    app = FastAPI()
    namespace = {"app": app, "deployment": Service(), "Query": Query, "HTTPException": HTTPException,
                 "DraftConflict": DraftConflict, "ArchitectUnavailable": type("Unavailable", (RuntimeError,), {}),
                 "LOG": logging.getLogger(__name__)}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    return TestClient(app), calls


@pytest.mark.parametrize("path,identity,kind", [
    ("/api/guide/owned/deployment/options?build_id=checked", "checked", "options"),
    ("/api/guide/owned/deployment/settings?deployment_id=installed", "installed", "settings"),
])
def test_optional_port_query_preserves_legacy_omitted_call_and_forwards_explicit_value(api, path, identity, kind):
    client, calls = api
    assert client.get(path).status_code == 200
    assert calls == [(kind, ("owned", identity), {})]
    assert client.get(path + "&console_port=59123").status_code == 200
    assert calls[-1] == (kind, ("owned", identity), {"console_port": 59123})


@pytest.mark.parametrize("value", ["0", "1023", "65536", "true", "not-a-port", "4110.5"])
@pytest.mark.parametrize("endpoint", ["options?build_id=checked", "settings?deployment_id=installed"])
def test_invalid_port_query_admits_no_probe_or_work(api, value, endpoint):
    client, calls = api
    assert client.get(f"/api/guide/owned/deployment/{endpoint}&console_port={value}").status_code == 422
    assert calls == []
