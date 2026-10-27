"""Pure connection proposal from an owner-checked Toolkit installation result.

The caller must obtain the job from its selected Toolkit owner. This function
does not verify installed files, launch a server, authenticate or grant tools.
"""
from __future__ import annotations

import copy
import re

from .blueprint import Connection, ENV, ID
from .connector_discovery import normalize_custom_config
from .templates_v2.application_configuration import RESERVED


PROTECTED_REFERENCES = RESERVED | {"SYSTEMROOT", "VIRTUAL_ENV", "GOOGLE_API_KEY"}


class ToolkitHandoffError(ValueError):
    """Fixed public diagnostics without source values or private launch data."""


def normalize_toolkit_handoff(job, *, connection_id, name):
    """Return one ungranted, current-host-only Connection and limited provenance."""
    if (not isinstance(connection_id, str) or re.fullmatch(ID, connection_id) is None
            or not isinstance(name, str) or not name.strip() or len(name) > 100
            or any(ord(character) < 32 or ord(character) == 127 for character in name)):
        raise ToolkitHandoffError("Choose a valid connection ID and display name.")
    if (not isinstance(job, dict) or job.get("kind") != "install"
            or job.get("status") != "succeeded" or job.get("recovered")
            or not isinstance(job.get("id"), str)
            or re.fullmatch(r"[0-9a-f]{32}", job["id"]) is None):
        raise ToolkitHandoffError("Use a succeeded live Toolkit installation job.")
    result = job.get("result")
    if not isinstance(result, dict) or result.get("scope") != "observed_software_installation_only":
        raise ToolkitHandoffError("The Toolkit job does not establish a configured software installation.")
    rows = result.get("credential_environment_references")
    if not isinstance(rows, list) or len(rows) > 256:
        raise ToolkitHandoffError("The installed validator must report its credential references, including an explicit empty list.")
    names, paths = [], set()
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"field_path", "environment_variable"}:
            raise ToolkitHandoffError("The Toolkit credential reference report is invalid.")
        path, variable = row["field_path"], row["environment_variable"]
        if (not isinstance(path, list) or not 1 <= len(path) <= 2
                or any(not isinstance(key, str) or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", key) is None for key in path)
                or (len(path) == 2 and path[0] != "config")
                or not isinstance(variable, str) or re.fullmatch(ENV, variable) is None
                or tuple(path) in paths):
            raise ToolkitHandoffError("The Toolkit credential reference report is invalid.")
        if variable in PROTECTED_REFERENCES or variable.startswith(("MOBKIT_", "PYTHON", "DYLD_", "LD_", "CONSOLE_")):
            raise ToolkitHandoffError("Configure dedicated connector credential names instead of protected host or provider names.")
        paths.add(tuple(path))
        if variable not in names:
            names.append(variable)
    if len(names) > 50:
        raise ToolkitHandoffError("The selected connection requests too many private credential fields.")

    config = result.get("mcp_configuration")
    servers = config.get("mcpServers") if isinstance(config, dict) and set(config) == {"mcpServers"} else None
    if not isinstance(servers, dict) or len(servers) != 1:
        raise ToolkitHandoffError("The Toolkit installation must publish exactly one MCP server.")
    server_name, server = next(iter(servers.items()))
    if (not isinstance(server_name, str) or not isinstance(server, dict)
            or not {"command", "args"} <= set(server)
            or set(server) - {"command", "args", "env"}
            or not isinstance(server["command"], str) or not isinstance(server["args"], list)
            or any(not isinstance(arg, str) for arg in server["args"])
            or ("env" in server and (not isinstance(server["env"], dict) or server["env"]))):
        raise ToolkitHandoffError("Use the Toolkit's literal stdio command and arguments without environment values.")
    try:
        normalized = normalize_custom_config(copy.deepcopy(config))
        connection = normalized["connections"][0]
        connection.update(id=connection_id, name=name,
            env={variable: "${" + variable + "}" for variable in names},
            required_environment=names,
            setup_inputs=[{"name": variable, "description": "Configure this credential privately on the deployment host.",
                           "secret": True, "required": True} for variable in names])
        connection = Connection.model_validate(connection).model_dump(mode="json")
        if connection["command"] != server["command"] or connection["args"] != server["args"]:
            raise ValueError("The literal launch changed.")
    except (ValueError, KeyError, IndexError, TypeError):
        raise ToolkitHandoffError("The Toolkit launch or requested connection identity is invalid.") from None
    return {"connection": connection, "provenance": {
        "job_id": job["id"], "scope": "software_only", "portability": "current_host_only",
        "credential_environment_references": copy.deepcopy(rows)}}
