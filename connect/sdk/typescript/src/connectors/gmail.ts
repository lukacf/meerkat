import { isObject, receiptJson } from "./json.js";
import type { JsonObject, JsonValue } from "./json.js";
export type { JsonObject, JsonValue } from "./json.js";

export type GmailFailureKind = "authentication" | "permission" | "not_found" | "rate_limited" | "quota_exceeded"
  | "redirect" | "http_status" | "invalid_response" | "response_too_large"
  | "unsupported_encoding" | "timeout" | "cancelled" | "transport";
export type GmailWriteOutcome = "not_applicable" | "unknown";
export type GmailProviderReason = "rateLimitExceeded" | "userRateLimitExceeded"
  | "dailyLimitExceeded" | "domainPolicy";

/** Recognize only unanimous documented evidence, never provider message text. */
function forbiddenReason(value: JsonValue): GmailProviderReason | null {
  if (!isObject(value) || !isObject(value.error) || value.error.code !== 403
    || !Array.isArray(value.error.errors) || value.error.errors.length < 1
    || value.error.errors.length > 32) return null;
  let selected: GmailProviderReason | null = null;
  for (const item of value.error.errors) {
    if (!isObject(item)) return null;
    const reason = item.reason;
    if (reason !== "rateLimitExceeded" && reason !== "userRateLimitExceeded"
      && reason !== "dailyLimitExceeded" && reason !== "domainPolicy") return null;
    if (item.domain !== (reason === "domainPolicy" ? "global" : "usageLimits")
      || (selected !== null && selected !== reason)) return null;
    selected = reason;
  }
  return selected;
}

/** Safe metadata only. No provider body, URL, MIME, credential, cause or abort reason. */
export class GmailError extends Error {
  constructor(
    readonly kind: GmailFailureKind,
    readonly writeOutcome: GmailWriteOutcome,
    readonly statusCode: number | null = null,
    readonly retryAfterSeconds: number | null = null,
    readonly providerReason: GmailProviderReason | null = null,
  ) {
    super(`Gmail ${kind}${statusCode === null ? "" : ` (HTTP ${statusCode})`}; write outcome: ${writeOutcome}`);
    this.name = "GmailError";
  }
}
export class GmailCancelled extends GmailError {
  constructor(writeOutcome: GmailWriteOutcome) {
    super("cancelled", writeOutcome);
    this.name = "GmailCancelled";
  }
}

/** Routing selected by the host, not proof of credential/account correspondence. */
export interface GmailContext {
  readonly baseUrl: string;
  readonly userId: string;
  /** Borrowed authenticated Fetch. No retargeting/retry/replay; honor signal/manual redirects. */
  readonly fetch: (request: Request) => Promise<Response>;
}
export interface GmailOptions {
  readonly timeoutMs?: number;
  readonly maxResponseBytes?: number;
  readonly maxMessageBytes?: number;
}
export interface GmailOperationOptions { readonly signal?: AbortSignal }
export interface GmailListOptions extends GmailOperationOptions {
  readonly query?: string;
  readonly labelIds?: readonly string[];
  readonly pageToken?: string;
  readonly maxResults?: number;
  readonly includeSpamTrash?: boolean;
}
export interface GmailGetOptions extends GmailOperationOptions {
  readonly format?: "full" | "metadata" | "minimal" | "raw";
  readonly metadataHeaders?: readonly string[];
}
export interface GmailMessage extends JsonObject { id: string; threadId: string }

