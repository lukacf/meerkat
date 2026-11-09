"""Declared Console rules are pure, exact and independent of scalar field names."""
import json

import pytest

from launchkit.templates_v2.console_configuration import (
    CONSOLE_FIELDS, console_authentication, console_configuration, console_credentials_issues,
    validate_console_credentials,
)


@pytest.mark.parametrize("principal", ["", "owner marker", "owner\n", "owner/marker", "owner:marker", "\u00e5", "x" * 201, None, 42])
def test_principal_format_rejections_are_safe(principal):
    issues = console_credentials_issues(CONSOLE_FIELDS, {
        "CONSOLE_USER_ID": principal, "CONSOLE_LOGIN_PASSWORD": "valid-password-123",
    })
    assert len(issues) == 1 and issues[0]["key"] == "CONSOLE_USER_ID"
    assert issues[0]["message"] == "Use 1-200 ASCII letters, digits, or . _ @ + - for the Console user identity."
    with pytest.raises(ValueError, match="CONSOLE_USER_ID"):
        validate_console_credentials(principal, "valid-password-123")


@pytest.mark.parametrize("password", ["tiny-marker", "x" * 4097, None, 42])
def test_password_bounds_rejections_do_not_echo_values(password):
    issues = console_credentials_issues(CONSOLE_FIELDS, {
        "CONSOLE_USER_ID": "owner", "CONSOLE_LOGIN_PASSWORD": password,
    })
    assert issues == [{"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password",
                      "message": "Use 12-4096 characters for the Console login password."}]
    with pytest.raises(ValueError, match="12-4096") as failure:
        validate_console_credentials("owner", password)
    assert "tiny-marker" not in str(failure.value)


@pytest.mark.parametrize("password", ["private-marker-\ud800", "private-marker-\udfff"])
def test_password_encoding_rejections_are_safe(password):
    issues = console_credentials_issues(CONSOLE_FIELDS, {
        "CONSOLE_USER_ID": "owner", "CONSOLE_LOGIN_PASSWORD": password,
    })
    assert issues == [{"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password",
                      "message": "Use valid UTF-8 text for the Console login password."}]
    with pytest.raises(ValueError, match="valid UTF-8") as failure:
        validate_console_credentials("owner", password)
    assert "private-marker" not in str(failure.value)


@pytest.mark.parametrize("principal,password", [
    ("a", "x" * 12), ("a" * 200, "x" * 4096), ("owner_.@+-09", "  password\n\tvalue  "),
    ("owner", "\U0001f600" * 12),
])
def test_valid_exact_values_are_not_normalized(principal, password):
    values = {"CONSOLE_USER_ID": principal, "CONSOLE_LOGIN_PASSWORD": password}
    before = dict(values)
    assert console_credentials_issues(CONSOLE_FIELDS, values) == []
    validate_console_credentials(principal, password)
    assert values == before


def test_field_names_do_not_enable_authentication_without_console_metadata(tmp_path):
    assert console_configuration(tmp_path, CONSOLE_FIELDS) is False
    (tmp_path / "application").mkdir()
    (tmp_path / "application/configuration.json").write_text(json.dumps({"fields": list(CONSOLE_FIELDS)}))
    assert console_configuration(tmp_path, CONSOLE_FIELDS) is False
    assert console_authentication({"authentication": "local-password"}, CONSOLE_FIELDS) == "local-password"
    with pytest.raises(ValueError, match="local-password"):
        console_authentication({"authentication": "none"}, CONSOLE_FIELDS)


@pytest.mark.parametrize("content", [b'not-json-private-marker', b'\xff'])
def test_invalid_metadata_error_does_not_echo_source_bytes(tmp_path, content):
    (tmp_path / "application").mkdir()
    (tmp_path / "application/console.json").write_bytes(content)
    with pytest.raises(ValueError, match="valid UTF-8 JSON") as failure:
        console_configuration(tmp_path, CONSOLE_FIELDS)
    assert "private-marker" not in str(failure.value)


def test_dangling_console_metadata_link_is_rejected(tmp_path):
    (tmp_path / "application").mkdir()
    (tmp_path / "application/console.json").symlink_to(tmp_path / "missing.json")
    with pytest.raises(ValueError, match="ordinary JSON"):
        console_configuration(tmp_path, CONSOLE_FIELDS)
