/**
 * The provider-neutral `reasoning_effort` option: its level vocabulary, the
 * typed refusal a request for an unsupported level gets, and the report of
 * what a run sent.
 */

/** The reasoning effort levels, lowest first. `xhigh` has one spelling. */
export const REASONING_EFFORT_LEVELS = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
] as const;

/** A provider-neutral reasoning effort level. */
export type ReasoningEffortLevel = (typeof REASONING_EFFORT_LEVELS)[number];

/**
 * `error.data.kind` of the refusal a request for a level the model or
 * route does not accept gets (see `reasoningEffortRefusal`).
 */
export const REASONING_EFFORT_REFUSED_KIND = 'reasoning_effort_refused';

/**
 * Why a level is refused. A closed vocabulary: a reason this SDK does not
 * know is not parsed (see `reasoningEffortRefusal`).
 */
export const REASONING_EFFORT_REFUSAL_REASONS = [
  'unsupported_level',
  'supported_levels_unknown',
  'supported_levels_unavailable',
  'conflicting_provider_effort',
  'conflicting_thinking_budget',
  'conflicting_thinking_type',
  'conflicting_reasoning_body',
  'conflicting_reasoning_mode',
] as const;

/** Why a level is refused. */
export type ReasoningEffortRefusalReason = (typeof REASONING_EFFORT_REFUSAL_REASONS)[number];

/**
 * The route that serves a request: the backend (`openai_api`, `copilot`,
 * `bedrock`, `self_hosted`, ...) and the provider API (`responses`,
 * `chat_completions`, `messages`, `generate_content`, `realtime`). Both are
 * names, not addresses, and both vocabularies may grow: a value this SDK
 * does not know is kept as sent.
 */
export interface ReasoningRoute {
  readonly backend: string;
  readonly api: string;
}

/**
 * A refused `reasoning_effort` level. The request was refused before any
 * provider request: Meerkat lowered nothing, changed no model and moved to
 * no other route.
 */
