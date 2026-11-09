"""Asyncio-only MCP 2.2 stdio exposure for explicit Agno profiles."""

from __future__ import annotations

import argparse
import asyncio
from contextlib import redirect_stdout
from copy import deepcopy
import functools
import json
import sys
from typing import TYPE_CHECKING, Any, Callable

from ._mcp_admission import AdmissionRefused, CallAdmission
from .agno_profiles import (
    AgnoMcpProfile, ProfileConfiguration, build_profile, load_configuration,
    configuration_help, list_profiles,
)

if TYPE_CHECKING:
    from .agno import AgnoAdapterError
    from mcp import types
    from mcp.server import Server


def _error_result(error: AgnoAdapterError) -> types.CallToolResult:
    from mcp import types

    metadata = {
        "kind": error.kind, "effect": error.effect, "entered": error.entered,
        "write_outcome": error.write_outcome,
    }
    return types.CallToolResult(
        content=[types.TextContent(type="text", text=f"Agno adapter {error.kind}")],
        structured_content={"error": metadata}, is_error=True,
    )


def _invoke(profile: AgnoMcpProfile, name: str, arguments: dict[str, Any]) -> types.CallToolResult:
    from mcp import types
    from .agno import AgnoAdapterError

    error = None
    effect = next((d["effect"] for d in profile.descriptions if d["name"] == name), None)
    try:
        value = profile.adapter.invoke(name, arguments)
    except AgnoAdapterError as exc:
        error = exc
    except BaseException:
        # A synchronous control exception cannot safely terminate an unrelated
        # event-loop task. The invoked boundary may already have had effects.
        error = AgnoAdapterError("entrypoint_error", effect=effect, entered=True)
    if error is not None:
        return _error_result(error)
    upstream_error = type(value) is str and value.startswith(profile.error_prefixes.get(name, ()))
    if name in profile.json_error_operations and type(value) is str:
        try:
            parsed = json.loads(value)
            upstream_error = type(parsed) is dict and set(parsed) == {"error"}
        except (ValueError, RecursionError):
            pass
    if upstream_error:
        # These exact failure conventions belong to the reviewed service
        # profiles, including exact single-key JSON error objects. Their
        # provider diagnostics can contain contact/query data.
        return types.CallToolResult(
            content=[types.TextContent(type="text", text="Upstream operation failed")],
            structured_content={"error": {
                "kind": "upstream_operation_error", "effect": effect, "entered": True,
                "write_outcome": "not_applicable" if effect == "read" else "unknown",
            }}, is_error=True,
        )
    text = value if type(value) is str else json.dumps(value, ensure_ascii=False, allow_nan=False)
    return types.CallToolResult(
        content=[types.TextContent(type="text", text=text)],
        structured_content={"value": value},
    )


async def _joined_worker(function: Callable[[], Any]) -> Any:
    import anyio

    # Match the SDK's owned synchronous-I/O pattern: direct Task.cancel is not
    # covered by AnyIO's worker cancellation option alone. Keep the owner and
    # join its actual completion, including repeated cancellation.
    with anyio.CancelScope(shield=True):
        owner = asyncio.create_task(anyio.to_thread.run_sync(function, abandon_on_cancel=False))
        cancellation = None
        while not owner.done():
            try:
                await asyncio.shield(owner)
            except asyncio.CancelledError as exc:
                cancellation = exc
            except Exception:
                break
        if cancellation is not None:
            if not owner.cancelled():
                owner.exception()
            raise cancellation
        return owner.result()


def create_server(profile: AgnoMcpProfile) -> Server:
    """Expose one explicit profile; run only under an asyncio MCP host.

    The host must declare its toolkit safe for serialized worker handoff.
    This is not a claim about arbitrary upstream clients or their affinity.
    The creator retains profile ownership and closes it after this server and
    all its calls have joined. This function does not take resource ownership.
    """
    if profile.thread_handoff_safe is not True:
        raise ValueError("Agno profile does not support worker handoff")
    return _create_server(profile.name, profile.descriptions, profile=profile)


def create_native_server(configuration: ProfileConfiguration) -> Server:
    """Expose Salesforce/get_record with private native metadata required per call."""
    from .agno_profiles import _salesforce_native_descriptions, validate_configuration
    from ._connector_call import ConnectorBinding, protect_server

    configuration = validate_configuration(configuration.name,
        operations=configuration.operations, config=configuration.config)
    binding = ConnectorBinding.from_environment()
    descriptions = _salesforce_native_descriptions(configuration)
    return protect_server(_create_server(configuration.name, descriptions,
        configuration=configuration, binding=binding))


