/** Wraps a polling event source with typed JSON decoding. */
export class EventSubscription<T> {
  private closed = false;
  private readonly pollSource: () => string;
  private readonly closeSource: () => void;
  private readonly parseEvents: (raw: unknown) => T[];

  /** @internal — use Session.subscribe(), Member.subscribe(), Mob.subscribeMemberEvents(), or Mob.subscribeEvents(). */
  constructor(
    pollSource: () => string,
    parseEvents: (raw: unknown) => T[],
    closeSource: () => void = () => {},
  ) {
    this.pollSource = pollSource;
    this.parseEvents = parseEvents;
    this.closeSource = closeSource;
  }

  /** Poll for new events. Returns an empty array when no events are available. */
  poll(): T[] {
    if (this.closed) return [];
    const json = this.pollSource();
    const parsed: unknown = JSON.parse(json);
    return this.parseEvents(parsed);
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
