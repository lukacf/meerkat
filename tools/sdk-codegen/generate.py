#!/usr/bin/env python3
"""SDK code generator for Meerkat.

Reads schema artifacts from artifacts/schemas/ and generates typed client
code for Python and TypeScript SDKs.
"""

import argparse
import json
import keyword
import re
import sys
from pathlib import Path
from typing import Any


MOB_RPC_CONTRACT_TYPES = [
    "MobIdParams",
    "MobMemberParams",
    "MobCreateParams",
    "MobCreateResult",
    "MobListResult",
    "MobStatusResult",
    "MobLifecycleParams",
    "MobLifecycleResult",
    "MobSpawnParams",
    "MobSpawnResult",
    "MobSpawnSpecParams",
    "MobSpawnManyParams",
    "MobSpawnManyResult",
    "MobSpawnManyResultEntry",
    "MobSpawnManySpawnedResult",
    "MobSpawnManyFailedResult",
    "MobSpawnReceiptWire",
    "MobMemberListEntryWire",
    "WireMobToolConfig",
    "WireMobProfile",
    "MobEnsureMemberParams",
    "MobEnsureMemberResult",
    "MobReconcileParams",
    "MobReconcileResult",
    "MobListMembersMatchingParams",
    "MobListMembersMatchingResult",
    "MobRetireResult",
    "MobRespawnParams",
    "MobRespawnResult",
    "MobWireParams",
    "MobWireResult",
    "MobUnwireParams",
    "MobUnwireResult",
    "MobMembersResult",
    "MobEventsParams",
    "MobEventsResult",
    "MobMemberSendParams",
    "MobMemberSendResult",
    "MobIngressInteractionParams",
    "MobIngressInteractionResult",
    "MobAppendSystemContextParams",
    "MobAppendSystemContextResult",
    "MobFlowsResult",
    "MobFlowRunParams",
    "MobFlowRunResult",
    "MobFlowStatusParams",
    "MobFlowStatusResult",
    "MobFlowCancelParams",
    "MobFlowCancelResult",
    "MobSpawnHelperParams",
    "MobForkHelperParams",
    "MobHelperResult",
    "MobForceCancelResult",
    "MobTurnStartParams",
    "MobMemberStatusResult",
    "MobSnapshotResult",
    "MobDestroyResult",
    "MobRotateSupervisorResult",
    "MobSubmitWorkParams",
    "MobSubmitWorkResult",
    "MobCancelWorkParams",
    "MobCancelWorkResult",
    "MobCancelAllWorkParams",
    "MobCancelAllWorkResult",
    "MobWaitParams",
    "MobWaitMembersResult",
    "MobProfileCreateParams",
    "MobProfileNameParams",
    "MobProfileLookupResult",
    "MobProfileListResult",
    "MobProfileUpdateParams",
    "MobProfileDeleteParams",
    "MobProfileDeleteResult",
    "MobStreamOpenParams",
    "MobStreamOpenResult",
    "MobStreamCloseParams",
    "MobStreamCloseResult",
]

COMMS_SESSION_STREAM_RPC_CONTRACT_TYPES = [
    "BridgeAck",
    "BridgeBindPayload",
    "BridgeBindResponse",
    "BridgeCapabilities",
    "BridgeDeliveryPayload",
    "BridgeDeliveryResponse",
    "BridgeDestroyResponse",
    "BridgeHardCancelPayload",
    "BridgeObservationResponse",
    "BridgePeerSpec",
    "BridgePeerWiringPayload",
    "BridgeRetireResponse",
    "BridgeSupervisorPayload",
    "CommsChecksumTokenParams",
    "CommsChecksumTokenResult",
    "CommsPeerLifecycleParams",
    "CommsPeersParams",
    "PeerAddress",
    "PeerCapabilitySet",
    "PeerDirectoryEntry",
    "CommsPeersResult",
    "SessionStreamOpenParams",
    "SessionStreamOpenResult",
    "SessionStreamCloseParams",
    "SessionStreamCloseResult",
]

COMMS_SESSION_STREAM_RPC_CONTRACT_ALIAS_TYPES = [
    "BlobId",
    "BridgeBootstrapToken",
    "BridgeCommand",
    "BridgeDeliveryOutcome",
    "BridgeDeliveryRejectionCause",
    "BridgeMemberRuntimeState",
    "BridgePeerConnectivity",
    "BridgeProtocolVersion",
    "BridgeRejectionCause",
    "BridgeReply",
    "ContentBlock",
    "ContentInput",
    "CommsSendParams",
    "CommsSendResult",
    "CommsChecksumTokenResultIntent",
    "CommsPeerRequestIntent",
    "CommsPeerRequestParams",
    "CommsPeerResponseResult",
    "HandlingMode",
    "PeerId",
    "PeerName",
    "PeerTransport",
    "PeerDirectorySource",
    "PeerSendability",
    "PeerReachability",
    "PeerReachabilityReason",
]

MCP_LIVE_CONTRACT_TYPES = [
    "McpAddParams",
    "McpRemoveParams",
    "McpReloadParams",
    "McpLiveOpResponse",
]

MCP_CONFIG_HELPER_TYPES = [
    "McpStdioConfig",
    "McpHttpConfig",
]

MCP_CONFIG_ALIAS_TYPES = [
    "McpHttpTransport",
]

MOB_RPC_CONTRACT_ALIAS_TYPES = [
    "WireMemberRef",
    "WireMobBackendKind",
    "WireRuntimeBinding",
    "WireMemberLaunchMode",
    "WireForkContext",
    "WireToolAccessPolicy",
    "WireBudgetSplitPolicy",
    "WireToolFilter",
    "WireMemberState",
    "WireMobMemberStatus",
    "WireMobRuntimeMode",
    "MobSpawnManyFailureCause",
    "MobSpawnManyResultStatus",
    "MobSpawnManyResultPayload",
    "MobCollectionPolicyInput",
    "MobConditionExprInput",
    "MobDependencyModeInput",
    "MobDispatchModeInput",
    "MobFlowNodeInput",
    "MobPolicyModeInput",
    "MobProfileBindingInput",
    "MobSkillSourceInput",
    "MobSpawnPolicyInput",
    "MobStepOutputFormatInput",
    "WireMobReconcileStage",
]

MOB_RPC_CONTRACT_HELPER_TYPES = [
    "MobDefinitionInput",
    "MobBackendConfigInput",
    "MobEventRouterConfigInput",
    "MobExternalBackendConfigInput",
    "MobFlowSpecInput",
    "MobFlowStepInput",
    "MobFrameSpecInput",
    "MobLimitsSpecInput",
    "MobMcpServerConfigInput",
    "MobOrchestratorInput",
    "MobProfileInput",
    "MobRoleWiringRuleInput",
    "MobSupervisorSpecInput",
    "MobToolConfigInput",
    "MobTopologyRuleInput",
    "MobTopologySpecInput",
    "MobWiringRulesInput",
    "MobMemberSpecWire",
    "MobReconcileOptionsWire",
    "MobReconcileReportWire",
    "MobReconcileFailureWire",
]

MOB_RPC_PROMOTED_SCHEMA_DEFS = frozenset(
    [
        *MOB_RPC_CONTRACT_ALIAS_TYPES,
        *MOB_RPC_CONTRACT_HELPER_TYPES,
        "MobMemberListEntryWire",
        "MobSpawnReceiptWire",
        "MobSpawnSpecParams",
        "MobSpawnManyResultEntry",
        "MobSpawnManySpawnedResult",
        "MobSpawnManyFailedResult",
        "WireContentBlock",
        "WireContentInput",
        "WireConnectionRef",
        "WireMobProfile",
        "WireMobToolConfig",
    ]
)


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
    return "".join(part.capitalize() for part in re.split(r"[^0-9A-Za-z]+", name) if part)


def _schema_root_with_local_defs(root_schema: dict[str, Any], schema: dict[str, Any]) -> dict[str, Any]:
    schema_root = dict(root_schema)
    schema_root["$defs"] = {
        **root_schema.get("$defs", {}),
        **(schema.get("$defs", {}) if isinstance(schema, dict) else {}),
    }
    return schema_root


def _schema_root_with_nested_defs(root_schema: dict[str, Any]) -> dict[str, Any]:
    """Promote schema-local $defs into the lookup root.

    The Rust schema generator emits many realtime contract types as local
    `$defs` on request/result schemas. SDK generation still needs those names
    to remain authoritative public types instead of erasing them to maps.
    """
    schema_root = dict(root_schema)
    defs = dict(root_schema.get("$defs", {}))
    for schema in root_schema.values():
        if isinstance(schema, dict):
            defs.update(
                {
                    name: nested
                    for name, nested in schema.get("$defs", {}).items()
                    if _promote_nested_schema_def(name)
                }
            )
    schema_root["$defs"] = defs
    schema_root["__promoted_defs"] = set(defs)
    return schema_root


def _promote_nested_schema_def(name: str) -> bool:
    return name.startswith("Realtime") or name in {
        "AudioFormatMismatchContext",
        "ToolCallTimeoutContext",
        "WireTrustedPeerIdentity",
        "McpServerConfig",
        *MCP_CONFIG_HELPER_TYPES,
        *MCP_CONFIG_ALIAS_TYPES,
        *MOB_RPC_PROMOTED_SCHEMA_DEFS,
        *COMMS_SESSION_STREAM_RPC_CONTRACT_TYPES,
        *COMMS_SESSION_STREAM_RPC_CONTRACT_ALIAS_TYPES,
    }


def _runtime_state_result_root(wire_schema: dict[str, Any]) -> dict[str, Any]:
    root = dict(wire_schema)
    root["RuntimeStateResult"] = {
        "description": "Response payload for runtime-backed session status projections.",
        "properties": {
            "state": {
                "$ref": "#/$defs/WireRuntimeState",
            },
        },
        "required": ["state"],
        "title": "RuntimeStateResult",
        "type": "object",
    }
    return root


def _variant_type_prefix(name: str) -> str:
    return name.removesuffix("Request")


def _typed_dict_variant_name(alias_name: str, discriminator_value: str) -> str:
    return f"{_variant_type_prefix(alias_name)}{_pascal_case(discriminator_value)}"


