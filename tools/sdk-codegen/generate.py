#!/usr/bin/env python3
"""SDK code generator for Meerkat.

Reads schema artifacts from artifacts/schemas/ and generates typed client
code for Python and TypeScript SDKs.
"""

import json
import os
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


def _python_type_from_schema(root: dict[str, Any], field_schema: Any) -> tuple[str, bool]:
    """Return (python_type, optional)."""
    if field_schema is True:
        return ("Any", False)
    if field_schema is False or not isinstance(field_schema, dict):
        return ("Any", True)
    if "$ref" in field_schema:
        resolved = _resolve_schema_ref(root, str(field_schema["$ref"]))
        return _python_type_from_schema(root, resolved)

    schema_type = field_schema.get("type")
    optional = False
    if isinstance(schema_type, list):
        optional = "null" in schema_type
        non_null = [t for t in schema_type if t != "null"]
        schema_type = non_null[0] if non_null else None

    match schema_type:
        case "string":
            return ("str", optional)
        case "boolean":
            return ("bool", optional)
        case "integer":
            return ("int", optional)
        case "number":
            return ("float", optional)
        case "array":
            return ("list[Any]", optional)
        case "object":
            return ("dict[str, Any]", optional)
        case _:
            return ("Any", optional)


def _typescript_type_from_schema(root: dict[str, Any], field_schema: Any) -> tuple[str, bool]:
    """Return (typescript_type, optional)."""
    if field_schema is True:
        return ("unknown", False)
    if field_schema is False or not isinstance(field_schema, dict):
        return ("unknown", True)
    if "$ref" in field_schema:
        resolved = _resolve_schema_ref(root, str(field_schema["$ref"]))
        return _typescript_type_from_schema(root, resolved)

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
            return ("unknown[]", optional)
        case "object":
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

    types_content = f'"""Generated wire types for Meerkat SDK.\n\nContract version: {contract_version}\n"""\n\n'
    types_content += "from dataclasses import dataclass, field\n"
    types_content += "from typing import Any, Optional\n\n\n"
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
    types_content += "    schema_warnings: Optional[list] = None\n\n\n"

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
    types_content += "    capabilities: list = field(default_factory=list)\n\n"

    # Conditional params based on available capabilities
    if has_comms:
        types_content += "\n@dataclass\nclass CommsParams:\n"
        types_content += '    """Comms parameters (available because comms capability is compiled)."""\n'
        types_content += "    host_mode: bool = False\n"
        types_content += "    comms_name: Optional[str] = None\n\n"

    if has_skills:
        types_content += "\n@dataclass\nclass SkillsParams:\n"
        types_content += '    """Skills parameters (available because skills capability is compiled)."""\n'
        types_content += "    skills_enabled: bool = False\n"
        types_content += "    skill_references: list = field(default_factory=list)\n\n"

    params_schema = schemas.get("params", {})
    wire_schema = schemas.get("wire-types", {})

    def append_python_dataclass(name: str, root_schema: dict[str, Any], default_doc: str) -> None:
        nonlocal types_content
        schema = root_schema.get(name, {})
        properties = schema.get("properties", {}) if isinstance(schema, dict) else {}
        required = set(schema.get("required", [])) if isinstance(schema, dict) else set()
        doc = schema.get("description", default_doc) if isinstance(schema, dict) else default_doc
        types_content += f"\n@dataclass\nclass {name}:\n"
        types_content += f'    """{doc}"""\n'
        for field_name, field_schema in properties.items():
            field_type, is_optional_type = _python_type_from_schema(schema, field_schema)
            is_required = field_name in required
            annotation = field_type
            if is_optional_type and not is_required:
                annotation = f"Optional[{field_type}]"
            default_value = "None" if (is_optional_type and not is_required) else ("False" if field_type == "bool" else ("''" if field_type == "str" else ("0" if field_type in {"int", "float"} else "None")))
            types_content += f"    {field_name}: {annotation} = {default_value}\n"
        types_content += "\n"

    append_python_dataclass("McpAddParams", params_schema, "Request payload for mcp/add.")
    append_python_dataclass("McpRemoveParams", params_schema, "Request payload for mcp/remove.")
    append_python_dataclass("McpReloadParams", params_schema, "Request payload for mcp/reload.")
    append_python_dataclass("McpLiveOpResponse", wire_schema, "Response payload for mcp/add|remove|reload.")

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
        types_content += "  host_mode: boolean;\n"
        types_content += "  comms_name?: string;\n"
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
        schema = root_schema.get(name, {})
        properties = schema.get("properties", {}) if isinstance(schema, dict) else {}
        required = set(schema.get("required", [])) if isinstance(schema, dict) else set()
        types_content += f"\nexport interface {name} {{\n"
        for field_name, field_schema in properties.items():
            field_type, optional_by_type = _typescript_type_from_schema(schema, field_schema)
            optional = "?" if (field_name not in required or optional_by_type) else ""
            types_content += f"  {field_name}{optional}: {field_type};\n"
        types_content += "}\n"

    append_typescript_interface("McpAddParams", params_schema)
    append_typescript_interface("McpRemoveParams", params_schema)
    append_typescript_interface("McpReloadParams", params_schema)
    append_typescript_interface("McpLiveOpResponse", wire_schema)

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


def main():
    root = Path(__file__).parent.parent.parent
    artifacts_dir = root / "artifacts" / "schemas"

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
    py_output = root / "sdks" / "python" / "meerkat" / "generated"
    generate_python_types(schemas, py_output, has_comms=has_comms, has_skills=has_skills)
    print(f"Generated Python types in {py_output}")

    # Generate TypeScript
    ts_output = root / "sdks" / "typescript" / "src" / "generated"
    generate_typescript_types(schemas, ts_output, has_comms=has_comms, has_skills=has_skills)
    print(f"Generated TypeScript types in {ts_output}")


if __name__ == "__main__":
    main()
