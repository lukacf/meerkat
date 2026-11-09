/**
 * Realistic console timeline generator for the typing-lag benchmarks.
 *
 * Emits gateway wire frames in the exact shape the acceptance fixture's real
 * runtime produces (captured from `console_acceptance_fixture`): per turn a
 * `send` user_input and its `frame_updated`, the live `console_event` run
 * (run/turn lifecycle, chunked text deltas, tool calls with results), and the
 * `session_history` twins plus an `assistant_history_snapshot` that lists
 * every assistant message so far. Frames are generated lazily per page so a
 * 5000-turn history costs only what the console actually loads.
 */

export type WireFrame = Record<string, unknown> & { cursor: string; kind: string };
/** An operator send as the gateway accepts it. */
export type OperatorSend = { content: string; idempotencyKey: string; origin: string };

const IDENTITY = "router:main";
const RUNTIME = "default";
const SESSION = "01a0f7d0-4be7-7d02-bce5-b29fc7d82249";
const BASE_MS = 1_790_000_000_000;
const TURN_SPACING_MS = 45_000;

const PARAGRAPH =
  "I went through the deployment notes and the incident timeline again. The short version is that the retry policy was doing exactly what it was configured to do, but the configuration assumed a much smaller fan-out than we actually run with today. When the upstream slowed down, every worker retried on the same schedule, which turned a brief latency spike into a sustained overload.";
const TABLE =
  "| Service | p50 | p95 | Errors | Owner |\n|---|---:|---:|---:|---|\n| gateway | 12 ms | 48 ms | 0.02% | platform |\n| scheduler | 4 ms | 19 ms | 0.00% | runtime |\n| projection | 31 ms | 140 ms | 0.31% | data |\n| voice | 88 ms | 210 ms | 1.10% | voice |";
const CODE = [
  "```rust\nuse std::collections::HashMap;\n\nfn tally(words: &[&str]) -> HashMap<&str, usize> {\n    let mut out = HashMap::new();\n    for w in words {\n        *out.entry(*w).or_insert(0) += 1;\n    }\n    out\n}\n```",
  "```ts\nexport async function load(id: string): Promise<Row[]> {\n  const res = await fetch(`/api/rows/${encodeURIComponent(id)}`);\n  if (!res.ok) throw new Error(`load failed: ${res.status}`);\n  return (await res.json()) as Row[];\n}\n```",
  "```python\ndef summarise(rows):\n    total = sum(r['amount'] for r in rows)\n    by_kind = {}\n    for r in rows:\n        by_kind.setdefault(r['kind'], []).append(r)\n    return total, by_kind\n```",
];

export function assistantReply(i: number): string {
  switch (i % 5) {
    case 0:
      return `## Status update ${i}\n\nHere is a **summary** with \`inline code\`, a [link](https://example.invalid/${i}) and some *emphasis*.\n\n- first point about the rollout\n- second point with \`config.retry.max = 3\`\n- third point, which wraps onto a second line because it is somewhat longer than the others\n\n${CODE[i % CODE.length]}\n\nLet me know if you want me to apply it.`;
    case 1:
      return `${PARAGRAPH}\n\n${PARAGRAPH}\n\n> Note: the change is backwards compatible.`;
    case 2:
      return `Here are the numbers from the last run:\n\n${TABLE}\n\nThe projection service is the outlier; its p95 doubled since last week.`;
    case 3:
      return `1. Check the queue depth\n2. Drain the stale leases\n3. Restart the worker pool\n   - one shard at a time\n   - verify health between shards\n4. Re-enable the scheduler\n\n${CODE[(i + 1) % CODE.length]}`;
    default:
      return `### Findings\n\n${PARAGRAPH}\n\n${TABLE}\n\n${CODE[(i + 2) % CODE.length]}\n\n${PARAGRAPH}`;
  }
}

function userText(i: number): string {
  return i % 7 === 3
    ? `${PARAGRAPH}\n\nCan you dig into this and propose a fix? (request ${i})`
    : `Question ${i}: what is the current state of the rollout, and are there any errors I should know about?`;
}

function hex(n: number, width: number): string {
  return (n >>> 0).toString(16).padStart(width, "0").slice(-width);
}

