"""All workbench entrypoints enforce the same private credential contract."""
import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit.blueprint import starter_blueprint
from launchkit.projects import ProjectStore


@pytest.mark.parametrize("connection", [
    {"id": "custom", "name": "Custom", "transport": "stdio", "command": "node", "args": ["--token", "literal-review-secret"]},
    {"id": "custom", "name": "Custom", "transport": "http", "url": "https://example.test/mcp", "headers": {"Authorization": "Bearer literal-review-secret ${DUMMY}"}},
    {"id": "custom", "name": "Custom", "transport": "http", "url": "https://example.test/mcp?api_key=literal-review-secret&x=${DUMMY}"},
])
def test_import_preview_download_and_save_reject_literal_connection_credentials(tmp_path, monkeypatch, connection):
    monkeypatch.setattr(server, "projects", ProjectStore(tmp_path))
    client = TestClient(server.app)
    blueprint = starter_blueprint().model_dump(mode="json")
    blueprint["connections"] = [connection]
    for path in ("import", "validate", "package"):
        response = client.post(f"/api/blueprint/{path}", json=blueprint)
        assert response.status_code == 422
        assert "literal-review-secret" not in response.text
    response = client.put(f"/api/projects/{blueprint['project']['slug']}", json={"blueprint": blueprint, "expected_revision": 0})
    assert response.status_code == 422
    assert "literal-review-secret" not in response.text
    assert not server.projects.list()
