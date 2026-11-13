/**
 * Operator instructions: standing System-plane instructions a host sets by
 * slot (a namespace it owns and a key), revision and body, for one session or
 * for the members of a mob.
 *
 * The write and status results keep the wire shape (snake_case fields) and
 * are checked on read. Dispositions, effects, entry kinds and state kinds are
 * closed: a value this SDK does not know fails the read (`INVALID_RESPONSE`).
 * The advisory values a status read carries (why a change is still pending,
 * why a delivery is blocked, where a member's session lives, how a model
 * lowers entries) may grow: a value this SDK does not know reads as
 * `{ unknown: <the value> }`, so a newer server never fails the read.
 */

import { MeerkatError } from "./generated/errors.js";
import type {
  InstructionEffect,
  InstructionEntryKind,
  InstructionSetErrorCode,
  WireInstructionBlockReason,
  WireInstructionLowering,
  WireInstructionPendingReason,
  WireInstructionResidency,
  WireMobInstructionDisposition,
  WireSessionInstructionDisposition,
} from "./generated/types.js";

/** An advisory value this SDK does not know, as the server sent it. */
export interface InstructionAdvisoryUnknown {
  unknown: string;
}

export type InstructionPendingReason = WireInstructionPendingReason | InstructionAdvisoryUnknown;
export type InstructionBlockReason = WireInstructionBlockReason | InstructionAdvisoryUnknown;
export type InstructionResidency = WireInstructionResidency | InstructionAdvisoryUnknown;
export type InstructionLowering = WireInstructionLowering | InstructionAdvisoryUnknown;

/**
 * Which members of a mob an operator instruction targets: every member, the
 * members of one profile, or one member. A target is a predicate over the
 * roster: a profile or member that does not exist yet selects nobody until
 * it does.
 *
 * A member target may carry `include_forks` (false when absent): the record
 * then follows the member's fork family from its root. Written for a member
 * that is not itself a fork it also selects every fork below it; written
 * for a fork it selects that member only. A status read carries the member
 * only when it is true.
 */
export type InstructionTarget =
  | { kind: "mob" }
  | { kind: "profile"; profile: string }
  | { kind: "member"; agent_identity: string; include_forks?: boolean };

/** Options of an operator-instruction write. */
export interface InstructionWriteOptions {
  /** The body's digest (`sha256:` and 64 lowercase hex digits), checked against it. */
  contentSha256?: string;
}

/** Filters and page of `mob/instruction_status`. */
export interface MobInstructionStatusOptions {
  namespace?: string;
  key?: string;
  agentIdentity?: string;
  profile?: string;
  /** Members to skip, in identity order. */
  offset?: number;
  /** 1 to 200 members; absent reads 100. */
  limit?: number;
}

/** The result of `mob/set_instruction` and `mob/revoke_instruction`. */
export interface MobInstructionWriteResult {
  mob_id: string;
  namespace: string;
  key: string;
  revision: number;
  disposition: WireMobInstructionDisposition;
  /** The mob's operator-instruction generation after the write. */
  generation: number;
  /** How many current members the record's target selects. */
  matched_members: number;
}

/** The result of `session/set_instruction` and `session/revoke_instruction`. */
export interface SessionInstructionWriteResult {
  session_id: string;
  namespace: string;
  key: string;
  revision: number;
  disposition: WireSessionInstructionDisposition;
  target_generation: number;
}

/** A slot's revision and effect. */
export interface InstructionSlotView {
  revision: number;
  effect: InstructionEffect;
}

/** The committed entry that put a slot's state in effect. */
export interface InstructionEffectiveSlot {
  revision: number;
  effect: InstructionEffect;
  entry_sequence: number;
  kind: InstructionEntryKind;
  run_id?: string;
  message_index: number;
}

export type InstructionSlotState =
  | { state: "effective" }
  | { state: "pending"; reason: InstructionPendingReason }
  | { state: "blocked"; reason: InstructionBlockReason }
  | { state: "not_targeted" };

