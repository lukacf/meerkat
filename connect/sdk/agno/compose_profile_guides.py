"""Build selected Agno guidance from checked spans of its canonical manual.

This is a packaging operation, not a runtime relevance filter. Profile names,
fields and operations come from the package's existing metadata functions.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys
import uuid


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def compose(manual: bytes, selection: dict, profiles: list[dict], fields: dict) -> dict:
    original_digest = sha256(manual)
    variant_text = None
    if original_digest != selection["manual_sha256"]:
        variant = selection.get("manual_variants", {}).get(original_digest)
        if variant is not None:
            inserted = variant["text"].encode("utf-8")
            anchor = variant["insertion_before"].encode("utf-8")
            if (not inserted or sha256(inserted) != variant["sha256"]
                    or manual.count(inserted + anchor) != 1 or manual.count(inserted) != 1):
                raise ValueError("Canonical manual insertion changed")
            manual = manual.replace(inserted, b"", 1)
            variant_text = variant["text"].rstrip("\n")
    if selection["schema_version"] != 1 or sha256(manual) != selection["manual_sha256"]:
        raise ValueError("Canonical manual changed; review selected guidance spans")
    lines = manual.decode("utf-8").splitlines()
    names = [row["profile"] for row in profiles]
    if len(names) != len(set(names)) or set(names) != set(selection["profiles"]) or set(names) != set(fields):
        raise ValueError("Selected guidance must cover exactly the package profiles")
    sections = {"source_variant": variant_text} if variant_text is not None else {}
    for name, span in selection["spans"].items():
        start, end = span["start_line"], span["end_line"]
        if not 1 <= start <= end <= len(lines):
            raise ValueError("Invalid manual span")
        selected = lines[start - 1:end]
        if (selected[0] != span["first_line"] or selected[-1] != span["last_line"]
                or sha256("\n".join(selected).encode()) != span["sha256"]):
            raise ValueError("Canonical manual span changed")
        if "last_line_end" in span:
            prefix = span["last_line_end"]
            if not prefix or not selected[-1].startswith(prefix):
                raise ValueError("Canonical manual sentence boundary changed")
            selected[-1] = prefix
        sections[name] = "\n".join(selected)

    def table(name):
        definition = selection[name]
        start, end = definition["start_line"], definition["end_line"]
        if (not 3 <= start <= end <= len(lines)
                or lines[start - 3] != definition["header"]
                or lines[start - 2] != definition["separator"]):
            raise ValueError("Canonical manual table changed")
        rows = lines[start - 1:end]
        if any(not line.startswith("| ") or not line.endswith(" |") for line in rows):
            raise ValueError("Malformed canonical manual table")
        return definition, rows

    settings, setting_rows = table("settings_table")
    operations, operation_rows = table("operations_table")
    field_rows = {}
    for number, line in enumerate(setting_rows):
        keys = re.findall(r"`([^`]+)`", line.split("|", 2)[1])
        if not keys or any(key in field_rows for key in keys):
            raise ValueError("Missing or repeated setting row")
        for key in keys:
            field_rows[key] = number
    rendered = {}
    for row in profiles:
        profile = row["profile"]
        choice = selection["profiles"][profile]
        field_names = [field["name"] for field in fields[profile]]
        if len(field_names) != len(set(field_names)):
            raise ValueError("Duplicate package configuration field")
        field_spans = choice.get("field_spans", {})
        if set(field_spans) - set(field_names):
            raise ValueError("Stale selected field span")
        for field in field_names:
            if field not in field_rows and field not in field_spans:
                raise ValueError("Configuration field lacks selected guidance")
            if field in field_spans and field_spans[field] not in choice["spans"]:
                raise ValueError("Configuration field span is not included")
        indices = sorted({field_rows[name] for name in field_names if name in field_rows and name not in field_spans})
        settings_key = "settings:" + profile
        sections[settings_key] = "\n".join([
            "## Configuration settings", settings["header"], settings["separator"],
            *(setting_rows[index] for index in indices),
        ])
        selected_operations, described = [], []
        for line in operation_rows:
            cell = line.split("|", 2)[1].strip()
            if cell.split("`", 1)[0].strip() == choice["operation_label"]:
                selected_operations.append(line)
                described.extend(re.findall(r"`([^`]+)`", cell))
        if len(described) != len(set(described)) or set(described) != set(row["operations"]):
            raise ValueError("Operation guidance does not match the package metadata")
        operations_key = "operations:" + profile
        sections[operations_key] = "\n".join([
            "## Available operations and limits", operations["header"], operations["separator"],
            *selected_operations,
        ])
        chosen = [*selection["common_before"], *([settings_key] if indices else []), operations_key, *choice["spans"]]
        if "api_key_env" in field_names:
            chosen.append(selection["credential_value_span"])
        chosen.extend(choice.get("common_after", selection["common_after"]))
        if variant_text is not None:
            chosen.append("source_variant")
        # Shared spans are stored once; each exact profile selects explicit IDs.
        chosen = list(dict.fromkeys(chosen))
        text = "\n\n".join(sections[key] for key in chosen)
        if not text.strip() or len(text.encode()) > 32 * 1024:
            raise ValueError("Selected guidance exceeds its package bound")
        rendered[profile] = chosen
    return {"schema_version": 1, "full_guide_sha256": original_digest,
            "sections": sections, "profiles": rendered}


def package_metadata(package: Path):
    """Import only the inert package and its metadata, not build_profile."""
    name = "_toolkit_guide_metadata_" + uuid.uuid4().hex
    spec = importlib.util.spec_from_file_location(name, package / "__init__.py",
                                                submodule_search_locations=[str(package)])
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
        source = __import__(name + ".agno_profiles", fromlist=["list_profiles"])
        profiles = source.list_profiles()["profiles"]
        return profiles, {row["profile"]: source.configuration_fields(row["profile"]) for row in profiles}
    finally:
        for key in list(sys.modules):
            if key == name or key.startswith(name + "."):
                del sys.modules[key]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--selections", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    profiles, fields = package_metadata(args.package)
    value = compose((args.package / "PROFILE-GUIDE.md").read_bytes(),
                    json.loads(args.selections.read_bytes()), profiles, fields)
    with args.output.open("xb") as output:
        output.write((json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode())


if __name__ == "__main__":
    main()
