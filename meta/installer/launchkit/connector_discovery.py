"""Read-only MCP Registry discovery and secret-free configuration normalization.

Discovery never starts a server, installs packages, follows provider URLs, or
reads local credentials. All runtime choices remain inspectable blueprint data.
"""
from __future__ import annotations

import asyncio
import hashlib
import json
import re
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

from .catalog import PROVIDERS, REGISTRY_URL
from .blueprint import HEADER_REFERENCE

MAX_RESPONSE = 2_000_000
ENV_REF = re.compile(r"\$\{([A-Z_][A-Z0-9_]*)\}")
VARIABLE = re.compile(r"(?<!\$)\{([A-Za-z_][A-Za-z0-9_-]*)\}")
ENV_NAME = re.compile(r"^[A-Z_][A-Z0-9_]*$")


class DiscoveryError(ValueError):
    """A useful, credential-free discovery error suitable for the local UI."""


class _RegistryRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urlsplit(newurl).netloc != "registry.modelcontextprotocol.io" or not newurl.startswith("https://"):
            raise DiscoveryError("The MCP Registry redirected outside its official host.")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _fetch_json(path: str) -> dict:
    request = Request(REGISTRY_URL + path, headers={"Accept": "application/json", "User-Agent": "Meta-Forge/0.2"})
    try:
        with build_opener(_RegistryRedirects).open(request, timeout=12) as response:
            raw = response.read(MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE:
            raise DiscoveryError("The registry response exceeded the size limit. Narrow your search.")
        result = json.loads(raw)
        if not isinstance(result, dict):
            raise DiscoveryError("The registry returned an unexpected document.")
        return result
    except (HTTPError, URLError, TimeoutError, json.JSONDecodeError) as error:
        raise DiscoveryError("The official MCP Registry could not be reached. Retry or paste an MCP configuration.") from error


def _text(value: Any, maximum: int = 2000) -> str:
    if not isinstance(value, str) or len(value) > maximum or any(c in value for c in "\x00\r\n"):
        raise DiscoveryError("MCP configuration contains an invalid or oversized text field.")
    return value


def _slug(value: str) -> str:
    value = re.sub(r"[^a-z0-9-]+", "-", value.lower()).strip("-")
    return ("mcp-" + value)[:32] if not value or not value[0].isalpha() else value[:32]


def _env_name(value: str, prefix: str = "MCP") -> str:
    value = re.sub(r"[^A-Z0-9_]", "_", value.upper()).strip("_")
    if not value or value[0].isdigit():
        value = prefix + "_" + value
    return value[:80]


def _url(value: str) -> str:
    value = _text(value, 4000)
    # Placeholder values are substituted only by the exported runtime.
    resolved_shape = ENV_REF.sub("placeholder", value)
    if "{" in resolved_shape or "}" in resolved_shape:
        raise DiscoveryError("The MCP URL contains an unresolved placeholder. Provide a complete URL or an environment-backed configuration.")
    parsed = urlsplit(resolved_shape)
    if parsed.scheme not in {"https", "http"} or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
        raise DiscoveryError("Use an HTTP MCP URL without embedded credentials or fragments.")
    if parsed.scheme == "http" and parsed.hostname not in {"localhost", "127.0.0.1", "::1"}:
        raise DiscoveryError("Remote MCP connections require HTTPS. HTTP is available for loopback services.")
    if parsed.query:
        raise DiscoveryError("Put MCP authentication in environment-backed headers, not URL query strings.")
    return value


class _Inputs:
    def __init__(self, prefix: str):
        self.prefix = _env_name(prefix)
        self.items: dict[str, dict] = {}
        self.optional_arguments: list[list[str]] = []
        self.variable_sources: dict[str, str] = {}

    def ref(self, name: str, description: str = "", secret: bool = False, required: bool = True, default: str = "") -> str:
        name = _env_name(name, self.prefix)
        item = self.items.setdefault(name, {"name": name, "description": description or f"Configuration for {name}", "secret": secret, "required": required})
        item["secret"] = item["secret"] or secret
        item["required"] = item["required"] or required
        if default and not item["secret"]:
            item["default"] = _text(default, 2000)
        return "${" + name + "}"

    def render(self, item: dict, fallback: str, *, secret: bool = False) -> str:
        secret = bool(item.get("isSecret", secret))
        required = bool(item.get("isRequired", True))
        value = item.get("value")
        if value is None:
            value = None if secret else item.get("default")
        if value is None:
            return self.ref(fallback, item.get("description", ""), secret, required)
        value = _text(str(value))
        variables = item.get("variables", {})
        if not isinstance(variables, dict):
            raise DiscoveryError("Invalid MCP input variable definitions.")
        def replace(match):
            key = match.group(1)
            definition = variables.get(key, {})
            if not isinstance(definition, dict):
                raise DiscoveryError("Invalid MCP input variable definition.")
            name = _env_name(f"{self.prefix}_{key}")
            if self.variable_sources.setdefault(name, key) != key:
                raise DiscoveryError("Distinct MCP variables map to the same environment input. Provide an explicit custom configuration.")
            return self.ref(name, definition.get("description", item.get("description", "")), bool(definition.get("isSecret", secret)), bool(definition.get("isRequired", required)), str(definition.get("default", "")))
        rendered = VARIABLE.sub(replace, value)
        if secret and rendered == value and not ENV_REF.search(value):
            return self.ref(fallback, item.get("description", ""), True, required)
        return rendered

    def arguments(self, entries: list[dict], stem: str) -> list[str]:
        if not isinstance(entries, list) or len(entries) > 60:
            raise DiscoveryError("MCP argument list is invalid or too long.")
        result = []
        for index, entry in enumerate(entries):
            if not isinstance(entry, dict):
                raise DiscoveryError("MCP arguments must be structured inputs.")
            # Optional inputs without defaults are still surfaced for explicit
            # configuration. Do not silently omit scope or access arguments.
            value = self.render(entry, entry.get("valueHint", f"{stem}_{index + 1}"))
            chunk = []
            if entry.get("type") == "named":
                chunk.append(_text(entry.get("name"), 100))
            elif entry.get("type") != "positional":
                raise DiscoveryError("Unsupported MCP argument type.")
            chunk.append(value)
            result.extend(chunk)
            if not entry.get("isRequired", True) and ENV_REF.search(value):
                if entry.get("type") != "named":
                    raise DiscoveryError("Optional positional MCP arguments need explicit custom configuration to preserve argument order.")
                self.optional_arguments.append(chunk)
        return result


def _base_connection(server: dict, suffix: str) -> dict:
    registry_id = _text(server.get("name"), 300)
    version = _text(server.get("version", ""), 100)
    digest = hashlib.sha256((registry_id + suffix).encode()).hexdigest()[:6]
    repository = server.get("repository") or {}
    return {
        "id": _slug(registry_id.rsplit("/", 1)[-1])[:28] + "-" + digest,
        "name": _text(server.get("title") or registry_id.rsplit("/", 1)[-1], 100),
        "description": _text(server.get("description", ""), 5000)[:2000], "required_environment": [],
        "transport": "stdio", "command": "", "args": [], "url": "", "env": {}, "headers": {},
        "source": {"registry_id": registry_id, "repository": _text(repository.get("url", ""), 1000), "version": version,
                   "url": REGISTRY_URL + "/v0.1/servers/" + quote(registry_id, safe="") + "/versions/" + quote(version, safe="")},
    }


def _package_option(server: dict, package: dict, index: int) -> dict:
    if package.get("transport", {}).get("type") != "stdio":
        raise DiscoveryError("This package needs a separately managed HTTP server; use its remote endpoint or custom configuration.")
    connection = _base_connection(server, f"package-{index}")
    inputs = _Inputs(connection["id"])
    identifier = _text(package.get("identifier"), 500)
    version = _text(package.get("version", ""), 255)
    kind = package.get("registryType")
    allowed_runtime = {"npm": "npx", "pypi": "uvx", "oci": "docker"}
    if kind not in allowed_runtime or package.get("runtimeHint", allowed_runtime[kind]) != allowed_runtime[kind]:
        raise DiscoveryError("This package runtime needs a custom MCP configuration.")
    runtime_args = inputs.arguments(package.get("runtimeArguments", []), "RUNTIME_ARG")
    package_args = inputs.arguments(package.get("packageArguments", []), "SERVER_ARG")
    if kind in {"npm", "pypi"} and (not version or version == "latest" or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+_-]*", version)):
        raise DiscoveryError("The package has no exact version. Select a pinned custom configuration.")
    if kind == "npm":
        if not re.fullmatch(r"(?:@[a-z0-9._-]+/)?[a-z0-9._-]+", identifier):
            raise DiscoveryError("Invalid npm package identifier.")
        command, args = "npx", ["--yes", *runtime_args, f"{identifier}@{version}", *package_args]
    elif kind == "pypi":
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", identifier):
            raise DiscoveryError("Invalid Python package identifier.")
        command, args = "uvx", [*runtime_args, f"{identifier}=={version}", *package_args]
    else:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/@:-]*", identifier):
            raise DiscoveryError("Invalid container image identifier.")
        if ":" not in identifier.rsplit("/", 1)[-1] and "@sha256:" not in identifier:
            if not version:
                raise DiscoveryError("The container image has no exact tag or digest.")
            identifier += ":" + version
        if identifier.endswith(":latest"):
            raise DiscoveryError("The container uses a mutable latest tag. Select an exact release.")
        command, args = "docker", ["run", "--rm", "-i", *runtime_args]
        for variable in package.get("environmentVariables", []):
            args.extend(["-e", _text(variable.get("name"), 100)])
        args.extend([identifier, *package_args])
    env = {}
    for item in package.get("environmentVariables", []):
        key = _text(item.get("name"), 100)
        if not ENV_NAME.fullmatch(key):
            raise DiscoveryError("Invalid MCP environment variable name.")
        # Environment values always remain local setup inputs, including
        # nonsecret paths and scope choices. Registry defaults are descriptions.
        env[key] = inputs.ref(f"{inputs.prefix}_{key}", item.get("description", ""), bool(item.get("isSecret")), bool(item.get("isRequired", False)),
                              str(item.get("default", "")) if not item.get("isSecret") else "")
    connection.update(command=command, args=args, env=env, required_environment=list(inputs.items))
    connection["setup_inputs"] = list(inputs.items.values())
    connection["optional_args"] = []
    for chunk in inputs.optional_arguments:
        for index in range(len(args) - len(chunk) + 1):
            if args[index:index + len(chunk)] == chunk:
                connection["optional_args"].extend(range(index, index + len(chunk)))
                break
    warnings = ["This package will run on the deployment host when an assigned agent starts. Review its source and exact arguments."]
    if package.get("fileSha256"):
        warnings.append("The registry publishes a file checksum; the package manager resolves this pinned version, without independently checking that registry file checksum.")
    return {"label": f"{kind}: {identifier}", "connection": connection, "required_environment": list(inputs.items.values()), "warnings": warnings, "auth": {"kind": "environment" if inputs.items else "unspecified", "detail": "Credentials and configuration are requested locally during installation."}}


