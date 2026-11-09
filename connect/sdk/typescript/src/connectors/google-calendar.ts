import { isObject, receiptJson, serviceBody } from "./json.js";
import type { JsonObject, JsonValue } from "./json.js";
export type { JsonObject, JsonValue } from "./json.js";

export type CalendarFailureKind = "authentication" | "permission" | "not_found" | "conflict"
  | "precondition" | "rate_limited" | "redirect" | "http_status" | "invalid_response"
  | "response_too_large" | "unsupported_encoding" | "timeout" | "cancelled" | "transport";
export type CalendarWriteOutcome = "not_applicable" | "unknown";

/** Optional evidence only: unknown or conflicting 403 reasons stay permission. */
function rateLimited(value: JsonValue): boolean {
  if (!isObject(value) || !isObject(value.error) || value.error.code !== 403
    || !Array.isArray(value.error.errors) || value.error.errors.length < 1
    || value.error.errors.length > 32) return false;
  let selected: string | undefined;
  for (const item of value.error.errors) {
    if (!isObject(item) || item.domain !== "usageLimits"
      || (item.reason !== "rateLimitExceeded" && item.reason !== "userRateLimitExceeded")
      || (selected !== undefined && selected !== item.reason)) return false;
    selected = item.reason;
  }
  return true;
}

/** Fixed metadata only: never provider text, credentials, URL, cause or abort reason. */
export class CalendarError extends Error {
  constructor(readonly kind: CalendarFailureKind, readonly writeOutcome: CalendarWriteOutcome,
    readonly statusCode: number | null = null, readonly retryAfterSeconds: number | null = null) {
    super(`Google Calendar ${kind}${statusCode === null ? "" : ` (HTTP ${statusCode})`}; write outcome: ${writeOutcome}`);
    this.name = "CalendarError";
  }
}
export class CalendarCancelled extends CalendarError {
  constructor(outcome: CalendarWriteOutcome) {
    super("cancelled", outcome);
    this.name = "CalendarCancelled";
  }
}

/** Host-selected routing, not proof of account correspondence or authority. */
export interface GoogleCalendarContext {
  readonly baseUrl: string;
  readonly calendarId: string;
  /** Borrowed authenticated Fetch; preserve exact request and signal, with no replay. */
  readonly fetch: (request: Request) => Promise<Response>;
}
export interface GoogleCalendarOptions {
  readonly timeoutMs?: number;
  readonly maxResponseBytes?: number;
  readonly maxRequestBytes?: number;
}
export interface CalendarOperationOptions { readonly signal?: AbortSignal }
export interface CalendarConditionalOptions extends CalendarOperationOptions { readonly expectedEtag?: string }
export interface CalendarListOptions extends CalendarOperationOptions {
  readonly timeMin?: string;
  readonly timeMax?: string;
  readonly q?: string;
  readonly pageToken?: string;
  readonly maxResults?: number;
  readonly singleEvents?: boolean;
  readonly orderBy?: "startTime" | "updated";
  readonly showDeleted?: boolean;
}
export interface CalendarEvent extends JsonObject { id: string }
export interface CalendarPage extends JsonObject { kind: "calendar#events" }

