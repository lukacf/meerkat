import { isObject, receiptJson } from "./json.js";
import type { JsonObject, JsonValue } from "./json.js";

export type SlackFailureKind = "authentication" | "permission" | "not_found" | "rate_limited"
  | "provider" | "redirect" | "http_status" | "invalid_response" | "response_too_large"
  | "unsupported_encoding" | "timeout" | "cancelled" | "transport";

/** Safe observations only: no response body, credential, cursor, URL or exception cause. */
export class SlackError extends Error {
  constructor(
    readonly kind: SlackFailureKind,
    readonly statusCode: number | null = null,
    readonly providerCode: string | null = null,
    readonly retryAfterSeconds: number | null = null,
  ) {
    super(`Slack ${kind}${statusCode === null ? "" : ` (HTTP ${statusCode})`}`);
    this.name = "SlackError";
  }
}
export class SlackCancelled extends SlackError {
  constructor() { super("cancelled"); this.name = "SlackCancelled"; }
}

export interface SlackContext {
  readonly baseUrl: string;
  /** Host-selected account/scopes; no retry, retargeting or ignored abort/manual redirect. */
  readonly fetch: (request: Request) => Promise<Response>;
}
export interface SlackOptions {
  readonly timeoutMs?: number;
  readonly maxResponseBytes?: number;
  /** Fixed host routing ceiling. Omitted is unrestricted; an empty array denies channel calls. */
  readonly allowedChannelIds?: readonly string[];
}
export interface SlackPageOptions {
  readonly oldest?: string;
  readonly latest?: string;
  readonly inclusive?: boolean;
  readonly cursor?: string;
  readonly limit?: number;
  readonly includeAllMetadata?: boolean;
  readonly signal?: AbortSignal;
}
export interface SlackPage extends JsonObject { ok: true; messages: JsonObject[] }
export interface SlackOperationOptions { readonly signal?: AbortSignal }
export interface SlackUserGetOptions extends SlackOperationOptions { readonly includeLocale?: boolean }
export interface SlackUser extends JsonObject { id: string }
export interface SlackUserResult extends JsonObject { ok: true; user: SlackUser }
export type SlackConversationType = "public_channel" | "private_channel" | "mpim" | "im";
export interface SlackConversationListOptions extends SlackOperationOptions {
  readonly types?: readonly SlackConversationType[];
  readonly excludeArchived?: boolean;
  readonly cursor?: string;
  readonly limit?: number;
  readonly teamId?: string;
}
export interface SlackConversationGetOptions extends SlackOperationOptions {
  readonly includeLocale?: boolean;
  readonly includeNumMembers?: boolean;
}
export interface SlackAuthObservation extends JsonObject { ok: true; team_id: string; user_id: string }
export interface SlackConversation extends JsonObject { id: string }
export interface SlackConversationsPage extends JsonObject { ok: true; channels: SlackConversation[] }
export interface SlackConversationResult extends JsonObject { ok: true; channel: SlackConversation }

export interface SlackIntervalOptions extends SlackOperationOptions {
  readonly start: string;
  readonly end: string;
  readonly rootOldest: string;
  readonly maxRequests: number;
  readonly maxMessages: number;
  /** Full compact ASCII JSON result, including diagnostics and continuation. */
  readonly maxResultBytes?: number;
  readonly pageLimit?: number;
  readonly includeAllMetadata?: boolean;
  readonly resumable?: boolean;
  readonly continuation?: SlackIntervalContinuation | null;
}
export type SlackIntervalSource = "history" | "replies";
export type SlackIntervalState = "pending" | "exhausted" | "stopped" | "not_requested";
export type SlackIntervalStop = "cursor_cycle" | "missing_cursor" | "unusable_cursor"
  | "message_budget" | "thread_not_found" | "request_failure" | "continuation_budget" | "result_budget";
export type SlackIntervalTermination = "finished" | "request_budget" | "message_budget" | "request_failure" | "continuation_budget" | "result_budget";
export type SlackIntervalGapKind = "older_roots_unsearched" | "history_outside_discovery"
  | "invalid_thread_reference" | "invalid_broadcast_reference" | "reply_chain_mismatch"
  | "pagination_contradiction" | "missing_cursor" | "unusable_cursor" | "cursor_cycle"
  | "retention_limited" | "invalid_coverage_metadata" | "thread_not_found" | "request_failure"
  | "request_budget" | "message_budget" | "continuation_budget" | "result_budget";
