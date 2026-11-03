import type {
  ConversationCouncilArtifactClaimRow,
  ConversationCouncilEntry,
  ConversationCouncilExchangeRow,
  ConversationCouncilParticipantRow,
  CouncilCardStatus,
} from "@console-core";
import type { ConsoleFrame } from "../types";

/// meerkat exposes exactly ONE council tool. A council is not an evolving
/// aggregate like the workgraph: a single synchronous call seats
/// participants, runs bounded exchanges, merges and tears everything down
/// before returning, so one call maps to one card.
export const COUNCIL_TOOL_NAME = "council";

/// Frame events that can carry a council call or its result. Same set the
/// workgraph fold watches - a council call surfaces through the ordinary
/// tool-call lifecycle.
const COUNCIL_TOOL_EVENTS = new Set([
  "tool_call_requested",
  "tool_call",
  "tool_execution_started",
  "tool_result_received",
  "tool_execution_completed",
]);

/// Exit reasons that mean "ran correctly, stopped at a budget the caller
/// set". Kept separate from failure so the card's colour stays meaningful:
/// if a deliberate round cap rendered the same red as a seating failure,
/// operators would learn to ignore red.
const BOUNDED_EXIT_REASONS = new Set(["max_exchanges_reached", "deadline_exceeded"]);

/// Render cap. A council with a large round schedule can produce many
/// receipts; the header keeps reporting the true total and the card renders
/// an explicit overflow row, so the cap never reads as "that was all of
/// them".
export const COUNCIL_EXCHANGE_ROW_CAP = 12;

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function frameToolName(frame: ConsoleFrame): string | undefined {
  const record = asRecord(frame.data);
  if (!record) return undefined;
  return asString(record.name) || asString(record.tool_name);
}

/// Whether this frame belongs to a council tool call.
///
/// Used to SUPPRESS the generic tool row: without it a council renders twice,
/// once as a raw tool block and once as this card.
export function isCouncilToolFrame(frame: ConsoleFrame): boolean {
  if (!COUNCIL_TOOL_EVENTS.has(frame.event)) return false;
  return frameToolName(frame) === COUNCIL_TOOL_NAME;
}

/// Parse a frame's tool result payload, whether it arrived as a JSON string
/// or as an already-decoded object. Returns null rather than throwing: a
/// council whose result cannot be parsed must fall back to the ordinary tool
/// row, not blank the conversation.
function parseResultPayload(record: Record<string, unknown>): Record<string, unknown> | null {
  const raw = record.result;
  if (typeof raw === "string") {
    try {
      return asRecord(JSON.parse(raw));
    } catch {
      return null;
    }
  }
  return asRecord(raw);
}

function parseArgsPayload(record: Record<string, unknown>): Record<string, unknown> | null {
  if (typeof record.arguments === "string") {
    try {
      return asRecord(JSON.parse(record.arguments));
    } catch {
      return null;
    }
  }
  return asRecord(record.arguments) || asRecord(record.args);
}

/// Coarse card status from the sealed result's `exit_reason` tag.
///
/// `#[non_exhaustive]` upstream, so an UNKNOWN reason must not silently read
/// as success. Anything not explicitly recognised is treated as a failure and
/// the verbatim tag is rendered, which fails loudly in the direction that
/// gets looked at.
export function councilStatusFromExitReason(reason: string | undefined): CouncilCardStatus {
  if (!reason) return "pending";
  if (reason === "completed") return "completed";
  if (BOUNDED_EXIT_REASONS.has(reason)) return "bounded";
  return "failed";
}

