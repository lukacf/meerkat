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

/** A runtime capability and its availability status. */
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
  enableSubagents?: boolean;
  enableMemory?: boolean;
  hostMode?: boolean;
  commsName?: string;
  peerMeta?: Record<string, unknown>;
  providerParams?: Record<string, unknown>;
  preloadSkills?: string[];
  skillRefs?: SkillRef[];
  skillReferences?: string[];
}
