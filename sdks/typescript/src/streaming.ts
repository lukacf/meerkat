/**
 * Streaming API for the Meerkat TypeScript SDK.
 *
 * {@link EventStream} is an `AsyncIterable<StreamEvent>` that yields typed
 * events from a running turn, with access to the final {@link RunResult}
 * after iteration.
 *
 * @example
 * ```ts
 * for await (const event of session.stream("prompt")) {
 *   if (event.type === "text_delta") {
 *     process.stdout.write(event.delta);
 *   }
 * }
 * console.log(stream.result.usage.inputTokens);
 * ```
 */

import type { StreamEvent } from "./events.js";
import { isTextDelta, parseEvent } from "./events.js";
import type { RunResult } from "./types.js";
import type { Session } from "./session.js";
import { MeerkatError } from "./generated/errors.js";

/** @internal Queue with promise-based get(). */
class AsyncQueue<T> {
  private buffer: T[] = [];
  private waiters: Array<(value: T) => void> = [];

  put(item: T): void {
    if (this.waiters.length > 0) {
      this.waiters.shift()!(item);
    } else {
      this.buffer.push(item);
    }
  }

  get(): Promise<T> {
    if (this.buffer.length > 0) {
      return Promise.resolve(this.buffer.shift()!);
    }
    return new Promise((resolve) => {
      this.waiters.push(resolve);
    });
  }

  tryGet(): T | undefined {
    return this.buffer.shift();
  }

  get isEmpty(): boolean {
    return this.buffer.length === 0;
  }

  failAll(error: Error): void {
    // Waiters receive a rejected promise; we wrap by rejecting in the next tick
    for (const waiter of this.waiters) {
      // Use a sentinel; callers handle it
      waiter(undefined as unknown as T);
    }
    this.waiters = [];
  }
}

/** Sentinel for end-of-stream. */
const SENTINEL = Symbol("eos");

/**
 * Typed async iterable of {@link StreamEvent} objects from a running turn.
 *
 * After iteration completes, the {@link result} getter returns the final
 * {@link RunResult}.
 */
export class EventStream implements AsyncIterable<StreamEvent> {
  /** @internal */ _sessionId: string;
  private readonly _eventQueue: AsyncQueue<Record<string, unknown> | null>;
  private readonly _responsePromise: Promise<Record<string, unknown>>;
  private readonly _parseResult: (raw: Record<string, unknown>) => RunResult;
  private _result: RunResult | null = null;
  private readonly _session: Session | null;

  /** @internal — constructed by MeerkatClient, not directly. */
  constructor(opts: {
    sessionId: string;
    eventQueue: AsyncQueue<Record<string, unknown> | null>;
    responsePromise: Promise<Record<string, unknown>>;
    parseResult: (raw: Record<string, unknown>) => RunResult;
    session?: Session;
  }) {
    this._sessionId = opts.sessionId;
    this._eventQueue = opts.eventQueue;
    this._responsePromise = opts.responsePromise;
    this._parseResult = opts.parseResult;
    this._session = opts.session ?? null;
  }

  /** The session ID for this turn. */
  get sessionId(): string {
    return this._sessionId;
  }

  /** The final {@link RunResult}. Available after iteration completes. */
  get result(): RunResult {
    if (this._result === null) {
      throw new MeerkatError(
        "STREAM_NOT_CONSUMED",
        "Iterate the stream before accessing result",
      );
    }
    return this._result;
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<StreamEvent, void, undefined> {
    // Race event queue items against the response promise
    let responseDone = false;
    let responseResult: Record<string, unknown> | null = null;

    const responseHandler = this._responsePromise.then((result) => {
      responseDone = true;
      responseResult = result;
    });

    while (true) {
      if (responseDone) {
        // Drain remaining queued events
        while (!this._eventQueue.isEmpty) {
          const raw = this._eventQueue.tryGet();
          if (raw == null) break;
          yield parseEvent(raw);
        }
        this._finalise(responseResult!);
        return;
      }

      // Race: next event vs response
      const eventPromise = this._eventQueue.get();
      const result = await Promise.race([
        eventPromise.then((raw) => ({ kind: "event" as const, raw })),
        responseHandler.then(() => ({ kind: "response" as const, raw: null })),
      ]);

      if (result.kind === "event") {
        if (result.raw == null) {
          // Sentinel — stream ended
          await responseHandler;
          this._finalise(responseResult!);
          return;
        }
        yield parseEvent(result.raw);
      }
      // If response, loop back to the top which handles drain
    }
  }

  /** Consume all events silently and return the final result. */
  async collect(): Promise<RunResult> {
    for await (const _ of this) {
      // discard
    }
    return this.result;
  }

  /** Consume events, accumulate text deltas, return `[fullText, result]`. */
  async collectText(): Promise<[string, RunResult]> {
    const parts: string[] = [];
    for await (const event of this) {
      if (isTextDelta(event)) {
        parts.push(event.delta);
      }
    }
    return [parts.join(""), this.result];
  }

  private _finalise(rawResult: Record<string, unknown>): void {
    this._result = this._parseResult(rawResult);
    if (!this._sessionId) {
      this._sessionId = this._result.sessionId;
    }
    if (this._session) {
      this._session._lastResult = this._result;
    }
  }
}

/**
 * Async iterable of raw comms stream notifications emitted via `comms/stream_event`.
 */
export class CommsEventStream implements AsyncIterable<Record<string, unknown>> {
  private readonly _streamId: string;
  private readonly _eventQueue: AsyncQueue<Record<string, unknown> | null>;
  private readonly _onClose: (streamId: string) => Promise<void>;

  constructor(opts: {
    streamId: string;
    eventQueue: AsyncQueue<Record<string, unknown> | null>;
    onClose: (streamId: string) => Promise<void>;
  }) {
    this._streamId = opts.streamId;
    this._eventQueue = opts.eventQueue;
    this._onClose = opts.onClose;
  }

  get streamId(): string {
    return this._streamId;
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<Record<string, unknown>, void, undefined> {
    while (true) {
      const raw = await this._eventQueue.get();
      if (raw == null) return;
      yield raw;
    }
  }

  async close(): Promise<void> {
    await this._onClose(this._streamId);
  }
}

// Re-export AsyncQueue for internal use by client
export { AsyncQueue };
