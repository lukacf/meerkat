"""An explicit host-selected candidate target, separate from a user's design.

The caller must already accept the producer's cryptographic provenance and the
exact locally built SDK tuple. This module checks those selected artifact bytes;
it neither performs that acceptance nor turns a digest into release authority.
The ordinary public release remains the compiler's default.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass
from pathlib import Path
import re

from .runtime_candidates import (ArtifactFile, CandidateBinding, MetaToolkitRuntimeTarget,
    PRODUCT_TARGETS, read_candidate_artifacts, recheck_candidate_artifacts,
    read_meta_toolkit_artifacts, recheck_meta_toolkit_artifacts)
from .sdk_wheel_integrity import WheelByteExpectation


SELECTION_FORMAT = "forge-runtime-candidate-selection-v1"
PRODUCT_SELECTION_FORMAT = "forge-meta-toolkit-runtime-selection-v1"


@dataclass(frozen=True)
class RuntimeCandidateTarget:
    tuple_path: Path
    artifact_root: Path
    accepted: CandidateBinding


def parse_candidate_selection(value):
    """Parse a previously protected selection without admitting its artifacts.

    This is a pure historical identity check. A caller must obtain the document
    from its host-owned framework or checked update journal. Parsing a browser
    value, package claim or receipt alone does not establish that authority.
    Current admission additionally calls candidate_selection to recheck bytes.
    """
    if type(value) is dict and value.get("format") == PRODUCT_SELECTION_FORMAT:
        return _parse_product_selection(value)
    if (type(value) is not dict or set(value) != {"format", "tuple_path", "artifact_root", "accepted"}
            or value.get("format") != SELECTION_FORMAT or type(value.get("accepted")) is not dict
            or set(value["accepted"]) != set(CandidateBinding.__dataclass_fields__)):
        raise ValueError("The protected candidate selection has an invalid shape.")
    binding = value["accepted"]
    if (binding["repository"] != "lukacf/meerkat-mobkit"
            or binding["native_version"] != "0.8.48" or binding["core_version"] != "0.8.51"
            or not isinstance(binding["platform"], str)
            or binding["platform"] not in {"aarch64-apple-darwin", "x86_64-apple-darwin",
                "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"}
            or any(type(binding[key]) is not int or binding[key] < 1
                for key in ("native_run_id", "native_run_attempt"))
            or any(not isinstance(binding[key], str) or not re.fullmatch(pattern, binding[key])
                for key, pattern in (("source_sha", "[0-9a-f]{40}"),
                    ("tuple_sha256", "[0-9a-f]{64}"),
                    ("native_manifest_sha256", "[0-9a-f]{64}"),
                    ("sdk_wheel_sha256", "[0-9a-f]{64}")))):
        raise ValueError("The protected candidate identity is invalid.")
    paths = []
    for name in ("tuple_path", "artifact_root"):
        item = value[name]
        if not isinstance(item, str) or not item or len(item) > 4096 or "\x00" in item:
            raise ValueError("The protected candidate locator is invalid.")
        path = Path(item)
        if not path.is_absolute() or str(path) != item or ".." in path.parts:
            raise ValueError("The protected candidate locator must be canonical and absolute.")
        paths.append(path)
    return RuntimeCandidateTarget(paths[0], paths[1], CandidateBinding(**binding))


def implementation_candidate_selection(target, *, deployment_target=None, verified_artifacts=None):
    """Validate physical product bytes for the original trusted Build owner.

    GCP is an application destination, not a native artifact platform. Only
    this source-construction branch may join its unchanged cloud plan to the
    explicitly selected Linux x86_64 product. It confers no remote Install,
    native preparation or cloud admission, and never accepts a saved path.
    """
    if deployment_target == "gcp":
        if type(target) is not MetaToolkitRuntimeTarget or target.target != "linux-x86_64":
            raise ValueError("GCP product source requires its trusted Linux x86_64 native product.")
        return candidate_selection(target, deployment_target="linux",
            verified_artifacts=verified_artifacts)
    return candidate_selection(target, deployment_target=deployment_target,
        verified_artifacts=verified_artifacts)


def candidate_selection(target, *, deployment_target=None, verified_artifacts=None,
        source_custody=None, destination=None, files=None):
    """Render a small protected binding after checking the selected bytes.

    Application source never contains the native archives or SDK wheel. The
    external bundle must remain available on this candidate deployment host.
    Returning this document is compilation, not update or runtime admission.
    """
    if type(target) is MetaToolkitRuntimeTarget:
        fields = asdict(target)
        if target.source_manifest is None:
            fields.pop("source_manifest")
        value = {"format": PRODUCT_SELECTION_FORMAT, **_json_paths(fields)}
        _parse_product_selection(value)
        artifacts = selected_runtime_artifacts(target, verified_artifacts=verified_artifacts,
            source_custody=source_custody, destination=destination, files=files)
        physical_target = deployment_target
        if deployment_target == "gcp" and source_custody is not None:
            from .product_runtime_custody import gcp_product_deployment_target
            physical_target = gcp_product_deployment_target(source_custody, target,
                destination=destination, files=files)
        if physical_target is not None and physical_target != (
                "mac" if artifacts.candidate["target"] == "macos-arm64" else "linux"):
            raise ValueError("The selected native product does not match this deployment platform.")
        return value
    if source_custody is not None:
        raise ValueError("Relocated byte custody is only available to its original native product owner.")
    if not isinstance(target, RuntimeCandidateTarget):
        raise ValueError("A candidate target needs an explicit host-owned selection.")
    binding = target.accepted
    if (not isinstance(binding, CandidateBinding) or binding.native_version != "0.8.48"
            or binding.core_version != "0.8.51"):
        raise ValueError("The selected candidate must be the qualified .48/Core .51 tuple.")
    if deployment_target is not None:
        supported = {"mac": "apple-darwin", "linux": "unknown-linux-gnu"}
        if (deployment_target not in supported
                or not binding.platform.endswith("-" + supported[deployment_target])):
            raise ValueError("The selected candidate does not match this local deployment platform.")
    value = {"format": SELECTION_FORMAT, "tuple_path": str(target.tuple_path),
        "artifact_root": str(target.artifact_root), "accepted": asdict(binding)}
    parse_candidate_selection(value)
    if verified_artifacts is None:
        read_candidate_artifacts(target.tuple_path, accepted=binding, artifact_root=target.artifact_root)
    else:
        recheck_candidate_artifacts(verified_artifacts, tuple_path=target.tuple_path,
            accepted=binding, artifact_root=target.artifact_root)
    return value


def _json_paths(value):
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, dict):
        return {key: _json_paths(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [_json_paths(item) for item in value]
    return value


def _product_path(value):
    if not isinstance(value, str) or not value or len(value) > 4096 or "\x00" in value:
        raise ValueError("The selected native product locator is invalid.")
    path = Path(value)
    if not path.is_absolute() or str(path) != value or ".." in path.parts:
        raise ValueError("The selected native product locator must be canonical and absolute.")
    return path


def _product_artifact(value):
    if (type(value) is not dict or set(value) != {"path", "size", "sha256"}
            or type(value["size"]) is not int or value["size"] < 1
            or not isinstance(value["sha256"], str) or not re.fullmatch("[a-f0-9]{64}", value["sha256"])):
        raise ValueError("The selected native product artifact binding is invalid.")
    return ArtifactFile(_product_path(value["path"]), value["size"], value["sha256"])


def _product_wheel(value):
    if type(value) is not dict or set(value) != set(WheelByteExpectation.__dataclass_fields__):
        raise ValueError("The selected SDK byte expectation is invalid.")
    fields = dict(value)
    if (not isinstance(fields["distribution"], str) or not re.fullmatch("[a-z][a-z-]+", fields["distribution"])
            or not isinstance(fields["version"], str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", fields["version"])
            or type(fields["size"]) is not int or fields["size"] < 1
            or not isinstance(fields["sha256"], str) or not re.fullmatch("[a-f0-9]{64}", fields["sha256"])):
        raise ValueError("The selected SDK identity is invalid.")
    for key in ("source_payload", "metadata"):
        rows = fields[key]
        if not isinstance(rows, (list, tuple)) or not rows or len(rows) > 4096:
            raise ValueError("The selected SDK inventory is invalid.")
        if any(not isinstance(row, (list, tuple)) or len(row) != 3
                or not isinstance(row[0], str) or not row[0] or len(row[0]) > 4096
                or not isinstance(row[1], str) or not re.fullmatch("[a-f0-9]{64}", row[1])
                or type(row[2]) is not int or row[2] < 0 for row in rows):
            raise ValueError("The selected SDK inventory entry is invalid.")
        fields[key] = tuple(tuple(row) for row in rows)
    return WheelByteExpectation(**fields)


def _parse_product_selection(value):
    required = {"format", *MetaToolkitRuntimeTarget.__dataclass_fields__} - {"source_manifest"}
    if set(value) not in (required, required | {"source_manifest"}):
        raise ValueError("The protected native product selection has an invalid shape.")
    if not isinstance(value["target"], str) or value["target"] not in PRODUCT_TARGETS:
        raise ValueError("The selected native product target is invalid.")
    fields = {"artifact_root": _product_path(value["artifact_root"]), "target": value["target"]}
    for name in ("assembly", "meta_wheel", "toolkit_wheel", "dependency_requirements", "model_catalog"):
        fields[name] = _product_artifact(value[name])
    if "source_manifest" in value:
        fields["source_manifest"] = _product_artifact(value["source_manifest"])
    for name in ("meta", "toolkit"):
        fields[name] = _product_wheel(value[name])
    dependencies = value["dependency_wheels"]
    if not isinstance(dependencies, (list, tuple)) or not 0 < len(dependencies) <= 256:
        raise ValueError("The selected native product dependency batch is invalid.")
    fields["dependency_wheels"] = tuple(_product_artifact(item) for item in dependencies)
    return MetaToolkitRuntimeTarget(**fields)


def selected_runtime_artifacts(target, *, verified_artifacts=None, source_custody=None,
        destination=None, files=None, retained=None):
    from . import runtime_candidates
    if retained is not None and (type(target) is not MetaToolkitRuntimeTarget
            or source_custody is not None or verified_artifacts is not None):
        # A carrier holds the reads it made with its own retained inventory.
        raise ValueError("A retained native inventory only replaces one fresh local product read.")
    if type(target) is MetaToolkitRuntimeTarget:
        if source_custody is not None:
            from .product_runtime_custody import relocated_artifacts
            return relocated_artifacts(target, source_custody=source_custody,
                verified_artifacts=verified_artifacts, destination=destination, files=files)
        return (read_meta_toolkit_artifacts(target, retained=retained) if verified_artifacts is None else
            recheck_meta_toolkit_artifacts(verified_artifacts, target))
    if source_custody is not None:
        raise ValueError("Relocated byte custody is only available to its original native product owner.")
    if type(target) is not RuntimeCandidateTarget:
        raise ValueError("The runtime requires an explicit protected application selection.")
    return (runtime_candidates.read_candidate_artifacts(target.tuple_path, accepted=target.accepted,
        artifact_root=target.artifact_root) if verified_artifacts is None else
        runtime_candidates.recheck_candidate_artifacts(verified_artifacts, tuple_path=target.tuple_path,
            accepted=target.accepted, artifact_root=target.artifact_root))


def selected_runtime_version(target):
    return target.meta.version if type(target) is MetaToolkitRuntimeTarget else target.accepted.native_version


def selected_runtime_marker(target):
    if type(target) is MetaToolkitRuntimeTarget:
        return {"source": "meta-toolkit-product", "selection": "runtime-candidate.json",
            "assembly_sha256": target.assembly.sha256}
    return {"source": "signed-candidate", "selection": "runtime-candidate.json",
        "tuple_sha256": target.accepted.tuple_sha256}


def selected_runtime_platform(target, *, verified_artifacts=None):
    if type(target) is MetaToolkitRuntimeTarget:
        return PRODUCT_TARGETS[target.target]
    return target.accepted.platform