export interface SlackIntervalSelection {
  readonly channel: string;
  readonly start: string;
  readonly end: string;
  readonly root_oldest: string;
  readonly page_limit: number;
  readonly include_all_metadata: boolean;
}
export interface SlackIntervalPending {
  readonly root_ts: string | null;
  readonly cursor: string | null;
  readonly seen_cursors: readonly string[];
}
/** Caller-retained scheduling data, not proof of prior observations or account authority. */
export interface SlackIntervalContinuation {
  readonly schema_version: 1;
  readonly selection: SlackIntervalSelection;
  readonly pending: readonly SlackIntervalPending[];
  readonly discovered_roots: readonly string[];
}
export interface SlackIntervalError {
  kind: SlackFailureKind;
  status_code: number | null;
  provider_code: string | null;
  retry_after_seconds: number | null;
}
export interface SlackIntervalChain {
  state: SlackIntervalState;
  pages: number;
  stop_reason: SlackIntervalStop | null;
  next_cursor?: string | null;
  has_more?: boolean;
  error?: SlackIntervalError;
}
export interface SlackIntervalThread extends SlackIntervalChain { root_ts: string }
export interface SlackIntervalGap {
  kind: SlackIntervalGapKind;
  source: SlackIntervalSource;
  root_ts?: string;
  message_ts?: string;
}
export interface SlackIntervalMessage {
  source: SlackIntervalSource;
  /** Requested chain context for replies; valid observed root or null for history. */
  root_ts: string | null;
  message: JsonObject;
}
export interface SlackIntervalLimits {
  max_requests: number;
  max_messages: number;
  max_result_bytes: number;
  page_limit: number;
  max_response_bytes: number;
}
export interface SlackIntervalResult {
  channel: string;
  start: string;
  end: string;
  root_oldest: string;
  limits: SlackIntervalLimits;
  messages: SlackIntervalMessage[];
  history: SlackIntervalChain;
  threads: SlackIntervalThread[];
  requests_made: number;
  skipped_non_threadable: number;
  termination: SlackIntervalTermination;
  coverage_gaps: SlackIntervalGap[];
  observation_scope?: "current_call_only";
  continuation?: SlackIntervalContinuation | null;
}

type SlackMethod = "conversations.history" | "conversations.replies" | "auth.test"
  | "conversations.list" | "conversations.info" | "users.info";
const CONVERSATION_TYPES = new Set<SlackConversationType>(["public_channel", "private_channel", "mpim", "im"]);

const PROVIDER_ERRORS = new Map<string, SlackFailureKind>([
  ...["not_authed", "invalid_auth", "token_expired", "token_revoked", "account_inactive"]
    .map((code): [string, SlackFailureKind] => [code, "authentication"]),
  ...["missing_scope", "no_permission", "access_denied", "accesslimited", "not_allowed_token_type",
    "team_access_not_granted", "not_in_channel"]
    .map((code): [string, SlackFailureKind] => [code, "permission"]),
  ["channel_not_found", "not_found"], ["thread_not_found", "not_found"], ["user_not_found", "not_found"], ["ratelimited", "rate_limited"],
  ...["invalid_cursor", "invalid_arguments", "invalid_ts_latest", "invalid_ts_oldest"]
    .map((code): [string, SlackFailureKind] => [code, "provider"]),
]);

function timestamp(value: string): string {
  if (typeof value !== "string" || value.length > 64 || /^[0-9]+(?:\.[0-9]+)?$/.exec(value)?.[0] !== value) {
    throw new TypeError("expected a decimal timestamp string of at most 64 characters");
  }
  return value;
}
function cursor(value: string): string {
  if (typeof value !== "string" || !value.length || /[\x00-\x1f\x7f]/.test(value)) {
    throw new TypeError("expected a nonempty cursor without control characters");
  }
  // TextEncoder replaces unpaired surrogates. Reject them instead of rewriting a cursor.
  for (let index = 0; index < value.length; index++) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(++index);
      if (!(next >= 0xdc00 && next <= 0xdfff)) throw new TypeError("cursor must be valid Unicode");
    } else if (unit >= 0xdc00 && unit <= 0xdfff) throw new TypeError("cursor must be valid Unicode");
  }
  if (new TextEncoder().encode(value).byteLength > 8192) throw new TypeError("cursor exceeds UTF-8 limit");
  return value;
}
function page(value: JsonValue): value is SlackPage {
  if (!isObject(value) || value.ok !== true || !Array.isArray(value.messages)) return false;
  for (const message of value.messages) {
    if (!isObject(message) || typeof message.ts !== "string") return false;
    try { timestamp(message.ts); } catch { return false; }
  }
  if (Object.hasOwn(value, "has_more") && typeof value.has_more !== "boolean") return false;
  if (Object.hasOwn(value, "response_metadata")) {
    if (!isObject(value.response_metadata)) return false;
    const metadata = value.response_metadata;
    if (Object.hasOwn(metadata, "next_cursor") && metadata.next_cursor !== null
      && typeof metadata.next_cursor !== "string") return false;
  }
  return true;
}

