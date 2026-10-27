"""Stage exact selected Toolkit service sources for an independent wheel."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
MODULES = (
    "__init__.py", "httpx_client.py", "connectors/__init__.py",
    "connectors/_httpx_stream.py", "connectors/gmail.py", "connectors/slack.py",
    "connectors/gmail_content.py", "connectors/_google_errors.py",
    "connectors/google_calendar.py", "connectors/google_drive.py",
    "adapters/__init__.py", "adapters/_mcp_admission.py", "adapters/services_config.py", "adapters/services_mcp.py",
    "adapters/gmail_composition.py", "adapters/_connector_call.py",
    "auth/__init__.py", "auth/google_oauth.py", "auth/slack_oauth.py",
)
GUIDES = ("SERVICES-HOST-CLIENT.md", "GOOGLE-OAUTH.md", "SLACK-OAUTH.md")


def stage(destination: Path) -> dict[str, object]:
    """Copy an exact allowlist into a new directory without installation or fetches."""
    payloads: dict[str, bytes] = {}
    mapping: dict[str, dict[str, str]] = {}
    guide_mapping: dict[str, dict[str, object]] = {}
    for name in MODULES:
        source = ROOT / "sdk/python/src/meerkat_toolkit" / name
        target = "src/meerkat_toolkit_services/" + name
        payloads[target] = source.read_bytes()
        mapping[target] = {"canonical_source": str(source.relative_to(ROOT)),
                           "sha256": hashlib.sha256(payloads[target]).hexdigest()}
    for name in GUIDES:
        source = ROOT / "sdk/python" / name
        raw = source.read_bytes()
        text = raw.decode("utf-8")
        replacements = [("meerkat_toolkit.", "meerkat_toolkit_services.")]
        if name == "SERVICES-HOST-CLIENT.md":
            replacements.append(("src/meerkat_toolkit/httpx_client.py", "httpx_client.py"))
        if name == "GOOGLE-OAUTH.md":
            replacements.append(("../../docs/coordination/20261004T003359.800515-claude-gcp-lead-45cc8b.txt",
                                 "https://github.com/lukacf/meerkat/issues/1631#issuecomment-5975028207"))
        if name == "SLACK-OAUTH.md":
            replacements.append(("It uses the existing `slack` extra's\nHTTPX and AnyIO dependencies.",
                                 "It uses this distribution's existing HTTPX and AnyIO dependencies."))
        for original, replacement in replacements:
            if original not in text:
                raise ValueError(f"expected guide projection text is absent: {name}")
            text = text.replace(original, replacement)
        target = "src/meerkat_toolkit_services/" + name
        payloads[target] = text.encode("utf-8")
        guide_mapping[target] = {
            "canonical_source": str(source.relative_to(ROOT)),
            "canonical_sha256": hashlib.sha256(raw).hexdigest(),
            "sha256": hashlib.sha256(payloads[target]).hexdigest(),
            "literal_replacements": [{"from": old, "to": new} for old, new in replacements],
        }
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        payloads[name] = (ROOT / "sdk/python" / name).read_bytes()
    for name in ("pyproject.toml", "README.md", "THIRD_PARTY_NOTICES.md"):
        payloads[name] = (HERE / name).read_bytes()
    payloads["src/meerkat_toolkit_services/GUIDE.md"] = payloads["README.md"]
    manifest = {
        "distribution": "meerkat-toolkit-services", "namespace": "meerkat_toolkit_services",
        "source_mapping": mapping,
        # This declaration ships in the same wheel as the exact entry member.
        # Consumers observe it only after verifying the selected wheel payload.
        "native_connector": {
            "schema_version": 1,
            "module": "meerkat_toolkit_services.adapters.services_mcp",
            "mode": "serve-native",
            "configuration_argument": "--config",
            "member": "meerkat_toolkit_services/adapters/services_mcp.py",
            "member_sha256": mapping["src/meerkat_toolkit_services/adapters/services_mcp.py"]["sha256"],
        },
        "guide_mapping": guide_mapping,
        "other_payload_sha256": {name: hashlib.sha256(data).hexdigest()
                                 for name, data in payloads.items() if name not in mapping},
        "scope": "Exact selected original Toolkit service and host-only OAuth protocol modules. "
                 "Guides project the standalone namespace and packaging references explicitly. "
                 "No private Gmail MIME builder, frontend, native host-control or Agno modules.",
    }
    payloads["src/meerkat_toolkit_services/PACKAGE-SOURCES.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    destination.mkdir()
    for name, data in payloads.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="new staging directory with an existing parent")
    arguments = parser.parse_args()
    stage(arguments.destination)
    print(str(arguments.destination.resolve()))


if __name__ == "__main__":
    main()
