import type { TurnOptions, TurnResult, EventEnvelope } from './types.js';

// WASM function signatures (bound at construction)
type StartTurnFn = (handle: number, prompt: string, options_json: string) => Promise<string>;
type GetSessionStateFn = (handle: number) => string;
type DestroySessionFn = (handle: number) => void;
type PollEventsFn = (handle: number) => string;

/** A direct (non-mob) agent session. */
export class Session {
  /** @internal — WASM session handle. */
  readonly handle: number;

  private startTurnFn: StartTurnFn;
  private getStateFn: GetSessionStateFn;
  private destroyFn: DestroySessionFn;
  private pollFn: PollEventsFn;
  private destroyed = false;

  /** @internal — use MeerkatRuntime.createSession() instead. */
  constructor(
    handle: number,
    startTurnFn: StartTurnFn,
    getStateFn: GetSessionStateFn,
    destroyFn: DestroySessionFn,
    pollFn: PollEventsFn,
  ) {
    this.handle = handle;
    this.startTurnFn = startTurnFn;
    this.getStateFn = getStateFn;
    this.destroyFn = destroyFn;
    this.pollFn = pollFn;
  }

  /** Run a turn through the agent loop. */
  async turn(prompt: string, options?: TurnOptions): Promise<TurnResult> {
    if (this.destroyed) throw new Error('Session has been destroyed');
    const opts = options
      ? JSON.stringify({
          additional_instructions: options.additionalInstructions,
        })
      : '{}';
    const json = await this.startTurnFn(this.handle, prompt, opts);
    return JSON.parse(json) as TurnResult;
  }

  /** Get the current session state. */
  getState(): unknown {
    if (this.destroyed) throw new Error('Session has been destroyed');
    return JSON.parse(this.getStateFn(this.handle));
  }

  /** Poll buffered agent events from the last turn. */
  pollEvents(): EventEnvelope[] {
    if (this.destroyed) return [];
    const json = this.pollFn(this.handle);
    const parsed: unknown = JSON.parse(json);
    return Array.isArray(parsed) ? (parsed as EventEnvelope[]) : [];
  }

  /** Destroy the session and release resources. */
  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.destroyFn(this.handle);
  }

  /** Whether this session has been destroyed. */
  get isDestroyed(): boolean {
    return this.destroyed;
  }
}