def _remote_option(server: dict, remote: dict, index: int) -> dict:
    if remote.get("type") != "streamable-http":
        raise DiscoveryError("Legacy SSE requires a bridge. Select Streamable HTTP or provide a stdio bridge configuration.")
    connection = _base_connection(server, f"remote-{index}")
    inputs = _Inputs(connection["id"])
    url = _url(inputs.render({"value": remote.get("url"), "variables": remote.get("variables", {})}, "MCP_URL"))
    headers = {}
    for item in remote.get("headers", []):
        key = _text(item.get("name"), 100)
        if not re.fullmatch(r"[A-Za-z0-9-]{1,80}", key):
            raise DiscoveryError("Invalid MCP header name.")
        headers[key] = inputs.render(item, connection["id"] + "_" + key, secret=key.lower() in {"authorization", "x-api-key"})
        if not ENV_REF.search(headers[key]):
            headers[key] = inputs.ref(connection["id"] + "_" + key, item.get("description", "") or f"Header {key}",
                                      bool(item.get("isSecret", key.lower() in {"authorization", "x-api-key"})),
                                      bool(item.get("isRequired", True)), headers[key])
        if not HEADER_REFERENCE.fullmatch(headers[key]):
            raise DiscoveryError(f"Header {key} combines several value parts. Provide a custom connection with its complete header value in one local environment input.")
    host = urlsplit(url).hostname
    auth = {"kind": "environment" if inputs.items else "unspecified", "detail": "Registry metadata does not prove authentication readiness. Installation can test the MCP handshake."}
    warnings = []
    if host == "api.githubcopilot.com" and server.get("name") == PROVIDERS["github"]["registry_id"]:
        inputs.items.clear()
        headers["Authorization"] = "Bearer " + inputs.ref("GITHUB_PERSONAL_ACCESS_TOKEN", "GitHub personal access or App token with the intended repository permissions.", True)
        auth = {"kind": "bearer", "detail": PROVIDERS["github"]["auth"], "documentation": PROVIDERS["github"]["documentation"]}
    elif host == "mcp.linear.app":
        headers["Authorization"] = "Bearer " + inputs.ref("LINEAR_API_KEY", "Linear API key or OAuth access token.", True)
        auth = {"kind": "bearer", "detail": PROVIDERS["linear"]["auth"], "documentation": PROVIDERS["linear"]["documentation"]}
    elif host == "mcp.notion.com":
        raise DiscoveryError("Hosted Notion requires an OAuth-capable client. Supply an authenticated bridge or the official stdio server with an integration token; this exporter does not invent a hosted API-key login.")
    elif not headers:
        warnings.append("No authentication metadata was published. The server may require OAuth or other setup; verify it during installation.")
    connection.update(transport="http", url=url, headers=headers, required_environment=list(inputs.items))
    connection["setup_inputs"] = list(inputs.items.values())
    return {"label": "Hosted MCP", "connection": connection, "required_environment": list(inputs.items.values()), "warnings": warnings, "auth": auth}