/// Human-facing detail carried by the failing `exit_reason` variants. The
/// variants are internally tagged (`#[serde(tag = "reason")]`), so their
/// payload fields sit alongside the tag.
function exitDetailOf(exit: Record<string, unknown>): string | undefined {
  const detail = asString(exit.detail);
  const target = asString(exit.target_identity);
  const order = asNumber(exit.participant_order);
  const round = asNumber(exit.round);
  // `participant_fork_instruction_refused` carries a typed refusal: the cause
  // tag and the inherited instruction keys it concerns.
  const refusal = asRecord(exit.refusal);
  const refusalCause = asString(asRecord(refusal?.cause)?.kind);
  const refusalKeys = refusal && Array.isArray(refusal.keys) ? refusal.keys.length : 0;
  const parts: string[] = [];
  if (order !== undefined) parts.push(`slot ${order}`);
  if (round !== undefined) parts.push(`round ${round + 1}`);
  if (target) parts.push(target);
  if (refusalCause) {
    parts.push(
      `fork instruction refusal: ${refusalCause.replace(/_/g, " ")}` +
        (refusalKeys > 0 ? ` (${refusalKeys} key${refusalKeys === 1 ? "" : "s"})` : ""),
    );
  }
  if (detail) parts.push(detail);
  return parts.length > 0 ? parts.join(" · ") : undefined;
}

function participantRows(result: Record<string, unknown>): ConversationCouncilParticipantRow[] {
  const raw = Array.isArray(result.participants) ? result.participants : [];
  return raw.flatMap((value) => {
    const row = asRecord(value);
    if (!row) return [];
    const order = asNumber(row.order);
    if (order === undefined) return [];
    return [{
      order,
      role: asString(row.role) || "participant",
      sourceMobId: asString(row.source_mob_id) || "",
      sourceIdentity: asString(row.source_identity) || "",
      targetIdentity: asString(row.target_identity) || "",
      seated: row.seated === true,
    }];
  });
}

function exchangeRows(result: Record<string, unknown>): ConversationCouncilExchangeRow[] {
  const raw = Array.isArray(result.exchanges) ? result.exchanges : [];
  return raw.flatMap((value) => {
    const row = asRecord(value);
    if (!row) return [];
    const round = asNumber(row.round);
    const sequence = asNumber(row.sequence);
    const participantOrder = asNumber(row.participant_order);
    if (round === undefined || sequence === undefined) return [];
    const outcome = asRecord(row.outcome);
    // `#[serde(tag = "status")]`: pending | completed | failed. An absent or
    // unrecognised status is reported as pending rather than assumed
    // complete - a receipt with no observed terminal is exactly what a
    // coordinator crash looks like, and that must stay visible.
    const rawStatus = outcome ? asString(outcome.status) : undefined;
    const status: ConversationCouncilExchangeRow["status"] =
      rawStatus === "completed" || rawStatus === "failed" ? rawStatus : "pending";
    const text = outcome
      ? (status === "failed" ? asString(outcome.detail) : asString(outcome.text))
      : undefined;
    return [{
      round,
      sequence,
      participantOrder: participantOrder ?? 0,
      targetIdentity: asString(row.target_identity) || "",
      status,
      ...(text ? { text } : {}),
      ...(outcome && outcome.truncated === true ? { truncated: true } : {}),
    }];
  });
}

function artifactClaimRows(merge: Record<string, unknown> | null): ConversationCouncilArtifactClaimRow[] {
  if (!merge) return [];
  const raw = Array.isArray(merge.artifacts) ? merge.artifacts : [];
  return raw.flatMap((value) => {
    const row = asRecord(value);
    const uri = row ? asString(row.uri) : undefined;
    if (!row || !uri) return [];
    return [{
      uri,
      ...(asString(row.media_type) ? { mediaType: asString(row.media_type) } : {}),
      ...(asString(row.digest) ? { digest: asString(row.digest) } : {}),
      ...(asNumber(row.byte_len) !== undefined ? { byteLen: asNumber(row.byte_len) } : {}),
    }];
  });
}

