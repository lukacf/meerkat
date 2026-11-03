import { displayableImageMediaType } from "./blob-image";
import { consumeSseResponse } from "./sse-reader";
import { createTimelineSubscription, type ConsoleTimelineSubscription, type ConsoleTimelineSubscriptionOptions, type ConsoleStreamFailure } from "./timeline-subscription";
import {
  normalizeConsoleInteractionRejectedError,
  normalizeReplayUnavailableError,
} from "./control-plane";
import {
  CONSOLE_BLOB_PATH_PREFIX,
  CONSOLE_REST_PATHS,
  CONSOLE_RPC_METHODS,
  CONSOLE_RPC_PATHS,
  CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE,
} from "./contract";
import type {
  ConsoleFrame,
  ConsoleGatewayInteractionRejectedError,
  ConsoleReplayUnavailablePayload,
  ConsoleTimelineAccepted,
  ConsoleTimelinePage,
} from "./runtime-types";

function unwrapConsoleEnvelope(
  eventName: string,
  data: unknown,
): {
  id?: string;
  event?: string;
  identity?: string;
  interactionId?: string;
  timestampMs?: number;
  cursor?: string;
  runtimeKey?: string;
  sessionId?: string;
  status?: string;
  sourceKind?: string;
  sourceCursor?: string;
  frameVersion?: number;
  updatedAtMs?: number;
  turnId?: string;
  runId?: string;
  data: unknown;
} {
  if (!data || typeof data !== "object") {
    return { data };
  }
  const record = data as Record<string, unknown>;
  if (typeof record.type === "string" && "frame" in record) {
    const frame = timelineFrameToConsoleFrame(record.frame);
    const isUpdateEnvelope = eventName === "frame_updated";
    return {
      id: frame.id,
      event: isUpdateEnvelope ? "frame_updated" : frame.event,
      identity: frame.identity,
      interactionId: frame.interactionId,
      timestampMs: frame.timestampMs,
      cursor: frame.cursor,
      runtimeKey: frame.runtimeKey,
      sessionId: frame.sessionId,
      status: frame.status,
      sourceKind: frame.sourceKind,
      sourceCursor: frame.sourceCursor,
      frameVersion: frame.frameVersion,
      updatedAtMs: frame.updatedAtMs,
      turnId: frame.turnId,
      runId: frame.runId,
      data: isUpdateEnvelope
        ? frame.event === "frame_updated"
          ? frame.data
          : { frame }
        : frame.data,
    };
  }
  return { data };
}

function timelineFrameToConsoleFrame(raw: unknown): ConsoleFrame {
  if (!raw || typeof raw !== "object") {
    return { id: "", event: "event", data: raw };
  }
  const record = raw as Record<string, unknown>;
  const cursor = typeof record.cursor === "string" ? record.cursor : undefined;
  const payload = "payload" in record ? record.payload : record;
  const source = record.source && typeof record.source === "object"
    ? record.source as Record<string, unknown>
    : null;
  if (
    record.kind === "frame_updated" &&
    payload &&
    typeof payload === "object" &&
    "frame" in payload
  ) {
    const updated = timelineFrameToConsoleFrame((payload as Record<string, unknown>).frame);
    return {
      id: String(record.id || cursor || ""),
      event: "frame_updated",
      identity: typeof record.identity === "string" ? record.identity : updated.identity,
      interactionId:
        typeof record.interaction_id === "string" ? record.interaction_id : updated.interactionId,
      timestampMs: typeof record.timestamp_ms === "number" ? record.timestamp_ms : undefined,
      cursor,
      runtimeKey: typeof record.runtime_key === "string" ? record.runtime_key : updated.runtimeKey,
      sessionId: typeof record.session_id === "string" ? record.session_id : updated.sessionId,
      status: typeof record.status === "string" ? record.status : updated.status,
      sourceKind: source && typeof source.kind === "string" ? source.kind : updated.sourceKind,
      sourceCursor: source && typeof source.source_cursor === "string" ? source.source_cursor : updated.sourceCursor,
      frameVersion:
        typeof record.frame_version === "number" ? record.frame_version : updated.frameVersion,
      updatedAtMs:
        typeof record.updated_at_ms === "number" ? record.updated_at_ms : updated.updatedAtMs,
      turnId: typeof record.turn_id === "string" ? record.turn_id : updated.turnId,
      runId: typeof record.run_id === "string" ? record.run_id : updated.runId,
      data: { frame: updated },
    };
  }
  return {
    id: String(record.id || cursor || ""),
    event: String(record.kind || "event"),
    identity: typeof record.identity === "string" ? record.identity : undefined,
    interactionId: typeof record.interaction_id === "string" ? record.interaction_id : undefined,
    timestampMs: typeof record.timestamp_ms === "number" ? record.timestamp_ms : undefined,
    cursor,
    runtimeKey: typeof record.runtime_key === "string" ? record.runtime_key : undefined,
    sessionId: typeof record.session_id === "string" ? record.session_id : undefined,
    status: typeof record.status === "string" ? record.status : undefined,
    sourceKind: source && typeof source.kind === "string" ? source.kind : undefined,
    sourceCursor: source && typeof source.source_cursor === "string" ? source.source_cursor : undefined,
    frameVersion: typeof record.frame_version === "number" ? record.frame_version : undefined,
    updatedAtMs: typeof record.updated_at_ms === "number" ? record.updated_at_ms : undefined,
    turnId: typeof record.turn_id === "string" ? record.turn_id : undefined,
    runId: typeof record.run_id === "string" ? record.run_id : undefined,
    data: payload,
  };
}

