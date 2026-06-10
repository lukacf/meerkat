import { AsyncQueue } from "./streaming.js";

export class EventSubscription<T> implements AsyncIterable<T> {
  private readonly streamId: string;
  private readonly queue: AsyncQueue<Record<string, unknown> | null>;
  private readonly closeRemote: (streamId: string) => Promise<void>;
  private readonly parseEvent: (raw: Record<string, unknown>) => T;
  private readonly getTerminalOutcome: () => Record<string, unknown> | undefined;
  private closed = false;
  private closeInFlight: Promise<void> | null = null;

  /** @internal */
  constructor(opts: {
    streamId: string;
    queue: AsyncQueue<Record<string, unknown> | null>;
    closeRemote: (streamId: string) => Promise<void>;
    parseEvent: (raw: Record<string, unknown>) => T;
    getTerminalOutcome: () => Record<string, unknown> | undefined;
  }) {
    this.streamId = opts.streamId;
    this.queue = opts.queue;
    this.closeRemote = opts.closeRemote;
    this.parseEvent = opts.parseEvent;
    this.getTerminalOutcome = opts.getTerminalOutcome;
  }

  get id(): string {
    return this.streamId;
  }

  get isClosed(): boolean {
    return this.closed;
  }

  get terminalOutcome(): Record<string, unknown> | undefined {
    return this.getTerminalOutcome();
  }

  async close(): Promise<void> {
    if (this.closed) return;
    // Stream-close authority is server-owned: await the RPC close BEFORE
    // surfacing closed terminal state locally. A rejected close propagates
    // the typed error and leaves the subscription open (retryable); only an
    // accepted close flips `isClosed` and ends the local queue.
    if (!this.closeInFlight) {
      this.closeInFlight = (async () => {
        try {
          await this.closeRemote(this.streamId);
        } catch (error) {
          this.closeInFlight = null;
          throw error;
        }
        this.closed = true;
        this.queue.put(null);
      })();
    }
    await this.closeInFlight;
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<T, void, undefined> {
    while (true) {
      const raw = await this.queue.get();
      if (raw == null) {
        return;
      }
      yield this.parseEvent(raw);
    }
  }
}
