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
  readonly skillDiagnostics?: unknown;
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
