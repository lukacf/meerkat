/**
 * Meerkat TypeScript SDK — communicate with the Meerkat agent runtime via JSON-RPC.
 */

export { MeerkatClient } from "./client";
export {
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
} from "./generated/errors";
export type {
  WireUsage,
  WireRunResult,
  WireEvent,
  CapabilitiesResponse,
  CapabilityEntry,
} from "./generated/types";
export { CONTRACT_VERSION } from "./generated/types";
