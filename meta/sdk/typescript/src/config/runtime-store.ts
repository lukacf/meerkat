/**
 * Runtime-store durability declaration for Meta runtime.
 *
 * The gateway's runtime store (`runtime.sqlite` — session resume, archive,
 * retire) is persistent SQLite by default and needs no configuration.
 * Since the storage-unification arc (M4) a failed open is a **startup
 * error**, not a silent fall-back to an in-memory twin; the only way to
 * run in-memory on a persistent launch is the explicit declaration this
 * module produces:
 *
 * ```ts
 * import { runtimeStore } from "@rkat/meta-sdk";
 * Meta.builder().runtimeStore(runtimeStore.memory());
 * ```
 *
 * which serializes to the `runtime_options.runtime_store =
 * {"storage": "memory"}` wire form. Sessions then do not survive gateway
 * restart, and the choice is visible in the storage census
 * (`meta/status` → `storage.slots`).
 */

/**
 * Explicit declaration of an in-memory runtime store. There is
 * deliberately no persistent variant: persistent SQLite is the default
 * and only alternative, so the sole declarable choice is the ephemeral
 * one.
 */
export interface EphemeralRuntimeStoreConfig {
  toDict(): Record<string, unknown>;
}

/** Declare the runtime store in-memory (sessions do not survive restart). */
export function memory(): EphemeralRuntimeStoreConfig {
  return {
    toDict() {
      return { storage: "memory" };
    },
  };
}
