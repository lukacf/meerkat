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

/** Present to whoever acts for the decision's approver id. */
export interface DecisionApproverAudience {
  readonly kind: "approver";
  readonly approverId: string;
}

/** Present to the decision's named reviewer (a principal key). */
export interface DecisionReviewerAudience {
  readonly kind: "reviewer";
  readonly reviewer: string;
}

export type DecisionAudience = DecisionApproverAudience | DecisionReviewerAudience;

/** One canonical rendering part; the gateway validates its digest and coverage. */
export interface DecisionCanonicalPart {
  readonly index: number;
  readonly count: number;
  readonly partDigest: string;
}

/** A summary's link to the full rendering and the digest it served. */
export interface DecisionPresentationLink {
  readonly reference: string;
  readonly servedDigest: string;
}

/** Typed input for both present and an attestation's nested presentation. */
export interface DecisionPresentationOptions {
  readonly audience: DecisionAudience;
  readonly subjectDigest: string;
  readonly partsShown: readonly DecisionCanonicalPart[];
  readonly channelRef: string;
  readonly messageRef: string;
  /** UNIX ms, limited to Number.MAX_SAFE_INTEGER for exact JSON numbers. */
  readonly atMs: number;
  readonly maxPartBytes?: number | null;
  readonly link?: DecisionPresentationLink | null;
}

function presentationRecord(
  value: unknown, name: string, fields: readonly string[],
): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${name} must be an object`);
  }
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => !fields.includes(key))) {
    throw new TypeError(`${name} contains an unknown field`);
  }
  return record;
}

function presentationString(value: unknown, name: string): string {
  if (typeof value !== "string") throw new TypeError(`${name} must be a string`);
  return value;
}

function presentationUint(value: unknown, name: string, maximum: number): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > maximum) {
    throw new TypeError(`${name} must be an unsigned integer no greater than ${maximum}`);
  }
  return value;
}

/**
 * What a host showed, as a typed request, not an authorization.
 *
 * Checks the closed request's structure and integer widths. The gateway still
 * validates digests, completeness, audience and attestation. atMs uses the
 * exactly representable safe-integer subset of Rust's u64. Null/undefined
 * optional fields are omitted. Gateway results remain raw objects.
 *
 * Existing raw Record inputs to the wrappers remain a separate compatibility
 * path and are sent without these local checks or camelCase conversion.
 */
export class DecisionPresentation {
  private readonly options: DecisionPresentationOptions;

  constructor(init: DecisionPresentationOptions) {
    const d = presentationRecord(init, "presentation", [
      "audience", "subjectDigest", "partsShown", "channelRef", "messageRef", "atMs",
      "maxPartBytes", "link",
    ]);
    const a = presentationRecord(d.audience, "audience", ["kind", "approverId", "reviewer"]);
    let audience: DecisionAudience;
    if (a.kind === "approver") {
      presentationRecord(a, "audience", ["kind", "approverId"]);
      audience = { kind: "approver", approverId: presentationString(a.approverId, "audience.approverId") };
    } else if (a.kind === "reviewer") {
      presentationRecord(a, "audience", ["kind", "reviewer"]);
      audience = { kind: "reviewer", reviewer: presentationString(a.reviewer, "audience.reviewer") };
    } else {
      throw new TypeError("audience.kind must be approver or reviewer");
    }
    if (!Array.isArray(d.partsShown)) throw new TypeError("partsShown must be an array");
    const parts = Array.from(d.partsShown, (value): DecisionCanonicalPart => {
      const part = presentationRecord(value, "part", ["index", "count", "partDigest"]);
      return Object.freeze({
        index: presentationUint(part.index, "part.index", 0xffff_ffff),
        count: presentationUint(part.count, "part.count", 0xffff_ffff),
        partDigest: presentationString(part.partDigest, "part.partDigest"),
      });
    });
    let link: DecisionPresentationLink | undefined;
    if (d.link != null) {
      const raw = presentationRecord(d.link, "link", ["reference", "servedDigest"]);
      link = Object.freeze({
        reference: presentationString(raw.reference, "link.reference"),
        servedDigest: presentationString(raw.servedDigest, "link.servedDigest"),
      });
    }
    this.options = Object.freeze({
      audience: Object.freeze(audience),
      subjectDigest: presentationString(d.subjectDigest, "subjectDigest"),
      partsShown: Object.freeze(parts),
      channelRef: presentationString(d.channelRef, "channelRef"),
      messageRef: presentationString(d.messageRef, "messageRef"),
      atMs: presentationUint(d.atMs, "atMs", Number.MAX_SAFE_INTEGER),
      maxPartBytes: d.maxPartBytes == null ? undefined
        : presentationUint(d.maxPartBytes, "maxPartBytes", 0xffff_ffff),
      link,
    });
  }

  /** A fresh canonical snake_case request object. */
  toDict(): Record<string, unknown> {
    const p = this.options;
    const wire: Record<string, unknown> = {
      audience: p.audience.kind === "approver"
        ? { kind: "approver", approver_id: p.audience.approverId }
        : { kind: "reviewer", reviewer: p.audience.reviewer },
      subject_digest: p.subjectDigest,
      parts_shown: p.partsShown.map((part) => ({
        index: part.index, count: part.count, part_digest: part.partDigest,
      })),
      channel_ref: p.channelRef,
      message_ref: p.messageRef,
      at_ms: p.atMs,
    };
    if (p.maxPartBytes != null) wire.max_part_bytes = p.maxPartBytes;
    if (p.link != null) wire.link = { reference: p.link.reference, served_digest: p.link.servedDigest };
    return wire;
  }
}

/** @internal Preserve raw compatibility; convert only explicitly typed input. */
export function decisionPresentationToDict(
  presentation: DecisionPresentation | Record<string, unknown>,
): Record<string, unknown> {
  return presentation instanceof DecisionPresentation ? presentation.toDict() : presentation;
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
  presentation?: DecisionPresentation | Record<string, unknown>;
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
 * ahead of the gateway's clock an attested event's time may be, at most one
 * hour (3_600_000).
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
      wire.presentation = decisionPresentationToDict(attestation.presentation);
    }
    params.attestation = wire;
  }
  return params;
}
