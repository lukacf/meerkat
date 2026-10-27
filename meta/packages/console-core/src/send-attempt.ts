import { serializeConsoleContextMessage, validateConsoleContexts, type ConsoleContextRecord } from "./context-record";

export type ConsoleSendAttemptState = "draft" | "attempting" | "accepted" | "definitely-rejected" | "outcome-unknown";
export interface ConsoleFrozenSendEnvelope {
  identity: string;
  content: string | Array<{ type: "text"; text: string }>;
  origin: string;
  origin_kind: "operator";
  idempotency_key: string;
  handling_mode: "queue" | "steer";
}
export interface ConsoleSendAttempt {
  version: 1;
  id: string;
  scope: string;
  destination: string;
  origin: string;
  idempotencyKey: string;
  text: string;
  contexts: ConsoleContextRecord[];
  addedAt: number;
  state: ConsoleSendAttemptState;
  /** Persisted serialized envelope is immutable after the first attempt. */
  envelopeJson?: string;
  lease?: { owner: string; expiresAt: number };
  error?: string;
  /** Typed reason for a settled failure; `error` is its rendered message. */
  failureKind?: ConsoleSendFailureKind;
  accepted?: { interactionId: string; inputFrameId?: string };
}

/** Why a send attempt did not produce an acceptance receipt. Classified from
 * typed transport facts (HTTP status, JSON-RPC code or `data.kind`, fetch
 * layer failure kind), never from message text. */
export type ConsoleSendFailureKind =
  | "unauthenticated"
  | "access_denied"
  | "read_only"
  | "rejected"
  | "refused"
  | "rate_limited"
  | "interrupted"
  | "capability_unavailable"
  | "connection_failed"
  | "timeout"
  | "invalid_response"
  | "gateway_error"
  | "unknown";
export interface ConsoleSendFailure {
  state: "definitely-rejected" | "outcome-unknown";
  kind: ConsoleSendFailureKind;
  /** Operator-facing sentence naming the reason and what to do next. */
  message: string;
}
const nonemptyString = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0;

export function createConsoleSendAttempt(input: {
  id: string; scope: string; destination: string; origin: string; idempotencyKey: string;
  text: string; contexts?: ConsoleContextRecord[]; now: number;
}): ConsoleSendAttempt {
  if (!input.text.trim()) throw new Error("Write a message before sending.");
  const attempt: ConsoleSendAttempt = {
    version: 1, id: input.id, scope: input.scope, destination: input.destination,
    origin: input.origin, idempotencyKey: input.idempotencyKey, text: input.text,
    contexts: structuredClone(input.contexts ?? []), addedAt: input.now, state: "draft",
  };
  validateConsoleSendAttempt(attempt);
  return attempt;
}