function identifier(value: unknown): string {
  if (typeof value !== "string" || !value.length || value.length > 128 || /[^A-Za-z0-9]/.test(value)) {
    throw new TypeError("expected a bounded alphanumeric Slack identifier");
  }
  return value;
}
function authObservation(value: JsonValue): value is SlackAuthObservation {
  if (!isObject(value) || value.ok !== true) return false;
  try { identifier(value.team_id); identifier(value.user_id); } catch { return false; }
  return true;
}
function conversationsPage(value: JsonValue): value is SlackConversationsPage {
  if (!isObject(value) || value.ok !== true || !Array.isArray(value.channels)) return false;
  for (const channel of value.channels) {
    if (!isObject(channel)) return false;
    try { identifier(channel.id); } catch { return false; }
  }
  if (Object.hasOwn(value, "response_metadata")) {
    if (!isObject(value.response_metadata)) return false;
    const metadata = value.response_metadata;
    if (Object.hasOwn(metadata, "next_cursor") && metadata.next_cursor !== null
      && typeof metadata.next_cursor !== "string") return false;
  }
  return true;
}
function conversationResult(value: JsonValue, channel: string): value is SlackConversationResult {
  return isObject(value) && value.ok === true && isObject(value.channel) && value.channel.id === channel;
}
function userResult(value: JsonValue, user: string): value is SlackUserResult {
  return isObject(value) && value.ok === true && isObject(value.user) && value.user.id === user;
}

/** Compare validated decimal strings without rewriting their provider representation. */
function decimalCompare(left: string, right: string): number {
  const [leftWholeRaw = "", leftFractionRaw = ""] = left.split(".");
  const [rightWholeRaw = "", rightFractionRaw = ""] = right.split(".");
  const leftWhole = leftWholeRaw.replace(/^0+/, "") || "0";
  const rightWhole = rightWholeRaw.replace(/^0+/, "") || "0";
  if (leftWhole.length !== rightWhole.length) return leftWhole.length > rightWhole.length ? 1 : -1;
  if (leftWhole !== rightWhole) return leftWhole > rightWhole ? 1 : -1;
  const width = Math.max(leftFractionRaw.length, rightFractionRaw.length);
  const leftFraction = leftFractionRaw.padEnd(width, "0");
  const rightFraction = rightFractionRaw.padEnd(width, "0");
  return leftFraction === rightFraction ? 0 : leftFraction > rightFraction ? 1 : -1;
}

function intervalReference(message: JsonObject): [string | null, SlackIntervalGapKind | null] {
  let root = message.ts as string;
  if (Object.hasOwn(message, "thread_ts")) {
    const candidate = message.thread_ts;
    try { timestamp(candidate as string); } catch { return [null, "invalid_thread_reference"]; }
    if (decimalCompare(candidate as string, root) > 0) return [null, "invalid_thread_reference"];
    root = candidate as string;
  }
  if (message.subtype === "thread_broadcast" || message.subtype === "reply_broadcast") {
    if (!Object.hasOwn(message, "thread_ts")) return [null, "invalid_broadcast_reference"];
    if (Object.hasOwn(message, "root") && (!isObject(message.root) || message.root.ts !== root)) {
      return [null, "invalid_broadcast_reference"];
    }
  }
  return [root, null];
}

const MAX_CONTINUATION_ROOTS = 1024;
const MAX_CONTINUATION_PENDING = 1025;
const MAX_CONTINUATION_CURSORS = 2048;
const MAX_CONTINUATION_BYTES = 262144;

/** Inspect only plain data properties, never invoke a supplied getter or toJSON. */
function continuationRecord(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)
    || ![Object.prototype, null].includes(Object.getPrototypeOf(value))) throw new TypeError();
  const actual = Reflect.ownKeys(value);
  if (actual.length !== keys.length || actual.some((key) => typeof key !== "string" || !keys.includes(key))) throw new TypeError();
  const result: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
  for (const key of keys) {
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor || !("value" in descriptor) || !descriptor.enumerable) throw new TypeError();
    result[key] = descriptor.value;
  }
  return result;
}

function continuationArray(value: unknown, maximum: number): unknown[] {
  if (!Array.isArray(value) || value.length > maximum || Object.getPrototypeOf(value) !== Array.prototype
    || Reflect.ownKeys(value).length !== value.length + 1) throw new TypeError();
  const result: unknown[] = [];
  for (let index = 0; index < value.length; index++) {
    const descriptor = Object.getOwnPropertyDescriptor(value, String(index));
    if (!descriptor || !("value" in descriptor) || !descriptor.enumerable) throw new TypeError();
    result.push(descriptor.value);
  }
  return result;
}

function continuationFits(value: SlackIntervalContinuation): boolean {
  return value.discovered_roots.length <= MAX_CONTINUATION_ROOTS
    && value.pending.length <= MAX_CONTINUATION_PENDING
    && value.pending.reduce((total, step) => total + step.seen_cursors.length, 0) <= MAX_CONTINUATION_CURSORS
    && new TextEncoder().encode(JSON.stringify(value)).byteLength <= MAX_CONTINUATION_BYTES;
}

