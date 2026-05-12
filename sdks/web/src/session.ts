import { EventSubscription } from './events.js';
import { STREAM_LAGGED_EVENT_TYPE } from './generated/events.js';
import type {
  ContentBlock,
  TurnResult,
  SessionEvent,
  SessionState,
  TurnTerminalResult,
  AppendSystemContextOptions,
  AppendSystemContextResult,
  SubscriptionLaggedEvent,
  ContentInput,
} from './types.js';

// WASM function signatures (bound at construction)
type StartTurnFn = (sessionId: string, prompt: ContentInput) => Promise<string>;
type GetSessionStateFn = (sessionId: string) => string;
type DestroySessionFn = (sessionId: string) => void;
type PollEventsFn = (sessionId: string) => string;
type AppendSystemContextFn = (
  sessionId: string,
  request_json: string,
) => Promise<string>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function normalizeSessionLaggedEvent(record: Record<string, unknown>): SubscriptionLaggedEvent {
  const skipped = record.skipped;
  if (typeof skipped !== 'number' || !Number.isFinite(skipped)) {
    throw new Error('Invalid session event: lagged event missing skipped count');
  }
  return {
    type: STREAM_LAGGED_EVENT_TYPE,
    skipped,
  };
}

function normalizeSessionEvent(raw: unknown): SessionEvent {
  if (!isRecord(raw)) {
    throw new Error('Invalid session event: expected object');
  }
  if (raw.type === STREAM_LAGGED_EVENT_TYPE) {
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

function normalizeTurnTerminal(raw: unknown): TurnTerminalResult {
  if (!isRecord(raw) || typeof raw.outcome !== 'string' || raw.outcome.length === 0) {
    throw new Error('Invalid turn result: missing typed terminal result');
  }
  return raw as unknown as TurnTerminalResult;
}

function parseJsonPayload(json: string, context: string): unknown {
  try {
    return JSON.parse(json) as unknown;
  } catch {
    throw new Error(`${context}: invalid JSON`);
  }
}

function parseJsonString(value: unknown): unknown {
  if (typeof value !== 'string') return undefined;
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return undefined;
  }
}

function normalizeTurnResult(raw: unknown): TurnResult {
  const context = 'Invalid turn response';
  if (!isRecord(raw)) {
    throw new Error(`${context}: expected object`);
  }
  const response =
    typeof raw.response === 'string'
      ? raw.response
      : typeof raw.text === 'string'
        ? raw.text
        : undefined;
  if (response === undefined) {
    throw new Error(`${context}: missing text`);
  }
  const terminal = normalizeTurnTerminal(raw.terminal);
  return {
    ...raw,
    text: typeof raw.text === 'string' ? raw.text : response,
    response,
    terminal,
    status: terminal.outcome,
  } as TurnResult;
}

/** Error thrown when a direct turn reaches typed terminal failure. */
export class TurnTerminalError extends Error {
  readonly code: string;
  readonly terminal: TurnTerminalResult;
  readonly payload: unknown;

  constructor(code: string, message: string, terminal: TurnTerminalResult, payload: unknown) {
    super(message);
    this.name = 'TurnTerminalError';
    this.code = code;
    this.terminal = terminal;
    this.payload = payload;
  }
}

function normalizeStartTurnError(error: unknown): unknown {
  const parsed = parseJsonString(error);
  if (isRecord(parsed) && typeof parsed.message === 'string' && isRecord(parsed.terminal)) {
    const code = typeof parsed.code === 'string' ? parsed.code : 'agent_error';
    return new TurnTerminalError(
      code,
      parsed.message,
      normalizeTurnTerminal(parsed.terminal),
      parsed,
    );
  }
  return error;
}

/** A direct (non-mob) agent session. */
export class Session {
  /** Authoritative runtime session ID for direct browser session control. */
  readonly sessionId: string;

  private startTurnFn: StartTurnFn;
  private getStateFn: GetSessionStateFn;
  private destroyFn: DestroySessionFn;
  private pollFn: PollEventsFn;
  private appendSystemContextFn: AppendSystemContextFn;

  /** @internal — use MeerkatRuntime.createSession() instead. */
  constructor(
    sessionId: string,
    startTurnFn: StartTurnFn,
    getStateFn: GetSessionStateFn,
    destroyFn: DestroySessionFn,
    pollFn: PollEventsFn,
    appendSystemContextFn: AppendSystemContextFn,
  ) {
    this.sessionId = sessionId;
    this.startTurnFn = startTurnFn;
    this.getStateFn = getStateFn;
    this.destroyFn = destroyFn;
    this.pollFn = pollFn;
    this.appendSystemContextFn = appendSystemContextFn;
  }

  /** Run a turn through the agent loop. */
  async turn(prompt: string | ContentBlock[]): Promise<TurnResult> {
    let json: string;
    try {
      json = await this.startTurnFn(this.sessionId, prompt);
    } catch (error) {
      throw normalizeStartTurnError(error);
    }
    return normalizeTurnResult(parseJsonPayload(json, 'Invalid turn response'));
  }

  /** Get the current runtime-backed session state. */
  getState(): SessionState {
    return JSON.parse(this.getStateFn(this.sessionId)) as SessionState;
  }

  /** Poll buffered agent events from the last turn. */
  pollEvents(): SessionEvent[] {
    const json = this.pollFn(this.sessionId);
    const parsed: unknown = JSON.parse(json);
    return normalizeSessionEvents(parsed);
  }

  /**
   * Observe session events through the direct session's buffered event source.
   *
   * This is the standalone observation API for direct WASM sessions. It uses
   * the same underlying event buffer as `pollEvents()`, so callers should use
   * either `pollEvents()` or the returned subscription, not both at once.
   */
  subscribe(): EventSubscription<SessionEvent> {
    return new EventSubscription<SessionEvent>(
      () => this.pollFn(this.sessionId),
      (raw) => raw.map((item) => normalizeSessionEvent(item)),
    );
  }

  /** Stage runtime system context for application at the next LLM boundary. */
  async appendSystemContext(
    options: AppendSystemContextOptions,
  ): Promise<AppendSystemContextResult> {
    const json = await this.appendSystemContextFn(
      this.sessionId,
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
      this.destroyFn(this.sessionId);
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
   * Browser-local sessions no longer report lifecycle state from cached object
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