export function validateConsoleSendAttempt(value: ConsoleSendAttempt): void {
  if (!value || value.version !== 1 || !nonemptyString(value.id) || !nonemptyString(value.scope) || !nonemptyString(value.destination) || !nonemptyString(value.origin) ||
    !nonemptyString(value.idempotencyKey) || typeof value.text !== "string" || !value.text.trim() ||
    !Number.isFinite(value.addedAt) || !Array.isArray(value.contexts) ||
    !["draft", "attempting", "accepted", "definitely-rejected", "outcome-unknown"].includes(value.state)) {
    throw new Error("The saved send attempt is invalid or uses an unsupported version.");
  }
  if ((value.error !== undefined && typeof value.error !== "string") ||
    // An unrecognized kind (a newer tab's vocabulary) renders with the
    // state's generic label rather than blocking the whole saved queue.
    (value.failureKind !== undefined && (!nonemptyString(value.failureKind) ||
      (value.state !== "definitely-rejected" && value.state !== "outcome-unknown"))) ||
    (value.lease !== undefined && (!value.lease || !nonemptyString(value.lease.owner) || !Number.isFinite(value.lease.expiresAt))) ||
    (value.state === "attempting" && !value.lease) || (value.state !== "attempting" && value.lease !== undefined) ||
    (value.state === "accepted" && (!value.accepted || !nonemptyString(value.accepted.interactionId) ||
      (value.accepted.inputFrameId !== undefined && !nonemptyString(value.accepted.inputFrameId)))) ||
    (value.state !== "accepted" && value.accepted !== undefined)) {
    throw new Error("The saved send lease or acceptance receipt is invalid.");
  }
  validateConsoleContexts(value.contexts);
  if (value.envelopeJson !== undefined && !nonemptyString(value.envelopeJson)) throw new Error("The saved send envelope is invalid.");
  if (value.state !== "draft" && !nonemptyString(value.envelopeJson)) throw new Error("A send attempt is missing its frozen envelope.");
  if (value.state === "draft" && value.envelopeJson) throw new Error("An attempted envelope cannot be changed back into a draft.");
  if (value.envelopeJson) {
    const envelope = JSON.parse(value.envelopeJson) as ConsoleFrozenSendEnvelope;
    const content = value.contexts.length ? serializeConsoleContextMessage(value.text, value.contexts) : value.text;
    if (envelope.identity !== value.destination || envelope.origin !== value.origin ||
      envelope.idempotency_key !== value.idempotencyKey || envelope.origin_kind !== "operator" ||
      !["queue", "steer"].includes(envelope.handling_mode) || JSON.stringify(envelope.content) !== JSON.stringify(content)) {
      throw new Error("The saved send envelope does not match its original intent.");
    }
  }
}

export function beginConsoleSendAttempt(
  attempt: ConsoleSendAttempt,
  options: {
    owner: string; now: number; handlingMode: "queue" | "steer"; retryRejected?: boolean;
    /** Explicit "Send again" of a settled attempt, including an uncertain
     * one. The frozen envelope (and so its idempotency key) is reused, so the
     * server replays an acceptance it already recorded or admits the message
     * once; it can never create a second message. */
    resend?: boolean;
  },
): ConsoleSendAttempt {
  validateConsoleSendAttempt(attempt);
  if (!nonemptyString(options.owner) || !Number.isFinite(options.now)) throw new Error("The browser send lease is invalid.");
  const resendable = (attempt.state === "definitely-rejected" && (options.retryRejected || options.resend))
    || (attempt.state === "outcome-unknown" && options.resend);
  if (attempt.state !== "draft" && !resendable) {
    throw new Error("This send may already have been accepted. Reconcile it before taking another action.");
  }
  const envelope: ConsoleFrozenSendEnvelope = {
    identity: attempt.destination,
    content: attempt.contexts.length ? serializeConsoleContextMessage(attempt.text, attempt.contexts) : attempt.text,
    origin: attempt.origin, origin_kind: "operator", idempotency_key: attempt.idempotencyKey,
    handling_mode: options.handlingMode,
  };
  if (attempt.envelopeJson && JSON.parse(attempt.envelopeJson).handling_mode !== options.handlingMode) {
    throw new Error("An attempted message cannot change handling mode.");
  }
  return { ...attempt, state: "attempting", envelopeJson: attempt.envelopeJson ?? JSON.stringify(envelope),
    lease: { owner: options.owner, expiresAt: options.now + 15_000 }, error: undefined, failureKind: undefined };
}

/** Expiring a browser lease never proves that the server did not accept a send. */
export function recoverConsoleSendAttempt(attempt: ConsoleSendAttempt, now: number): ConsoleSendAttempt {
  validateConsoleSendAttempt(attempt);
  if (attempt.state === "attempting" && (!attempt.lease || attempt.lease.expiresAt <= now)) {
    return { ...attempt, state: "outcome-unknown", lease: undefined,
      error: "The page closed before the answer arrived, so delivery is unconfirmed." };
  }
  return attempt;
}