def normalize_registry_entry(entry: dict) -> dict:
    server = entry.get("server", entry)
    if not isinstance(server, dict):
        raise DiscoveryError("Invalid registry server record.")
    base = _base_connection(server, "")
    official = entry.get("_meta", {}).get("io.modelcontextprotocol.registry/official", {})
    options, warnings = [], []
    for kind, entries in [("remote", server.get("remotes", [])), ("package", server.get("packages", []))]:
        if not isinstance(entries, list):
            continue
        for index, item in enumerate(entries[:8]):
            try:
                options.append((_remote_option if kind == "remote" else _package_option)(server, item, index))
            except (DiscoveryError, TypeError, AttributeError) as error:
                warnings.append(str(error) if isinstance(error, DiscoveryError) else "A registry option could not be normalized.")
    if official.get("status", "active") != "active":
        options = []
        warnings.append("This registry entry is not active.")
    preferred = options[0] if options else {}
    return {
        "registry_id": base["source"]["registry_id"], "name": base["name"],
        "description": _text(server.get("description", ""), 5000), "repository": base["source"]["repository"],
        "version": base["source"]["version"], "source_url": base["source"]["url"],
        "connection": preferred.get("connection"), "alternatives": options,
        "required_environment": preferred.get("required_environment", []),
        "warnings": [*warnings, *preferred.get("warnings", [])], "installable": bool(options),
        "auth": preferred.get("auth", {"kind": "manual", "detail": "Follow the publisher's installation instructions."}),
    }


