"""Configured asyncio MCP stdio over selected Slack, Gmail, Calendar and Drive operations."""

from __future__ import annotations

import argparse
import asyncio
import base64
from copy import deepcopy
from dataclasses import dataclass, field
from email.message import Message
import json
import os
import re
import sys
from typing import Any, Callable, TYPE_CHECKING
from urllib.parse import quote

from ._mcp_admission import AdmissionRefused, CallAdmission
from .services_config import (
    Configuration, ConfigurationError, ORIGINS, configuration_help,
    load_configuration, profiles, tool_descriptions, validate_configuration,
)

if TYPE_CHECKING:
    import httpx
    from mcp.server import Server


def selected_token(configuration: Configuration) -> str:
    invalid = False
    try:
        value = os.environ[configuration.config["token_env"]]
        if type(value) is not str or not 1 <= len(value) <= 4096 or any(not 33 <= ord(char) <= 126 for char in value):
            raise ValueError()
    except Exception:
        invalid = True
    if invalid:
        raise ConfigurationError()
    return value


async def _joined(task: asyncio.Task[Any]) -> Any:
    """Retain cleanup through repeated direct caller cancellation."""
    import anyio

    cancellation = None
    with anyio.CancelScope(shield=True):
        while not task.done():
            try:
                await asyncio.shield(task)
            except asyncio.CancelledError as error:
                cancellation = error
            except Exception:
                break
        if cancellation is not None:
            if not task.cancelled():
                task.exception()
            raise cancellation
        return task.result()


@dataclass
class ServiceProfile:
    configuration: Configuration
    operations: Any = field(repr=False)
    client: httpx.AsyncClient = field(repr=False)
    _owns_client: bool = field(default=True, init=False, repr=False)
    _close_task: asyncio.Task[None] | None = field(default=None, init=False, repr=False)
    _lock: asyncio.Lock = field(default_factory=asyncio.Lock, init=False, repr=False)
    _admission: CallAdmission = field(init=False, repr=False)

    def __post_init__(self) -> None:
        # Multiple MCP servers borrowing this profile share its existing serial
        # execution owner and the same finite admission capacity.
        self._admission = CallAdmission(self._lock)

    async def aclose(self) -> None:
        async def close_after_calls() -> None:
            async with self._lock:
                if self._owns_client:
                    await self.client.aclose()

        if self._close_task is None:
            self._close_task = asyncio.create_task(close_after_calls())
        await _joined(self._close_task)


def _profile_configuration(configuration: Configuration) -> Configuration:
    values = dict(configuration.config)
    # The public configuration accepts JSON arrays. Validated instances freeze
    # the ceiling as a tuple, so restore its wire shape before revalidation.
    for field in ("allowed_channel_ids", "allowed_recipient_addresses"):
        if type(values.get(field)) is tuple:
            values[field] = list(values[field])
    return validate_configuration({"profile": configuration.profile,
        "operations": list(configuration.operations), "config": values})


def _profile_with_client(
    configuration: Configuration, client: httpx.AsyncClient, *, owns_client: bool,
) -> ServiceProfile:
    from ..connectors.gmail import GmailOperations
    from ..connectors.google_calendar import GoogleCalendarOperations
    from ..connectors.google_drive import GoogleDriveOperations
    from ..connectors.slack import SlackOperations

    common = {"timeout_seconds": configuration.config["timeout_seconds"],
              "max_response_bytes": configuration.config["max_response_bytes"]}
    if configuration.profile == "slack":
        operations = SlackOperations(client, allowed_channel_ids=configuration.config.get("allowed_channel_ids"), **common)
    elif configuration.profile == "google-calendar":
        operations = GoogleCalendarOperations(client, calendar_id=configuration.config["calendar_id"], **common)
    elif configuration.profile == "google-drive":
        operations = GoogleDriveOperations(client, max_content_bytes=configuration.config["max_content_bytes"], **common)
    else:
        operations = GmailOperations(client, user_id=configuration.config["user_id"],
                                     max_message_bytes=configuration.config["max_message_bytes"], **common)
    profile = ServiceProfile(configuration, operations, client)
    profile._owns_client = owns_client
    return profile


