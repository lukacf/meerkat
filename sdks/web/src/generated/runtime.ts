// Generated runtime bootstrap contracts for @rkat/web
// Source: tools/sdk-codegen/generate.py (generate_web_runtime_types)

/** Configuration for runtime initialization. */
export interface RuntimeConfig {
  /** Anthropic API key. */
  anthropicApiKey?: string;
  /** OpenAI API key. */
  openaiApiKey?: string;
  /** Gemini API key. */
  geminiApiKey?: string;
  /** Default model for new sessions. */
  model?: string;
  /** Maximum concurrent sessions. Default: 100000 (matches the WASM runtime
   * `default_max_sessions` / `MAX_SESSIONS`). */
  maxSessions?: number;
  /** Anthropic base URL (e.g. for proxy deployments). */
  anthropicBaseUrl?: string;
  /** OpenAI base URL. */
  openaiBaseUrl?: string;
  /** Gemini base URL. */
  geminiBaseUrl?: string;
}

/** Result from runtime initialization. */
export interface InitResult {
  status: 'initialized';
  model: string;
  providers: string[];
  max_sessions?: number;
}

/** Structural reference to a realm binding. */
export interface AuthBindingRef {
  realm: string;
  binding: string;
  profile?: string;
}

/** Configuration for creating a direct (non-mob) session.
 *
 * Per-session api_key / base_url fields are deleted. Credentials flow from
 * bootstrap-populated realm config (`initRuntimeFromConfig`) or the host's
 * registered external-auth resolver (`register_external_auth_resolver`).
 */
export interface SessionConfig {
  /** LLM model identifier. */
  model: string;
  /** Optional structural auth binding reference. */
  authBinding?: AuthBindingRef;
  /** System prompt. */
  systemPrompt?: string;
  /** Max tokens per response. Default: 4096. */
  maxTokens?: number;
  /** Enable comms for this session. */
  commsName?: string;
  /** Whether this session runs in keep-alive mode. */
  keepAlive?: boolean;
  /** Application-defined labels. */
  labels?: Record<string, string>;
  /** Additional instruction sections appended to the system prompt. */
  additionalInstructions?: string[];
  /** Opaque application context. */
  appContext?: unknown;
}

/** Fail-closed parse guard for WASM `init_runtime*` output (K19/K21).
 *
 * Throws on anything that is not the `InitResult` contract — malformed init
 * output must never be blind-cast into a success shape.
 */
export function parseInitResult(json: string): InitResult {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch (err) {
    throw new Error(`invalid InitResult: not JSON: ${String(err)}`);
  }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`invalid InitResult: expected object, got: ${json}`);
  }
  const record = value as Record<string, unknown>;
  if (record.status !== 'initialized') {
    throw new Error(`invalid InitResult: status must be 'initialized': ${json}`);
  }
  if (typeof record.model !== 'string') {
    throw new Error(`invalid InitResult: model must be a string: ${json}`);
  }
  if (
    !Array.isArray(record.providers) ||
    !record.providers.every((provider) => typeof provider === 'string')
  ) {
    throw new Error(`invalid InitResult: providers must be a string array: ${json}`);
  }
  if (record.max_sessions !== undefined && typeof record.max_sessions !== 'number') {
    throw new Error(`invalid InitResult: max_sessions must be a number: ${json}`);
  }
  return {
    status: 'initialized',
    model: record.model,
    providers: record.providers as string[],
    max_sessions: record.max_sessions as number | undefined,
  };
}