function snapshotContinuation(value: unknown, selection: SlackIntervalSelection): SlackIntervalContinuation {
  try {
    const record = continuationRecord(value, ["schema_version", "selection", "pending", "discovered_roots"]);
    if (record.schema_version !== 1) throw new TypeError();
    const selected = continuationRecord(record.selection, Object.keys(selection));
    for (const key of Object.keys(selection) as (keyof SlackIntervalSelection)[]) {
      if (selected[key] !== selection[key]) throw new TypeError();
    }
    const roots = continuationArray(record.discovered_roots, MAX_CONTINUATION_ROOTS).map((root) => {
      if (typeof root !== "string" || decimalCompare(timestamp(root), selection.end) >= 0) throw new TypeError();
      return root;
    });
    const rootSet = new Set(roots);
    if (rootSet.size !== roots.length) throw new TypeError();
    const pendingRoots = new Set<string | null>();
    let cursorCount = 0;
    const pending = continuationArray(record.pending, MAX_CONTINUATION_PENDING).map((item): SlackIntervalPending => {
      const step = continuationRecord(item, ["root_ts", "cursor", "seen_cursors"]);
      if (step.root_ts !== null && (typeof step.root_ts !== "string" || !rootSet.has(step.root_ts))) throw new TypeError();
      const root = step.root_ts as string | null;
      if (pendingRoots.has(root)) throw new TypeError();
      pendingRoots.add(root);
      const selectedCursor = step.cursor === null ? null : cursor(step.cursor as string);
      const seen = continuationArray(step.seen_cursors, MAX_CONTINUATION_CURSORS - cursorCount).map((item) => cursor(item as string));
      cursorCount += seen.length;
      if (new Set(seen).size !== seen.length
        || (selectedCursor === null ? seen.length !== 0 : seen.at(-1) !== selectedCursor)) throw new TypeError();
      return { root_ts: root, cursor: selectedCursor, seen_cursors: seen };
    });
    const snapshot: SlackIntervalContinuation = { schema_version: 1, selection: { ...selection }, pending, discovered_roots: roots };
    if (!pending.length || !continuationFits(snapshot)) throw new TypeError();
    return snapshot;
  } catch {
    throw new TypeError("invalid interval continuation");
  }
}

/** Single-page operations plus opt-in bounded intervals. Host owns account, policy and transport lifetime. */
export class SlackOperations {
  readonly #base: string;
  readonly #fetch: SlackContext["fetch"];
  readonly #timeoutMs: number;
  readonly #maxBytes: number;
  readonly #allowedChannelIds: readonly string[] | null;

  constructor(context: SlackContext, options: SlackOptions = {}) {
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
    if (!base.pathname.endsWith("/")) base.pathname += "/";
    this.#base = base.href;
    const fetch = context.fetch;
    this.#fetch = (request) => fetch(request);
    this.#timeoutMs = options.timeoutMs === undefined ? 20_000 : options.timeoutMs;
    this.#maxBytes = options.maxResponseBytes === undefined ? 2_097_152 : options.maxResponseBytes;
    if (!Number.isFinite(this.#timeoutMs) || this.#timeoutMs <= 0 || this.#timeoutMs > 2_147_483_647) {
      throw new TypeError("timeoutMs must be finite, positive and within the timer range");
    }
    if (!Number.isSafeInteger(this.#maxBytes) || this.#maxBytes <= 0) {
      throw new TypeError("maxResponseBytes must be a positive safe integer");
    }
    if (options.allowedChannelIds === undefined) {
      this.#allowedChannelIds = null;
    } else {
      try {
        const channels = continuationArray(options.allowedChannelIds, 256).map(identifier);
        if (new Set(channels).size !== channels.length) throw new TypeError();
        this.#allowedChannelIds = Object.freeze(channels);
      } catch {
        throw new TypeError("allowedChannelIds must contain at most 256 unique Slack identifiers");
      }
    }
  }

  #channel(value: string): string {
    const channel = identifier(value);
    if (this.#allowedChannelIds !== null && !this.#allowedChannelIds.includes(channel)) {
      throw new SlackError("permission");
    }
    return channel;
  }

  #failure(kind: SlackFailureKind, response?: Response, code: string | null = null): SlackError {
    const retry = response?.headers.get("retry-after") ?? "";
    return new SlackError(kind, response?.status ?? null, code,
      /^[0-9]{1,9}$/.test(retry) ? Number(retry) : null);
  }

