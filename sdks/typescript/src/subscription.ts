import { AsyncQueue } from "./streaming.js";
import { MeerkatError } from "./generated/errors.js";

/** @internal Keep RPC delivery order separate from the canonical envelope. */
export interface SubscriptionEvent {
  event: Record<string, unknown>;
  rpcSequence: number | undefined;
}

/** @internal A received terminal retains custody until the queue drains. */
export type SubscriptionItem = SubscriptionEvent | { terminalOutcome: Record<string, unknown> } | null;

/** A server-reported terminal error, raised after earlier queued events drain. */
export class EventSubscriptionError extends MeerkatError {
  constructor(
    public readonly streamId: string,
    public readonly terminalOutcome: Record<string, unknown>,
    public readonly lastRpcSequence: number | undefined,
    public readonly lastEventSeq: number | undefined,
  ) {
    const error = terminalOutcome.error;
    const valid = typeof error === "object" && error !== null && !Array.isArray(error)
      && typeof (error as Record<string, unknown>).code === "string"
      && typeof (error as Record<string, unknown>).message === "string";
    const detail = error as Record<string, unknown>;
    super(
      valid ? detail.code as string : "INVALID_RESPONSE",
      valid ? detail.message as string : "Malformed stream_end error",
      {
        stream_id: streamId,
        terminal_outcome: terminalOutcome,
        last_rpc_sequence: lastRpcSequence,
        last_event_seq: lastEventSeq,
      },
    );
    this.name = "EventSubscriptionError";
  }
}

export class EventSubscription<T> implements AsyncIterable<T> {
  private readonly streamId: string;
  private readonly queue: AsyncQueue<SubscriptionItem>;
  private readonly closeRemote: (streamId: string) => Promise<void>;
  private readonly parseEvent: (raw: Record<string, unknown>) => T;
  private readonly getTerminalOutcome: () => Record<string, unknown> | undefined;
  private closed = false;
  private closeInFlight: Promise<void> | null = null;
  private deliveredRpcSequence: number | undefined;
  private deliveredEventSeq: number | undefined;
  private receivedTerminalOutcome: Record<string, unknown> | undefined;

  /** @internal */
  constructor(opts: {
    streamId: string;
    queue: AsyncQueue<SubscriptionItem>;
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
    return this.receivedTerminalOutcome ?? this.getTerminalOutcome();
  }

  /** RPC sequence of the last delivered event, when supplied by the server. */
  get lastRpcSequence(): number | undefined {
    return this.deliveredRpcSequence;
  }

  /** Last delivered canonical event seq, not a replay cursor.
   * Gap markers do not replace it.
   */
  get lastEventSeq(): number | undefined {
    return this.deliveredEventSeq;
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
      const delivery = await this.queue.get();
      if (delivery == null || "terminalOutcome" in delivery) {
        if (delivery != null) {
          this.receivedTerminalOutcome = delivery.terminalOutcome;
        }
        const terminal = this.terminalOutcome;
        if (terminal != null && terminal.error != null) {
          throw new EventSubscriptionError(
            this.streamId, terminal, this.deliveredRpcSequence, this.deliveredEventSeq,
          );
        }
        return;
      }
      const event = this.parseEvent(delivery.event);
      const record = typeof event === "object" && event !== null
        ? event as Record<string, unknown> : undefined;
      const envelope = (record?.envelope ?? record) as Record<string, unknown> | undefined;
      this.deliveredRpcSequence = delivery.rpcSequence;
      const payload = envelope?.payload as Record<string, unknown> | undefined;
      if (payload?.type !== "stream_truncated") {
        this.deliveredEventSeq = typeof envelope?.seq === "number" ? envelope.seq : undefined;
      }
      yield event;
    }
  }
}
