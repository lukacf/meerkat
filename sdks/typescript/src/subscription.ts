import { AsyncQueue } from "./streaming.js";

export class EventSubscription<T> implements AsyncIterable<T> {
  private readonly streamId: string;
  private readonly queue: AsyncQueue<Record<string, unknown> | null>;
  private readonly closeRemote: (streamId: string) => Promise<void>;
  private readonly parseEvent: (raw: Record<string, unknown>) => T;
  private closed = false;

  /** @internal */
  constructor(opts: {
    streamId: string;
    queue: AsyncQueue<Record<string, unknown> | null>;
    closeRemote: (streamId: string) => Promise<void>;
    parseEvent: (raw: Record<string, unknown>) => T;
  }) {
    this.streamId = opts.streamId;
    this.queue = opts.queue;
    this.closeRemote = opts.closeRemote;
    this.parseEvent = opts.parseEvent;
  }

  get id(): string {
    return this.streamId;
  }

  get isClosed(): boolean {
    return this.closed;
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.queue.put(null);
    await this.closeRemote(this.streamId);
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
