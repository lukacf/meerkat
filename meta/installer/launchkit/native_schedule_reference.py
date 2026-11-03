"""Portable primary-source scheduling documentation, not a scheduler client."""
from __future__ import annotations

import hashlib
from importlib import resources
import json


REFERENCE_VERSION = "0.8.42"
REFERENCE_REVISION = 4
MAX_REFERENCE_BYTES = 64 * 1024
REFERENCE_KIND = "static_pinned_primary_source"

# General application lifecycle guidance; schemas and authority stay native.
RECURRING_APPLICATION_GUIDANCE = "Agent-owned recurring application work\n\nUse an existing non-read-only responsible member with the schedule tool grant. Keep the user's continuing job, original request, owned context, input/configuration revisions and explicit cadence/timezone/recovery choices. This pattern does not require another agent, a fixed roster or a host scheduler adapter.\n\nThe native current_session prompt can read current application input through that member's scoped callbacks, perform the requested work, and publish its useful domain result through a scoped callback. Register those callbacks in the member's initial catalogue. The callback must check current input/configuration revision and enabled state, retain immutable result keys for unknown-response recovery, and preserve earlier results. Do not replace the user's question with a generic checklist.\n\nSerialize schedule initialization for the continuing job. Inventory actual native schedules and read back any retained native ID before creating. Compare job/context labels, resolved owner, trigger, prompt, policies and phase/revision. Labels are not an ownership ACL. An unknown create response requires observation/recovery, never an automatic second create. current_session is resolved by the host; compare its returned stable ownership rather than literal input target equality.\n\nDisable the application publication gate immediately, then pause and read back the exact native record. Pause does not cancel in-flight work. Use flat update arguments and verify native revision/configuration before resuming. Keep startup catalogue registration before native restore delivery. After restart, inventory/read back the same schedule and verify restored delivery plus the saved domain result.\n\nNative schedule phase and occurrence delivery/work_ref are separate from useful application completion. Save native tool observations separately from domain callback output. Agent-supplied schedule/occurrence fields are claims, not verified receipts. If no authentic occurrence identifier reaches the application callback, leave that join unknown. A callback result may precede native conversational completion; report both honestly. A stopped local host cannot deliver the weekly job."


def selected_recurrence_guidance():
    """Useful lifecycle guidance without borrowing another native tool schema."""
    return ("Recurring application work on the selected native runtime\n\n"
        "Use an existing responsible member and its genuinely granted native scheduling tools. "
        "Retain the user's continuing job, context, current inputs, cadence, timezone, "
        "missed-run and overlap choices, enabled state, provider cost and exact action authorization. "
        "Register scoped application callbacks and any named host runnables before native restore delivery. "
        "Use only targets and arguments advertised by that runtime's native tools; a callback name "
        "does not create a schedule or grant its authoring.\n\n"
        "Serialize initialization, inspect actual native schedules and read back the exact owned ID "
        "before creating. Compare resolved ownership and current native configuration. Labels are "
        "not an ownership ACL. Preserve unknown create outcomes for observation instead of creating "
        "another recurrence. Disable application publication immediately, then use the native pause "
        "and readback contract; pause does not cancel in-flight work.\n\n"
        "Keep native schedule/occurrence observations separate from useful immutable domain results. "
        "Agent-supplied records are claims unless independently authenticated. Leave unavailable "
        "occurrence joins unknown. After restart, observe the same persisted native schedule and "
        "actual delivery plus saved useful work; callback binding and application catch-up are not "
        "native occurrence proof. A stopped local host cannot deliver the recurring job.\n\n"
        "Native authoring schema: this selected SDK source reference does not supply a native "
        "schedule-tool schema. Inspect the actual granted tools on the selected native runtime. "
        "No legacy schema, native grant, installed schedule or qualified delivery is inferred.")


def load_schedule_reference():
    """Read the bundled contract without a Rust checkout, network or gateway."""
    raw = resources.files(__package__).joinpath("native_schedule_reference.json").read_bytes()
    if len(raw) > MAX_REFERENCE_BYTES:
        raise ValueError("The bundled schedule reference exceeds its size bound.")
    reference = json.loads(raw)
    if (not isinstance(reference, dict) or reference.get("version") != REFERENCE_VERSION
            or reference.get("reference_revision") != REFERENCE_REVISION
            or reference.get("kind") != REFERENCE_KIND
            or reference.get("observed_destination") is not False):
        raise ValueError("The bundled schedule reference has invalid version or provenance.")
    return reference, hashlib.sha256(raw).hexdigest()


def _json(value):
    return json.dumps(value, ensure_ascii=True, separators=(",", ":"))


