import type { ConsoleFrame } from "./runtime-types";
import { assistantHistorySnapshot, assistantMessageCursorSequence } from "./assistant-message-identity";

type RecordValue = Record<string, unknown>;
type Origin = { session_id: string; run_id: string; input_id: string; append_ordinal: number };
type Position = Array<string | number>;
type ProjectedFrame = {
  frame: ConsoleFrame;
  scope: string | null;
  position?: Position;
  sequence?: number;
  /** Where `sequence` is comparable: the session scope and, when the
   * publisher stamps one, its stream epoch. */
  sequenceScope: string | null;
  origin?: Origin;
  canonical?: boolean;
  observedThrough?: number;
  settled?: boolean;
};
type NoticeSnapshot = {
  frame: ConsoleFrame;
  cursor: number;
  observedThrough: number;
  notices: Array<{ offset: number; message: RecordValue; origin: Origin }>;
  settled: Set<string>;
  historyPositions?: Map<string, Position>;
  sparseHistoryPositions: boolean;
  removedHistoryFrameIds: Set<string>;
};
type HistoryPositionImage = {
  full?: NoticeSnapshot;
  deltas: Map<string, { observedThrough: number; position?: Position }>;
};

function record(value: unknown): RecordValue | null {
  return value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : null;
}

function identifier(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.trim() === value;
}

