import { EventSubscription } from './events.js';
import { foldUnknownSkillResolutionReason, isKnownEvent } from './types.js';
import type {
  ContentBlock,
  WireTurnInputOptions,
  TurnResult,
  SessionEvent,
  SessionState,
  AppendSystemContextOptions,
  AppendSystemContextResult,
} from './types.js';
import type { WireRunStopReceipt } from './generated/session.js';

// WASM function signatures (bound at construction)
type StartTurnFn = (handle: number, prompt: string, optionsJson?: string) => Promise<string>;
type GetSessionStateFn = (handle: number) => Promise<string>;
type DestroySessionFn = (handle: number) => Promise<void>;
type InterruptSessionFn = (handle: number) => Promise<void>;
type WirePeerFn = (handle: number, peerHandle: number) => Promise<void>;
type StopSessionRunFn = (handle: number, runId: string, reason: string) => Promise<string>;
type PollEventsFn = (handle: number) => string;
type AppendSystemContextFn = (
  handle: number,
  request_json: string,
) => Promise<string>;

/**
 * Metadata carried by the canonical runtime input for this turn.
 */
export interface BrowserTurnOptions {
  readonly handlingMode?: WireTurnInputOptions['handling_mode'];
  readonly transientTurnContext?: WireTurnInputOptions['transient_turn_context'];
  readonly skillReferences?: WireTurnInputOptions['skill_references'];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Serialize a prompt into the tagged content-input wire shape carried by the
 * WASM content-bearing exports.
 *
 * K19: the discriminator is the tag — `{"text": ...}` for plain text and
 * `{"blocks": [...]}` for structured blocks. The Rust side parses the tagged
 * shape fail-closed, so a plain-text prompt whose body happens to be valid
 * block-array JSON can never be misread as structured blocks, and malformed
 * blocks surface INVALID_PARAMS instead of being downgraded to text.
 */
export function serializePromptContentInput(
  prompt: string | ContentBlock[],
): string {
  return typeof prompt === 'string'
    ? JSON.stringify({ text: prompt })
    : JSON.stringify({ blocks: prompt });
}

/**
 * Typed error surfaced on the Err channel of a runtime operation.
 *
 * Carries the stable wire `code` (e.g. `AGENT_ERROR`, `SESSION_BUSY`) emitted
 * by the WASM runtime's typed `{ code, message }` envelope, so callers can
 * classify a terminal fault without sniffing message strings.
 */
export class MeerkatError extends Error {
  readonly code: string;
  readonly data?: unknown;

  constructor(code: string, message: string, data?: unknown) {
    super(message);
    this.name = 'MeerkatError';
    this.code = code;
    this.data = data;
  }

  /**
   * Build a {@link MeerkatError} from a rejected WASM call. The rejection
   * reason is the runtime's `{ code, message }` JSON envelope (as a string or
   * object); fall back to the raw reason when it is not a typed envelope.
   */
  static fromWasm(reason: unknown): MeerkatError {
    const envelope = parseErrorEnvelope(reason);
    if (envelope) {
      return new MeerkatError(envelope.code, envelope.message, envelope.data);
    }
    const message =
      reason instanceof Error
        ? reason.message
        : typeof reason === 'string'
          ? reason
          : String(reason);
    return new MeerkatError('UNKNOWN', message);
  }
}

function parseErrorEnvelope(
  reason: unknown,
): { code: string; message: string; data?: unknown } | undefined {
  let record: unknown = reason;
  if (typeof reason === 'string') {
    try {
      record = JSON.parse(reason) as unknown;
    } catch {
      return undefined;
    }
  }
  if (!isRecord(record)) {
    return undefined;
  }
  const code = record.code;
  if (typeof code !== 'string') {
    return undefined;
  }
  const message = typeof record.message === 'string' ? record.message : code;
  return { code, message, data: record.data };
}

function normalizeSessionEvent(raw: unknown): SessionEvent {
  if (!isRecord(raw)) {
    throw new Error('Invalid session event: expected object');
  }
  // K19: a lagged receiver arrives as the generated `stream_truncated`
  // AgentEvent — it flows through the generated-event inventory check below
  // like every other event; there is no hand-modeled lag sentinel.
  if (typeof raw.type !== 'string' || raw.type.length === 0) {
    throw new Error('Invalid session event: missing type');
  }
  // Fail closed on an unrecognized event type: validate the full record
  // against the generated `AgentEvent` inventory via `isKnownEvent` rather
  // than blindly casting an arbitrary record to `SessionEvent`. An unknown
  // discriminant is a malformed/forward-incompatible wire shape, not a
  // silently-coercible one. The full record is passed (not a synthetic
  // `{ type }`) so the structural skill-resolution guards inside
  // `isKnownEvent` validate against the real payload.
  const event = foldUnknownSkillResolutionReason(raw as { type: string });
  if (!isKnownEvent(event)) {
    throw new Error(`Invalid session event: unknown event type "${raw.type}"`);
  }
  return event as unknown as SessionEvent;
}

function normalizeSessionEvents(raw: unknown): SessionEvent[] {
  if (!Array.isArray(raw)) {
    throw new Error('Invalid session poll response: expected event array');
  }
  return raw.map((item) => normalizeSessionEvent(item));
}

/** A direct (non-mob) agent session. */
export class Session {
  /** @internal — browser-local façade handle, not the authoritative session ID. */
  readonly handle: number;