function identifier(value: string): string {
  if (typeof value !== "string" || !value.length || value.length > 1024 || /[^A-Za-z0-9_-]/.test(value)) {
    throw new TypeError("expected a Gmail message or label identifier");
  }
  return value;
}
function text(value: string): string {
  if (typeof value !== "string" || !value.length || /[\x00-\x1f\x7f]/.test(value)) {
    throw new TypeError("expected bounded nonempty text without control characters");
  }
  // TextEncoder replaces unpaired surrogates; refuse them instead of changing a token/query.
  for (let index = 0; index < value.length; index++) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(++index);
      if (!(next >= 0xdc00 && next <= 0xdfff)) throw new TypeError("text must be valid Unicode");
    } else if (unit >= 0xdc00 && unit <= 0xdfff) throw new TypeError("text must be valid Unicode");
  }
  if (new TextEncoder().encode(value).byteLength > 8192) throw new TypeError("text exceeds the UTF-8 limit");
  return value;
}
function strings(value: readonly string[]): readonly string[] {
  if (!Array.isArray(value) || value.length > 100) throw new TypeError("expected a bounded string array");
  // Index access intentionally rejects sparse arrays through the per-item validator.
  return value;
}
function message(value: JsonValue): value is GmailMessage {
  return isObject(value) && typeof value.id === "string" && value.id.length > 0
    && typeof value.threadId === "string" && value.threadId.length > 0
    && (!Object.hasOwn(value, "payload") || isObject(value.payload))
    && (!Object.hasOwn(value, "raw") || typeof value.raw === "string")
    && (!Object.hasOwn(value, "labelIds") || (Array.isArray(value.labelIds)
      && value.labelIds.every((label) => typeof label === "string")));
}

/** Independent Gmail REST operations. The host retains auth, permissions and transport lifetime. */
export class GmailOperations {
  readonly #base: string;
  readonly #userPath: string;
  readonly #fetch: GmailContext["fetch"];
  readonly #timeoutMs: number;
  readonly #maxBytes: number;
  readonly #maxMessageBytes: number;

