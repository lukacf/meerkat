/** Incremental SSE decoding with an explicit choice of ownership for delivered frames. */
export interface SseReadOptions<T> {
  parseBlock(block: string): readonly T[];
  accept?(frame: T): boolean;
  onFrame?(frame: T): void;
  terminal?(frame: T): boolean;
  signal?: AbortSignal;
}

/** A consumer exception is not a network fault and must never trigger replay. */
export class ConsoleConsumerError extends Error {
  readonly cause: unknown;
  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause));
    this.name = "ConsoleConsumerError";
    this.cause = cause;
  }
}

export function acceptConsoleFrame<T>(consumer: ((frame: T) => void) | undefined, frame: T): void {
  try {
    const result: unknown = consumer?.(frame);
    if (result && typeof (result as unknown as PromiseLike<unknown>).then === "function") {
      // Synchronous acceptance is the contract. Consume a rejection so misuse
      // cannot also become an unhandled promise rejection.
      void Promise.resolve(result).catch(() => {});
      throw new Error("Console frame consumers must accept synchronously");
    }
  } catch (error) {
    throw error instanceof ConsoleConsumerError ? error : new ConsoleConsumerError(error);
  }
}

export function consumeSseResponse<T>(response: Response, options: SseReadOptions<T> & { mode: "collect" }): Promise<T[]>;
export function consumeSseResponse<T>(response: Response, options: SseReadOptions<T> & { mode: "consume" }): Promise<void>;
export async function consumeSseResponse<T>(
  response: Response,
  options: SseReadOptions<T> & { mode: "collect" | "consume" },
): Promise<T[] | void> {
  // Continuous subscriptions never create a collector. A finite caller owns
  // its result and explicitly opts into collection.
  const collected = options.mode === "collect" ? [] as T[] : undefined;
  let terminal = false;
  let buffer = "";
  const dispatch = (block: string) => {
    for (const frame of options.parseBlock(block)) {
      if (options.signal?.aborted || terminal) break;
      if (options.accept && !options.accept(frame)) continue;
      acceptConsoleFrame(options.onFrame, frame);
      collected?.push(frame);
      if (options.terminal?.(frame)) terminal = true;
    }
  };
  let pendingCarriageReturn = false;
  const decodeLines = (chunk: string, final = false) => {
    if (pendingCarriageReturn) {
      chunk = "\n" + (chunk.startsWith("\n") ? chunk.slice(1) : chunk);
      pendingCarriageReturn = false;
    }
    if (!final && chunk.endsWith("\r")) {
      pendingCarriageReturn = true;
      chunk = chunk.slice(0, -1);
    }
    return chunk.replace(/\r\n|\r/g, "\n");
  };
  const flush = (final = false) => {
    let boundary: number;
    while (!terminal && (boundary = buffer.indexOf("\n\n")) >= 0) {
      dispatch(buffer.slice(0, boundary + 2));
      buffer = buffer.slice(boundary + 2);
    }
    if (final && !terminal && buffer.trim()) dispatch(buffer);
  };
  if (options.signal?.aborted) return collected;
  if (!response.body || typeof response.body.getReader !== "function") {
    buffer = decodeLines(await response.text(), true);
    if (!options.signal?.aborted) flush(true);
    return collected;
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const abort = () => { void reader.cancel().catch(() => {}); };
  options.signal?.addEventListener("abort", abort, { once: true });
  try {
    while (!terminal && !options.signal?.aborted) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decodeLines(decoder.decode(value, { stream: true }));
      flush();
    }
    if (!terminal && !options.signal?.aborted) {
      buffer += decodeLines(decoder.decode(), true);
      flush(true);
    }
  } finally {
    options.signal?.removeEventListener("abort", abort);
    try { await reader.cancel(); } catch { /* Cancellation is best effort. */ }
    reader.releaseLock();
  }
  return collected;
}