function ordinal(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

const string = (value: unknown): value is string => typeof value === "string";
const boolean = (value: unknown): value is boolean => typeof value === "boolean";
const optional = (value: unknown, check: (value: unknown) => boolean) => value == null || check(value);
const defaulted = (value: unknown, check: (value: unknown) => boolean) => value === undefined || check(value);
const strings = (value: unknown) => Array.isArray(value) && value.every(string);
const oneOf = (value: unknown, values: readonly string[]) => string(value) && values.includes(value);
const uuid = (value: unknown) => string(value)
  && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
const optionalStrings = (value: RecordValue, keys: string[]) => keys.every(key => optional(value[key], string));
const operation = (value: unknown) => oneOf(value, ["add", "remove", "reload"]);
const phase = (value: unknown) => oneOf(value, ["pending", "applied", "draining", "forced", "failed"]);

// Validate the Core wire shape before a complete image can invalidate existing
// rows. Core's SystemNoticeBlock decoder preserves unknown block tags, while
// known tags and SystemNoticeKind use their typed contracts.
function contentBlock(value: unknown): boolean {
  const block = record(value);
  if (!block) return false;
  switch (block.type) {
    case "text": return string(block.text);
    case "image": return string(block.media_type) && (block.source === "inline" ? string(block.data)
      : block.source === "blob" && string(block.blob_id));
    case "video": return string(block.media_type) && ordinal(block.duration_ms)
      && (block.source === "inline" ? string(block.data) : block.source === "uri" && string(block.uri));
    case "structured": return Object.hasOwn(block, "data");
    case "skill_context": {
      const key = record(block.skill_key);
      return string(block.text) && !!key && uuid(key.source_uuid) && string(key.skill_name)
        && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(key.skill_name);
    }
    default: return false;
  }
}

function toolConfigStatus(value: unknown): boolean {
  const status = record(value);
  if (!status) return false;
  switch (status.kind) {
    case "boundary_applied": return boolean(status.base_changed) && boolean(status.visible_changed) && ordinal(status.revision);
    case "deferred_catalog_delta": return ordinal(status.added_hidden_count)
      && ordinal(status.removed_hidden_count) && ordinal(status.pending_source_count);
    case "warning_failed_closed": return string(status.error);
    case "external_tool_delta": return phase(status.phase) && optional(status.detail, string);
    default: return false;
  }
}

// Core 0.9.4 writes the tool-config lifecycle bit as `settled`; rows that 0.9.3
// and earlier stored keep their `persisted` and are served back as stored.
const lifecycleBit = (value: RecordValue) => Object.hasOwn(value, "settled") ? value.settled : value.persisted;

function toolConfig(value: unknown): boolean {
  const payload = record(value);
  return !!payload && operation(payload.operation) && string(payload.target) && boolean(lifecycleBit(payload))
    && toolConfigStatus(payload.status_info)
    && optional(payload.applied_at_turn, value => ordinal(value) && value <= 0xffff_ffff)
    && optional(payload.domain, value => oneOf(value, ["tool_scope", "deferred_catalog"]))
    && optional(payload.deferred_catalog_delta, value => {
      const delta = record(value);
      return !!delta && ["added_hidden_names", "removed_hidden_names", "pending_sources"]
        .every(key => defaulted(delta[key], strings));
    });
}

function noticeBlock(value: unknown): boolean {
  const block = record(value);
  if (!block || !string(block.type)) return false;
  const content = (value: unknown) => Array.isArray(value) && value.every(contentBlock);
  switch (block.type) {
    case "comms": return string(block.kind) && oneOf(block.direction, ["incoming", "outgoing", "internal"])
      && optional(block.peer, value => {
        const peer = record(value);
        return !!peer && uuid(peer.id) && optional(peer.display_name, string);
      })
      && optional(block.sender_taint, value => oneOf(value, ["clean", "tainted"]))
      && optionalStrings(block, ["request_id", "intent", "status", "summary"])
      && defaulted(block.content, content);
    case "external_event": return string(block.source) && string(block.event_type)
      && optionalStrings(block, ["summary", "body"]) && defaulted(block.content, content);
    case "tool_config": return toolConfig(block.payload);
    case "mcp": return optionalStrings(block, ["server_id", "detail"])
      && optional(block.operation, operation) && optional(block.phase, phase)
      && defaulted(block.settled, boolean) && defaulted(block.persisted, boolean)
      && defaulted(block.pending_sources, strings);
    case "background_job": return string(block.job_id)
      && oneOf(block.status, ["completed", "failed", "aborted", "cancelled", "retired", "terminated"])
      && optionalStrings(block, ["display_name", "detail"]) && defaulted(block.persisted, boolean);
    case "auth": return string(block.state) && optionalStrings(block, ["binding", "detail"]);
    case "runtime_notice": return string(block.category) && optional(block.detail, string);
    case "unknown": return optional(block.summary, string);
    default: return true;
  }
}

function canonicalNotice(message: RecordValue): boolean {
  return message.role === "system_notice"
    && oneOf(message.kind, ["generic", "comms", "external_event", "mcp_pending", "mcp", "background_job",
      "tool_scope", "tool_scope_warning", "auth_reauth_required"])
    && optional(message.body, string)
    && defaulted(message.blocks, value => Array.isArray(value) && value.every(noticeBlock))
    && string(message.created_at) && Number.isFinite(Date.parse(message.created_at));
}

function consoleCursor(value: unknown): number | null {
  if (typeof value !== "string" || !/^console:\d+$/.test(value)) return null;
  const cursor = Number(value.slice(8));
  return ordinal(cursor) ? cursor : null;
}

function originOf(message: RecordValue | null): Origin | null {
  const origin = record(message?.runtime_origin);
  return origin && identifier(origin.session_id) && identifier(origin.run_id)
    && identifier(origin.input_id) && ordinal(origin.append_ordinal)
    ? origin as Origin : null;
}

function scopeOf(frame: ConsoleFrame): string | null {
  return identifier(frame.runtimeKey) && identifier(frame.sessionId)
    ? JSON.stringify([frame.runtimeKey, frame.sessionId]) : null;
}

/** Source sequences number one stream lifetime: the publisher starts again at
 * a process restart or member respawn and stamps `source_epoch` to say so.
 * Only frames of one epoch are ordered by sequence; frames without an epoch
 * (older stores) keep the session scope. */
function sequenceScopeOf(frame: ConsoleFrame): string | null {
  const scope = scopeOf(frame);
  const epoch = record(frame.data)?.source_epoch;
  return scope && identifier(epoch) ? JSON.stringify([frame.runtimeKey, frame.sessionId, epoch]) : scope;
}

function logicalKey(frame: ConsoleFrame, origin: Origin): string {
  return `runtime-notice:${JSON.stringify([
    frame.runtimeKey, frame.sessionId, origin.session_id, origin.input_id, origin.append_ordinal,
  ])}`;
}

/** Exact notice identity also scopes older comms de-duplication callbacks. */
export function runtimeAppendNoticeKey(frame: ConsoleFrame): string | null {
  if (frame.event !== "system_notice" || !scopeOf(frame)) return null;
  const origin = originOf(record(record(frame.data)?.message));
  return origin ? logicalKey(frame, origin) : null;
}

function attemptKey(scope: string, runId: string, inputId: string): string {
  return JSON.stringify([scope, runId, inputId]);
}

function noticeSnapshot(frame: ConsoleFrame): NoticeSnapshot | null {
  const data = record(frame.data), scope = scopeOf(frame);
  const cursor = consoleCursor(frame.cursor), observedThrough = consoleCursor(data?.observed_through);
  if (frame.event !== "runtime_notice_snapshot" || frame.sourceKind !== "session_history"
    || !scope || data?.session_id !== frame.sessionId || data.complete !== true
    || cursor === null || observedThrough === null || observedThrough >= cursor
    || !Array.isArray(data.notices) || !Array.isArray(data.settled_attempts)) return null;
  const notices: NoticeSnapshot["notices"] = [];
  const offsets = new Set<number>(), identities = new Set<string>();
  for (const value of data.notices) {
    const row = record(value), message = record(row?.message), origin = originOf(message);
    if (!row || !ordinal(row.offset) || !message || !origin
      || !canonicalNotice(message)) return null;
    const key = logicalKey(frame, origin);
    if (offsets.has(row.offset) || identities.has(key)) return null;
    offsets.add(row.offset);
    identities.add(key);
    notices.push({ offset: row.offset, message, origin });
  }
  const settled = new Set<string>();
  for (const value of data.settled_attempts) {
    const attempt = record(value);
    if (!attempt || !identifier(attempt.run_id) || !identifier(attempt.input_id)) return null;
    settled.add(attemptKey(scope, attempt.run_id, attempt.input_id));
  }
  let historyPositions: NoticeSnapshot["historyPositions"];
  if (data.history_positions !== undefined) {
    if (!Array.isArray(data.history_positions)) return null;
    historyPositions = new Map();
    for (const value of data.history_positions) {
      const row = record(value);
      const position = positionFromCursor(frame.sessionId!, row?.source_cursor);
      if (!row || !identifier(row.frame_id) || !position || historyPositions.has(row.frame_id)) return null;
      historyPositions.set(row.frame_id, position);
    }
  }
  const sparseHistoryPositions = data.history_positions_mode === "sparse";
  const removedHistoryFrameIds = new Set<string>();
  if (data.history_positions_mode !== undefined && !sparseHistoryPositions) return null;
  if (sparseHistoryPositions) {
    if (!historyPositions || !Array.isArray(data.removed_history_frame_ids)) return null;
    for (const id of data.removed_history_frame_ids) {
      if (!identifier(id) || removedHistoryFrameIds.has(id) || historyPositions.has(id)) return null;
      removedHistoryFrameIds.add(id);
    }
  } else if (data.removed_history_frame_ids !== undefined) return null;
  return { frame, cursor, observedThrough, notices, settled, historyPositions,
    sparseHistoryPositions, removedHistoryFrameIds };
}

function sameHistoryPositions(left: NoticeSnapshot, right: NoticeSnapshot): boolean {
  return left.observedThrough === right.observedThrough
    && left.sparseHistoryPositions === right.sparseHistoryPositions
    && left.historyPositions!.size === right.historyPositions!.size
    && [...left.historyPositions!].every(([id, position]) => {
      const other = right.historyPositions!.get(id);
      return other !== undefined && comparePosition(position, other) === 0;
    })
    && left.removedHistoryFrameIds.size === right.removedHistoryFrameIds.size
    && [...left.removedHistoryFrameIds].every(id => right.removedHistoryFrameIds.has(id));
}

/** Full images establish a base; sparse rows change only their explicit IDs. */
function historyPositionImages(snapshots: NoticeSnapshot[], scope: (frame: ConsoleFrame) => string | null,
  limits?: ReadonlyMap<string, bigint>): Map<string, HistoryPositionImage> {
  const observations = new Map<string, Map<number, { snapshot: NoticeSnapshot; conflict: boolean }>>();
  for (const snapshot of snapshots) {
    const key = scope(snapshot.frame);
    if (!key || !snapshot.historyPositions) continue;
    const byCursor = observations.get(key) ?? new Map();
    const prior = byCursor.get(snapshot.cursor);
    if (!prior) byCursor.set(snapshot.cursor, { snapshot, conflict: false });
    else if (!sameHistoryPositions(prior.snapshot, snapshot)) prior.conflict = true;
    observations.set(key, byCursor);
  }
  const images = new Map<string, HistoryPositionImage>();
  for (const [key, byCursor] of observations) {
    const limit = limits?.get(key);
    if (limits && limit === undefined) continue;
    const image: HistoryPositionImage = { deltas: new Map() };
    for (const { snapshot, conflict } of [...byCursor.values()].sort((a, b) => a.snapshot.cursor - b.snapshot.cursor)) {
      if (conflict) {
        // Either conflicting image could restore an older removed row. Earlier
        // negative evidence stays unsafe until a later complete base resolves it.
        image.full = undefined;
        image.deltas.clear();
        continue;
      }
      // Check all valid twins before settlement eligibility. A conflicting twin
      // beyond the cutoff must not leave older negative evidence uncontested.
      if (limit !== undefined && BigInt(snapshot.observedThrough) > limit) continue;
      if (!snapshot.sparseHistoryPositions) {
        image.full = snapshot;
        image.deltas.clear();
        continue;
      }
      for (const [id, position] of snapshot.historyPositions!) image.deltas.set(id, { observedThrough: snapshot.observedThrough, position });
      for (const id of snapshot.removedHistoryFrameIds) image.deltas.set(id, { observedThrough: snapshot.observedThrough });
    }
    images.set(key, image);
  }
  return images;
}

function historyPosition(frame: ConsoleFrame, image: HistoryPositionImage | undefined): { position?: Position } | undefined {
  const cursor = consoleCursor(frame.cursor);
  if (!image || cursor === null) return undefined;
  const delta = image.deltas.get(frame.id);
  if (delta && cursor <= delta.observedThrough) return delta;
  return image.full && cursor <= image.full.observedThrough
    ? { position: image.full.historyPositions!.get(frame.id) } : undefined;
}

/** A settled assistant observation authorizes covered position evidence only. */
export function reconcileAssistantHistoryPositions(frames: ConsoleFrame[]): ConsoleFrame[] {
  const scope = (frame: ConsoleFrame) => JSON.stringify([frame.runtimeKey ?? null, frame.identity ?? null, frame.sessionId]);
  const assistants = new Map<string, { cursor: bigint; observedThrough: bigint; ids: ReadonlySet<string>; conflict: boolean }>();
  const positions: NoticeSnapshot[] = [];
  for (const frame of frames) {
    const assistant = assistantHistorySnapshot(frame), cursor = assistantMessageCursorSequence(frame.cursor);
    if (assistant && cursor !== undefined) {
      const key = scope(frame), prior = assistants.get(key);
      if (!prior || cursor > prior.cursor) assistants.set(key, { cursor, observedThrough: assistant.observedThrough, ids: assistant.assistantMessageIds, conflict: false });
      else if (cursor === prior.cursor && (assistant.observedThrough !== prior.observedThrough
        || assistant.assistantMessageIds.size !== prior.ids.size
        || [...assistant.assistantMessageIds].some(id => !prior.ids.has(id)))) prior.conflict = true;
    }
    const position = noticeSnapshot(frame);
    if (position?.historyPositions) positions.push(position);
  }
  const images = historyPositionImages(positions, scope,
    new Map([...assistants].filter(([, assistant]) => !assistant.conflict).map(([key, assistant]) => [key, assistant.observedThrough])));
  return frames.filter(frame => {
    if (frame.sourceKind !== "session_history") return true;
    const message = record(record(frame.data)?.message);
    if (message?.role !== "assistant" && message?.role !== "block_assistant") return true;
    const key = scope(frame), assistant = assistants.get(key);
    const cursor = assistantMessageCursorSequence(frame.cursor);
    if (!assistant || assistant.conflict
      || cursor === undefined || cursor > assistant.observedThrough) return true;
    const observation = historyPosition(frame, images.get(key));
    return observation === undefined || observation.position !== undefined;
  });
}

function positionFromCursor(sessionId: string, sourceCursor: unknown): Position | undefined {
  if (!string(sourceCursor) || !sourceCursor.startsWith(`${sessionId}:`)) return undefined;
  const parts = sourceCursor.slice(sessionId.length + 1).split(":");
  if (!/^\d+$/.test(parts[0]) || !ordinal(Number(parts[0]))) return undefined;
  if (parts.some(part => /^\d+$/.test(part) ? !ordinal(Number(part)) : !/^[a-zA-Z_][a-zA-Z0-9_-]*$/.test(part))) return undefined;
  return parts.map(part => /^\d+$/.test(part) ? Number(part) : part);
}

function canonicalPosition(frame: ConsoleFrame): Position | undefined {
  return frame.sourceKind === "session_history" && identifier(frame.sessionId)
    ? positionFromCursor(frame.sessionId, frame.sourceCursor) : undefined;
}

function sourceSequence(frame: ConsoleFrame): number | undefined {
  const value = record(frame.data)?.source_sequence;
  return frame.sourceKind === "console_event" && ordinal(value) ? value : undefined;
}

function comparePosition(left: Position, right: Position): number {
  for (let index = 0; index < Math.min(left.length, right.length); index++) {
    const a = left[index], b = right[index];
    if (a === b) continue;
    if (typeof a === "number" && typeof b === "number") return a - b;
    if (typeof a !== typeof b) return typeof a === "number" ? -1 : 1;
    return String(a).localeCompare(String(b));
  }
  return left.length - right.length;
}

function projected(frame: ConsoleFrame): ProjectedFrame {
  return { frame, scope: scopeOf(frame), position: canonicalPosition(frame), sequence: sourceSequence(frame), sequenceScope: sequenceScopeOf(frame) };
}

function newestObservation(nodes: ProjectedFrame[], sourceOrder: boolean): ProjectedFrame | undefined {
  if (sourceOrder) {
    const sequenced = nodes.filter(node => node.sequence !== undefined);
    // Sequences of different stream epochs are not comparable; observation
    // order decides between them.
    if (sequenced.length && new Set(sequenced.map(node => node.sequenceScope)).size === 1) {
      const attempts = new Set(sequenced.map(node => node.origin!.run_id));
      const latest = sequenced.reduce((latest, node) => Math.max(latest, node.sequence!), 0);
      // Compare the strongest source witnesses first. A legacy replay of an
      // already sequenced attempt cannot create a cyclic recency comparison.
      return newestObservation(nodes.filter(node => node.sequence === latest
        || (node.sequence === undefined && !attempts.has(node.origin!.run_id))), false);
    }
  }
  return nodes.reduce<ProjectedFrame | undefined>((previous, node) => {
    if (!previous) return node;
    const cursor = node.observedThrough ?? consoleCursor(node.frame.cursor);
    const previousCursor = previous.observedThrough ?? consoleCursor(previous.frame.cursor);
    return cursor !== null && (previousCursor === null || cursor > previousCursor) ? node : previous;
  }, undefined);
}

function toolCounterpartKey(item: ProjectedFrame): string | null {
  if (!item.scope) return null;
  const call = ["tool_call_requested", "tool_call", "tool_execution_started"].includes(item.frame.event);
  const result = ["tool_result_received", "tool_execution_completed"].includes(item.frame.event);
  if (!call && !result) return null;
  const data = record(item.frame.data);
  const id = data?.tool_call_id ?? data?.id;
  return identifier(id) ? JSON.stringify([item.scope, call ? "call" : "result", id]) : null;
}

function userCounterpartKey(item: ProjectedFrame): string | null {
  const id = item.frame.interactionId;
  return item.scope && item.frame.event === "user_input" && typeof id === "string"
    && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)
    ? JSON.stringify([item.scope, "user", id]) : null;
}

