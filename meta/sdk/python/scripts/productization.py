#!/usr/bin/env python3

from __future__ import annotations

import asyncio
import inspect
import json
import os
import sys
from pathlib import Path
from typing import Any, Awaitable, Callable, cast
from urllib.error import URLError

try:
    from meerkat_meta._client import MetaAsyncTypedClient, MetaRpcError
    from meerkat_meta.helpers import (
        ModuleSpec,
        build_console_experience_route,
        build_console_modules_route,
        build_console_route,
        build_console_routes,
        build_module_spec,
        decorate_module_spec,
        define_module,
        define_module_spec,
        define_module_tool,
    )
except ModuleNotFoundError:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from meerkat_meta._client import MetaAsyncTypedClient, MetaRpcError  # type: ignore[no-redef]
    from meerkat_meta.helpers import (  # type: ignore[no-redef]
        ModuleSpec,
        build_console_experience_route,
        build_console_modules_route,
        build_console_route,
        build_console_routes,
        build_module_spec,
        decorate_module_spec,
        define_module,
        define_module_spec,
        define_module_tool,
    )


async def main() -> int:
    checks: list[dict[str, Any]] = []
    gateway_bin = os.environ.get("META_RPC_GATEWAY_BIN")
    client_module = sys.modules[MetaAsyncTypedClient.__module__]
    expected_contract_version = "0.5.0"

    async def check(name: str, fn: Callable[[], Awaitable[None]]) -> None:
        try:
            await fn()
            checks.append({"name": name, "ok": True})
        except Exception as exc:  # broad for per-check reporting
            checks.append({"name": name, "ok": False, "error": str(exc)})

    async def transport(request: dict[str, Any]) -> dict[str, Any]:
        if request.get("jsonrpc") != "2.0" or not isinstance(request.get("id"), str):
            raise ValueError("invalid JSON-RPC request")

        request_id = request["id"]
        method = request.get("method")
        params = request.get("params") if isinstance(request.get("params"), dict) else {}

        if method == "meta/status":
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "contract_version": expected_contract_version,
                    "running": True,
                    "loaded_modules": ["routing", "delivery"],
                },
            }

        if method == "meta/capabilities":
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "contract_version": expected_contract_version,
                    "methods": [
                        "meta/status",
                        "meta/capabilities",
                        "meta/reconcile",
                        "meta/spawn_member",
                        "meta/events/subscribe",
                    ],
                    "loaded_modules": ["routing", "delivery"],
                },
            }

        if method == "meta/reconcile":
            modules = [
                item for item in params.get("modules", []) if isinstance(item, str)
            ]
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "accepted": True,
                    "reconciled_modules": modules,
                    "added": 1 if "delivery" in modules else 0,
                },
            }

        if method == "meta/spawn_member":
            module_id = params.get("module_id")
            if not isinstance(module_id, str) or not module_id.strip():
                return {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32602,
                        "message": "Invalid params: module_id required",
                    },
                }
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "accepted": True,
                    "module_id": module_id.strip(),
                },
            }

        if method == "meta/events/subscribe":
            scope = params.get("scope")
            if scope not in {"mob", "agent", "interaction"}:
                scope = "mob"
            if scope == "agent" and not isinstance(params.get("agent_id"), str):
                return {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": -32602,
                        "message": "Invalid params: scope=agent requires non-empty agent_id",
                    },
                }
            return {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "scope": scope,
                    "replay_from_event_id": params.get("last_event_id"),
                    "keep_alive": {"interval_ms": 15000, "event": "keep-alive"},
                    "keep_alive_comment": ": keep-alive\n\n",
                    "event_frames": [
                        "id: evt-routing\nevent: ready\ndata: {\"kind\":\"module\"}\n\n"
                    ],
                    "events": [
                        {
                            "event_id": "evt-routing",
                            "source": "module",
                            "timestamp_ms": 101,
                            "event": {
                                "kind": "module",
                                "module": "routing",
                                "event_type": "ready",
                                "payload": {"ok": True},
                            },
                        }
                    ],
                },
            }

        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {
                "code": -32601,
                "message": f"Method not found: {method}",
            },
        }

    client = MetaAsyncTypedClient(transport)

    async def status_check() -> None:
        status = await client.status("py-prod-status")
        _assert_eq(
            status,
            {
                "contract_version": expected_contract_version,
                "running": True,
                "loaded_modules": ["routing", "delivery"],
            },
            "unexpected status",
        )

    await check("async client status typed result", status_check)

    async def caps_check() -> None:
        caps = await client.capabilities("py-prod-caps")
        methods = caps["methods"]
        if "meta/events/subscribe" not in methods:
            raise AssertionError(
                f"missing events subscribe capability: {methods}"
            )
        if "meta/reconcile" not in methods or "meta/spawn_member" not in methods:
            raise AssertionError(f"missing key methods: {methods}")

    await check("async client capabilities typed result", caps_check)

    async def reconcile_check() -> None:
        reconcile = await client.reconcile(
            ["routing", "delivery"], "py-prod-reconcile"
        )
        _assert_eq(
            reconcile,
            {
                "accepted": True,
                "reconciled_modules": ["routing", "delivery"],
                "added": 1,
            },
            "unexpected reconcile",
        )

    await check("async client reconcile typed result", reconcile_check)

    async def spawn_check() -> None:
        spawned = await client.spawn_member("delivery", "py-prod-spawn")
        _assert_eq(
            spawned,
            {"accepted": True, "module_id": "delivery"},
            "unexpected spawn_member",
        )

    await check("async client spawn_member typed result", spawn_check)

    async def events_check() -> None:
        subscribed = await client.subscribe_events(
            {"scope": "mob"}, "py-prod-events"
        )
        if subscribed["scope"] != "mob":
            raise AssertionError(f"unexpected scope: {subscribed['scope']}")
        if len(subscribed["event_frames"]) != 1:
            raise AssertionError(
                f"unexpected event frames: {subscribed['event_frames']}"
            )
        if len(subscribed["events"]) != 1:
            raise AssertionError(f"unexpected events: {subscribed['events']}")

    await check("async client events subscribe typed shape", events_check)

    async def rpc_error_check() -> None:
        observed: Exception | None = None
        try:
            await client.spawn_member("", "py-prod-invalid-spawn")
        except Exception as exc:  # broad for explicit type assertion
            observed = exc

        if not isinstance(observed, MetaRpcError):
            raise AssertionError(f"expected MetaRpcError, got {observed}")
        if observed.code != -32602 or observed.method != "meta/spawn_member":
            raise AssertionError(
                f"unexpected rpc error metadata: code={observed.code} method={observed.method} request_id={observed.request_id}"
            )

    await check("async client rpc errors surface typed metadata", rpc_error_check)

    async def gateway_factory_status_check() -> None:
        if not gateway_bin:
            raise AssertionError(
                "META_RPC_GATEWAY_BIN must be set for from_gateway_bin checks"
            )
        factory_client = MetaAsyncTypedClient.from_gateway_bin(gateway_bin)
        status = await factory_client.status("py-prod-factory-gateway-status")
        _assert_eq(
            status,
            {
                "contract_version": expected_contract_version,
                "running": True,
                "loaded_modules": ["routing"],
            },
            "unexpected from_gateway_bin status",
        )

    await check("async factory fromGatewayBin status success", gateway_factory_status_check)

    async def gateway_factory_error_check() -> None:
        factory_client = MetaAsyncTypedClient.from_gateway_bin(
            "/__meta__/missing/sdk-test-gateway"
        )
        observed: Exception | None = None
        try:
            await factory_client.status("py-prod-factory-gateway-missing")
        except Exception as exc:  # broad for explicit assertion
            observed = exc

        if observed is None:
            raise AssertionError("expected transport exception for missing gateway binary")

        message = str(observed)
        if (
            not isinstance(observed, FileNotFoundError)
            and "No such file or directory" not in message
            and "not found" not in message.lower()
        ):
            raise AssertionError(f"unexpected transport error message: {message}")

    await check(
        "async factory fromGatewayBin transport errors surface",
        gateway_factory_error_check,
    )

    async def http_factory_status_check() -> None:
        observed: dict[str, Any] = {}

        def fake_read_http_body(http_request: Any, timeout_seconds: float) -> str:
            observed["url"] = http_request.full_url
            observed["method"] = http_request.get_method()
            observed["timeout_seconds"] = timeout_seconds
            observed["headers"] = {
                key.lower(): value for key, value in http_request.header_items()
            }
            payload = json.loads((http_request.data or b"{}").decode("utf-8"))
            observed["payload"] = payload
            return json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": payload["id"],
                    "result": {
                        "contract_version": expected_contract_version,
                        "running": True,
                        "loaded_modules": ["routing", "delivery"],
                    },
                }
            )

        original_read_http_body = getattr(client_module, "_read_http_body")
        setattr(client_module, "_read_http_body", fake_read_http_body)
        try:
            factory_client = MetaAsyncTypedClient.from_http(
                "https://meta.local/rpc",
                headers={"x-sdk-productization": "true"},
                timeout_seconds=0.25,
            )
            status = await factory_client.status("py-prod-factory-http-status")
        finally:
            setattr(client_module, "_read_http_body", original_read_http_body)

        _assert_eq(
            status,
            {
                "contract_version": expected_contract_version,
                "running": True,
                "loaded_modules": ["routing", "delivery"],
            },
            "unexpected from_http status",
        )
        if observed.get("url") != "https://meta.local/rpc":
            raise AssertionError(f"unexpected from_http endpoint: {observed}")
        if observed.get("method") != "POST":
            raise AssertionError(f"unexpected from_http method: {observed}")
        if observed.get("timeout_seconds") != 0.25:
            raise AssertionError(f"unexpected from_http timeout: {observed}")
        headers = cast(dict[str, str], observed.get("headers") or {})
        if headers.get("x-sdk-productization") != "true":
            raise AssertionError(f"missing custom header in from_http request: {headers}")
        payload = cast(dict[str, Any], observed.get("payload") or {})
        if payload.get("method") != "meta/status":
            raise AssertionError(f"unexpected from_http rpc payload: {payload}")

    await check("async factory fromHttp status success", http_factory_status_check)

    async def http_factory_error_check() -> None:
        def failing_read_http_body(_http_request: Any, _timeout_seconds: float) -> str:
            raise URLError("connection refused")

        original_read_http_body = getattr(client_module, "_read_http_body")
        observed: Exception | None = None
        setattr(client_module, "_read_http_body", failing_read_http_body)
        try:
            factory_client = MetaAsyncTypedClient.from_http(
                "https://meta.local/rpc"
            )
            try:
                await factory_client.status("py-prod-factory-http-error")
            except Exception as exc:  # broad for explicit assertion
                observed = exc
        finally:
            setattr(client_module, "_read_http_body", original_read_http_body)

        if not isinstance(observed, RuntimeError):
            raise AssertionError(f"expected RuntimeError, got {observed}")
        message = str(observed)
        if "http transport failed" not in message:
            raise AssertionError(f"unexpected transport error message: {message}")

    await check(
        "async factory fromHttp transport errors surface",
        http_factory_error_check,
    )

    async def route_helpers_check() -> None:
        modules = build_console_modules_route("token+/=?")
        experience = build_console_experience_route("token+/=?")
        routes = build_console_routes("token+/=?")
        explicit = build_console_route("/console/modules", "token+/=?")

        if modules != "/console/modules?auth_token=token%2B%2F%3D%3F":
            raise AssertionError(f"unexpected modules route: {modules}")
        if experience != "/console/experience?auth_token=token%2B%2F%3D%3F":
            raise AssertionError(f"unexpected experience route: {experience}")
        if explicit != modules:
            raise AssertionError(
                f"explicit route helper mismatch: {explicit} vs {modules}"
            )
        _assert_eq(
            routes,
            {"modules": modules, "experience": experience},
            "unexpected route map",
        )

    await check(
        "console route helpers expose modules and experience routes",
        route_helpers_check,
    )

    async def module_helpers_check() -> None:
        base_spec = build_module_spec(
            module_id="routing",
            command="python3",
            args=["routing.py"],
            restart_policy="never",
        )
        compat_dict = define_module_spec(
            module_id="routing",
            command="python3",
            args=["routing.py"],
            restart_policy="never",
        )
        _assert_eq(
            compat_dict,
            {
                "id": "routing",
                "command": "python3",
                "args": ["routing.py"],
                "restart_policy": "never",
            },
            "compat module spec mismatch",
        )
        env_dict = define_module_spec(
            module_id="routing",
            command="python3",
            args=["routing.py"],
            boundary="mcp",
        )
        _assert_eq(
            env_dict["boundary"],
            "mcp",
            "module boundary should be preserved for gateway init",
        )

        decorated_spec = decorate_module_spec(
            base_spec,
            lambda spec: ModuleSpec(
                id=spec.id,
                command=spec.command,
                args=spec.args + ("--prod",),
                restart_policy="on_failure",
                boundary=spec.boundary,
                env=spec.env,
            ),
        )

        def add_decorator(next_handler: Callable[[Any, dict[str, Any]], Any]):
            async def wrapped(payload: Any, context: dict[str, Any]) -> Any:
                result = await _await_if_needed(next_handler(payload, context))
                if not isinstance(result, dict):
                    raise AssertionError(
                        f"unexpected tool result type: {type(result)}"
                    )
                return {**result, "decorated": True}

            return wrapped

        async def tool_handler(payload: Any, context: dict[str, Any]) -> dict[str, Any]:
            return {
                "module_id": context.get("module_id"),
                "request_id": context.get("request_id"),
                "probe": payload.get("probe"),
            }

        tool = define_module_tool(
            name="health",
            description="returns module health",
            handler=tool_handler,
            decorators=[add_decorator],
        )

        definition = define_module(
            spec=decorated_spec,
            description="routing module",
            tools=[tool],
        )
        tool_result = await _await_if_needed(
            definition.tools[0].handler(
                {"probe": "ready"},
                {"module_id": "routing", "request_id": "tool-1"},
            )
        )

        _assert_eq(
            definition.spec.to_dict(),
            {
                "id": "routing",
                "command": "python3",
                "args": ["routing.py", "--prod"],
                "restart_policy": "on_failure",
            },
            "unexpected decorated spec",
        )
        _assert_eq(
            tool_result,
            {
                "module_id": "routing",
                "request_id": "tool-1",
                "probe": "ready",
                "decorated": True,
            },
            "unexpected decorated tool result",
        )

    await check(
        "module authoring helpers support base structures and decorators",
        module_helpers_check,
    )

    failed = sum(1 for c in checks if not c["ok"])
    passed = len(checks) - failed
    print(
        json.dumps(
            {
                "sdk": "python",
                "suite": "productization",
                "passed": passed,
                "failed": failed,
                "checks": checks,
            }
        ),
        end="",
    )
    return 0 if failed == 0 else 1


async def _await_if_needed(value: Any) -> Any:
    if inspect.isawaitable(value):
        return await cast(Awaitable[Any], value)
    return value


def _assert_eq(observed: Any, expected: Any, label: str) -> None:
    if observed != expected:
        raise AssertionError(f"{label}: observed={observed} expected={expected}")


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
