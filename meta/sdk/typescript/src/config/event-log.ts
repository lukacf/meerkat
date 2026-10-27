/**
 * Event-log durability declaration for Meta runtime.
 *
 * The gateway ingests operational events only when
 * `runtime_options.event_log` is configured; the absence of the key means
 * no ingestion at all (the honest silent case). The two wire-supported
 * storage kinds are both *declared ephemeral* choices:
 *
 * - {@link memory} — a bounded, queryable in-process store
 *   (`{"storage": "memory"}`); serves `meta/query_events`.
 * - {@link nullStore} — events are explicitly dropped
 *   (`{"storage": "null"}`); queries return empty.
 *
 * Durable event-log backends are embedder-only today: the Rust
 * `UnifiedRuntimeBuilder` accepts any `EventLogStore` implementation, but
 * the gateway wire supports only the declarations above and rejects
 * anything else at startup. The resolved choice is visible in the storage
 * census (`meta/status` → `storage.slots`).
 */

/** A typed event-log declaration for `MetaBuilder.eventLog()`. */
export interface EventLogDeclaration {
  toDict(): Record<string, unknown>;
}

/** Declare a bounded queryable in-process event store. */
export function memory(options?: {
  batchSize?: number;
  flushIntervalMs?: number;
}): EventLogDeclaration {
  // A zero interval panics the gateway's ingestion task
  // (`tokio::time::interval` requires a non-zero period), and the wire
  // only accepts positive integers.
  const flushIntervalMs = options?.flushIntervalMs;
  if (
    flushIntervalMs !== undefined &&
    (!Number.isInteger(flushIntervalMs) || flushIntervalMs <= 0)
  ) {
    throw new Error(
      `eventLog.memory: flushIntervalMs must be a positive integer (got ${flushIntervalMs})`,
    );
  }
  return {
    toDict() {
      const result: Record<string, unknown> = { storage: "memory" };
      if (options?.batchSize !== undefined) result.batch_size = options.batchSize;
      if (options?.flushIntervalMs !== undefined) {
        result.flush_interval_ms = options.flushIntervalMs;
      }
      return result;
    },
  };
}

/**
 * Declare that operational events are dropped. (Named `nullStore` because
 * `null` is a reserved word; re-exported as `eventLog.nullStore`.)
 */
export function nullStore(): EventLogDeclaration {
  return {
    toDict() {
      return { storage: "null" };
    },
  };
}
