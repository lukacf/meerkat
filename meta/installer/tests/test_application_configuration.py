"""A generated application's setup must be usable without editing private files."""
import importlib.util
import json
from pathlib import Path

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.templates_v2.application_configuration import (
    MAX_INTEGER, application_fields, configuration_issue, validate_application_fields,
    validate_configuration_value, validate_configuration_values,
)


def field(**changes):
    return {"key": "OBSERVATORY_TOKEN", "label": "Observatory access token", "description": "Provided by your observatory.",
            "secret": True, "required": True, **changes}


@pytest.mark.parametrize("changes", [
    {"key": "HOME"}, {"key": "PYTHONPATH"}, {"key": "MOBKIT_PORT"},
    {"key": "OPENAI_API_KEY"}, {"key": "ANTHROPIC_API_KEY"}, {"key": "GEMINI_API_KEY"},
    {"default": "source-credential"}, {"secret": False}, {"required": "yes"},
])
def test_authored_setup_metadata_cannot_override_host_or_embed_credentials(changes):
    with pytest.raises(ValueError):
        validate_application_fields({"fields": [field(**changes)]})


def test_private_application_field_metadata_never_contains_a_configured_value(tmp_path):
    folder = tmp_path / "application"
    folder.mkdir()
    (folder / "configuration.json").write_text(json.dumps({"fields": [field(), field(
        key="OBSERVATORY_URL", label="Observatory address", secret=False, default="https://example.org")]}))
    fields = application_fields(tmp_path, {"OPENAI_API_KEY"})
    assert fields[0]["application"] and fields[0]["secret"] and fields[0]["required"]
    assert fields[1]["default"] == "https://example.org"
    assert "value" not in fields[0] and "default" not in fields[0]
    with pytest.raises(ValueError, match="distinct"):
        application_fields(tmp_path, {"OBSERVATORY_TOKEN"})


@pytest.mark.parametrize("changes", [
    {"min_length": True}, {"max_length": -1}, {"min_length": 32769},
    {"min_length": 8, "max_length": 7}, {"nonblank": "yes"},
    {"max_length": 0},
    {"type": "number"}, {"type": "integer", "minimum": 1},
    {"type": "integer", "minimum": False, "maximum": 10},
    {"type": "integer", "minimum": 10, "maximum": 1},
    {"type": "integer", "minimum": 100, "maximum": 200, "max_length": 2},
    {"type": "integer", "minimum": -99, "maximum": -1, "max_length": 1},
    {"type": "integer", "minimum": 0, "maximum": MAX_INTEGER + 1},
    {"minimum": 0, "maximum": 10},
    {"secret": False, "key": "OBSERVATORY_NAME", "min_length": 12, "default": "tiny-marker"},
    {"secret": False, "key": "OBSERVATORY_NAME", "max_length": 2, "default": "long-marker"},
    {"secret": False, "key": "OBSERVATORY_NAME", "default": "   "},
    {"secret": False, "key": "OBSERVATORY_PORT", "type": "integer", "minimum": 1024,
     "maximum": 65535, "default": "outside-marker"},
])
def test_declarations_and_source_defaults_must_have_consistent_scalar_constraints(changes):
    with pytest.raises(ValueError) as failure:
        validate_application_fields({"fields": [field(**changes)]})
    assert "marker" not in str(failure.value)


def test_valid_constraints_and_defaults_are_public_metadata_without_value_normalization():
    rows = validate_application_fields({"fields": [
        field(min_length=12, max_length=80, nonblank=True),
        field(key="OBSERVATORY_PORT", label="Listener port", secret=False, required=False,
              type="integer", minimum=1024, maximum=65535, default="+04111"),
    ]})
    assert rows[0]["min_length"] == 12 and rows[0]["max_length"] == 80 and rows[0]["nonblank"] is True
    assert rows[1]["default"] == "+04111" and rows[1]["type"] == "integer"
    secret = " \tprivate value\t "
    assert validate_configuration_value(rows[0], secret) == secret
    validate_configuration_values(rows, {"OBSERVATORY_TOKEN": secret, "OBSERVATORY_PORT": "+04111"})
    assert configuration_issue(rows[1], "") is None  # Optional removal remains valid.


@pytest.mark.parametrize("value", ["999", "65536", "4111.0", " 4111", "4111 ", "four-private-marker", "\u0664\u0661\u0661\u0661"])
def test_integer_input_rejects_unsupported_or_out_of_range_values_without_echo(value):
    declared = validate_application_fields({"fields": [field(
        key="OBSERVATORY_PORT", label="Listener port", secret=False,
        type="integer", minimum=1024, maximum=65535)]})[0]
    issue = configuration_issue(declared, value)
    assert issue == {"key": "OBSERVATORY_PORT", "label": "Listener port",
                     "message": "Enter a whole number between 1024 and 65535."}


def test_required_whitespace_is_rejected_without_requiring_new_metadata():
    declared = validate_application_fields({"fields": [field()]})[0]
    with pytest.raises(ValueError, match="not only whitespace"):
        validate_configuration_value(declared, " \t\n ")


def test_optional_nonblank_constraint_rejects_whitespace_but_accepts_absence():
    declared = validate_application_fields({"fields": [field(required=False, nonblank=True, min_length=4)]})[0]
    assert validate_configuration_value(declared, "") == ""
    with pytest.raises(ValueError, match="not only whitespace"):
        validate_configuration_value(declared, "    ")


def test_integer_length_constraints_support_exact_long_zero_padded_text():
    declared = validate_application_fields({"fields": [field(
        type="integer", minimum=-9, maximum=9, min_length=5000, max_length=5000)]})[0]
    value = "-" + "0" * 4998 + "1"
    assert validate_configuration_value(declared, value) == value
    with pytest.raises(ValueError, match="whole number"):
        validate_configuration_value(declared, "1" * 5000)


def test_destination_doctor_checks_application_prerequisites_without_printing_their_values(tmp_path, monkeypatch, capsys):
    generated = generate_files(starter_blueprint())
    spec = json.loads(generated["deployment.json"])
    for name, content in generated.items():
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    folder = tmp_path / "application"
    folder.mkdir()
    (folder / "__init__.py").write_text("def build(context):\n    return object()\n")
    (folder / "configuration.json").write_text(json.dumps({"fields": [field()]}))
    (tmp_path / ".bin").mkdir()
    for name in ("rpc_gateway", "mobkit_gateway"):
        (tmp_path / ".bin" / name).touch()
    monkeypatch.syspath_prepend(str(tmp_path))
    definition = importlib.util.spec_from_file_location("configuration_test_host", tmp_path / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    monkeypatch.setenv("OPENAI_API_KEY", "private-provider-test-value")
    monkeypatch.delenv("OBSERVATORY_TOKEN", raising=False)
    assert host.doctor(spec) is False
    assert "[needs setup] Observatory access token" in capsys.readouterr().out
    monkeypatch.setenv("OBSERVATORY_TOKEN", "private-domain-test-value")
    assert host.doctor(spec) is True
    output = capsys.readouterr().out
    assert "[ready] Observatory access token" in output
    assert "private-domain-test-value" not in output and "private-provider-test-value" not in output