function uuid(kind: number, i: number, k = 0): string {
  return `01a0f7d0-${hex(kind, 4)}-7${hex(i, 3)}-8${hex(k, 3)}-${hex(i * 7919 + k, 12)}`;
}

const interactionId = (i: number) => uuid(1, i);
const runId = (i: number) => uuid(2, i);
const messageId = (i: number, k: number) => uuid(3, i, k);
const toolTurn = (i: number) => i % 3 === 1;
/** Assistant messages per turn: two tool-use messages before the answer. */
const messagesIn = (i: number) => (toolTurn(i) ? 3 : 1);

function ids(i: number) {
  return { interaction_id: interactionId(i), run_id: runId(i) };
}

/** Wire frames for one turn, without cursors (assigned by the caller).
 * `sequenceBase` continues the session's source sequence, which the runtime
 * numbers monotonically per session, not per run. */
function turnFrames(i: number, sequenceBase = 0, epoch?: string): Array<Omit<WireFrame, "cursor">> {
  const out: Array<Omit<WireFrame, "cursor">> = [];
  const t0 = BASE_MS + i * TURN_SPACING_MS;
  const iid = interactionId(i);
  const rid = runId(i);
  const common = { conversation_id: IDENTITY, identity: IDENTITY, runtime_key: RUNTIME, session_id: SESSION };
  let sequence = sequenceBase;
  let evt = 0;
  const live = (kind: string, ts: number, payload: Record<string, unknown>, extra: Record<string, unknown> = {}) => {
    sequence += 1;
    const id = `evt-agent-${uuid(4, i, (evt += 1))}`;
    out.push({
      ...common,
      id,
      dedupe_key: `console-event:${RUNTIME}:${id}`,
      frame_version: 1,
      interaction_id: iid,
      kind,
      payload: { identity: ids(i), run_id: rid, session_id: SESSION, source_event_type: kind, source_sequence: sequence, ...(epoch ? { source_epoch: epoch } : {}), type: kind, ...payload },
      run_id: rid,
      source: { kind: "console_event" },
      source_event_id: id,
      status: "delivered",
      timestamp_ms: ts,
      ...extra,
    });
  };
  const text = userText(i);
  const sendId = `console-frame-${hex(i, 8)}${"a".repeat(56)}`;
  const send = {
    ...common,
    dedupe_key: `send:default:${IDENTITY}:console:seed:seed-${i}`,
    frame_version: 2,
    id: sendId,
    interaction_id: iid,
    kind: "user_input",
    payload: { content: text, handling_mode: "queue", idempotency_key: `seed-${i}`, origin: "console:seed", origin_kind: "operator" },
    source: { kind: "send", source_cursor: hex(i * 31337, 16) },
    status: "delivered",
    timestamp_ms: t0,
    updated_at_ms: t0 + 18,
  };
  out.push(send);
  out.push({
    ...common,
    caused_by_frame_id: sendId,
    dedupe_key: `frame-update:${sendId}:2`,
    frame_version: 1,
    id: `console-frame-${hex(i, 8)}${"b".repeat(56)}`,
    interaction_id: iid,
    kind: "frame_updated",
    parent_frame_id: sendId,
    payload: { frame: send },
    source: { kind: "synthetic" },
    status: "delivered",
    timestamp_ms: t0 + 18,
  });
  live("run_started", t0 + 55, { input: { content: text, kind: "content" } });
  let ts = t0 + 55;
  const history: Array<Omit<WireFrame, "cursor">> = [];
  let historyCursor = 0;
  const historyFrame = (kind: string, created: number, payload: Record<string, unknown>, interaction = true) => {
    historyCursor += 1;
    history.push({
      ...common,
      dedupe_key: `session-history:${RUNTIME}:${SESSION}:${i}:${historyCursor}`,
      frame_version: 1,
      id: `console-frame-${hex(i, 8)}${hex(historyCursor, 4)}${"c".repeat(52)}`,
      ...(interaction ? { interaction_id: iid, run_id: rid } : {}),
      kind,
      payload: { source_event_type: "session_history", type: "session_history", ...payload },
      source: { kind: "session_history", source_cursor: `${SESSION}:${i}:${historyCursor}` },
      status: "completed",
      timestamp_ms: created,
    });
  };
  historyFrame("user_input", t0 + 50, {
    content: [{ text, type: "text" }],
    message: { content: text, created_at: new Date(t0 + 50).toISOString(), identity: ids(i), role: "user" },
  });
  const reply = assistantReply(i);
  for (let k = 0; k < messagesIn(i); k += 1) {
    const mid = messageId(i, k);
    ts += 2;
    live("turn_started", ts, { assistant_message_id: mid, turn_number: k });
    if (k < messagesIn(i) - 1) {
      const calls = [0, 1].map((c) => ({
        id: `call-${i}-${k}-${c}`,
        name: c === 0 ? "read_file" : "list_files",
        args: c === 0 ? { path: `services/gateway/src/retry_${(i + k) % 9}.rs` } : {},
        result: c === 0
          ? Array.from({ length: 10 }, (_, line) => `${line + 10}: let backoff = base * 2u32.pow(attempt); // turn ${i}`).join("\n")
          : "late-review.txt\nrelease-notes.txt\nretry.rs\n",
      }));
      for (const call of calls) {
        live("tool_call_requested", ts, { args: call.args, id: call.id, name: call.name, tool_call_id: call.id });
        live("tool_execution_started", ts, { id: call.id, name: call.name, tool_call_id: call.id });
        live("tool_execution_completed", ts + 1, { duration_ms: 1, id: call.id, is_error: false, name: call.name, tool_call_id: call.id });
        live("tool_result_received", ts + 1, { content: [{ text: call.result, type: "text" }], id: call.id, is_error: false, name: call.name, result: call.result, tool_call_id: call.id });
      }
      live("turn_completed", ts + 2, { assistant_message_id: mid, stop_reason: "tool_use" });
      historyFrame("assistant_message", ts, {
        assistant_message_id: mid,
        message: {
          assistant_message_id: mid,
          blocks: calls.map((call) => ({ block_type: "tool_use", data: { args: call.args, id: call.id, name: call.name } })),
          created_at: new Date(ts).toISOString(),
          identity: ids(i),
          role: "block_assistant",
          stop_reason: "tool_use",
        },
        result: "",
        text: "",
      });
      for (const call of calls) {
        historyFrame("tool_call_requested", ts, { args: call.args, assistant_message_id: mid, id: call.id, name: call.name, tool_call_id: call.id });
      }
      for (const call of calls) {
        historyFrame("tool_execution_completed", ts + 1, { content: [{ text: call.result, type: "text" }], id: call.id, is_error: false, result: call.result, tool_call_id: call.id }, false);
      }
      continue;
    }
    for (let at = 0; at < reply.length; at += 96) {
      live("text_delta", ts, { assistant_message_id: mid, delta: reply.slice(at, at + 96) });
    }
    live("text_complete", ts, { assistant_message_id: mid, content: reply });
    live("turn_completed", ts, { assistant_message_id: mid, stop_reason: "end_turn" });
    live("interaction_complete", ts + 12, { assistant_message_id: mid, extraction_required: false, result: reply, source_event_type: "run_completed", type: "run_completed" }, { status: "completed" });
    historyFrame("text_complete", ts - 1, {
      assistant_message_id: mid,
      message: {
        assistant_message_id: mid,
        blocks: [{ block_type: "text", data: { text: reply } }],
        created_at: new Date(ts - 1).toISOString(),
        identity: ids(i),
        role: "block_assistant",
        stop_reason: "end_turn",
      },
      result: reply,
      text: reply,
    });
  }
  return [...out, ...history];
}