/// Build a council card entry from one council tool-call frame pair.
///
/// Returns null when the frame carries no parseable sealed result - the
/// caller then leaves the ordinary tool row in place rather than rendering an
/// empty card. A council that is still running, or one whose result could not
/// be read, is better shown as a plain pending tool call than as a card
/// asserting a shape nobody confirmed.
export function councilEntryFromFrame(
  frame: ConsoleFrame,
  identity: ConversationCouncilEntry["identity"],
  argsByCallId?: Map<string, Record<string, unknown>>,
): ConversationCouncilEntry | null {
  const record = asRecord(frame.data);
  if (!record) return null;
  const payload = parseResultPayload(record);
  if (!payload) return null;
  const result = asRecord(payload.result);
  if (!result) return null;

  const councilId = asString(result.council_id);
  if (!councilId) return null;

  const callId = asString(record.tool_call_id) || asString(record.id);
  const args = parseArgsPayload(record)
    || (callId ? argsByCallId?.get(callId) ?? null : null);

  const exit = asRecord(result.exit_reason);
  const exitReason = exit ? asString(exit.reason) : undefined;
  const merge = asRecord(result.merge);
  const cleanup = asRecord(payload.cleanup);
  const debtsRaw = cleanup && Array.isArray(cleanup.debts) ? cleanup.debts : [];
  const debts = debtsRaw.flatMap((value) => {
    const row = asRecord(value);
    const subject = row ? asString(row.subject) : undefined;
    if (!row || !subject) return [];
    return [{ subject, detail: asString(row.detail) || "" }];
  });

  const allExchanges = exchangeRows(result);
  const shown = allExchanges.slice(0, COUNCIL_EXCHANGE_ROW_CAP);
  const overflow = allExchanges.length - shown.length;
  const claims = artifactClaimRows(merge);

  return {
    kind: "council",
    // Stable across re-renders and adapter passes so live updates land in
    // place, exactly like the workgraph card's `workgraph:{rootId}`.
    id: `council:${councilId}`,
    identity,
    ...(frame.timestampMs ? { createdAt: new Date(frame.timestampMs).toISOString() } : {}),
    ...(frame.interactionId ? { interactionId: frame.interactionId } : {}),
    councilId,
    topic: (args ? asString(args.topic) : undefined) || "Council",
    status: councilStatusFromExitReason(exitReason),
    exitReason: exitReason || "unknown",
    ...(exit && exitDetailOf(exit) ? { exitDetail: exitDetailOf(exit) } : {}),
    roundsCompleted: asNumber(result.rounds_completed) ?? 0,
    participants: participantRows(result),
    exchanges: shown,
    ...(overflow > 0 ? { exchangeOverflowCount: overflow } : {}),
    ...(merge && asString(merge.kind) ? { mergeKind: asString(merge.kind) } : {}),
    ...(merge && asString(merge.text) ? { mergeText: asString(merge.text) } : {}),
    ...(merge && asString(merge.finalizer) ? { mergeFinalizer: asString(merge.finalizer) } : {}),
    ...(merge && merge.truncated === true ? { mergeTruncated: true } : {}),
    ...(claims.length > 0 ? { artifactClaims: claims } : {}),
    ...(asNumber(result.truncated_exchange_count)
      ? { truncatedExchangeCount: asNumber(result.truncated_exchange_count) }
      : {}),
    ...(asString(result.durability) ? { durability: asString(result.durability) } : {}),
    ...(payload.replayed === true ? { replayed: true } : {}),
    ...(debts.length > 0 ? { cleanupDebts: debts } : {}),
    ...(cleanup && cleanup.budget_exhausted === true ? { cleanupBudgetExhausted: true } : {}),
    ...(asString(result.concluded_at) ? { concludedAt: asString(result.concluded_at) } : {}),
  };
}

/// Collect council call arguments by tool-call id.
///
/// The topic lives in the CALL, the result in the completion frame, and the
/// two are different frames. Without this the card would have a result and no
/// topic - the one field an operator scans for first.
export function councilArgsByCallId(frames: ConsoleFrame[]): Map<string, Record<string, unknown>> {
  const out = new Map<string, Record<string, unknown>>();
  for (const frame of frames) {
    if (!isCouncilToolFrame(frame)) continue;
    const record = asRecord(frame.data);
    if (!record) continue;
    const callId = asString(record.tool_call_id) || asString(record.id);
    if (!callId) continue;
    const args = parseArgsPayload(record);
    if (args && !out.has(callId)) out.set(callId, args);
  }
  return out;
}
