#!/usr/bin/env python3
"""SDK code generator for Meerkat.

Reads schema artifacts from artifacts/schemas/ and generates typed client
code for Python and TypeScript SDKs.
"""

import argparse
import json
import keyword
import sys
from pathlib import Path
from typing import Any


def load_schemas(artifacts_dir: Path) -> dict:
    """Load all schema artifacts."""
    schemas = {}
    for f in artifacts_dir.glob("*.json"):
        with open(f) as fh:
            schemas[f.stem] = json.load(fh)
    return schemas


def _resolve_schema_ref(root: dict[str, Any], ref: str) -> dict[str, Any]:
    if not ref.startswith("#/$defs/"):
        return {}
    name = ref.removeprefix("#/$defs/")
    defs = root.get("$defs", {})
    return defs.get(name, {})


def _resolve_schema_ref_name(ref: str) -> str | None:
    if not ref.startswith("#/$defs/"):
        return None
    return ref.removeprefix("#/$defs/")


def _lookup_named_schema(root: dict[str, Any], name: str) -> dict[str, Any]:
    schema = root.get(name)
    if isinstance(schema, dict):
        return schema
    defs = root.get("$defs", {})
    nested = defs.get(name)
    return nested if isinstance(nested, dict) else {}


def _python_identifier(name: str) -> str:
    return f"{name}_" if keyword.iskeyword(name) else name


def _pascal_case(name: str) -> str:
    return "".join(part.capitalize() for part in name.split("_"))


def _python_type_from_schema(
    root: dict[str, Any],
    field_schema: Any,
    local_defs: set[str] | None = None,
) -> tuple[str, bool]:
    """Return (python_type, optional)."""
    if field_schema is True:
        return ("Any", False)
    if field_schema is False or not isinstance(field_schema, dict):
        return ("Any", True)
    for key in ("anyOf", "oneOf"):
        variants = field_schema.get(key)
        if isinstance(variants, list) and variants:
            non_null = [
                variant
                for variant in variants
                if not (isinstance(variant, dict) and variant.get("type") == "null")
            ]
            optional = len(non_null) != len(variants)
            if len(non_null) == 1:
                inner_type, inner_optional = _python_type_from_schema(root, non_null[0], local_defs)
                return (inner_type, optional or inner_optional)
            variant_types: list[str] = []
            for variant in non_null:
                inner_type, _ = _python_type_from_schema(root, variant, local_defs)
                if inner_type not in variant_types:
                    variant_types.append(inner_type)
            if variant_types:
                return (" | ".join(variant_types), optional)
    if "$ref" in field_schema:
        ref_name = _resolve_schema_ref_name(str(field_schema["$ref"]))
        resolved = _resolve_schema_ref(root, str(field_schema["$ref"]))
        if not resolved and ref_name:
            resolved = _lookup_named_schema(root, ref_name)
        if ref_name and resolved and ref_name not in (local_defs or set()):
            return (ref_name, False)
        return _python_type_from_schema(root, resolved, local_defs)

    schema_type = field_schema.get("type")
    optional = False
    if isinstance(schema_type, list):
        optional = "null" in schema_type
        non_null = [t for t in schema_type if t != "null"]
        schema_type = non_null[0] if non_null else None

    match schema_type:
        case "string":
            if "enum" in field_schema and isinstance(field_schema["enum"], list):
                values = ", ".join([repr(v) for v in field_schema["enum"]])
                return (f"Literal[{values}]", optional)
            return ("str", optional)
        case "boolean":
            return ("bool", optional)
        case "integer":
            return ("int", optional)
        case "number":
            return ("float", optional)
        case "array":
            item_schema = field_schema.get("items")
            item_type, _ = _python_type_from_schema(root, item_schema, local_defs)
            if item_type == "Any":
                return ("list[Any]", optional)
            return (f"list[{item_type}]", optional)
        case "object":
            properties = field_schema.get("properties")
            additional = field_schema.get("additionalProperties", True)
            if isinstance(properties, dict) and len(properties) == 1 and additional is False:
                (_, value_schema), = properties.items()
                value_type, _ = _python_type_from_schema(root, value_schema)
                return (f"dict[str, {value_type}]", optional)
            return ("dict[str, Any]", optional)
        case _:
            return ("Any", optional)


def _typescript_type_from_schema(
    root: dict[str, Any],
    field_schema: Any,
    local_defs: set[str] | None = None,
) -> tuple[str, bool]:
    """Return (typescript_type, optional)."""
    if field_schema is True:
        return ("unknown", False)
    if field_schema is False or not isinstance(field_schema, dict):
        return ("unknown", True)
    for key in ("anyOf", "oneOf"):
        variants = field_schema.get(key)
        if isinstance(variants, list) and variants:
            non_null = [
                variant
                for variant in variants
                if not (isinstance(variant, dict) and variant.get("type") == "null")
            ]
            optional = len(non_null) != len(variants)
            if len(non_null) == 1:
                inner_type, inner_optional = _typescript_type_from_schema(root, non_null[0], local_defs)
                return (inner_type, optional or inner_optional)
            variant_types: list[str] = []
            for variant in non_null:
                inner_type, _ = _typescript_type_from_schema(root, variant, local_defs)
                if inner_type not in variant_types:
                    variant_types.append(inner_type)
            if variant_types:
                return (" | ".join(variant_types), optional)
    if "$ref" in field_schema:
        ref_name = _resolve_schema_ref_name(str(field_schema["$ref"]))
        resolved = _resolve_schema_ref(root, str(field_schema["$ref"]))
        if not resolved and ref_name:
            resolved = _lookup_named_schema(root, ref_name)
        if ref_name and resolved and ref_name not in (local_defs or set()):
            return (ref_name, False)
        return _typescript_type_from_schema(root, resolved, local_defs)

    schema_type = field_schema.get("type")
    optional = False
    if isinstance(schema_type, list):
        optional = "null" in schema_type
        non_null = [t for t in schema_type if t != "null"]
        schema_type = non_null[0] if non_null else None

    match schema_type:
        case "string":
            if "enum" in field_schema and isinstance(field_schema["enum"], list):
                values = " | ".join([f'"{v}"' for v in field_schema["enum"]])
                return (values, optional)
            return ("string", optional)
        case "boolean":
            return ("boolean", optional)
        case "integer":
            return ("number", optional)
        case "number":
            return ("number", optional)
        case "array":
            item_schema = field_schema.get("items")
            item_type, _ = _typescript_type_from_schema(root, item_schema, local_defs)
            if item_type == "unknown":
                return ("unknown[]", optional)
            return (f"{item_type}[]", optional)
        case "object":
            properties = field_schema.get("properties")
            additional = field_schema.get("additionalProperties", True)
            if isinstance(properties, dict) and len(properties) == 1 and additional is False:
                (field_name, value_schema), = properties.items()
                value_type, _ = _typescript_type_from_schema(root, value_schema)
                return (f"{{ {field_name}: {value_type} }}", optional)
            return ("Record<string, unknown>", optional)
        case _:
            return ("unknown", optional)