  private startTurnFn: StartTurnFn;
  private getStateFn: GetSessionStateFn;
  private destroyFn: DestroySessionFn;
  private pollFn: PollEventsFn;
  private appendSystemContextFn: AppendSystemContextFn;
  private interruptFn: InterruptSessionFn;
  private wirePeerFn: WirePeerFn;
  private stopRunFn?: StopSessionRunFn;

  /** @internal — use MeerkatRuntime.createSession() instead. */
  constructor(
    handle: number,
    startTurnFn: StartTurnFn,
    getStateFn: GetSessionStateFn,
    destroyFn: DestroySessionFn,
    pollFn: PollEventsFn,
    appendSystemContextFn: AppendSystemContextFn,
    interruptFn: InterruptSessionFn,
    wirePeerFn: WirePeerFn,
    stopRunFn?: StopSessionRunFn,
  ) {
    this.handle = handle;
    this.startTurnFn = startTurnFn;
    this.getStateFn = getStateFn;
    this.destroyFn = destroyFn;
    this.pollFn = pollFn;
    this.appendSystemContextFn = appendSystemContextFn;
    this.interruptFn = interruptFn;
    this.wirePeerFn = wirePeerFn;
    this.stopRunFn = stopRunFn;
  }

  /**
   * Run a turn through the agent loop.
   *
   * On success, resolves with the canonical {@link TurnResult} (the
   * `WireRunResult` shape, exposing `terminal_cause_kind`). On any agent-level
   * fault the underlying WASM call rejects on the Err channel; this method
   * surfaces it as a {@link MeerkatError} carrying the typed error code —
   * never a synthetic empty-text "success".
   */
  async turn(
    prompt: string | ContentBlock[],
    options?: BrowserTurnOptions,
  ): Promise<TurnResult> {
    const promptStr = serializePromptContentInput(prompt);
    let json: string;
    try {
      json = await this.startTurnFn(
        this.handle,
        promptStr,
        options === undefined ? undefined : JSON.stringify({
          handling_mode: options.handlingMode,
          transient_turn_context: options.transientTurnContext,
          skill_references: options.skillReferences,
        }),
      );
    } catch (error) {
      throw MeerkatError.fromWasm(error);
    }
    const parsed = JSON.parse(json) as Partial<TurnResult>;
    if (typeof parsed.text !== 'string') {
      throw new MeerkatError(
        'MALFORMED_RUN_RESULT',
        'turn result is missing canonical text field',
      );
    }
    return parsed as TurnResult;
  }

  /** Get the current canonical session state. */
  async getState(): Promise<SessionState> {
    try {
      return JSON.parse(await this.getStateFn(this.handle)) as SessionState;
    } catch (error) {
      throw MeerkatError.fromWasm(error);
    }
  }

  /** The canonical session ID behind this local browser handle. */
  get sessionId(): Promise<string> {
    return this.getState().then((state) => state.session_id);
  }

  /** Poll buffered agent events from the last turn. */
  pollEvents(): SessionEvent[] {
    const json = this.pollFn(this.handle);
    const parsed: unknown = JSON.parse(json);
    return normalizeSessionEvents(parsed);
  }

  /**
   * Observe session events through the direct handle's buffered event source.
   *
   * This projects the runtime event stream for direct WASM sessions. It uses
   * the same underlying event buffer as `pollEvents()`, so callers should use
   * either `pollEvents()` or the returned subscription, not both at once.
   */
  subscribe(): EventSubscription<SessionEvent> {
    return new EventSubscription<SessionEvent>(
      () => this.pollFn(this.handle),
      (raw) => raw.map((item) => normalizeSessionEvent(item)),
    );
  }

  /** Append one ordinary durable ordered System message at the admitted transcript boundary. */
  async appendSystemContext(
    options: AppendSystemContextOptions,
  ): Promise<AppendSystemContextResult> {
    const json = await this.appendSystemContextFn(
      this.handle,
      JSON.stringify({
        text: options.text,
        source: options.source,
        idempotency_key: options.idempotencyKey,
      }),
    );
    return JSON.parse(json) as AppendSystemContextResult;
  }

  /**
   * Destroy the session and release resources.
   *
   * Idempotent: a tear-down or an already-retired handle is classified by the
   * runtime's typed error `code` (`not_initialized` / `invalid_session_handle`),
   * not by sniffing the message string.
   */
  async destroy(): Promise<void> {
    try {
      await this.destroyFn(this.handle);
    } catch (error) {
      if (isAlreadyGoneError(error)) {
        return;
      }
      throw MeerkatError.fromWasm(error);
    }
  }

