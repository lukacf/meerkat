import { isObject, receiptJson, serviceBody } from "./json.js";
import type { JsonObject, JsonValue } from "./json.js";
export type { JsonObject, JsonValue } from "./json.js";

export type FailureKind = "authentication" | "permission" | "not_found" | "rate_limited"
  | "redirect" | "http_status" | "invalid_response" | "response_too_large"
  | "unsupported_encoding" | "timeout" | "cancelled" | "transport";
export type WriteOutcome = "not_applicable" | "unknown";

/** Safe metadata only. Provider bodies, URLs, credentials and underlying causes are omitted. */
export class HomeAssistantError extends Error {
  constructor(
    readonly kind: FailureKind,
    readonly writeOutcome: WriteOutcome,
    readonly statusCode: number | null = null,
    readonly retryAfterSeconds: number | null = null,
  ) {
    super(`Home Assistant ${kind}${statusCode === null ? "" : ` (HTTP ${statusCode})`}; write outcome: ${writeOutcome}`);
    this.name = "HomeAssistantError";
  }
}

export class HomeAssistantCancelled extends HomeAssistantError {
  constructor(writeOutcome: WriteOutcome) {
    super("cancelled", writeOutcome);
    this.name = "HomeAssistantCancelled";
  }
}

/** Already selected by the host. The closure owns the actual account/credential binding. */
export interface HomeAssistantContext {
  readonly baseUrl: string;
  /** Borrowed authenticated Fetch. Must honor signal/manual redirects; no retarget/replay/retry. */
  readonly fetch: (request: Request) => Promise<Response>;
}

export interface HomeAssistantOptions {
  readonly timeoutMs?: number;
  readonly maxResponseBytes?: number;
}
export interface OperationOptions { readonly signal?: AbortSignal }
export interface ServiceOptions extends OperationOptions {
  readonly data?: JsonObject;
  readonly returnResponse?: boolean;
}
export interface EntityState extends JsonObject {
  entity_id: string;
  state: string;
  attributes: JsonObject;
}
export interface ServiceDomain extends JsonObject { domain: string; services: JsonObject }
export interface ServiceCallResult {
  readonly changedStates: EntityState[];
  /** False means absent; true plus null means a requested JSON null response. */
  readonly responseRequested: boolean;
  readonly serviceResponse: JsonObject | null;
}

function segment(value: string): string {
  if (typeof value !== "string" || value.length === 0 || /[^a-z0-9_]/.test(value)) {
    throw new TypeError("expected a canonical Home Assistant identifier");
  }
  return value;
}
function entity(value: string): string {
  if (typeof value !== "string") throw new TypeError("expected a canonical entity identifier");
  const parts = value.split(".");
  if (parts.length !== 2) throw new TypeError("expected a canonical entity identifier");
  segment(parts[0]!);
  segment(parts[1]!);
  return value;
}
function state(value: JsonValue): value is EntityState {
  return isObject(value) && typeof value.entity_id === "string" && typeof value.state === "string"
    && isObject(value.attributes);
}

/** REST operations only. This object owns neither authentication nor transport lifetime. */
export class HomeAssistantOperations {
  readonly #base: string;
  readonly #fetch: HomeAssistantContext["fetch"];
  readonly #timeoutMs: number;
  readonly #maxBytes: number;

