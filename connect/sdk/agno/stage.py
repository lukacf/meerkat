"""Stage the shared Agno sources for a separate wheel, without installing anything."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
MODULES = ("__init__.py", "_mcp_admission.py", "_connector_call.py", "agno.py", "agno_guidance.py", "agno_profiles.py", "agno_transports.py", "agno_mcp.py")
NOTICES = ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno")


def project_guide(guide: str) -> str:
    """Project the existing canonical manual; selected guidance binds these bytes."""
    marker = "## Select a profile\n"
    if guide.count(marker) != 1:
        raise ValueError("Canonical profile guide section changed; review the component projection")
    guide = "# Profile configuration and runtime limits\n\n" + marker + guide.split(marker, 1)[1]
    # Earlier host/test receipts belong to the combined SDK artifact. Retain
    # configuration and runtime documentation without projecting those claims.
    native_start = guide.index("## Standard Meerkat configuration\n")
    native_end = guide.index("## Available operations and limits\n", native_start)
    guide = guide[:native_start] + guide[native_end:]
    guide = guide.split("## Verification\n", 1)[0]
    guide = guide.replace("meerkat_toolkit.adapters", "meerkat_toolkit_agno")
    guide = guide.replace("/absolute/path/repository/.venv-agno-mcp/bin/python", "/absolute/path/consuming-environment/bin/python")
    return guide.replace(".venv-agno-mcp/bin/python", "python")


def stage(destination: Path) -> dict[str, object]:
    """Copy an explicit source allowlist into a new directory; never rewrite modules."""
    payloads: dict[str, bytes] = {}
    sources: dict[str, dict[str, str]] = {}
    for name in MODULES:
        original = ROOT / "sdk/python/src/meerkat_toolkit/adapters" / name
        target = "src/meerkat_toolkit_agno/" + name
        payloads[target] = original.read_bytes()
        sources[target] = {
            "canonical_source": str(original.relative_to(ROOT)),
            "sha256": hashlib.sha256(payloads[target]).hexdigest(),
        }
    for name in NOTICES:
        payloads[name] = (ROOT / "sdk/python" / name).read_bytes()
    for name in ("pyproject.toml", "README.md", "THIRD_PARTY_NOTICES.md"):
        payloads[name] = (HERE / name).read_bytes()
    guide_source = ROOT / "sdk/python/AGNO-MCP.md"
    guide = project_guide(guide_source.read_text(encoding="utf-8"))
    payloads["src/meerkat_toolkit_agno/PROFILE-GUIDE.md"] = guide.encode()
    composer_path = HERE / "compose_profile_guides.py"
    composer_spec = importlib.util.spec_from_file_location("agno_profile_guide_composer", composer_path)
    composer = importlib.util.module_from_spec(composer_spec)
    composer_spec.loader.exec_module(composer)
    selections_path = HERE / "profile-guide-selections.json"
    selections_bytes = selections_path.read_bytes()
    profiles, fields = composer.package_metadata(ROOT / "sdk/python/src/meerkat_toolkit/adapters")
    selected_guides = composer.compose(guide.encode(), json.loads(selections_bytes), profiles, fields)
    selected_bytes = (json.dumps(selected_guides, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()
    payloads["src/meerkat_toolkit_agno/CONFIGURATION-GUIDES.json"] = selected_bytes
    manifest: dict[str, object] = {
        "schema_version": 1,
        "distribution": "meerkat-toolkit-agno",
        "namespace": "meerkat_toolkit_agno",
        "source_mapping": sources,
        # Only the existing Salesforce consumer accepts native per-call
        # credentials. Other Agno profiles keep their ordinary serve mode.
        "native_connector": {
            "schema_version": 1,
            "module": "meerkat_toolkit_agno.agno_mcp",
            "mode": "serve-native",
            "configuration_argument": "--config",
            "profiles": ["salesforce"],
            "member": "meerkat_toolkit_agno/agno_mcp.py",
            "member_sha256": sources["src/meerkat_toolkit_agno/agno_mcp.py"]["sha256"],
        },
        "packaging_sha256": {
            name: hashlib.sha256(data).hexdigest()
            for name, data in payloads.items() if not name.startswith("src/")
        },
        "profile_guide": {
            "canonical_source": str(guide_source.relative_to(ROOT)),
            "source_sha256": hashlib.sha256(guide_source.read_bytes()).hexdigest(),
            "projection": "Profile/runtime sections with component namespace and consuming interpreter; original native-host and verification sections omitted",
            "sha256": hashlib.sha256(guide.encode()).hexdigest(),
        },
        "configuration_guides": {
            "generator": str(composer_path.relative_to(ROOT)),
            "generator_sha256": hashlib.sha256(composer_path.read_bytes()).hexdigest(),
            "selections": str(selections_path.relative_to(ROOT)),
            "selections_sha256": hashlib.sha256(selections_bytes).hexdigest(),
            "full_guide_sha256": hashlib.sha256(guide.encode()).hexdigest(),
            "sha256": hashlib.sha256(selected_bytes).hexdigest(),
            "scope": "Build-time exact profile projection; installed validator remains authoritative",
        },
        "scope": "Exact shared adapter sources only. No PairedHost-derived modules or parent distribution.",
    }
    payloads["src/meerkat_toolkit_agno/PACKAGE-SOURCES.json"] = (
        json.dumps(manifest, indent=2) + "\n"
    ).encode()
    # Existing directories, including symlinks, are refused rather than replaced.
    destination.mkdir()
    for name, data in payloads.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="new staging directory with an existing parent")
    args = parser.parse_args()
    stage(args.destination)
    print(str(args.destination.resolve()))


if __name__ == "__main__":
    main()