def _render(reference):
    """Avoid repeating the large, identical create/update property schemas."""
    schemas = reference["tool_schemas"]
    create = schemas["meerkat_schedule_create"]
    update = schemas["meerkat_schedule_update"]
    if ({key: value for key, value in update["properties"].items() if key != "schedule_id"}
            != create["properties"]):
        raise ValueError("The bundled update schema no longer shares create property shapes.")
    lines = [reference["contract"].rstrip(), "", RECURRING_APPLICATION_GUIDANCE, "", "Pinned native input schemas:",
             "meerkat_schedule_create:",
             "  type=" + _json(create["type"]),
             "  additionalProperties=" + _json(create["additionalProperties"]),
             "  required=" + _json(create["required"]), "  properties:"]
    lines.extend("    " + name + "=" + _json(schema)
                 for name, schema in create["properties"].items())
    lines.extend(["meerkat_schedule_update:",
                  "  type=" + _json(update["type"]),
                  "  additionalProperties=" + _json(update["additionalProperties"]),
                  "  required=" + _json(update["required"]),
                  "  schedule_id=" + _json(update["properties"]["schedule_id"]),
                  "  All other properties have the exact create schemas above; none is required."])
    lines.extend(name + "=" + _json(schema) for name, schema in schemas.items()
                 if name not in {"meerkat_schedule_create", "meerkat_schedule_update"})
    lines.extend(["", "Pinned native result fields=" + _json(reference["result_fields"]),
                  "Native result serialization=" + _json(reference["result_serialization"])])
    if "observation_error_contract" in reference:
        lines.append("Pinned identity observation error=" + _json(reference["observation_error_contract"]))
    lines.extend(["", "Primary source provenance:"])
    lines.extend(source["url"] + "\n  SHA256=" + source["sha256"]
                 + "; anchors=" + source["anchors"] for source in reference["sources"])
    content = "\n".join(lines)
    if len(content.encode("utf-8")) > MAX_REFERENCE_BYTES:
        raise ValueError("The rendered schedule reference exceeds its size bound.")
    return content


def meta_schedule_reference(sdk_version):
    """Select current product documentation without borrowing another release."""
    if sdk_version != "0.9.4":
        return {"content": selected_recurrence_guidance(),
                "native_authoring_schema": "No bundled contract matches this Meta SDK version; inspect the actual granted native tools."}
    filename = "meta_schedule_reference_0_9_4.json"
    try:
        raw = resources.files(__package__).joinpath(filename).read_bytes()
        if len(raw) > MAX_REFERENCE_BYTES:
            raise ValueError("The bundled Meta schedule reference exceeds its size bound.")
        reference = json.loads(raw)
        if (reference.get("version") != sdk_version or reference.get("reference_revision") != 1
                or reference.get("kind") != REFERENCE_KIND
                or reference.get("observed_destination") is not False):
            raise ValueError("The bundled Meta schedule reference has invalid provenance.")
        return {"content": _render(reference), "schedule_reference_kind": REFERENCE_KIND,
                "schedule_reference_version": sdk_version, "schedule_reference_revision": 1,
                "schedule_reference_sha256": hashlib.sha256(raw).hexdigest(),
                "schedule_reference_file": filename, "schedule_reference_sources": reference["sources"],
                "native_authoring_schema": "Pinned Meta/Core 0.9.4 source documentation with the agent-facing current_session rewrite; not a destination catalog or grant.",
                "observed_destination": False}
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        return {"ok": False, "observed_destination": False,
                "error": "The bundled Meta scheduling reference could not be read: " + str(error)}


def schedule_reference(installed_sdk_version):
    """Refuse applying a pinned source contract to a different installed SDK."""
    metadata = {"reference_kind": REFERENCE_KIND,
                "reference_version": REFERENCE_VERSION,
                "reference_revision": REFERENCE_REVISION,
                "installed_sdk_version": installed_sdk_version,
                "version_matches": installed_sdk_version == REFERENCE_VERSION,
                "observed_destination": False,
                "version_guard_scope": "Installed Python SDK only; verify the destination gateway and tool catalog separately."}
    if not metadata["version_matches"]:
        return {"ok": False, **metadata,
                "error": "The bundled native scheduling contract requires meerkat-mobkit "
                    + REFERENCE_VERSION + ". The installed SDK version differs or is unavailable. "
                    "Inspect the actual runtime contract instead of applying these pinned schemas."}
    try:
        reference, digest = load_schedule_reference()
        return {"ok": True, **metadata, "reference_sha256": digest,
                "sources": reference["sources"], "content": _render(reference)}
    except (OSError, ValueError, KeyError, TypeError) as error:
        return {"ok": False, **metadata,
                "error": "The bundled native scheduling reference could not be read: " + str(error)}