export function parseSseFrames(rawText: string): ConsoleFrame[] {
  const blocks = rawText
    .split(/\n\n+/)
    .map((part) => part.trim())
    .filter(Boolean);
  const frames: ConsoleFrame[] = [];

  for (const block of blocks) {
    const lines = block.split("\n");
    let id = "";
    let event = "message";
    const dataLines: string[] = [];

    for (const line of lines) {
      if (line.startsWith("id:")) {
        id = line.slice(3).trim();
        continue;
      }
      if (line.startsWith("event:")) {
        event = line.slice(6).trim();
        continue;
      }
      if (line.startsWith("data:")) {
        dataLines.push(line.slice(5).trim());
      }
    }

    if (!id && dataLines.length === 0) {
      continue;
    }

    const rawData = dataLines.join("\n");
    let data: unknown = rawData;
    if (rawData) {
      try {
        data = JSON.parse(rawData);
      } catch (_) {
        data = rawData;
      }
    }

    const normalized = unwrapConsoleEnvelope(event, data);
    frames.push({
      id: normalized.id || id,
      event: normalized.event || event,
      identity: normalized.identity,
      interactionId: normalized.interactionId,
      timestampMs: normalized.timestampMs,
      cursor: normalized.cursor,
      runtimeKey: normalized.runtimeKey,
      sessionId: normalized.sessionId,
      status: normalized.status,
      sourceKind: normalized.sourceKind,
      sourceCursor: normalized.sourceCursor,
      frameVersion: normalized.frameVersion,
      updatedAtMs: normalized.updatedAtMs,
      turnId: normalized.turnId,
      runId: normalized.runId,
      data: normalized.data,
    });
  }

  return frames;
}

export const DEFAULT_CONSOLE_FETCH_TIMEOUT_MS = 60_000;
const ERROR_BODY_PREVIEW_LIMIT = 500;

function formatTimeoutReason(timeoutMs: number): string {
  if (timeoutMs % 1000 === 0) {
    return `${timeoutMs / 1000} s`;
  }
  return `${timeoutMs} ms`;
}

