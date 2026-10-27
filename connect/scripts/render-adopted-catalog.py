#!/usr/bin/env python3
"""Project adopted OSS entries into inert toolkit-package discovery metadata.

This is a build-time projection of the maintainer's recorded choices. It neither
selects integrations for a request nor imports or executes integration code.
"""

from __future__ import annotations

import argparse
from datetime import datetime
import hashlib
import importlib.util
import io
import json
from pathlib import Path, PurePosixPath
import re
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "docs/adoption/oss-inventory.json"
OUTPUT = ROOT / "crates/toolkit-package/catalog/adopted.json"
_spec = importlib.util.spec_from_file_location("adoption_docs", ROOT / "scripts/render-adoption-docs.py")
_inventory = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_inventory)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()


def repository_file(root: Path, path: str, prefix: str) -> bytes:
    if not isinstance(path, str):
        raise ValueError("expected a repository-relative path")
    relative = PurePosixPath(path)
    if (str(relative) != path or relative.is_absolute()
            or ".." in relative.parts or not path.startswith(prefix)
            or len(path) > 192 or not re.fullmatch(r"[A-Za-z0-9_./-]+", path)):
        raise ValueError("expected a bounded repository-relative " + prefix + " path")
    resolved = (root / path).resolve(strict=True)
    if not resolved.is_relative_to(root.resolve()):
        raise ValueError("evidence escapes the repository")
    return resolved.read_bytes()


def evidence_file(root: Path, path: str) -> bytes:
    return repository_file(root, path, "docs/")


