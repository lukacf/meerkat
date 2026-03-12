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
export class AsyncQueue<T> {
  private buffer: T[] = [];
  private waiters: Array<{ id: number; resolve: () => void }> = [];
  private nextWaiterId = 0;

  put(item: T): void {
    this.buffer.push(item);
    if (this.waiters.length === 0) {
      return;
    }

    const waiters = this.waiters;
    this.waiters = [];
    for (const waiter of waiters) {
      waiter.resolve();
    }
  }

  get(): Promise<T> {
    return this._get();
  }

  private async _get(): Promise<T> {
    while (this.buffer.length === 0) {
      await this.waitForItemCancelable().promise;
    }
    return this.buffer.shift()!;
  }

  waitForItemCancelable(): { promise: Promise<void>; cancel: () => void } {
    if (this.buffer.length > 0) {
      return {
        promise: Promise.resolve(),
        cancel: () => {
          // Already resolved from the buffer.
        },
      };
    }

    const waiterId = this.nextWaiterId++;
    let settled = false;
    const promise = new Promise<void>((resolve) => {
      this.waiters.push({
        id: waiterId,
        resolve: () => {
          settled = true;
          resolve();
        },
      });
    });

    return {
      promise,
      cancel: () => {
        if (settled) {
          return;
        }
        const waiterIndex = this.waiters.findIndex((waiter) => waiter.id === waiterId);
        if (waiterIndex >= 0) {
          this.waiters.splice(waiterIndex, 1);
        }
        settled = true;
      },
    };
  }

  tryGet(): T | undefined {
    return this.buffer.shift();
  }

  get isEmpty(): boolean {
    return this.buffer.length === 0;
  }

  failAll(error: Error): void {
    // Wake every current waiter with a sentinel so pending gets can complete.
    for (let i = 0; i < Math.max(this.waiters.length, 1); i += 1) {
      this.buffer.push(undefined as unknown as T);
    }
    const waiters = this.waiters;
    this.waiters = [];
    for (const waiter of waiters) {
      waiter.resolve();
    }
  }
}

const RESPONSE_DRAIN_GRACE_MS = 75;

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
        yield* this._drainLateEvents();
        this._finalise(responseResult!);
        return;
      }

      // Race: next event vs response
      const { promise: itemReady, cancel } = this._eventQueue.waitForItemCancelable();
      const result = await Promise.race([
        itemReady.then(() => ({ kind: "event" as const })),
        responseHandler.then(() => ({ kind: "response" as const, raw: null })),
      ]);

      if (result.kind === "event") {
        const raw = this._eventQueue.tryGet();
        if (raw === undefined) {
          continue;
        }
        if (raw === null) {
          // Sentinel — stream ended
          await responseHandler;
          this._finalise(responseResult!);
          return;
        }
        yield parseEvent(raw);
      } else {
        cancel();
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

  private async *_drainLateEvents(): AsyncGenerator<StreamEvent, void, undefined> {
    let deadline = Date.now() + RESPONSE_DRAIN_GRACE_MS;
    while (true) {
      let yieldedEvent = false;
      while (!this._eventQueue.isEmpty) {
        const raw = this._eventQueue.tryGet();
        if (raw === undefined) {
          break;
        }
        if (raw === null) {
          return;
        }
        yieldedEvent = true;
        yield parseEvent(raw);
      }

      if (yieldedEvent) {
        deadline = Date.now() + RESPONSE_DRAIN_GRACE_MS;
        continue;
      }

      const remainingMs = deadline - Date.now();
      if (remainingMs <= 0) {
        return;
      }

      await new Promise((resolve) => {
        setTimeout(resolve, Math.min(remainingMs, 5));
      });
    }
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
