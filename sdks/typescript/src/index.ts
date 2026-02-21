/**
 * Meerkat TypeScript SDK — communicate with the Meerkat agent runtime.
 *
 * @example
 * ```ts
 * import { MeerkatClient } from "@rkat/sdk";
 *
 * const client = new MeerkatClient();
 * await client.connect();
 *
 * const session = await client.createSession("Hello!");
 * console.log(session.text);
 *
 * for await (const event of session.stream("Tell me a joke")) {
 *   if (event.type === "text_delta") {
 *     process.stdout.write(event.delta);
 *   }
 * }
 *
 * await client.close();
 * ```
 */

// Core client and session
export { MeerkatClient } from "./client.js";
export type { ConnectOptions } from "./client.js";
export { Session } from "./session.js";
export { EventStream } from "./streaming.js";

// Domain types (clean, Wire-free public names)
export type {
  Capability,
  RunResult,
  SchemaWarning,
  SessionInfo,
  SessionOptions,
  SkillKey,
  SkillRef,
  Usage,
} from "./types.js";

// Error hierarchy
export {
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
} from "./generated/errors.js";

// Contract version
export { CONTRACT_VERSION } from "./generated/types.js";

// Typed events — discriminated union and all interfaces
export type {
  AgentEvent,
  RunStartedEvent,
  RunCompletedEvent,
  RunFailedEvent,
  TurnStartedEvent,
  TextDeltaEvent,
  TextCompleteEvent,
  ToolCallRequestedEvent,
  ToolResultReceivedEvent,
  TurnCompletedEvent,
  ToolExecutionStartedEvent,
  ToolExecutionCompletedEvent,
  ToolExecutionTimedOutEvent,
  CompactionStartedEvent,
  CompactionCompletedEvent,
  CompactionFailedEvent,
  BudgetWarningEvent,
  RetryingEvent,
  HookStartedEvent,
  HookCompletedEvent,
  HookFailedEvent,
  HookDeniedEvent,
  HookRewriteAppliedEvent,
  HookPatchPublishedEvent,
  SkillsResolvedEvent,
  SkillResolutionFailedEvent,
  InteractionCompleteEvent,
  InteractionFailedEvent,
  StreamTruncatedEvent,
  UnknownEvent,
  StopReason,
  BudgetType,
  HookPoint,
} from "./events.js";

// Event utilities
export {
  parseEvent,
  isTextDelta,
  isTextComplete,
  isTurnCompleted,
  isToolCallRequested,
  isRunCompleted,
  isRunFailed,
} from "./events.js";
