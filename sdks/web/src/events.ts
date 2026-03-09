import type { EventEnvelope } from './types.js';

/** Wraps a polling event source with typed JSON decoding. */
export class EventSubscription {
  private closed = false;
  private readonly pollSource: () => string;
  private readonly closeSource: () => void;

  /** @internal — use Session.subscribe(), Mob.subscribe(), or Mob.subscribeAll(). */
  constructor(
    pollSource: () => string,
    closeSource: () => void = () => {},
  ) {
    this.pollSource = pollSource;
    this.closeSource = closeSource;
  }

  /** Poll for new events. Returns an empty array when no events are available. */
  poll(): EventEnvelope[] {
    if (this.closed) return [];
    const json = this.pollSource();
    const parsed: unknown = JSON.parse(json);
    return Array.isArray(parsed) ? (parsed as EventEnvelope[]) : [];
  }

  /** Close the subscription and release the underlying source if needed. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.closeSource();
  }

  /** Whether this subscription has been closed. */
  get isClosed(): boolean {
    return this.closed;
  }
}
