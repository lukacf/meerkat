/**
 * Meerkat TypeScript SDK — communicate with the Meerkat agent runtime via JSON-RPC.
 */

export { MeerkatClient } from "./client.js";
export { CapabilityChecker } from "./capabilities.js";
export { EventStream } from "./streaming.js";
export { SkillHelper } from "./skills.js";
export {
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
} from "./generated/errors.js";
export type {
  WireUsage,
  WireRunResult,
  WireEvent,
  CapabilitiesResponse,
  CapabilityEntry,
} from "./generated/types.js";
export { CONTRACT_VERSION } from "./generated/types.js";
