import type { EventEnvelope } from './types.js';

// Re-exported from wasm glue — will be resolved at build time
type PollSubscriptionFn = (handle: number) => string;
type CloseSubscriptionFn = (handle: number) => void;

/** Wraps a WASM subscription handle with typed polling. */
export class EventSubscription {
  private handle: number;
  private closed = false;
  private pollFn: PollSubscriptionFn;
  private closeFn: CloseSubscriptionFn;

  /** @internal — use Mob.subscribe() or Mob.subscribeAll() instead. */
  constructor(
    handle: number,
    pollFn: PollSubscriptionFn,
    closeFn: CloseSubscriptionFn,
  ) {
    this.handle = handle;
    this.pollFn = pollFn;
    this.closeFn = closeFn;
  }

  /** Poll for new events. Returns an empty array when no events are available. */
  poll(): EventEnvelope[] {
    if (this.closed) return [];
    const json = this.pollFn(this.handle);
    const parsed: unknown = JSON.parse(json);
    return Array.isArray(parsed) ? (parsed as EventEnvelope[]) : [];
  }

  /** Close the subscription and release the handle. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.closeFn(this.handle);
  }

  /** Whether this subscription has been closed. */
  get isClosed(): boolean {
    return this.closed;
  }
}