// Compute a stable linear extension of two owner orders, never a comparator that
// sometimes compares timestamps and sometimes offsets. The original order is
// the tie-breaker for frames for which neither source supplies a relationship.
function orderBySource(nodes: ProjectedFrame[]): ConsoleFrame[] {
  const canonicalCounterparts = new Map<string, Position>();
  for (const node of nodes) {
    const key = toolCounterpartKey(node) ?? userCounterpartKey(node);
    if (key && node.position && node.frame.sourceKind === "session_history") canonicalCounterparts.set(key, node.position);
  }
  for (const node of nodes) {
    const key = toolCounterpartKey(node) ?? userCounterpartKey(node);
    if (!node.position && key && node.frame.sourceKind === "console_event") node.position = canonicalCounterparts.get(key);
  }
  const scopes = new Map<string, number[]>();
  const sequenceScopes = new Map<string, number[]>();
  nodes.forEach((node, index) => {
    if (node.scope) {
      const indices = scopes.get(node.scope) ?? [];
      indices.push(index);
      scopes.set(node.scope, indices);
    }
    if (node.sequenceScope && node.sequence !== undefined) {
      const indices = sequenceScopes.get(node.sequenceScope) ?? [];
      indices.push(index);
      sequenceScopes.set(node.sequenceScope, indices);
    }
  });
  const edges = nodes.map(() => new Set<number>());
  const incoming = nodes.map(() => 0);
  const connect = (from: number, to: number) => {
    if (from === to || edges[from].has(to)) return;
    edges[from].add(to);
    incoming[to]++;
  };
  const liveFirst = (a: number, b: number) => (
    Number(nodes[a].frame.sourceKind === "session_history") - Number(nodes[b].frame.sourceKind === "session_history")
    || a - b
  );
  for (const indices of scopes.values()) {
    const positioned = indices.filter(index => nodes[index].position)
      .sort((a, b) => comparePosition(nodes[a].position!, nodes[b].position!) || liveFirst(a, b));
    for (let index = 1; index < positioned.length; index++) connect(positioned[index - 1], positioned[index]);
  }
  for (const indices of sequenceScopes.values()) {
    const sequenced = [...indices].sort((a, b) => nodes[a].sequence! - nodes[b].sequence! || a - b);
    for (let index = 1; index < sequenced.length; index++) connect(sequenced[index - 1], sequenced[index]);
  }
  // Min heap keeps unconnected source rows in their existing stable order and
  // avoids quadratic scans during a long-running transcript's render pass.
  const ready: number[] = [];
  const push = (value: number) => {
    let index = ready.length;
    ready.push(value);
    while (index > 0) {
      const parent = (index - 1) >> 1;
      if (ready[parent] <= value) break;
      ready[index] = ready[parent];
      index = parent;
    }
    ready[index] = value;
  };
  const pop = () => {
    const value = ready[0];
    const tail = ready.pop()!;
    if (ready.length) {
      let index = 0;
      while (index * 2 + 1 < ready.length) {
        let child = index * 2 + 1;
        if (child + 1 < ready.length && ready[child + 1] < ready[child]) child++;
        if (ready[child] >= tail) break;
        ready[index] = ready[child];
        index = child;
      }
      ready[index] = tail;
    }
    return value;
  };
  incoming.forEach((count, index) => { if (!count) push(index); });
  const ordered: ConsoleFrame[] = [];
  while (ready.length) {
    const index = pop();
    ordered.push(nodes[index].frame);
    for (const next of edges[index]) if (--incoming[next] === 0) push(next);
  }
  // Conflicting history images are not authority to reorder a transcript. The
  // backend's complete-current-image projection resolves those observations.
  return ordered.length === nodes.length ? ordered : nodes.map(node => node.frame);
}

