/**
 * Session-history projection for the typing-lag benchmarks.
 *
 * `projectSessionHistory` turns a meerkat Session (as persisted: version, id,
 * messages, metadata) into the `session_history` console frames the gateway
 * serves after a reload, following
 * `frames_from_session_history_message_with_namespace` in
 * crates/meerkat-meta/src/console_aggregator: conversational user messages
 * become `user_input`, each tool result a `tool_execution_completed` carrying
 * its full text, assistant messages `text_complete` (with text) or
 * `assistant_message`, and tool-only steps their `tool_call_requested`
 * frames. Turn and message ids are re-minted per turn so masked copies of a
 * real session (whose ids collide once masked) still pair correctly.
 *
 * `flowForensicsLikeSession` builds a session with the shape of a real slow
 * production session: 62 messages whose block kinds and byte lengths match it
 * exactly (a 362 KB tool result, others of 121, 71 and 60 KB, 13 KB answers),
 * with masked-style content that keeps markdown, tables and code fences.
 */
import type { WireFrame } from "./realistic-transcript";

type Json = Record<string, unknown>;

export interface MeerkatSessionLike {
  id?: string;
  messages: Json[];
}

const IDENTITY = "router:main";
const RUNTIME = "default";

function hex(n: number, width: number): string {
  return (n >>> 0).toString(16).padStart(width, "0").slice(-width);
}

function uuid(kind: number, i: number): string {
  return `01a0f7d1-${hex(kind, 4)}-7${hex(i, 3)}-8${hex(i * 31 + kind, 3)}-${hex(i * 7919 + kind, 12)}`;
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content.map((block) => (block && typeof block === "object" ? text((block as Json).text) : "")).join("");
  }
  return "";
}

/** Gateway `session_history` frames for every message, with cursors from 1. */
export function projectSessionHistory(session: MeerkatSessionLike): WireFrame[] {
  const sessionId = "01a0f7d1-5e55-7000-8000-000000000001";
  const frames: Array<Omit<WireFrame, "cursor">> = [];
  let turn = -1;
  let assistant = 0;
  const base = (kind: string, offset: number, timestampMs: number, payload: Json, extra: Json = {}) => ({
    conversation_id: IDENTITY,
    dedupe_key: `session-history:${RUNTIME}:${sessionId}:${offset}:${frames.length}`,
    frame_version: 1,
    id: `console-frame-${hex(offset, 8)}${hex(frames.length, 8)}${"e".repeat(48)}`,
    identity: IDENTITY,
    kind,
    payload: { source_event_type: "session_history", type: "session_history", ...payload },
    runtime_key: RUNTIME,
    session_id: sessionId,
    source: { kind: "session_history", source_cursor: `${sessionId}:${offset}` },
    status: "completed",
    timestamp_ms: timestampMs,
    ...extra,
  });
  session.messages.forEach((raw, offset) => {
    const created = Date.parse(text(raw.created_at)) || 1_790_000_000_000 + offset * 1_000;
    const role = raw.role;
    if (role === "user") {
      // Runtime summaries share the user channel but are not conversational.
      if (raw.transcript_role && raw.transcript_role !== "user") return;
      turn += 1;
      const content = text(raw.content);
      const identity = { interaction_id: uuid(1, turn), run_id: uuid(2, turn) };
      frames.push(base("user_input", offset, created, {
        content: [{ text: content, type: "text" }],
        message: { ...raw, identity },
      }, identity));
      return;
    }
    const identity = turn >= 0 ? { interaction_id: uuid(1, turn), run_id: uuid(2, turn) } : {};
    if (role === "tool_results") {
      const results = Array.isArray(raw.results) ? raw.results as Json[] : [];
      results.forEach((result) => {
        const id = text(result.tool_use_id);
        frames.push(base("tool_execution_completed", offset, created, {
          id,
          tool_call_id: id,
          result: resultText(result.content),
          content: result.content,
          is_error: result.is_error === true,
        }));
      });
      return;
    }
    if (role !== "block_assistant") return;
    const blocks = Array.isArray(raw.blocks) ? raw.blocks as Json[] : [];
    const messageId = raw.assistant_message_id ? uuid(3, assistant++) : undefined;
    const message = { ...raw, identity, ...(messageId ? { assistant_message_id: messageId } : {}) };
    const answer = blocks
      .filter((block) => block.block_type === "text")
      .map((block) => text((block.data as Json | undefined)?.text))
      .join("");
    const toolCalls = () => blocks.filter((block) => block.block_type === "tool_use").forEach((block) => {
      const data = (block.data ?? {}) as Json;
      frames.push(base("tool_call_requested", offset, created, {
        id: data.id,
        tool_call_id: data.id,
        name: data.name,
        args: data.args,
        ...(messageId ? { assistant_message_id: messageId } : {}),
      }, identity));
    });
    if (!answer && !messageId) {
      toolCalls();
      return;
    }
    frames.push(base(answer ? "text_complete" : "assistant_message", offset, created, {
      result: answer,
      text: answer,
      message,
      ...(messageId ? { assistant_message_id: messageId } : {}),
    }, identity));
    if (!answer) toolCalls();
  });
  return frames.map((frame, index) => ({ ...frame, cursor: `console:${index + 1}` }));
}