export function finishConsoleSendAttempt(attempt: ConsoleSendAttempt, result:
  | { state: "accepted"; interactionId: string; inputFrameId?: string }
  | { state: "definitely-rejected" | "outcome-unknown"; error: string; kind?: ConsoleSendFailureKind },
): ConsoleSendAttempt {
  validateConsoleSendAttempt(attempt);
  if (attempt.state === "accepted") return attempt;
  if (!attempt.envelopeJson) throw new Error("A draft has not been dispatched.");
  if (result.state === "accepted") {
    if (!nonemptyString(result.interactionId) || (result.inputFrameId !== undefined && !nonemptyString(result.inputFrameId))) throw new Error("Server response did not prove acceptance.");
    return { ...attempt, state: "accepted", lease: undefined, error: undefined, failureKind: undefined,
      accepted: { interactionId: result.interactionId, inputFrameId: result.inputFrameId } };
  }
  return { ...attempt, state: result.state, lease: undefined, error: result.error, failureKind: result.kind };
}

type TypedRpcError = { code?: unknown; message?: unknown; data?: { kind?: unknown } | null };
interface TypedSendError {
  rpcError?: TypedRpcError;
  responseRpcError?: TypedRpcError;
  httpStatus?: unknown;
  transportFailure?: unknown;
  timeoutMs?: unknown;
  message?: unknown;
  name?: unknown;
}

/** HTTP statuses the gateway (or a fronting proxy) answers before any
 * reservation: the request was refused as a request, so nothing was sent.
 * 408/409/429/5xx and everything unlisted stay unknown (the console REST
 * send answers 429 after its reservation, when the member's admission
 * backlog is full). */
const PRE_INGRESS_REFUSAL_STATUSES = new Set([400, 404, 405, 413, 414, 415, 422, 431]);

/**
 * Classify a failed send into a typed state, kind and operator message.
 *
 * Only a refusal that proves the request never reached dispatch is
 * `definitely-rejected`: a 401 or typed `unauthenticated` refusal, a 403 or
 * typed `access_denied`, a typed read-only refusal, invalid params (-32602),
 * or a listed pre-ingress HTTP status. Network loss, timeouts, 5xx,
 * non-JSON answers and conflicts are `outcome-unknown`: the server may have
 * reserved the message, so the attempt keeps its envelope for reconciliation.
 */