  constructor(context: GmailContext, options: GmailOptions = {}) {
    let base: URL;
    try {
      if (typeof context.baseUrl !== "string" || typeof context.fetch !== "function") throw new TypeError();
      base = new URL(context.baseUrl);
      if (base.href !== context.baseUrl || !["http:", "https:"].includes(base.protocol)
        || !base.hostname || base.username || base.password || base.href.includes("?")
        || base.href.includes("#")) throw new TypeError();
    } catch {
      throw new TypeError("select one canonical HTTP(S) base URL without userinfo, query or fragment");
    }
    const userId = context.userId;
    if (typeof userId !== "string" || userId.length > 320 || (userId !== "me"
      && !/^[A-Za-z0-9._+-]+@[A-Za-z0-9.-]+$/.test(userId)) || /[\r\n]/.test(userId)) {
      throw new TypeError("userId must be me or a supported ASCII email address");
    }
    if (!base.pathname.endsWith("/")) base.pathname += "/";
    this.#base = base.href;
    this.#userPath = encodeURIComponent(userId);
    const fetch = context.fetch;
    this.#fetch = (request) => fetch(request);
    this.#timeoutMs = options.timeoutMs === undefined ? 20_000 : options.timeoutMs;
    this.#maxBytes = options.maxResponseBytes === undefined ? 2_097_152 : options.maxResponseBytes;
    this.#maxMessageBytes = options.maxMessageBytes === undefined ? 4_194_304 : options.maxMessageBytes;
    if (!Number.isFinite(this.#timeoutMs) || this.#timeoutMs <= 0 || this.#timeoutMs > 2_147_483_647) {
      throw new TypeError("timeoutMs must be finite, positive and within the timer range");
    }
    for (const limit of [this.#maxBytes, this.#maxMessageBytes]) {
      if (!Number.isSafeInteger(limit) || limit <= 0) throw new TypeError("byte limits must be positive safe integers");
    }
  }
  #failure(kind: GmailFailureKind, write: boolean, response?: Response,
    reason: GmailProviderReason | null = null): GmailError {
    const retry = response?.headers.get("retry-after") ?? "";
    return new GmailError(kind, write ? "unknown" : "not_applicable", response?.status ?? null,
      /^[0-9]{1,9}$/.test(retry) ? Number(retry) : null, reason);
  }

  async #json(method: "GET" | "POST", path: string, write: boolean, signal: AbortSignal | undefined,
    content?: string, params: readonly (readonly [string, string])[] = []): Promise<JsonValue> {
    if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
    const url = new URL(`gmail/v1/users/${this.#userPath}/${path}`, this.#base);
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
        // A borrowed body may not listen to the request signal. Cancel the
        // owned reader to release its pending read, and retain the same cleanup.
        cleanup = Promise.resolve().then(() => owned.cancel()).catch(() => { cleanupFailed = true; });
      }
    };
    const stop = (kind: "cancelled" | "timeout"): void => {
      if (stopped === undefined) { stopped = kind; controller.abort(); cancelReader(); }
    };
    const onCancel = (): void => stop("cancelled");
    const deadline = performance.now() + this.#timeoutMs;
    const timer = setTimeout(() => stop("timeout"), this.#timeoutMs);
    let failure: GmailError | undefined;
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
      let diagnostic = false;
      const fail: (kind: GmailFailureKind) => never = (kind) => {
        // An unusable diagnostic must not erase the already observed HTTP refusal.
        if (!diagnostic) failure = this.#failure(kind, write, response);
        throw new Error();
      };
      if (response.redirected || response.type === "opaqueredirect"
        || (response.url !== "" && response.url !== url.href)) fail("redirect");
      if (response.status !== 200) {
        if (response.status >= 300 && response.status < 400) fail("redirect");
        if (response.status === 403) {
          diagnostic = true;
          failure = this.#failure("permission", write, response);
        } else {
          fail(({
            401: "authentication", 404: "not_found", 429: "rate_limited",
          } as Record<number, GmailFailureKind>)[response.status] ?? "http_status");
        }
      }
      const byteLimit = diagnostic ? Math.min(this.#maxBytes, 65_536) : this.#maxBytes;
      if ((response.headers.get("content-encoding") ?? "identity").toLowerCase() !== "identity") {
        fail("unsupported_encoding");
      }
      if (response.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase() !== "application/json") {
        fail("invalid_response");
      }
      const length = response.headers.get("content-length");
      if (length !== null && !/^[0-9]+$/.test(length)) fail("invalid_response");
      if (length !== null && (length.length > 12 || Number(length) > byteLimit)) fail("response_too_large");
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
        if (bytes > byteLimit) fail("response_too_large");
        try { chunks.push(decoder.decode(chunk.value, { stream: true })); }
        catch { fail("invalid_response"); }
      }
      if (length !== null && bytes !== Number(length)) fail("invalid_response");
      try { chunks.push(decoder.decode()); result = receiptJson(chunks.join("")); }
      catch { fail("invalid_response"); }
      if (diagnostic) {
        const reason = forbiddenReason(result);
        if (reason !== null) {
          const kind = reason === "domainPolicy" ? "permission"
            : reason === "dailyLimitExceeded" ? "quota_exceeded" : "rate_limited";
          failure = this.#failure(kind, write, response, reason);
        }
      }
      checkStopped();
    } catch {
      // Never attach the provider/host exception, its message, or abort reason as a cause.
      failure ??= this.#failure("transport", write);
    } finally {
      // Abort signals initiate teardown; do not detach reader cancellation or host fetch work.
      try {
        if (reader) { cancelReader(); await cleanup; }
        else if (response?.body) await response.body.cancel();
      } catch { cleanupFailed = true; }
      finally { reader?.releaseLock(); clearTimeout(timer); signal?.removeEventListener("abort", onCancel); }
    }
    if (stopped === "cancelled") throw new GmailCancelled(write ? "unknown" : "not_applicable");
    if (stopped === "timeout") throw this.#failure("timeout", write);
    if (cleanupFailed) failure ??= this.#failure("transport", write);
    if (failure) throw failure;
    return result;
  }

  /** One page only, preserving absence, order, duplicates, estimates and opaque continuation. */
  async listMessagesPage(options: GmailListOptions = {}): Promise<JsonObject> {
    const maximum = options.maxResults === undefined ? 100 : options.maxResults;
    const spam = options.includeSpamTrash === undefined ? false : options.includeSpamTrash;
    if (!Number.isInteger(maximum) || maximum < 1 || maximum > 500) {
      throw new TypeError("maxResults must be an integer from 1 to 500");
    }
    if (typeof spam !== "boolean") throw new TypeError("includeSpamTrash must be boolean");
    const params: [string, string][] = [["maxResults", String(maximum)], ["includeSpamTrash", String(spam)]];
    if (options.query !== undefined) params.push(["q", text(options.query)]);
    if (options.pageToken !== undefined) params.push(["pageToken", text(options.pageToken)]);
    for (const label of strings(options.labelIds === undefined ? [] : options.labelIds)) {
      params.push(["labelIds", identifier(label)]);
    }
    const result = await this.#json("GET", "messages", false, options.signal, undefined, params);
    if (!isObject(result)
      || (Object.hasOwn(result, "messages") && (!Array.isArray(result.messages) || !result.messages.every(message)))
      || (Object.hasOwn(result, "nextPageToken") && (typeof result.nextPageToken !== "string" || !result.nextPageToken.length))
      || (Object.hasOwn(result, "resultSizeEstimate") && (typeof result.resultSizeEstimate !== "number"
        || !Number.isSafeInteger(result.resultSizeEstimate) || result.resultSizeEstimate < 0))) {
      throw this.#failure("invalid_response", false);
    }
    return result;
  }

  async getMessage(messageId: string, options: GmailGetOptions = {}): Promise<GmailMessage> {
    messageId = identifier(messageId);
    const format = options.format === undefined ? "full" : options.format;
    if (!["full", "metadata", "minimal", "raw"].includes(format)) throw new TypeError("unsupported Gmail message format");
    const headers = strings(options.metadataHeaders === undefined ? [] : options.metadataHeaders);
    if (headers.length && format !== "metadata") throw new TypeError("metadataHeaders requires metadata format");
    const params: [string, string][] = [["format", format]];
    for (const header of headers) {
      if (typeof header !== "string" || !header.length || header.length > 998 || /[^!#$%&'*+.^_`|~0-9A-Za-z-]/.test(header)) {
        throw new TypeError("invalid metadata header name");
      }
      params.push(["metadataHeaders", header]);
    }
    const result = await this.#json("GET", `messages/${messageId}`, false, options.signal, undefined, params);
    if (!message(result) || result.id !== messageId || (format === "raw" && !Object.hasOwn(result, "raw"))) {
      throw this.#failure("invalid_response", false);
    }
    return result;
  }

  /** Snapshot exact bytes before suspension. No MIME rewriting, idempotency promise or retry. */
  async sendRaw(mime: Uint8Array, options: GmailOperationOptions = {}): Promise<GmailMessage> {
    let content: string;
    try {
      if (!(mime instanceof Uint8Array) || !(mime.buffer instanceof ArrayBuffer)
        || mime.byteLength === 0 || mime.byteLength > this.#maxMessageBytes) throw new TypeError();
      // Copies the view's bytes, including its offset, not the whole backing buffer.
      // SharedArrayBuffer is refused because another thread can mutate during copying.
      const snapshot = new Uint8Array(mime);
      const parts: string[] = [];
      for (let index = 0; index < snapshot.length; index += 16_384) {
        parts.push(String.fromCharCode(...snapshot.subarray(index, index + 16_384)));
      }
      content = JSON.stringify({ raw: btoa(parts.join("")).replaceAll("+", "-").replaceAll("/", "_") });
    } catch {
      throw new TypeError("mime must be nonempty non-shared Uint8Array bytes within maxMessageBytes");
    }
    const result = await this.#json("POST", "messages/send", true, options.signal, content);
    if (!message(result)) throw this.#failure("invalid_response", true);
    return result;
  }
}
