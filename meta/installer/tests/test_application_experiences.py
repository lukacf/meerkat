import hashlib
import json

import pytest

from launchkit.application_experiences import (declared_application_experiences,
                                               project_application_experiences,
                                               resolve_application_experiences)
from launchkit.compiler import PackageFile

SOURCE = "a" * 64
DEFINITION = {"port": 8787, "environment": [
    {"key": "SERVICE_TOKEN", "secret": True},
    {"key": "NATIVE_UI_PORT", "secret": False},
]}


def source(entries=None, fields=None):
    files = []
    if entries is not None:
        files.append(PackageFile("application/experience.json", json.dumps({"entries": entries})))
    if fields is not None:
        files.append(PackageFile("application/configuration.json", json.dumps({"fields": fields})))
    return files


def field(key="APP_UI_PORT", secret=False):
    return {"key": key, "label": "Application UI port", "secret": secret,
            "required": False, "default": "" if secret else "8800"}


def project(files, **kwargs):
    return project_application_experiences(files, DEFINITION, source_digest=SOURCE, **kwargs)


def test_optional_metadata_without_custom_interface_is_empty_and_not_verified():
    result = project([])
    assert result["entries"] == [] and result["metadata_sha256"] is None
    assert result["source_digest"] == SOURCE and result["verified"] is False


def test_source_bound_interfaces_support_package_list_and_archive_bytes_without_imports():
    files = source([{"label": "Reading room", "path": "/reading", "port": 8800}])
    digest = hashlib.sha256(files[0].content.encode()).hexdigest()
    archived = {item.path: (item.content.encode(), 0o644) for item in files}
    first = project(files, console_url="http://127.0.0.1:8787/console")
    second = project(archived, console_url="http://127.0.0.1:8787/console")
    assert first == second
    assert first["metadata_sha256"] == digest
    assert first["entries"][0]["url"] == "http://127.0.0.1:8800/reading"
    assert first["entries"][0]["status"] == "declared"
    assert first["verified"] is False and first["entries"][0]["verified"] is False
    # Source content is never imported, including application/__init__.py.
    files.append(PackageFile("application/__init__.py", "raise RuntimeError('must not import')"))
    assert project(files, console_url="http://127.0.0.1:8787/console") == first


def test_local_link_requires_current_installed_console_observation():
    files = source([{"label": "User app", "path": "/", "port": 8800}])
    row = project(files)["entries"][0]
    assert row["url"] is None and "Start and verify" in row["reason"]
    same_origin = source([{"label": "Extra view", "path": "/views/"}])
    row = project(same_origin, console_url="http://[::1]:9000/console/")["entries"][0]
    assert row["url"] == "http://[::1]:9000/views/"


def test_private_nonsecret_port_projection_never_exposes_other_configuration():
    files = source([{"id": "reader", "label": "Reading room", "path": "/", "port_field": "APP_UI_PORT"}], [field()])
    result = project(files, console_url="http://127.0.0.1:8787/console",
                     configuration_values={"APP_UI_PORT": "8801", "SERVICE_TOKEN": "PRIVATE-DO-NOT-RETURN"})
    assert result["entries"][0]["url"] == "http://127.0.0.1:8801/"
    assert "PRIVATE-DO-NOT-RETURN" not in json.dumps(result)
    missing = project(files, console_url="http://127.0.0.1:8787/console")["entries"][0]
    assert missing["url"] is None and "Configure" in missing["reason"]


@pytest.mark.parametrize("value", [True, 80, 65536, "not-a-port", "8800?token=PRIVATE"])
def test_invalid_private_port_values_are_not_echoed_or_made_clickable(value):
    files = source([{"label": "User app", "path": "/", "port_field": "APP_UI_PORT"}], [field()])
    row = project(files, console_url="http://127.0.0.1:8787/console",
                  configuration_values={"APP_UI_PORT": value})["entries"][0]
    assert row["url"] is None
    assert str(value) not in row["reason"]


def test_remote_links_require_explicit_tunnel_for_each_exact_destination_port():
    files = source([{"label": "Custom app", "path": "/reading", "port": 8800},
                    {"label": "Admin view", "path": "/extra-admin"}])
    result = project(files, remote=True, console_url="http://127.0.0.1:8787/console")
    assert all(row["url"] is None for row in result["entries"])
    mapped = project(files, remote=True, tunnel_mappings={8800: 9900})
    assert mapped["entries"][0]["url"] == "http://127.0.0.1:9900/reading"
    assert mapped["entries"][0]["tunnel_mapping"] == {"destination_port": 8800, "local_port": 9900}
    assert mapped["entries"][1]["url"] is None
    assert mapped["verified"] is False


