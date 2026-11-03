#!/usr/bin/env python3

from __future__ import annotations

from dataclasses import asdict, is_dataclass
import os
import sys
from pathlib import Path
from typing import Any, Literal

from fastapi import FastAPI
from pydantic import BaseModel, Field

try:
    from meerkat_meta import Meta, MetaRuntime, RpcError
except ModuleNotFoundError:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from meerkat_meta import (  # type: ignore[no-redef]
        Meta,
        MetaRuntime,
        RpcError,
    )


class RpcRequestIdPayload(BaseModel):
    request_id: str = Field(min_length=1)


class ReconcilePayload(BaseModel):
    request_id: str = Field(min_length=1)
    modules: list[str] = Field(default_factory=list)


class SpawnMemberPayload(BaseModel):
    request_id: str = Field(min_length=1)
    module_id: str | None = None


class EventsSubscribePayload(BaseModel):
    request_id: str = Field(min_length=1)
    scope: Literal["mob", "agent", "interaction"] = "mob"
    last_event_id: str | None = None
    agent_id: str | None = None


app = FastAPI(title="Meta H2 Python RPC-Mode Reference App")

H2_REFERENCE_MOB_TOML = """
[mob]
id = "h2-reference-app"

[profiles.default]
model = "gpt-5.4"
external_addressable = true
"""

H2_REFERENCE_ROUTING_MODULE = {
    "id": "routing",
    "command": "sh",
    "args": [
        "-c",
        "printf '%s\\n' '{\"event_id\":\"evt-routing\",\"source\":\"module\",\"timestamp_ms\":101,"
        "\"event\":{\"kind\":\"module\",\"module\":\"routing\",\"event_type\":\"ready\",\"payload\":"
        "{\"family\":\"routing\",\"health\":{\"state\":\"healthy\"},\"tools\":{\"list_method\":\"routing/tools.list\","
        "\"representative_call\":{\"method\":\"routing/tool.call\",\"params_schema\":{\"tool\":\"string\",\"input\":\"json\"}}}}}}'",
    ],
    "restart_policy": "never",
}


def _required_gateway_bin() -> str:
    gateway_bin = os.environ.get("META_RPC_GATEWAY_BIN")
    if not gateway_bin:
        raise RuntimeError("META_RPC_GATEWAY_BIN must be set for h2_reference_app")
    return gateway_bin


def _runtime() -> MetaRuntime:
    rt = getattr(app.state, "meta_runtime", None)
    if not isinstance(rt, MetaRuntime):
        raise RuntimeError("Meta runtime is not initialized")
    return rt


async def _typed_call_result(awaitable: Any) -> tuple[Any | None, dict[str, Any] | None]:
    try:
        return await awaitable, None
    except RpcError as exc:
        return None, {
            "code": exc.code,
            "message": str(exc),
            "request_id": exc.request_id,
            "method": exc.method,
        }
    except Exception as exc:  # broad for transparent route diagnostics
        return None, {"kind": type(exc).__name__, "message": str(exc)}


def _to_wire_value(value: Any) -> Any:
    if is_dataclass(value):
        return asdict(value)
    if isinstance(value, list):
        return [_to_wire_value(item) for item in value]
    if isinstance(value, dict):
        return {key: _to_wire_value(inner) for key, inner in value.items()}
    return value


def _jsonrpc_envelope(
    request_id: str,
    typed_result: Any | None,
    typed_error: dict[str, Any] | None,
) -> dict[str, Any]:
    if typed_error is not None and {"code", "message"} <= typed_error.keys():
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {
                "code": typed_error["code"],
                "message": typed_error["message"],
            },
        }
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "result": _to_wire_value(typed_result),
    }


@app.on_event("startup")
async def startup_event() -> None:
    gateway_bin = _required_gateway_bin()
    app.state.gateway_bin = gateway_bin
    rt = await (
        Meta.builder()
        .mob_inline(H2_REFERENCE_MOB_TOML)
        .gateway(gateway_bin)
        .modules([H2_REFERENCE_ROUTING_MODULE])
        .build()
    )
    await rt.connect()
    app.state.meta_runtime = rt