/** Every assistant message id from turn 0 through `i`, as the runtime lists them. */
function assistantIdsThrough(i: number, cache: Map<number, string[]>): string[] {
  const cached = cache.get(i);
  if (cached) return cached;
  const list: string[] = [];
  for (let turn = 0; turn <= i; turn += 1) {
    for (let k = 0; k < messagesIn(turn); k += 1) list.push(messageId(turn, k));
  }
  cache.set(i, list);
  return list;
}

/**
 * A server-side identity log of `turns` completed turns, paged like the
 * gateway's `mode: "recent"` query (newest `limit` frames before `before`).
 */
export class RealisticTimeline {
  private replies = 0;
  readonly turns: number;
  private readonly turnStart: number[] = [];
  private readonly turnCount: number[] = [];
  private readonly sequenceStart: number[] = [];
  private nextSequence = 0;
  private readonly total: number;
  private readonly idCache = new Map<number, string[]>();
  private readonly turnCache = new Map<number, WireFrame[]>();
  private liveCursor: number;
  /** Stream epochs of the stored history and of live replies (see `restarted`). */
  private readonly historyEpoch?: string;
  private readonly liveEpoch?: string;

  /** `restarted`: the history was published by a previous gateway process and
   * live replies by this one. The gateway numbers a member's event stream from
   * the start again in a new process and stamps each event with its stream
   * epoch, so live sequences restart below the history's. */
  constructor(turns: number, options: { restarted?: boolean } = {}) {
    this.turns = turns;
    if (options.restarted) {
      this.historyEpoch = "previous-process.0.1";
      this.liveEpoch = "current-process.0.1";
    }
    let cursor = 1;
    for (let i = 0; i < turns; i += 1) {
      this.turnStart.push(cursor);
      this.sequenceStart.push(this.nextSequence);
      const frames = turnFrames(i);
      this.nextSequence += frames.filter((frame) => (frame.source as { kind?: string }).kind === "console_event").length;
      // turnFrames + the per-turn snapshot frame.
      const count = frames.length + 1;
      this.turnCount.push(count);
      cursor += count;
    }
    this.total = cursor - 1;
    this.liveCursor = cursor;
    if (options.restarted) this.nextSequence = 0;
  }

