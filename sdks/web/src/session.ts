import { EventSubscription } from './events.js';
import type {
  ContentBlock,
  TurnOptions,
  TurnResult,
  SessionEvent,
  SessionState,
  AppendSystemContextOptions,
  AppendSystemContextResult,
} from './types.js';

// WASM function signatures (bound at construction)
type StartTurnFn = (handle: number, prompt: string, options_json: string) => Promise<string>;
type GetSessionStateFn = (handle: number) => string;
type DestroySessionFn = (handle: number) => void;
type PollEventsFn = (handle: number) => string;
type AppendSystemContextFn = (
  handle: number,
  request_json: string,
) => string;

/** A direct (non-mob) agent session. */
export class Session {
  /** @internal — browser-local façade handle, not the authoritative session ID. */
  readonly handle: number;

  private startTurnFn: StartTurnFn;
  private getStateFn: GetSessionStateFn;
  private destroyFn: DestroySessionFn;
  private pollFn: PollEventsFn;
  private appendSystemContextFn: AppendSystemContextFn;
  private destroyed = false;
  private cachedSessionId?: string;

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

  /** Run a turn through the agent loop. */
  async turn(prompt: string | ContentBlock[], options?: TurnOptions): Promise<TurnResult> {
    if (this.destroyed) throw new Error('Session has been destroyed');
    const opts = options
      ? JSON.stringify({
          additional_instructions: options.additionalInstructions,
        })
      : '{}';
    const promptStr = typeof prompt === 'string' ? prompt : JSON.stringify(prompt);
    const json = await this.startTurnFn(this.handle, promptStr, opts);
    const parsed = JSON.parse(json) as Partial<TurnResult> & {
      text?: string;
      response?: string;
    };
    const text =
      typeof parsed.text === 'string'
        ? parsed.text
        : typeof parsed.response === 'string'
          ? parsed.response
          : '';
    return {
      ...parsed,
      text,
      response: typeof parsed.response === 'string' ? parsed.response : text,
    } as TurnResult;
  }

  /** Get the current runtime-backed session state. */
  getState(): SessionState {
    if (this.destroyed) throw new Error('Session has been destroyed');
    const state = JSON.parse(this.getStateFn(this.handle)) as SessionState;
    this.cachedSessionId = state.session_id;
    return state;
  }

  /** The authoritative runtime session ID behind this local browser handle. */
  get sessionId(): string {
    if (this.destroyed) throw new Error('Session has been destroyed');
    return this.cachedSessionId ?? this.getState().session_id;
  }

  /** Poll buffered agent events from the last turn. */
  pollEvents(): SessionEvent[] {
    if (this.destroyed) return [];
    const json = this.pollFn(this.handle);
    const parsed: unknown = JSON.parse(json);
    return Array.isArray(parsed) ? (parsed as SessionEvent[]) : [];
  }

  /**
   * Observe session events through the direct handle's buffered event source.
   *
   * This is the standalone observation API for direct WASM sessions. It uses
   * the same underlying event buffer as `pollEvents()`, so callers should use
   * either `pollEvents()` or the returned subscription, not both at once.
   */
  subscribe(): EventSubscription<SessionEvent> {
    if (this.destroyed) throw new Error('Session has been destroyed');
    return new EventSubscription<SessionEvent>(
      () => (this.destroyed ? '[]' : this.pollFn(this.handle)),
      (raw) => (Array.isArray(raw) ? (raw as SessionEvent[]) : []),
    );
  }

  /** Stage runtime system context for application at the next LLM boundary. */
  appendSystemContext(
    options: AppendSystemContextOptions,
  ): AppendSystemContextResult {
    if (this.destroyed) throw new Error('Session has been destroyed');
    const json = this.appendSystemContextFn(
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
    if (this.destroyed) return;
    try {
      this.destroyFn(this.handle);
      this.destroyed = true;
    } catch (error) {
      if (isRuntimeNotInitializedError(error)) {
        this.destroyed = true;
        return;
      }
      throw error;
    }
  }

  /** Whether this session has been destroyed. */
  get isDestroyed(): boolean {
    return this.destroyed;
  }
}

function isRuntimeNotInitializedError(error: unknown): boolean {
  const message =
    error instanceof Error
      ? error.message
      : typeof error === 'string'
        ? error
        : '';
  return /not_initialized/i.test(message);
}
