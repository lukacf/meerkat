// Generated session façade contracts for @rkat/web
// Source: tools/sdk-codegen/generate.py (generate_web_session_types)
import type { SchemaWarning, SessionId, TurnTerminalCauseKind, Usage } from './events.js';

/**
 * Canonical run-result wire envelope (mirrors `meerkat_contracts::WireRunResult`,
 * the same shape RPC's `turn/start` returns and the WASM `start_turn` export
 * resolves with).
 */
export interface WireRunResult {
  session_id: SessionId;
  session_ref?: string | null;
  text: string;
  turns: number;
  tool_calls: number;
  usage: Usage;
  structured_output?: unknown;
  extraction_error?: { last_output: string; attempts: number; reason: string } | null;
  schema_warnings?: SchemaWarning[] | null;
  skill_diagnostics?: unknown;
  /**
   * Runtime-owned terminal cause for this turn (e.g. `budget_exhausted`).
   * Present only when the turn terminated on a typed terminal condition.
   */
  terminal_cause_kind?: TurnTerminalCauseKind | null;
}

/**
 * Runtime-backed state for a direct browser session façade. Mirrors the JSON
 * envelope produced by the WASM `get_session_state` export.
 */
export interface SessionState {
  handle: number;
  session_id: SessionId;
  mob_id: string;
  model: string;
  usage: Usage;
  message_count: number;
  is_active: boolean;
  last_assistant_text?: string | null;
}