export interface ReasoningEffortRefusal {
  /** The level the request named. */
  readonly requested: ReasoningEffortLevel;
  /** The provider family of the model. */
  readonly provider: string;
  /** The model the request would use. */
  readonly model: string;
  /** The route the request would be served on; `null` when none is named. */
  readonly route: ReasoningRoute | null;
  /** Why the level is refused. */
  readonly reason: ReasoningEffortRefusalReason;
  /**
   * The levels the route accepts for the model, lowest first: `null` when
   * no source states them or the source cannot be read now, and empty when
   * the route or the model takes no level.
   */
  readonly supported: readonly ReasoningEffortLevel[] | null;
  /**
   * The source of `supported` (`catalog`, `account`, `operator`; the
   * vocabulary may grow), `null` exactly when `supported` is.
   */
  readonly facts: string | null;
  /** The setting the level conflicts with, for the `conflicting_*` reasons. */
  readonly conflictingParam?: string;
  /** That setting's own level, when it is one. */
  readonly conflictingLevel?: ReasoningEffortLevel;
  /**
   * Whether the same request can succeed later with no configuration
   * change. True only for `supported_levels_unavailable` (the account's
   * model list is not loaded yet); every other reason is permanent until
   * the request or the configuration changes. Read from `reason`.
   */
  readonly transient: boolean;
  /**
   * The server's own mark of the transient class (`error.data.retryable`),
   * when it sent one. It agrees with `transient`.
   */
  readonly retryable?: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isLevel(value: unknown): value is ReasoningEffortLevel {
  return typeof value === 'string' && (REASONING_EFFORT_LEVELS as readonly string[]).includes(value);
}

function isReason(value: unknown): value is ReasoningEffortRefusalReason {
  return (
    typeof value === 'string' &&
    (REASONING_EFFORT_REFUSAL_REASONS as readonly string[]).includes(value)
  );
}

/** A route object, `null` for an explicit or absent `null`, else `undefined`. */
function parseRoute(value: unknown): ReasoningRoute | null | undefined {
  if (value === null || value === undefined) return null;
  if (!isRecord(value)) return undefined;
  if (typeof value.backend !== 'string' || typeof value.api !== 'string') return undefined;
  return { backend: value.backend, api: value.api };
}

/**
 * Parse a refusal object (the wire `reasoning_effort_refusal`). `undefined`
 * when it does not have the documented shape, a reason or level this SDK
 * does not know included.
 */
export function parseReasoningEffortRefusal(
  value: unknown,
  retryable?: unknown,
): ReasoningEffortRefusal | undefined {
  if (!isRecord(value)) return undefined;
  const { requested, provider, model, reason, supported, facts } = value;
  const route = parseRoute(value.route);
  if (
    !isLevel(requested) ||
    typeof provider !== 'string' ||
    typeof model !== 'string' ||
    route === undefined ||
    !isReason(reason)
  ) {
    return undefined;
  }
  let levels: ReasoningEffortLevel[] | null = null;
  if (supported !== null && supported !== undefined) {
    if (!Array.isArray(supported) || !supported.every(isLevel)) return undefined;
    levels = [...supported];
  }
  if (facts !== null && facts !== undefined && typeof facts !== 'string') return undefined;
  const conflictingParam = value.conflicting_param;
  const conflictingLevel = value.conflicting_level;
  if (conflictingParam !== undefined && conflictingParam !== null && typeof conflictingParam !== 'string') {
    return undefined;
  }
  if (conflictingLevel !== undefined && conflictingLevel !== null && !isLevel(conflictingLevel)) {
    return undefined;
  }
  return {
    requested,
    provider,
    model,
    route,
    reason,
    supported: levels,
    facts: typeof facts === 'string' ? facts : null,
    ...(typeof conflictingParam === 'string' ? { conflictingParam } : {}),
    ...(isLevel(conflictingLevel) ? { conflictingLevel } : {}),
    transient: reason === 'supported_levels_unavailable',
    ...(typeof retryable === 'boolean' ? { retryable } : {}),
  };
}

/**
 * The typed refusal of a request whose `reasoning_effort` level the model
 * or route does not accept.
 *
 * Such a request is rejected with `INVALID_PARAMS` and the data
 * `{kind: 'reasoning_effort_refused', retryable, reasoning_effort_refusal}`,
 * which the SDK keeps as `data`. Branch on it, never on the error text.
 * `undefined` for any other error, or when the data does not parse (a
 * reason this SDK does not know included): `error.data` then still holds
 * the data as sent.
 */
export function reasoningEffortRefusal(error: unknown): ReasoningEffortRefusal | undefined {
  if (typeof error !== 'object' || error === null) return undefined;
  const details = (error as { data?: unknown }).data;
  if (!isRecord(details) || details.kind !== REASONING_EFFORT_REFUSED_KIND) return undefined;
  return parseReasoningEffortRefusal(details.reasoning_effort_refusal, details.retryable);
}

/**
 * One distinct resolution a run sent: the level, the model and provider it
 * went to, the route, the provider fields written (`fields`), the source of
 * the route's levels (`facts`), who decided the level (`decidedBy`:
 * `request`, or `preference` on a delegated live turn) and how many
 * provider requests carried it. `fields`, `facts`, `decidedBy` and the
 * route's names are advisory: a value this SDK does not know is kept as
 * sent.
 */
export interface ReasoningEffortSent {
  readonly level: ReasoningEffortLevel | string;
  readonly provider: string;
  readonly model: string;
  readonly route: ReasoningRoute | null;
  readonly fields: readonly string[];
  readonly facts: string | null;
  readonly decidedBy: string | null;
  readonly requests: number;
}

/**
 * What a run did with the session's level: the level requested, and each
 * distinct resolution the run sent, in first-use order (two rows after a
 * model fallback switch).
 */
export interface ReasoningEffortReport {
  readonly requested: ReasoningEffortLevel | string;
  readonly sent: readonly ReasoningEffortSent[];
}

/**
 * The resolution of one provider request, or of a run's start: the same
 * members as a `sent` row, with the level that was requested and no count.
 */
export interface ReasoningEffortResolution {
  readonly requested: ReasoningEffortLevel | string;
  readonly level: ReasoningEffortLevel | string;
  readonly provider: string;
  readonly model: string;
  readonly route: ReasoningRoute | null;
  readonly fields: readonly string[];
  readonly facts: string | null;
  readonly decidedBy: string | null;
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}

function stringOrNull(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}

function parseSent(value: unknown): ReasoningEffortSent | undefined {
  if (!isRecord(value)) return undefined;
  const { level, provider, model, requests } = value;
  if (typeof level !== 'string' || typeof provider !== 'string' || typeof model !== 'string') {
    return undefined;
  }
  return {
    level,
    provider,
    model,
    route: parseRoute(value.route) ?? null,
    fields: stringArray(value.fields),
    facts: stringOrNull(value.facts),
    decidedBy: stringOrNull(value.decided_by ?? value.decidedBy),
    requests: typeof requests === 'number' ? requests : 0,
  };
}

/**
 * Parse the `reasoning_effort` block of a run result. `undefined` when the
 * run carried no level (the block is absent) or the block has no readable
 * `requested` level. Advisory values this SDK does not know are kept.
 */
export function parseReasoningEffortReport(value: unknown): ReasoningEffortReport | undefined {
  if (!isRecord(value) || typeof value.requested !== 'string') return undefined;
  const sent = Array.isArray(value.sent)
    ? value.sent.map(parseSent).filter((row): row is ReasoningEffortSent => row !== undefined)
    : [];
  return { requested: value.requested, sent };
}

/**
 * Parse a resolution: the `reasoning_effort` member of `run_started` and
 * `run_resumed`, and the `resolution` member of
 * `request_reasoning_effort_resolved`. `undefined` when absent or unreadable.
 */
export function parseReasoningEffortResolution(value: unknown): ReasoningEffortResolution | undefined {
  if (!isRecord(value)) return undefined;
  const { requested, level, provider, model } = value;
  if (
    typeof requested !== 'string' ||
    typeof level !== 'string' ||
    typeof provider !== 'string' ||
    typeof model !== 'string'
  ) {
    return undefined;
  }
  return {
    requested,
    level,
    provider,
    model,
    route: parseRoute(value.route) ?? null,
    fields: stringArray(value.fields),
    facts: stringOrNull(value.facts),
    decidedBy: stringOrNull(value.decided_by ?? value.decidedBy),
  };
}

/**
 * The report of what a run sent for the session's level, from a run result
 * (the SDK's result object or the wire result). `undefined` when the run
 * carried no level.
 */
export function reasoningEffortReport(result: unknown): ReasoningEffortReport | undefined {
  if (!isRecord(result)) return undefined;
  return parseReasoningEffortReport(result.reasoningEffort ?? result.reasoning_effort);
}