async def search_registry(query: str, limit: int = 12) -> dict:
    query = _text(query.strip(), 100)
    if not query:
        raise DiscoveryError("Enter an app, service, or capability to find its MCP servers.")
    limit = max(1, min(int(limit), 30))
    paths = ["/v0.1/servers?" + urlencode({"search": query, "version": "latest", "limit": 100})]
    preferred = PROVIDERS.get(query.lower(), {}).get("registry_id")
    if preferred:
        paths.append("/v0.1/servers/" + quote(preferred, safe="") + "/versions/latest")
    responses = await asyncio.gather(*(asyncio.to_thread(_fetch_json, path) for path in paths), return_exceptions=True)
    if isinstance(responses[0], Exception):
        raise responses[0]
    raw = responses[0].get("servers", [])
    if not isinstance(raw, list):
        raise DiscoveryError("The registry returned an invalid server list.")
    if len(responses) > 1 and isinstance(responses[1], dict):
        raw = [responses[1], *raw]
    results, seen, warnings = [], set(), []
    for entry in raw:
        try:
            normalized = normalize_registry_entry(entry)
        except (DiscoveryError, TypeError, AttributeError):
            warnings.append("One registry entry had unsupported metadata and was skipped.")
            continue
        if normalized["registry_id"] not in seen:
            results.append(normalized)
            seen.add(normalized["registry_id"])
    results.sort(key=lambda item: (item["registry_id"] != preferred, not item["installable"], item["registry_id"]))
    if responses[0].get("metadata", {}).get("nextCursor"):
        warnings.append("More matches exist. Narrow the search to a publisher or server name.")
    return {"results": results[:limit], "warnings": sorted(set(warnings)), "source": "registry"}