def generate_python_types(schemas: dict, output_dir: Path, *, has_comms: bool = True, has_skills: bool = True) -> None:
    """Generate Python type definitions from schemas."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # Generate __init__.py
    init_content = '"""Generated types for Meerkat Python SDK."""\n\n'
    init_content += "from .types import *  # noqa: F401,F403\n"
    init_content += "from .errors import *  # noqa: F401,F403\n"
    (output_dir / "__init__.py").write_text(init_content)

    # Generate types from version and wire-types schemas
    version_info = schemas.get("version", {})
    contract_version = version_info.get("contract_version", "0.2.0")

    types_content = "from __future__ import annotations\n\n"
    types_content += f'"""Generated wire types for Meerkat SDK.\n\nContract version: {contract_version}\n"""\n\n'
    types_content += "from dataclasses import dataclass, field\n"
    types_content += "from typing import Any, Literal, Optional\n\n\n"
    types_content += f'CONTRACT_VERSION = "{contract_version}"\n\n\n'

    # WireUsage
    types_content += "@dataclass\nclass WireUsage:\n"
    types_content += '    """Token usage information."""\n'
    types_content += "    input_tokens: int = 0\n"
    types_content += "    output_tokens: int = 0\n"
    types_content += "    total_tokens: int = 0\n"
    types_content += "    cache_creation_tokens: Optional[int] = None\n"
    types_content += "    cache_read_tokens: Optional[int] = None\n\n\n"

    # WireRunResult
    types_content += "@dataclass\nclass WireRunResult:\n"
    types_content += '    """Run result from agent execution."""\n'
    types_content += "    session_id: str = ''\n"
    types_content += "    session_ref: Optional[str] = None\n"
    types_content += "    text: str = ''\n"
    types_content += "    turns: int = 0\n"
    types_content += "    tool_calls: int = 0\n"
    types_content += "    usage: Optional[WireUsage] = None\n"
    types_content += "    structured_output: Optional[Any] = None\n"
    types_content += "    schema_warnings: Optional[list[Any]] = None\n"
    types_content += "    skill_diagnostics: Optional[dict] = None\n\n\n"

    types_content += "@dataclass\nclass WireProviderMeta:\n"
    types_content += '    """Provider continuity metadata."""\n'
    types_content += "    provider: str = ''\n\n\n"

    types_content += "@dataclass\nclass WireAssistantBlock:\n"
    types_content += '    """Block assistant transcript item."""\n'
    types_content += "    block_type: str = ''\n"
    types_content += "    data: Optional[dict] = None\n\n\n"

    types_content += "@dataclass\nclass WireToolCall:\n"
    types_content += '    """Legacy assistant tool call."""\n'
    types_content += "    id: str = ''\n"
    types_content += "    name: str = ''\n"
    types_content += "    args: Optional[Any] = None\n\n\n"

    types_content += "@dataclass\nclass WireToolResult:\n"
    types_content += '    """Tool result transcript item."""\n'
    types_content += "    tool_use_id: str = ''\n"
    types_content += "    content: Optional[WireToolResultContent] = None\n"
    types_content += "    is_error: Optional[bool] = None\n\n\n"

    types_content += "@dataclass\nclass WireSessionMessage:\n"
    types_content += '    """Canonical transcript message."""\n'
    types_content += "    role: str = ''\n"
    types_content += "    content: Optional[WireContentInput] = None\n"
    types_content += "    tool_calls: Optional[list[WireToolCall]] = None\n"
    types_content += "    stop_reason: Optional[WireStopReason] = None\n"
    types_content += "    blocks: Optional[list[WireAssistantBlock]] = None\n"
    types_content += "    results: Optional[list[WireToolResult]] = None\n\n\n"

    types_content += "@dataclass\nclass WireSessionHistory:\n"
    types_content += '    """Paginated transcript page."""\n'
    types_content += "    session_id: str = ''\n"
    types_content += "    session_ref: Optional[str] = None\n"
    types_content += "    message_count: int = 0\n"
    types_content += "    offset: int = 0\n"
    types_content += "    limit: Optional[int] = None\n"
    types_content += "    has_more: bool = False\n"
    types_content += "    messages: list[WireSessionMessage] = field(default_factory=list)\n\n\n"

    # WireEvent
    types_content += "@dataclass\nclass WireEvent:\n"
    types_content += '    """Event from agent execution stream."""\n'
    types_content += "    session_id: str = ''\n"
    types_content += "    sequence: int = 0\n"
    types_content += "    event: Optional[dict] = None\n"
    types_content += "    contract_version: str = ''\n\n\n"

    # CapabilitiesResponse
    types_content += "@dataclass\nclass CapabilityEntry:\n"
    types_content += '    """A single capability status."""\n'
    types_content += "    id: str = ''\n"
    types_content += "    description: str = ''\n"
    types_content += "    status: str = 'available'\n\n\n"

    types_content += "@dataclass\nclass CapabilitiesResponse:\n"
    types_content += '    """Response from capabilities/get."""\n'
    types_content += "    contract_version: str = ''\n"
    types_content += "    capabilities: list[CapabilityEntry] = field(default_factory=list)\n\n"

    # Conditional params based on available capabilities
    if has_comms:
        types_content += "\n@dataclass\nclass CommsParams:\n"
        types_content += '    """Comms parameters (available because comms capability is compiled)."""\n'
        types_content += "    keep_alive: Optional[bool] = None\n"
        types_content += "    comms_name: Optional[str] = None\n\n"
        types_content += "    peer_meta: Optional[dict[str, Any]] = None\n\n"

    if has_skills:
        types_content += "\n@dataclass\nclass SkillsParams:\n"
        types_content += '    """Skills parameters (available because skills capability is compiled)."""\n'
        types_content += "    skills_enabled: bool = False\n"
        types_content += "    skill_references: list[str] = field(default_factory=list)\n\n"

    params_schema = schemas.get("params", {})
    wire_schema = schemas.get("wire-types", {})

    def append_python_dataclass(name: str, root_schema: dict[str, Any], default_doc: str) -> None:
        nonlocal types_content
        schema = _lookup_named_schema(root_schema, name)
        properties = schema.get("properties", {}) if isinstance(schema, dict) else {}
        required = set(schema.get("required", [])) if isinstance(schema, dict) else set()
        doc = schema.get("description", default_doc) if isinstance(schema, dict) else default_doc
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = dict(root_schema)
        schema_root["$defs"] = {
            **root_schema.get("$defs", {}),
            **(schema.get("$defs", {}) if isinstance(schema, dict) else {}),
        }
        types_content += f"\n@dataclass\nclass {name}:\n"
        types_content += f'    """{doc}"""\n'
        for field_name, field_schema in properties.items():
            python_field_name = _python_identifier(field_name)
            field_type, is_optional_type = _python_type_from_schema(
                schema_root,
                field_schema,
                local_defs,
            )
            is_required = field_name in required
            annotation = field_type
            if is_optional_type and not is_required:
                annotation = f"Optional[{field_type}]"
            if is_optional_type and not is_required:
                default_expr = "None"
            elif field_type == "bool":
                default_expr = "False"
            elif field_type == "str":
                default_expr = "''"
            elif field_type in {"int", "float"}:
                default_expr = "0"
            elif field_type == "list[Any]" or field_type.startswith("list["):
                default_expr = "field(default_factory=list)"
            elif field_type == "dict[str, Any]":
                default_expr = "field(default_factory=dict)"
            else:
                default_expr = "None"
            types_content += f"    {python_field_name}: {annotation} = {default_expr}\n"
        types_content += "\n"

    def append_python_alias(name: str, root_schema: dict[str, Any], default_doc: str) -> None:
        nonlocal types_content
        schema = _lookup_named_schema(root_schema, name)
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = dict(root_schema)
        schema_root["$defs"] = {
            **root_schema.get("$defs", {}),
            **(schema.get("$defs", {}) if isinstance(schema, dict) else {}),
        }
        alias_type, _ = _python_type_from_schema(schema_root, schema, local_defs)
        doc = schema.get("description", default_doc) if isinstance(schema, dict) else default_doc
        # Ensure multi-line descriptions are fully commented out.
        doc_lines = str(doc).splitlines() or [""]
        doc_block = "\n".join(f"# {line}" for line in doc_lines)
        types_content += f"\n{doc_block}\n{name} = {alias_type}\n"

    append_python_dataclass("McpAddParams", params_schema, "Request payload for mcp/add.")
    append_python_dataclass("McpRemoveParams", params_schema, "Request payload for mcp/remove.")
    append_python_dataclass("McpReloadParams", params_schema, "Request payload for mcp/reload.")
    append_python_dataclass("MobWireParams", params_schema, "Request payload for mob/wire.")
    append_python_dataclass("MobUnwireParams", params_schema, "Request payload for mob/unwire.")
    append_python_dataclass("RuntimeStateParams", params_schema, "Request payload for runtime/state.")
    append_python_dataclass(
        "RuntimeRealtimeAttachmentStatusParams",
        params_schema,
        "Request payload for runtime/realtime_attachment_status.",
    )
    append_python_dataclass("RealtimeOpenRequest", params_schema, "Request payload for realtime/open_info.")
    append_python_dataclass("RealtimeStatusParams", params_schema, "Request payload for realtime/status.")
    append_python_dataclass("RealtimeCapabilitiesParams", params_schema, "Request payload for realtime/capabilities.")
    append_python_dataclass("RuntimeAcceptParams", params_schema, "Request payload for runtime/accept.")
    append_python_dataclass("RuntimeRetireParams", params_schema, "Request payload for runtime/retire.")
    append_python_dataclass("RuntimeResetParams", params_schema, "Request payload for runtime/reset.")
    append_python_dataclass("InputStateParams", params_schema, "Request payload for input/state.")
    append_python_dataclass("InputListParams", params_schema, "Request payload for input/list.")
    append_python_dataclass("ScheduleIdParams", params_schema, "Request payload for schedule id lookups.")
    append_python_dataclass("ListSchedulesParams", params_schema, "Request payload for schedule/list.")
    append_python_dataclass("ScheduleOccurrencesParams", params_schema, "Request payload for schedule/occurrences.")
    append_python_dataclass("UpdateScheduleParams", params_schema, "Request payload for schedule/update.")
    append_python_dataclass("McpLiveOpResponse", wire_schema, "Response payload for mcp/add|remove|reload.")
    append_python_dataclass("WireRenderMetadata", wire_schema, "Render metadata for mob member delivery.")
    append_python_dataclass("WireTrustedPeerSpec", wire_schema, "Minimal trusted peer spec for mob wiring.")
    append_python_dataclass("MobWireResult", wire_schema, "Response payload for mob/wire.")
    append_python_dataclass("MobUnwireResult", wire_schema, "Response payload for mob/unwire.")
    append_python_dataclass("RuntimeStateResult", wire_schema, "Response payload for runtime/state.")
    append_python_dataclass(
        "RuntimeRealtimeAttachmentStatusResult",
        wire_schema,
        "Response payload for runtime/realtime_attachment_status.",
    )
    append_python_dataclass("RealtimeReconnectPolicy", wire_schema, "Reconnect policy for realtime channels.")
    append_python_dataclass("RealtimeCapabilities", wire_schema, "Capability set for a realtime channel.")
    append_python_dataclass("RealtimeChannelStatus", wire_schema, "Public realtime channel status projection.")
    append_python_dataclass("RealtimeOpenInfo", wire_schema, "Response payload for realtime/open_info.")
    append_python_dataclass("RealtimeStatusResult", wire_schema, "Response payload for realtime/status.")
    append_python_dataclass("RealtimeCapabilitiesResult", wire_schema, "Response payload for realtime/capabilities.")
    append_python_dataclass("RealtimeTextChunk", wire_schema, "Text chunk for realtime ingress.")
    append_python_dataclass("RealtimeTextDelta", wire_schema, "Text delta for realtime output.")
    append_python_dataclass("RealtimeAudioChunk", wire_schema, "Opaque realtime audio chunk.")
    append_python_dataclass("RealtimeVideoChunk", wire_schema, "Opaque realtime video chunk.")
    append_python_dataclass("RealtimeChannelOpenFrame", wire_schema, "Payload for channel.open.")
    append_python_dataclass("RealtimeChannelInputFrame", wire_schema, "Payload for channel.input.")
    append_python_dataclass("RealtimeChannelOpenedFrame", wire_schema, "Payload for channel.opened.")
    append_python_dataclass("RealtimeChannelStatusFrame", wire_schema, "Payload for channel.status.")
    append_python_dataclass("RealtimeChannelEventFrame", wire_schema, "Payload for channel.event.")
    append_python_dataclass("RealtimeChannelErrorFrame", wire_schema, "Payload for channel.error.")
    append_python_dataclass("RealtimeChannelClosedFrame", wire_schema, "Payload for channel.closed.")
    append_python_dataclass("RuntimeAcceptResult", wire_schema, "Response payload for runtime/accept.")
    append_python_dataclass("RuntimeRetireResult", wire_schema, "Response payload for runtime/retire.")
    append_python_dataclass("RuntimeResetResult", wire_schema, "Response payload for runtime/reset.")
    append_python_dataclass("WireInputStateHistoryEntry", wire_schema, "Input transition history entry.")
    append_python_dataclass("WireInputState", wire_schema, "Runtime input state snapshot.")
    append_python_dataclass("InputListResult", wire_schema, "Response payload for input/list.")
    append_python_dataclass("ScheduleListResult", wire_schema, "Response payload for schedule/list.")
    append_python_dataclass("ScheduleOccurrencesResult", wire_schema, "Response payload for schedule/occurrences.")
    append_python_dataclass("WireSessionInfo", wire_schema, "Detailed session metadata payload.")
    append_python_dataclass("WireSessionSummary", wire_schema, "Session summary payload returned by session/list.")
    append_python_dataclass("ContractVersion", schemas.get("models", {}), "Semantic contract version triple.")
    append_python_dataclass("CatalogModelEntry", schemas.get("models", {}), "Catalog model entry.")
    append_python_dataclass("ProviderCatalog", schemas.get("models", {}), "Provider grouping in the model catalog.")
    append_python_dataclass("ModelsCatalogResponse", schemas.get("models", {}), "Response payload for models/catalog.")
    append_python_dataclass("WireModelProfile", schemas.get("models", {}), "Wire-level model capability profile.")

    # Phase 4c — connection/auth wire types.
    append_python_dataclass("WireConnectionRef", wire_schema, "Session-facing reference to a binding inside a realm.")
    append_python_dataclass("WireBackendProfile", wire_schema, "Backend profile projected for the wire.")
    append_python_dataclass("WireAuthProfile", wire_schema, "Auth profile projected for the wire (secret material not included).")
    append_python_dataclass("WireProviderBinding", wire_schema, "Provider binding: backend+auth pair plus policy.")
    append_python_dataclass("WireRealmConnectionSet", wire_schema, "Full realm connection set (backends, auth profiles, bindings).")
    append_python_dataclass("WireAuthStatus", wire_schema, "Auth profile health snapshot.")
    append_python_alias("WireAuthError", wire_schema, "Auth error (tagged variant).")

    # Keep aliases after the dataclasses they reference. Unlike annotations, alias
    # assignments are evaluated eagerly at import time.
    append_python_alias("WireContentBlock", wire_schema, "Wire-safe content block.")
    append_python_alias("WireContentInput", wire_schema, "Wire-safe content input.")
    append_python_alias("McpLiveOperation", wire_schema, "Shared operation kind for live MCP operations.")
    append_python_alias("McpLiveOpStatus", wire_schema, "Shared status for live MCP operations.")
    append_python_alias("MobPeerTarget", wire_schema, "Target for a mob wire/unwire call.")
    append_python_alias("WireHandlingMode", wire_schema, "Public handling mode for mob member delivery.")
    append_python_alias("WireRenderClass", wire_schema, "Public render class contract for mob member delivery.")
    append_python_alias("WireRenderSalience", wire_schema, "Public render salience contract for mob member delivery.")
    append_python_alias("WireRuntimeState", wire_schema, "Public runtime state projection used by RPC surfaces.")
    append_python_alias(
        "WireRealtimeAttachmentStatus",
        wire_schema,
        "Public live attachment status projection used by runtime and mob surfaces.",
    )
    append_python_alias("RealtimeChannelTarget", wire_schema, "Public realtime target union.")
    append_python_alias("RealtimeChannelRole", wire_schema, "Realtime channel opening role.")
    append_python_alias("RealtimeTurningMode", wire_schema, "Realtime turning mode.")
    append_python_alias("RealtimeInputKind", wire_schema, "Realtime input kind.")
    append_python_alias("RealtimeOutputKind", wire_schema, "Realtime output kind.")
    append_python_alias("RealtimeChannelState", wire_schema, "Realtime channel lifecycle state.")
    append_python_alias("RealtimeInputChunk", wire_schema, "Realtime input chunk union.")
    append_python_alias("RealtimeOutputChunk", wire_schema, "Realtime output chunk union.")
    append_python_alias("RealtimeEvent", wire_schema, "Realtime event union.")
    append_python_alias("RealtimeClientFrame", wire_schema, "Realtime client frame union.")
    append_python_alias("RealtimeServerFrame", wire_schema, "Realtime server frame union.")
    append_python_alias("RuntimeAcceptOutcomeType", wire_schema, "Discriminator for runtime/accept responses.")
    append_python_alias("WireInputLifecycleState", wire_schema, "Public input lifecycle state projection used by RPC surfaces.")
    append_python_alias("WireStopReason", wire_schema, "Canonical stop reason for transcript messages.")
    append_python_alias("WireToolResultContent", wire_schema, "Wire-safe tool result content.")
    append_python_alias("WireModelTier", schemas.get("models", {}), "Wire-level model recommendation tier.")
    append_python_alias(
        "CommsCommandRequest",
        wire_schema,
        "Typed comms/send command (serde-tagged on `kind`).",
    )
    types_content += "\n# Response payload for `input/state`.\nInputStateResult = Optional[WireInputState]\n"

    (output_dir / "types.py").write_text(types_content)

    # Generate error types
    errors_content = '"""Generated error types for Meerkat SDK."""\n\n\n'
    errors_content += "class MeerkatError(Exception):\n"
    errors_content += '    """Base error for Meerkat SDK."""\n\n'
    errors_content += "    def __init__(self, code: str, message: str, details=None, capability_hint=None):\n"
    errors_content += "        super().__init__(message)\n"
    errors_content += "        self.code = code\n"
    errors_content += "        self.message = message\n"
    errors_content += "        self.details = details\n"
    errors_content += "        self.capability_hint = capability_hint\n\n\n"

    errors_content += "class CapabilityUnavailableError(MeerkatError):\n"
    errors_content += '    """Raised when a capability is not available."""\n'
    errors_content += "    pass\n\n\n"

    errors_content += "class SessionNotFoundError(MeerkatError):\n"
    errors_content += '    """Raised when a session is not found."""\n'
    errors_content += "    pass\n\n\n"

    errors_content += "class SkillNotFoundError(MeerkatError):\n"
    errors_content += '    """Raised when a skill reference cannot be resolved."""\n'
    errors_content += "    pass\n"

    (output_dir / "errors.py").write_text(errors_content)


def generate_typescript_types(schemas: dict, output_dir: Path, *, has_comms: bool = True, has_skills: bool = True) -> None:
    """Generate TypeScript type definitions from schemas."""
    output_dir.mkdir(parents=True, exist_ok=True)

    version_info = schemas.get("version", {})
    contract_version = version_info.get("contract_version", "0.2.0")

    # Generate types
    types_content = f"// Generated wire types for Meerkat SDK\n// Contract version: {contract_version}\n\n"
    types_content += f'export const CONTRACT_VERSION = "{contract_version}";\n\n'

    types_content += "export interface WireUsage {\n"
    types_content += "  input_tokens: number;\n"
    types_content += "  output_tokens: number;\n"
    types_content += "  total_tokens: number;\n"
    types_content += "  cache_creation_tokens?: number;\n"
    types_content += "  cache_read_tokens?: number;\n"
    types_content += "}\n\n"

    types_content += "export interface WireRunResult {\n"
    types_content += "  session_id: string;\n"
    types_content += "  session_ref?: string;\n"
    types_content += "  text: string;\n"
    types_content += "  turns: number;\n"
    types_content += "  tool_calls: number;\n"
    types_content += "  usage: WireUsage;\n"
    types_content += "  structured_output?: unknown;\n"
    types_content += "  schema_warnings?: Array<{ provider: string; path: string; message: string }>;\n"
    types_content += "}\n\n"

    types_content += "export interface WireProviderMeta {\n"
    types_content += "  provider: string;\n"
    types_content += "  [key: string]: unknown;\n"
    types_content += "}\n\n"

    types_content += "export interface WireAssistantBlock {\n"
    types_content += "  block_type: string;\n"
    types_content += "  data: Record<string, unknown>;\n"
    types_content += "}\n\n"

    types_content += "export interface WireToolCall {\n"
    types_content += "  id: string;\n"
    types_content += "  name: string;\n"
    types_content += "  args: unknown;\n"
    types_content += "}\n\n"

    types_content += "export interface WireToolResult {\n"
    types_content += "  tool_use_id: string;\n"
    types_content += "  content: WireToolResultContent;\n"
    types_content += "  is_error?: boolean;\n"
    types_content += "}\n\n"

    types_content += "export interface WireSessionMessage {\n"
    types_content += "  role: string;\n"
    types_content += "  content?: WireContentInput;\n"
    types_content += "  tool_calls?: WireToolCall[];\n"
    types_content += "  stop_reason?: WireStopReason;\n"
    types_content += "  blocks?: WireAssistantBlock[];\n"
    types_content += "  results?: WireToolResult[];\n"
    types_content += "}\n\n"

    types_content += "export interface WireSessionHistory {\n"
    types_content += "  session_id: string;\n"
    types_content += "  session_ref?: string;\n"
    types_content += "  message_count: number;\n"
    types_content += "  offset: number;\n"
    types_content += "  limit?: number;\n"
    types_content += "  has_more: boolean;\n"
    types_content += "  messages: WireSessionMessage[];\n"
    types_content += "}\n\n"

    types_content += "export interface WireEvent {\n"
    types_content += "  session_id: string;\n"
    types_content += "  sequence: number;\n"
    types_content += "  event: Record<string, unknown>;\n"
    types_content += "  contract_version: string;\n"
    types_content += "}\n\n"

    types_content += "export interface CapabilityEntry {\n"
    types_content += "  id: string;\n"
    types_content += "  description: string;\n"
    types_content += "  status: string;\n"
    types_content += "}\n\n"

    types_content += "export interface CapabilitiesResponse {\n"
    types_content += "  contract_version: string;\n"
    types_content += "  capabilities: CapabilityEntry[];\n"
    types_content += "}\n"

    # Conditional params based on available capabilities
    if has_comms:
        types_content += "\nexport interface CommsParams {\n"
        types_content += "  keep_alive?: boolean | null;\n"
        types_content += "  comms_name?: string;\n"
        types_content += "  peer_meta?: Record<string, unknown>;\n"
        types_content += "}\n"

    if has_skills:
        types_content += "\nexport interface SkillsParams {\n"
        types_content += "  skills_enabled: boolean;\n"
        types_content += "  skill_references: string[];\n"
        types_content += "}\n"

    params_schema = schemas.get("params", {})
    wire_schema = schemas.get("wire-types", {})

    def append_typescript_interface(name: str, root_schema: dict[str, Any]) -> None:
        nonlocal types_content
        schema = _lookup_named_schema(root_schema, name)
        properties = schema.get("properties", {}) if isinstance(schema, dict) else {}
        required = set(schema.get("required", [])) if isinstance(schema, dict) else set()
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = dict(root_schema)
        schema_root["$defs"] = {
            **root_schema.get("$defs", {}),
            **(schema.get("$defs", {}) if isinstance(schema, dict) else {}),
        }
        types_content += f"\nexport interface {name} {{\n"
        for field_name, field_schema in properties.items():
            field_type, optional_by_type = _typescript_type_from_schema(
                schema_root,
                field_schema,
                local_defs,
            )
            optional = "?" if (field_name not in required or optional_by_type) else ""
            types_content += f"  {field_name}{optional}: {field_type};\n"
        types_content += "}\n"

    def append_typescript_alias(name: str, root_schema: dict[str, Any]) -> None:
        nonlocal types_content
        schema = _lookup_named_schema(root_schema, name)
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = dict(root_schema)
        schema_root["$defs"] = {
            **root_schema.get("$defs", {}),
            **(schema.get("$defs", {}) if isinstance(schema, dict) else {}),
        }
        alias_type, _ = _typescript_type_from_schema(schema_root, schema, local_defs)
        types_content += f"\nexport type {name} = {alias_type};\n"

    append_typescript_interface("McpAddParams", params_schema)
    append_typescript_interface("McpRemoveParams", params_schema)
    append_typescript_interface("McpReloadParams", params_schema)
    append_typescript_interface("MobWireParams", params_schema)
    append_typescript_interface("MobUnwireParams", params_schema)
    append_typescript_interface("RuntimeStateParams", params_schema)
    append_typescript_interface("RuntimeRealtimeAttachmentStatusParams", params_schema)
    append_typescript_interface("RealtimeOpenRequest", params_schema)
    append_typescript_interface("RealtimeStatusParams", params_schema)
    append_typescript_interface("RealtimeCapabilitiesParams", params_schema)
    append_typescript_interface("RuntimeAcceptParams", params_schema)
    append_typescript_interface("RuntimeRetireParams", params_schema)
    append_typescript_interface("RuntimeResetParams", params_schema)
    append_typescript_interface("InputStateParams", params_schema)
    append_typescript_interface("InputListParams", params_schema)
    append_typescript_interface("ScheduleIdParams", params_schema)
    append_typescript_interface("ListSchedulesParams", params_schema)
    append_typescript_interface("ScheduleOccurrencesParams", params_schema)
    append_typescript_interface("UpdateScheduleParams", params_schema)
    append_typescript_interface("McpLiveOpResponse", wire_schema)
    types_content += "\nexport type InputStateResult = WireInputState | null;\n"
    append_typescript_alias("WireContentBlock", wire_schema)
    append_typescript_alias("WireContentInput", wire_schema)
    append_typescript_alias("McpLiveOperation", wire_schema)
    append_typescript_alias("McpLiveOpStatus", wire_schema)
    append_typescript_alias("MobPeerTarget", wire_schema)
    append_typescript_alias("WireHandlingMode", wire_schema)
    append_typescript_alias("WireRenderClass", wire_schema)
    append_typescript_alias("WireRenderSalience", wire_schema)
    append_typescript_alias("WireRuntimeState", wire_schema)
    append_typescript_alias("WireRealtimeAttachmentStatus", wire_schema)
    append_typescript_alias("RealtimeChannelTarget", wire_schema)
    append_typescript_alias("RealtimeChannelRole", wire_schema)
    append_typescript_alias("RealtimeTurningMode", wire_schema)
    append_typescript_alias("RealtimeInputKind", wire_schema)
    append_typescript_alias("RealtimeOutputKind", wire_schema)
    append_typescript_alias("RealtimeChannelState", wire_schema)
    append_typescript_alias("RealtimeInputChunk", wire_schema)
    append_typescript_alias("RealtimeOutputChunk", wire_schema)
    append_typescript_alias("RealtimeEvent", wire_schema)
    append_typescript_alias("RealtimeClientFrame", wire_schema)
    append_typescript_alias("RealtimeServerFrame", wire_schema)
    append_typescript_alias("RuntimeAcceptOutcomeType", wire_schema)
    append_typescript_alias("WireInputLifecycleState", wire_schema)
    append_typescript_alias("WireStopReason", wire_schema)
    append_typescript_alias("WireToolResultContent", wire_schema)
    append_typescript_alias("WireModelTier", schemas.get("models", {}))
    append_typescript_alias("CommsCommandRequest", wire_schema)
    append_typescript_interface("WireRenderMetadata", wire_schema)
    append_typescript_interface("WireTrustedPeerSpec", wire_schema)
    append_typescript_interface("MobWireResult", wire_schema)
    append_typescript_interface("MobUnwireResult", wire_schema)
    append_typescript_interface("RuntimeStateResult", wire_schema)
    append_typescript_interface("RuntimeRealtimeAttachmentStatusResult", wire_schema)
    append_typescript_interface("RealtimeReconnectPolicy", wire_schema)
    append_typescript_interface("RealtimeCapabilities", wire_schema)
    append_typescript_interface("RealtimeChannelStatus", wire_schema)
    append_typescript_interface("RealtimeOpenInfo", wire_schema)
    append_typescript_interface("RealtimeStatusResult", wire_schema)
    append_typescript_interface("RealtimeCapabilitiesResult", wire_schema)
    append_typescript_interface("RealtimeTextChunk", wire_schema)
    append_typescript_interface("RealtimeTextDelta", wire_schema)
    append_typescript_interface("RealtimeAudioChunk", wire_schema)
    append_typescript_interface("RealtimeVideoChunk", wire_schema)
    append_typescript_interface("RealtimeChannelOpenFrame", wire_schema)
    append_typescript_interface("RealtimeChannelInputFrame", wire_schema)
    append_typescript_interface("RealtimeChannelOpenedFrame", wire_schema)
    append_typescript_interface("RealtimeChannelStatusFrame", wire_schema)
    append_typescript_interface("RealtimeChannelEventFrame", wire_schema)
    append_typescript_interface("RealtimeChannelErrorFrame", wire_schema)
    append_typescript_interface("RealtimeChannelClosedFrame", wire_schema)
    append_typescript_interface("RuntimeAcceptResult", wire_schema)
    append_typescript_interface("RuntimeRetireResult", wire_schema)
    append_typescript_interface("RuntimeResetResult", wire_schema)
    append_typescript_interface("WireInputStateHistoryEntry", wire_schema)
    append_typescript_interface("WireInputState", wire_schema)
    append_typescript_interface("InputListResult", wire_schema)
    append_typescript_interface("ScheduleListResult", wire_schema)
    append_typescript_interface("ScheduleOccurrencesResult", wire_schema)
    append_typescript_interface("WireSessionInfo", wire_schema)
    append_typescript_interface("WireSessionSummary", wire_schema)
    append_typescript_interface("ContractVersion", schemas.get("models", {}))
    append_typescript_interface("CatalogModelEntry", schemas.get("models", {}))
    append_typescript_interface("ProviderCatalog", schemas.get("models", {}))
    append_typescript_interface("ModelsCatalogResponse", schemas.get("models", {}))
    append_typescript_interface("WireModelProfile", schemas.get("models", {}))

    # Phase 4c — connection/auth wire types.
    append_typescript_interface("WireConnectionRef", wire_schema)
    append_typescript_interface("WireBackendProfile", wire_schema)
    append_typescript_interface("WireAuthProfile", wire_schema)
    append_typescript_interface("WireProviderBinding", wire_schema)
    append_typescript_interface("WireRealmConnectionSet", wire_schema)
    append_typescript_interface("WireAuthStatus", wire_schema)
    append_typescript_alias("WireAuthError", wire_schema)

    (output_dir / "types.ts").write_text(types_content)

    # Generate errors
    errors_content = "// Generated error types for Meerkat SDK\n\n"
    errors_content += "export class MeerkatError extends Error {\n"
    errors_content += "  constructor(\n"
    errors_content += "    public readonly code: string,\n"
    errors_content += "    message: string,\n"
    errors_content += "    public readonly details?: unknown,\n"
    errors_content += "    public readonly capabilityHint?: { capability_id: string; message: string },\n"
    errors_content += "  ) {\n"
    errors_content += "    super(message);\n"
    errors_content += "    this.name = 'MeerkatError';\n"
    errors_content += "  }\n"
    errors_content += "}\n\n"

    errors_content += "export class CapabilityUnavailableError extends MeerkatError {\n"
    errors_content += "  constructor(code: string, message: string, details?: unknown, capabilityHint?: { capability_id: string; message: string }) {\n"
    errors_content += "    super(code, message, details, capabilityHint);\n"
    errors_content += "    this.name = 'CapabilityUnavailableError';\n"
    errors_content += "  }\n"
    errors_content += "}\n\n"

    errors_content += "export class SessionNotFoundError extends MeerkatError {\n"
    errors_content += "  constructor(code: string, message: string) {\n"
    errors_content += "    super(code, message);\n"
    errors_content += "    this.name = 'SessionNotFoundError';\n"
    errors_content += "  }\n"
    errors_content += "}\n\n"

    errors_content += "export class SkillNotFoundError extends MeerkatError {\n"
    errors_content += "  constructor(code: string, message: string) {\n"
    errors_content += "    super(code, message);\n"
    errors_content += "    this.name = 'SkillNotFoundError';\n"
    errors_content += "  }\n"
    errors_content += "}\n"

    (output_dir / "errors.ts").write_text(errors_content)

    # Index file
    index_content = "// Generated exports\nexport * from './types.js';\nexport * from './errors.js';\n"
    (output_dir / "index.ts").write_text(index_content)


def _web_events_ts_type(root: dict[str, Any], schema: Any) -> str:
    if schema is True:
        return "unknown"
    if schema is False or not isinstance(schema, dict):
        return "unknown"
    if "const" in schema:
        return json.dumps(schema["const"])
    if "$ref" in schema:
        ref_name = _resolve_schema_ref_name(str(schema["$ref"]))
        if ref_name:
            return ref_name
        return "unknown"
    if "enum" in schema and isinstance(schema["enum"], list):
        return " | ".join(json.dumps(value) for value in schema["enum"])
    for key in ("anyOf", "oneOf"):
        variants = schema.get(key)
        if isinstance(variants, list) and variants:
            variant_types: list[str] = []
            for variant in variants:
                inner = _web_events_ts_type(root, variant)
                if inner not in variant_types:
                    variant_types.append(inner)
            return " | ".join(variant_types) if variant_types else "unknown"

    schema_type = schema.get("type")
    if isinstance(schema_type, list):
        non_null = [item for item in schema_type if item != "null"]
        if not non_null:
            return "null"
        if len(non_null) == 1:
            base = _web_events_ts_type(root, {**schema, "type": non_null[0]})
            return f"{base} | null" if "null" in schema_type else base
        variant_types: list[str] = []
        for item in non_null:
            inner = _web_events_ts_type(root, {**schema, "type": item})
            if inner not in variant_types:
                variant_types.append(inner)
        if "null" in schema_type and "null" not in variant_types:
            variant_types.append("null")
        return " | ".join(variant_types)

    if schema_type == "string":
        return "string"
    if schema_type in {"integer", "number"}:
        return "number"
    if schema_type == "boolean":
        return "boolean"
    if schema_type == "array":
        item_type = _web_events_ts_type(root, schema.get("items"))
        return f"{item_type}[]"
    if schema_type == "object":
        properties = schema.get("properties")
        required = set(schema.get("required", []))
        if isinstance(properties, dict):
            lines = ["{"]
            for field_name, field_schema in properties.items():
                field_type = _web_events_ts_type(root, field_schema)
                optional = "?" if field_name not in required else ""
                lines.append(f"  {field_name}{optional}: {field_type};")
            additional = schema.get("additionalProperties", False)
            if additional is True:
                lines.append("  [key: string]: unknown;")
            elif isinstance(additional, dict):
                additional_type = _web_events_ts_type(root, additional)
                lines.append(f"  [key: string]: {additional_type};")
            lines.append("}")
            return "\n".join(lines)
        return "Record<string, unknown>"
    return "unknown"


def generate_web_event_types(schemas: dict, output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    events_schema = schemas.get("events", {})
    agent_schema = events_schema.get("AgentEvent", {})
    wire_event = events_schema.get("WireEvent", {})
    defs = agent_schema.get("$defs", {})
    known_event_types = wire_event.get("known_event_types", [])
    variants = agent_schema.get("oneOf", [])

    def is_simple_object_literal(type_string: str) -> bool:
        stripped = type_string.strip()
        return stripped.startswith("{") and stripped.endswith("}") and " | " not in stripped

    def object_literal_body(type_string: str) -> str:
        stripped = type_string.strip()
        return stripped[1:-1].strip()

    lines: list[str] = [
        "// Generated raw event types for @rkat/web",
        "// Source: artifacts/schemas/events.json",
        "",
    ]

    for def_name, def_schema in defs.items():
        alias_type = _web_events_ts_type(agent_schema, def_schema)
        if is_simple_object_literal(alias_type):
            lines.append(f"export interface {def_name} {{")
            body = object_literal_body(alias_type)
            if body:
                lines.extend(f"  {line}" if not line.startswith("  ") else line for line in body.splitlines())
            lines.append("}")
        else:
            lines.append(f"export type {def_name} = {alias_type};")
        lines.append("")

    event_interface_names: list[str] = []
    for variant in variants:
        if not isinstance(variant, dict):
            continue
        properties = variant.get("properties", {})
        event_type = (
            properties.get("type", {}).get("const")
            if isinstance(properties.get("type"), dict)
            else None
        )
        if not isinstance(event_type, str):
            continue
        interface_name = f"{_pascal_case(event_type)}Event"
        event_interface_names.append(interface_name)
        lines.append(f"export interface {interface_name} {{")
        required = set(variant.get("required", []))
        for field_name, field_schema in properties.items():
            field_type = _web_events_ts_type(agent_schema, field_schema)
            optional = "?" if field_name not in required else ""
            lines.append(f"  {field_name}{optional}: {field_type};")
        lines.append("}")
        lines.append("")

    if known_event_types:
        known_items = ",\n  ".join(json.dumps(value) for value in known_event_types)
        lines.append("export const KNOWN_AGENT_EVENT_TYPES = [")
        lines.append(f"  {known_items}")
        lines.append("] as const;")
        lines.append("")
        lines.append(
            "export type KnownAgentEventType = typeof KNOWN_AGENT_EVENT_TYPES[number];"
        )
        lines.append("")

    if event_interface_names:
        union = " |\n  ".join(event_interface_names)
        lines.append("export type AgentEvent =")
        lines.append(f"  {union};")
        lines.append("")

    (output_dir / "events.ts").write_text("\n".join(lines))


def load_available_capabilities(artifacts_dir: Path) -> set[str]:
    """Load available capability IDs from capabilities.json."""
    caps_file = artifacts_dir / "capabilities.json"
    if not caps_file.exists():
        return set()  # All capabilities if no file
    with open(caps_file) as f:
        data = json.load(f)
    # Extract capability IDs from the CapabilityId schema's "enum" array
    cap_schema = data.get("CapabilityId", {})
    enum_values = cap_schema.get("enum", [])
    return set(enum_values)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        help="Repository root to read schemas from (defaults to the workspace root inferred from this script).",
    )
    parser.add_argument(
        "--artifacts-dir",
        type=Path,
        help="Schema artifact directory to read from (defaults to <root>/artifacts/schemas).",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        help="Root directory under which generated SDK files should be written (defaults to <root>).",
    )
    return parser.parse_args()


def main():
    args = parse_args()
    root = (args.root or Path(__file__).resolve().parent.parent.parent).resolve()
    artifacts_dir = (args.artifacts_dir or (root / "artifacts" / "schemas")).resolve()
    output_root = (args.output_root or root).resolve()

    if not artifacts_dir.exists():
        print(f"Error: {artifacts_dir} does not exist. Run emit-schemas first.", file=sys.stderr)
        sys.exit(1)

    schemas = load_schemas(artifacts_dir)
    print(f"Loaded {len(schemas)} schema files")

    # Load available capabilities for conditional codegen
    available_caps = load_available_capabilities(artifacts_dir)
    if available_caps:
        print(f"Available capabilities: {sorted(available_caps)}")
    else:
        print("No capabilities.json found; generating full surface")

    # Determine which optional param types to include
    has_comms = "comms" in available_caps or not available_caps
    has_skills = "skills" in available_caps or not available_caps

    # Generate Python
    py_output = output_root / "sdks" / "python" / "meerkat" / "generated"
    generate_python_types(schemas, py_output, has_comms=has_comms, has_skills=has_skills)
    print(f"Generated Python types in {py_output}")

    # Generate TypeScript
    ts_output = output_root / "sdks" / "typescript" / "src" / "generated"
    generate_typescript_types(schemas, ts_output, has_comms=has_comms, has_skills=has_skills)
    print(f"Generated TypeScript types in {ts_output}")

    # Generate web event types from the canonical contracts artifact.
    web_events_output = output_root / "sdks" / "web" / "src" / "generated"
    generate_web_event_types(schemas, web_events_output)
    print(f"Generated web event types in {web_events_output}")


if __name__ == "__main__":
    main()