/** One desired record of a mob. */
export interface InstructionRecord {
  namespace: string;
  key: string;
  revision: number;
  effect: InstructionEffect;
  target: InstructionTarget;
  content_sha256?: string;
  body_bytes?: number;
}

/** One slot of one mob member. */
export interface MemberInstructionSlot {
  namespace: string;
  key: string;
  desired: InstructionSlotView;
  recorded?: InstructionSlotView;
  effective?: InstructionEffectiveSlot;
  state: InstructionSlotState;
}

export type MemberInstructionDeliveryState =
  | { state: "current" }
  | { state: "owed"; generation: number; last_failure?: string }
  | { state: "blocked"; reason: InstructionBlockReason };

export interface MemberInstructionDelivery {
  recorded_generation?: number;
  state: MemberInstructionDeliveryState;
}

export type InstructionEffectiveRead =
  | { status: "ok" }
  | { status: "unreadable"; reason: string };

/** One member's operator-instruction status. */
export interface MemberInstructionStatus {
  agent_identity: string;
  profile: string;
  residency: InstructionResidency;
  delivery: MemberInstructionDelivery;
  slots: MemberInstructionSlot[];
  effective_read: InstructionEffectiveRead;
  /** Operator instructions apply to the member's turns, not to a live channel. */
  live_channel_open: boolean;
}

/** The result of `mob/instruction_status`. */
export interface MobInstructionStatus {
  mob_id: string;
  generation: number;
  desired: InstructionRecord[];
  members: MemberInstructionStatus[];
  next_offset?: number;
}

export type InstructionTargetOwner =
  | { kind: "unowned" }
  | { kind: "session" }
  | { kind: "mob"; mob_id: string; agent_identity: string };

/** One slot of a session. */
export interface SessionInstructionSlot {
  namespace: string;
  key: string;
  recorded?: InstructionSlotView;
  effective?: InstructionEffectiveSlot;
  state: InstructionSlotState;
}

/** The result of `session/instruction_status`. */
export interface SessionInstructionStatus {
  session_id: string;
  owner: InstructionTargetOwner;
  target_generation: number;
  entry_sequence: number;
  lowering?: InstructionLowering;
  slots: SessionInstructionSlot[];
}

// Each closed and advisory vocabulary, checked against the generated union
// at compile time: a value missing here, or one the union does not have,
// fails the build.
const EFFECTS = { absent: true, active: true } as const satisfies Record<InstructionEffect, true>;
const ENTRY_KINDS = { delta: true, snapshot: true } as const satisfies Record<
  InstructionEntryKind,
  true
>;
const MOB_DISPOSITIONS = { committed: true, duplicate: true } as const satisfies Record<
  WireMobInstructionDisposition,
  true
>;
const SESSION_DISPOSITIONS = { recorded: true, duplicate: true } as const satisfies Record<
  WireSessionInstructionDisposition,
  true
>;
const PENDING_REASONS = {
  awaiting_run_start: true,
  leading_prefix_only_lowering: true,
  lowering_unavailable: true,
  session_unborn: true,
  delivery_owed: true,
} as const satisfies Record<WireInstructionPendingReason, true>;
const BLOCK_REASONS = {
  member_host_unsupported: true,
  slot_managed_by_activation: true,
  durability_unavailable: true,
  owned_by_session: true,
  owned_by_other_mob: true,
} as const satisfies Record<WireInstructionBlockReason, true>;
const RESIDENCIES = { live: true, dormant: true, unborn: true } as const satisfies Record<
  WireInstructionResidency,
  true
>;
const LOWERINGS = {
  mid_conversation: true,
  leading_prefix_only: true,
  unavailable: true,
} as const satisfies Record<WireInstructionLowering, true>;

/**
 * The refusal codes, keyed so the compiler holds the list to the generated
 * union: a code the server adds fails the build until it is named here.
 */