def normalize_custom_config(config: dict) -> dict:
    if not isinstance(config, dict):
        raise DiscoveryError("Paste a JSON MCP configuration object.")
    servers = config.get("mcpServers", config.get("servers"))
    if servers is None:
        servers = {config.get("name", "custom-mcp"): config}
    if not isinstance(servers, dict) or not servers or len(servers) > 30:
        raise DiscoveryError("Provide one to thirty MCP server configurations.")
    connections, required, warnings = [], [], []
    for name, raw in servers.items():
        if not isinstance(raw, dict):
            raise DiscoveryError("Each MCP server configuration must be an object.")
        if raw.get("disabled"):
            continue
        key = _slug(_text(name, 100))
        inputs = _Inputs(key)
        connection = {"id": key, "name": _text(name, 100), "transport": "stdio", "command": "", "args": [], "url": "", "env": {}, "headers": {}, "source": {"registry_id": "", "repository": "", "version": "", "url": ""}}
        if "url" in raw:
            if raw.get("type", raw.get("transport", "http")) not in {"http", "streamable-http"}:
                raise DiscoveryError("Use Streamable HTTP or a stdio bridge for legacy SSE.")
            connection.update(transport="http", url=_url(raw["url"]))
            header_values = raw.get("headers", {})
            if not isinstance(header_values, dict):
                raise DiscoveryError("MCP headers must be a JSON object.")
            for header, value in header_values.items():
                _text(header, 100)
                if not re.fullmatch(r"[A-Za-z0-9-]{1,80}", header):
                    raise DiscoveryError("Invalid MCP header name.")
                value = _text(value)
                if ENV_REF.fullmatch(value) or re.fullmatch(r"(?:Bearer|Basic) \$\{[A-Z_][A-Z0-9_]*\}", value):
                    for ref in ENV_REF.findall(value):
                        inputs.ref(ref, f"Value for {name} {header}", True)
                    connection["headers"][header] = value
                else:
                    prefix = "Bearer " if value.lower().startswith("bearer ") else ""
                    connection["headers"][header] = prefix + inputs.ref(f"{key}_{header}", f"Credential for {name} {header}", True)
                    warnings.append("Pasted credentials were replaced with local environment references and were not saved.")
        else:
            command = _text(raw.get("command"), 1000)
            if not command or any(char.isspace() for char in command):
                raise DiscoveryError("Use one executable in command and a separate args array.")
            args = raw.get("args", [])
            if not isinstance(args, list) or len(args) > 100:
                raise DiscoveryError("MCP args must be an array of at most 100 strings.")
            connection.update(command=command, args=[_text(arg, 4000) for arg in args])
            for index, arg in enumerate(args):
                if re.search(r"[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@", arg, re.I) and not ENV_REF.search(arg):
                    raise DiscoveryError("Move credential-bearing connection URLs from arguments into environment references before importing.")
                if re.fullmatch(r"--?(?:token|password|secret|api[-_]?key|authorization)", arg, re.I) and index + 1 < len(args) and not ENV_REF.search(args[index + 1]):
                    raise DiscoveryError("Move literal credentials from command arguments into environment references before importing.")
                if re.search(r"(?:Authorization:|Bearer )", arg, re.I) and not ENV_REF.search(arg):
                    raise DiscoveryError("Move literal credentials from command arguments into environment references before importing.")
            if any(re.search(r"(?:token|password|secret|api[-_]?key)=([^$\s]+)", arg, re.I) for arg in args):
                raise DiscoveryError("Move literal credentials from command arguments into environment references before importing.")
            env = raw.get("env", {})
            if not isinstance(env, dict):
                raise DiscoveryError("MCP env must be a JSON object.")
            for variable, value in env.items():
                if not ENV_NAME.fullmatch(variable):
                    raise DiscoveryError("Environment names must use uppercase letters, digits, and underscores.")
                value = _text(value)
                existing = ENV_REF.fullmatch(value)
                ref = existing.group(1) if existing else _env_name(f"{key}_{variable}")
                connection["env"][variable] = inputs.ref(ref, f"Local value for {name}: {variable}", bool(re.search(r"TOKEN|KEY|PASS|SECRET|CREDENTIAL", variable)))
                if not existing:
                    warnings.append("Pasted environment values were replaced with local setup inputs and were not saved.")
        connection["required_environment"] = list(inputs.items)
        connections.append(connection)
        required.extend(inputs.items.values())
    if len({item["id"] for item in connections}) != len(connections):
        raise DiscoveryError("Server names become duplicate connection IDs. Rename them before importing.")
    return {"connections": connections, "required_environment": required, "warnings": sorted(set(warnings))}