@app.get("/healthz")
async def healthz() -> dict[str, Any]:
    return {
        "ok": True,
        "gateway_bin": getattr(app.state, "gateway_bin", None),
    }


@app.post("/rpc/status")
async def rpc_status(payload: RpcRequestIdPayload) -> dict[str, Any]:
    handle = _runtime().mob_handle()
    typed_result, typed_error = await _typed_call_result(handle.status())
    return {
        "route": "meta/status",
        "jsonrpc_envelope": _jsonrpc_envelope(
            payload.request_id, typed_result, typed_error
        ),
        "typed_result": _to_wire_value(typed_result),
        "typed_error": typed_error,
    }


@app.post("/rpc/capabilities")
async def rpc_capabilities(payload: RpcRequestIdPayload) -> dict[str, Any]:
    handle = _runtime().mob_handle()
    typed_result, typed_error = await _typed_call_result(handle.capabilities())
    return {
        "route": "meta/capabilities",
        "jsonrpc_envelope": _jsonrpc_envelope(
            payload.request_id, typed_result, typed_error
        ),
        "typed_result": _to_wire_value(typed_result),
        "typed_error": typed_error,
    }


@app.post("/rpc/reconcile")
async def rpc_reconcile(payload: ReconcilePayload) -> dict[str, Any]:
    handle = _runtime().mob_handle()
    typed_result, typed_error = await _typed_call_result(
        handle.reconcile(payload.modules)
    )
    return {
        "route": "meta/reconcile",
        "jsonrpc_envelope": _jsonrpc_envelope(
            payload.request_id, typed_result, typed_error
        ),
        "typed_result": _to_wire_value(typed_result),
        "typed_error": typed_error,
    }


@app.post("/rpc/spawn_member")
async def rpc_spawn_member(payload: SpawnMemberPayload) -> dict[str, Any]:
    handle = _runtime().mob_handle()
    typed_result, typed_error = await _typed_call_result(
        handle.spawn_member(payload.module_id or "")
    )
    return {
        "route": "meta/spawn_member",
        "jsonrpc_envelope": _jsonrpc_envelope(
            payload.request_id, typed_result, typed_error
        ),
        "typed_result": _to_wire_value(typed_result),
        "typed_error": typed_error,
    }


@app.post("/rpc/events/subscribe")
async def rpc_events_subscribe(payload: EventsSubscribePayload) -> dict[str, Any]:
    handle = _runtime().mob_handle()
    typed_result, typed_error = await _typed_call_result(
        handle.subscribe_events(
            scope=payload.scope,
            last_event_id=payload.last_event_id,
            agent_id=payload.agent_id,
        )
    )
    return {
        "route": "meta/events/subscribe",
        "jsonrpc_envelope": _jsonrpc_envelope(
            payload.request_id, typed_result, typed_error
        ),
        "typed_result": _to_wire_value(typed_result),
        "typed_error": typed_error,
    }


@app.get("/flow/reference")
async def flow_reference() -> dict[str, Any]:
    handle = _runtime().mob_handle()
    status = await handle.status()
    capabilities = await handle.capabilities()
    reconcile = await handle.reconcile(["routing"])
    spawn_member = await handle.spawn_member("routing")
    events_subscribe = await handle.subscribe_events(scope="mob")

    return {
        "route": "h2-flow",
        "status": _jsonrpc_envelope("h2-flow-status", status, None),
        "capabilities": _jsonrpc_envelope("h2-flow-capabilities", capabilities, None),
        "reconcile": _jsonrpc_envelope("h2-flow-reconcile", reconcile, None),
        "spawn_member": _jsonrpc_envelope("h2-flow-spawn", spawn_member, None),
        "events_subscribe": _jsonrpc_envelope("h2-flow-events", events_subscribe, None),
        "typed": {
            "status": _to_wire_value(status),
            "capabilities": _to_wire_value(capabilities),
            "reconcile": _to_wire_value(reconcile),
            "spawn_member": _to_wire_value(spawn_member),
            "events_subscribe": _to_wire_value(events_subscribe),
        },
    }