/** Where a newest-first window of a longer timeline begins: the time of its
 * oldest frame, or +Infinity while no loaded frame carries a time. */
export function loadedTimelineStartMs(frames: readonly ConsoleFrame[]): number {
  let start = Number.POSITIVE_INFINITY;
  for (const frame of frames) {
    if (typeof frame.timestampMs === "number" && frame.timestampMs < start) start = frame.timestampMs;
  }
  return start;
}

export type RuntimeAppendProjectionOptions = {
  /** Set while older timeline pages are not loaded: `loadedTimelineStartMs`
   * of the loaded frames. History before it is unloaded. */
  unloadedBeforeMs?: number;
};

/** Project exact runtime appends over the caller's existing stable frame order. */
export function reconcileRuntimeAppendFrames(frames: readonly ConsoleFrame[],
  options: RuntimeAppendProjectionOptions = {}): ConsoleFrame[] {
  const snapshots = new Map<string, NoticeSnapshot>();
  const positionSnapshots: NoticeSnapshot[] = [];
  for (const frame of frames) {
    const snapshot = noticeSnapshot(frame);
    if (!snapshot) continue;
    positionSnapshots.push(snapshot);
    const scope = scopeOf(frame)!;
    const previous = snapshots.get(scope);
    if (!previous || snapshot.cursor > previous.cursor) snapshots.set(scope, snapshot);
  }
  const positionImages = historyPositionImages(positionSnapshots, scopeOf);
  const discarded = new Set<string>();
  for (const frame of frames) {
    if (frame.event !== "boundary_appends_discarded" || frame.sourceKind !== "console_event") continue;
    const data = record(frame.data), scope = scopeOf(frame);
    if (!scope || data?.session_id !== frame.sessionId || data?.run_id !== frame.runId
      || !identifier(data?.run_id) || !Array.isArray(data?.input_ids)) continue;
    for (const input of data.input_ids) if (identifier(input)) discarded.add(attemptKey(scope, data.run_id, input));
  }

  const nodes: ProjectedFrame[] = [];
  const candidates = new Map<string, ProjectedFrame[]>();
  const append = (node: ProjectedFrame, before = nodes.length) => {
    nodes.splice(before, 0, node);
    if (!node.origin) return;
    const key = logicalKey(node.frame, node.origin);
    const twins = candidates.get(key) ?? [];
    twins.push(node);
    candidates.set(key, twins);
  };
  for (const frame of frames) {
    if (frame.event === "boundary_appends_discarded" || frame.event === "runtime_notice_snapshot") continue;
    const data = record(frame.data), scope = scopeOf(frame);
    const snapshot = scope ? snapshots.get(scope) : undefined;
    const cursor = consoleCursor(frame.cursor);
    const observed = snapshot && cursor !== null && cursor <= snapshot.observedThrough;
    if (frame.event === "boundary_append_applied") {
      // Meerkat omits an empty notices vector for ordinary user appends.
      // The transport acknowledgement itself is never a transcript row.
      if (!Array.isArray(data?.notices) || !data.notices.length) continue;
      if (frame.sourceKind !== "console_event" || !scope || !identifier(frame.runId)
        || data.run_id !== frame.runId || !identifier(data.input_id)
        || !ordinal(data.append_count) || !ordinal(data.transcript_start)) continue;
      for (const value of data.notices) {
        const message = record(value), origin = originOf(message);
        if (!message || !origin || origin.session_id !== frame.sessionId || origin.run_id !== frame.runId
          || origin.input_id !== data.input_id || origin.append_ordinal >= data.append_count
          || !ordinal(data.transcript_start + origin.append_ordinal)) continue;
        const timestamp = typeof message.created_at === "string" ? Date.parse(message.created_at) : NaN;
        const noticeFrame: ConsoleFrame = {
          ...frame, id: logicalKey(frame, origin), event: "system_notice",
          timestampMs: Number.isFinite(timestamp) ? timestamp : frame.timestampMs,
          data: { message: { ...message, role: "system_notice" } },
        };
        append({ frame: noticeFrame, scope, origin, sequenceScope: sequenceScopeOf(frame),
          position: [data.transcript_start + origin.append_ordinal], sequence: sourceSequence(frame),
          settled: Boolean(observed && snapshot.settled.has(attemptKey(scope, origin.run_id, origin.input_id))) });
      }
      continue;
    }
    const node = projected(frame);
    if (frame.sourceKind === "session_history" && scope) {
      // A notice-only refresh does not replace the last complete position base.
      // Rows beyond that base retain their own canonical coordinates until an
      // explicit later position observation covers them.
      const position = historyPosition(frame, positionImages.get(scope));
      if (position) node.position = position.position;
    }
    if (frame.event === "system_notice" && frame.sourceKind === "session_history" && scope) {
      const message = record(data?.message), origin = originOf(message);
      if (origin) {
        if (observed) continue;
        node.origin = origin;
        node.canonical = true;
        node.frame = { ...frame, id: logicalKey(frame, origin) };
      }
    }
    append(node);
  }
  for (const [scope, snapshot] of snapshots) {
    for (const { offset, message, origin } of snapshot.notices) {
      const key = logicalKey(snapshot.frame, origin);
      const frame: ConsoleFrame = {
        ...snapshot.frame, event: "system_notice", id: key,
        runId: origin.run_id, sourceCursor: `${snapshot.frame.sessionId}:${offset}`,
        timestampMs: Date.parse(String(message.created_at)), data: { message },
      };
      // A snapshot carries every notice of its session, but a newest-first
      // page loads only recent history. A notice older than every loaded
      // frame would sit directly above much newer rows, hiding the unloaded
      // history between them. It waits until older pages reach its time,
      // unless its exact live or history twin is already loaded.
      if (options.unloadedBeforeMs !== undefined && frame.timestampMs! < options.unloadedBeforeMs
        && !candidates.has(key)) continue;
      // A snapshot projects an authored row, not a row born when the snapshot
      // arrived. Keep its chronological fallback among the caller's rows before
      // applying canonical/source-sequence edges. Appending it at the tail
      // strands all later canonical history behind new live deltas, making an
      // old notice split and finalize the current assistant document.
      const before = nodes.findIndex(node => typeof node.frame.timestampMs === "number"
        && node.frame.timestampMs > frame.timestampMs!);
      append({ frame, scope, origin, position: [offset], canonical: true, observedThrough: snapshot.observedThrough, sequenceScope: scope },
        before < 0 ? nodes.length : before);
    }
  }

  const chosen = new Map<string, ProjectedFrame>();
  for (const [key, twins] of candidates) {
    const canonical = newestObservation(twins.filter(node => node.canonical), false);
    const eligible = twins.filter(node => !node.settled
      && !discarded.has(attemptKey(node.scope!, node.origin!.run_id, node.origin!.input_id)));
    const winner = canonical ?? newestObservation(eligible, true);
    if (!winner) continue;
    // Positive history proves this exact attempt remains current, including its
    // source order. Settlement or a delayed discard cannot erase that anchor.
    const liveTwin = newestObservation((canonical ? twins : eligible).filter(node => !node.canonical
      && node.origin!.run_id === winner.origin!.run_id), true);
    if (liveTwin?.sequence !== undefined) {
      winner.sequence = liveTwin.sequence;
      winner.sequenceScope = liveTwin.sequenceScope;
    }
    chosen.set(key, winner);
  }
  const emitted = new Set<string>();
  const reconciled: ProjectedFrame[] = [];
  for (const node of nodes) {
    if (!node.origin) { reconciled.push(node); continue; }
    const key = logicalKey(node.frame, node.origin);
    if (emitted.has(key)) continue;
    emitted.add(key);
    const winner = chosen.get(key);
    if (winner) reconciled.push(winner);
  }
  return orderBySource(reconciled);
}

/** Ordering inputs of `reconcileRuntimeAppendFrames`, for incremental callers
 * that must prove an appended frame cannot reorder: the source-sequence scope
 * and the source sequence that orders frames within it. */
export { sequenceScopeOf as runtimeSourceScope, sourceSequence as runtimeSourceSequence };
