#!/usr/bin/env python3
"""A local credential interview. The deployment studio never receives secrets."""
import getpass
import json
import os
from pathlib import Path
import re
import sys

from connectors import load_environment

ROOT = Path(__file__).resolve().parent


def main():
    if not sys.stdin.isatty():
        print("Interactive setup needs a terminal. Edit .env locally, then run bash start.sh.")
        return
    path = ROOT / ".env"
    load_environment(path)
    spec = json.loads((ROOT / "deployment.json").read_text())
    keys = [(spec["provider"]["envKey"], "Your AI provider API key", True)]
    for connector in spec["connectors"]:
        for field in connector.get("environment", []):
            keys.append((field["key"], field["label"], field["secret"]))
    values = {}
    for line in path.read_text().splitlines() if path.exists() else []:
        if line and not line.startswith("#") and "=" in line:
            key = line.split("=", 1)[0]
            if re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
                values[key] = os.environ.get(key, "")
    print(f"\nFinish setting up {spec['name']}")
    print("Values stay in this folder's private .env file. Press Enter to keep an existing value or skip an optional connection.\n")
    for key, label, secret in keys:
        existing = os.environ.get(key, "")
        suffix = " [already configured]" if existing else ""
        prompt = f"{label}{suffix}: "
        if key == "TELEGRAM_ALLOWED_CHAT_IDS" and not existing and values.get("TELEGRAM_BOT_TOKEN"):
            print("Send your bot a message in Telegram first. This setup can show the chat IDs that contacted it.")
            discover = input("Look up recent chat IDs now? [y/N]: ").strip().lower()
            if discover == "y":
                from telegram_bridge import TelegramTransport, TelegramError
                try:
                    transport = TelegramTransport(values["TELEGRAM_BOT_TOKEN"])
                    updates = transport._request("getUpdates", {"limit": 30, "timeout": 0, "allowed_updates": ["message"]})
                    seen = set()
                    for update in updates:
                        message = update.get("message", {})
                        chat = message.get("chat", {})
                        chat_id = chat.get("id")
                        if type(chat_id) is int and chat_id not in seen and not message.get("from", {}).get("is_bot", True):
                            seen.add(chat_id)
                            print(f"  Chat ID {chat_id}")
                    if not seen:
                        print("No recent chat IDs found. Message your bot, then run setup again.")
                    print("Enter only the IDs you recognize and trust. Nothing is authorized automatically.")
                except (TelegramError, ValueError):
                    print("Could not look up chats. Check your bot token and ensure this bot has no existing webhook.")
        value = getpass.getpass(prompt) if secret else input(prompt)
        value = value.strip() or existing
        if value:
            if key == "TELEGRAM_ALLOWED_CHAT_IDS":
                from telegram_bridge import chat_allowlist
                chat_allowlist(value)
            values[key] = value
    path.write_text("# Private configuration. Never commit this file.\n" + "".join(
        f"{key}={json.dumps(value)}\n" for key, value in sorted(values.items())
    ))
    os.chmod(path, 0o600)
    print("\nSaved. Run bash start.sh --doctor to check setup.")


if __name__ == "__main__":
    main()