@pytest.mark.parametrize("path", ["https://outside.invalid/ui", "//outside.invalid/ui", "/../admin",
                                  "/x/./admin", "/x%2f..%2fadmin", "/%252e%252e/admin",
                                  "/%25252e%25252e/admin", "/ui?token=PRIVATE", "/ui#PRIVATE",
                                  "/ui%3ftoken=PRIVATE", "/ui%23PRIVATE", "/ui\\admin",
                                  "/ui%5cadmin", "/ui%00", "/ui\n", "/ui%ff"])
def test_external_credential_and_traversal_paths_fail_closed(path):
    files = source([{"label": "Valid first", "path": "/", "port": 8800},
                    {"label": "Unsafe second", "path": path, "port": 8800}])
    with pytest.raises(ValueError):
        project(files, console_url="http://127.0.0.1:8787/console")


@pytest.mark.parametrize("entry", [
    {"label": "Secret", "path": "/", "port_field": "SERVICE_TOKEN"},
    {"label": "Missing", "path": "/", "port_field": "UNDECLARED"},
    {"label": "Ambiguous", "path": "/", "port": 8800, "port_field": "NATIVE_UI_PORT"},
    {"label": "Token", "path": "/", "token": "PRIVATE"},
    {"label": "URL", "path": "/", "url": "https://outside.invalid"},
    {"label": "Privilege", "path": "/", "port": 443},
    {"label": "Boolean", "path": "/", "port": True},
])
def test_unsupported_fields_and_secret_or_undeclared_ports_are_rejected(entry):
    with pytest.raises(ValueError):
        declared_application_experiences(source([entry]), DEFINITION)


def test_source_declared_private_field_cannot_supply_link_port():
    files = source([{"label": "App", "path": "/", "port_field": "APP_UI_PORT"}], [field(secret=True)])
    with pytest.raises(ValueError, match="nonsecret"):
        project(files, configuration_values={"APP_UI_PORT": "8800"})


@pytest.mark.parametrize("url", ["http://outside.invalid:8787/console", "https://127.0.0.1:8787/console",
                                 "http://user:PRIVATE@127.0.0.1:8787/console", "http://127.0.0.1:8787/console?token=PRIVATE",
                                 "http://127.0.0.1:8787/console#PRIVATE", "http://127.0.0.1:8787/other"])
def test_observed_console_must_be_loopback_and_credential_free(url):
    with pytest.raises(ValueError, match="observed loopback"):
        project(source([{"label": "App", "path": "/"}]), console_url=url)


def test_duplicate_fields_identifiers_and_excessive_metadata_are_rejected():
    files = [PackageFile("application/experience.json", '{"entries":[],"entries":[]}')]
    with pytest.raises(ValueError, match="duplicate"):
        project(files)
    files = source([{"id": "same", "label": "First", "path": "/"},
                    {"id": "same", "label": "Second", "path": "/"}])
    with pytest.raises(ValueError, match="distinct"):
        project(files)
    files = [PackageFile("application/experience.json", " " * 64_001)]
    with pytest.raises(ValueError, match="size limit"):
        project(files)


def test_changed_metadata_changes_source_binding_but_never_adds_readiness():
    first = project(source([{"label": "App", "path": "/", "port": 8800}]))
    second = project_application_experiences(source([{"label": "New app", "path": "/", "port": 8800}]),
                                              DEFINITION, source_digest="b" * 64)
    assert first["metadata_sha256"] != second["metadata_sha256"]
    assert first["source_digest"] != second["source_digest"]
    assert all(result["verified"] is False for result in (first, second))


def test_shared_resolution_uses_observed_console_port_instead_of_compiled_remote_default():
    files = source([{"id": "extra", "label": "Extra view", "path": "/extra"}])
    result = resolve_application_experiences(files, DEFINITION, source_digest=SOURCE,
        console_url="http://127.0.0.1:9911/console")
    assert result["entries"][0]["destination_port"] == 9911
    assert result["entries"][0]["reason"] is None and result["verified"] is False
    row = project(files, remote=True, console_url="http://127.0.0.1:9911/console",
                  tunnel_mappings={8787: 8787, 9911: 9911})["entries"][0]
    assert row["url"] == "http://127.0.0.1:9911/extra"
    missing = project(files, remote=True, tunnel_mappings={8787: 8787})["entries"][0]
    assert missing["destination_port"] is None and missing["url"] is None


def test_shared_resolution_exposes_only_validated_nonsecret_port_values():
    files = source([{"id": "reader", "label": "Reader", "path": "/", "port_field": "APP_UI_PORT"}], [field()])
    result = resolve_application_experiences(files, DEFINITION, source_digest=SOURCE,
        configuration_values={"APP_UI_PORT": "9912", "SERVICE_TOKEN": "private-canary"})
    assert result["entries"][0]["destination_port"] == 9912
    assert "private-canary" not in json.dumps(result)
    result = resolve_application_experiences(files, DEFINITION, source_digest=SOURCE,
        configuration_values={"APP_UI_PORT": "private-canary"})
    assert result["entries"][0]["destination_port"] is None
    assert "private-canary" not in json.dumps(result)