  constructor(context: HomeAssistantContext, options: HomeAssistantOptions = {}) {
    let base: URL;
    try {
      if (typeof context.baseUrl !== "string" || typeof context.fetch !== "function") throw new TypeError();
      base = new URL(context.baseUrl);
      // Require canonical host-selected URL spelling: no hidden normalization, query or userinfo.
      if (base.href !== context.baseUrl || !["http:", "https:"].includes(base.protocol)
        || !base.hostname || base.username || base.password || base.href.includes("?")
        || base.href.includes("#")) throw new TypeError();
    } catch {
      throw new TypeError("select one canonical HTTP(S) base URL without userinfo, query or fragment");
    }
    if (!base.pathname.endsWith("/")) base.pathname += "/";
    this.#base = base.href;
    const fetch = context.fetch;
    this.#fetch = (request) => fetch(request);
    this.#timeoutMs = options.timeoutMs === undefined ? 20_000 : options.timeoutMs;
    this.#maxBytes = options.maxResponseBytes === undefined ? 1_048_576 : options.maxResponseBytes;
    if (!Number.isFinite(this.#timeoutMs) || this.#timeoutMs <= 0 || this.#timeoutMs > 2_147_483_647) {
      throw new TypeError("timeoutMs must be finite, positive and within the timer range");
    }
    if (!Number.isSafeInteger(this.#maxBytes) || this.#maxBytes <= 0) {
      throw new TypeError("maxResponseBytes must be a positive safe integer");
    }
  }

  #failure(kind: FailureKind, write: boolean, response?: Response): HomeAssistantError {
    const retry = response?.headers.get("retry-after") ?? "";
    return new HomeAssistantError(kind, write ? "unknown" : "not_applicable", response?.status ?? null,
      /^[0-9]{1,9}$/.test(retry) ? Number(retry) : null);
  }

  async #json(method: "GET" | "POST", path: string, write: boolean, signal: AbortSignal | undefined,
    content?: string, responseRequested = false): Promise<JsonValue> {
    if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
    const url = new URL(path, this.#base);
    if (responseRequested) url.search = "return_response";
    const controller = new AbortController();
    let stopped: "cancelled" | "timeout" | undefined;
    const stop = (kind: "cancelled" | "timeout"): void => {
      if (stopped === undefined) { stopped = kind; controller.abort(); }
    };
    const onCancel = (): void => stop("cancelled");
    const deadline = performance.now() + this.#timeoutMs;
    const timer = setTimeout(() => stop("timeout"), this.#timeoutMs);
    let response: Response | undefined;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let complete = false;
    let failure: HomeAssistantError | undefined;
    let result: JsonValue = null;
    const checkStopped = (): void => {
      if (performance.now() >= deadline) stop("timeout");
      if (stopped !== undefined) throw new Error();
    };
    signal?.addEventListener("abort", onCancel, { once: true });
    if (signal?.aborted) onCancel();
    try {
      checkStopped();
      const headers = new Headers({ Accept: "application/json", "Accept-Encoding": "identity" });
      if (content !== undefined) headers.set("Content-Type", "application/json");
      const request = new Request(url, {
        method, headers, ...(content === undefined ? {} : { body: content }),
        redirect: "manual", credentials: "omit", cache: "no-store", signal: controller.signal,
      });
      response = await this.#fetch(request); // One attempt. Account/credential stays inside host closure.
      checkStopped();
      const fail: (kind: FailureKind) => never = (kind) => {
        failure = this.#failure(kind, write, response);
        throw new Error();
      };
      if (response.redirected || response.type === "opaqueredirect"
        || (response.url !== "" && response.url !== url.href)) fail("redirect");
      if (response.status !== 200) {
        if (response.status >= 300 && response.status < 400) fail("redirect");
        fail(({
          401: "authentication", 403: "permission", 404: "not_found", 429: "rate_limited",
        } as Record<number, FailureKind>)[response.status] ?? "http_status");
      }
      if ((response.headers.get("content-encoding") ?? "identity").toLowerCase() !== "identity") {
        fail("unsupported_encoding");
      }
      if (response.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase() !== "application/json") {
        fail("invalid_response");
      }
      const length = response.headers.get("content-length");
      if (length !== null && !/^[0-9]+$/.test(length)) fail("invalid_response");
      if (length !== null && (length.length > 12 || Number(length) > this.#maxBytes)) fail("response_too_large");
      if (!response.body) fail("invalid_response");
      reader = response.body.getReader();
      const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
      let bytes = 0;
      const chunks: string[] = [];
      while (true) {
        const chunk = await reader.read();
        checkStopped();
        if (chunk.done) { complete = true; break; }
        bytes += chunk.value.byteLength;
        if (bytes > this.#maxBytes) fail("response_too_large");
        try { chunks.push(decoder.decode(chunk.value, { stream: true })); }
        catch { fail("invalid_response"); }
      }
      if (length !== null && bytes !== Number(length)) fail("invalid_response");
      try { chunks.push(decoder.decode()); result = receiptJson(chunks.join("")); }
      catch { fail("invalid_response"); }
      checkStopped();
    } catch {
      // Never attach the provider/host exception, its message, or abort reason as a cause.
      failure ??= this.#failure("transport", write);
    } finally {
      // Abort signals initiate teardown; do not detach reader cancellation or host fetch work.
      try {
        if (reader) { if (!complete) await reader.cancel(); }
        else if (response?.body) await response.body.cancel();
      } catch { failure ??= this.#failure("transport", write); }
      finally { reader?.releaseLock(); clearTimeout(timer); signal?.removeEventListener("abort", onCancel); }
    }
    if (stopped === "cancelled") throw new HomeAssistantCancelled(write ? "unknown" : "not_applicable");
    if (stopped === "timeout") throw this.#failure("timeout", write);
    if (failure) throw failure;
    return result;
  }

  async getState(entityId: string, options: OperationOptions = {}): Promise<EntityState> {
    entityId = entity(entityId);
    const result = await this.#json("GET", `api/states/${entityId}`, false, options.signal);
    if (!state(result) || result.entity_id !== entityId) throw this.#failure("invalid_response", false);
    return result;
  }

  async listServices(options: OperationOptions = {}): Promise<ServiceDomain[]> {
    const result = await this.#json("GET", "api/services", false, options.signal);
    if (!Array.isArray(result) || !result.every((row): row is ServiceDomain =>
      isObject(row) && typeof row.domain === "string" && isObject(row.services))) {
      throw this.#failure("invalid_response", false);
    }
    return result;
  }

  async callService(domain: string, service: string, options: ServiceOptions = {}): Promise<ServiceCallResult> {
    domain = segment(domain);
    service = segment(service);
    const requested = options.returnResponse === undefined ? false : options.returnResponse;
    if (typeof requested !== "boolean") throw new TypeError("returnResponse must be boolean");
    const content = serviceBody(options.data === undefined ? {} : options.data);
    const result = await this.#json("POST", `api/services/${domain}/${service}`, true,
      options.signal, content, requested);
    let changedStates: JsonValue = result;
    let serviceResponse: JsonObject | null = null;
    if (requested) {
      if (!isObject(result) || !Object.hasOwn(result, "changed_states") || !Object.hasOwn(result, "service_response")) {
        throw this.#failure("invalid_response", true);
      }
      changedStates = result.changed_states!;
      const response = result.service_response!;
      if (response !== null && !isObject(response)) throw this.#failure("invalid_response", true);
      serviceResponse = response;
    }
    if (!Array.isArray(changedStates) || !changedStates.every(state)) throw this.#failure("invalid_response", true);
    return { changedStates, responseRequested: requested, serviceResponse };
  }
}
