"""Public source metadata for inert host sequencing tests, without credentials."""
import json


def write_console_sources(root, deployment):
    (root / "application").mkdir(exist_ok=True)
    (root / "deployment.json").write_text(json.dumps(deployment))
    (root / "application/console.json").write_text(json.dumps({
        "authentication": "local-password"}))
    (root / "application/configuration.json").write_text(json.dumps({"fields": [
        {"key": "CONSOLE_USER_ID", "label": "User", "secret": False, "required": True},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Password", "secret": True, "required": True},
    ]}))
