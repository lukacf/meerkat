/**
 * Async event stream over JSON-RPC notifications from rkat rpc.
 */

import type { Interface as ReadlineInterface } from "node:readline";
import type { WireEvent } from "./generated/types.js";

/**
 * Async iterator that yields WireEvent objects from JSON-RPC notifications.
 *
 * Skips response messages (which have an `id` field) and only yields
 * notification payloads parsed as WireEvent.
 *
 * @example
 * ```ts
 * for await (const event of new EventStream(readline)) {
 *   console.log(event.session_id, event.event);
 * }
 * ```
 */
export class EventStream implements AsyncIterable<WireEvent> {
  private buffer: WireEvent[] = [];
  private waiters: Array<(value: IteratorResult<WireEvent>) => void> = [];
  private closed = false;

  constructor(rl: ReadlineInterface) {
    rl.on("line", (line: string) => {
      try {
        const data = JSON.parse(line);
        // Skip responses (they have an "id" field)
        if ("id" in data) return;

        const params = (data.params ?? {}) as Record<string, unknown>;
        const event: WireEvent = {
          session_id: String(params.session_id ?? ""),
          sequence: Number(params.sequence ?? 0),
          event: (params.event ?? {}) as Record<string, unknown>,
          contract_version: String(params.contract_version ?? ""),
        };

        if (this.waiters.length > 0) {
          const waiter = this.waiters.shift()!;
          waiter({ value: event, done: false });
        } else {
          this.buffer.push(event);
        }
      } catch {
        // Ignore non-JSON lines
      }
    });

    rl.on("close", () => {
      this.closed = true;
      // Resolve any pending waiters with done
      for (const waiter of this.waiters) {
        waiter({ value: undefined as unknown as WireEvent, done: true });
      }
      this.waiters = [];
    });
  }

  [Symbol.asyncIterator](): AsyncIterator<WireEvent> {
    return {
      next: (): Promise<IteratorResult<WireEvent>> => {
        if (this.buffer.length > 0) {
          return Promise.resolve({
            value: this.buffer.shift()!,
            done: false,
          });
        }
        if (this.closed) {
          return Promise.resolve({
            value: undefined as unknown as WireEvent,
            done: true,
          });
        }
        return new Promise((resolve) => {
          this.waiters.push(resolve);
        });
      },
    };
  }
}