def _const_variants(variants: list[Any]) -> list[Any] | None:
    values: list[Any] = []
    for variant in variants:
        if not isinstance(variant, dict) or "const" not in variant:
            return None
        value = variant["const"]
        if value not in values:
            values.append(value)
    return values


def _merge_ref_variant(root: dict[str, Any], variant: dict[str, Any]) -> dict[str, Any]:
    ref = variant.get("$ref")
    if not isinstance(ref, str):
        return variant
    base = _resolve_schema_ref(root, ref)
    if not base:
        ref_name = _resolve_schema_ref_name(ref)
        base = _lookup_named_schema(root, ref_name) if ref_name else {}
    if not isinstance(base, dict) or base.get("type") != "object":
        return variant

    merged = dict(base)
    merged["properties"] = {
        **(base.get("properties", {}) if isinstance(base.get("properties"), dict) else {}),
        **(variant.get("properties", {}) if isinstance(variant.get("properties"), dict) else {}),
    }
    required: list[str] = []
    for candidate in [base.get("required", []), variant.get("required", [])]:
        if isinstance(candidate, list):
            for field_name in candidate:
                if field_name not in required:
                    required.append(field_name)
    if required:
        merged["required"] = required
    merged["type"] = "object"
    return merged


def _one_of_typed_dict_variants(
    root: dict[str, Any],
    schema: Any,
) -> tuple[str, list[tuple[str, dict[str, Any]]]] | None:
    if not isinstance(schema, dict) or not isinstance(schema.get("oneOf"), list):
        return None

    discriminator: str | None = None
    variants: list[tuple[str, dict[str, Any]]] = []
    for variant in schema["oneOf"]:
        if isinstance(variant, dict):
            variant = _merge_ref_variant(root, variant)
        if not isinstance(variant, dict) or variant.get("type") != "object":
            return None
        properties = variant.get("properties")
        if not isinstance(properties, dict):
            return None
        const_fields = [
            (field_name, field_schema["const"])
            for field_name, field_schema in properties.items()
            if isinstance(field_schema, dict) and isinstance(field_schema.get("const"), str)
        ]
        if len(const_fields) != 1:
            return None
        field_name, discriminator_value = const_fields[0]
        if discriminator is None:
            discriminator = field_name
        elif discriminator != field_name:
            return None
        variants.append((discriminator_value, variant))

    if discriminator is None or not variants:
        return None
    return (discriminator, variants)


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
            const_values = _const_variants(non_null)
            if const_values is not None:
                values = ", ".join(repr(value) for value in const_values)
                return (f"Literal[{values}]", optional)
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
        if ref_name and resolved and (
            ref_name not in (local_defs or set()) or ref_name in root.get("__promoted_defs", set())
        ):
            return (ref_name, False)
        return _python_type_from_schema(root, resolved, local_defs)

    if "const" in field_schema:
        return (f"Literal[{field_schema['const']!r}]", False)

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
            if isinstance(additional, dict) and not properties:
                value_type, _ = _python_type_from_schema(root, additional, local_defs)
                return (f"dict[str, {value_type}]", optional)
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
            const_values = _const_variants(non_null)
            if const_values is not None:
                values = " | ".join(json.dumps(value) for value in const_values)
                return (values, optional)
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
        if ref_name and resolved and (
            ref_name not in (local_defs or set()) or ref_name in root.get("__promoted_defs", set())
        ):
            return (ref_name, False)
        return _typescript_type_from_schema(root, resolved, local_defs)

    if "const" in field_schema:
        return (json.dumps(field_schema["const"]), False)

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
            if "|" in item_type and not item_type.strip().startswith(("(", "{")):
                item_type = f"({item_type})"
            return (f"{item_type}[]", optional)
        case "object":
            properties = field_schema.get("properties")
            additional = field_schema.get("additionalProperties", True)
            if isinstance(additional, dict) and not properties:
                value_type, _ = _typescript_type_from_schema(root, additional, local_defs)
                return (f"Record<string, {value_type}>", optional)
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
    types_content += "from typing import Any, Literal, NotRequired, Optional, Required, TypedDict\n\n\n"
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
    types_content += "    terminal_cause_kind: Optional[str] = None\n"
    types_content += "    structured_output: Optional[Any] = None\n"
    types_content += "    schema_warnings: Optional[list[Any]] = None\n"
    types_content += "    skill_diagnostics: Optional[dict] = None\n\n\n"

    types_content += "@dataclass\nclass WireProviderMeta:\n"
    types_content += '    """Provider continuity metadata."""\n'
    types_content += "    provider: str = ''\n\n\n"

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
    types_content += "    created_at: str = ''\n"
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
        types_content += "    skill_refs: list[dict[str, str]] = field(default_factory=list)\n\n"

    params_schema = _schema_root_with_nested_defs(schemas.get("params", {}))
    wire_schema = _schema_root_with_nested_defs(schemas.get("wire-types", {}))
    runtime_state_result_root = _runtime_state_result_root(wire_schema)
    emitted_python_dataclasses: set[str] = set()

    def append_python_dataclass(name: str, root_schema: dict[str, Any], default_doc: str) -> None:
        nonlocal types_content
        if name in emitted_python_dataclasses:
            return
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

        required_lines: list[str] = []
        optional_lines: list[str] = []
        for field_name, field_schema in properties.items():
            python_field_name = _python_identifier(field_name)
            field_type, is_optional_type = _python_type_from_schema(
                schema_root,
                field_schema,
                local_defs,
            )
            is_required = field_name in required
            annotation = field_type
            if not is_required or is_optional_type:
                annotation = f"Optional[{field_type}]"
            if is_required:
                required_lines.append(f"    {python_field_name}: {annotation}\n")
            else:
                optional_lines.append(f"    {python_field_name}: {annotation} = None\n")
        for line in required_lines + optional_lines:
            types_content += line
        types_content += "\n"
        emitted_python_dataclasses.add(name)

    def append_python_contract_dataclass(name: str) -> None:
        if _lookup_named_schema(params_schema, name):
            append_python_dataclass(name, params_schema, f"Request payload for {name}.")
            return
        if _lookup_named_schema(wire_schema, name):
            append_python_dataclass(name, wire_schema, f"Wire payload for {name}.")
            return
        raise KeyError(f"schema for generated Python contract type {name} not found")

    def append_python_alias(name: str, root_schema: dict[str, Any], default_doc: str) -> None:
        nonlocal types_content
        schema = _lookup_named_schema(root_schema, name)
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = _schema_root_with_local_defs(root_schema, schema)
        typed_dict_variants = _one_of_typed_dict_variants(schema_root, schema)
        if typed_dict_variants is not None:
            doc = schema.get("description", default_doc)
            doc_lines = str(doc).splitlines() or [""]
            doc_block = "\n".join(f"# {line}" if line else "#" for line in doc_lines)
            types_content += f"\n{doc_block}\n"
            variant_names: list[str] = []
            for discriminator_value, variant in typed_dict_variants[1]:
                properties = variant.get("properties", {})
                variant_name = _typed_dict_variant_name(name, discriminator_value)
                variant_names.append(variant_name)
                required = set(variant.get("required", []))
                local_variant_defs = set(variant.get("$defs", {}).keys()) | local_defs
                types_content += f"class {variant_name}(TypedDict, total=False):\n"
                for field_name, field_schema in properties.items():
                    field_type, _ = _python_type_from_schema(
                        schema_root,
                        field_schema,
                        local_variant_defs,
                    )
                    wrapper = "Required" if field_name in required else "NotRequired"
                    types_content += (
                        f"    {_python_identifier(field_name)}: {wrapper}[{field_type}]\n"
                    )
                types_content += "\n"
            if variant_names:
                types_content += f"{name} = {' | '.join(variant_names)}\n"
                return
        alias_type, _ = _python_type_from_schema(schema_root, schema, local_defs)
        doc = schema.get("description", default_doc) if isinstance(schema, dict) else default_doc
        # Ensure multi-line descriptions are fully commented out.
        doc_lines = str(doc).splitlines() or [""]
        doc_block = "\n".join(f"# {line}" if line else "#" for line in doc_lines)
        types_content += f"\n{doc_block}\n{name} = {alias_type}\n"

    for name in MCP_CONFIG_HELPER_TYPES:
        append_python_contract_dataclass(name)
    types_content += "\nclass McpStdioServerConfig(TypedDict, total=False):\n"
    types_content += '    """Typed stdio variant for MCP server configuration."""\n'
    types_content += "    name: Required[str]\n"
    types_content += "    command: Required[str]\n"
    types_content += "    args: NotRequired[list[str]]\n"
    types_content += "    env: NotRequired[dict[str, str]]\n"
    types_content += "    connect_timeout_secs: NotRequired[int]\n\n"
    types_content += "\nclass McpHttpServerConfig(TypedDict, total=False):\n"
    types_content += '    """Typed HTTP variant for MCP server configuration."""\n'
    types_content += "    name: Required[str]\n"
    types_content += "    url: Required[str]\n"
    types_content += "    headers: NotRequired[dict[str, str]]\n"
    types_content += "    transport: NotRequired[McpHttpTransport]\n"
    types_content += "    connect_timeout_secs: NotRequired[int]\n\n"
    types_content += "\nMcpServerConfig = McpStdioServerConfig | McpHttpServerConfig\n"
    for name in MCP_LIVE_CONTRACT_TYPES:
        append_python_contract_dataclass(name)
    append_python_dataclass("MobWireParams", params_schema, "Request payload for mob/wire.")
    append_python_dataclass("MobUnwireParams", params_schema, "Request payload for mob/unwire.")
    for name in MOB_RPC_CONTRACT_TYPES:
        append_python_contract_dataclass(name)
    for name in MOB_RPC_CONTRACT_HELPER_TYPES:
        append_python_contract_dataclass(name)
    for name in COMMS_SESSION_STREAM_RPC_CONTRACT_TYPES:
        append_python_contract_dataclass(name)
    append_python_dataclass(
        "RuntimeRealtimeAttachmentStatusParams",
        params_schema,
        "Request payload for session/realtime_attachment_status.",
    )
    append_python_dataclass("RealtimeOpenRequest", params_schema, "Request payload for realtime/open_info.")
    append_python_dataclass("RealtimeStatusParams", params_schema, "Request payload for realtime/status.")
    append_python_dataclass("RealtimeCapabilitiesParams", params_schema, "Request payload for realtime/capabilities.")
    append_python_dataclass("ScheduleIdParams", params_schema, "Request payload for schedule id lookups.")
    append_python_dataclass("ListSchedulesParams", params_schema, "Request payload for schedule/list.")
    append_python_dataclass("ScheduleOccurrencesParams", params_schema, "Request payload for schedule/occurrences.")
    append_python_dataclass("UpdateScheduleParams", params_schema, "Request payload for schedule/update.")
    append_python_dataclass("WireRenderMetadata", wire_schema, "Render metadata for mob member delivery.")
    append_python_alias("WireTrustedPeerIdentity", wire_schema, "Typed external peer identity.")
    append_python_dataclass("WireTrustedPeerSpec", wire_schema, "Minimal trusted peer spec for mob wiring.")
    append_python_dataclass("MobWireResult", wire_schema, "Response payload for mob/wire.")
    append_python_dataclass("MobUnwireResult", wire_schema, "Response payload for mob/unwire.")
    append_python_dataclass(
        "RuntimeStateResult",
        runtime_state_result_root,
        "Response payload for runtime-backed session status projections.",
    )
    append_python_dataclass(
        "RuntimeRealtimeAttachmentStatusResult",
        wire_schema,
        "Response payload for session/realtime_attachment_status.",
    )
    append_python_dataclass("RealtimeReconnectPolicy", wire_schema, "Reconnect policy for realtime channels.")
    append_python_dataclass("RealtimeChannelConfig", params_schema, "Runtime knobs for a realtime channel.")
    append_python_dataclass("RealtimeAudioFormat", wire_schema, "Realtime audio format descriptor.")
    append_python_dataclass("RealtimeCapabilities", wire_schema, "Capability set for a realtime channel.")
    append_python_dataclass("RealtimeChannelStatus", wire_schema, "Public realtime channel status projection.")
    append_python_dataclass("RealtimeOpenInfo", wire_schema, "Response payload for realtime/open_info.")
    append_python_dataclass("RealtimeStatusResult", wire_schema, "Response payload for realtime/status.")
    append_python_dataclass("RealtimeCapabilitiesResult", wire_schema, "Response payload for realtime/capabilities.")
    append_python_dataclass("RealtimeTextChunk", wire_schema, "Text chunk for realtime ingress.")
    append_python_dataclass("RealtimeTextDelta", wire_schema, "Text delta for realtime output.")
    append_python_dataclass("RealtimeAudioChunk", wire_schema, "Opaque realtime audio chunk.")
    append_python_dataclass("RealtimeVideoChunk", wire_schema, "Opaque realtime video chunk.")
    append_python_dataclass("RealtimeBargeInTruncateFrame", wire_schema, "Payload for channel.barge_in_truncate.")
    append_python_dataclass("AudioFormatMismatchContext", wire_schema, "Typed context for audio format mismatch errors.")
    append_python_dataclass("ToolCallTimeoutContext", wire_schema, "Typed context for tool timeout errors.")
    append_python_dataclass("RealtimeChannelOpenFrame", wire_schema, "Payload for channel.open.")
    append_python_dataclass("RealtimeChannelInputFrame", wire_schema, "Payload for channel.input.")
    append_python_dataclass("RealtimeChannelOpenedFrame", wire_schema, "Payload for channel.opened.")
    append_python_dataclass("RealtimeChannelStatusFrame", wire_schema, "Payload for channel.status.")
    append_python_dataclass("RealtimeChannelEventFrame", wire_schema, "Payload for channel.event.")
    append_python_dataclass("RealtimeChannelErrorFrame", wire_schema, "Payload for channel.error.")
    append_python_dataclass("RealtimeChannelClosedFrame", wire_schema, "Payload for channel.closed.")
    append_python_dataclass(
        "RuntimeAcceptResult",
        wire_schema,
        "Response payload for runtime-backed input submission.",
    )
    append_python_dataclass("WireInputStateHistoryEntry", wire_schema, "Input transition history entry.")
    append_python_dataclass("WireInputState", wire_schema, "Runtime input state snapshot.")
    append_python_dataclass("ScheduleListResult", wire_schema, "Response payload for schedule/list.")
    append_python_dataclass("ScheduleOccurrencesResult", wire_schema, "Response payload for schedule/occurrences.")
    append_python_dataclass("WireSessionInfo", wire_schema, "Detailed session metadata payload.")
    append_python_dataclass("WireSessionSummary", wire_schema, "Session summary payload returned by session/list.")
    append_python_dataclass("ContractVersion", schemas.get("models", {}), "Semantic contract version triple.")
    append_python_dataclass("CatalogModelEntry", schemas.get("models", {}), "Catalog model entry.")
    append_python_dataclass("ProviderCatalog", schemas.get("models", {}), "Provider grouping in the model catalog.")
    append_python_dataclass("ModelsCatalogResponse", schemas.get("models", {}), "Response payload for models/catalog.")
    append_python_dataclass("WireModelProfile", schemas.get("models", {}), "Wire-level model capability profile.")
    append_python_dataclass("WireAssistantImageRef", wire_schema, "Generated assistant image reference.")
    append_python_dataclass("WireGenerateImageRequest", wire_schema, "Canonical generate_image request payload.")
    append_python_dataclass("WireGenerateImageExecutionPlan", wire_schema, "Provider-owned image generation execution plan.")
    append_python_dataclass("WireImageGenerationToolResult", wire_schema, "Canonical generate_image tool result payload.")

    # Phase 4c — connection/auth wire types.
    append_python_dataclass("WireConnectionRef", wire_schema, "Session-facing reference to a binding inside a realm.")
    append_python_dataclass("WireBackendProfile", wire_schema, "Backend profile projected for the wire.")
    append_python_dataclass("WireAuthProfile", wire_schema, "Auth profile projected for the wire (secret material not included).")
    append_python_dataclass("WireProviderBinding", wire_schema, "Provider binding: backend+auth pair plus policy.")
    append_python_dataclass("WireRealmConnectionSet", wire_schema, "Full realm connection set (backends, auth profiles, bindings).")
    append_python_dataclass("WireBindingIdentity", wire_schema, "Binding identity returned by auth response envelopes.")
    append_python_dataclass("WireAuthProfileCreated", wire_schema, "Response payload for auth/profile/create.")
    append_python_dataclass("WireAuthProfileDetail", wire_schema, "Response payload for auth/profile/get.")
    append_python_dataclass("WireAuthProfileCleared", wire_schema, "Response payload for auth/profile/delete and auth/logout.")
    append_python_dataclass("WireLoginStart", wire_schema, "Response payload for auth/login/start.")
    append_python_dataclass("WireLoginReady", wire_schema, "Ready response payload for auth login completion.")
    append_python_dataclass("WireDeviceStart", wire_schema, "Response payload for auth/login/device_start.")
    append_python_dataclass("WireRealmSummary", wire_schema, "Realm summary returned by realm/list.")
    append_python_dataclass("WireRealmList", wire_schema, "Response payload for realm/list.")
    append_python_dataclass("WireAuthProfilesList", wire_schema, "Response payload for auth/profile/list.")
    append_python_dataclass("WireAuthStatus", wire_schema, "Auth profile health snapshot.")
    append_python_dataclass("WireAuthStatusDetail", wire_schema, "Binding-scoped auth profile health snapshot.")
    append_python_alias("WireAuthError", wire_schema, "Auth error (tagged variant).")

    # Keep aliases after the dataclasses they reference. Unlike annotations, alias
    # assignments are evaluated eagerly at import time.
    append_python_alias("WireContentBlock", wire_schema, "Wire-safe content block.")
    append_python_alias("WireContentInput", wire_schema, "Wire-safe content input.")
    for name in MOB_RPC_CONTRACT_ALIAS_TYPES:
        append_python_alias(name, wire_schema, f"Mob RPC helper wire type for {name}.")
    append_python_alias("McpLiveOperation", wire_schema, "Shared operation kind for live MCP operations.")
    append_python_alias("McpLiveOpStatus", wire_schema, "Shared status for live MCP operations.")
    for name in MCP_CONFIG_ALIAS_TYPES:
        root_schema = params_schema if _lookup_named_schema(params_schema, name) else wire_schema
        append_python_alias(name, root_schema, f"MCP config alias {name}.")
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
    append_python_alias("RealtimeProtocolVersion", wire_schema, "Realtime protocol version.")
    append_python_alias("RealtimeInputKind", wire_schema, "Realtime input kind.")
    append_python_alias("RealtimeOutputKind", wire_schema, "Realtime output kind.")
    append_python_alias("RealtimeChannelState", wire_schema, "Realtime channel lifecycle state.")
    append_python_alias("RealtimeErrorCode", wire_schema, "Realtime error code.")
    append_python_alias("RealtimeErrorDetails", wire_schema, "Realtime error details.")
    append_python_alias("RealtimeInputChunk", wire_schema, "Realtime input chunk union.")
    append_python_alias("RealtimeOutputChunk", wire_schema, "Realtime output chunk union.")
    append_python_alias("RealtimeEvent", wire_schema, "Realtime event union.")
    append_python_alias("RealtimeClientFrame", wire_schema, "Realtime client frame union.")
    append_python_alias("RealtimeServerFrame", wire_schema, "Realtime server frame union.")
    append_python_alias(
        "RuntimeAcceptOutcomeType",
        wire_schema,
        "Discriminator for runtime-backed input submission responses.",
    )
    append_python_alias("WireInputLifecycleState", wire_schema, "Public input lifecycle state projection used by RPC surfaces.")
    append_python_alias("WireStopReason", wire_schema, "Canonical stop reason for transcript messages.")
    append_python_alias("WireToolResultContent", wire_schema, "Wire-safe tool result content.")
    append_python_alias("WireAssistantBlock", wire_schema, "Block assistant transcript item.")
    append_python_alias("WireImageOperationPhase", wire_schema, "Machine-owned image operation phase.")
    append_python_alias("WireModelTier", schemas.get("models", {}), "Wire-level model recommendation tier.")
    append_python_alias(
        "CommsCommandRequest",
        wire_schema,
        "Typed comms/send command (serde-tagged on `kind`).",
    )
    for name in COMMS_SESSION_STREAM_RPC_CONTRACT_ALIAS_TYPES:
        root_schema = params_schema if _lookup_named_schema(params_schema, name) else wire_schema
        append_python_alias(name, root_schema, f"Comms/session-stream RPC contract for {name}.")
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
    types_content += "  terminal_cause_kind?: string;\n"
    types_content += "  structured_output?: unknown;\n"
    types_content += "  schema_warnings?: Array<{ provider: string; path: string; message: string }>;\n"
    types_content += "}\n\n"

    types_content += "export interface WireProviderMeta {\n"
    types_content += "  provider: string;\n"
    types_content += "  [key: string]: unknown;\n"
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
    types_content += "  created_at: string;\n"
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
        types_content += "  skill_refs: Array<{ source_uuid: string; skill_name: string }>;\n"
        types_content += "}\n"

    params_schema = _schema_root_with_nested_defs(schemas.get("params", {}))
    wire_schema = _schema_root_with_nested_defs(schemas.get("wire-types", {}))
    runtime_state_result_root = _runtime_state_result_root(wire_schema)
    emitted_typescript_interfaces: set[str] = set()

    def append_typescript_interface(name: str, root_schema: dict[str, Any]) -> None:
        nonlocal types_content
        if name in emitted_typescript_interfaces:
            return
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
        emitted_typescript_interfaces.add(name)

    def append_typescript_contract_interface(name: str) -> None:
        if _lookup_named_schema(params_schema, name):
            append_typescript_interface(name, params_schema)
            return
        if _lookup_named_schema(wire_schema, name):
            append_typescript_interface(name, wire_schema)
            return
        raise KeyError(f"schema for generated TypeScript contract type {name} not found")

    def append_typescript_alias(name: str, root_schema: dict[str, Any]) -> None:
        nonlocal types_content
        schema = _lookup_named_schema(root_schema, name)
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = _schema_root_with_local_defs(root_schema, schema)
        typed_dict_variants = _one_of_typed_dict_variants(schema_root, schema)
        if typed_dict_variants is not None:
            variant_names: list[str] = []
            for discriminator_value, variant in typed_dict_variants[1]:
                properties = variant.get("properties", {})
                variant_name = _typed_dict_variant_name(name, discriminator_value)
                variant_names.append(variant_name)
                required = set(variant.get("required", []))
                local_variant_defs = set(variant.get("$defs", {}).keys()) | local_defs
                types_content += f"\nexport interface {variant_name} {{\n"
                for field_name, field_schema in properties.items():
                    field_type, _ = _typescript_type_from_schema(
                        schema_root,
                        field_schema,
                        local_variant_defs,
                    )
                    optional = "" if field_name in required else "?"
                    types_content += f"  {field_name}{optional}: {field_type};\n"
                types_content += "}\n"
            if variant_names:
                types_content += f"\nexport type {name} = {' | '.join(variant_names)};\n"
                return
        alias_type, _ = _typescript_type_from_schema(schema_root, schema, local_defs)
        types_content += f"\nexport type {name} = {alias_type};\n"

    for name in MCP_CONFIG_HELPER_TYPES:
        append_typescript_contract_interface(name)
    types_content += (
        "\nexport type McpServerConfig =\n"
        "  | ({ name: string; connect_timeout_secs?: number } & McpStdioConfig)\n"
        "  | ({ name: string; connect_timeout_secs?: number } & McpHttpConfig);\n"
    )
    for name in MCP_LIVE_CONTRACT_TYPES:
        append_typescript_contract_interface(name)
    append_typescript_interface("MobWireParams", params_schema)
    append_typescript_interface("MobUnwireParams", params_schema)
    for name in MOB_RPC_CONTRACT_TYPES:
        append_typescript_contract_interface(name)
    for name in MOB_RPC_CONTRACT_HELPER_TYPES:
        append_typescript_contract_interface(name)
    for name in COMMS_SESSION_STREAM_RPC_CONTRACT_TYPES:
        append_typescript_contract_interface(name)
    append_typescript_interface("RuntimeRealtimeAttachmentStatusParams", params_schema)
    append_typescript_interface("RealtimeOpenRequest", params_schema)
    append_typescript_interface("RealtimeStatusParams", params_schema)
    append_typescript_interface("RealtimeCapabilitiesParams", params_schema)
    append_typescript_interface("ScheduleIdParams", params_schema)
    append_typescript_interface("ListSchedulesParams", params_schema)
    append_typescript_interface("ScheduleOccurrencesParams", params_schema)
    append_typescript_interface("UpdateScheduleParams", params_schema)
    append_typescript_alias("WireContentBlock", wire_schema)
    append_typescript_alias("WireContentInput", wire_schema)
    for name in MOB_RPC_CONTRACT_ALIAS_TYPES:
        append_typescript_alias(name, wire_schema)
    append_typescript_alias("McpLiveOperation", wire_schema)
    append_typescript_alias("McpLiveOpStatus", wire_schema)
    for name in MCP_CONFIG_ALIAS_TYPES:
        root_schema = params_schema if _lookup_named_schema(params_schema, name) else wire_schema
        append_typescript_alias(name, root_schema)
    append_typescript_alias("MobPeerTarget", wire_schema)
    append_typescript_alias("WireHandlingMode", wire_schema)
    append_typescript_alias("WireRenderClass", wire_schema)
    append_typescript_alias("WireRenderSalience", wire_schema)
    append_typescript_alias("WireRuntimeState", wire_schema)
    append_typescript_alias("WireRealtimeAttachmentStatus", wire_schema)
    append_typescript_alias("RealtimeChannelTarget", wire_schema)
    append_typescript_alias("RealtimeChannelRole", wire_schema)
    append_typescript_alias("RealtimeTurningMode", wire_schema)
    append_typescript_alias("RealtimeProtocolVersion", wire_schema)
    append_typescript_alias("RealtimeInputKind", wire_schema)
    append_typescript_alias("RealtimeOutputKind", wire_schema)
    append_typescript_alias("RealtimeChannelState", wire_schema)
    append_typescript_alias("RealtimeErrorCode", wire_schema)
    append_typescript_alias("RealtimeErrorDetails", wire_schema)
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
    for name in COMMS_SESSION_STREAM_RPC_CONTRACT_ALIAS_TYPES:
        root_schema = params_schema if _lookup_named_schema(params_schema, name) else wire_schema
        append_typescript_alias(name, root_schema)
    append_typescript_interface("WireRenderMetadata", wire_schema)
    append_typescript_alias("WireTrustedPeerIdentity", wire_schema)
    append_typescript_interface("WireTrustedPeerSpec", wire_schema)
    append_typescript_interface("MobWireResult", wire_schema)
    append_typescript_interface("MobUnwireResult", wire_schema)
    append_typescript_interface("RuntimeStateResult", runtime_state_result_root)
    append_typescript_interface("RuntimeRealtimeAttachmentStatusResult", wire_schema)
    append_typescript_interface("RealtimeReconnectPolicy", wire_schema)
    append_typescript_interface("RealtimeChannelConfig", params_schema)
    append_typescript_interface("RealtimeAudioFormat", wire_schema)
    append_typescript_interface("RealtimeCapabilities", wire_schema)
    append_typescript_interface("RealtimeChannelStatus", wire_schema)
    append_typescript_interface("RealtimeOpenInfo", wire_schema)
    append_typescript_interface("RealtimeStatusResult", wire_schema)
    append_typescript_interface("RealtimeCapabilitiesResult", wire_schema)
    append_typescript_interface("RealtimeTextChunk", wire_schema)
    append_typescript_interface("RealtimeTextDelta", wire_schema)
    append_typescript_interface("RealtimeAudioChunk", wire_schema)
    append_typescript_interface("RealtimeVideoChunk", wire_schema)
    append_typescript_interface("RealtimeBargeInTruncateFrame", wire_schema)
    append_typescript_interface("AudioFormatMismatchContext", wire_schema)
    append_typescript_interface("ToolCallTimeoutContext", wire_schema)
    append_typescript_interface("RealtimeChannelOpenFrame", wire_schema)
    append_typescript_interface("RealtimeChannelInputFrame", wire_schema)
    append_typescript_interface("RealtimeChannelOpenedFrame", wire_schema)
    append_typescript_interface("RealtimeChannelStatusFrame", wire_schema)
    append_typescript_interface("RealtimeChannelEventFrame", wire_schema)
    append_typescript_interface("RealtimeChannelErrorFrame", wire_schema)
    append_typescript_interface("RealtimeChannelClosedFrame", wire_schema)
    append_typescript_interface("RuntimeAcceptResult", wire_schema)
    append_typescript_interface("WireInputStateHistoryEntry", wire_schema)
    append_typescript_interface("WireInputState", wire_schema)
    append_typescript_interface("ScheduleListResult", wire_schema)
    append_typescript_interface("ScheduleOccurrencesResult", wire_schema)
    append_typescript_interface("WireSessionInfo", wire_schema)
    append_typescript_interface("WireSessionSummary", wire_schema)
    append_typescript_interface("ContractVersion", schemas.get("models", {}))
    append_typescript_interface("CatalogModelEntry", schemas.get("models", {}))
    append_typescript_interface("ProviderCatalog", schemas.get("models", {}))
    append_typescript_interface("ModelsCatalogResponse", schemas.get("models", {}))
    append_typescript_interface("WireModelProfile", schemas.get("models", {}))
    append_typescript_interface("WireAssistantImageRef", wire_schema)
    append_typescript_interface("WireGenerateImageRequest", wire_schema)
    append_typescript_interface("WireGenerateImageExecutionPlan", wire_schema)
    append_typescript_interface("WireImageGenerationToolResult", wire_schema)

    # Phase 4c — connection/auth wire types.
    append_typescript_interface("WireConnectionRef", wire_schema)
    append_typescript_interface("WireBackendProfile", wire_schema)
    append_typescript_interface("WireAuthProfile", wire_schema)
    append_typescript_interface("WireProviderBinding", wire_schema)
    append_typescript_interface("WireRealmConnectionSet", wire_schema)
    append_typescript_interface("WireBindingIdentity", wire_schema)
    append_typescript_interface("WireAuthProfileCreated", wire_schema)
    append_typescript_interface("WireAuthProfileDetail", wire_schema)
    append_typescript_interface("WireAuthProfileCleared", wire_schema)
    append_typescript_interface("WireLoginStart", wire_schema)
    append_typescript_interface("WireLoginReady", wire_schema)
    append_typescript_interface("WireDeviceStart", wire_schema)
    append_typescript_interface("WireRealmSummary", wire_schema)
    append_typescript_interface("WireRealmList", wire_schema)
    append_typescript_interface("WireAuthProfilesList", wire_schema)
    append_typescript_interface("WireAuthStatus", wire_schema)
    append_typescript_interface("WireAuthStatusDetail", wire_schema)
    append_typescript_alias("WireAuthError", wire_schema)
    append_typescript_alias("WireAssistantBlock", wire_schema)
    append_typescript_alias("WireImageOperationPhase", wire_schema)

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
    if schema_type == "null":
        return "null"
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