def create_profile_from_client(
    configuration: Configuration, client: httpx.AsyncClient,
) -> ServiceProfile:
    """Borrow a host-authenticated client for one fixed service/account selection.

    The existing token_env configuration reference is validated but never read.
    The host must prepare the existing response-custody hook before borrowing.
    The host retains
    authentication, refresh, transport and client lifetime; its auth/hooks must
    not retarget or replay requests. Closing the profile joins its calls without
    closing the client. This entry point is not exposed by the CLI or MCP tools.
    """
    configuration = _profile_configuration(configuration)
    import httpx
    from ..httpx_client import require_prepared_httpx_client

    invalid = False
    try:
        if (not isinstance(client, httpx.AsyncClient) or client.is_closed
                or client.base_url != httpx.URL(ORIGINS[configuration.profile])):
            raise ValueError()
        require_prepared_httpx_client(client)
        profile = _profile_with_client(configuration, client, owns_client=False)
    except Exception:
        invalid = True
    if invalid:
        raise ConfigurationError()
    return profile


async def create_profile(
    configuration: Configuration, *, client_factory: Callable[..., httpx.AsyncClient] | None = None,
) -> ServiceProfile:
    """Own a new client; an explicit host factory may supply its trusted transport.

    The CLI never exposes this factory, a provider URL or arbitrary authentication.
    A custom host factory must preserve the supplied account/auth/origin settings
    and cannot retry, retarget or detach entered requests.
    """
    # Revalidate before any credential/runtime access, including direct callers.
    configuration = _profile_configuration(configuration)
    token = selected_token(configuration)
    import httpx
    from ..httpx_client import prepare_httpx_client
    factory = httpx.AsyncClient if client_factory is None else client_factory
    client = factory(base_url=ORIGINS[configuration.profile], headers={"Authorization": f"Bearer {token}"},
                     trust_env=False, follow_redirects=False, timeout=None)
    try:
        prepare_httpx_client(client)
        return _profile_with_client(configuration, client, owns_client=True)
    except BaseException:
        await _joined(asyncio.create_task(client.aclose()))
        raise


def _failure(kind: str, *, effect: str | None = None, entered: bool = False, **metadata: Any):
    from mcp import types

    error = {"kind": kind, "effect": effect, "operation_invoked": entered,
             "write_outcome": ("unknown" if entered else "not_attempted") if effect == "write" else "not_applicable",
             **metadata}
    return types.CallToolResult(content=[types.TextContent(type="text", text=f"Service operation {kind}")],
                                structured_content={"error": error}, is_error=True)



def _interval_tool_result(value: Any, maximum: int):
    """Preserve both MCP projections while bounding their combined serialization.

    The core owns the complete-result budget. An ASCII JSON text of B bytes
    needs at most 2B bytes when embedded as a JSON string; its structured form
    needs at most B. The remaining envelope is measured from the pinned MCP type.
    A defensive breach returns a read failure, never clipped data or completeness.
    """
    from mcp import types

    text = json.dumps(value, ensure_ascii=True, allow_nan=False, separators=(",", ":"))
    if len(text.encode("ascii")) > maximum:
        return _failure("result_too_large", effect="read", entered=True)
    result = types.CallToolResult(content=[types.TextContent(type="text", text=text)],
                                  structured_content={"value": value})
    empty = types.CallToolResult(content=[types.TextContent(type="text", text="")],
                                 structured_content={"value": None})
    overhead = len(empty.model_dump_json(by_alias=True, exclude_none=True).encode("utf-8"))
    if len(result.model_dump_json(by_alias=True, exclude_none=True).encode("utf-8")) > 3 * maximum + overhead:
        return _failure("result_too_large", effect="read", entered=True)
    return result


def create_server(profile: ServiceProfile) -> Server:
    """Borrow a profile. The caller closes it after this server and its calls join."""
    return _create_server(profile.configuration, profile=profile)


def create_native_server(configuration: Configuration) -> Server:
    """Require private native metadata for every call; never read token_env."""
    from ._connector_call import ConnectorBinding, protect_server

    binding = ConnectorBinding.from_environment()
    return protect_server(_create_server(_profile_configuration(configuration), binding=binding))