async function fetchWithConsoleTimeout(
  input: RequestInfo | URL,
  init: RequestInit,
  label: string,
  timeoutMs?: number,
  signal?: AbortSignal,
): Promise<Response>;
/// With `consume`, the timeout also covers reading the response: the timer
/// stays armed until `consume` settles, so headers that arrive in time cannot
/// hold a stalled body open past it. `consume`'s own failures (an HTTP status,
/// an unusable body) keep their errors; only the timer becomes a timeout and a
/// caller cancellation keeps its abort error.
async function fetchWithConsoleTimeout<T>(
  input: RequestInfo | URL,
  init: RequestInit,
  label: string,
  timeoutMs: number | undefined,
  signal: AbortSignal | undefined,
  consume: (response: Response) => Promise<T>,
): Promise<T>;
async function fetchWithConsoleTimeout<T>(
  input: RequestInfo | URL,
  init: RequestInit,
  label: string,
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
  signal?: AbortSignal,
  consume?: (response: Response) => Promise<T>,
): Promise<Response | T> {
  const controller = new AbortController();
  const timeoutReason = `${label} timeout after ${formatTimeoutReason(timeoutMs)}`;
  const timer = globalThis.setTimeout(() => controller.abort(timeoutReason), timeoutMs);
  const fetchSignal = signal ? AbortSignal.any([controller.signal, signal]) : controller.signal;
  const timedOut = () => {
    if (!controller.signal.aborted || typeof controller.signal.reason !== "string") return undefined;
    const failure = new Error(controller.signal.reason) as ConsoleTransportFailure;
    failure.transportFailure = "timeout";
    failure.timeoutMs = timeoutMs;
    return failure;
  };
  try {
    let response: Response;
    try {
      response = await fetch(input, {
        ...init,
        signal: fetchSignal,
      });
    } catch (error) {
      const timeout = timedOut();
      if (timeout) throw timeout;
      // A caller cancellation keeps its own abort error. Anything else is the
      // fetch itself failing (DNS, refused, reset, offline, CORS): no HTTP
      // response exists, so the gateway could not be reached or did not answer.
      if (!signal?.aborted && error && typeof error === "object") {
        (error as ConsoleTransportFailure).transportFailure = "connection_failed";
      }
      throw error;
    }
    if (!consume) return response;
    try {
      return await consume(response);
    } catch (error) {
      throw timedOut() ?? error;
    }
  } finally {
    globalThis.clearTimeout(timer);
  }
}

/** Typed failure of the console fetch layer. Set only where the failure is
 * observed (timer, fetch rejection, non-JSON body, HTTP status), never parsed
 * back out of a message. */
/** `connection_failed`: the fetch rejected with no HTTP response. The
 * browser cannot tell a gateway it never reached from a response lost after
 * the gateway handled the request, so this never implies "not sent". */
export type ConsoleTransportFailureKind = "timeout" | "connection_failed" | "invalid_response";
export interface ConsoleTransportFailure extends Error {
  httpStatus?: number;
  transportFailure?: ConsoleTransportFailureKind;
  timeoutMs?: number;
  /** JSON-RPC error carried by a non-2xx response body (e.g. the typed 401
   * `data.kind: "unauthenticated"`). Kept apart from `rpcError`, which marks
   * an answered 200 JSON-RPC error. */
  responseRpcError?: { code?: unknown; message?: unknown; data?: unknown };
}

function jsonRpcErrorFromText(text: string): ConsoleTransportFailure["responseRpcError"] | undefined {
  try {
    const parsed = JSON.parse(text) as unknown;
    const error = parsed && typeof parsed === "object" ? (parsed as Record<string, unknown>).error : undefined;
    return error && typeof error === "object" ? error as ConsoleTransportFailure["responseRpcError"] : undefined;
  } catch {
    return undefined;
  }
}

async function consoleResponseFailure(label: string, response: Response): Promise<ConsoleTransportFailure> {
  const text = await response.text().catch(() => "");
  const preview = responseTextErrorPreview(text);
  const failure = new Error(`${label} ${response.status}${preview ? `: ${preview}` : ""}`) as ConsoleTransportFailure;
  failure.httpStatus = response.status;
  const responseRpcError = jsonRpcErrorFromText(text);
  if (responseRpcError) failure.responseRpcError = responseRpcError;
  return failure;
}

async function consoleResponseJson(label: string, response: Response): Promise<any> {
  try {
    return await response.json();
  } catch (error) {
    const failure = new Error(
      `${label} returned a non-JSON response (HTTP ${response.status}${response.headers.get("content-type") ? `, ${response.headers.get("content-type")}` : ""})`,
      { cause: error },
    ) as ConsoleTransportFailure;
    failure.httpStatus = response.status;
    failure.transportFailure = "invalid_response";
    throw failure;
  }
}

async function responseErrorPreview(response: Response): Promise<string> {
  const text = await response.text();
  return responseTextErrorPreview(text);
}

function responseTextErrorPreview(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) {
    return "";
  }
  try {
    const parsed = JSON.parse(trimmed) as unknown;
    if (parsed && typeof parsed === "object") {
      const record = parsed as Record<string, unknown>;
      const message = typeof record.message === "string" ? record.message : undefined;
      const error = record.error && typeof record.error === "object"
        ? record.error as Record<string, unknown>
        : null;
      const errorMessage = error && typeof error.message === "string" ? error.message : undefined;
      const errorCode = error && (typeof error.code === "string" || typeof error.code === "number")
        ? String(error.code)
        : undefined;
      const selected = [
        errorCode ? `code=${errorCode}` : "",
        errorMessage || message || "",
      ].filter(Boolean).join(" ");
      if (selected) {
        return selected;
      }
    }
  } catch {
    // Fall through to a bounded text preview.
  }
  return trimmed.length > ERROR_BODY_PREVIEW_LIMIT
    ? `${trimmed.slice(0, ERROR_BODY_PREVIEW_LIMIT)}...`
    : trimmed;
}

