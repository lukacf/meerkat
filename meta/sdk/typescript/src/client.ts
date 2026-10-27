/**
 * Low-level JSON-RPC clients for Meta.
 *
 * These are the original typed client classes preserved for backward
 * compatibility. For new code, prefer the high-level {@link MobHandle} API
 * via `runtime.mobHandle()`.
 */

import { RpcError } from "./errors.js";
import {
  buildJsonRpcRequest,
  createGatewaySyncTransport,
  createGatewayAsyncTransport,
  createJsonRpcHttpTransport,
  type JsonRpcRequest,
  type JsonRpcResponse,
  type JsonRpcTransport,
  type JsonRpcSyncTransport,
  type FetchLike,
} from "./transport.js";

// -- Wire-format result types (snake_case, backward compat) ---------------

export type MetaStatusResult = {
  contract_version: string;
  running: boolean;
  loaded_modules: string[];
};

export type MetaCapabilitiesResult = {
  contract_version: string;
  feature_capabilities?: string[];
  methods: string[];
  loaded_modules: string[];
};

export type MetaReconcileResult = {
  accepted: boolean;
  reconciled_modules: string[];
  added: number;
};

export type MetaSpawnMemberResult = {
  accepted: boolean;
  module_id: string;
};

export type MetaSubscribeScope = "mob" | "agent" | "interaction";

export type MetaSubscribeParams = {
  scope?: MetaSubscribeScope;
  last_event_id?: string;
  agent_id?: string;
};

export type MetaSubscribeKeepAlive = {
  interval_ms: number;
  event: string;
};

export type MetaEventEnvelope = {
  event_id: string;
  source: string;
  timestamp_ms: number;
  event: unknown;
};

export type MetaSubscribeResult = {
  scope: MetaSubscribeScope;
  replay_from_event_id: string | null;
  keep_alive: MetaSubscribeKeepAlive;
  keep_alive_comment: string;
  event_frames: string[];
  events: MetaEventEnvelope[];
};

// -- Type guards ----------------------------------------------------------

function asValueObject(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null) return {};
  return value as Record<string, unknown>;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function isMetaStatusResult(value: unknown): value is MetaStatusResult {
  const o = asValueObject(value);
  return (
    typeof o.contract_version === "string" &&
    typeof o.running === "boolean" &&
    isStringArray(o.loaded_modules)
  );
}

function isMetaCapabilitiesResult(
  value: unknown,
): value is MetaCapabilitiesResult {
  const o = asValueObject(value);
  return (
    typeof o.contract_version === "string" &&
    (o.feature_capabilities === undefined || isStringArray(o.feature_capabilities)) &&
    isStringArray(o.methods) &&
    isStringArray(o.loaded_modules)
  );
}

function isMetaReconcileResult(
  value: unknown,
): value is MetaReconcileResult {
  const o = asValueObject(value);
  return (
    typeof o.accepted === "boolean" &&
    isStringArray(o.reconciled_modules) &&
    typeof o.added === "number" &&
    Number.isInteger(o.added)
  );
}

function isMetaSpawnMemberResult(
  value: unknown,
): value is MetaSpawnMemberResult {
  const o = asValueObject(value);
  return typeof o.accepted === "boolean" && typeof o.module_id === "string";
}

function isMetaSubscribeResult(
  value: unknown,
): value is MetaSubscribeResult {
  const o = asValueObject(value);
  if (o.scope !== "mob" && o.scope !== "agent" && o.scope !== "interaction") {
    return false;
  }
  if (
    !(
      o.replay_from_event_id === null ||
      typeof o.replay_from_event_id === "string"
    )
  ) {
    return false;
  }
  const ka = asValueObject(o.keep_alive);
  if (!Number.isInteger(ka.interval_ms) || typeof ka.event !== "string") {
    return false;
  }
  if (typeof o.keep_alive_comment !== "string") return false;
  if (!isStringArray(o.event_frames)) return false;
  if (!Array.isArray(o.events)) return false;
  return o.events.every((ev) => {
    const e = asValueObject(ev);
    return (
      typeof e.event_id === "string" &&
      typeof e.source === "string" &&
      typeof e.timestamp_ms === "number" &&
      Number.isInteger(e.timestamp_ms) &&
      Object.prototype.hasOwnProperty.call(e, "event")
    );
  });
}

// -- Response parsing -----------------------------------------------------

function parseJsonRpcResponse(
  payload: unknown,
  expectedId: string,
): JsonRpcResponse {
  if (typeof payload !== "object" || payload === null) {
    throw new Error("invalid JSON-RPC response envelope");
  }
  const envelope = payload as Record<string, unknown>;
  if (envelope.jsonrpc !== "2.0" || envelope.id !== expectedId) {
    throw new Error("invalid JSON-RPC response envelope");
  }
  const hasResult = Object.prototype.hasOwnProperty.call(envelope, "result");
  const hasError = Object.prototype.hasOwnProperty.call(envelope, "error");
  if (hasResult === hasError) {
    throw new Error("invalid JSON-RPC response envelope");
  }
  if (hasError) {
    const rpcError = asValueObject(envelope.error);
    if (
      !Number.isInteger(rpcError.code) ||
      typeof rpcError.message !== "string"
    ) {
      throw new Error("invalid JSON-RPC response envelope");
    }
  }
  return envelope as unknown as JsonRpcResponse;
}