  get frameCount(): number {
    return this.total;
  }

  /// Live frames (streamReply) continue after `cursor`.
  continueLiveAfter(cursor: number): void {
    this.liveCursor = Math.max(this.liveCursor, cursor + 1);
  }

  private framesOfTurn(i: number): WireFrame[] {
    const cached = this.turnCache.get(i);
    if (cached) return cached;
    let cursor = this.turnStart[i];
    const frames: WireFrame[] = turnFrames(i, this.sequenceStart[i], this.historyEpoch).map((frame) => ({ ...frame, cursor: `console:${cursor++}` }));
    const observed = frames.filter((frame) => (frame.source as { kind?: string }).kind !== "session_history").at(-1)!.cursor;
    frames.push({
      conversation_id: IDENTITY,
      cursor: `console:${cursor}`,
      dedupe_key: `assistant-history-snapshot-v1:${RUNTIME}:${SESSION}:${i}`,
      frame_version: 1,
      id: `console-frame-${hex(i, 8)}${"d".repeat(56)}`,
      identity: IDENTITY,
      kind: "assistant_history_snapshot",
      payload: { assistant_message_ids: assistantIdsThrough(i, this.idCache), complete: true, observed_through: observed, session_id: SESSION },
      runtime_key: RUNTIME,
      session_id: SESSION,
      source: { kind: "session_history" },
      status: "delivered",
      timestamp_ms: BASE_MS + i * TURN_SPACING_MS + 40_000,
    });
    if (this.turnCache.size > 2_000) this.turnCache.clear();
    this.turnCache.set(i, frames);
    return frames;
  }

  private turnOfCursor(seq: number): number {
    let lo = 0;
    let hi = this.turns - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.turnStart[mid] <= seq) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  /** Newest `limit` frames strictly before `before` (or the end), oldest first. */
  recent(limit: number, before?: string): { frames: WireFrame[]; exhausted: boolean } {
    if (this.turns === 0) return { frames: [], exhausted: true };
    const end = before ? Math.min(this.total + 1, Number(before.split(":")[1])) : this.total + 1;
    const start = Math.max(1, end - limit);
    const frames: WireFrame[] = [];
    if (end > start) {
      for (let turn = this.turnOfCursor(start); turn < this.turns && this.turnStart[turn] < end; turn += 1) {
        for (const frame of this.framesOfTurn(turn)) {
          const seq = Number(frame.cursor.split(":")[1]);
          if (seq >= start && seq < end) frames.push(frame);
        }
      }
    }
    return { frames, exhausted: start <= 1 };
  }

  /** The accepted user_input frame of an operator send, then its streamed reply. */
  sendReply(input: OperatorSend, text: string, chunkChars: number): WireFrame[] {
    return this.streamReply(text, chunkChars, undefined, input);
  }

