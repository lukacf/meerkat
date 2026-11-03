"""Exact completed application policy inputs, without the Forge model/compiler.

Native retains typed policy validation, canonicalization, authority and Ready.
This maintained projection stages facts from one authenticated completed bundle.
"""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json


@dataclass(frozen=True)
class PolicyArtifact:
    """Small immutable staging artifact; the compiler preserves PackageFile."""

    path: str
    content: str | bytes
    mode: int = 0o644

    def as_bytes(self):
        return self.content if isinstance(self.content, bytes) else self.content.encode("utf-8")


def completed_application_policy_inputs(bundle, source_policy):
    """Complete policy input from the same live preparation's retained output.

    This is the trusted compiler's configuration join, not native policy
    compilation, publication or readiness. Its caller retains the stopped-host
    transaction and stages these new bytes alongside the exact source artifacts
    before NativeInstallerCaller.prepare_application_policy validates them.
    Toolkit still owns typed projection, canonicalization and authority.
    """
    return _completed_application_policy_inputs(bundle, source_policy, owner_fences=True)


def _completed_application_policy_inputs_files(bundle, source_policy):
    """Project the same exact facts inside a joined filesystem worker."""
    return _completed_application_policy_inputs(bundle, source_policy, owner_fences=False)


def _completed_application_policy_inputs(bundle, source_policy, *, owner_fences):
    from .connect_installer_caller import (
        CompletedApplicationSetupBundle, CustodyError, ExactFile, _selected_standing_policy,
    )
    if (not isinstance(bundle, CompletedApplicationSetupBundle)
            or bundle.preparation_plan is None
            or not isinstance(source_policy, ExactFile)
            or source_policy not in bundle.source_files):
        raise CustodyError("Policy finalization requires the retained driver bundle and source policy.")
    # authenticate() checks the original plan, complete native output and source
    # closure under the supplied currentness owner. Never join model/browser
    # fields or pre-issuance members in place of these returned facts.
    authenticate = bundle.authenticate if owner_fences else bundle._authenticate_files
    setup_bytes, _ = authenticate()
    plan_bytes = bundle.preparation_plan.read()
    source_bytes = source_policy.read()
    from .connect_installer_caller import _json as read_json
    plan, setup = read_json(plan_bytes), read_json(setup_bytes)
    parameters = plan.get("acquisition")
    required = {"backend_generation", "proposal_ceiling", "output_audience", "limits"}
    if "helper" in setup:
        if (not isinstance(parameters, dict) or not required <= parameters.keys()
                or parameters.keys() - required - {"console_setup", "standing_policy"}):
            raise CustodyError("Finalization requires the original acquisition parameters unchanged.")
        _selected_standing_policy(parameters.get("standing_policy"), helper_tools=setup.get("helper_tools"))
        acquisition = {"schema": 1, **parameters}
    else:
        if "acquisition" in plan:
            raise CustodyError("Helperless finalization must omit acquisition parameters.")
        acquisition = {"schema": 2}
    acquisition_name = "native-setup.json"
    original_artifacts = (source_policy, bundle.setup, bundle.application_tool_policy,
        *(output for _, output in bundle.member_surfaces))
    # The native policy closure uses a canonical artifact name. Preserve the
    # original selected pin; its source filename is not an authority choice.
    artifact_names = ("policy.json", *(pin.path.name for pin in original_artifacts[1:]))
    if (len(set(artifact_names)) != len(original_artifacts)
            or acquisition_name in set(artifact_names)):
        raise CustodyError("Final policy artifacts require distinct selected names.")
    acquisition["prepared_application_setup"] = {
        "name": bundle.setup.path.name, "sha256": bundle.setup.sha256}
    acquisition_bytes = (_json(acquisition) + "\n").encode()
    owner = setup["native_owner"]
    native = owner["native_host"]
    policy = {"schema": 7, "deployment": owner["deployment"],
        "source_policy_sha256": source_policy.sha256,
        "components": owner["components"], "channels": owner["channels"],
        "native_host": {**{key: value for key, value in native.items() if key != "members"},
            "members": setup["native_members"],
            "application_tool_policy": {"name": bundle.application_tool_policy.path.name,
                "sha256": bundle.application_tool_policy.sha256},
            "setup": {"name": acquisition_name,
                "sha256": hashlib.sha256(acquisition_bytes).hexdigest()}}}
    policy_bytes = (_json(policy) + "\n").encode()
    # Recheck late source/owner changes before returning any staging input.
    if (authenticate()[0] != setup_bytes
            or bundle.preparation_plan.read() != plan_bytes
            or source_policy.read() != source_bytes):
        raise CustodyError("The selected finalization source changed during compilation.")
    if owner_fences:
        bundle._require_current()
    elif bundle._require_filesystem_current is not None:
        bundle._require_filesystem_current()
    return {"local_policy_input": policy_bytes,
        "acquisition": PolicyArtifact(acquisition_name, acquisition_bytes.decode()),
        "source_artifacts": original_artifacts,
        "source_artifact_names": artifact_names}



def _json(value):
    return json.dumps(value, ensure_ascii=True)