// Block kinds and byte lengths of the real session, in order. s: system,
// u: user (1 = compaction summary), a: assistant ("r<len>" reasoning, "t<len>"
// text, "u<len>" tool args, "w" server tool; 1 = has a message id),
// r: tool results (lengths, error flags).
const FLOW_FORENSICS_SHAPE: Array<[string, unknown, unknown?]> = [["s",46614],["u",14185,1],["u",34,0],["a","r2424 t7029",0],["u",63,0],["a","t9427",0],["u",233,0],["a","r4876 u82 u89 u63 u81",0],["r",[1157,732,1743,3327],[0,0,0,0]],["a","u263",0],["r",[361993],[0]],["a","r4280 t12976",0],["u",5028,0],["a","r4132 t7562",0],["u",118,0],["a","u71",0],["r",[2441],[0]],["a","r2488 t535 u1434 u1164 u1191",0],["r",[78,78,78],[1,1,1]],["a","u88 u88 u86 u87 u64 u69",0],["r",[20330,3063,19065,3598,3191,700],[0,0,0,0,0,0]],["a","u222 u79 u80 u29 u29 u73 u61",0],["r",[59952,1878,3016,1359,120,706,1702],[0,0,0,0,0,0,0]],["a","r1508 u161 u30 u29 u25 u80 u65 u67 u81",0],["r",[71095,7045,3718,2729,2919,2619,1362,2770],[0,0,0,0,0,0,0,0]],["a","r1740 u157 u221 u221 u26 u147 u28 u25",0],["r",[12051,13143,12114,120,121294,1338,1457],[0,0,0,0,0,0,0]],["a","r4600 t8422",0],["u",30,0],["a","r4004 u221 u156 u230 u160 u66 u76",0],["r",[9565,9284,9400,9506,1701,1509],[0,0,0,0,0,0]],["a","r4260 t10040",0],["u",215,0],["a","r3512 t450 u25 u27 u25 u25 u25 u32 u26 u28 u24 u30",0],["r",[1309,1347,1329,1390,1304,1334,1371,1308,3001,1334],[0,0,0,0,0,0,0,0,0,0]],["a","u24 u26 u30 u27 u22 u33 u25 u30 u26 u26",0],["r",[2952,3191,1312,1413,2463,1409,1370,1345,1363,5004],[0,0,0,0,0,0,0,0,0,0]],["a","r3852 t7192",0],["u",82,0],["a","r3532 t324 u23 u33 u25 u30 u24 u27 u27",0],["r",[2710,3178,2459,3080,2149,1325,1828],[0,0,0,0,0,0,0]],["a","r2680 u61 u63 u63 u62 u61 u24 u27",0],["r",[1715,1319,1339,1292,1026,3091,4202],[0,0,0,0,0,0,0]],["a","r1868 u253 u155 u250 u155 u258",0],["r",[12188,7968,4586,6221,10355],[0,0,0,0,0]],["a","r3980 r3788 t10253",0],["u",32,0],["a","r3980 t8142",0],["u",114,0],["a","r3660 t223 u428",1],["r",[368],[0]],["a","w u820",1],["r",[3878],[0]],["a","r1356 w r2764 u1226",1],["r",[443],[0]],["a","r4580 u383",1],["r",[513],[0]],["a","r4708 t8519",1],["u",7133,0],["a","r1656 w r1400 w r1336 w r4324 t4159 w",1],["u",91,0],["a","r4748 t8811",1]];