export async function fetchJson<T>(
  baseUrl: string,
  path: string,
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
): Promise<T> {
  const response = await fetchWithConsoleTimeout(
    `${baseUrl}${path}`,
    {},
    "console fetch",
    timeoutMs,
  );
  if (!response.ok) {
    const preview = await responseErrorPreview(response);
    throw new Error(`Request failed ${response.status} for ${path}${preview ? `: ${preview}` : ""}`);
  }
  return response.json() as Promise<T>;
}

async function rpc<T>(
  baseUrl: string,
  method: string,
  params: Record<string, unknown>,
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<T> {
  const response = await fetchWithConsoleTimeout(
    `${baseUrl}${CONSOLE_RPC_PATHS.jsonRpc}`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: `${method}:${Date.now()}`,
        method,
        params,
      }),
    },
    "console rpc",
    timeoutMs,
    signal,
  );

  if (!response.ok) {
    throw await consoleResponseFailure(`${method} request failed`, response);
  }

  const result = await consoleResponseJson(method, response);
  if (result.error) {
    const typedError = normalizeConsoleInteractionRejectedError(result.error) as ConsoleGatewayInteractionRejectedError | null;
    if (typedError) {
      const error = new Error(`${method} RPC error ${typedError.code}: ${typedError.message}`);
      (error as Error & { rpcError?: ConsoleGatewayInteractionRejectedError & { data?: unknown } }).rpcError =
        result.error.data === undefined ? typedError : { ...typedError, data: result.error.data };
      throw error;
    }
    const replayError = normalizeReplayUnavailableError(result.error.data) as ConsoleReplayUnavailablePayload | null;
    if (replayError || result.error.code === CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE) {
      const error = new Error(
        `${method} RPC replay unavailable: ${result.error.message || JSON.stringify(result.error)}`,
      );
      const annotated = error as Error & {
        replayError?: ConsoleReplayUnavailablePayload;
        timelineReplayUnavailable?: boolean;
      };
      if (replayError) {
        annotated.replayError = replayError;
      }
      annotated.timelineReplayUnavailable = true;
      throw error;
    }
    const error = new Error(`${method} RPC error: ${result.error.message || JSON.stringify(result.error)}`);
    (error as Error & { rpcError?: { code?: unknown; message?: unknown; data?: unknown } }).rpcError = result.error;
    if (result.error.code === -32030 || result.error.data?.kind === "access_denied") {
      (error as Error & { httpStatus?: number }).httpStatus = 403;
    }
    throw error;
  }

  return result.result as T;
}

export async function sendConsoleMultipart(
  baseUrl: string,
  identity: string,
  contentInput: string | Array<Record<string, unknown>>,
  attachments: File[],
  origin: string,
  idempotencyKey: string,
  handlingMode: "queue" | "steer" = "queue",
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
): Promise<ConsoleTimelineAccepted> {
  const content: Array<Record<string, unknown>> = typeof contentInput === "string"
    ? (contentInput.trim() ? [{ type: "text", text: contentInput }] : [])
    : [...contentInput];
  const form = new FormData();
  attachments.forEach((file, index) => {
    const uploadId = `upload-${Date.now().toString(36)}-${index}`;
    content.push({
      type: "image_upload",
      upload_id: uploadId,
      media_type: file.type || "application/octet-stream",
      alt: file.name,
    });
    form.append(`file:${uploadId}`, file, file.name);
  });
  form.append("payload", JSON.stringify({
    jsonrpc: "2.0",
    id: `${CONSOLE_RPC_METHODS.send}:${Date.now()}`,
    method: CONSOLE_RPC_METHODS.send,
    params: {
      identity,
      content,
      origin,
      origin_kind: "operator",
      idempotency_key: idempotencyKey,
      handling_mode: handlingMode,
    },
  }));

  const response = await fetchWithConsoleTimeout(
    `${baseUrl}${CONSOLE_RPC_PATHS.multipartJsonRpc}`,
    {
      method: "POST",
      body: form,
    },
    "console multipart",
    timeoutMs,
  );
  if (!response.ok) {
    throw await consoleResponseFailure(`${CONSOLE_RPC_METHODS.send} multipart failed`, response);
  }
  const result = await consoleResponseJson(`${CONSOLE_RPC_METHODS.send} multipart`, response);
  if (result.error) {
    // Same typed annotation as the JSON door, so a multipart send's refusal
    // is classified exactly like a text send's.
    const error = new Error(`${CONSOLE_RPC_METHODS.send} RPC error: ${result.error.message || JSON.stringify(result.error)}`);
    (error as Error & { rpcError?: { code?: unknown; message?: unknown; data?: unknown } }).rpcError = result.error;
    if (result.error.code === -32030 || result.error.data?.kind === "access_denied") {
      (error as Error & { httpStatus?: number }).httpStatus = 403;
    }
    throw error;
  }
  return normalizeConsoleTimelineAccepted(result.result);
}