def _contract_string_list(source: dict[str, Any], key: str) -> list[str]:
    values = source.get(key)
    if not isinstance(values, list) or not all(isinstance(value, str) for value in values):
        raise KeyError(f"auth connection contract `{key}` must be a string array")
    return values


def _contract_string_map(
    source: dict[str, Any],
    key: str,
    required_keys: list[str],
) -> dict[str, list[str]]:
    values = source.get(key)
    if not isinstance(values, dict):
        raise KeyError(f"auth connection contract `{key}` must be a string array map")
    missing = [provider for provider in required_keys if provider not in values]
    extra = sorted(provider for provider in values if provider not in required_keys)
    if missing or extra:
        raise KeyError(
            f"auth connection contract `{key}` provider keys mismatch: "
            f"missing={missing}, extra={extra}"
        )
    result: dict[str, list[str]] = {}
    for provider in required_keys:
        provider_values = values[provider]
        if not isinstance(provider_values, list) or not all(
            isinstance(value, str) for value in provider_values
        ):
            raise KeyError(
                f"auth connection contract `{key}.{provider}` must be a string array"
            )
        result[provider] = provider_values
    return result


def _ts_const_array(name: str, values: list[str]) -> list[str]:
    rendered = ",\n  ".join(json.dumps(value) for value in values)
    return [
        f"export const {name} = [",
        f"  {rendered}",
        "] as const;",
        "",
    ]