function masked(length: number, seed: number): string {
  let out = "";
  let i = seed;
  while (out.length < length) {
    const word = "x".repeat(2 + ((i * 7) % 9));
    out += i % 13 === 0 ? `${word}00 ` : `${word} `;
    i += 1;
  }
  return out.slice(0, length);
}

/** Masked markdown of exactly `length` characters: headings, lists, tables, code. */
function maskedMarkdown(length: number, seed: number): string {
  const parts: string[] = [];
  let size = 0;
  let i = seed;
  while (size < length) {
    let part: string;
    switch (i % 5) {
      case 0: part = `## ${masked(28, i)}\n\n`; break;
      case 1: part = `${masked(320, i)}\n\n`; break;
      case 2: part = `- **${masked(18, i)}**: ${masked(90, i + 1)}\n- ${masked(110, i + 2)}\n\n`; break;
      case 3: part = `| ${masked(10, i)} | ${masked(12, i)} | 000 |\n|---|---|---:|\n| ${masked(10, i + 1)} | ${masked(12, i + 2)} | 00 |\n| ${masked(10, i + 3)} | ${masked(12, i + 4)} | 0 |\n\n`; break;
      default: part = "```xxx\n" + `${masked(60, i)}\n${masked(48, i + 1)}\n` + "```\n\n";
    }
    parts.push(part);
    size += part.length;
    i += 1;
  }
  return parts.join("").slice(0, length);
}

/** Masked JSON-looking tool output of exactly `length` characters. */
function maskedToolOutput(length: number, seed: number): string {
  const rows: string[] = [];
  let size = 2;
  let i = seed;
  while (size < length) {
    const row = `  {\n    "xxxxx": "${masked(30, i)}",\n    "xxx": "xxxxx://xxxx.xxxxxx.xxx/${masked(20, i)}",\n    "xxxxxxx": "${masked(140, i + 1)}"\n  },\n`;
    rows.push(row);
    size += row.length;
    i += 1;
  }
  return `[\n${rows.join("")}`.slice(0, length);
}

export function flowForensicsLikeSession(): MeerkatSessionLike {
  const messages: Json[] = [];
  let call = 0;
  let pending: string[] = [];
  const start = Date.parse("2026-09-22T12:15:00Z");
  FLOW_FORENSICS_SHAPE.forEach(([kind, a, b], index) => {
    const created_at = new Date(start + index * 45_000).toISOString();
    if (kind === "s") messages.push({ role: "system", content: maskedMarkdown(a as number, index), created_at });
    else if (kind === "u") {
      messages.push({
        role: "user",
        content: maskedMarkdown(a as number, index),
        ...(b ? { transcript_role: "compaction_summary" } : { identity: {} }),
        created_at,
      });
    } else if (kind === "r") {
      const lengths = a as number[];
      const errors = b as number[];
      messages.push({
        role: "tool_results",
        results: lengths.map((length, i) => ({ tool_use_id: pending[i] ?? `call_orphan_${index}_${i}`, content: maskedToolOutput(length, index + i), is_error: errors[i] === 1 })),
        created_at,
      });
      pending = [];
    } else {
      const blocks = String(a).split(" ").map((token, i) => {
        const size = Number(token.slice(1));
        if (token[0] === "t") return { block_type: "text", data: { text: maskedMarkdown(size, index + i) } };
        if (token[0] === "r") return { block_type: "reasoning", data: { text: "", meta: { provider: "xxxx_xx", encrypted_content: masked(size, i) } } };
        if (token[0] === "w") return { block_type: "server_tool_content", data: { id: `ws_${index}_${i}`, kind: { kind: "web_search" }, content: { action: { query: masked(60, i), type: "search" }, status: "completed", type: "web_search_call" } } };
        const id = `call_${String(call++).padStart(6, "0")}`;
        pending.push(id);
        return { block_type: "tool_use", data: { id, name: "xxxx_xxxxxx", args: { query: masked(Math.max(1, size - 12), i) } } };
      });
      messages.push({ role: "block_assistant", blocks, stop_reason: pending.length ? "tool_use" : "end_turn", ...(b ? { assistant_message_id: "masked" } : {}), created_at });
    }
  });
  return { id: "flow-forensics-like", messages };
}