export async function uploadConsoleBlobMultipart(
  baseUrl: string,
  input: { blobId?: string; file?: File; mediaType?: string },
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
): Promise<{ blob_id: string; url?: string }> {
  const file = input.file;
  if (!file) {
    throw new Error(`${CONSOLE_RPC_METHODS.blobUpload} requires a file`);
  }
  const mediaType = input.mediaType || file.type || "application/octet-stream";
  const uploadId = input.blobId?.trim() || `upload-${Date.now().toString(36)}-0`;
  const uploadFile = file.type === mediaType
    ? file
    : new File([file], file.name || "upload", { type: mediaType });
  const form = new FormData();
  form.append(`file:${uploadId}`, uploadFile, uploadFile.name || file.name || "upload");
  form.append("payload", JSON.stringify({
    jsonrpc: "2.0",
    id: `${CONSOLE_RPC_METHODS.blobUpload}:${Date.now()}`,
    method: CONSOLE_RPC_METHODS.blobUpload,
    params: {
      upload: {
        type: "image_upload",
        upload_id: uploadId,
        media_type: mediaType,
        alt: file.name || "upload",
      },
    },
  }));

  const response = await fetchWithConsoleTimeout(
    `${baseUrl}${CONSOLE_RPC_PATHS.multipartJsonRpc}`,
    {
      method: "POST",
      body: form,
    },
    "console multipart",
    timeoutMs,
  );
  if (!response.ok) {
    const preview = await responseErrorPreview(response);
    throw new Error(`${CONSOLE_RPC_METHODS.blobUpload} multipart failed ${response.status}${preview ? `: ${preview}` : ""}`);
  }
  const result = await response.json();
  if (result.error) {
    throw new Error(`${CONSOLE_RPC_METHODS.blobUpload} RPC error: ${result.error.message || JSON.stringify(result.error)}`);
  }
  const record = result.result && typeof result.result === "object"
    ? result.result as Record<string, unknown>
    : {};
  const blobId = typeof record.blob_id === "string" ? record.blob_id : "";
  if (!blobId) {
    throw new Error(`${CONSOLE_RPC_METHODS.blobUpload} returned an invalid blob payload`);
  }
  return {
    blob_id: blobId,
    url: typeof record.url === "string" ? record.url : undefined,
  };
}