def _create_server(name, selected_descriptions, *, profile=None, configuration=None, binding=None) -> Server:
    import anyio
    from jsonschema import Draft202012Validator
    from mcp import types
    from mcp.server import Server
    from referencing import Registry

    descriptions = {item["name"]: deepcopy(item) for item in selected_descriptions}
    tools = [types.Tool(
        name=item["name"], description=item["description"], input_schema=item["parameters"],
        annotations=types.ToolAnnotations(read_only_hint=item["effect"] == "read"),
    ) for item in descriptions.values()]
    validators = {
        name: Draft202012Validator(item["parameters"], registry=Registry())
        for name, item in descriptions.items()
    }
    admission = CallAdmission(asyncio.Lock())

    async def list_tools(_ctx, _params) -> types.ListToolsResult:
        return types.ListToolsResult(tools=deepcopy(tools))

    async def call_tool(context, params: types.CallToolRequestParams) -> types.CallToolResult:
        from .agno import AgnoAdapterError

        asyncio.get_running_loop()
        if params.name not in descriptions:
            return _error_result(AgnoAdapterError("unknown_function"))
        effect = descriptions[params.name]["effect"]
        try:
            async with admission.enter():
                return await invoke(context, params, effect)
        except AdmissionRefused as error:
            assert error.kind != "closed"  # This gate has no close predicate or lifecycle ownership.
            return _error_result(AgnoAdapterError(error.kind, effect=effect))

    async def invoke(context, params: types.CallToolRequestParams, effect) -> types.CallToolResult:
        from .agno import AgnoAdapterError

        await anyio.lowlevel.checkpoint_if_cancelled()
        caller = asyncio.current_task()
        if caller is not None and caller.cancelling():
            raise asyncio.CancelledError()
        arguments = {} if params.arguments is None else params.arguments
        invalid = False
        try:
            if params.task is not None or params.request_state is not None or params.input_responses:
                raise ValueError()
            validators[params.name].validate(arguments)
        except Exception:
            invalid = True
        if invalid:
            return _error_result(AgnoAdapterError("invalid_arguments", effect=effect))
        if profile is not None:
            return await _joined_worker(functools.partial(_invoke, profile, params.name, arguments))
        from ._connector_call import ConnectorCall, CredentialRefused
        try:
            call = ConnectorCall(context, binding, params.name, configuration.config["instance_url"])
        except CredentialRefused:
            return _error_result(AgnoAdapterError("invalid_arguments", effect=effect))

        def native_worker():
            from .agno_profiles import build_salesforce_profile_from_client
            invoked = False
            result = None
            try:
                with call.requests_session() as session:
                    current = build_salesforce_profile_from_client(configuration, session)
                    try:
                        invoked = True
                        result = _invoke(current, params.name, arguments)
                    finally:
                        current.close()
            except BaseException:
                if result is None:
                    return _error_result(AgnoAdapterError("entrypoint_error", effect=effect,
                                                          entered=invoked or call.request_entered))
                # The synchronous owner has returned, but cleanup failed.
                result.meta = {**(result.meta or {}),
                    "meerkat/connector_cleanup": {"status": "failed"}}
            return result

        return await _joined_worker(native_worker)

    return Server(
        f"meerkat-toolkit-{name}", version="0.1.0",
        on_list_tools=list_tools, on_call_tool=call_tool,
    )


async def serve_stdio(configuration: ProfileConfiguration) -> None:
    """Claim stdio before importing/constructing the selected upstream toolkit."""
    from mcp.server.stdio import stdio_server

    async with stdio_server() as (read_stream, write_stream):
        profile = None

        def construct() -> None:
            nonlocal profile
            # Retain a completed profile even when cancellation prevents the
            # joined worker from returning its result to this task.
            profile = build_profile(
                configuration.name, operations=configuration.operations, config=configuration.config,
            )

        try:
            await _joined_worker(construct)
            server = create_server(profile)
            await server.run(read_stream, write_stream, server.create_initialization_options())
        finally:
            # Construction and MCP 2.2 calls join their workers before this
            # owner closes the profile, including cancellation during startup.
            if profile is not None:
                await _joined_worker(profile.close)


async def serve_native_stdio(configuration: ProfileConfiguration) -> None:
    from mcp.server.stdio import stdio_server
    from ._connector_call import private_diagnostics

    with private_diagnostics():
        async with stdio_server() as (read_stream, write_stream):
            # Catalog construction reads no token. Each request owns its
            # prepared client/profile entirely within the joined worker.
            server = create_native_server(configuration)
            await server.run(read_stream, write_stream, server.create_initialization_options())


class _ArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        # Invalid arguments may contain credentials or account information.
        raise ValueError("Invalid command arguments")


def main(argv: list[str] | None = None) -> int:
    parser = _ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("mode", choices=("profiles", "config-help", "check-config", "describe", "serve", "serve-native"))
    parser.add_argument("--config", help="explicit local JSON profile configuration")
    parser.add_argument("--profile", help="exact supported profile ID")
    failed = False
    try:
        args = parser.parse_args(argv)
        if args.mode == "profiles":
            if args.config is not None or args.profile is not None:
                raise ValueError()
            print(json.dumps(list_profiles(), allow_nan=False))
            return 0
        if args.mode == "config-help":
            if args.profile is None or args.config is not None:
                raise ValueError()
            print(json.dumps(configuration_help(args.profile), allow_nan=False))
            return 0
        if args.config is None or args.profile is not None:
            raise ValueError()
        configuration = load_configuration(args.config)
        if args.mode == "check-config":
            # Descriptive selected names only. The host owns credential lookup
            # and admission of any environment injection, including aliases.
            references = [
                {"field_path": ["config", field["name"]],
                 "environment_variable": configuration.config[field["name"]]}
                for field in configuration_help(configuration.name)["config_fields"]
                if field["credential_reference"] and field["name"] in configuration.config
            ]
            print(json.dumps({"profile": configuration.name, "valid": True, "scope": "configuration_only",
                              "credential_environment_references": references}))
            return 0
        if args.mode == "describe":
            with redirect_stdout(sys.stderr):
                profile = build_profile(
                    configuration.name, operations=configuration.operations,
                    config=configuration.config,
                )
                try:
                    descriptions = profile.descriptions
                finally:
                    profile.close()
            print(json.dumps({"profile": profile.name, "tools": descriptions}, allow_nan=False))
        else:
            import anyio

            serve = serve_native_stdio if args.mode == "serve-native" else serve_stdio
            anyio.run(serve, configuration, backend="asyncio")
    except (Exception, KeyboardInterrupt):
        failed = True
    if failed:
        # Config, dependency and startup errors never reflect file contents,
        # account information, upstream exceptions or raw arguments to stdout.
        print("Agno MCP command failed; check the selected profile and dependencies.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