// Reject unsupported options rather than silently dropping a requested condition.
function fields(value: object, allowed: readonly string[]): Record<string, unknown> {
  try {
    if (!isObject(value) || (Object.getPrototypeOf(value) !== Object.prototype
      && Object.getPrototypeOf(value) !== null)) throw new TypeError();
    const result: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
    for (const key of Reflect.ownKeys(value)) {
      const property = Object.getOwnPropertyDescriptor(value, key);
      if (typeof key !== "string" || !allowed.includes(key) || !property
        || !("value" in property) || !property.enumerable) throw new TypeError();
      result[key] = property.value;
    }
    return result;
  } catch { throw new TypeError("expected plain supported Calendar options"); }
}
function text(value: unknown): string {
  if (typeof value !== "string" || !value.length || /[\x00-\x1f\x7f]/.test(value)) {
    throw new TypeError("expected nonempty Calendar text without control characters");
  }
  for (let i = 0; i < value.length; i++) {
    const unit = value.charCodeAt(i);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) throw new TypeError("text must be valid Unicode");
    } else if (unit >= 0xdc00 && unit <= 0xdfff) throw new TypeError("text must be valid Unicode");
  }
  if (new TextEncoder().encode(value).byteLength > 8192) throw new TypeError("text exceeds the UTF-8 limit");
  return value;
}
function segment(value: unknown): string {
  const raw = text(value);
  if (raw === "." || raw === "..") throw new TypeError("dot segments are not Calendar identifiers");
  return encodeURIComponent(raw).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);
}
function etag(value: unknown): string {
  const raw = text(value);
  if (!/^"[\x21\x23-\x7e]*"$/.test(raw)) {
    throw new TypeError("expected one exact quoted ASCII ETag, not a wildcard, weak tag or list");
  }
  return raw;
}
function signal(value: unknown): AbortSignal | undefined {
  if (value !== undefined && !(value instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
  return value;
}
function event(value: JsonValue, write: boolean, expectedId?: string): value is CalendarEvent {
  if (!isObject(value)) return false;
  try {
    text(value.id);
    if (expectedId !== undefined && value.id !== expectedId) return false;
    if (write || value.status !== "cancelled") { etag(value.etag); text(value.updated); }
  } catch { return false; }
  return true;
}
function page(value: JsonValue): value is CalendarPage {
  if (!isObject(value) || value.kind !== "calendar#events") return false;
  if (Object.hasOwn(value, "items") && (!Array.isArray(value.items)
    || !value.items.every((item) => event(item, false)))) return false;
  try {
    for (const key of ["nextPageToken", "nextSyncToken"]) if (Object.hasOwn(value, key)) text(value[key]);
  } catch { return false; }
  return true;
}

/** Explicit single-page Calendar Events REST operations, without auth or retry policy. */
export class GoogleCalendarOperations {
  readonly #base: string;
  readonly #path: string;
  readonly #fetch: GoogleCalendarContext["fetch"];
  readonly #timeoutMs: number;
  readonly #maxBytes: number;
  readonly #maxRequestBytes: number;

  constructor(context: GoogleCalendarContext, options: GoogleCalendarOptions = {}) {
    const selected = fields(context, ["baseUrl", "calendarId", "fetch"]);
    let base: URL;
    try {
      if (typeof selected.baseUrl !== "string" || typeof selected.fetch !== "function") throw new TypeError();
      base = new URL(selected.baseUrl);
      if (base.href !== selected.baseUrl || !["http:", "https:"].includes(base.protocol)
        || !base.hostname || base.username || base.password || base.href.includes("?")
        || base.href.includes("#")) throw new TypeError();
    } catch { throw new TypeError("select one canonical HTTP(S) base URL without userinfo, query or fragment"); }
    if (!base.pathname.endsWith("/")) base.pathname += "/";
    this.#base = base.href;
    this.#path = `calendar/v3/calendars/${segment(selected.calendarId)}/events`;
    const fetch = selected.fetch as GoogleCalendarContext["fetch"];
    this.#fetch = (request) => fetch(request);
    const limits = fields(options, ["timeoutMs", "maxResponseBytes", "maxRequestBytes"]);
    const timeout = limits.timeoutMs === undefined ? 20_000 : limits.timeoutMs;
    if (typeof timeout !== "number" || !Number.isFinite(timeout) || timeout <= 0 || timeout > 2_147_483_647) {
      throw new TypeError("timeoutMs must be finite, positive and within the timer range");
    }
    this.#timeoutMs = timeout;
    const bytes = limits.maxResponseBytes === undefined ? 2_097_152 : limits.maxResponseBytes;
    const requestBytes = limits.maxRequestBytes === undefined ? 2_097_152 : limits.maxRequestBytes;
    for (const limit of [bytes, requestBytes]) {
      if (typeof limit !== "number" || !Number.isSafeInteger(limit) || limit <= 0) {
        throw new TypeError("byte limits must be positive safe integers");
      }
    }
    this.#maxBytes = bytes as number;
    this.#maxRequestBytes = requestBytes as number;
  }

  #failure(kind: CalendarFailureKind, write: boolean, response?: Response): CalendarError {
    const retry = response?.headers.get("retry-after") ?? "";
    return new CalendarError(kind, write ? "unknown" : "not_applicable", response?.status ?? null,
      /^[0-9]{1,9}$/.test(retry) ? Number(retry) : null);
  }
  #body(value: JsonObject): { content: string; snapshot: JsonObject } {
    const content = serviceBody(value);
    if (new TextEncoder().encode(content).byteLength > this.#maxRequestBytes) {
      throw new TypeError("Calendar request exceeds maxRequestBytes");
    }
    return { content, snapshot: JSON.parse(content) as JsonObject };
  }

  async #request(method: "GET" | "POST" | "PATCH" | "DELETE", path: string,
    callerSignal: AbortSignal | undefined, validate: (value: JsonValue) => boolean,
    content?: string, expectedEtag?: string, params: readonly (readonly [string, string])[] = []): Promise<JsonValue> {
    const write = method !== "GET";
    const url = new URL(path, this.#base);
    for (const [key, value] of params) url.searchParams.append(key, value);
    const controller = new AbortController();
    let stopped: "cancelled" | "timeout" | undefined;
    let response: Response | undefined;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let complete = false;
    let cleanup: Promise<void> | undefined;
    let cleanupFailed = false;
    const cancelReader = (): void => {
      if (reader && !complete && !cleanup) {
        const owned = reader;
        // Retain and observe cleanup immediately, then join it before returning.
        cleanup = Promise.resolve().then(() => owned.cancel()).catch(() => { cleanupFailed = true; });
      }
    };
    const stop = (kind: "cancelled" | "timeout"): void => {
      if (stopped === undefined) { stopped = kind; controller.abort(); cancelReader(); }
    };
    const onCancel = (): void => stop("cancelled");
    const deadline = performance.now() + this.#timeoutMs;
    const timer = setTimeout(() => stop("timeout"), this.#timeoutMs);
    const checkStopped = (): void => {
      if (performance.now() >= deadline) stop("timeout");
      if (stopped !== undefined) throw new Error();
    };
    let failure: CalendarError | undefined;
    let result: JsonValue = null;
    callerSignal?.addEventListener("abort", onCancel, { once: true });
    if (callerSignal?.aborted) onCancel();
    try {
      checkStopped();
      const headers = new Headers({ Accept: "application/json", "Accept-Encoding": "identity" });
      if (content !== undefined) headers.set("Content-Type", "application/json");
      if (expectedEtag !== undefined) headers.set("If-Match", expectedEtag);
      const request = new Request(url, { method, headers, ...(content === undefined ? {} : { body: content }),
        signal: controller.signal, redirect: "manual", credentials: "omit", cache: "no-store" });
      const received = await this.#fetch(request);
      if (!(received instanceof Response)) { failure = this.#failure("invalid_response", write); throw new Error(); }
      response = received;
      checkStopped();
      let diagnostic = false;
      const fail = (kind: CalendarFailureKind): never => {
        // Failed optional diagnostics preserve the already received refusal.
        if (!diagnostic) failure = this.#failure(kind, write, response);
        throw new Error();
      };
      if (response.redirected || response.type === "opaqueredirect"
        || (response.url !== "" && response.url !== url.href)) fail("redirect");
      if (response.status !== (method === "DELETE" ? 204 : 200)) {
        if (response.status >= 300 && response.status < 400) fail("redirect");
        if (response.status === 403) {
          diagnostic = true;
          failure = this.#failure("permission", write, response);
        } else {
          fail(({ 401: "authentication", 404: "not_found", 409: "conflict",
            412: "precondition", 429: "rate_limited" } as Record<number, CalendarFailureKind>)[response.status] ?? "http_status");
        }
      }
      const byteLimit = diagnostic ? Math.min(this.#maxBytes, 65_536) : this.#maxBytes;
      if ((response.headers.get("content-encoding") ?? "identity").toLowerCase() !== "identity") fail("unsupported_encoding");
      const length = response.headers.get("content-length");
      if (length !== null && !/^[0-9]+$/.test(length)) fail("invalid_response");
      if (length !== null && (length.length > 12 || Number(length) > byteLimit)) fail("response_too_large");
      if (method === "DELETE" && !diagnostic) {
        if (length !== null && Number(length) !== 0) fail("invalid_response");
      } else if (!/^application\/json(?:\s*;\s*charset\s*=\s*(?:utf-8|"utf-8"))?\s*$/i
        .test(response.headers.get("content-type") ?? "")) fail("invalid_response");
      if ((method !== "DELETE" || diagnostic) && !response.body) fail("invalid_response");
      const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
      let bytes = 0;
      const chunks: string[] = [];
      if (response.body) {
        reader = response.body.getReader();
        while (true) {
          const chunk = await reader.read();
          checkStopped();
          if (chunk.done) { complete = true; break; }
          bytes += chunk.value.byteLength;
          if (bytes > byteLimit) fail("response_too_large");
          if (method === "DELETE" && !diagnostic && bytes !== 0) fail("invalid_response");
          try { chunks.push(decoder.decode(chunk.value, { stream: true })); } catch { fail("invalid_response"); }
        }
      } else complete = true;
      if (length !== null && bytes !== Number(length)) fail("invalid_response");
      if (method !== "DELETE" || diagnostic) {
        try { chunks.push(decoder.decode()); result = receiptJson(chunks.join("")); } catch { fail("invalid_response"); }
        if (diagnostic) {
          if (rateLimited(result)) failure = this.#failure("rate_limited", write, response);
        } else if (!validate(result)) fail("invalid_response");
      }
      checkStopped();
    } catch { failure ??= this.#failure("transport", write); }
    finally {
      try {
        if (reader) { cancelReader(); await cleanup; }
        else if (response?.body) await response.body.cancel();
      } catch { cleanupFailed = true; }
      finally { reader?.releaseLock(); clearTimeout(timer); callerSignal?.removeEventListener("abort", onCancel); }
    }
    if (stopped === "cancelled") throw new CalendarCancelled(write ? "unknown" : "not_applicable");
    if (stopped === "timeout") throw this.#failure("timeout", write);
    if (cleanupFailed) failure ??= this.#failure("transport", write);
    if (failure) throw failure;
    return result;
  }

  async listEventsPage(options: CalendarListOptions = {}): Promise<CalendarPage> {
    const value = fields(options, ["timeMin", "timeMax", "q", "pageToken", "maxResults", "singleEvents", "orderBy", "showDeleted", "signal"]);
    const params: [string, string][] = [];
    for (const key of ["timeMin", "timeMax", "q", "pageToken"]) if (value[key] !== undefined) params.push([key, text(value[key])]);
    if (value.maxResults !== undefined) {
      if (typeof value.maxResults !== "number" || !Number.isInteger(value.maxResults)
        || value.maxResults < 1 || value.maxResults > 2500) throw new TypeError("maxResults must be an integer from 1 to 2500");
      params.push(["maxResults", String(value.maxResults)]);
    }
    for (const key of ["singleEvents", "showDeleted"]) if (value[key] !== undefined) {
      if (typeof value[key] !== "boolean") throw new TypeError("Calendar event flags must be booleans");
      params.push([key, String(value[key])]);
    }
    if (value.orderBy !== undefined) {
      if ((value.orderBy !== "startTime" && value.orderBy !== "updated")
        || (value.orderBy === "startTime" && value.singleEvents !== true)) throw new TypeError("unsupported orderBy or missing singleEvents");
      params.push(["orderBy", value.orderBy]);
    }
    return await this.#request("GET", this.#path, signal(value.signal), page, undefined, undefined, params) as CalendarPage;
  }
  async getEvent(eventId: string, options: CalendarOperationOptions = {}): Promise<CalendarEvent> {
    const path = `${this.#path}/${segment(eventId)}`;
    const value = fields(options, ["signal"]);
    return await this.#request("GET", path, signal(value.signal), (item) => event(item, false, eventId)) as CalendarEvent;
  }
  async insertEvent(body: JsonObject, options: CalendarOperationOptions = {}): Promise<CalendarEvent> {
    const value = fields(options, ["signal"]);
    const { content, snapshot } = this.#body(body);
    const id = Object.hasOwn(snapshot, "id") ? text(snapshot.id) : undefined;
    return await this.#request("POST", this.#path, signal(value.signal), (item) => event(item, true, id), content) as CalendarEvent;
  }
  async patchEvent(eventId: string, body: JsonObject, options: CalendarConditionalOptions = {}): Promise<CalendarEvent> {
    const path = `${this.#path}/${segment(eventId)}`;
    const value = fields(options, ["expectedEtag", "signal"]);
    const expected = value.expectedEtag === undefined ? undefined : etag(value.expectedEtag);
    const { content, snapshot } = this.#body(body);
    if (Object.hasOwn(snapshot, "id") && snapshot.id !== eventId) throw new TypeError("patch id must match the selected event");
    return await this.#request("PATCH", path, signal(value.signal), (item) => event(item, true, eventId), content, expected) as CalendarEvent;
  }
  async deleteEvent(eventId: string, options: CalendarConditionalOptions = {}): Promise<void> {
    const path = `${this.#path}/${segment(eventId)}`;
    const value = fields(options, ["expectedEtag", "signal"]);
    const expected = value.expectedEtag === undefined ? undefined : etag(value.expectedEtag);
    await this.#request("DELETE", path, signal(value.signal), () => true, undefined, expected);
  }
}