  /** Trust another direct session for incoming in-process comms. */
  async wirePeer(peer: Session): Promise<void> {
    try {
      await this.wirePeerFn(this.handle, peer.handle);
    } catch (error) {
      throw MeerkatError.fromWasm(error);
    }
  }

  /**
   * Stop one exact run and terminalize every input bound to it. `runId`
   * comes from the `run_started` event (`identity.run_id`). Unlike
   * `interrupt()`, a durable steer that already joined the run is cancelled
   * with it instead of taking a follow-up turn. A stale `runId` resolves to a
   * `not_current` receipt and touches nothing.
   */
  async stopRun(runId: string, reason: string): Promise<WireRunStopReceipt> {
    if (!this.stopRunFn) {
      throw new MeerkatError('CAPABILITY_UNAVAILABLE', 'this runtime binding has no run-fenced stop');
    }
    let json: string;
    try {
      json = await this.stopRunFn(this.handle, runId, reason);
    } catch (error) {
      throw MeerkatError.fromWasm(error);
    }
    return parseRunStopReceipt(JSON.parse(json));
  }

  /** Interrupt active work through the runtime's cancellation owner. */
  async interrupt(): Promise<void> {
    try {
      await this.interruptFn(this.handle);
    } catch (error) {
      throw MeerkatError.fromWasm(error);
    }
  }


}

/**
 * Codes that mean "the session/runtime this handle pointed at is already gone".
 *
 * A destroyed handle is retired by the runtime (fail-closed), so a repeat
 * `destroy()` surfaces `invalid_session_handle`; a torn-down runtime surfaces
 * `not_initialized`. Both make `destroy()` idempotent.
 */
const ALREADY_GONE_CODES = new Set(['not_initialized', 'invalid_session_handle']);

/**
 * Classify a destroy failure as "already gone" using the typed wire `code`
 * only — never by matching message strings.
 */
function isAlreadyGoneError(error: unknown): boolean {
  const code = extractErrorCode(error);
  return code != null && ALREADY_GONE_CODES.has(code.toLowerCase());
}

function extractErrorCode(error: unknown): string | undefined {
  // The runtime rejects with its typed `{ code, message }` JSON envelope (as a
  // string or object). Parse it for the stable code rather than sniffing text.
  const envelope = parseErrorEnvelope(error);
  if (envelope) {
    return envelope.code;
  }
  if (!error || typeof error !== 'object') return undefined;
  const record = error as { code?: unknown; cause?: unknown };
  if (typeof record.code === 'string') {
    return record.code;
  }
  if (record.cause && typeof record.cause === 'object') {
    const causeCode = (record.cause as { code?: unknown }).code;
    if (typeof causeCode === 'string') {
      return causeCode;
    }
  }
  return undefined;
}

/**
 * Validate a run-stop receipt from the WASM runtime. The receipt is a tagged
 * union on `outcome`: `stopped` carries the contributors, `not_current` the
 * current run (if any), and `not_stoppable` the runtime state that refused
 * the stop.
 */
export function parseRunStopReceipt(value: unknown): WireRunStopReceipt {
  if (typeof value !== 'object' || value === null) {
    throw new MeerkatError('INVALID_RESPONSE', 'run-stop receipt must be an object');
  }
  const receipt = value as Record<string, unknown>;
  if (typeof receipt.run_id !== 'string') {
    throw new MeerkatError('INVALID_RESPONSE', 'run-stop receipt run_id must be a string');
  }
  switch (receipt.outcome) {
    case 'stopped':
      if (
        !Array.isArray(receipt.contributors) ||
        !receipt.contributors.every(
          (row) =>
            typeof row === 'object' &&
            row !== null &&
            typeof (row as Record<string, unknown>).input_id === 'string' &&
            typeof (row as Record<string, unknown>).completion === 'string',
        )
      ) {
        throw new MeerkatError('INVALID_RESPONSE', 'stopped receipt contributors are malformed');
      }
      break;
    case 'not_current':
      if (
        receipt.current_run_id !== undefined &&
        receipt.current_run_id !== null &&
        typeof receipt.current_run_id !== 'string'
      ) {
        throw new MeerkatError('INVALID_RESPONSE', 'not_current receipt current_run_id must be a string');
      }
      break;
    case 'not_stoppable':
      if (typeof receipt.state !== 'string') {
        throw new MeerkatError('INVALID_RESPONSE', 'not_stoppable receipt needs a state');
      }
      break;
    default:
      throw new MeerkatError('INVALID_RESPONSE', `unknown run-stop receipt outcome ${String(receipt.outcome)}`);
  }
  return receipt as unknown as WireRunStopReceipt;
}
