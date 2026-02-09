#!/usr/bin/env python3
"""SDK code generator for Meerkat.

Reads schema artifacts from artifacts/schemas/ and generates typed client
code for Python and TypeScript SDKs.
"""

import json
import os
import sys
from pathlib import Path


def load_schemas(artifacts_dir: Path) -> dict:
    """Load all schema artifacts."""
    schemas = {}
    for f in artifacts_dir.glob("*.json"):
        with open(f) as fh:
            schemas[f.stem] = json.load(fh)
    return schemas


def generate_python_types(schemas: dict, output_dir: Path) -> None:
    """Generate Python type definitions from schemas."""
    output_dir.mkdir(parents=True, exist_ok=True)

    # Generate __init__.py
    init_content = '"""Generated types for Meerkat Python SDK."""\n\n'
    init_content += "from .types import *  # noqa: F401,F403\n"
    init_content += "from .errors import *  # noqa: F401,F403\n"
    (output_dir / "__init__.py").write_text(init_content)

    # Generate types from version and wire-types schemas
    version_info = schemas.get("version", {})
    contract_version = version_info.get("contract_version", "0.1.0")

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
    types_content += "    capabilities: list = field(default_factory=list)\n"

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


def generate_typescript_types(schemas: dict, output_dir: Path) -> None:
    """Generate TypeScript type definitions from schemas."""
    output_dir.mkdir(parents=True, exist_ok=True)

    version_info = schemas.get("version", {})
    contract_version = version_info.get("contract_version", "0.1.0")

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
    index_content = "// Generated exports\nexport * from './types';\nexport * from './errors';\n"
    (output_dir / "index.ts").write_text(index_content)


def main():
    root = Path(__file__).parent.parent.parent
    artifacts_dir = root / "artifacts" / "schemas"

    if not artifacts_dir.exists():
        print(f"Error: {artifacts_dir} does not exist. Run emit-schemas first.", file=sys.stderr)
        sys.exit(1)

    schemas = load_schemas(artifacts_dir)
    print(f"Loaded {len(schemas)} schema files")

    # Generate Python
    py_output = root / "sdks" / "python" / "meerkat" / "generated"
    generate_python_types(schemas, py_output)
    print(f"Generated Python types in {py_output}")

    # Generate TypeScript
    ts_output = root / "sdks" / "typescript" / "src" / "generated"
    generate_typescript_types(schemas, ts_output)
    print(f"Generated TypeScript types in {ts_output}")


if __name__ == "__main__":
    main()