  async #request<T extends JsonObject>(method: SlackMethod,
    params: readonly (readonly [string, string])[], validate: (value: JsonValue) => value is T,
    signal?: AbortSignal): Promise<T> {
    if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
    const url = new URL(`api/${method}`, this.#base);
    for (const [key, value] of params) url.searchParams.append(key, value);
    const controller = new AbortController();
    let stopped: "cancelled" | "timeout" | undefined;
    const stop = (kind: "cancelled" | "timeout"): void => {
      if (stopped === undefined) { stopped = kind; controller.abort(); }
    };
    const onCancel = (): void => stop("cancelled");
    const deadline = performance.now() + this.#timeoutMs;
    const timer = setTimeout(() => stop("timeout"), this.#timeoutMs);
    const checkStopped = (): void => {
      if (performance.now() >= deadline) stop("timeout");
      if (stopped !== undefined) throw new Error();
    };
    let response: Response | undefined;
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let complete = false;
    let failure: SlackError | undefined;
    let result: T | undefined;
    signal?.addEventListener("abort", onCancel, { once: true });
    if (signal?.aborted) onCancel();
    try {
      checkStopped();
      response = await this.#fetch(new Request(url, {
        method: method === "auth.test" ? "POST" : "GET",
        headers: { Accept: "application/json", "Accept-Encoding": "identity",
          ...(method === "auth.test" ? { "Content-Type": "application/x-www-form-urlencoded" } : {}) },
        ...(method === "auth.test" ? { body: "" } : {}),
        redirect: "manual", credentials: "omit", cache: "no-store", signal: controller.signal,
      }));
      checkStopped();
      const fail: (kind: SlackFailureKind, code?: string | null) => never = (kind, code = null) => {
        failure = this.#failure(kind, response, code);
        throw new Error();
      };
      if (response.redirected || response.type === "opaqueredirect"
        || (response.url !== "" && response.url !== url.href)) fail("redirect");
      if (response.status !== 200) {
        if (response.status >= 300 && response.status < 400) fail("redirect");
        fail(({ 401: "authentication", 403: "permission", 404: "not_found", 429: "rate_limited" } as
          Record<number, SlackFailureKind>)[response.status] ?? "http_status");
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
      const chunks: string[] = [];
      let bytes = 0;
      while (true) {
        const chunk = await reader.read();
        checkStopped();
        if (chunk.done) { complete = true; break; }
        bytes += chunk.value.byteLength;
        if (bytes > this.#maxBytes) fail("response_too_large");
        try { chunks.push(decoder.decode(chunk.value, { stream: true })); }
        catch { fail("invalid_response"); }
      }
      let payload: JsonValue = null;
      if (length !== null && bytes !== Number(length)) fail("invalid_response");
      try { chunks.push(decoder.decode()); payload = receiptJson(chunks.join("")); }
      catch { fail("invalid_response"); }
      if (isObject(payload) && payload.ok === false) {
        const code = payload.error;
        if (typeof code !== "string" || !code.length) fail("invalid_response");
        const kind = PROVIDER_ERRORS.get(code);
        fail(kind ?? "provider", kind === undefined ? null : code);
      }
      if (!validate(payload)) fail("invalid_response");
      result = payload;
      checkStopped();
    } catch {
      failure ??= this.#failure("transport");
    } finally {
      // A deadline/abort requests cleanup; the same call still owns and joins it.
      try {
        if (reader) { if (!complete) await reader.cancel(); }
        else if (response?.body) await response.body.cancel();
      } catch { failure ??= this.#failure("transport"); }
      finally { reader?.releaseLock(); clearTimeout(timer); signal?.removeEventListener("abort", onCancel); }
    }
    if (stopped === "cancelled") throw new SlackCancelled();
    if (stopped === "timeout") throw this.#failure("timeout");
    if (failure) throw failure;
    if (!result) throw this.#failure("invalid_response");
    return result;
  }

  #params(channel: string, options: SlackPageOptions): [string, string][] {
    if (typeof channel !== "string" || !channel.length || channel.length > 128 || /[^A-Za-z0-9]/.test(channel)) {
      throw new TypeError("expected an alphanumeric conversation ID of at most 128 characters");
    }
    this.#channel(channel);
    const limit = options.limit === undefined ? 15 : options.limit;
    const inclusive = options.inclusive === undefined ? false : options.inclusive;
    const metadata = options.includeAllMetadata === undefined ? false : options.includeAllMetadata;
    if (!Number.isInteger(limit) || limit < 1 || limit > 999) throw new TypeError("limit must be an integer from 1 to 999");
    if (typeof inclusive !== "boolean" || typeof metadata !== "boolean") throw new TypeError("flags must be boolean");
    const params: [string, string][] = [["channel", channel], ["limit", String(limit)],
      ["inclusive", String(inclusive)], ["include_all_metadata", String(metadata)]];
    if (options.oldest !== undefined) params.push(["oldest", timestamp(options.oldest)]);
    if (options.latest !== undefined) params.push(["latest", timestamp(options.latest)]);
    if (options.cursor !== undefined) params.push(["cursor", cursor(options.cursor)]);
    return params;
  }

  async historyPage(channel: string, options: SlackPageOptions = {}): Promise<SlackPage> {
    return this.#request("conversations.history", this.#params(channel, options), page, options.signal);
  }
  async repliesPage(channel: string, ts: string, options: SlackPageOptions = {}): Promise<SlackPage> {
    const params = this.#params(channel, options);
    params.push(["ts", timestamp(ts)]);
    return this.#request("conversations.replies", params, page, options.signal);
  }

  /** Explicit bounded creation-time retrieval. Finished is not a historical completeness claim. */
  async readInterval(channel: string, options: SlackIntervalOptions): Promise<SlackIntervalResult> {
    const { start, end, rootOldest, maxRequests, maxMessages, pageLimit = 15,
      includeAllMetadata = false, signal, resumable = false, continuation, maxResultBytes = 262144 } = options;
    timestamp(start); timestamp(end); timestamp(rootOldest);
    if (decimalCompare(rootOldest, start) > 0 || decimalCompare(start, end) >= 0) {
      throw new TypeError("expected rootOldest <= start < end");
    }
    for (const budget of [maxRequests, maxMessages]) {
      if (!Number.isSafeInteger(budget) || budget <= 0) throw new TypeError("interval budgets must be positive safe integers");
    }
    if (!Number.isSafeInteger(maxResultBytes) || maxResultBytes < 4096 || maxResultBytes > 16777216) {
      throw new TypeError("maxResultBytes must be an integer from 4096 to 16777216");
    }
    if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
    this.#params(channel, { oldest: rootOldest, latest: end, inclusive: true,
      limit: pageLimit, includeAllMetadata });
    if (typeof resumable !== "boolean" || (continuation != null && !resumable)) {
      throw new TypeError("continuation requires resumable=true");
    }
    const selection: SlackIntervalSelection = { channel, start, end, root_oldest: rootOldest,
      page_limit: pageLimit, include_all_metadata: includeAllMetadata };
    const incoming = continuation === undefined || continuation === null ? null
      : snapshotContinuation(continuation, selection);
    const history: SlackIntervalChain = { state: incoming !== null
      && !incoming.pending.some((step) => step.root_ts === null) ? "not_requested" : "pending", pages: 0, stop_reason: null };
    let result: SlackIntervalResult = {
      channel, start, end, root_oldest: rootOldest,
      limits: { max_requests: maxRequests, max_messages: maxMessages,
        page_limit: pageLimit, max_response_bytes: this.#maxBytes, max_result_bytes: maxResultBytes },
      messages: [], history, threads: [], requests_made: 0, skipped_non_threadable: 0,
      termination: "finished", coverage_gaps: [],
    };
    type Step = { root: string | null; chain: SlackIntervalChain; selectedCursor: string | undefined; seen: Set<string> };
    let queue: Step[] = incoming === null
      ? [{ root: null, chain: history, selectedCursor: undefined, seen: new Set() }]
      : incoming.pending.map((step) => {
        const chain: SlackIntervalChain = step.root_ts === null ? history
          : { root_ts: step.root_ts, state: "pending", pages: 0, stop_reason: null } as SlackIntervalThread;
        if (step.root_ts !== null) result.threads.push(chain as SlackIntervalThread);
        return { root: step.root_ts, chain, selectedCursor: step.cursor ?? undefined, seen: new Set(step.seen_cursors) };
      });
    let roots = new Set(incoming?.discovered_roots ?? []);
    const checkpoint = (pending: readonly Step[], discovered: ReadonlySet<string>): SlackIntervalContinuation | null =>
      pending.length === 0 ? null : { schema_version: 1, selection: { ...selection },
        pending: pending.map((step) => ({ root_ts: step.root, cursor: step.selectedCursor ?? null,
          seen_cursors: [...step.seen] })), discovered_roots: [...discovered] };
    const projected = (value: SlackIntervalResult, pending: readonly Step[], discovered: ReadonlySet<string>): SlackIntervalResult =>
      resumable ? { ...value, observation_scope: "current_call_only", continuation: checkpoint(pending, discovered) } : value;
    const fits = (value: SlackIntervalResult): boolean => {
      // JSON.stringify is compact. Escape UTF-16 units, including DEL, to measure ASCII JSON.
      const ascii = JSON.stringify(value).replace(/[\u007f-\uffff]/g,
        (unit) => `\\u${unit.charCodeAt(0).toString(16).padStart(4, "0")}`);
      return ascii.length <= maxResultBytes;
    };
    const reservedError: SlackIntervalError = { kind: "unsupported_encoding", status_code: null,
      provider_code: [...PROVIDER_ERRORS.keys()].reduce((a, b) => a.length >= b.length ? a : b),
      retry_after_seconds: 999999999 };
    const capacity = (value: SlackIntervalResult, pending: readonly Step[], discovered: ReadonlySet<string>): boolean => {
      if (!fits(projected(value, pending, discovered))) return false;
      const step = pending[0];
      if (step === undefined) return true;
      const stops = value.requests_made >= maxRequests ? ["request_budget"] as const
        : ["result_budget", "message_budget", "continuation_budget", "request_failure"] as const;
      for (const reason of stops) {
        const chain: SlackIntervalChain = reason === "request_budget" ? step.chain
          : { ...step.chain, state: "stopped", stop_reason: reason,
            ...(reason === "request_failure" ? { error: reservedError } : {}) };
        const stopped: SlackIntervalResult = { ...value,
          history: value.history === step.chain ? chain : value.history,
          threads: value.threads.map((item) => item === step.chain ? chain as SlackIntervalThread : item),
          requests_made: value.requests_made + (reason === "request_budget" ? 0 : 1),
          termination: reason,
          coverage_gaps: [...value.coverage_gaps, { kind: reason,
            source: step.root === null ? "history" : "replies",
            ...(step.root === null ? {} : { root_ts: step.root }) }] };
        if (!fits(projected(stopped, pending, discovered))) return false;
      }
      return true;
    };
    const gap = (kind: SlackIntervalGapKind, root: string | null, messageTs?: string): void => {
      result.coverage_gaps.push({ kind, source: root === null ? "history" : "replies",
        ...(root === null ? {} : { root_ts: root }),
        ...(messageTs === undefined ? {} : { message_ts: messageTs }) });
    };
    if (decimalCompare(rootOldest, "0") > 0) gap("older_roots_unsearched", null);
    let head = 0;
    type Snapshot = { result: SlackIntervalResult; queue: Step[]; roots: Set<string> };
    const refusePage = (before: Snapshot): void => {
      ({ result, queue, roots } = before);
      result.requests_made++;
      const step = queue[head]!;
      step.chain.state = "stopped"; step.chain.stop_reason = "result_budget";
      result.termination = "result_budget";
      gap("result_budget", step.root);
    };
    if (!capacity(result, queue, roots)) throw new TypeError("interval result budget cannot represent pending work");
    for (; head < queue.length; head++) {
      const { root, chain, selectedCursor, seen } = queue[head]!;
      if (result.requests_made >= maxRequests) {
        result.termination = "request_budget";
        gap("request_budget", root);
        break;
      }
      if (!capacity(result, queue.slice(head), roots)) throw new TypeError("interval result budget cannot represent pending work");
      // Clone the graph together so queued steps still refer to the cloned result chains.
      const before: Snapshot = structuredClone({ result, queue, roots });
      const source: SlackIntervalSource = root === null ? "history" : "replies";
      const pageOptions: SlackPageOptions = {
        oldest: root === null ? rootOldest : start, latest: end, inclusive: true,
        limit: pageLimit, includeAllMetadata,
        ...(selectedCursor === undefined ? {} : { cursor: selectedCursor }),
        ...(signal === undefined ? {} : { signal }),
      };
      result.requests_made++;
      let value: SlackPage;
      try {
        value = root === null ? await this.historyPage(channel, pageOptions)
          : await this.repliesPage(channel, root, pageOptions);
      } catch (error) {
        if (error instanceof SlackCancelled || !(error instanceof SlackError)) throw error;
        const unavailable = root !== null && error.providerCode === "thread_not_found";
        const reason = unavailable ? "thread_not_found" : "request_failure";
        chain.state = "stopped"; chain.stop_reason = reason;
        chain.error = { kind: error.kind, status_code: error.statusCode,
          provider_code: error.providerCode, retry_after_seconds: error.retryAfterSeconds };
        gap(reason, root);
        if (unavailable) {
          if (!capacity(result, queue.slice(head + 1), roots)) { refusePage(before); break; }
          continue;
        }
        result.termination = "request_failure";
        if (!fits(projected(result, queue.slice(head), roots))) refusePage(before);
        break;
      }
      const selected = value.messages.filter((row) => decimalCompare(row.ts as string, start) >= 0
        && decimalCompare(row.ts as string, end) < 0);
      if (selected.length > maxMessages - result.messages.length) {
        chain.state = "stopped"; chain.stop_reason = "message_budget";
        result.termination = "message_budget";
        gap("message_budget", root);
        break;
      }
      // Plan the whole page before admitting observations or advancing its checkpoint.
      const stagedRoots = new Set(roots);
      const additions: Step[] = [];
      const pageGaps: SlackIntervalGap[] = [];
      const messages: SlackIntervalMessage[] = [];
      let skipped = 0;
      const pageGap = (kind: SlackIntervalGapKind, messageTs?: string): void => {
        pageGaps.push({ kind, source, ...(root === null ? {} : { root_ts: root }),
          ...(messageTs === undefined ? {} : { message_ts: messageTs }) });
      };
      for (const row of value.messages) {
        const ts = row.ts as string;
        let observedRoot: string | null = null;
        if (root === null && (decimalCompare(ts, rootOldest) < 0 || decimalCompare(ts, end) > 0)) {
          pageGap("history_outside_discovery", ts);
        } else if (root === null && (row.subtype === "channel_join" || row.subtype === "channel_leave")) {
          skipped++;
        } else {
          const [reference, referenceGap] = intervalReference(row);
          observedRoot = reference;
          if (referenceGap !== null) pageGap(referenceGap, ts);
          else if (root !== null && observedRoot !== root) pageGap("reply_chain_mismatch", ts);
          else if (root === null && observedRoot !== null
            && decimalCompare(observedRoot, end) < 0 && !stagedRoots.has(observedRoot)) {
            stagedRoots.add(observedRoot);
            const thread: SlackIntervalThread = { root_ts: observedRoot, state: "pending", pages: 0, stop_reason: null };
            additions.push({ root: observedRoot, chain: thread, selectedCursor: undefined, seen: new Set() });
          }
        }
        if (decimalCompare(ts, start) >= 0 && decimalCompare(ts, end) < 0) {
          messages.push({ source, root_ts: root === null ? observedRoot : root, message: row });
        }
      }
      if (Object.hasOwn(value, "is_limited")) {
        if (typeof value.is_limited !== "boolean") pageGap("invalid_coverage_metadata");
        else if (value.is_limited) pageGap("retention_limited");
      }
      const metadata = isObject(value.response_metadata) ? value.response_metadata : {};
      const nextCursor = metadata.next_cursor;
      let stop: SlackIntervalStop | null = null;
      let following: Step | null = null;
      if (typeof nextCursor === "string" && nextCursor.length > 0) {
        try { cursor(nextCursor); } catch { stop = "unusable_cursor"; }
        if (stop === null && seen.has(nextCursor)) stop = "cursor_cycle";
        if (stop === null) {
          if (value.has_more === false) pageGap("pagination_contradiction");
          following = { root, chain, selectedCursor: nextCursor, seen: new Set([...seen, nextCursor]) };
        }
      } else if (value.has_more === true) {
        stop = Object.hasOwn(metadata, "next_cursor") ? "unusable_cursor" : "missing_cursor";
      }
      if (resumable) {
        const planned = checkpoint([...queue.slice(head + 1), ...additions, ...(following === null ? [] : [following])], stagedRoots);
        if (planned !== null && !continuationFits(planned)) {
          chain.state = "stopped"; chain.stop_reason = "continuation_budget";
          result.termination = "continuation_budget";
          gap("continuation_budget", root);
          break;
        }
      }
      chain.pages++;
      delete chain.next_cursor; delete chain.has_more;
      if (Object.hasOwn(metadata, "next_cursor")) chain.next_cursor = metadata.next_cursor as string | null;
      if (Object.hasOwn(value, "has_more")) chain.has_more = value.has_more as boolean;
      for (const discovered of stagedRoots) roots.add(discovered);
      for (const message of messages) result.messages.push(message);
      for (const item of pageGaps) result.coverage_gaps.push(item);
      result.skipped_non_threadable += skipped;
      for (const step of additions) result.threads.push(step.chain as SlackIntervalThread);
      for (const step of additions) queue.push(step);
      if (following !== null) queue.push(following);
      else if (stop !== null) {
        chain.state = "stopped"; chain.stop_reason = stop;
        gap(stop, root);
      } else chain.state = "exhausted";
      if (!capacity(result, queue.slice(head + 1), roots)) { refusePage(before); break; }
    }
    if (resumable) {
      result.observation_scope = "current_call_only";
      result.continuation = checkpoint(queue.slice(head), roots);
    }
    if (!fits(result)) throw new TypeError("interval result exceeded its byte budget");
    return result;
  }

  /** Observe the selected provider identity. This does not establish a host binding or grant. */
  async authTest(options: SlackOperationOptions = {}): Promise<SlackAuthObservation> {
    return this.#request("auth.test", [], authObservation, options.signal);
  }

  /** Observe an explicitly selected user; this does not establish channel membership or a grant. */
  async getUser(user: string, options: SlackUserGetOptions = {}): Promise<SlackUserResult> {
    user = identifier(user);
    const { includeLocale = false, signal } = options;
    if (typeof includeLocale !== "boolean") throw new TypeError("includeLocale must be boolean");
    return this.#request("users.info", [["user", user], ["include_locale", String(includeLocale)]],
      (value): value is SlackUserResult => userResult(value, user), signal);
  }

  /** Read one caller-selected page, preserving empty nonterminal pages and every provider field. */
  async listConversationsPage(options: SlackConversationListOptions = {}): Promise<SlackConversationsPage> {
    if (this.#allowedChannelIds !== null) throw new SlackError("permission");
    const { types = ["public_channel"], excludeArchived = false, cursor: selectedCursor,
      limit = 100, teamId, signal } = options;
    const selectedTypes = Array.isArray(types) ? Array.from(types) : [];
    if (selectedTypes.length < 1 || selectedTypes.length > 4
      || selectedTypes.some((value) => !CONVERSATION_TYPES.has(value)) || new Set(selectedTypes).size !== selectedTypes.length) {
      throw new TypeError("types must contain one to four unique supported conversation types");
    }
    if (typeof excludeArchived !== "boolean") throw new TypeError("excludeArchived must be boolean");
    if (!Number.isInteger(limit) || limit < 1 || limit > 999) throw new TypeError("limit must be an integer from 1 to 999");
    const params: [string, string][] = [["types", selectedTypes.join(",")],
      ["exclude_archived", String(excludeArchived)], ["limit", String(limit)]];
    if (selectedCursor !== undefined) params.push(["cursor", cursor(selectedCursor)]);
    if (teamId !== undefined) params.push(["team_id", identifier(teamId)]);
    return this.#request("conversations.list", params, conversationsPage, signal);
  }

  /** Read the exact selected conversation; metadata is not audience or membership authority. */
  async getConversation(channel: string, options: SlackConversationGetOptions = {}): Promise<SlackConversationResult> {
    channel = this.#channel(channel);
    const { includeLocale = false, includeNumMembers = false, signal } = options;
    if (typeof includeLocale !== "boolean" || typeof includeNumMembers !== "boolean") {
      throw new TypeError("conversation detail flags must be boolean");
    }
    return this.#request("conversations.info", [["channel", channel],
      ["include_locale", String(includeLocale)], ["include_num_members", String(includeNumMembers)]],
    (value): value is SlackConversationResult => conversationResult(value, channel), signal);
  }
}
