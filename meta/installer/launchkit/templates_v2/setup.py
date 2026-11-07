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
from console_configuration import (console_account_issue, console_authentication_choice, console_configuration,
    console_credential_issue, console_password_issue, validate_console_configuration_values)

ROOT = Path(__file__).resolve().parent
ACCOUNT_PROMPT = "Named account to add or to change its password (Enter finishes, :remove NAME removes): "


def manage_named_accounts(installation_account):
    """Keep named Console accounts as password hashes in the private .mobkit folder."""
    from console_auth import console_accounts_for_change, hash_console_password, save_console_accounts
    accounts, original = console_accounts_for_change(ROOT, installation_account)
    before = dict(accounts)
    print("\nNamed Console accounts: one per person, besides this installation's own account ("
          + installation_account + "). Each person signs in with their account name and password.")
    print("Current named accounts: " + (", ".join(accounts) if accounts else "none") + ".")
    while True:
        entry = input(ACCOUNT_PROMPT).strip()
        if not entry:
            break
        if entry.startswith(":remove "):
            name = entry.removeprefix(":remove ").strip()
            print(f"Removed {name}." if accounts.pop(name, None) is not None else f"There is no named account {name}.")
            continue
        issue = console_account_issue(entry, installation_account)
        if issue is not None:
            print(issue)
            continue
        while True:
            password = getpass.getpass(f"Password for {entry} (Enter keeps the account unchanged): ")
            if not password:
                break
            repeated = getpass.getpass(f"Repeat the password for {entry}: ")
            issue = console_password_issue(password)
            if issue is None and repeated != password:
                issue = "The two passwords differ."
            if issue is not None:
                print(issue + " Enter the password again.")
                continue
            accounts[entry] = hash_console_password(password)
            print(f"Saved the password for {entry}.")
            break
    if accounts != before:
        save_console_accounts(ROOT, accounts, original, installation_account)
        print("Named accounts saved. Restart the application to apply them.")


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
    content = "# Local private configuration. Never commit this file.\n" + "".join(
        f"{key}={json.dumps(value)}\n" for key, value in sorted(values.items()))
    if len(content.encode("utf-8")) > 256 * 1024:
        raise ValueError("The private configuration exceeded its safe size. No values were changed. Reduce the saved setting values and run setup again.")
    descriptor, temporary_name = tempfile.mkstemp(prefix=".env.", suffix=".pending", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w") as output:
            output.write(content)
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
    authentication = console_authentication_choice(ROOT, fields) if authenticated_console else None
    named = authentication == "local-accounts"
    if named:
        from console_auth import load_console_accounts
        # The installation's own account cannot take a named account's name.
        existing_accounts = set(load_console_accounts(ROOT, None))
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
                issue = console_credential_issue(field, value, authentication) if authenticated_console else None
                if issue is not None:
                    raise ValueError(f"{issue['label']} ({issue['key']}): {issue['message']}")
                if named and key == "CONSOLE_USER_ID" and value in existing_accounts:
                    raise ValueError("That name is a named account. Choose another name for the installation's "
                                     "own account, or remove that named account first.")
            except ValueError as error:
                print(str(error))
                continue
            if value:
                values[key] = value
            break
    validate_configuration_values([field for field in fields if field["key"] not in generated_keys], values)
    if authenticated_console:
        validate_console_configuration_values(fields, values, authentication)
    save_values(path, values, original)
    if named:
        manage_named_accounts(values["CONSOLE_USER_ID"])
    if authentication == "oidc":
        port = values.get("MOBKIT_PORT") or os.environ.get("MOBKIT_PORT") or spec["port"]
        print("People sign in to the Console at its identity provider. Register the Console's sign-in "
              f"callback with the provider: http://127.0.0.1:{port}/login/callback here, or the same path "
              "on the Console's public address.")
    if local_knowledge:
        print("Preparing the local knowledge service credentials and project space...")
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