def selected_operations(operations: object) -> None:
    if (not isinstance(operations, list) or not operations
            or any(not isinstance(op, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+", op) for op in operations)
            or len(operations) != len(set(operations))):
        raise ValueError("adopted entry needs unique selected operation names")


def assessment_timestamp(value: str) -> int:
    observed = datetime.fromisoformat(value)
    if observed.tzinfo is None or observed.timestamp() < 0:
        raise ValueError("inventory assessment time must include a timezone")
    return int(observed.timestamp() * 1000)


def validate_services_component_receipt(receipt: dict, alternative: dict,
                                        indexed_paths: dict, root: Path) -> None:
    """Bind retained component evidence without inventing an old receipt.

    Each supported receipt binds its original component acceptance and actual
    installed command records. It makes no new target-runtime claim.
    """
    bindings = receipt.get("evidence")
    expected = {
        "component_acceptance": alternative["installation_acceptance"],
        "source_freeze": alternative["provenance"]["source_document"],
        "profile_command": alternative["inspection_commands"],
        "profile_output": alternative["profile_descriptor"],
        "package_review": alternative["review"],
        "guide": alternative["configuration_document"],
    }
    current = alternative["receipt_kind"] == "configured-services-consumer-v3"
    if (receipt.get("schema_version") != 1
            or receipt.get("receipt_kind") != alternative["receipt_kind"]
            or not isinstance(bindings, dict)
            or set(bindings) != set(expected) | {"installed_checks"}):
        raise ValueError("unsupported services component evidence binding")
    for name, path in expected.items():
        if bindings[name] != {"path": path, "sha256": indexed_paths[path]}:
            if name == "profile_output":
                raise ValueError("alternative profile descriptor differs from the accepted command output")
            raise ValueError("services component evidence binding differs: " + name)
    component = json.loads(evidence_file(root, expected["component_acceptance"]))
    if not isinstance(component, dict):
        raise ValueError("unsupported services component acceptance")
    if component.get("wheel_sha256") != alternative["artifact_sha256"]:
        raise ValueError("alternative component differs from its accepted installation")
    if current:
        if component.get("status") != "passed":
            raise ValueError("services component installation did not succeed")
    else:
        bundle = component.get("bundle_sha256")
        if (not isinstance(bundle, dict)
                or component.get("source_freeze_sha256") != receipt["source_freeze_sha256"]
                or bundle.get(PurePosixPath(alternative["artifact_path"]).name) != alternative["artifact_sha256"]
                or bundle.get("README.md") != indexed_paths[expected["guide"]]):
            raise ValueError("alternative component differs from its accepted installation")
    command = json.loads(evidence_file(root, expected["profile_command"]))
    if (not isinstance(command, dict)
            or type(command.get("exit_code")) is not int or command["exit_code"] != 0
            or command.get("stdout_sha256") != indexed_paths[expected["profile_output"]]
            or not isinstance(command.get("argv"), list) or command["argv"][-1:] != ["profiles"]):
        raise ValueError("alternative profile descriptor differs from the accepted command output")
    if current and command.get("wheel_sha256") != alternative["artifact_sha256"]:
        raise ValueError("services profile command differs from accepted component")
    review = json.loads(evidence_file(root, expected["package_review"]))
    reviewed = (review.get("final_wheel_sha256") if current else
                review.get("package_review", {}).get("wheel_sha256")) if isinstance(review, dict) else None
    if (reviewed != alternative["artifact_sha256"]
            or (current and review.get("status") != "passed")):
        raise ValueError("services package review differs from accepted component")
    checks = bindings["installed_checks"]
    required = ({"installed-scope.json", "pip-check.json"} if current else
                {"installed-mcp-tests.json", "installed-structured-tests.json",
                 "installed-guidance-tests.json", "pip-check.json"})
    if (not isinstance(checks, list) or len(checks) != len(required)
            or any(not isinstance(item, dict) or set(item) != {"path", "sha256"}
                   or not isinstance(item["path"], str) for item in checks)
            or {PurePosixPath(item["path"]).name for item in checks} != required):
        raise ValueError("services component requires its installed verification records")
    for item in checks:
        if indexed_paths.get(item["path"]) != item["sha256"]:
            raise ValueError("services installed verification record is not indexed")
        check = json.loads(evidence_file(root, item["path"]))
        if not isinstance(check, dict) or type(check.get("exit_code")) is not int or check["exit_code"] != 0:
            raise ValueError("services installed verification did not succeed")


def append_service_alternatives(package: dict, row: dict, evidence: dict, root: Path) -> None:
    """Project the supported configured-services consumer receipt shape.

    The package model deliberately has informational export hints rather than a
    runtime selection API. Route ownership therefore stays explicit in each
    strategy, export description and assessment; no shared compatibility claim
    is inferred from the primary implementation's provenance.

    This is a receipt adapter for the accepted first-party services component,
    not a universal alternate-provider proof format. Other receipt shapes need
    their own explicit maintainer review before this projection can accept them.
    """
    alternatives = row.get("alternative_implementations", [])
    if not isinstance(alternatives, list):
        raise ValueError("alternative implementations must be a list")
    if not alternatives:
        return
    primary = row["implementation"]
    primary_id = package["strategies"][0]["id"]
    package["provenance"]["notices"].append(
        "This provenance describes primary strategy " + primary_id
        + " only. Alternative strategies retain their own source, artifact and support evidence.")
    package["strategies"][0]["instructions"] = (
        "Strategy " + primary_id + " is the primary OSS adoption route. Its source, exports and evidence apply only to this route.\n"
        + package["strategies"][0]["instructions"])
    package["assessments"][0]["scope"] = "Strategy " + primary_id + " only. " + package["assessments"][0]["scope"]
    for export in package["known_exports"]:
        export["description"] = ("Strategy " + primary_id + " only; effect " + primary["effect"]
            + ". " + export["description"])
    strategy_ids = {primary_id}
    export_ids = {item["id"] for item in package["known_exports"]}
    fields = {"strategy_id", "receipt_kind", "assessed_at", "role", "profile", "selected_operations", "operation_effects", "artifact",
              "artifact_path", "artifact_sha256", "configuration_document", "acceptance", "review", "support",
              "native_host_scope", "limits", "prerequisites", "unknowns", "evidence_refs", "provenance",
              "profile_descriptor", "installation_acceptance", "inspection_commands"}
    for alternative in alternatives:
        if not isinstance(alternative, dict) or set(alternative) != fields:
            raise ValueError("unexpected alternative implementation fields")
        if alternative["receipt_kind"] not in ("configured-services-consumer-v1", "configured-services-consumer-v2", "configured-services-consumer-v3"):
            raise ValueError("unsupported alternative receipt kind")
        timestamp = assessment_timestamp(alternative["assessed_at"])
        strategy_id = alternative["strategy_id"]
        if not isinstance(strategy_id, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+", strategy_id):
            raise ValueError("invalid alternative strategy identity")
        if strategy_id in strategy_ids:
            raise ValueError("duplicate strategy identity")
        strategy_ids.add(strategy_id)
        if alternative["role"] != "service_connector":
            raise ValueError("alternative service evidence cannot establish frontend compatibility")
        operations = alternative["selected_operations"]
        selected_operations(operations)
        if export_ids.intersection(operations):
            raise ValueError("ambiguous export ownership across strategies")
        export_ids.update(operations)
        effects = alternative["operation_effects"]
        if (not isinstance(effects, dict) or set(effects) != set(operations)
                or any(effect not in ("read", "write") for effect in effects.values())):
            raise ValueError("each alternative operation needs its own read/write effect")
        if any(not isinstance(alternative[key], str) or not alternative[key].strip()
               for key in ("profile", "artifact", "native_host_scope", "limits")):
            raise ValueError("alternative setup and scope details must be explicit")
        for key in ("prerequisites", "unknowns", "evidence_refs"):
            values = alternative[key]
            if (not isinstance(values, list) or not values
                    or any(not isinstance(value, str) or not value.strip() for value in values)
                    or len(values) != len(set(values))):
                raise ValueError("alternative " + key + " must be a nonempty unique list")

        indexed_paths, indexed_artifacts = {}, {}
        for reference in alternative["evidence_refs"]:
            item = evidence[reference]
            if "path" in item and "source_snapshot" not in item:
                path = item["path"]
                actual = digest(evidence_file(root, path))
                if actual != item["sha256"]:
                    raise ValueError("evidence digest mismatch: " + reference)
                indexed_paths[path] = actual
            elif item.get("kind") == "accepted-local-artifact" and "artifact_path" in item:
                path = item["artifact_path"]
                actual = digest(repository_file(root, path, "dist/"))
                if actual != item["sha256"]:
                    raise ValueError("artifact digest mismatch: " + reference)
                indexed_artifacts[path] = actual
            else:
                raise ValueError("alternative evidence must identify reviewed local documents or artifacts")
        provenance = alternative["provenance"]
        if (not isinstance(provenance, dict)
                or set(provenance) != {"owner", "source_document", "source_revision", "license", "notices"}
                or any(not isinstance(value, str) or not value.strip() for value in provenance.values())):
            raise ValueError("alternative provenance must identify its own source and notices")
        paths = list(dict.fromkeys(alternative[key] for key in
            ("acceptance", "review", "support", "configuration_document", "profile_descriptor",
             "installation_acceptance", "inspection_commands"))) + [provenance["source_document"]]
        if any(path not in indexed_paths for path in paths):
            raise ValueError("every alternative evidence path needs an indexed reviewed digest")
        artifact_digest = alternative["artifact_sha256"]
        if (not isinstance(artifact_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", artifact_digest)
                or indexed_artifacts.get(alternative["artifact_path"]) != artifact_digest):
            raise ValueError("alternative artifact needs its exact indexed digest")
        receipt = json.loads(evidence_file(root, alternative["acceptance"]))
        if not isinstance(receipt, dict):
            raise ValueError("unsupported alternative consumer receipt")
        status = ("accepted_configured_standalone_local_consumer"
                  if alternative["receipt_kind"] == "configured-services-consumer-v1"
                  else "retained_accepted_component_evidence")
        if (receipt.get("status") != status
                or receipt.get("wheel_sha256") != artifact_digest):
            raise ValueError("alternative artifact differs from its accepted consumer receipt")
        source_digest = indexed_paths[provenance["source_document"]]
        if (provenance["source_revision"] != "sha256:" + source_digest
                or receipt.get("source_freeze_sha256") != source_digest):
            raise ValueError("alternative source differs from its accepted consumer receipt")
        source_bytes = evidence_file(root, provenance["source_document"])
        source_freeze = json.loads(source_bytes)
        if alternative["receipt_kind"] == "configured-services-consumer-v3":
            # This receipt's original result does not carry a source-map hash.
            # Bind the retained JSON map to the exact accepted wheel. The
            # original verification record reserializes this member as JSON.
            artifact = repository_file(root, alternative["artifact_path"], "dist/")
            try:
                with zipfile.ZipFile(io.BytesIO(artifact)) as wheel:
                    members = [item for item in wheel.infolist()
                               if item.filename == "meerkat_toolkit_services/PACKAGE-SOURCES.json"]
                    if (len(members) != 1 or members[0].is_dir()
                            or not 0 < members[0].file_size <= 1024 * 1024
                            or members[0].flag_bits & 1
                            or (members[0].external_attr >> 16) & 0o170000 not in (0, 0o100000)
                            or json.loads(wheel.read(members[0])) != source_freeze):
                        raise ValueError("alternative source map differs from accepted wheel")
            except (zipfile.BadZipFile, RuntimeError) as error:
                raise ValueError("invalid services source-map archive") from error
        guide_digest = (source_freeze.get("other_payload_sha256", {}).get("README.md")
                        if alternative["receipt_kind"] == "configured-services-consumer-v3"
                        else source_freeze.get("sdk/services/README.md"))
        if guide_digest != indexed_paths[alternative["configuration_document"]]:
            raise ValueError("alternative guide differs from its accepted source freeze")
        if alternative["receipt_kind"] in ("configured-services-consumer-v2", "configured-services-consumer-v3"):
            validate_services_component_receipt(receipt, alternative, indexed_paths, root)
        else:
            installation = json.loads(evidence_file(root, alternative["installation_acceptance"]))
            if (receipt.get("final_installation_sha256") != indexed_paths[alternative["installation_acceptance"]]
                    or installation.get("wheel_sha256") != artifact_digest
                    or installation.get("commands_sha256") != indexed_paths[alternative["inspection_commands"]]):
                raise ValueError("alternative profile inspection differs from its accepted installation")
            commands = json.loads(evidence_file(root, alternative["inspection_commands"]))
            profile_commands = [command for command in commands if command.get("name") == "console-profiles"]
            if (len(profile_commands) != 1 or profile_commands[0].get("exit_code") != 0
                    or profile_commands[0].get("stdout_sha256") != indexed_paths[alternative["profile_descriptor"]]):
                raise ValueError("alternative profile descriptor differs from the accepted command output")
        descriptor = json.loads(evidence_file(root, alternative["profile_descriptor"]))
        profiles = [profile for profile in descriptor["profiles"] if profile["profile"] == alternative["profile"]]
        if len(profiles) != 1 or alternative["profile"] != row["id"]:
            raise ValueError("alternative profile must describe this service identity")
        accepted_effects = {operation["tool"]: operation["effect"] for operation in profiles[0]["operations"]}
        if any(accepted_effects.get(operation) != effects[operation] for operation in operations):
            raise ValueError("alternative export/effect differs from its accepted profile descriptor")

        instructions = [
            "Strategy " + strategy_id + " is an alternative first-party route for this same service identity; it does not replace or expand the primary OSS adoption claim.",
            "Use " + alternative["artifact"] + " at repository-relative " + alternative["artifact_path"]
                + " (SHA-256 " + artifact_digest + ").",
            "Provenance for this strategy only: " + provenance["owner"] + "; source record "
                + provenance["source_document"] + " at " + provenance["source_revision"]
                + "; license " + provenance["license"] + ". " + provenance["notices"],
            "Follow the repository-relative configuration document " + alternative["configuration_document"]
                + " at its evidence digest. Preserve its dependency constraints, notices and host requirements.",
            "Select profile " + alternative["profile"] + "; configure an explicit nonempty operation subset using the guide and the component's profiles/config-help/check-config commands.",
            "Exports for this strategy only: " + ", ".join(op + " (" + effects[op] + ")" for op in operations) + ".",
        ] + alternative["prerequisites"] + [
            "Obtain account/configuration inputs through the host. Read actual MCP tool names and schemas through tools/list after connection. These hints do not establish runtime bindings, authorization or readiness."
        ]
        package["strategies"].append({
            "id": strategy_id, "kind": "local_mcp", "instructions": "\n".join(instructions),
            "sources": [], "known_requirements": [],
            "unresolved_details": alternative["unknowns"] + [alternative["limits"]],
        })
        package["known_exports"].extend({
            "id": op, "role": alternative["role"],
            "description": "Strategy " + strategy_id + " only; effect " + effects[op]
                + ". Selected operation " + op + "; account, readiness and authorization remain host-owned.",
        } for op in operations)
        package["assessments"].append({
            "status": "compatible", "scope": "Strategy " + strategy_id + " only. " + alternative["native_host_scope"]
                + " This metadata assessment does not install, authenticate, authorize or activate anything.",
            "source_revision": provenance["source_revision"], "assessed_at_unix_ms": timestamp,
            "evidence": [{"owner": "meerkat-toolkit/library-maintainers", "record": path,
                          "revision": "sha256:" + indexed_paths[path]} for path in paths],
        })


def project(inventory: dict, root: Path = ROOT) -> list[dict]:
    _inventory.validate(inventory)
    timestamp = assessment_timestamp(inventory["generated_at"])
    snapshots = {item["id"]: item for item in inventory["source_snapshots"]}
    evidence = {item["id"]: item for item in inventory["evidence"]}
    if len(evidence) != len(inventory["evidence"]):
        raise ValueError("duplicate evidence identity")
    packages = []
    for row in sorted(inventory["integrations"], key=lambda item: item["id"]):
        if row["status"] != "adopted":
            continue
        if row["role"] != "service_connector":
            raise ValueError("service adoption cannot establish frontend compatibility")
        implementation = row["implementation"]
        operations = implementation["selected_operations"]
        selected_operations(operations)
        if row["license_review"]["status"] != "selected-artifact-notices-reviewed":
            raise ValueError("adopted entry lacks its selected-artifact license review")
        # Verify current local evidence referenced by this adopted record. Old
        # source observations are retained but do not establish installed support.
        indexed_paths = {}
        for reference in row["evidence_refs"]:
            item = evidence[reference]
            if "path" in item and "source_snapshot" not in item:
                actual = digest(evidence_file(root, item["path"]))
                if actual != item["sha256"]:
                    raise ValueError(f"evidence digest mismatch: {reference}")
                indexed_paths[item["path"]] = actual
        required = [implementation.get("adoption_acceptance", implementation["acceptance"]),
                    implementation["review"], implementation["support"]]
        if any(path not in indexed_paths for path in required):
            raise ValueError("adoption acceptance, review and support need indexed evidence")
        paths = list(dict.fromkeys(required + [implementation["configuration_document"], implementation["acceptance"]] + [
            implementation[key] for key in ("adoption_acceptance", "recovery_review", "reproduction", "live_public_read")
            if key in implementation
        ]))
        if any(path not in indexed_paths for path in paths):
            raise ValueError("every emitted evidence path needs a recorded reviewed digest")
        references = [{
            "owner": "meerkat-toolkit/library-maintainers",
            "record": path,
            "revision": "sha256:" + indexed_paths[path],
        } for path in paths]
        source = row["sources"][0]
        snapshot = snapshots[source["source_snapshot"]]
        artifact = implementation["artifact"]
        if "artifact_sha256" in implementation:
            if not re.fullmatch(r"[0-9a-f]{64}", implementation["artifact_sha256"]):
                raise ValueError("invalid selected artifact digest")
            receipt = json.loads(evidence_file(root, required[0]))
            bindings = [receipt[key] for key in ("wheel_sha256", "archive_sha256", "binary_sha256")
                        if key in receipt]
            if len(bindings) != 1 or bindings[0] != implementation["artifact_sha256"]:
                raise ValueError("selected artifact differs from the verified adoption receipt")
            artifact += " (SHA-256 " + implementation["artifact_sha256"] + ")"
        guidance = [
            "Use the selected local MCP implementation: " + artifact + ".",
            "Follow the repository-relative configuration document " + implementation["configuration_document"]
            + " at its evidence digest. Preserve its dependency lock, notices and host requirements.",
        ]
        if "profile" in implementation:
            guidance.append("Select profile " + implementation["profile"] + ".")
        guidance += ["Selected logical operations: " + ", ".join(operations) + "."]
        guidance += row["prerequisites"]
        guidance.append("Obtain account/configuration inputs through the host. Read actual MCP tool names and schemas through tools/list after connection; these logical operation IDs do not establish runtime bindings. Verify current readiness before use.")
        unresolved = list(row["unknowns"])
        if "limits" in implementation:
            unresolved.append(implementation["limits"])
        if not unresolved:
            raise ValueError("adopted local scope must retain its readiness limits")
        package = {
            "schema_version": 1,
            "identity": {"publisher": "meerkat-toolkit", "name": row["id"]},
            "display_name": row["name"],
            "description": row["status_scope"],
            "provenance": {
                "source": source["repository"], "source_revision": source["revision"],
                "license": snapshot["license_observation"]["declared_or_observed_license"],
                "notices": [row["license_review"]["note"],
                    "Toolkit adaptation and source-port details are in the configuration and review evidence; upstream revision alone does not identify the installed artifact."],
                "local_patches": [],
            },
            "strategies": [{
                "id": "selected-local-mcp", "kind": "local_mcp",
                "instructions": "\n".join(guidance),
                "sources": list(dict.fromkeys(item["url"] for item in row["sources"])),
                "known_requirements": [], "unresolved_details": unresolved,
            }],
            "known_exports": [{"id": op, "role": "service_connector",
                "description": "Selected logical profile operation " + op + "; readiness and access remain host-owned."}
                for op in operations],
            "assessments": [{
                "status": "compatible",
                "scope": "Retained installed local MCP compatibility only. " + implementation["native_host_scope"]
                    + " This metadata assessment does not install, authenticate, authorize or activate anything.",
                "source_revision": source["revision"], "assessed_at_unix_ms": timestamp,
                "evidence": references,
            }],
        }
        append_service_alternatives(package, row, evidence, root)
        # Content-derived metadata revisions cannot silently overwrite an older
        # description. This is distinct from both upstream and artifact versions.
        package["identity"]["version"] = "metadata-sha256:" + digest(canonical(package))
        packages.append(package)
    if len(packages) != inventory["counts"]["adopted"]:
        raise ValueError("projection does not match adopted inventory count")
    return packages


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        packages = project(json.loads(INVENTORY.read_bytes()))
        output = json.dumps(packages, ensure_ascii=True, indent=2) + "\n"
        if args.check:
            if not OUTPUT.is_file() or OUTPUT.read_text() != output:
                raise ValueError("stale catalog; run python3 scripts/render-adopted-catalog.py")
        else:
            OUTPUT.write_text(output)
        print(f"adopted-catalog: {'checked' if args.check else 'wrote'} {len(packages)} entries")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"adopted-catalog: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