const SET_ERROR_CODES = {
  invalid_request: true,
  digest_mismatch: true,
  stale_revision: true,
  revision_conflict: true,
  unknown_slot: true,
  slot_limit_exceeded: true,
  body_too_large: true,
  active_set_budget_exceeded: true,
  owned_by_mob: true,
  owned_by_session: true,
  owned_by_other_mob: true,
  slot_managed_by_activation: true,
  durability_unavailable: true,
  mob_terminal: true,
  malformed_instruction_history: true,
  prepared_authorization_required: true,
} as const satisfies Record<InstructionSetErrorCode, true>;

/** Every operator-instruction refusal code, as the server's closed vocabulary. */
export const INSTRUCTION_SET_ERROR_CODES: readonly InstructionSetErrorCode[] = Object.keys(
  SET_ERROR_CODES,
) as InstructionSetErrorCode[];

/**
 * The closed code of a refused operator-instruction write or read (the
 * operator-instruction methods carry it in `error.data.instruction_set_code`,
 * which the SDK keeps as `details`). Branch on it, never on the error text.
 * `undefined` for any other error.
 */
export function instructionSetCode(error: unknown): InstructionSetErrorCode | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const details = (error as { details?: unknown }).details;
  if (typeof details !== "object" || details === null) return undefined;
  const code = (details as { instruction_set_code?: unknown }).instruction_set_code;
  return typeof code === "string" &&
    (INSTRUCTION_SET_ERROR_CODES as readonly string[]).includes(code)
    ? (code as InstructionSetErrorCode)
    : undefined;
}

function invalid(context: string, message: string): MeerkatError {
  return new MeerkatError("INVALID_RESPONSE", `${context}: ${message}`);
}

function record(value: unknown, context: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalid(context, "expected an object");
  }
  return value as Record<string, unknown>;
}

function str(data: Record<string, unknown>, key: string, context: string): string {
  const value = data[key];
  if (typeof value !== "string") throw invalid(context, `${key} must be a string`);
  return value;
}

function count(data: Record<string, unknown>, key: string, context: string): number {
  const value = data[key];
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw invalid(context, `${key} must be a non-negative integer`);
  }
  return value;
}

function list(data: Record<string, unknown>, key: string, context: string): unknown[] {
  const value = data[key];
  if (!Array.isArray(value)) throw invalid(context, `${key} must be a list`);
  return value;
}

function closed<T extends string>(
  value: unknown,
  known: Record<T, true>,
  field: string,
  context: string,
): T {
  if (typeof value !== "string" || !Object.prototype.hasOwnProperty.call(known, value)) {
    throw invalid(context, `unsupported ${field} ${JSON.stringify(value)}`);
  }
  return value as T;
}

function tolerant<T extends string>(
  value: unknown,
  known: Record<T, true>,
  field: string,
  context: string,
): T | InstructionAdvisoryUnknown {
  if (typeof value !== "string") throw invalid(context, `${field} must be a string`);
  return Object.prototype.hasOwnProperty.call(known, value)
    ? (value as T)
    : { unknown: value };
}

function slotView(value: unknown, context: string): InstructionSlotView {
  const data = record(value, context);
  return {
    revision: count(data, "revision", context),
    effect: closed(data.effect, EFFECTS, "effect", context),
  };
}

function effectiveSlot(value: unknown, context: string): InstructionEffectiveSlot {
  const data = record(value, context);
  const slot: InstructionEffectiveSlot = {
    revision: count(data, "revision", context),
    effect: closed(data.effect, EFFECTS, "effect", context),
    entry_sequence: count(data, "entry_sequence", context),
    kind: closed(data.kind, ENTRY_KINDS, "kind", context),
    message_index: count(data, "message_index", context),
  };
  if (data.run_id != null) slot.run_id = str(data, "run_id", context);
  return slot;
}