function unwrapTypedResult<TResult>(
  response: JsonRpcResponse,
  requestId: string,
  method: string,
  isExpected: (value: unknown) => value is TResult,
): TResult {
  if ("error" in response && response.error) {
    throw new RpcError(
      response.error.code,
      response.error.message,
      requestId,
      method,
    );
  }
  const result = (response as { result: unknown }).result;
  if (!isExpected(result)) {
    throw new Error(`invalid result payload for ${method}`);
  }
  return result;
}

function buildSubscribeParams(
  params: MetaSubscribeParams,
): Record<string, unknown> {
  const next: Record<string, unknown> = {};
  if (params.scope !== undefined) next.scope = params.scope;
  if (params.last_event_id !== undefined) {
    next.last_event_id = params.last_event_id;
  }
  if (params.agent_id !== undefined) next.agent_id = params.agent_id;
  return next;
}

// -- MetaTypedClient (sync) ---------------------------------------------

export class MetaTypedClient {
  private readonly syncTransport: JsonRpcSyncTransport;

  constructor(private readonly gatewayBin: string) {
    this.syncTransport = createGatewaySyncTransport(gatewayBin);
  }

  rpc(
    id: string,
    method: string,
    params: Record<string, unknown>,
  ): JsonRpcResponse {
    const payload = this.syncTransport(buildJsonRpcRequest(id, method, params));
    return parseJsonRpcResponse(payload, id);
  }

  status(requestId = "status"): MetaStatusResult {
    return unwrapTypedResult(
      this.rpc(requestId, "meta/status", {}),
      requestId,
      "meta/status",
      isMetaStatusResult,
    );
  }

  capabilities(requestId = "capabilities"): MetaCapabilitiesResult {
    return unwrapTypedResult(
      this.rpc(requestId, "meta/capabilities", {}),
      requestId,
      "meta/capabilities",
      isMetaCapabilitiesResult,
    );
  }

  reconcile(
    modules: string[],
    requestId = "reconcile",
  ): MetaReconcileResult {
    return unwrapTypedResult(
      this.rpc(requestId, "meta/reconcile", { modules }),
      requestId,
      "meta/reconcile",
      isMetaReconcileResult,
    );
  }

  spawnMember(
    moduleId: string,
    requestId = "spawn_member",
  ): MetaSpawnMemberResult {
    return unwrapTypedResult(
      this.rpc(requestId, "meta/spawn_member", { module_id: moduleId }),
      requestId,
      "meta/spawn_member",
      isMetaSpawnMemberResult,
    );
  }

  subscribeEvents(
    params: MetaSubscribeParams = {},
    requestId = "events_subscribe",
  ): MetaSubscribeResult {
    return unwrapTypedResult(
      this.rpc(
        requestId,
        "meta/events/subscribe",
        buildSubscribeParams(params),
      ),
      requestId,
      "meta/events/subscribe",
      isMetaSubscribeResult,
    );
  }
}

// -- MetaAsyncClient (async) --------------------------------------------

export class MetaAsyncClient {
  constructor(private readonly transport: JsonRpcTransport) {}

  static fromGatewayBin(gatewayBin: string): MetaAsyncClient {
    return new MetaAsyncClient(createGatewayAsyncTransport(gatewayBin));
  }

  static fromHttp(
    endpoint: string,
    options: {
      headers?: Record<string, string>;
      fetchImpl?: FetchLike;
    } = {},
  ): MetaAsyncClient {
    return new MetaAsyncClient(
      createJsonRpcHttpTransport(endpoint, options),
    );
  }

  async rpc(
    id: string,
    method: string,
    params: Record<string, unknown>,
  ): Promise<JsonRpcResponse> {
    const payload = await this.transport(
      buildJsonRpcRequest(id, method, params),
    );
    return parseJsonRpcResponse(payload, id);
  }

  async request<TResult>(
    id: string,
    method: string,
    params: Record<string, unknown>,
    isExpected: (value: unknown) => value is TResult,
  ): Promise<TResult> {
    const response = await this.rpc(id, method, params);
    return unwrapTypedResult(response, id, method, isExpected);
  }

  async status(requestId = "status"): Promise<MetaStatusResult> {
    return this.request(
      requestId,
      "meta/status",
      {},
      isMetaStatusResult,
    );
  }

  async capabilities(
    requestId = "capabilities",
  ): Promise<MetaCapabilitiesResult> {
    return this.request(
      requestId,
      "meta/capabilities",
      {},
      isMetaCapabilitiesResult,
    );
  }

  async reconcile(
    modules: string[],
    requestId = "reconcile",
  ): Promise<MetaReconcileResult> {
    return this.request(
      requestId,
      "meta/reconcile",
      { modules },
      isMetaReconcileResult,
    );
  }

  async spawnMember(
    moduleId: string,
    requestId = "spawn_member",
  ): Promise<MetaSpawnMemberResult> {
    return this.request(
      requestId,
      "meta/spawn_member",
      { module_id: moduleId },
      isMetaSpawnMemberResult,
    );
  }

  async subscribeEvents(
    params: MetaSubscribeParams = {},
    requestId = "events_subscribe",
  ): Promise<MetaSubscribeResult> {
    return this.request(
      requestId,
      "meta/events/subscribe",
      buildSubscribeParams(params),
      isMetaSubscribeResult,
    );
  }
}
