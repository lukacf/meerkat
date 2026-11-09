"""Private wizard fields declared by ordinary application source."""
from __future__ import annotations

import json
from pathlib import Path
import re

RESERVED = {"PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TEMP", "TMP",
            "FILES_ROOT", "SSL_CERT_FILE", "SSL_CERT_DIR", "NODE_OPTIONS", "BASH_ENV", "ENV",
            "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"}
CONSTRAINTS = {"min_length", "max_length", "nonblank", "type", "minimum", "maximum"}
MAX_VALUE_LENGTH = 32768
MAX_INTEGER = 2**53 - 1


def configuration_issue(field, value):
    """Describe invalid input using source metadata, never the entered value."""
    message = None
    if not isinstance(value, str) or len(value) > MAX_VALUE_LENGTH or "\x00" in value:
        message = "Use at most 32768 characters without a null character."
    elif field.get("required") and not value.strip():
        message = "Enter a required value that is not only whitespace."
    elif not value:
        return None  # Optional absence is distinct from an invalid supplied value.
    elif field.get("nonblank") and not value.strip():
        message = "Enter a value that is not only whitespace."
    elif len(value) < field.get("min_length", 0):
        message = f"Use at least {field['min_length']} characters."
    elif len(value) > field.get("max_length", MAX_VALUE_LENGTH):
        message = f"Use at most {field['max_length']} characters."
    elif field.get("type") == "integer":
        number = None
        if re.fullmatch(r"[+-]?[0-9]+", value):
            digits = value[1:] if value[0] in "+-" else value
            significant = digits.lstrip("0") or "0"
            # Compare bounded significant digits, retaining the original text.
            if len(significant) <= 16:
                number = int(significant) * (-1 if value[0] == "-" else 1)
        if number is None or not field["minimum"] <= number <= field["maximum"]:
            message = f"Enter a whole number between {field['minimum']} and {field['maximum']}."
    if message is None:
        return None
    return {"key": field["key"], "label": field["label"], "message": message}


def configuration_issues(fields, values):
    """Validate merged configuration without normalizing or exposing values."""
    return [issue for field in fields
            if (issue := configuration_issue(field, values.get(field["key"], ""))) is not None]


def validate_configuration_value(field, value):
    issue = configuration_issue(field, value)
    if issue is not None:
        raise ValueError(f"{issue['label']} ({issue['key']}): {issue['message']}")
    return value


def validate_configuration_values(fields, values):
    for field in fields:
        validate_configuration_value(field, values.get(field["key"], ""))


def validate_application_fields(value, existing_keys=()):
    if not isinstance(value, dict) or set(value) != {"fields"}:
        raise ValueError("Application configuration declares a fields list.")
    rows = value["fields"]
    if not isinstance(rows, list) or len(rows) > 100:
        raise ValueError("Application configuration has too many fields.")
    seen = set(existing_keys)
    result = []
    for row in rows:
        if not isinstance(row, dict) or set(row) - ({"key", "label", "description", "secret", "required", "default"} | CONSTRAINTS):
            raise ValueError("An application configuration field has unsupported properties.")
        key = row.get("key")
        if (not isinstance(key, str) or not re.fullmatch(r"[A-Z][A-Z0-9_]{0,79}", key)
                or key in RESERVED or key.startswith(("MOBKIT_", "PYTHON", "DYLD_", "LD_"))):
            raise ValueError("An application configuration name is invalid or reserved for its host.")
        if key in seen:
            raise ValueError("Application configuration names must be distinct from native setup fields.")
        seen.add(key)
        label, description = row.get("label"), row.get("description", "")
        if (not isinstance(label, str) or not label.strip() or len(label) > 200
                or not isinstance(description, str) or len(description) > 2000):
            raise ValueError("Describe each application configuration field with a concise label.")
        if type(row.get("secret")) is not bool or type(row.get("required")) is not bool:
            raise ValueError("Application configuration must declare secret and required booleans.")
        if re.search(r"(?:TOKEN|SECRET|PASSWORD|API_KEY|CREDENTIAL)", key) and not row["secret"]:
            raise ValueError("Credential fields must be private.")
        default = row.get("default", "")
        if not isinstance(default, str) or len(default) > 32768 or "\x00" in default:
            raise ValueError("An application configuration default is invalid.")
        if row["secret"] and default:
            raise ValueError("Private application fields cannot include source defaults.")
        field = {"key": key, "label": label.strip(), "description": description,
                 "secret": row["secret"], "required": row["required"], "application": True}
        for name in ("min_length", "max_length"):
            if name in row:
                if type(row[name]) is not int or not 0 <= row[name] <= MAX_VALUE_LENGTH:
                    raise ValueError(f"{field['label']} ({key}): Length limits must be integers from 0 to 32768.")
                field[name] = row[name]
        if field.get("min_length", 0) > field.get("max_length", MAX_VALUE_LENGTH):
            raise ValueError(f"{field['label']} ({key}): Minimum length cannot exceed maximum length.")
        if field["required"] and field.get("max_length", MAX_VALUE_LENGTH) == 0:
            raise ValueError(f"{field['label']} ({key}): A required field must allow a nonempty value.")
        if "nonblank" in row:
            if type(row["nonblank"]) is not bool:
                raise ValueError(f"{field['label']} ({key}): nonblank must be a boolean.")
            field["nonblank"] = row["nonblank"]
        if "type" in row:
            if row["type"] != "integer":
                raise ValueError(f"{field['label']} ({key}): Only integer type constraints are supported.")
            if any(type(row.get(name)) is not int or not -MAX_INTEGER <= row[name] <= MAX_INTEGER
                   for name in ("minimum", "maximum")):
                raise ValueError(f"{field['label']} ({key}): Declare minimum and maximum within the safe-integer range.")
            if row["minimum"] > row["maximum"]:
                raise ValueError(f"{field['label']} ({key}): Minimum cannot exceed maximum.")
            closest = 0 if row["minimum"] <= 0 <= row["maximum"] else row["minimum"] if row["minimum"] > 0 else row["maximum"]
            if len(str(closest)) > field.get("max_length", MAX_VALUE_LENGTH):
                raise ValueError(f"{field['label']} ({key}): Maximum length cannot represent any integer in this range.")
            field.update(type="integer", minimum=row["minimum"], maximum=row["maximum"])
        elif "minimum" in row or "maximum" in row:
            raise ValueError(f"{field['label']} ({key}): Numeric limits require integer type.")
        if default:
            validate_configuration_value(field, default)
            field["default"] = default
        result.append(field)
    return result


def application_fields(root, existing_keys=()):
    path = Path(root) / "application/configuration.json"
    if not path.exists():
        return []
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 256_000:
        raise ValueError("Application configuration must be a bounded ordinary JSON file.")
    return validate_application_fields(json.loads(path.read_text()), existing_keys)
