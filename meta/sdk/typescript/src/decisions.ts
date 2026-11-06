/**
 * Durable decisions on the stdio gateway (`meta/decisions/*`).
 *
 * The host reads, presents and withdraws the durable decisions of the
 * runtime's persistent state directory through the `MobHandle` wrappers
 * (`decisionsList`, `decisionsGet`, `decisionsRendering`, `decisionsPresent`,
 * `decisionsWithdraw`, `decisionsDecide`). A `decisionsDecide` without an
 * attestation is the stdio operator's; with a {@link DecisionAttestation}, a
 * decision ingress the launch declares (`runtime_options.decisions`) attests
 * the person who decided.
 *
 * `decisionsDecide` refuses while this SDK serves a host tool callback
 * ({@link DecideInsideToolCallbackError}): an agent's tool call never becomes
 * a decision.
 */
import { AsyncLocalStorage } from "node:async_hooks";

import { DecideInsideToolCallbackError } from "./errors.js";

const servingToolCallback = new AsyncLocalStorage<boolean>();

/** @internal Run `fn` as the handler of an agent's tool call. */
export function runAsToolCallback<T>(fn: () => Promise<T>): Promise<T> {
  return servingToolCallback.run(true, fn);
}

/** @internal Refuse a decide inside a host tool callback. */
export function refuseInsideToolCallback(): void {
  if (servingToolCallback.getStore() === true) {
    throw new DecideInsideToolCallbackError();
  }
}

/**
 * A host ingress's attestation of the person who decided: `ingress` names a
 * decision ingress the launch declares, `subject` the person it
 * authenticated, `scope` the authentication scope (for example the chat
 * session), `eventRef` the ingress's reference of the decide event (no other
 * decision may use it), and `observedAtMs` the time the ingress observed it.
 * `presentation` is what the ingress showed the person, in the
 * `meta/decisions/present` shape, when it showed the decision itself.
 */
export interface DecisionAttestation {
  ingress: string;
  subject: string;
  scope: string;
  eventRef: string;
  observedAtMs: number;
  presentation?: Record<string, unknown>;
}

/** The filters and page of a `decisionsList`. */
export interface DecisionListOptions {
  kind?: string;
  phase?: string;
  approverId?: string;
  requester?: string;
  after?: string;
  limit?: number;
}

/** A `decisionsDecide`. */
export interface DecisionDecideOptions {
  approve: boolean;
  presentedDigest: string;
  reason?: string;
  attestation?: DecisionAttestation;
}

/**
 * One decision ingress a launch declares (`runtime_options.decisions`): a
 * host channel that authenticates the people who decide through it.
 * `ingress` is 1 to 64 of `a-z`, `0-9`, `.`, `_` and `-`, never
 * `operator` (the built-in stdio operator); `maxClockSkewMs` is how far
 * ahead of the gateway's clock an attested event's time may be.
 */
export interface DecisionIngressDeclaration {
  ingress: string;
  attests: "person";
  maxClockSkewMs: number;
}

/** @internal The wire form of `runtime_options.decisions`. */
export function decisionIngressesOption(
  ingresses: readonly DecisionIngressDeclaration[],
): Record<string, unknown> {
  return {
    ingresses: ingresses.map((entry) => ({
      ingress: entry.ingress,
      attests: entry.attests,
      max_clock_skew_ms: entry.maxClockSkewMs,
    })),
  };
}

/** @internal The wire params of a `meta/decisions/list`. */
export function decisionListParams(
  options: DecisionListOptions = {},
): Record<string, unknown> {
  const params: Record<string, unknown> = {};
  if (options.kind !== undefined) params.kind = options.kind;
  if (options.phase !== undefined) params.phase = options.phase;
  if (options.approverId !== undefined) params.approver_id = options.approverId;
  if (options.requester !== undefined) params.requester = options.requester;
  if (options.after !== undefined) params.after = options.after;
  if (options.limit !== undefined) params.limit = options.limit;
  return params;
}

/** @internal The wire params of a `meta/decisions/decide`. */
export function decisionDecideParams(
  decisionId: string,
  options: DecisionDecideOptions,
): Record<string, unknown> {
  const params: Record<string, unknown> = {
    decision_id: decisionId,
    approve: options.approve,
    presented_digest: options.presentedDigest,
  };
  if (options.reason !== undefined) params.reason = options.reason;
  if (options.attestation !== undefined) {
    const attestation = options.attestation;
    const wire: Record<string, unknown> = {
      ingress: attestation.ingress,
      subject: attestation.subject,
      scope: attestation.scope,
      event_ref: attestation.eventRef,
      observed_at_ms: attestation.observedAtMs,
    };
    if (attestation.presentation !== undefined) {
      wire.presentation = attestation.presentation;
    }
    params.attestation = wire;
  }
  return params;
}