export function classifyConsoleSendFailure(error: unknown): ConsoleSendFailure {
  const typed = (error && typeof error === "object" ? error : {}) as TypedSendError;
  const rpc = typed.rpcError ?? typed.responseRpcError;
  const rpcKind = typeof rpc?.data?.kind === "string" ? rpc.data.kind : undefined;
  const rpcMessage = typeof rpc?.message === "string" && rpc.message.trim() ? rpc.message.trim() : undefined;
  const status = typeof typed.httpStatus === "number" ? typed.httpStatus : undefined;
  const detail = error instanceof Error && error.message.trim() ? error.message.trim()
    : typeof typed.message === "string" && typed.message.trim() ? typed.message.trim() : undefined;
  if (rpcKind === "unauthenticated" || status === 401) {
    return { state: "definitely-rejected", kind: "unauthenticated",
      message: "Sign-in required (401). Nothing was sent. Sign in again or check this connection's authentication, then review the saved message before resending." };
  }
  if (rpcKind === "access_denied" || rpc?.code === -32030 || status === 403) {
    return { state: "definitely-rejected", kind: "access_denied",
      message: `Not allowed to send to this agent (403)${rpcMessage ? `: ${rpcMessage}` : ""}. Nothing was sent.` };
  }
  if (rpcKind === "read_only") {
    return { state: "definitely-rejected", kind: "read_only",
      message: "The console is read-only. Nothing was sent." };
  }
  if (rpc?.code === -32602) {
    return { state: "definitely-rejected", kind: "rejected",
      message: `Send rejected: ${rpcMessage ?? detail ?? "invalid request"}. Nothing was sent.` };
  }
  // The console's own lifetime ended (a reload, remount or account switch)
  // while the request was in flight: the answer was discarded, so the send
  // may well have been accepted. Typed by the abort's DOMException name.
  if (typed.name === "AbortError" && typed.transportFailure === undefined) {
    return { state: "outcome-unknown", kind: "interrupted",
      message: "The console was reloaded or switched while this send was in flight, so its answer was not received. It may have been accepted: check acceptance before retrying." };
  }
  if (typed.transportFailure === "timeout") {
    const seconds = typeof typed.timeoutMs === "number" ? ` within ${Math.round(typed.timeoutMs / 1000)} s` : "";
    return { state: "outcome-unknown", kind: "timeout",
      message: `No response from the gateway${seconds}. It may still have accepted the message: check acceptance before retrying.` };
  }
  // No HTTP response at all. The browser cannot distinguish a gateway it
  // never reached from an acknowledgement lost after the gateway accepted the
  // message, so this is "acceptance unknown", never "not sent".
  if (typed.transportFailure === "connection_failed") {
    return { state: "outcome-unknown", kind: "connection_failed",
      message: `The connection failed before the gateway answered${detail ? ` (${detail})` : ""}. The message may already have been accepted: check acceptance before retrying.` };
  }
  if (typed.transportFailure === "invalid_response") {
    return { state: "outcome-unknown", kind: "invalid_response",
      message: `${detail ?? "The gateway returned an unreadable response"}. Something between the console and the gateway answered instead of it: check acceptance before retrying.` };
  }
  if (status !== undefined && PRE_INGRESS_REFUSAL_STATUSES.has(status)) {
    return { state: "definitely-rejected", kind: "rejected",
      message: `Send rejected by the gateway (HTTP ${status})${rpcMessage ? `: ${rpcMessage}` : ""}. Nothing was sent.` };
  }
  // The gateway answered the send with a typed JSON-RPC error that does not
  // prove pre-reservation refusal (e.g. a steer that was reserved and then
  // refused by the member, or an idempotency conflict): name the reason, but
  // keep the attempt reconcilable.
  if (typed.rpcError && (rpcMessage || detail)) {
    return { state: "outcome-unknown", kind: "refused",
      message: `Send failed: ${rpcMessage ?? detail}. The gateway may have recorded this attempt: check acceptance before retrying.` };
  }
  if (status === 429) {
    return { state: "outcome-unknown", kind: "rate_limited",
      message: "The agent is not taking more input right now (HTTP 429). It may already have recorded this attempt: check acceptance before retrying." };
  }
  if (status !== undefined && status >= 500) {
    return { state: "outcome-unknown", kind: "gateway_error",
      message: `Gateway error (HTTP ${status}). The console could not confirm acceptance: check acceptance before retrying.` };
  }
  return { state: "outcome-unknown", kind: "unknown",
    message: `The console could not confirm acceptance${detail ? `: ${detail}` : ""}. Check acceptance before retrying.` };
}

/** Only structured pre-ingress rejection is proof. Network errors and conflicts are unknown. */
export function consoleSendFailureState(error: unknown): "definitely-rejected" | "outcome-unknown" {
  return classifyConsoleSendFailure(error).state;
}

/** Short row label for a settled attempt, from its typed failure kind. */
export function consoleSendFailureLabel(attempt: Pick<ConsoleSendAttempt, "state" | "failureKind">): string {
  switch (attempt.failureKind) {
    case "unauthenticated": return "Not authorized";
    case "access_denied": return "Not allowed";
    case "read_only": return "Console read-only";
    case "capability_unavailable": return "Send unavailable";
    case "connection_failed": return "Acceptance unknown";
    case "timeout": return "No response";
    case "invalid_response": return "Unreadable response";
    case "gateway_error": return "Gateway error";
    case "rejected": return "Rejected";
    case "refused": return "Send failed";
    case "rate_limited": return "Gateway busy";
    case "interrupted": return "Interrupted";
    default: return attempt.state === "definitely-rejected" ? "Not accepted" : "Acceptance unknown";
  }
}