  /** Live frames of a new streamed reply, in arrival order, with fresh cursors.
   * With `input`, the reply answers an operator send: the accepted user_input
   * frame comes first, as the gateway echoes it after accepting the send. */
  streamReply(text: string, chunkChars: number, startMs?: number, input?: OperatorSend): WireFrame[] {
    const i = this.turns + 1_000 + (this.replies += 1);
    const t0 = startMs ?? BASE_MS + i * TURN_SPACING_MS;
    const rid = runId(i);
    const mid = messageId(i, 0);
    const frames: WireFrame[] = [];
    if (input !== undefined) {
      frames.push({
        conversation_id: IDENTITY,
        cursor: `console:${this.liveCursor++}`,
        dedupe_key: `send:default:${IDENTITY}:${input.origin}:${input.idempotencyKey}`,
        frame_version: 1,
        id: `console-frame-${hex(i, 8)}${"e".repeat(56)}`,
        identity: IDENTITY,
        interaction_id: interactionId(i),
        kind: "user_input",
        payload: { content: input.content, handling_mode: "queue", idempotency_key: input.idempotencyKey, origin: input.origin, origin_kind: "operator" },
        runtime_key: RUNTIME,
        session_id: SESSION,
        source: { kind: "send", source_cursor: hex(i * 31337, 16) },
        status: "delivered",
        timestamp_ms: t0 - 40,
      });
    }
    const live = (kind: string, payload: Record<string, unknown>) => {
      const sequence = (this.nextSequence += 1);
      const id = `evt-agent-${uuid(5, i, sequence)}`;
      frames.push({
        conversation_id: IDENTITY,
        cursor: `console:${this.liveCursor++}`,
        dedupe_key: `console-event:${RUNTIME}:${id}`,
        frame_version: 1,
        id,
        identity: IDENTITY,
        interaction_id: interactionId(i),
        kind,
        payload: { identity: ids(i), run_id: rid, session_id: SESSION, source_event_type: kind, source_sequence: sequence, ...(this.liveEpoch ? { source_epoch: this.liveEpoch } : {}), type: kind, assistant_message_id: mid, ...payload },
        run_id: rid,
        runtime_key: RUNTIME,
        session_id: SESSION,
        source: { kind: "console_event" },
        source_event_id: id,
        status: "delivered",
        timestamp_ms: t0 + frames.length,
      });
    };
    live("run_started", { input: { content: input?.content ?? "Write the long report now.", kind: "content" } });
    live("turn_started", { turn_number: 0 });
    for (let at = 0; at < text.length; at += chunkChars) live("text_delta", { delta: text.slice(at, at + chunkChars) });
    // The run's terminal frame. A stream stopped early still delivers it, so
    // the member is idle again (the console holds sends to a busy member).
    live("interaction_complete", { extraction_required: false, result: text, source_event_type: "run_completed", type: "run_completed" });
    frames[frames.length - 1].status = "completed";
    return frames;
  }
}

/** A fixed server log (e.g. a projected session), paged like the gateway. */
export class FixedTimeline {
  private readonly frames: WireFrame[];
  private liveUntilMs = 0;
  private readonly live = new RealisticTimeline(0);

  constructor(frames: WireFrame[]) {
    this.frames = frames;
    this.live.continueLiveAfter(frames.length);
  }

  get frameCount(): number {
    return this.frames.length;
  }

  recent(limit: number, before?: string): { frames: WireFrame[]; exhausted: boolean } {
    const end = before ? Math.min(this.frames.length, Number(before.split(":")[1]) - 1) : this.frames.length;
    const start = Math.max(0, end - limit);
    return { frames: this.frames.slice(start, end), exhausted: start === 0 };
  }

  sendReply(input: OperatorSend, text: string, chunkChars: number): WireFrame[] {
    return this.streamReply(text, chunkChars, input);
  }

  streamReply(text: string, chunkChars: number, input?: OperatorSend): WireFrame[] {
    // A live reply is newer than everything already in the log, including
    // earlier replies.
    const latest = this.frames.reduce((max, frame) => Math.max(max, Number(frame.timestamp_ms) || 0), this.liveUntilMs);
    const frames = this.live.streamReply(text, chunkChars, latest + 60_000, input);
    this.liveUntilMs = frames.reduce((max, frame) => Math.max(max, Number(frame.timestamp_ms) || 0), latest);
    return frames;
  }
}
