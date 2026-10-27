import type { ConsoleFrame } from "./runtime-types";
import {
  assistantHistorySnapshot, assistantMessageCursorSequence, assistantMessageId,
  assistantMessageKey, hasAssistantMessageIdCarrier,
} from "./assistant-message-identity";

const CONTENT_EVENTS = new Set([
  "text_delta", "text_complete", "reasoning_delta", "reasoning_complete",
  "server_tool_content", "assistant_image", "assistant_image_appended",
]);

function contextsConflict(left: ConsoleFrame, right: ConsoleFrame): boolean {
  return (["runtimeKey", "identity", "sessionId"] as const)
    .some(key => Boolean(left[key] && right[key] && left[key] !== right[key]));
}

function snapshotScope(frame: ConsoleFrame): string {
  return JSON.stringify([frame.runtimeKey ?? null, frame.identity ?? null, frame.sessionId]);
}

export function isCanonicalAssistantMessage(frame: ConsoleFrame): boolean {
  if (frame.sourceKind !== "session_history") return false;
  const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
  const message = data.message && typeof data.message === "object" ? data.message as Record<string, unknown> : {};
  return message.role === "block_assistant" || message.role === "assistant";
}

/** Standalone call frames remain available as evidence but render in their row. */
export function canonicalAssistantToolCounterparts(frames: ConsoleFrame[]): Set<ConsoleFrame> {
  const owners = new Map<string, ConsoleFrame[]>();
  for (const frame of frames) {
    if (!assistantMessageKey(frame) || !isCanonicalAssistantMessage(frame)) continue;
    const data = frame.data as Record<string, unknown>, message = data.message as Record<string, unknown>;
    if (!Array.isArray(message.blocks)) continue;
    for (const value of message.blocks) {
      if (!value || typeof value !== "object") continue;
      const block = value as Record<string, unknown>;
      if ((block.block_type ?? block.type) !== "tool_use") continue;
      const data = block.data && typeof block.data === "object" ? block.data as Record<string, unknown> : block;
      const id = data.id;
      if (typeof id !== "string" || !id.trim()) continue;
      const rows = owners.get(id) ?? [];
      rows.push(frame);
      owners.set(id, rows);
    }
  }
  const counterparts = new Set<ConsoleFrame>();
  for (const frame of frames) {
    if (!["tool_call_requested", "tool_call", "tool_execution_started"].includes(frame.event)) continue;
    const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
    const id = data.tool_call_id ?? data.id;
    if (typeof id !== "string") continue;
    const rows = owners.get(id)?.filter(owner => !contextsConflict(owner, frame)) ?? [];
    if (rows.length === 1) counterparts.add(frame);
  }
  return counterparts;
}