function slotState(value: unknown, context: string): InstructionSlotState {
  const data = record(value, context);
  switch (data.state) {
    case "effective":
      return { state: "effective" };
    case "not_targeted":
      return { state: "not_targeted" };
    case "pending":
      return {
        state: "pending",
        reason: tolerant(data.reason, PENDING_REASONS, "pending reason", context),
      };
    case "blocked":
      return {
        state: "blocked",
        reason: tolerant(data.reason, BLOCK_REASONS, "block reason", context),
      };
    default:
      throw invalid(context, `unsupported slot state ${JSON.stringify(data.state)}`);
  }
}

function optionalViews(
  data: Record<string, unknown>,
  out: { recorded?: InstructionSlotView; effective?: InstructionEffectiveSlot },
  context: string,
): void {
  if (data.recorded != null) out.recorded = slotView(data.recorded, `${context}: recorded`);
  if (data.effective != null) {
    out.effective = effectiveSlot(data.effective, `${context}: effective`);
  }
}

function target(value: unknown, context: string): InstructionTarget {
  const data = record(value, context);
  switch (data.kind) {
    case "mob":
      return { kind: "mob" };
    case "profile":
      return { kind: "profile", profile: str(data, "profile", context) };
    case "member": {
      const agentIdentity = str(data, "agent_identity", context);
      // The fork scope is a boolean, left out when false.
      const includeForks = data.include_forks ?? false;
      if (typeof includeForks !== "boolean") {
        throw invalid(context, "include_forks must be a boolean");
      }
      return includeForks
        ? { kind: "member", agent_identity: agentIdentity, include_forks: true }
        : { kind: "member", agent_identity: agentIdentity };
    }
    default:
      throw invalid(context, `unsupported target kind ${JSON.stringify(data.kind)}`);
  }
}

/** @internal Check a `mob/set_instruction` or `mob/revoke_instruction` result. */
export function parseMobInstructionWriteResult(
  value: unknown,
  context: string,
): MobInstructionWriteResult {
  const data = record(value, context);
  return {
    mob_id: str(data, "mob_id", context),
    namespace: str(data, "namespace", context),
    key: str(data, "key", context),
    revision: count(data, "revision", context),
    disposition: closed(data.disposition, MOB_DISPOSITIONS, "disposition", context),
    generation: count(data, "generation", context),
    matched_members: count(data, "matched_members", context),
  };
}

/** @internal Check a `session/set_instruction` or `session/revoke_instruction` result. */
export function parseSessionInstructionWriteResult(
  value: unknown,
  context: string,
): SessionInstructionWriteResult {
  const data = record(value, context);
  return {
    session_id: str(data, "session_id", context),
    namespace: str(data, "namespace", context),
    key: str(data, "key", context),
    revision: count(data, "revision", context),
    disposition: closed(data.disposition, SESSION_DISPOSITIONS, "disposition", context),
    target_generation: count(data, "target_generation", context),
  };
}

function instructionRecord(value: unknown, context: string): InstructionRecord {
  const data = record(value, context);
  const out: InstructionRecord = {
    namespace: str(data, "namespace", context),
    key: str(data, "key", context),
    revision: count(data, "revision", context),
    effect: closed(data.effect, EFFECTS, "effect", context),
    target: target(data.target, `${context}: target`),
  };
  if (data.content_sha256 != null) out.content_sha256 = str(data, "content_sha256", context);
  if (data.body_bytes != null) out.body_bytes = count(data, "body_bytes", context);
  return out;
}

function deliveryState(value: unknown, context: string): MemberInstructionDeliveryState {
  const data = record(value, context);
  switch (data.state) {
    case "current":
      return { state: "current" };
    case "owed": {
      const owed: Extract<MemberInstructionDeliveryState, { state: "owed" }> = {
        state: "owed",
        generation: count(data, "generation", context),
      };
      if (data.last_failure != null) owed.last_failure = str(data, "last_failure", context);
      return owed;
    }
    case "blocked":
      return {
        state: "blocked",
        reason: tolerant(data.reason, BLOCK_REASONS, "block reason", context),
      };
    default:
      throw invalid(context, `unsupported delivery state ${JSON.stringify(data.state)}`);
  }
}

