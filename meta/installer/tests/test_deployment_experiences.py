from datetime import datetime, timezone
import json
import os

import pytest

from launchkit.deployment import materialize, update_receipt
from launchkit.deployment_experiences import nonsecret_port_settings
from test_deployment import Guide, Implementation, Service, submit


async def prepared(tmp_path, *, metadata=None, field=False):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    implementation.extra_files["application/experience.json"] = json.dumps(metadata or {"entries": [
        {"id": "user-ui", "label": "Your dashboard", "path": "/app", **({"port_field": "UI_PORT"} if field else {})}]})
    if field:
        implementation.application_configuration = {"fields": [{"key": "UI_PORT", "label": "Interface port",
            "secret": False, "required": True, "description": "Private setup port for this application."}]}
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination, settings={"OPENAI_API_KEY": "private-token", **({"UI_PORT": "18199"} if field else {})})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped")
    return service, directory, destination


async def observed_runtime(service, directory):
    # This deterministic receipt fixture tests projection boundaries, not native
    # startup. Owned real native startup has its separate destination proof.
    service.tasks[directory.name] = object()
    update_receipt(directory, status="running", health="ready", process_live=True,
        console_url="http://127.0.0.1:18088/console", health_checked_at=datetime.now(timezone.utc).isoformat(),
        bootstrap={"complete": True, "ready": True, "counts": {"broken": 0}})


async def test_stopped_source_has_declarations_without_clickable_or_verified_ui(tmp_path):
    service, _, _ = await prepared(tmp_path)
    result = await service.experiences(service.guide.identity)
    assert result["source_digest"] == "a" * 64 and result["scope"] == "declared-source"
    assert result["entries"][0]["label"] == "Your dashboard" and result["entries"][0]["url"] is None
    assert result["verified"] is False and "private-token" not in json.dumps(result)


async def test_loopback_links_resolve_only_declared_nonsecret_port_without_private_values(tmp_path):
    service, directory, destination = await prepared(tmp_path, field=True)
    await observed_runtime(service, directory)
    result = await service.experiences(service.guide.identity)
    assert result["entries"][0]["url"] == "http://127.0.0.1:18199/app"
    assert result["entries"][0]["status"] == "declared" and result["entries"][0]["verified"] is False
    assert "private-token" not in str(result) and str(destination) not in str(result)
    update_receipt(directory, health="unknown")
    assert (await service.experiences(service.guide.identity))["entries"][0]["url"] is None
    service.tasks.clear()


async def test_modified_installed_source_cannot_publish_source_bound_links(tmp_path):
    service, directory, destination = await prepared(tmp_path)
    await observed_runtime(service, directory)
    (destination / "application/experience.json").write_text('{"entries": []}')
    result = await service.experiences(service.guide.identity)
    assert result["entries"] == [] and "could not be verified" in result["error"]
    assert result["source_digest"] is None
    service.tasks.clear()


@pytest.mark.parametrize("metadata", [{"entries": [{"label": "Unsafe", "path": "//outside.example"}]},
    {"entries": [{"label": "Unsafe", "path": "/ui", "port_field": "OPENAI_API_KEY"}]}])
async def test_bad_metadata_is_nonblocking_and_excludes_private_parse_failures(tmp_path, metadata):
    service, directory, _ = await prepared(tmp_path, metadata=metadata)
    result = await service.experiences(service.guide.identity)
    assert result["entries"] == [] and result["verified"] is False and result["error"]
    assert "private-token" not in str(result)
    assert (await service.status(service.guide.identity))["status"] == "stopped"


def test_nonsecret_port_reader_rejects_linked_or_public_configuration(tmp_path):
    destination = tmp_path / "app"
    destination.mkdir()
    (destination / ".env").write_text('UI_PORT="18199"\nOPENAI_API_KEY="private-token"\n')
    os.chmod(destination / ".env", 0o600)
    assert nonsecret_port_settings(destination, {"UI_PORT"}) == {"UI_PORT": "18199"}
    os.chmod(destination / ".env", 0o644)
    with pytest.raises(ValueError, match="owned private"):
        nonsecret_port_settings(destination, {"UI_PORT"})
    (destination / ".env").rename(destination / "actual-env")
    (destination / ".env").symlink_to(destination / "actual-env")
    with pytest.raises(OSError):
        nonsecret_port_settings(destination, {"UI_PORT"})