function sameFrozenContent(actual: unknown, expected: ConsoleFrozenSendEnvelope["content"]): boolean {
  if (typeof expected === "string") return actual === expected;
  if (!Array.isArray(actual) || actual.length !== expected.length) return false;
  return actual.every((block, index) => block !== null && typeof block === "object" &&
    Object.keys(block).length === 2 && block.type === expected[index].type && block.text === expected[index].text);
}

/** Reconcile only an exact owner user_input receipt from an authorized timeline. */
export function reconcileConsoleSendReceipt(attempt: ConsoleSendAttempt, frame: {
  id: string; event: string; identity?: string; interactionId?: string; data: unknown;
}, resolution?: { requestedIdentity: string; canonicalIdentity: string }): ConsoleSendAttempt | null {
  const destinationResolved = frame.identity === attempt.destination ||
    (resolution?.requestedIdentity === attempt.destination
      && nonemptyString(resolution.canonicalIdentity) && frame.identity === resolution.canonicalIdentity);
  if (!attempt.envelopeJson || frame.event !== "user_input" || !destinationResolved || !frame.interactionId || !frame.id) return null;
  const envelope = JSON.parse(attempt.envelopeJson) as ConsoleFrozenSendEnvelope;
  const payload = frame.data as Partial<ConsoleFrozenSendEnvelope> | null;
  if (!payload || payload.origin !== envelope.origin || payload.origin_kind !== envelope.origin_kind ||
    payload.idempotency_key !== envelope.idempotency_key || payload.handling_mode !== envelope.handling_mode ||
    !sameFrozenContent(payload.content, envelope.content)) return null;
  return finishConsoleSendAttempt(attempt, { state: "accepted", interactionId: frame.interactionId, inputFrameId: frame.id });
}

/** Typed outcome of an explicit "Check acceptance" that did not find a
 * receipt. A failed check never changes the attempt's own state: it proves
 * nothing about the send, and the saved message is never resent by it. */
export interface ConsoleAcceptanceCheckResult {
  kind: ConsoleSendFailureKind | "no_receipt";
  message: string;
}

export const CONSOLE_ACCEPTANCE_NO_RECEIPT: ConsoleAcceptanceCheckResult = {
  kind: "no_receipt",
  message: "No acceptance receipt: this agent's timeline has no record of this message. It remains saved and will not be resent automatically.",
};

export function describeConsoleAcceptanceCheckFailure(error: unknown): ConsoleAcceptanceCheckResult {
  const failure = classifyConsoleSendFailure(error);
  const unchanged = "The saved message is unchanged and was not resent.";
  switch (failure.kind) {
    case "unauthenticated":
      return { kind: failure.kind, message: `Could not check acceptance: authentication required (401). ${unchanged}` };
    case "access_denied":
      return { kind: failure.kind, message: `Could not check acceptance: not allowed to read this agent's timeline (403). ${unchanged}` };
    case "connection_failed":
      return { kind: failure.kind, message: `Could not check acceptance: the connection failed before the gateway answered. ${unchanged}` };
    case "timeout":
      return { kind: failure.kind, message: `Could not check acceptance: no response from the gateway. ${unchanged}` };
    default: {
      const status = (error as { httpStatus?: unknown } | null)?.httpStatus;
      const detail = error instanceof Error && error.message.trim() ? error.message.trim() : "the check failed";
      return { kind: failure.kind, message: `Could not check acceptance${typeof status === "number" ? ` (HTTP ${status})` : ""}: ${detail}. ${unchanged}` };
    }
  }
}

// ---------------------------------------------------------------------------
// Plain-language row copy (MobKit 0.8.45). Rows are worded from the typed
// state and failure kind only, never from a stored error string, so rows
// saved by older consoles (untyped `error`, no `failureKind`) read the same.
// ---------------------------------------------------------------------------

/** Names the row copy uses: the destination agent and the embedding host. */
export interface ConsolePendingRowNames {
  agent: string;
  /** The embedding host's display name (console branding), if it has one. */
  host?: string;
}

