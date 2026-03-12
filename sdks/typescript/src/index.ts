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

export { MeerkatClient } from "./client.js";
export type { ConnectOptions } from "./client.js";
export { DeferredSession, Session } from "./session.js";
export type { DeferredTurnOptions } from "./session.js";
export { Mob } from "./mob.js";
export { EventStream } from "./streaming.js";
export { EventSubscription } from "./subscription.js";

export type {
  AgentEventEnvelope,
  AttributedEvent,
  AttributedMobEvent,
  Capability,
  EventEnvelope,
  MobCreateOptions,
  MobDefinition,
  MobFlowStatus,
  MobLifecycleAction,
  MobMember,
  MobStatus,
  MobSummary,
  RunResult,
  SchemaWarning,
  SessionAssistantBlock,
  SessionHistory,
  SessionInfo,
  SessionMessage,
  SessionOptions,
  SessionToolCall,
  SessionToolResult,
  SkillQuarantineDiagnostic,
  SkillRuntimeDiagnostics,
  SkillKey,
  SkillRef,
  SourceHealthSnapshot,
  SpawnSpec,
  Usage,
} from "./types.js";

export {
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
} from "./generated/errors.js";

export { CONTRACT_VERSION } from "./generated/types.js";
export type {
  McpAddParams,
  McpRemoveParams,
  McpReloadParams,
  McpLiveOpResponse,
} from "./generated/types.js";

export type {
  AgentEvent,
  CoreAgentEvent,
  StreamEvent,
  ScopedAgentEvent,
  StreamScopeFrame,
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
  ToolConfigChangedEvent,
  ToolConfigChangedPayload,
  ToolConfigChangeOperation,
  UnknownEvent,
  StopReason,
  BudgetType,
  HookPoint,
} from "./events.js";

export {
  parseEvent,
  parseCoreEvent,
  isTextDelta,
  isTextComplete,
  isTurnCompleted,
  isToolCallRequested,
  isRunCompleted,
  isRunFailed,
} from "./events.js";