def _create_server(configuration: Configuration, *, profile: ServiceProfile | None = None, binding=None) -> Server:
    import anyio
    from jsonschema import Draft202012Validator
    from referencing import Registry
    from mcp import types
    from mcp.server import Server
    from ..connectors.gmail import GmailError
    from ..connectors.gmail_content import GmailContentError, readable_message, readable_text_part, select_text_part
    from ..connectors.google_calendar import CalendarError
    from ..connectors.google_drive import DriveContent, DriveError
    from ..connectors.slack import SlackError
    from .gmail_composition import RecipientPermissionError, compose_message

    descriptions = {item["name"]: item for item in tool_descriptions(configuration)}
    tools = [types.Tool(name=item["name"], description=item["description"], input_schema=item["inputSchema"],
        annotations=types.ToolAnnotations(read_only_hint=item["effect"] == "read",
            destructive_hint=item["effect"] == "write", idempotent_hint=item["effect"] == "read", open_world_hint=True))
        for item in descriptions.values()]
    validators = {name: Draft202012Validator(item["inputSchema"], registry=Registry()) for name, item in descriptions.items()}

    async def list_tools(_context, _params):
        return types.ListToolsResult(tools=deepcopy(tools))

    admission = profile._admission if profile is not None else CallAdmission(asyncio.Lock())

    async def call_tool(context, params):
        item = descriptions.get(params.name)
        if item is None:
            return _failure("unknown_function")
        effect = item["effect"]
        try:
            async with admission.enter(is_closed=lambda: profile is not None and profile._close_task is not None):
                if profile is not None:
                    return await invoke(params, item, profile)
                return await native_invoke(context, params, item)
        except AdmissionRefused as error:
            return _failure(error.kind, effect=effect)

    async def native_invoke(context, params, item):
        from ._connector_call import ConnectorCall, CredentialRefused

        call = None
        invoked = False
        try:
            call = ConnectorCall(context, binding, params.name, ORIGINS[configuration.profile])
            client = call.httpx_client(ORIGINS[configuration.profile])
            current = None
            result = None
            cancelled = False
            try:
                current = create_profile_from_client(configuration, client)
                invoked = True
                result = await invoke(params, item, current)
            except asyncio.CancelledError:
                cancelled = True
                raise
            finally:
                async def close():
                    try:
                        if current is not None:
                            await current.aclose()
                    finally:
                        await client.aclose()
                try:
                    await _joined(asyncio.create_task(close()))
                except Exception:
                    if result is not None:
                        # Cleanup cannot replace a captured operation result.
                        result.meta = {**(result.meta or {}),
                            "meerkat/connector_cleanup": {"status": "failed"}}
                    elif not cancelled:
                        raise
            return result
        except asyncio.CancelledError:
            raise
        except CredentialRefused:
            # Only pre-operation structural refusal is unentered. A later
            # origin/auth refusal cannot erase an already entered sequence.
            return _failure("permission", effect=item["effect"],
                            entered=invoked or (call is not None and call.request_entered))
        except Exception:
            return _failure("operation_error", effect=item["effect"],
                            entered=invoked or (call is not None and call.request_entered))

    async def invoke(params, item, current):
        await anyio.lowlevel.checkpoint_if_cancelled()
        caller = asyncio.current_task()
        if caller is not None and caller.cancelling():
            raise asyncio.CancelledError()
        if current._close_task is not None:
            return _failure("closed", effect=item["effect"])
        effect, operation = item["effect"], item["operation"]
        invalid = False
        try:
            if params.task is not None or params.request_state is not None or params.input_responses:
                raise ValueError()
            arguments = {} if params.arguments is None else deepcopy(params.arguments)
            validators[params.name].validate(arguments)
            if operation == "read_interval":
                arguments.setdefault("max_result_bytes",
                    configuration.config.get("max_interval_result_bytes", 262144))
            if operation == "read_text_part" and re.fullmatch(r"[A-Za-z0-9_.-]*", arguments["part_id"]) is None:
                # JSON Schema's '$' can also match before a final newline.
                # Refuse that non-identity before the full-message request.
                raise ValueError()
            if configuration.profile == "google-drive" and operation in {"list_files_page", "get_file"}:
                fields = "id,name,description,mimeType,modifiedTime,size,webViewLink,shortcutDetails,capabilities"
                arguments["fields"] = ("nextPageToken,incompleteSearch,files(" + fields + ")"
                                       if operation == "list_files_page" else fields)
            if operation in {"send_message", "create_draft_message"}:
                arguments = {"mime": compose_message(
                    **arguments, max_message_bytes=configuration.config["max_message_bytes"],
                    allowed_recipient_addresses=configuration.config.get("allowed_recipient_addresses")
                )}
                operation = "send_raw" if operation == "send_message" else "create_draft_raw"
            elif operation in {"send_raw", "create_draft_raw"}:
                encoded = arguments.pop("mime_base64")
                mime = base64.b64decode(encoded, validate=True)
                if (not mime or len(mime) > configuration.config["max_message_bytes"]
                        or base64.b64encode(mime).decode("ascii") != encoded):
                    raise ValueError()
                arguments["mime"] = mime
        except RecipientPermissionError:
            return _failure("permission", effect=effect)
        except Exception:
            invalid = True
        if invalid:
            return _failure("invalid_arguments", effect=effect)
        try:
            if operation in {"read_message", "read_text_part"}:
                # One operation budget covers the explicit full-message read
                # and its optional selected external text-body read.
                async with asyncio.timeout(configuration.config["timeout_seconds"]):
                    message = await current.operations.get_message(arguments["message_id"], format="full")
                    maximum = configuration.config["max_response_bytes"]
                    if operation == "read_message":
                        value = readable_message(message, max_bytes=maximum)
                    else:
                        selection = select_text_part(message, arguments["part_id"], max_bytes=maximum)
                        external = None
                        if selection.body.get("attachmentId") is not None:
                            external = await current.operations.get_attachment(
                                arguments["message_id"], selection.body["attachmentId"])
                        value = readable_text_part(selection, external_body=external, max_bytes=maximum)
            else:
                value = await getattr(current.operations, operation)(**arguments)
        except asyncio.CancelledError:
            # Connector owners already joined cleanup. They sanitize their
            # cancellation exception, so re-observe an active MCP/AnyIO
            # scope here: its original cancellation must settle this one
            # request rather than look like a transport shutdown. A peer
            # cancellation sends no tool result; entered writes stay unknown.
            # Without an active scope, preserve the connector's exception.
            await anyio.lowlevel.checkpoint_if_cancelled()
            raise
        except (GmailError, SlackError, CalendarError, DriveError) as error:
            details = {key: getattr(error, key) for key in ("status_code", "retry_after_seconds")
                       if getattr(error, key) is not None}
            if isinstance(error, SlackError) and error.provider_code is not None:
                details["provider_code"] = error.provider_code
            if isinstance(error, GmailError) and error.provider_reason is not None:
                details["provider_reason"] = error.provider_reason
            return _failure(error.kind.value, effect=effect, entered=True, **details)
        except GmailContentError as error:
            return _failure("content_error", effect=effect, entered=True, content_code=error.code)
        except TimeoutError:
            return _failure("timeout" if operation in {"read_message", "read_text_part"} else "operation_error",
                            effect=effect, entered=True)
        except (TypeError, ValueError):
            # Once invoked, conservatively retain write uncertainty even
            # for unexpected errors raised after provider dispatch.
            return _failure("operation_arguments", effect=effect, entered=True)
        except Exception:
            return _failure("operation_error", effect=effect, entered=True)
        try:
            # Serialize after invocation under the same effect classification.
            # An encoding or model error must not turn an entered write safe.
            if isinstance(value, DriveContent):
                media = Message()
                media["content-type"] = value.mime_type
                mime = media.get_content_type()
                charset = media.get_content_charset() or "utf-8"
                text_value = None
                if (charset in {"utf-8", "utf8", "us-ascii", "ascii"}
                        and (mime.startswith("text/") or mime in {"application/json", "application/xml"}
                             or mime.endswith(("+json", "+xml")))):
                    try:
                        text_value = value.data.decode("ascii" if charset in {"us-ascii", "ascii"} else "utf-8")
                    except UnicodeError:
                        pass  # Preserve exact bytes as a blob; never decode with replacement.
                if (text_value is not None and len(json.dumps(text_value, ensure_ascii=False).encode("utf-8"))
                        > 4 * ((len(value.data) + 2) // 3) + 2):
                    # JSON escaping of control-heavy text can expand sixfold.
                    # Keep the exact bytes in a blob when that is more compact.
                    text_value = None
                uri = "gdrive:///files/" + quote(arguments["file_id"], safe="") + "/" + operation
                if operation == "export_content":
                    uri += "?mime_type=" + quote(arguments["mime_type"], safe="")
                resource = (types.TextResourceContents(uri=uri, mime_type=value.mime_type, text=text_value)
                            if text_value is not None else types.BlobResourceContents(
                                uri=uri, mime_type=value.mime_type,
                                blob=base64.b64encode(value.data).decode("ascii")))
                metadata = {"file_id": arguments["file_id"], "bytes": len(value.data),
                            "mime_type": value.mime_type, "representation": "text" if text_value is not None else "blob",
                            "resource_uri": uri}
                result = types.CallToolResult(content=[
                    types.TextContent(type="text", text=json.dumps(metadata, ensure_ascii=True)),
                    types.EmbeddedResource(type="resource", resource=resource)],
                    structured_content={"value": metadata})
                result.model_dump_json(by_alias=True, exclude_none=True).encode("utf-8")
                return result
            if operation == "read_interval":
                return _interval_tool_result(value, arguments["max_result_bytes"])
            text = json.dumps(value, ensure_ascii=True, allow_nan=False)
            result = types.CallToolResult(content=[types.TextContent(type="text", text=text)],
                                          structured_content={"value": value})
            result.model_dump_json(by_alias=True, exclude_none=True).encode("utf-8")
            return result
        except Exception:
            return _failure("invalid_result", effect=effect, entered=True)

    return Server(f"meerkat-toolkit-{configuration.profile}", version="0.1.8",
                  on_list_tools=list_tools, on_call_tool=call_tool)


async def serve_stdio(configuration: Configuration) -> None:
    from mcp.server.stdio import stdio_server

    async with stdio_server() as (read_stream, write_stream):
        profile = await create_profile(configuration)
        try:
            server = create_server(profile)
            await server.run(read_stream, write_stream, server.create_initialization_options())
        finally:
            await profile.aclose()


async def serve_native_stdio(configuration: Configuration) -> None:
    from mcp.server.stdio import stdio_server
    from ._connector_call import private_diagnostics

    with private_diagnostics():
        async with stdio_server() as (read_stream, write_stream):
            server = create_native_server(configuration)
            # Server.run joins its request tasks. Each native call retains its
            # client close before its existing admission owner releases it.
            await server.run(read_stream, write_stream, server.create_initialization_options())


class _Parser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        raise ConfigurationError()


def main(argv: list[str] | None = None) -> int:
    parser = _Parser(description=__doc__, allow_abbrev=False)
    parser.add_argument("mode", choices=("profiles", "config-help", "check-config", "describe", "serve", "serve-native"))
    parser.add_argument("--profile")
    parser.add_argument("--config")
    failed = False
    mode = None
    diagnostic = None
    try:
        arguments = parser.parse_args(argv)
        mode = arguments.mode
        if arguments.mode == "profiles":
            if arguments.profile is not None or arguments.config is not None:
                raise ConfigurationError()
            result = profiles()
        elif arguments.mode == "config-help":
            if arguments.profile is None or arguments.config is not None:
                raise ConfigurationError()
            result = configuration_help(arguments.profile)
        else:
            if arguments.config is None or arguments.profile is not None:
                raise ConfigurationError()
            configuration = load_configuration(arguments.config)
            if arguments.mode in {"serve", "serve-native"}:
                import anyio

                serve = serve_native_stdio if arguments.mode == "serve-native" else serve_stdio
                anyio.run(serve, configuration, backend="asyncio")
                return 0
            if arguments.mode == "check-config":
                # These are configured names, not a lookup or permission to
                # overwrite the host's protected process environment.
                references = [
                    {"field_path": ["config", name], "environment_variable": configuration.config[name]}
                    for name, field in configuration_help(configuration.profile)["config_fields"].items()
                    if field.get("credential_reference", False) and name in configuration.config
                ]
                result = {"profile": configuration.profile, "valid": True, "scope": "configuration_only",
                          "credential_environment_references": references}
            else:
                result = {"profile": configuration.profile, "tools": tool_descriptions(configuration)}
        print(json.dumps(result, allow_nan=False))
    except ConfigurationError as error:
        failed = True
        if mode == "check-config":
            diagnostic = error.diagnostic
    except (Exception, KeyboardInterrupt):
        failed = True
    if failed:
        if diagnostic is not None:
            print(json.dumps({"valid": False, "scope": "configuration_only", "diagnostic": diagnostic},
                             allow_nan=False))
        print("Service MCP command failed; check the selected configuration and dependencies.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