export interface ConsolePendingRowCopy {
  /** Short state tag shown beside the age. */
  label: string;
  /** The row's headline sentence. */
  title?: string;
  /** What happened, when the typed failure kind says. */
  detail?: string;
}

const NEUTRAL_HOST = "the server";
function hostOf(names: ConsolePendingRowNames): string {
  return names.host?.trim() || NEUTRAL_HOST;
}
function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** "Couldn't reach <host> (offline or signed out)." */
export function consoleCannotReachHost(names: ConsolePendingRowNames): string {
  return `Couldn't reach ${hostOf(names)} (offline or signed out).`;
}

export function describeConsolePendingRow(
  attempt: Pick<ConsoleSendAttempt, "state" | "failureKind">,
  names: ConsolePendingRowNames,
): ConsolePendingRowCopy {
  const host = hostOf(names);
  if (attempt.state === "draft") return { label: "Queued" };
  if (attempt.state === "attempting") {
    return { label: "Sending", title: "Sending...", detail: `Waiting for ${host} to confirm.` };
  }
  if (attempt.state === "accepted") return { label: "Delivered" };
  if (attempt.state === "definitely-rejected") {
    const detail = (() => {
      switch (attempt.failureKind) {
        case "unauthenticated": return "Sign in again or check this connection's authentication. Then review your saved message before choosing Send again.";
        case "access_denied": return `You don't have permission to message ${names.agent}.`;
        case "read_only": return "This console is read-only.";
        case "capability_unavailable": return "Sending isn't available here.";
        default: return `${capitalize(host)} refused it.`;
      }
    })();
    return { label: "Not sent", title: `Not sent: this message never reached ${names.agent}.`, detail };
  }
  const detail = (() => {
    switch (attempt.failureKind) {
      case "connection_failed": return consoleCannotReachHost(names);
      case "timeout": return `${capitalize(host)} didn't answer in time.`;
      case "invalid_response": return `Got an unreadable answer from ${host}.`;
      case "gateway_error": return `${capitalize(host)} had a problem.`;
      case "rate_limited": return `${names.agent} was busy.`;
      case "refused": return `${capitalize(host)} reported a problem with this message.`;
      case "interrupted": return "The page was reloaded while sending.";
      default: return "It may or may not have arrived.";
    }
  })();
  return { label: "Not confirmed", title: `We couldn't confirm ${names.agent} got this.`, detail };
}

/** A Check that ran and did not find the message: the row stays uncertain. */
export function consoleCheckNotFound(names: ConsolePendingRowNames): string {
  return `Not found in ${names.agent}'s recent messages.`;
}

/** A Check found the message: "Delivered at 14:03." */
export function consoleCheckDelivered(timestampMs: number | undefined): string {
  if (typeof timestampMs !== "number" || !Number.isFinite(timestampMs)) return "Delivered.";
  return `Delivered at ${new Date(timestampMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}.`;
}

/** A Check that could not run, worded from the typed failure. A browser
 * fetch failure (`TypeError`, e.g. "Failed to fetch") means the host was not
 * reached. */
export function describeConsoleCheckFailure(error: unknown, names: ConsolePendingRowNames): string {
  const host = hostOf(names);
  const fetchFailed = error instanceof TypeError
    || (typeof error === "object" && error !== null && (error as { name?: unknown }).name === "TypeError");
  const kind = fetchFailed ? "connection_failed" : classifyConsoleSendFailure(error).kind;
  const reason = (() => {
    switch (kind) {
      case "connection_failed": return `couldn't reach ${host} (offline or signed out).`;
      case "unauthenticated": return "you were signed out. Sign in and check again.";
      case "access_denied": return `you don't have access to ${names.agent}'s messages.`;
      case "timeout": return `${host} didn't answer in time.`;
      case "gateway_error": return `${host} had a problem.`;
      case "invalid_response": return `got an unreadable answer from ${host}.`;
      default: return `${host} couldn't look up ${names.agent}'s messages.`;
    }
  })();
  return `Couldn't check: ${reason}`;
}
