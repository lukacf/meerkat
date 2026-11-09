/**
 * Configuration modules for Meta runtime.
 *
 * @example
 * ```ts
 * import { auth, memory } from "@rkat/meta-sdk";
 *
 * const authConfig = auth.google("my-client-id");
 * const memConfig = memory.localJson();
 * ```
 */

export * as auth from "./auth.js";
export * as eventLog from "./event-log.js";
export * as memory from "./memory.js";
export * as runtimeStore from "./runtime-store.js";
