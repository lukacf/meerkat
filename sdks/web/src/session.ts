import { EventSubscription } from './events.js';
import type {
  ContentBlock,
  TurnResult,
  SessionEvent,
  SessionState,
  AppendSystemContextOptions,
  AppendSystemContextResult,
  SubscriptionLaggedEvent,
} from './types.js';

// WASM function signatures (bound at construction)
type StartTurnFn = (handle: number, prompt: string) => Promise<string>;
type GetSessionStateFn = (handle: number) => string;
type DestroySessionFn = (handle: number) => void;
type PollEventsFn = (handle: number) => string;
type AppendSystemContextFn = (
  handle: number,
  request_json: string,
) => Promise<string>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
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

  constructor(code: string, message: string) {
    super(message);
    this.name = 'MeerkatError';
    this.code = code;
  }

  /**
   * Build a {@link MeerkatError} from a rejected WASM call. The rejection
   * reason is the runtime's `{ code, message }` JSON envelope (as a string or
   * object); fall back to the raw reason when it is not a typed envelope.
   */
  static fromWasm(reason: unknown): MeerkatError {
    const envelope = parseErrorEnvelope(reason);
    if (envelope) {
      return new MeerkatError(envelope.code, envelope.message);
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
): { code: string; message: string } | undefined {
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
  return { code, message };
}

function normalizeSessionLaggedEvent(record: Record<string, unknown>): SubscriptionLaggedEvent {
  const skipped = record.skipped;
  if (typeof skipped !== 'number' || !Number.isFinite(skipped)) {
    throw new Error('Invalid session event: lagged event missing skipped count');
  }
  return {
    type: 'lagged',
    skipped,
  };
}

function normalizeSessionEvent(raw: unknown): SessionEvent {
  if (!isRecord(raw)) {
    throw new Error('Invalid session event: expected object');
  }
  if (raw.type === 'lagged') {
    return normalizeSessionLaggedEvent(raw);
  }
  if (typeof raw.type !== 'string' || raw.type.length === 0) {
    throw new Error('Invalid session event: missing type');
  }
  return raw as unknown as SessionEvent;
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

  /** @internal — use MeerkatRuntime.createSession() instead. */
  constructor(
    handle: number,
    startTurnFn: StartTurnFn,
    getStateFn: GetSessionStateFn,
    destroyFn: DestroySessionFn,
    pollFn: PollEventsFn,
    appendSystemContextFn: AppendSystemContextFn,
  ) {
    this.handle = handle;
    this.startTurnFn = startTurnFn;
    this.getStateFn = getStateFn;
    this.destroyFn = destroyFn;
    this.pollFn = pollFn;
    this.appendSystemContextFn = appendSystemContextFn;
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
  async turn(prompt: string | ContentBlock[]): Promise<TurnResult> {
    const promptStr = typeof prompt === 'string' ? prompt : JSON.stringify(prompt);
    let json: string;
    try {
      json = await this.startTurnFn(this.handle, promptStr);
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
    return {
      ...parsed,
      text: parsed.text,
      // `response` is a backward-compatible alias for the canonical text; it is
      // not a separate truth source and must not synthesize empty content.
      response: parsed.text,
    } as TurnResult;
  }

  /** Get the current runtime-backed session state. */
  getState(): SessionState {
    return JSON.parse(this.getStateFn(this.handle)) as SessionState;
  }

  /** The authoritative runtime session ID behind this local browser handle. */
  get sessionId(): string {
    return this.getState().session_id;
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
   * This is the standalone observation API for direct WASM sessions. It uses
   * the same underlying event buffer as `pollEvents()`, so callers should use
   * either `pollEvents()` or the returned subscription, not both at once.
   */
  subscribe(): EventSubscription<SessionEvent> {
    return new EventSubscription<SessionEvent>(
      () => this.pollFn(this.handle),
      (raw) => raw.map((item) => normalizeSessionEvent(item)),
    );
  }

  /** Stage runtime system context for application at the next LLM boundary. */
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

  /** Destroy the session and release resources. */
  destroy(): void {
    try {
      this.destroyFn(this.handle);
    } catch (error) {
      if (isRuntimeNotInitializedError(error)) {
        return;
      }
      throw error;
    }
  }

  /**
   * Deprecated compatibility surface.
   *
   * Browser-local sessions no longer report lifecycle state from cached handle
   * flags. Use `getState()` and canonical runtime/session errors instead.
   */
  get isDestroyed(): boolean {
    throw new Error(
      'Session.isDestroyed is deprecated: lifecycle state is owned by the runtime; use getState() or canonical operation errors.',
    );
  }
}

function isRuntimeNotInitializedError(error: unknown): boolean {
  const code = extractErrorCode(error);
  if (code?.toLowerCase() === 'not_initialized') {
    return true;
  }
  const message =
    error instanceof Error
      ? error.message
      : typeof error === 'string'
        ? error
        : '';
  return /not_initialized/i.test(message);
}

function extractErrorCode(error: unknown): string | undefined {
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