function effectiveRead(value: unknown, context: string): InstructionEffectiveRead {
  const data = record(value, context);
  switch (data.status) {
    case "ok":
      return { status: "ok" };
    case "unreadable":
      return { status: "unreadable", reason: str(data, "reason", context) };
    default:
      throw invalid(context, `unsupported effective_read status ${JSON.stringify(data.status)}`);
  }
}

function member(value: unknown, context: string): MemberInstructionStatus {
  const data = record(value, context);
  const rawDelivery = record(data.delivery, `${context}: delivery`);
  const delivery: MemberInstructionDelivery = {
    state: deliveryState(rawDelivery.state, `${context}: delivery.state`),
  };
  if (rawDelivery.recorded_generation != null) {
    delivery.recorded_generation = count(
      rawDelivery,
      "recorded_generation",
      `${context}: delivery`,
    );
  }
  const slots = list(data, "slots", context).map((raw, index) => {
    const slotContext = `${context}: slots[${index}]`;
    const slotData = record(raw, slotContext);
    const slot: MemberInstructionSlot = {
      namespace: str(slotData, "namespace", slotContext),
      key: str(slotData, "key", slotContext),
      desired: slotView(slotData.desired, `${slotContext}: desired`),
      state: slotState(slotData.state, `${slotContext}: state`),
    };
    optionalViews(slotData, slot, slotContext);
    return slot;
  });
  if (typeof data.live_channel_open !== "boolean") {
    throw invalid(context, "live_channel_open must be a boolean");
  }
  return {
    agent_identity: str(data, "agent_identity", context),
    profile: str(data, "profile", context),
    residency: tolerant(data.residency, RESIDENCIES, "residency", context),
    delivery,
    slots,
    effective_read: effectiveRead(data.effective_read, `${context}: effective_read`),
    live_channel_open: data.live_channel_open,
  };
}

/** @internal Check a `mob/instruction_status` result. */
export function parseMobInstructionStatus(value: unknown, context: string): MobInstructionStatus {
  const data = record(value, context);
  const status: MobInstructionStatus = {
    mob_id: str(data, "mob_id", context),
    generation: count(data, "generation", context),
    desired: list(data, "desired", context).map((raw, index) =>
      instructionRecord(raw, `${context}: desired[${index}]`),
    ),
    members: list(data, "members", context).map((raw, index) =>
      member(raw, `${context}: members[${index}]`),
    ),
  };
  if (data.next_offset != null) status.next_offset = count(data, "next_offset", context);
  return status;
}

function owner(value: unknown, context: string): InstructionTargetOwner {
  const data = record(value, context);
  switch (data.kind) {
    case "unowned":
      return { kind: "unowned" };
    case "session":
      return { kind: "session" };
    case "mob":
      return {
        kind: "mob",
        mob_id: str(data, "mob_id", context),
        agent_identity: str(data, "agent_identity", context),
      };
    default:
      throw invalid(context, `unsupported owner kind ${JSON.stringify(data.kind)}`);
  }
}

/** @internal Check a `session/instruction_status` result. */
export function parseSessionInstructionStatus(
  value: unknown,
  context: string,
): SessionInstructionStatus {
  const data = record(value, context);
  const status: SessionInstructionStatus = {
    session_id: str(data, "session_id", context),
    owner: owner(data.owner, `${context}: owner`),
    target_generation: count(data, "target_generation", context),
    entry_sequence: count(data, "entry_sequence", context),
    slots: list(data, "slots", context).map((raw, index) => {
      const slotContext = `${context}: slots[${index}]`;
      const slotData = record(raw, slotContext);
      const slot: SessionInstructionSlot = {
        namespace: str(slotData, "namespace", slotContext),
        key: str(slotData, "key", slotContext),
        state: slotState(slotData.state, `${slotContext}: state`),
      };
      optionalViews(slotData, slot, slotContext);
      return slot;
    }),
  };
  if (data.lowering != null) {
    status.lowering = tolerant(data.lowering, LOWERINGS, "lowering", context);
  }
  return status;
}