/// Reads a stored blob for display: a same-origin GET carrying the browser's
/// own credentials, like every request of this transport, so a session cookie
/// or an authenticating front for `/blobs/{id}` applies to it. No token is put
/// in the URL. Only displayable image bytes are returned. The timeout covers
/// reading the body as well as the headers.
export async function readConsoleBlob(
  baseUrl: string,
  blobId: string,
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<Blob> {
  return fetchWithConsoleTimeout(
    `${baseUrl}${CONSOLE_BLOB_PATH_PREFIX}${encodeURIComponent(blobId)}`,
    { method: "GET", credentials: "same-origin" },
    "console blob read",
    timeoutMs,
    signal,
    async (response) => {
      if (!response.ok) {
        throw new Error(`Blob read failed ${response.status} for ${blobId}`);
      }
      const mediaType = displayableImageMediaType(response.headers.get("content-type"));
      if (!mediaType) {
        throw new Error(`Blob ${blobId} is not a displayable image`);
      }
      return new Blob([await response.arrayBuffer()], { type: mediaType });
    },
  );
}

const TERMINAL_SSE_EVENTS = new Set([
  "interaction_complete",
  "run_completed",
  "interaction_failed",
  "run_failed",
  "turn_completed",
]);

interface TerminalCorrelation {
  sessionId?: string;
  interactionId?: string;
}

interface StreamFramesOptions {
  correlation?: TerminalCorrelation;
  onFrame?: (frame: ConsoleFrame) => void;
  stopOnTerminal?: boolean;
  signal?: AbortSignal;
}

function matchesCorrelation(
  candidate: unknown,
  correlation?: TerminalCorrelation,
  allowUnscoped = true,
): boolean {
  if (!correlation?.sessionId && !correlation?.interactionId) {
    return true;
  }
  if (candidate === null || typeof candidate !== "object") {
    return allowUnscoped;
  }
  const record = candidate as Record<string, unknown>;
  const sessionId = record.session_id ?? record.sessionId;
  const interactionId = record.interaction_id ?? record.interactionId;
  const hasScopedField = sessionId !== undefined || interactionId !== undefined;
  if (!hasScopedField) {
    return allowUnscoped;
  }
  if (correlation.sessionId && sessionId === correlation.sessionId) {
    return true;
  }
  if (correlation.interactionId && interactionId === correlation.interactionId) {
    return true;
  }
  return false;
}

function isTerminalTurnCompletedData(data: unknown): boolean {
  const record = data && typeof data === "object" ? data as Record<string, unknown> : {};
  const stopReason = record.stop_reason ?? record.stopReason;
  return typeof stopReason === "string" ? stopReason !== "tool_use" : true;
}

function isTerminalSseFrame(frame: ConsoleFrame): boolean {
  if (!TERMINAL_SSE_EVENTS.has(frame.event || "")) return false;
  if (frame.event !== "turn_completed") return true;
  return isTerminalTurnCompletedData(frame.data);
}

function replayStreamError(frame: ConsoleFrame): ConsoleStreamFailure {
  const error = new Error("Timeline replay is unavailable") as ConsoleStreamFailure;
  error.replayFrame = frame;
  return error;
}

async function streamFramesFromResponse(
  response: Response,
  options: StreamFramesOptions = {},
  mode: "consume" | "collect" = "collect",
): Promise<ConsoleFrame[] | void> {
  if (!response.ok) {
    const text = await response.text();
    let parsed: unknown;
    try { parsed = JSON.parse(text); } catch { parsed = undefined; }
    const replayError = normalizeReplayUnavailableError(parsed);
    const ownerFault = parsed && typeof parsed === "object" ? parsed as Record<string, unknown> : undefined;
    // The typed owner response permits an absent latest cursor (empty/reset
    // log). A repair still needs to run; no speculative frontier is invented.
    if (response.status === 409 && (replayError || ownerFault?.error === "replay_unavailable")) {
      throw replayStreamError({ id: "", event: "replay_unavailable", data: replayError || parsed });
    }
    const preview = responseTextErrorPreview(text);
    const error = new Error(`interaction stream request failed ${response.status}${preview ? `: ${preview}` : ""}`) as ConsoleStreamFailure;
    error.httpStatus = response.status;
    throw error;
  }
  const readerOptions = {
    parseBlock: parseSseFrames,
    signal: options.signal,
    accept(frame: ConsoleFrame) {
      // This is a server control frame, including cursorless source resets.
      if (frame.event === "replay_unavailable") throw replayStreamError(frame);
      return matchesCorrelation(frame, options.correlation, true);
    },
    onFrame: options.onFrame,
    terminal: (options.stopOnTerminal ?? Boolean(options.correlation)) ? isTerminalSseFrame : undefined,
  };
  return mode === "consume"
    ? consumeSseResponse(response, { ...readerOptions, mode: "consume" })
    : consumeSseResponse(response, { ...readerOptions, mode: "collect" });
}

export async function queryTimeline(
  baseUrl: string,
  target: {
    identity?: string;
    conversationId?: string;
    after?: string;
    before?: string;
    mode?: "since" | "recent";
  },
  limit = 400,
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<ConsoleTimelinePage> {
  const result = await rpc<unknown>(baseUrl, CONSOLE_RPC_METHODS.queryTimeline, {
    limit,
    ...(target.identity?.trim() ? { identity: target.identity.trim() } : {}),
    ...(target.conversationId?.trim() ? { conversation_id: target.conversationId.trim() } : {}),
    ...(target.after?.trim() ? { after: target.after.trim() } : {}),
    ...(target.before?.trim() ? { before: target.before.trim() } : {}),
    ...(target.mode ? { mode: target.mode } : {}),
  }, timeoutMs, signal);
  if (!result || typeof result !== "object") {
    return { frames: [], available: false };
  }
  const record = result as Record<string, unknown>;
  const rawFrames = Array.isArray(record.frames) ? record.frames : [];
  return {
    frames: rawFrames.map(timelineFrameToConsoleFrame),
    nextCursor: typeof record.next_cursor === "string" ? record.next_cursor : undefined,
    latestCursor: typeof record.latest_cursor === "string" ? record.latest_cursor : undefined,
    exhausted: record.exhausted === true,
    available: record.available !== false,
  };
}

export async function sendConsole(
  baseUrl: string,
  identity: string,
  content: string | Array<Record<string, unknown>>,
  origin: string,
  idempotencyKey: string,
  handlingMode: "queue" | "steer" = "queue",
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
): Promise<ConsoleTimelineAccepted> {
  const accepted = await rpc<unknown>(baseUrl, CONSOLE_RPC_METHODS.send, {
    identity,
    content,
    origin,
    origin_kind: "operator",
    idempotency_key: idempotencyKey,
    handling_mode: handlingMode,
  }, timeoutMs);
  if (!accepted || typeof accepted !== "object") {
    throw new Error(`${CONSOLE_RPC_METHODS.send} returned an invalid acceptance payload`);
  }
  const record = accepted as Record<string, unknown>;
  return normalizeConsoleTimelineAccepted(record);
}

function normalizeConsoleTimelineAccepted(
  accepted: unknown,
): ConsoleTimelineAccepted {
  const record = accepted && typeof accepted === "object" ? accepted as Record<string, unknown> : {};
  // Only explicit owner receipts can settle a send. A malformed success is
  // still an unknown outcome; never synthesize destination or interaction IDs.
  // The owner resolves aliases and returns its canonical identity.
  if (typeof record.interaction_id !== "string" || !record.interaction_id.trim()
    || typeof record.identity !== "string" || !record.identity.trim()
    || ("input_frame_id" in record && record.input_frame_id != null
      && (typeof record.input_frame_id !== "string" || !record.input_frame_id.trim()))) {
    throw new Error(`${CONSOLE_RPC_METHODS.send} returned an invalid acceptance payload`);
  }
  return {
    interaction_id: record.interaction_id,
    identity: record.identity,
    conversation_id: typeof record.conversation_id === "string" ? record.conversation_id : undefined,
    session_id: typeof record.session_id === "string" ? record.session_id : undefined,
    input_frame_id: typeof record.input_frame_id === "string" ? record.input_frame_id : undefined,
    cursor: typeof record.cursor === "string" ? record.cursor : undefined,
    status: typeof record.status === "string" ? record.status : undefined,
  };
}

export async function callConsoleRpc<T>(
  baseUrl: string,
  method: string,
  params: Record<string, unknown> = {},
  timeoutMs = DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<T> {
  return rpc<T>(baseUrl, method, params, timeoutMs, signal);
}

function timelineStreamPath(target: { identity?: string; conversationId?: string }): string {
  const params = new URLSearchParams();
  if (target.identity?.trim()) params.set("identity", target.identity.trim());
  if (target.conversationId?.trim()) params.set("conversation_id", target.conversationId.trim());
  return `${CONSOLE_REST_PATHS.timelineStream}${params.size > 0 ? `?${params.toString()}` : ""}`;
}

export function subscribeTimelineEvents(
  baseUrl: string,
  target: { identity?: string; conversationId?: string; after?: string },
  onFrame: (frame: ConsoleFrame) => void,
  options: ConsoleTimelineSubscriptionOptions = {},
): ConsoleTimelineSubscription {
  const fetchImpl = globalThis.fetch;
  return createTimelineSubscription({
    after: target.after,
    options,
    onFrame,
    async open(signal, after, connected, deliver) {
      const headers: Record<string, string> = { "content-type": "application/json" };
      if (after) headers["Last-Event-ID"] = after;
      const response = await fetchImpl(`${baseUrl}${timelineStreamPath(target)}`, {
        method: "GET", headers, signal,
      });
      if (response.ok) connected();
      await streamFramesFromResponse(response, {
        signal, onFrame: deliver, stopOnTerminal: false,
      }, "consume");
    },
  });
}