def _ts_const_array_record(
    name: str,
    values: dict[str, list[str]],
    satisfies: str,
) -> list[str]:
    lines = [f"export const {name} = {{"]
    for key, entries in values.items():
        lines.append(f"  {key}: [")
        for entry in entries:
            lines.append(f"    {json.dumps(entry)},")
        lines.append("  ],")
    lines.append(f"}} as const satisfies {satisfies};")
    lines.append("")
    return lines


def _auth_rpc_method_key(method_name: str) -> str:
    parts = method_name.removeprefix("auth/").split("/")
    return parts[0] + "".join(_pascal_case(part) for part in parts[1:])


def _auth_rpc_methods(schemas: dict[str, Any]) -> dict[str, str]:
    expected = [
        "auth/profile/list",
        "auth/profile/get",
        "auth/profile/create",
        "auth/profile/delete",
        "auth/login/start",
        "auth/login/complete",
        "auth/login/device_start",
        "auth/login/device_complete",
        "auth/login/provision_api_key",
        "auth/status/get",
        "auth/logout",
    ]
    methods = schemas.get("rpc-methods", {}).get("methods", [])
    if not isinstance(methods, list):
        raise KeyError("rpc-methods.json must contain a methods array")
    available = {
        method.get("name")
        for method in methods
        if isinstance(method, dict) and isinstance(method.get("name"), str)
    }
    missing = [name for name in expected if name not in available]
    if missing:
        raise KeyError(f"rpc-methods.json is missing auth methods: {missing}")
    return {_auth_rpc_method_key(name): name for name in expected}


