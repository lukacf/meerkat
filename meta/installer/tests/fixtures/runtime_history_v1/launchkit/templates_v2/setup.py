#!/usr/bin/env python3
"""Configure credentials only on the deployment computer."""
import getpass
import json
import os
from pathlib import Path
import re
import sys
import subprocess
import stat
import tempfile

from mcp_connections import configuration_value
from application_configuration import application_fields, validate_configuration_value, validate_configuration_values
from console_configuration import console_configuration, console_credential_issue, validate_console_configuration_values

ROOT = Path(__file__).resolve().parent


def private_values(path):
    if path.is_symlink():
        raise ValueError("The private configuration is a link. No values were changed.")
    if not path.exists():
        return {}, None
    info = path.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_size > 4_000_000:
        raise ValueError("The private configuration must be an ordinary file owned by you.")
    original = path.read_bytes()
    values = {}
    try:
        lines = original.decode().splitlines()
    except UnicodeDecodeError:
        raise ValueError("Existing private configuration could not be read. No values were changed.") from None
    for line in lines:
        row = line.strip()
        if not row or row.startswith("#"):
            continue
        key, separator, value = row.partition("=")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            raise ValueError("Existing private configuration could not be read. No values were changed.")
        value = value.strip()
        if value.startswith('"'):
            try:
                value = json.loads(value)
            except ValueError:
                raise ValueError("Existing private configuration could not be read. No values were changed.") from None
        elif len(value) >= 2 and value[0] == value[-1] == "'":
            value = value[1:-1]
        if not isinstance(value, str) or len(value) > 32768 or "\x00" in value:
            raise ValueError("Existing private configuration contains an invalid value. No values were changed.")
        values[key] = value
    return values, original


def save_values(path, values, original):
    _, current = private_values(path)
    if current != original:
        raise ValueError("Your private configuration changed during setup. Run setup again; the newer values were preserved.")
    descriptor, temporary_name = tempfile.mkstemp(prefix=".env.", suffix=".pending", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w") as output:
            output.write("# Local private configuration. Never commit this file.\n")
            for key, value in sorted(values.items()):
                output.write(f"{key}={json.dumps(value)}\n")
            output.flush()
            os.fsync(output.fileno())
        _, current = private_values(path)
        if current != original:
            raise ValueError("Your private configuration changed during setup. Run setup again; the newer values were preserved.")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    if not sys.stdin.isatty():
        print("Open a terminal in this project folder and run .venv/bin/python setup.py. Setup asks for the values; no file editing is needed.")
        return 1
    spec = json.loads((ROOT / "deployment.json").read_text())
    path = ROOT / ".env"
    values, original = private_values(path)
    print(f"\nConfigure {spec['name']}. Values remain in this folder's private .env.\n")
    local_knowledge = spec["knowledge"]["mode"] == "local"
    generated_keys = {spec["knowledge"]["token_env"], spec["knowledge"]["space_env"]} if local_knowledge else set()
    fields = [*spec["environment"], *application_fields(ROOT, (field["key"] for field in spec["environment"]))]
    authenticated_console = console_configuration(ROOT, fields)
    for field in fields:
        key = field["key"]
        if key in generated_keys:
            continue
        description = field.get("description", "").strip()
        if description:
            print(description)
        fallback = configuration_value(field)
        current = values.get(key) or os.environ.get(key, "")
        keep_absent = original is not None and key not in values and not field["required"]
        suffix = " [not saved; Enter leaves unset]" if keep_absent else " [configured; Enter keeps value]" if current else f" [Enter uses {fallback}]" if fallback else " [optional]" if not field["required"] else ""
        if not field["required"]:
            suffix += " [:remove deletes the saved value; application defaults may still apply]"
        prompt = f"{field['label']}{suffix}: "
        while True:
            value = getpass.getpass(prompt) if field["secret"] else input(prompt)
            if value == ":remove":
                if field["required"] or field.get("automatic"):
                    print("This required or automatic setting cannot be removed. Enter a replacement or press Enter to keep it.")
                    continue
                values.pop(key, None)
                break
            if not value and keep_absent:
                break
            value = value or current or fallback
            try:
                validate_configuration_value(field, value)
                issue = console_credential_issue(field, value) if authenticated_console else None
                if issue is not None:
                    raise ValueError(f"{issue['label']} ({issue['key']}): {issue['message']}")
            except ValueError as error:
                print(str(error))
                continue
            if value:
                values[key] = value
            break
    validate_configuration_values([field for field in fields if field["key"] not in generated_keys], values)
    if authenticated_console:
        validate_console_configuration_values(fields, values)
    save_values(path, values, original)
    if local_knowledge:
        print("Preparing the local Elephant credentials and project space...")
        subprocess.run([sys.executable, str(ROOT / "knowledge/manage.py"), "setup"], check=True)
        print("Start the isolated knowledge service: .venv/bin/python knowledge/manage.py up")
    print("Saved. Run bash start.sh --doctor, then bash start.sh.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyboardInterrupt, EOFError):
        print("\nSetup interrupted. Run .venv/bin/python setup.py to continue.")
        raise SystemExit(1) from None