/** Rebuild from current history each time. Partial-page absence is not a deletion. */
export function reconcileAssistantMessageFrames(frames: ConsoleFrame[]): ConsoleFrame[] {
  const snapshots = new Map<string, { cursor: bigint; snapshot: NonNullable<ReturnType<typeof assistantHistorySnapshot>>; conflict: boolean }>();
  for (const frame of frames) {
    const snapshot = assistantHistorySnapshot(frame);
    const cursor = assistantMessageCursorSequence(frame.cursor);
    if (!snapshot || cursor === undefined) continue;
    const scope = snapshotScope(frame), previous = snapshots.get(scope);
    if (!previous || cursor > previous.cursor) snapshots.set(scope, { cursor, snapshot, conflict: false });
    else if (cursor === previous.cursor && (snapshot.observedThrough !== previous.snapshot.observedThrough
      || snapshot.assistantMessageIds.size !== previous.snapshot.assistantMessageIds.size
      || [...snapshot.assistantMessageIds].some(id => !previous.snapshot.assistantMessageIds.has(id)))) previous.conflict = true;
  }
  frames = frames.filter(frame => {
    if (frame.event === "assistant_history_snapshot") return false;
    const id = assistantMessageId(frame), current = snapshots.get(snapshotScope(frame));
    const snapshot = current && !current.conflict ? current.snapshot : undefined;
    const cursor = assistantMessageCursorSequence(frame.cursor);
    // A complete observation may invalidate only the console prefix it read.
    // Missing cursors and later live events remain provisional, never absent.
    return !id || !snapshot || cursor === undefined || cursor > snapshot.observedThrough
      || snapshot.assistantMessageIds.has(id);
  });
  const canonical = new Map<string, ConsoleFrame[]>();
  for (const frame of frames) {
    const key = assistantMessageKey(frame);
    if (!key || !isCanonicalAssistantMessage(frame)) continue;
    const rows = canonical.get(key) ?? [];
    if (!rows.some(row => snapshotScope(row) === snapshotScope(frame))) rows.push(frame);
    canonical.set(key, rows);
  }
  const canonicalFor = (frame: ConsoleFrame): ConsoleFrame | undefined => {
    const key = assistantMessageKey(frame);
    if (!key) return undefined;
    const candidates = canonical.get(key)?.filter(row => !contextsConflict(row, frame)) ?? [];
    const exact = candidates.filter(row => snapshotScope(row) === snapshotScope(frame));
    if (exact.length === 1) return exact[0];
    return candidates.length === 1 ? candidates[0] : undefined;
  };
  const suppressed = new Set<ConsoleFrame>();
  const replacement = new Map<ConsoleFrame, ConsoleFrame>();
  const placed = new Set<ConsoleFrame>();
  type Attempt = { owner: ConsoleFrame; open: boolean; frames: ConsoleFrame[] };
  const attempts = new Map<string, Attempt[]>();
  const compactions: ConsoleFrame[] = [];
  for (const frame of frames) {
    const key = assistantMessageKey(frame);
    if (frame.sourceKind === "session_history") {
      const row = canonicalFor(frame);
      if (row && isCanonicalAssistantMessage(frame)) {
        if (placed.has(row)) suppressed.add(frame);
        else { replacement.set(frame, row); placed.add(row); }
      }
      continue;
    }
    if (frame.event === "compaction_started") compactions.push(frame);
    if (frame.event === "compaction_completed" || frame.event === "compaction_failed") {
      for (let i = compactions.length - 1; i >= 0; i--) {
        if (!contextsConflict(compactions[i], frame)) compactions.splice(i, 1);
      }
    }
    if (!hasAssistantMessageIdCarrier(frame) && CONTENT_EVENTS.has(frame.event)
      && compactions.some(owner => !contextsConflict(owner, frame))) {
      suppressed.add(frame);
      continue;
    }
    const resultReference = frame.event === "run_completed"
      || frame.event === "interaction_complete";
    // Results are references, never a second message or a source of its text.
    if (resultReference && hasAssistantMessageIdCarrier(frame)) {
      suppressed.add(frame);
      continue;
    }
    if (!key) continue;
    const row = canonicalFor(frame);
    if (row && CONTENT_EVENTS.has(frame.event)) {
      if (!placed.has(row)) { replacement.set(frame, row); placed.add(row); }
      else suppressed.add(frame);
      continue;
    }
    const bucket = attempts.get(key) ?? [];
    let attempt = bucket.find(item => snapshotScope(item.owner) === snapshotScope(frame));
    if (!attempt) {
      attempt = { owner: frame, open: true, frames: [] };
      bucket.push(attempt);
      attempts.set(key, bucket);
    }
    if (frame.event === "retrying" || (frame.event === "turn_started" && attempt.open)) {
      for (const prior of attempt.frames) suppressed.add(prior);
      attempt.frames = [];
      attempt.open = true;
    } else if (frame.event === "turn_started") {
      attempt.open = true;
    }
    if (frame.event === "text_complete") {
      // The provider's finished text is still provisional, but it replaces
      // the text assembled for this exact attempt, including changed bytes.
      const priorText = attempt.frames.filter(prior => prior.event === "text_delta" || prior.event === "text_complete");
      if (priorText.length) {
        replacement.set(priorText[0], frame);
        suppressed.delete(priorText[0]);
        for (const prior of priorText.slice(1)) suppressed.add(prior);
        suppressed.add(frame);
      }
    }
    if (CONTENT_EVENTS.has(frame.event)) attempt.frames.push(frame);
    if (frame.event === "turn_completed") attempt.open = false;
  }
  return frames.flatMap(frame => suppressed.has(frame) ? [] : [replacement.get(frame) ?? frame]);
}