def generate_web_auth_types(schemas: dict, output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    contracts = schemas.get("auth-connection-contracts", {})
    wire_schema = schemas.get("wire-types", {})
    params_schema = schemas.get("params", {})
    required_wire_types = [
        "WireConnectionRef",
        "WireBackendProfile",
        "WireAuthProfile",
        "WireProviderBinding",
        "WireRealmConnectionSet",
        "WireBindingIdentity",
        "WireAuthProfileCreated",
        "WireAuthProfileDetail",
        "WireAuthProfileCleared",
        "WireLoginStart",
        "WireLoginReady",
        "WireDeviceStart",
        "WireDeviceCompleteResult",
        "WireProvisionApiKeyResult",
        "WireRealmSummary",
        "WireRealmList",
        "WireAuthProfilesList",
        "WireAuthStatus",
        "WireAuthStatusDetail",
        "WireAuthError",
    ]
    required_param_types = [
        "RealmIdParams",
        "BindingIdParams",
        "CreateProfileParams",
        "LoginStartParams",
        "LoginCompleteParams",
        "DeviceStartParams",
        "DeviceCompleteParams",
        "ProvisionApiKeyParams",
    ]
    missing_wire = [name for name in required_wire_types if not _lookup_named_schema(wire_schema, name)]
    missing_params = [name for name in required_param_types if not _lookup_named_schema(params_schema, name)]
    if missing_wire or missing_params:
        raise KeyError(
            "missing auth connection schemas for web codegen: "
            f"wire={missing_wire}, params={missing_params}"
        )
    providers = _contract_string_list(contracts, "providers")
    backend_kinds = _contract_string_list(contracts, "backend_kinds")
    auth_methods = _contract_string_list(contracts, "auth_methods")
    provider_backend_kinds = _contract_string_map(
        contracts,
        "provider_backend_kinds",
        providers,
    )
    provider_auth_methods = _contract_string_map(
        contracts,
        "provider_auth_methods",
        providers,
    )
    source_kinds = _contract_string_list(contracts, "credential_source_kinds")
    status_states = _contract_string_list(contracts, "auth_status_states")
    device_states = _contract_string_list(contracts, "device_complete_states")
    ready_state = contracts.get("login_ready_state")
    if not isinstance(ready_state, str):
        raise KeyError("auth connection contract `login_ready_state` must be a string")
    rpc_methods = _auth_rpc_methods(schemas)

    lines: list[str] = [
        "// Generated auth/connection wire contracts for @rkat/web",
        "// Sources: artifacts/schemas/rpc-methods.json, wire-types.json, auth-connection-contracts.json",
        "",
    ]

    lines.extend(_ts_const_array("WIRE_AUTH_PROVIDERS", providers))
    lines.append("export type WireAuthProvider = typeof WIRE_AUTH_PROVIDERS[number];")
    lines.append("")
    lines.extend(_ts_const_array("WIRE_BACKEND_KINDS", backend_kinds))
    lines.append("export type WireBackendKind = typeof WIRE_BACKEND_KINDS[number];")
    lines.append("")
    lines.extend(_ts_const_array("WIRE_AUTH_METHODS", auth_methods))
    lines.append("export type WireAuthMethod = typeof WIRE_AUTH_METHODS[number];")
    lines.append("")
    lines.extend(
        _ts_const_array_record(
            "WIRE_PROVIDER_BACKEND_KINDS",
            provider_backend_kinds,
            "Record<WireAuthProvider, readonly WireBackendKind[]>",
        )
    )
    lines.extend(
        _ts_const_array_record(
            "WIRE_PROVIDER_AUTH_METHODS",
            provider_auth_methods,
            "Record<WireAuthProvider, readonly WireAuthMethod[]>",
        )
    )
    lines.extend(_ts_const_array("WIRE_CREDENTIAL_SOURCE_KINDS", source_kinds))
    lines.append(
        "export type WireCredentialSourceKind = typeof WIRE_CREDENTIAL_SOURCE_KINDS[number];"
    )
    lines.append("")
    lines.extend(_ts_const_array("WIRE_AUTH_STATUS_STATES", status_states))
    lines.append("export type WireAuthStatusState = typeof WIRE_AUTH_STATUS_STATES[number];")
    lines.append("")
    lines.extend(_ts_const_array("WIRE_DEVICE_COMPLETE_STATES", device_states))
    lines.append(
        "export type WireDeviceCompleteState = typeof WIRE_DEVICE_COMPLETE_STATES[number];"
    )
    lines.append("")
    lines.append(f"export const WIRE_LOGIN_READY_STATE = {json.dumps(ready_state)} as const;")
    lines.append("")

    lines.append("export const AUTH_RPC_METHODS = {")
    for key, method in rpc_methods.items():
        lines.append(f"  {key}: {json.dumps(method)},")
    lines.append("} as const;")
    lines.append("")
    lines.append("export type AuthRpcMethod = typeof AUTH_RPC_METHODS[keyof typeof AUTH_RPC_METHODS];")
    lines.append("")

    lines.extend(
        [
            "export interface WireConnectionRef {",
            "  realm: string;",
            "  binding: string;",
            "  profile?: string | null;",
            "}",
            "",
            "export interface RealmIdParams {",
            "  realm_id: string;",
            "}",
            "",
            "export interface BindingIdParams {",
            "  realm_id: string;",
            "  binding_id: string;",
            "  profile_id?: string | null;",
            "}",
            "",
            "export interface CreateProfileParams extends BindingIdParams {",
            "  auth_method: WireAuthMethod;",
            "  secret: string;",
            "}",
            "",
            "export interface LoginStartParams extends BindingIdParams {",
            "  provider: WireAuthProvider;",
            "  redirect_uri: string;",
            "}",
            "",
            "export interface LoginCompleteParams extends BindingIdParams {",
            "  provider: WireAuthProvider;",
            "  code: string;",
            "  state: string;",
            "  redirect_uri: string;",
            "}",
            "",
            "export interface DeviceStartParams extends BindingIdParams {",
            "  provider: WireAuthProvider;",
            "}",
            "",
            "export interface DeviceCompleteParams extends BindingIdParams {",
            "  provider: WireAuthProvider;",
            "  device_code: string;",
            "}",
            "",
            "export interface ProvisionApiKeyParams {",
            "  access_token: string;",
            "  realm_id?: string | null;",
            "  binding_id?: string | null;",
            "  profile_id?: string | null;",
            "}",
            "",
            "export interface WireBackendProfile {",
            "  id: string;",
            "  provider: WireAuthProvider;",
            "  backend_kind: WireBackendKind;",
            "  base_url?: string | null;",
            "  options?: unknown;",
            "}",
            "",
            "export interface WireAuthProfile {",
            "  id: string;",
            "  provider: WireAuthProvider;",
            "  auth_method: WireAuthMethod;",
            "  source_kind: WireCredentialSourceKind;",
            "}",
            "",
            "export interface WireProviderBinding {",
            "  id: string;",
            "  backend_profile: string;",
            "  auth_profile: string;",
            "  default_model?: string | null;",
            "  allow_auth_override?: boolean;",
            "  require_metadata_account?: boolean;",
            "  require_metadata_workspace?: boolean;",
            "}",
            "",
            "export interface WireRealmConnectionSet {",
            "  realm_id: string;",
            "  backends: Record<string, WireBackendProfile>;",
            "  auth_profiles: Record<string, WireAuthProfile>;",
            "  bindings: Record<string, WireProviderBinding>;",
            "  default_binding?: string | null;",
            "}",
            "",
            "export interface WireBindingIdentity {",
            "  realm_id: string;",
            "  binding_id: string;",
            "  connection_ref: WireConnectionRef;",
            "}",
            "",
            "export interface WireAuthProfileCreated extends WireBindingIdentity {",
            "  profile_id: string;",
            "  provider: WireAuthProvider;",
            "  auth_method: WireAuthMethod;",
            "  stored: boolean;",
            "}",
            "",
            "export interface WireAuthProfileDetail {",
            "  connection_ref: WireConnectionRef;",
            "  binding_id: string;",
            "  profile_id: string;",
            "  auth_profile: WireAuthProfile;",
            "}",
            "",
            "export interface WireAuthProfileCleared extends WireBindingIdentity {",
            "  profile_id: string;",
            "  cleared: boolean;",
            "}",
            "",
            "export interface WireLoginStart {",
            "  authorize_url: string;",
            "  state: string;",
            "  redirect_uri: string;",
            "  provider: WireAuthProvider;",
            "}",
            "",
            "export interface WireLoginReady extends WireBindingIdentity {",
            "  state?: typeof WIRE_LOGIN_READY_STATE | null;",
            "  profile_id: string;",
            "  provider: WireAuthProvider;",
            "  expires_at?: string | null;",
            "  has_refresh_token: boolean;",
            "  scopes: string[];",
            "}",
            "",
            "export interface WireDeviceStart {",
            "  device_code: string;",
            "  user_code: string;",
            "  verification_uri: string;",
            "  verification_uri_complete?: string | null;",
            "  expires_in: number;",
            "  interval: number;",
            "  provider: WireAuthProvider;",
            "}",
            "",
            "export type WireDeviceCompletePending = { state: \"pending\" };",
            "export type WireDeviceCompleteSlowDown = { state: \"slow_down\" };",
            "export type WireDeviceCompleteAccessDenied = { state: \"access_denied\" };",
            "export type WireDeviceCompleteExpired = { state: \"expired\" };",
            "export type WireDeviceCompleteReady = WireLoginReady & { state: typeof WIRE_LOGIN_READY_STATE };",
            "export type WireDeviceCompleteResult =",
            "  | WireDeviceCompletePending",
            "  | WireDeviceCompleteSlowDown",
            "  | WireDeviceCompleteAccessDenied",
            "  | WireDeviceCompleteExpired",
            "  | WireDeviceCompleteReady;",
            "",
            "export interface WireProvisionApiKeyResult extends WireBindingIdentity {",
            "  profile_id: string;",
            "  provider: WireAuthProvider;",
            "  auth_mode: WireAuthMethod;",
            "  has_api_key: boolean;",
            "  scopes: string[];",
            "}",
            "",
            "export interface WireRealmSummary {",
            "  realm_id: string;",
            "  default_binding?: string | null;",
            "  backend_count: number;",
            "  auth_profile_count: number;",
            "  binding_count: number;",
            "}",
            "",
            "export interface WireRealmList {",
            "  realms: WireRealmSummary[];",
            "}",
            "",
            "export interface WireAuthProfilesList {",
            "  realm_id: string;",
            "  auth_profiles: WireAuthProfile[];",
            "  backend_profiles: WireBackendProfile[];",
            "  bindings: WireProviderBinding[];",
            "}",
            "",
            "export interface WireAuthError {",
            "  kind: string;",
            "  [key: string]: unknown;",
            "}",
            "",
            "export interface WireAuthStatus {",
            "  profile_id: string;",
            "  provider: WireAuthProvider;",
            "  auth_method: WireAuthMethod;",
            "  state: WireAuthStatusState;",
            "  expires_at?: string | null;",
            "  last_refresh_at?: string | null;",
            "  account_id?: string | null;",
            "  last_error?: WireAuthError | null;",
            "}",
            "",
            "export interface WireAuthStatusDetail extends WireBindingIdentity {",
            "  profile_id: string;",
            "  provider: WireAuthProvider;",
            "  auth_method: WireAuthMethod;",
            "  state: WireAuthStatusState;",
            "  expires_at?: string | null;",
            "  last_refresh_at?: string | null;",
            "  account_id?: string | null;",
            "  has_refresh_token: boolean;",
            "}",
            "",
            "export class WebAuthContractError extends TypeError {",
            "  constructor(message: string) {",
            "    super(message);",
            "    this.name = 'WebAuthContractError';",
            "  }",
            "}",
            "",
            "function fail(path: string, expected: string): never {",
            "  throw new WebAuthContractError(`${path}: expected ${expected}`);",
            "}",
            "",
            "function hasOwn(record: Record<string, unknown>, key: string): boolean {",
            "  return Object.prototype.hasOwnProperty.call(record, key);",
            "}",
            "",
            "function expectRecord(value: unknown, path: string): Record<string, unknown> {",
            "  if (value === null || typeof value !== 'object' || Array.isArray(value)) {",
            "    fail(path, 'object');",
            "  }",
            "  return value as Record<string, unknown>;",
            "}",
            "",
            "function expectString(value: unknown, path: string): string {",
            "  if (typeof value !== 'string') fail(path, 'string');",
            "  return value;",
            "}",
            "",
            "function expectBoolean(value: unknown, path: string): boolean {",
            "  if (typeof value !== 'boolean') fail(path, 'boolean');",
            "  return value;",
            "}",
            "",
            "function expectNumber(value: unknown, path: string): number {",
            "  if (typeof value !== 'number' || !Number.isFinite(value)) fail(path, 'finite number');",
            "  return value;",
            "}",
            "",
            "function optionalString(record: Record<string, unknown>, key: string, path: string): void {",
            "  if (hasOwn(record, key) && record[key] !== null && record[key] !== undefined) {",
            "    expectString(record[key], path);",
            "  }",
            "}",
            "",
            "function optionalBoolean(record: Record<string, unknown>, key: string, path: string): void {",
            "  if (hasOwn(record, key) && record[key] !== null && record[key] !== undefined) {",
            "    expectBoolean(record[key], path);",
            "  }",
            "}",
            "",
            "function expectStringArray(value: unknown, path: string): string[] {",
            "  if (!Array.isArray(value)) fail(path, 'string array');",
            "  value.forEach((item, index) => expectString(item, `${path}[${index}]`));",
            "  return value as string[];",
            "}",
            "",
            "function parseLiteral<T extends string>(",
            "  value: unknown,",
            "  allowed: readonly T[],",
            "  path: string,",
            "  label: string,",
            "): T {",
            "  if (typeof value !== 'string' || !(allowed as readonly string[]).includes(value)) {",
            "    fail(path, `${label} (${(allowed as readonly string[]).join(', ')})`);",
            "  }",
            "  return value as T;",
            "}",
            "",
            "function expectRecordMap<T>(",
            "  value: unknown,",
            "  path: string,",
            "  parse: (entry: unknown, entryPath: string) => T,",
            "): Record<string, T> {",
            "  const record = expectRecord(value, path);",
            "  const parsed: Record<string, T> = {};",
            "  for (const [key, entry] of Object.entries(record)) {",
            "    parsed[key] = parse(entry, `${path}.${key}`);",
            "  }",
            "  return parsed;",
            "}",
            "",
            "export function parseWireAuthProvider(value: unknown, path = 'provider'): WireAuthProvider {",
            "  return parseLiteral(value, WIRE_AUTH_PROVIDERS, path, 'wire auth provider');",
            "}",
            "",
            "export function parseWireBackendKind(value: unknown, path = 'backend_kind'): WireBackendKind {",
            "  return parseLiteral(value, WIRE_BACKEND_KINDS, path, 'wire backend kind');",
            "}",
            "",
            "export function parseWireAuthMethod(value: unknown, path = 'auth_method'): WireAuthMethod {",
            "  return parseLiteral(value, WIRE_AUTH_METHODS, path, 'wire auth method');",
            "}",
            "",
            "function validateProviderBackendKind(",
            "  provider: WireAuthProvider,",
            "  backendKind: WireBackendKind,",
            "  path: string,",
            "): void {",
            "  const allowed = WIRE_PROVIDER_BACKEND_KINDS[provider];",
            "  if (!(allowed as readonly string[]).includes(backendKind)) {",
            "    fail(path, `wire backend kind for provider ${provider} (${allowed.join(', ')})`);",
            "  }",
            "}",
            "",
            "function validateProviderAuthMethod(",
            "  provider: WireAuthProvider,",
            "  authMethod: WireAuthMethod,",
            "  path: string,",
            "): void {",
            "  const allowed = WIRE_PROVIDER_AUTH_METHODS[provider];",
            "  if (!(allowed as readonly string[]).includes(authMethod)) {",
            "    fail(path, `wire auth method for provider ${provider} (${allowed.join(', ')})`);",
            "  }",
            "}",
            "",
            "export function parseWireCredentialSourceKind(",
            "  value: unknown,",
            "  path = 'source_kind',",
            "): WireCredentialSourceKind {",
            "  return parseLiteral(value, WIRE_CREDENTIAL_SOURCE_KINDS, path, 'wire credential source kind');",
            "}",
            "",
            "export function parseWireAuthStatusState(value: unknown, path = 'state'): WireAuthStatusState {",
            "  return parseLiteral(value, WIRE_AUTH_STATUS_STATES, path, 'wire auth status state');",
            "}",
            "",
            "export function parseWireConnectionRef(value: unknown, path = 'connection_ref'): WireConnectionRef {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.realm, `${path}.realm`);",
            "  expectString(record.binding, `${path}.binding`);",
            "  optionalString(record, 'profile', `${path}.profile`);",
            "  return value as WireConnectionRef;",
            "}",
            "",
            "function validateBindingIdentity(record: Record<string, unknown>, path: string): void {",
            "  const realmId = expectString(record.realm_id, `${path}.realm_id`);",
            "  const bindingId = expectString(record.binding_id, `${path}.binding_id`);",
            "  const connectionRef = parseWireConnectionRef(record.connection_ref, `${path}.connection_ref`);",
            "  if (connectionRef.realm !== realmId) fail(`${path}.connection_ref.realm`, `same value as ${path}.realm_id`);",
            "  if (connectionRef.binding !== bindingId) fail(`${path}.connection_ref.binding`, `same value as ${path}.binding_id`);",
            "}",
            "",
            "export function parseWireBackendProfile(value: unknown, path = 'backend_profile'): WireBackendProfile {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.id, `${path}.id`);",
            "  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  const backendKind = parseWireBackendKind(record.backend_kind, `${path}.backend_kind`);",
            "  validateProviderBackendKind(provider, backendKind, `${path}.backend_kind`);",
            "  optionalString(record, 'base_url', `${path}.base_url`);",
            "  return value as WireBackendProfile;",
            "}",
            "",
            "export function parseWireAuthProfile(value: unknown, path = 'auth_profile'): WireAuthProfile {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.id, `${path}.id`);",
            "  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);",
            "  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);",
            "  parseWireCredentialSourceKind(record.source_kind, `${path}.source_kind`);",
            "  return value as WireAuthProfile;",
            "}",
            "",
            "export function parseWireProviderBinding(value: unknown, path = 'binding'): WireProviderBinding {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.id, `${path}.id`);",
            "  expectString(record.backend_profile, `${path}.backend_profile`);",
            "  expectString(record.auth_profile, `${path}.auth_profile`);",
            "  optionalString(record, 'default_model', `${path}.default_model`);",
            "  optionalBoolean(record, 'allow_auth_override', `${path}.allow_auth_override`);",
            "  optionalBoolean(record, 'require_metadata_account', `${path}.require_metadata_account`);",
            "  optionalBoolean(record, 'require_metadata_workspace', `${path}.require_metadata_workspace`);",
            "  return value as WireProviderBinding;",
            "}",
            "",
            "function indexWireProfiles<T extends { id: string }>(",
            "  profiles: readonly T[],",
            "  path: string,",
            "  label: string,",
            "): Record<string, T> {",
            "  const index: Record<string, T> = {};",
            "  profiles.forEach((profile, entryIndex) => {",
            "    if (hasOwn(index, profile.id)) fail(`${path}[${entryIndex}].id`, `unique ${label} id`);",
            "    index[profile.id] = profile;",
            "  });",
            "  return index;",
            "}",
            "",
            "function validateWireProviderBindings(",
            "  bindings: readonly (readonly [WireProviderBinding, string])[],",
            "  backends: Record<string, WireBackendProfile>,",
            "  authProfiles: Record<string, WireAuthProfile>,",
            "): void {",
            "  bindings.forEach(([binding, bindingPath]) => {",
            "    const backend = backends[binding.backend_profile];",
            "    if (backend === undefined) fail(`${bindingPath}.backend_profile`, 'known backend profile id');",
            "    const authProfile = authProfiles[binding.auth_profile];",
            "    if (authProfile === undefined) fail(`${bindingPath}.auth_profile`, 'known auth profile id');",
            "    if (backend.provider !== authProfile.provider) {",
            "      fail(",
            "        `${bindingPath}.auth_profile`,",
            "        `auth profile with provider ${backend.provider} for backend ${binding.backend_profile}`,",
            "      );",
            "    }",
            "  });",
            "}",
            "",
            "export function parseWireRealmConnectionSet(",
            "  value: unknown,",
            "  path = 'realm_connection_set',",
            "): WireRealmConnectionSet {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.realm_id, `${path}.realm_id`);",
            "  const backends = expectRecordMap(record.backends, `${path}.backends`, parseWireBackendProfile);",
            "  const authProfiles = expectRecordMap(record.auth_profiles, `${path}.auth_profiles`, parseWireAuthProfile);",
            "  const bindings = expectRecordMap(record.bindings, `${path}.bindings`, parseWireProviderBinding);",
            "  validateWireProviderBindings(",
            "    Object.entries(bindings).map(([key, binding]) => [binding, `${path}.bindings.${key}`] as const),",
            "    backends,",
            "    authProfiles,",
            "  );",
            "  optionalString(record, 'default_binding', `${path}.default_binding`);",
            "  return value as WireRealmConnectionSet;",
            "}",
            "",
            "export function parseWireAuthProfileCreated(",
            "  value: unknown,",
            "  path = 'auth_profile_created',",
            "): WireAuthProfileCreated {",
            "  const record = expectRecord(value, path);",
            "  validateBindingIdentity(record, path);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);",
            "  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);",
            "  expectBoolean(record.stored, `${path}.stored`);",
            "  return value as WireAuthProfileCreated;",
            "}",
            "",
            "export function parseWireAuthProfileDetail(",
            "  value: unknown,",
            "  path = 'auth_profile_detail',",
            "): WireAuthProfileDetail {",
            "  const record = expectRecord(value, path);",
            "  const bindingId = expectString(record.binding_id, `${path}.binding_id`);",
            "  const connectionRef = parseWireConnectionRef(record.connection_ref, `${path}.connection_ref`);",
            "  if (connectionRef.binding !== bindingId) fail(`${path}.connection_ref.binding`, `same value as ${path}.binding_id`);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  parseWireAuthProfile(record.auth_profile, `${path}.auth_profile`);",
            "  return value as WireAuthProfileDetail;",
            "}",
            "",
            "export function parseWireAuthProfileCleared(",
            "  value: unknown,",
            "  path = 'auth_profile_cleared',",
            "): WireAuthProfileCleared {",
            "  const record = expectRecord(value, path);",
            "  validateBindingIdentity(record, path);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  expectBoolean(record.cleared, `${path}.cleared`);",
            "  return value as WireAuthProfileCleared;",
            "}",
            "",
            "export function parseWireLoginStart(value: unknown, path = 'login_start'): WireLoginStart {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.authorize_url, `${path}.authorize_url`);",
            "  expectString(record.state, `${path}.state`);",
            "  expectString(record.redirect_uri, `${path}.redirect_uri`);",
            "  parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  return value as WireLoginStart;",
            "}",
            "",
            "export function parseWireLoginReady(value: unknown, path = 'login_ready'): WireLoginReady {",
            "  const record = expectRecord(value, path);",
            "  if (hasOwn(record, 'state') && record.state !== null && record.state !== undefined) {",
            "    parseLiteral(record.state, [WIRE_LOGIN_READY_STATE], `${path}.state`, 'wire login ready state');",
            "  }",
            "  validateBindingIdentity(record, path);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  optionalString(record, 'expires_at', `${path}.expires_at`);",
            "  expectBoolean(record.has_refresh_token, `${path}.has_refresh_token`);",
            "  expectStringArray(record.scopes, `${path}.scopes`);",
            "  return value as WireLoginReady;",
            "}",
            "",
            "export function parseWireDeviceStart(value: unknown, path = 'device_start'): WireDeviceStart {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.device_code, `${path}.device_code`);",
            "  expectString(record.user_code, `${path}.user_code`);",
            "  expectString(record.verification_uri, `${path}.verification_uri`);",
            "  optionalString(record, 'verification_uri_complete', `${path}.verification_uri_complete`);",
            "  expectNumber(record.expires_in, `${path}.expires_in`);",
            "  expectNumber(record.interval, `${path}.interval`);",
            "  parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  return value as WireDeviceStart;",
            "}",
            "",
            "export function parseWireDeviceCompleteResult(",
            "  value: unknown,",
            "  path = 'device_complete',",
            "): WireDeviceCompleteResult {",
            "  const record = expectRecord(value, path);",
            "  const state = parseLiteral(record.state, WIRE_DEVICE_COMPLETE_STATES, `${path}.state`, 'wire device complete state');",
            "  if (state === WIRE_LOGIN_READY_STATE) {",
            "    parseWireLoginReady(value, path);",
            "  }",
            "  return value as WireDeviceCompleteResult;",
            "}",
            "",
            "export function parseWireProvisionApiKeyResult(",
            "  value: unknown,",
            "  path = 'provision_api_key',",
            "): WireProvisionApiKeyResult {",
            "  const record = expectRecord(value, path);",
            "  validateBindingIdentity(record, path);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  const authMode = parseWireAuthMethod(record.auth_mode, `${path}.auth_mode`);",
            "  validateProviderAuthMethod(provider, authMode, `${path}.auth_mode`);",
            "  expectBoolean(record.has_api_key, `${path}.has_api_key`);",
            "  expectStringArray(record.scopes, `${path}.scopes`);",
            "  return value as WireProvisionApiKeyResult;",
            "}",
            "",
            "export function parseWireAuthProfilesList(",
            "  value: unknown,",
            "  path = 'auth_profiles_list',",
            "): WireAuthProfilesList {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.realm_id, `${path}.realm_id`);",
            "  if (!Array.isArray(record.auth_profiles)) fail(`${path}.auth_profiles`, 'array');",
            "  const authProfiles = record.auth_profiles.map((entry, index) => parseWireAuthProfile(entry, `${path}.auth_profiles[${index}]`));",
            "  if (!Array.isArray(record.backend_profiles)) fail(`${path}.backend_profiles`, 'array');",
            "  const backendProfiles = record.backend_profiles.map((entry, index) => parseWireBackendProfile(entry, `${path}.backend_profiles[${index}]`));",
            "  if (!Array.isArray(record.bindings)) fail(`${path}.bindings`, 'array');",
            "  const bindings = record.bindings.map((entry, index) => [",
            "    parseWireProviderBinding(entry, `${path}.bindings[${index}]`),",
            "    `${path}.bindings[${index}]`,",
            "  ] as const);",
            "  validateWireProviderBindings(",
            "    bindings,",
            "    indexWireProfiles(backendProfiles, `${path}.backend_profiles`, 'backend profile'),",
            "    indexWireProfiles(authProfiles, `${path}.auth_profiles`, 'auth profile'),",
            "  );",
            "  return value as WireAuthProfilesList;",
            "}",
            "",
            "export function parseWireAuthError(value: unknown, path = 'auth_error'): WireAuthError {",
            "  const record = expectRecord(value, path);",
            "  parseLiteral(",
            "    record.kind,",
            "    [",
            "      'missing_secret',",
            "      'unsupported_combination',",
            "      'missing_required_metadata',",
            "      'workspace_mismatch',",
            "      'expired',",
            "      'refresh_failed',",
            "      'interactive_login_required',",
            "      'host_owned_unavailable',",
            "      'io',",
            "      'other',",
            "    ],",
            "    `${path}.kind`,",
            "    'wire auth error kind',",
            "  );",
            "  return value as WireAuthError;",
            "}",
            "",
            "export function parseWireAuthStatus(value: unknown, path = 'auth_status'): WireAuthStatus {",
            "  const record = expectRecord(value, path);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);",
            "  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);",
            "  parseWireAuthStatusState(record.state, `${path}.state`);",
            "  optionalString(record, 'expires_at', `${path}.expires_at`);",
            "  optionalString(record, 'last_refresh_at', `${path}.last_refresh_at`);",
            "  optionalString(record, 'account_id', `${path}.account_id`);",
            "  if (hasOwn(record, 'last_error') && record.last_error !== null && record.last_error !== undefined) {",
            "    parseWireAuthError(record.last_error, `${path}.last_error`);",
            "  }",
            "  return value as WireAuthStatus;",
            "}",
            "",
            "export function parseWireAuthStatusDetail(",
            "  value: unknown,",
            "  path = 'auth_status_detail',",
            "): WireAuthStatusDetail {",
            "  const record = expectRecord(value, path);",
            "  validateBindingIdentity(record, path);",
            "  expectString(record.profile_id, `${path}.profile_id`);",
            "  const provider = parseWireAuthProvider(record.provider, `${path}.provider`);",
            "  const authMethod = parseWireAuthMethod(record.auth_method, `${path}.auth_method`);",
            "  validateProviderAuthMethod(provider, authMethod, `${path}.auth_method`);",
            "  parseWireAuthStatusState(record.state, `${path}.state`);",
            "  optionalString(record, 'expires_at', `${path}.expires_at`);",
            "  optionalString(record, 'last_refresh_at', `${path}.last_refresh_at`);",
            "  optionalString(record, 'account_id', `${path}.account_id`);",
            "  expectBoolean(record.has_refresh_token, `${path}.has_refresh_token`);",
            "  return value as WireAuthStatusDetail;",
            "}",
            "",
            "export function parseCreateProfileParams(params: CreateProfileParams): CreateProfileParams {",
            "  const record = expectRecord(params, 'create_profile.params');",
            "  expectString(record.realm_id, 'create_profile.params.realm_id');",
            "  expectString(record.binding_id, 'create_profile.params.binding_id');",
            "  optionalString(record, 'profile_id', 'create_profile.params.profile_id');",
            "  parseWireAuthMethod(record.auth_method, 'create_profile.params.auth_method');",
            "  expectString(record.secret, 'create_profile.params.secret');",
            "  return params;",
            "}",
            "",
            "export function parseLoginStartParams(params: LoginStartParams): LoginStartParams {",
            "  const record = expectRecord(params, 'login_start.params');",
            "  parseWireAuthProvider(record.provider, 'login_start.params.provider');",
            "  expectString(record.redirect_uri, 'login_start.params.redirect_uri');",
            "  expectString(record.realm_id, 'login_start.params.realm_id');",
            "  expectString(record.binding_id, 'login_start.params.binding_id');",
            "  optionalString(record, 'profile_id', 'login_start.params.profile_id');",
            "  return params;",
            "}",
            "",
            "export function parseLoginCompleteParams(params: LoginCompleteParams): LoginCompleteParams {",
            "  const record = expectRecord(params, 'login_complete.params');",
            "  parseWireAuthProvider(record.provider, 'login_complete.params.provider');",
            "  expectString(record.code, 'login_complete.params.code');",
            "  expectString(record.state, 'login_complete.params.state');",
            "  expectString(record.redirect_uri, 'login_complete.params.redirect_uri');",
            "  expectString(record.realm_id, 'login_complete.params.realm_id');",
            "  expectString(record.binding_id, 'login_complete.params.binding_id');",
            "  optionalString(record, 'profile_id', 'login_complete.params.profile_id');",
            "  return params;",
            "}",
            "",
            "export function parseDeviceStartParams(params: DeviceStartParams): DeviceStartParams {",
            "  const record = expectRecord(params, 'device_start.params');",
            "  parseWireAuthProvider(record.provider, 'device_start.params.provider');",
            "  expectString(record.realm_id, 'device_start.params.realm_id');",
            "  expectString(record.binding_id, 'device_start.params.binding_id');",
            "  optionalString(record, 'profile_id', 'device_start.params.profile_id');",
            "  return params;",
            "}",
            "",
            "export function parseDeviceCompleteParams(",
            "  params: DeviceCompleteParams,",
            "): DeviceCompleteParams {",
            "  const record = expectRecord(params, 'device_complete.params');",
            "  parseWireAuthProvider(record.provider, 'device_complete.params.provider');",
            "  expectString(record.device_code, 'device_complete.params.device_code');",
            "  expectString(record.realm_id, 'device_complete.params.realm_id');",
            "  expectString(record.binding_id, 'device_complete.params.binding_id');",
            "  optionalString(record, 'profile_id', 'device_complete.params.profile_id');",
            "  return params;",
            "}",
            "",
            "export function parseProvisionApiKeyParams(",
            "  params: ProvisionApiKeyParams,",
            "): ProvisionApiKeyParams {",
            "  const record = expectRecord(params, 'provision_api_key.params');",
            "  expectString(record.access_token, 'provision_api_key.params.access_token');",
            "  optionalString(record, 'realm_id', 'provision_api_key.params.realm_id');",
            "  optionalString(record, 'binding_id', 'provision_api_key.params.binding_id');",
            "  optionalString(record, 'profile_id', 'provision_api_key.params.profile_id');",
            "  return params;",
            "}",
            "",
        ]
    )

    (output_dir / "auth.ts").write_text("\n".join(lines))


def generate_web_mob_types(schemas: dict, output_dir: Path) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    wire_schema = _schema_root_with_nested_defs(schemas.get("wire-types", {}))
    emitted: set[str] = set()
    lines: list[str] = [
        "// Generated mob wire types for @rkat/web",
        "// Source: artifacts/schemas/wire-types.json",
        "",
    ]

    def append_interface(name: str) -> None:
        if name in emitted:
            return
        schema = _lookup_named_schema(wire_schema, name)
        if not schema:
            raise KeyError(f"schema for generated web mob type {name} not found")
        properties = schema.get("properties", {}) if isinstance(schema, dict) else {}
        required = set(schema.get("required", [])) if isinstance(schema, dict) else set()
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = _schema_root_with_local_defs(wire_schema, schema)
        lines.append(f"export interface {name} {{")
        for field_name, field_schema in properties.items():
            field_type, optional_by_type = _typescript_type_from_schema(
                schema_root,
                field_schema,
                local_defs,
            )
            optional = "?" if (field_name not in required or optional_by_type) else ""
            lines.append(f"  {field_name}{optional}: {field_type};")
        lines.append("}")
        lines.append("")
        emitted.add(name)

    def append_alias(name: str) -> None:
        if name in emitted:
            return
        schema = _lookup_named_schema(wire_schema, name)
        if not schema:
            raise KeyError(f"schema for generated web mob alias {name} not found")
        local_defs = set(schema.get("$defs", {}).keys()) if isinstance(schema, dict) else set()
        schema_root = _schema_root_with_local_defs(wire_schema, schema)
        alias_type, _ = _typescript_type_from_schema(schema_root, schema, local_defs)
        lines.append(f"export type {name} = {alias_type};")
        lines.append("")
        emitted.add(name)

    append_alias("WireMobMemberStatus")
    append_alias("WireMemberRef")
    append_interface("MobStatusResult")
    lines.append("export interface MobListResult {")
    lines.append("  mobs: MobStatusResult[];")
    lines.append("}")
    lines.append("")
    emitted.add("MobListResult")
    append_interface("MobRespawnResult")
    append_interface("MobEventsResult")
    append_interface("MobMemberSendResult")
    append_interface("MobFlowStatusResult")
    append_interface("MobHelperResult")
    append_interface("MobMemberStatusResult")
    append_interface("MobAppendSystemContextResult")

    (output_dir / "mob.ts").write_text("\n".join(lines))


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
    generate_web_mob_types(schemas, web_events_output)
    print(f"Generated web mob types in {web_events_output}")

    # Generate web auth/connection wire contracts from canonical artifacts.
    generate_web_auth_types(schemas, web_events_output)
    print(f"Generated web auth contracts in {web_events_output}")


if __name__ == "__main__":
    main()
