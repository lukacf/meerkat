/**
 * Public domain types for the Meerkat TypeScript SDK.
 *
 * These replace the `Wire*` prefixed generated types.  All fields use
 * idiomatic camelCase.
 */

import type { Usage } from "./events.js";

export type { Usage } from "./events.js";

/** Warning emitted when structured output doesn't match a provider's schema. */
export interface SchemaWarning {
  readonly provider: string;
  readonly path: string;
  readonly message: string;
}

/** Runtime health snapshot for skill source resolution. */
export interface SourceHealthSnapshot {
  readonly state: string;
  readonly invalidRatio: number;
  readonly invalidCount: number;
  readonly totalCount: number;
  readonly failureStreak: number;
  readonly handshakeFailed: boolean;
}

/** Diagnostic details for a single quarantined skill entry. */
export interface SkillQuarantineDiagnostic {
  readonly sourceUuid: string;
  readonly skillId: string;
  readonly location: string;
  readonly errorCode: string;
  readonly errorClass: string;
  readonly message: string;
  readonly firstSeenUnixSecs: number;
  readonly lastSeenUnixSecs: number;
}

/** Runtime diagnostics emitted by the Rust skill subsystem. */
export interface SkillRuntimeDiagnostics {
  readonly sourceHealth: SourceHealthSnapshot;
  readonly quarantined: readonly SkillQuarantineDiagnostic[];
}

/** Structured skill identifier (source UUID + skill name). */
export interface SkillKey {
  readonly sourceUuid: string;
  readonly skillName: string;
}

/** A skill reference — either a {@link SkillKey} or a legacy string. */
export type SkillRef = SkillKey | string;

/** A content block in a multimodal prompt. */
export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; media_type: string; data: string };

/** Ephemeral per-turn tool visibility overlay. */
export interface TurnToolOverlay {
  readonly allowedTools?: readonly string[];
  readonly blockedTools?: readonly string[];
}

/** Result of an agent session creation or turn. */
export interface RunResult {
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly text: string;
  readonly turns: number;
  readonly toolCalls: number;
  readonly usage: Usage;
  readonly structuredOutput?: unknown;
  readonly schemaWarnings?: readonly SchemaWarning[];
  readonly skillDiagnostics?: SkillRuntimeDiagnostics;
}

/** Summary of an active session. */
export interface SessionInfo {
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly messageCount: number;
  readonly totalTokens: number;
  readonly isActive: boolean;
}

export interface SessionToolCall {
  readonly id: string;
  readonly name: string;
  readonly args: unknown;
}

export interface SessionToolResult {
  readonly toolUseId: string;
  readonly content: string;
  readonly isError: boolean;
}

export interface SessionAssistantBlock {
  readonly blockType: string;
  readonly text?: string;
  readonly id?: string;
  readonly name?: string;
  readonly args?: unknown;
  readonly meta?: Record<string, unknown>;
}

export interface SessionMessage {
  readonly role: string;
  readonly content?: string;
  readonly toolCalls: readonly SessionToolCall[];
  readonly stopReason?: string;
  readonly blocks: readonly SessionAssistantBlock[];
  readonly results: readonly SessionToolResult[];
}

export interface SessionHistory {
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly messageCount: number;
  readonly offset: number;
  readonly limit?: number;
  readonly hasMore: boolean;
  readonly messages: readonly SessionMessage[];
}

/** A runtime capability and its availability status. */


export interface EventEnvelope<T = unknown> {
  readonly timestamp_ms: number;
  readonly source_id: string;
  readonly seq: number;
  readonly event_id: string;
  readonly payload: T;
}

export interface AttributedEvent {
  readonly source: string;
  readonly profile: string;
  readonly envelope: EventEnvelope;
}

export interface MobDefinition {
  readonly id: string;
  readonly profiles: Record<string, unknown>;
  readonly wiring?: Record<string, unknown>;
  readonly flows?: Record<string, unknown>;
  readonly mcp_servers?: Record<string, unknown>;
  readonly skills?: Record<string, unknown>;
  readonly orchestrator?: unknown;
  readonly backend?: unknown;
}

export interface SpawnSpec {
  readonly profile: string;
  readonly meerkatId: string;
  readonly initialMessage?: string | ContentBlock[];
  readonly runtimeMode?: "turn_driven" | "autonomous_host";
  readonly backend?: "session" | "external";
  readonly labels?: Record<string, string>;
  readonly context?: Record<string, unknown>;
  readonly resumeSessionId?: string;
  readonly additionalInstructions?: string[];
}

export interface MobMember {
  readonly meerkatId: string;
  readonly profile: string;
  readonly memberRef?: Record<string, unknown>;
  readonly peerId?: string;
  readonly externalPeerSpecs?: Readonly<Record<string, Record<string, unknown>>>;
  readonly runtimeMode?: string;
  readonly state?: string;
  readonly wiredTo?: readonly string[];
  readonly labels?: Record<string, string>;
  readonly status?: string;
  readonly error?: string;
  readonly isFinal?: boolean;
  readonly currentSessionId?: string;
  readonly sessionId?: string;
}

export interface MobSummary {
  readonly mobId: string;
  readonly status: string;
}

export interface MobStatus {
  readonly mobId: string;
  readonly status: string;
}

export type MobLifecycleAction = "stop" | "resume" | "complete" | "destroy" | "reset";

export interface MobFlowStatus {
  readonly run?: Record<string, unknown> | null;
}
export interface Capability {
  readonly id: string;
  readonly description: string;
  readonly status: string;
}

/** Options for creating a new session. */
export interface SessionOptions {
  model?: string;
  provider?: string;
  systemPrompt?: string;
  maxTokens?: number;
  outputSchema?: Record<string, unknown>;
  structuredOutputRetries?: number;
  hooksOverride?: Record<string, unknown>;
  enableBuiltins?: boolean;
  enableShell?: boolean;
  enableMemory?: boolean;
  enableMob?: boolean;
  hostMode?: boolean;
  commsName?: string;
  peerMeta?: Record<string, unknown>;
  budgetLimits?: Record<string, unknown>;
  providerParams?: Record<string, unknown>;
  preloadSkills?: string[];
  skillRefs?: SkillRef[];
  skillReferences?: string[];
}


/** Explicit standalone session-event envelope. */
export interface AgentEventEnvelope {
  readonly eventId: string;
  readonly sourceId: string;
  readonly seq: number;
  readonly timestampMs: number;
  readonly payload: import("./events.js").AgentEvent;
}

/** Mob-wide attributed event emitted by member/mob observation streams. */
export interface AttributedMobEvent {
  readonly source: string;
  readonly profile: string;
  readonly envelope: AgentEventEnvelope;
}

/** Options for creating a mob through the RPC-backed SDK surface. */
export interface MobCreateOptions {
  prefab?: string;
  definition?: Record<string, unknown>;
}
