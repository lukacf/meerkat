// The shared frame classifier: one reading of a console frame for every
// panel that lists or counts events (Today, Logs, Signals, Topology
// traffic, Agent details), so they agree on what a frame is, how serious
// it is and the words for it. Severity reads the payload (`is_error`,
// `error_report.class`, typed failure reasons, delivery failures, timed-out
// tools, memory warnings), never substrings of the event name. Pure module
// (no React).

import { normalizeStatusWord, type Tone } from "../../../packages/console-components/src/panel-kit/tone";
import type { AgentDirectory } from "../../../packages/console-components/src/panel-kit/agents";
import { humanizeEnum } from "../../../packages/console-components/src/panel-kit/format";
import { memberKickoffNotice, memberKickoffPhaseLabel } from "../../../packages/console-core/src/member-kickoff";
import { stripPeerTransportScaffold } from "./adapters";
import { describeFailure, summarizeFailureData } from "./failure-summary";
import { isOpenLoopNudge, quarantineVerdictWords } from "./memory-event-words";
import { turnSender, turnSenderWords } from "./turn-origin";
import type { ConsoleFrame } from "../types";

export type FrameFamily =
  | "turn"
  | "tool"
  | "message"
  | "approval"
  | "lifecycle"
  | "memory"
  | "workgraph"
  | "routing"
  | "system";

/** How much a frame asks for attention: crit means something failed. */
export type FrameSeverity = "info" | "warn" | "crit";

/** Part of a sentence: plain words, or an agent by key (null reads "System"). */
export type FrameSegment = string | { agent: string | null };

/** Where a frame starts or ends a span of work, for timelines and the ledger. */
export interface FrameSpan {
  kind: "run" | "turn" | "tool";
  /** The span's key: the same for its start and its end. */
  key: string;
  edge: "start" | "end";
  outcome?: "ok" | "failed" | "cancelled" | "timed_out";
  /** The run or interaction the span belongs to, when the frame names them. */
  runId?: string;
  interactionId?: string;
}

export interface ClassifiedFrame {
  family: FrameFamily;
  /** The family in words: "Turn", "Tool", "Message", "Approval", ... */
  familyWord: string;
  severity: FrameSeverity;
  /** The tone of the frame's mark: ok for a good outcome, crit for a failure. */
  tone: Tone;
  /** A short title without the agent: "Turn finished", "Approval requested". */
  title: string;
  /** What the agent did, after its name: "finished a turn". */
  verb: string;
  /** The sentence, agents by key: [{ agent }, " finished a turn"]. */
  segments: FrameSegment[];
  /**
   * The agent the event belongs to (its lane): the agent that acted, the
   * agent a request went to, or for a message its sender. Null for the
   * console's and the system's own events.
   */
  actor: string | null;
  /** The other agent: a message's recipient. */
  target?: string;
  /** A short excerpt: the reply, the request, the failure's reason. */
  snippet?: string;
  span?: FrameSpan;
  /** How long the work took, when the frame says (tool executions). */
  durationMs?: number;
  /**
   * One logical event's key across the copies the console can see (a live
   * frame and its saved history counterpart, or an alias event), for
   * counting: two frames with the same key are one event.
   */
  eventKey: string;
  /** Plumbing frames (snapshots, deltas, updates): no panel lists them. */
  internal: boolean;
}

const FAMILY_WORD: Readonly<Record<FrameFamily, string>> = {
  turn: "Turn",
  tool: "Tool",
  message: "Message",
  approval: "Approval",
  lifecycle: "Lifecycle",
  memory: "Memory",
  workgraph: "WorkGraph",
  routing: "Routing",
  system: "System",
};

/** Plumbing frames: snapshots, stream control, deltas and in-place updates. */
const INTERNAL_EVENTS: ReadonlySet<string> = new Set([
  "runtime_notice_snapshot",
  "assistant_history_snapshot",
  "runtime_bootstrapped",
  "boundary_append_applied",
  "boundary_appends_discarded",
  "text_delta",
  "text_complete",
  "reasoning_delta",
  "reasoning_complete",
  "frame_updated",
  "keep-alive",
  "subscribed",
  "snapshot_started",
  "snapshot_complete",
  "replay_unavailable",
  "stream_lagged",
  "mcp_app",
  "tool_config_changed",
  "tool_scope_changed",
  "skills_resolved",
  "server_tool_content",
  "assistant_message",
  "provider_cache_breakpoints_discarded",
  "turn_usage_accounting_unmeasured",
  "turn_usage_accounting_identity_disputed",
  "transcript_rewrite_committed",
  "transcript_rewrite_audit_receipt_committed",
  "hook_started",
  "hook_completed",
  "extraction_succeeded",
  "peer_content_ingested",
]);

const PEER_TOOLS: ReadonlySet<string> = new Set(["send_message", "send_request", "send_response", "reply_to_peer"]);
const RUN_END_OK_EVENTS: ReadonlySet<string> = new Set(["interaction_complete", "run_completed"]);
const RUN_END_FAILED_EVENTS: ReadonlySet<string> = new Set(["interaction_failed", "run_failed"]);
const RUN_CANCELLED_EVENTS: ReadonlySet<string> = new Set(["run_cancelled", "run_canceled"]);
/** Failure classes and reasons that end a turn without anything going wrong. */
const QUIET_FAILURES: ReadonlySet<string> = new Set([
  "cancelled",
  "canceled",
  "lifecycle_mutation",
  "superseded_by_later_run",
]);
const SNIPPET_MAX = 140;

type Data = Record<string, unknown>;

function recordOf(value: unknown): Data {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Data : {};
}

function textValue(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function numberValue(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** Readable text from a payload: strings, arrays of parts, and assistant
 * messages (their text blocks only, never reasoning or tool blocks). */
function textOf(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (Array.isArray(value)) return value.map(textOf).filter(Boolean).join(" ").trim();
  if (value && typeof value === "object") {
    const record = value as Data;
    if (Array.isArray(record.blocks)) {
      return record.blocks
        .map((block) => {
          const part = recordOf(block);
          const type = part.block_type ?? part.type;
          return type === "text" ? textOf(recordOf(part.data).text ?? part.text) : "";
        })
        .filter(Boolean)
        .join(" ")
        .trim();
    }
    return textOf(record.text ?? record.result ?? record.summary ?? record.body ?? record.content);
  }
  return "";
}

function snippetOf(value: string): string | undefined {
  const text = stripPeerTransportScaffold(value).replace(/\s+/g, " ").trim();
  if (!text || /^You have been spawned as\b/i.test(text)) return undefined;
  return text.length > SNIPPET_MAX ? `${text.slice(0, SNIPPET_MAX - 3).trimEnd()}...` : text;
}

function lastPathSegment(value: string): string {
  return value.split("/").filter(Boolean).pop() || value;
}

/** A peer reference as the directory knows it: its canonical key when known. */
function peerKey(reference: string, directory: AgentDirectory | undefined): string {
  const trimmed = reference.trim();
  if (!trimmed) return "";
  const entry = directory?.lookup(trimmed) ?? directory?.lookup(lastPathSegment(trimmed));
  return entry?.id || lastPathSegment(trimmed);
}

function runIdOf(frame: ConsoleFrame, data: Data): string {
  return frame.runId?.trim() || textValue(data.run_id) || textValue(recordOf(data.identity).run_id);
}

function interactionIdOf(frame: ConsoleFrame, data: Data): string {
  return frame.interactionId?.trim() || textValue(data.interaction_id) || textValue(recordOf(data.identity).interaction_id);
}

function frameAgent(frame: ConsoleFrame): string | null {
  const identity = frame.identity?.trim();
  return identity && identity !== "_system" ? identity : null;
}

function toolNameOf(data: Data): string {
  return textValue(data.name) || textValue(data.tool_name);
}

function toolCallIdOf(data: Data): string {
  return textValue(data.id) || textValue(data.tool_call_id) || textValue(data.tool_use_id);
}

interface Parts {
  family: FrameFamily;
  severity?: FrameSeverity;
  tone: Tone;
  title: string;
  verb: string;
  segments: FrameSegment[];
  actor?: string | null;
  target?: string;
  snippet?: string;
  span?: FrameSpan;
  durationMs?: number;
  eventKey?: string;
  internal?: boolean;
}

function severityForTone(tone: Tone): FrameSeverity {
  if (tone === "crit") return "crit";
  if (tone === "warn") return "warn";
  return "info";
}

function runSpan(frame: ConsoleFrame, data: Data, edge: "start" | "end", outcome?: FrameSpan["outcome"]): FrameSpan | undefined {
  const agent = frameAgent(frame);
  if (!agent) return undefined;
  const runId = runIdOf(frame, data);
  const interactionId = interactionIdOf(frame, data);
  const key = runId ? `run:${agent}:${runId}` : interactionId ? `run:${agent}:interaction:${interactionId}` : "";
  if (!key) return undefined;
  return {
    kind: "run",
    key,
    edge,
    ...(outcome ? { outcome } : {}),
    ...(runId ? { runId } : {}),
    ...(interactionId ? { interactionId } : {}),
  };
}

/** A failed turn's typed reason, as a word to compare: the report's class,
 * a typed reason's kind, or a flat reason. */
function failureWord(data: Data): string {
  const report = recordOf(data.error_report);
  const reason = recordOf(data.reason);
  return normalizeStatusWord(
    textValue(report.class)
    || textValue(reason.kind)
    || textValue(reason.reason_type)
    || textValue(data.reason),
  );
}

function classifyTurn(frame: ConsoleFrame, data: Data, who: FrameSegment): Parts | null {
  const event = frame.event;
  const agent = frameAgent(frame);
  if (event === "user_input") {
    // The typed origin names the sender: you, another operator, a
    // scheduled run, a connector.
    const words = turnSenderWords(turnSender(data));
    return {
      family: "turn",
      tone: "info",
      title: "Turn requested",
      verb: "got a request",
      segments: words.after ? [words.before, who, words.after] : [words.before, who],
      snippet: snippetOf(textOf(data.content ?? data.input ?? data.prompt ?? data.text)),
      eventKey: `input:${frame.id || interactionIdOf(frame, data)}`,
    };
  }
  if (event === "interaction_started") {
    return {
      family: "turn",
      tone: "info",
      title: "Turn started",
      verb: "started a turn",
      segments: [who, " started a turn"],
      snippet: snippetOf(textOf(data.content ?? data.text ?? data.prompt ?? data.input)),
      span: runSpan(frame, data, "start"),
      eventKey: `turn-start:${agent ?? ""}:${runIdOf(frame, data) || interactionIdOf(frame, data) || frame.id}`,
    };
  }
  if (event === "run_started") {
    return {
      family: "turn",
      tone: "info",
      title: "Turn started",
      verb: "started a turn",
      segments: [who, " started a turn"],
      span: runSpan(frame, data, "start"),
      eventKey: `turn-start:${agent ?? ""}:${runIdOf(frame, data) || interactionIdOf(frame, data) || frame.id}`,
      // The run's request already lists (user_input, a message); its end lists its outcome.
      internal: true,
    };
  }
  if (RUN_END_OK_EVENTS.has(event)) {
    const runId = runIdOf(frame, data);
    return {
      family: "turn",
      tone: "ok",
      title: "Turn finished",
      verb: "finished a turn",
      segments: [who, " finished a turn"],
      snippet: snippetOf(textOf(data.result ?? data.text ?? data.message ?? data.content)),
      span: runSpan(frame, data, "end", "ok"),
      eventKey: `turn-end:${agent ?? ""}:${runId || interactionIdOf(frame, data) || frame.id}`,
    };
  }
  if (RUN_END_FAILED_EVENTS.has(event) || RUN_CANCELLED_EVENTS.has(event)) {
    const runId = runIdOf(frame, data);
    const word = RUN_CANCELLED_EVENTS.has(event) ? "cancelled" : failureWord(data);
    const eventKey = `turn-end:${agent ?? ""}:${runId || interactionIdOf(frame, data) || frame.id}`;
    const reason = snippetOf(describeFailure(data, ""));
    if (QUIET_FAILURES.has(word)) {
      // Stopped by someone, replaced by a later turn, or ended by a
      // lifecycle action: nothing went wrong.
      const verb = word === "superseded_by_later_run" ? "turn was replaced by a later one"
        : word === "lifecycle_mutation" ? "turn ended with a lifecycle change"
        : "turn was stopped";
      return {
        family: "turn",
        tone: "neutral",
        title: word === "superseded_by_later_run" ? "Turn replaced" : word === "lifecycle_mutation" ? "Turn ended" : "Turn stopped",
        verb,
        segments: [who, `'s ${verb}`],
        span: runSpan(frame, data, "end", "cancelled"),
        eventKey,
      };
    }
    if (word === "queue_overflow") {
      return {
        family: "turn",
        tone: "warn",
        title: "Turn dropped",
        verb: "turn was dropped: the queue was full",
        segments: [who, "'s turn was dropped: the queue was full"],
        snippet: snippetOf(textOf(data.content)),
        span: runSpan(frame, data, "end", "failed"),
        eventKey,
      };
    }
    return {
      family: "turn",
      tone: "crit",
      title: "Turn failed",
      verb: "turn failed",
      segments: [who, "'s turn failed"],
      snippet: reason,
      span: runSpan(frame, data, "end", "failed"),
      eventKey,
    };
  }
  if (event === "turn_started" || event === "turn_completed") {
    const runId = runIdOf(frame, data) || interactionIdOf(frame, data);
    const start = event === "turn_started";
    const turnNumber = numberValue(data.turn_number);
    return {
      family: "turn",
      tone: "neutral",
      title: start ? "Model call started" : "Model call finished",
      verb: start ? "called the model" : "got the model's answer",
      segments: [who, start ? " called the model" : " got the model's answer"],
      ...(agent ? { span: { kind: "turn" as const, key: `turn:${agent}:${runId}`, edge: start ? "start" as const : "end" as const, ...(runId ? { runId } : {}) } } : {}),
      ...(start && turnNumber !== undefined ? { eventKey: `model-call:${agent ?? ""}:${runId}:${turnNumber}` } : {}),
      internal: true,
    };
  }
  if (event === "interaction_callback_pending") {
    const tool = textValue(data.tool_name);
    return {
      family: "turn",
      tone: "info",
      title: "Waiting for a result",
      verb: tool ? `is waiting for ${humanizeEnum(tool).toLowerCase()}` : "is waiting for a tool result",
      segments: [who, tool ? ` is waiting for ${humanizeEnum(tool).toLowerCase()}` : " is waiting for a tool result"],
    };
  }
  if (event === "assistant_image" || event === "assistant_image_appended") {
    const image = recordOf(data.image);
    return {
      family: "turn",
      tone: "ok",
      title: "Image generated",
      verb: "generated an image",
      segments: [who, " generated an image"],
      snippet: snippetOf(textValue(data.revised_prompt) || textValue(data.prompt) || textValue(image.prompt) || textValue(image.alt)),
      eventKey: `image:${textValue(data.image_id) || textValue(image.image_id) || textValue(data.blob_id) || frame.id}`,
    };
  }
  if (event === "message_delivery_failed") {
    return {
      family: "turn",
      tone: "crit",
      title: "Delivery failed",
      verb: "did not get a message",
      segments: ["A message to ", who, " was not delivered"],
      snippet: snippetOf(textValue(data.reason) || describeFailure(data, "")),
      eventKey: `delivery-failed:${interactionIdOf(frame, data) || frame.id}`,
    };
  }
  if (event === "retrying") {
    return {
      family: "turn",
      tone: "info",
      title: "Retrying",
      verb: "is retrying the model",
      segments: [who, " is retrying the model"],
    };
  }
  if (event === "extraction_failed") {
    return {
      family: "turn",
      tone: "warn",
      title: "Structured output failed",
      verb: "could not produce its structured output",
      segments: [who, " could not produce its structured output"],
      snippet: snippetOf(textValue(data.reason)),
    };
  }
  if (event === "budget_warning") {
    const used = numberValue(data.percent);
    const budget = humanizeEnum(textValue(data.budget_type)).toLowerCase();
    return {
      family: "turn",
      tone: "warn",
      title: "Budget nearly used",
      verb: budget ? `has used most of its ${budget} budget` : "has used most of its budget",
      segments: [who, budget ? ` has used most of its ${budget} budget` : " has used most of its budget"],
      snippet: used !== undefined ? `${Math.round(used)}% used` : undefined,
    };
  }
  if (event === "compaction_started" || event === "compaction_completed" || event === "compaction_failed") {
    const failed = event === "compaction_failed";
    return {
      family: "turn",
      tone: failed ? "warn" : event === "compaction_completed" ? "ok" : "neutral",
      title: failed ? "Compaction failed" : event === "compaction_completed" ? "Conversation compacted" : "Compacting the conversation",
      verb: failed ? "could not compact its conversation" : event === "compaction_completed" ? "compacted its conversation" : "is compacting its conversation",
      segments: [who, failed ? " could not compact its conversation" : event === "compaction_completed" ? " compacted its conversation" : " is compacting its conversation"],
      snippet: failed ? snippetOf(textOf(data.reason) || describeFailure(data, "")) : undefined,
    };
  }
  if (event === "stream_truncated") {
    return {
      family: "turn",
      tone: "warn",
      title: "Live stream cut",
      verb: "lost part of its live stream",
      segments: [who, " lost part of its live stream"],
      snippet: snippetOf(humanizeEnum(textValue(data.reason) || textValue(recordOf(data.reason).kind))),
    };
  }
  return null;
}

function classifyTool(frame: ConsoleFrame, data: Data, who: FrameSegment, directory: AgentDirectory | undefined): Parts | null {
  const event = frame.event;
  const agent = frameAgent(frame);
  const name = toolNameOf(data);
  const callId = toolCallIdOf(data);
  const callKey = `${agent ?? ""}:${callId || frame.id}`;
  const toolWords = humanizeEnum(name).toLowerCase() || "a tool";
  const toolSpan = (edge: "start" | "end", outcome?: FrameSpan["outcome"]): FrameSpan | undefined =>
    agent && callId ? { kind: "tool", key: `tool:${agent}:${callId}`, edge, ...(outcome ? { outcome } : {}) } : undefined;

  if (event === "tool_call_requested" || event === "tool_call") {
    // The request lists the call; "tool_call" is the same call again.
    const repeat = event === "tool_call";
    if (PEER_TOOLS.has(name)) return { ...classifyPeerTool(frame, data, who, name, directory), internal: repeat };
    if (name.startsWith("workgraph_")) return { ...classifyWorkGraphTool(data, who, name, callKey), internal: repeat };
    return {
      family: "tool",
      tone: "neutral",
      title: "Tool used",
      verb: `used ${toolWords}`,
      segments: [who, ` used ${toolWords}`],
      eventKey: `tool:${callKey}`,
      internal: repeat,
    };
  }
  if (event === "tool_execution_started") {
    return {
      family: "tool",
      tone: "neutral",
      title: "Tool started",
      verb: `started ${toolWords}`,
      segments: [who, ` started ${toolWords}`],
      span: toolSpan("start"),
      eventKey: `tool-start:${callKey}`,
      internal: true,
    };
  }
  if (event === "tool_execution_timed_out") {
    const timeout = numberValue(data.timeout_ms);
    return {
      family: "tool",
      tone: "warn",
      title: "Tool timed out",
      verb: `${toolWords} timed out`,
      segments: [who, `'s ${toolWords} timed out`],
      snippet: timeout !== undefined ? `No result after ${formatSeconds(timeout)}` : undefined,
      span: toolSpan("end", "timed_out"),
      eventKey: `tool-problem:${callKey}`,
    };
  }
  if (event === "tool_execution_completed" || event === "tool_result_received") {
    const failed = data.is_error === true;
    const durationMs = numberValue(data.duration_ms);
    return {
      family: name.startsWith("workgraph_") ? "workgraph" : "tool",
      tone: failed ? "warn" : "ok",
      title: failed ? "Tool failed" : "Tool finished",
      verb: failed ? `${toolWords} failed` : `finished ${toolWords}`,
      segments: failed ? [who, `'s ${toolWords} failed`] : [who, ` finished ${toolWords}`],
      snippet: failed ? snippetOf(textOf(data.content ?? data.result) || describeFailure(data, "")) : undefined,
      ...(event === "tool_execution_completed" ? { span: toolSpan("end", failed ? "failed" : "ok") } : {}),
      ...(durationMs !== undefined ? { durationMs } : {}),
      // One copy of a failure lists: both events report it, under one key.
      eventKey: failed ? `tool-problem:${callKey}` : `tool-done:${event}:${callKey}`,
      internal: !failed,
    };
  }
  if (event === "background_job_completed") {
    const status = normalizeStatusWord(textValue(data.terminal_status) || textValue(recordOf(data.terminal_status).kind));
    const failed = ["failed", "failure", "error", "errored", "timed_out"].includes(status);
    const job = textValue(data.display_name) || "a background job";
    return {
      family: "tool",
      tone: failed ? "warn" : status === "cancelled" || status === "canceled" ? "neutral" : "ok",
      title: failed ? "Background job failed" : "Background job finished",
      verb: failed ? `background job ${job} failed` : `finished ${job}`,
      segments: failed ? [who, `'s background job ${job} failed`] : [who, ` finished ${job}`],
      snippet: snippetOf(textValue(data.detail)),
      eventKey: `job:${agent ?? ""}:${textValue(data.job_id) || frame.id}`,
    };
  }
  return null;
}

function formatSeconds(ms: number): string {
  const seconds = ms / 1000;
  if (seconds < 60) return `${Number.isInteger(seconds) ? seconds : seconds.toFixed(1)} s`;
  const minutes = Math.round(seconds / 60);
  return `${minutes} min`;
}

/** The recipient a peer tool call names, in the words it uses (never the opaque peer id). */
function peerTargetOf(args: Data): string {
  for (const key of ["display_name", "to", "identity", "target_identity", "recipient"]) {
    const value = textValue(args[key]);
    if (value) return value;
  }
  return "";
}

function classifyPeerTool(frame: ConsoleFrame, data: Data, who: FrameSegment, name: string, directory: AgentDirectory | undefined): Parts {
  const args = recordOf(data.args);
  const reference = peerTargetOf(args);
  const target = reference ? peerKey(reference, directory) : "";
  const verb = name === "send_request" ? "asked" : name === "send_response" || name === "reply_to_peer" ? "replied to" : "messaged";
  const agent = frameAgent(frame);
  return {
    family: "message",
    tone: "neutral",
    title: name === "send_request" ? "Request sent" : verb === "replied to" ? "Reply sent" : "Message sent",
    verb: target ? verb : `${verb} a peer`,
    segments: [who, ` ${verb} `, target ? { agent: target } : "a peer"],
    actor: agent,
    ...(target ? { target } : {}),
    snippet: snippetOf(textOf(args.body ?? args.params ?? args.result ?? args.intent ?? args.blocks)),
    eventKey: `tool:${agent ?? ""}:${toolCallIdOf(data) || frame.id}`,
  };
}

/** WorkGraph tool calls in words: [verb, title]. Reads are "checked the work graph". */
const WORKGRAPH_WORDS: Readonly<Record<string, readonly [string, string]>> = {
  workgraph_create: ["created a work item", "Work item created"],
  workgraph_ensure_items: ["added work items", "Work items added"],
  workgraph_claim: ["claimed a work item", "Work item claimed"],
  workgraph_close: ["closed a work item", "Work item closed"],
  workgraph_release: ["released a work item", "Work item released"],
  workgraph_block: ["marked a work item blocked", "Work item blocked"],
  workgraph_update: ["updated a work item", "Work item updated"],
  workgraph_link: ["linked work items", "Work items linked"],
  workgraph_add_evidence: ["added evidence to a work item", "Evidence added"],
  workgraph_attention_reassign: ["reassigned work", "Work reassigned"],
  workgraph_policy_escalate: ["escalated a work item", "Work item escalated"],
};

function classifyWorkGraphTool(data: Data, who: FrameSegment, name: string, callKey: string): Parts {
  const [verb, title] = WORKGRAPH_WORDS[name] ?? ["checked the work graph", "Work graph read"];
  const args = recordOf(data.args);
  return {
    family: "workgraph",
    tone: name === "workgraph_close" ? "ok" : name === "workgraph_block" || name === "workgraph_policy_escalate" ? "warn" : "neutral",
    title,
    verb,
    segments: [who, ` ${verb}`],
    snippet: snippetOf(textValue(args.title) || textValue(args.summary) || textValue(args.reason)),
    eventKey: `tool:${callKey}`,
  };
}

/** The comms blocks of a system notice (or of a saved message carrying one). */
function commsBlocks(data: Data): Data[] {
  const blocks: Data[] = [];
  for (const candidate of [data, recordOf(data.message)]) {
    if (textValue(candidate.type) === "comms") blocks.push(candidate);
    if (!Array.isArray(candidate.blocks)) continue;
    for (const block of candidate.blocks) {
      const record = recordOf(block);
      if (textValue(record.type) === "comms") blocks.push(record);
    }
  }
  return blocks;
}

function classifySystemNotice(frame: ConsoleFrame, data: Data, who: FrameSegment, directory: AgentDirectory | undefined): Parts | null {
  const blocks = commsBlocks(data);
  if (blocks.length === 0) {
    return {
      family: "system",
      tone: "neutral",
      title: "Notice",
      verb: "got a notice",
      segments: [who, " got a notice"],
      snippet: snippetOf(textOf(data)),
      internal: true,
    };
  }
  // A member-kickoff status is its typed phase, not a peer message.
  const kickoff = blocks.map(memberKickoffNotice).find((notice) => notice !== null);
  if (kickoff) {
    const failed = kickoff.phase === "failed";
    const member = peerKey(kickoff.member, directory) || kickoff.member;
    const phase = memberKickoffPhaseLabel(kickoff.phase).toLowerCase();
    return {
      family: "lifecycle",
      tone: failed ? "warn" : "neutral",
      title: `Kickoff ${phase}`,
      verb: `kickoff ${phase}`,
      segments: [{ agent: member }, `'s kickoff ${phase}`],
      actor: member,
      snippet: kickoff.role ? humanizeEnum(kickoff.role) : undefined,
      eventKey: `kickoff:${member}:${kickoff.phase}:${frame.id}`,
    };
  }
  const block = blocks.find((candidate) => textValue(candidate.kind) !== "lifecycle" && textValue(candidate.kind) !== "response_progress");
  if (!block) {
    return {
      family: "message",
      tone: "neutral",
      title: "Peer notice",
      verb: "got a peer notice",
      segments: [who, " got a peer notice"],
      internal: true,
    };
  }
  const peer = recordOf(block.peer);
  const peerReference = textValue(peer.display_name) || textValue(peer.id);
  const peerAgent = peerReference ? peerKey(peerReference, directory) : "";
  const receiver = frameAgent(frame);
  const outgoing = textValue(block.direction) === "outgoing";
  const sender = outgoing ? receiver : peerAgent || null;
  const recipient = outgoing ? peerAgent : receiver ?? "";
  const kind = textValue(block.kind);
  const verb = kind === "request" ? "asked" : kind === "response_terminal" ? "replied to" : "messaged";
  const content = stripPeerTransportScaffold(textOf(block.content));
  const detail = content || textValue(block.summary) || textValue(block.intent) || textOf(block.payload);
  return {
    family: "message",
    tone: textValue(block.status) === "failed" ? "warn" : "neutral",
    title: kind === "request" ? "Request" : kind === "response_terminal" ? "Reply" : "Message",
    verb: recipient ? verb : `${verb} a peer`,
    segments: [sender ? { agent: sender } : "A peer", ` ${verb} `, recipient ? { agent: recipient } : "a peer"],
    actor: sender,
    ...(recipient ? { target: recipient } : {}),
    snippet: snippetOf(detail),
    // The envelope's delivery id names one message in every copy of its notice.
    eventKey: `comms:${receiver ?? ""}:${outgoing ? "out" : "in"}:${textValue(block.delivery_id) || textValue(block.request_id) || frame.id}`,
  };
}

function classifyApproval(data: Data, who: FrameSegment): Parts {
  const decision = normalizeStatusWord(textValue(data.decision));
  const action = textValue(data.action_id) ? humanizeEnum(textValue(data.action_id)) : "";
  const snippet = action || snippetOf(textValue(data.reason));
  if (decision === "requested" || decision === "pending") {
    return { family: "approval", tone: "warn", title: "Approval requested", verb: "asked for approval", segments: [who, " asked for approval"], snippet };
  }
  if (["approve", "approved", "auto_approved", "allowed"].includes(decision)) {
    return { family: "approval", tone: "ok", title: "Approved", verb: "got approval", segments: ["Approval granted to ", who], snippet };
  }
  if (["reject", "rejected", "denied"].includes(decision)) {
    return { family: "approval", severity: "warn", tone: "crit", title: "Approval refused", verb: "was refused approval", segments: ["Approval refused for ", who], snippet };
  }
  if (decision === "escalate" || decision === "escalated") {
    return { family: "approval", tone: "warn", title: "Approval escalated", verb: "had an approval escalated", segments: ["Approval for ", who, " was escalated"], snippet };
  }
  return {
    family: "approval",
    tone: "neutral",
    title: decision ? `Approval ${humanizeEnum(decision).toLowerCase()}` : "Approval decision",
    verb: "had an approval decided",
    segments: ["Approval decision for ", who, decision ? `: ${humanizeEnum(decision).toLowerCase()}` : ""],
    snippet,
  };
}

function classifyLifecycle(frame: ConsoleFrame, data: Data, who: FrameSegment): Parts | null {
  const reason = snippetOf(textOf(data.reason));
  switch (frame.event) {
    case "member_ready":
      return { family: "lifecycle", tone: "ok", title: "Ready", verb: "is ready", segments: [who, " is ready"] };
    case "member_spawned":
      return { family: "lifecycle", tone: "info", title: "Joined", verb: "joined the team", segments: [who, " joined the team"] };
    case "member_retired":
      return { family: "lifecycle", tone: "neutral", title: "Member retired", verb: "was retired", segments: [who, " was retired"], snippet: reason };
    case "identity_retired":
      return { family: "lifecycle", tone: "neutral", title: "Retired", verb: "was retired", segments: [who, " was retired"], snippet: reason };
    case "identity_respawned": {
      const generation = numberValue(data.generation);
      const warning = textOf(data.live_respawn_warning) || textOf(data.cleanup_warning) || textOf(data.topology_restore_warning);
      return {
        family: "lifecycle",
        tone: warning ? "warn" : "info",
        title: "Respawned",
        verb: "was respawned",
        segments: [who, " was respawned"],
        snippet: snippetOf(warning) ?? (generation !== undefined ? `Generation ${generation}` : undefined),
      };
    }
    case "identity_reset":
      return {
        family: "lifecycle",
        tone: textOf(data.topology_restore_warning) ? "warn" : "info",
        title: "Reset",
        verb: "was reset",
        segments: [who, " was reset"],
        snippet: snippetOf(textOf(data.topology_restore_warning)),
      };
    case "identity_deleted":
      return { family: "lifecycle", tone: "neutral", title: "Deleted", verb: "was deleted", segments: [who, " was deleted"] };
    case "member_retirement_started":
      return { family: "lifecycle", tone: "neutral", title: "Retiring", verb: "is retiring", segments: [who, " is retiring"] };
    case "member_reset":
      return { family: "lifecycle", tone: "info", title: "Reset", verb: "was reset", segments: [who, " was reset"] };
    case "state_changed": {
      const state = textOf(data.state ?? data.new_state);
      const word = normalizeStatusWord(state);
      const tone: Tone = word === "broken" || word === "stalled" || word === "wedged" ? "crit"
        : word === "degraded" || word === "needs_repair" ? "warn" : "neutral";
      return {
        family: "lifecycle",
        tone,
        title: "State changed",
        verb: state ? `is now ${humanizeEnum(state).toLowerCase()}` : "changed state",
        segments: [who, state ? ` is now ${humanizeEnum(state).toLowerCase()}` : " changed state"],
      };
    }
    default:
      return null;
  }
}

/** A memory scope in words, agents by key: [{ agent }, "'s memory"], "team memory". */
function memoryScopeSegments(data: Data): FrameSegment[] {
  const key = textValue(data.scope_key);
  switch (textValue(data.scope_kind)) {
    case "identity": return key ? [{ agent: key }, "'s memory"] : ["an agent's memory"];
    case "mob": return ["team memory"];
    case "operator": return ["operator memory"];
    case "realm": return ["shared memory"];
    default: return ["another memory scope"];
  }
}

function classifyMemory(frame: ConsoleFrame, data: Data, who: FrameSegment): Parts {
  const event = frame.event;
  const reason = snippetOf(textOf(data.reason));
  const parts = (tone: Tone, title: string, segments: FrameSegment[], verb: string, snippet?: string): Parts =>
    ({ family: "memory", tone, title, verb, segments, ...(snippet ? { snippet } : {}) });
  switch (event) {
    case "memory.dream.completed": return parts("ok", "Memory dream completed", [who, " finished a memory dream"], "finished a memory dream");
    case "memory.dream.started": return parts("neutral", "Memory dream started", [who, " started a memory dream"], "started a memory dream");
    case "memory.dream.skipped": return parts("neutral", "Memory dream skipped", [who, " skipped a memory dream"], "skipped a memory dream", reason);
    case "memory.record.promoted": return parts("ok", "Memory record moved", [who, " moved a memory record"], "moved a memory record");
    case "memory.write.quarantined":
      return parts("warn", "Memory write quarantined", ["A memory write from ", who, " was quarantined"], "had a memory write quarantined", reason);
    case "memory.quarantine.verdict": {
      // The steward's open-loop nudge rides the verdict event; nobody
      // reviewed anything.
      if (isOpenLoopNudge(event, data)) return parts("neutral", "Open loop needs follow-up", ["An open loop needs follow-up"], "flagged an open loop");
      const verdict = textOf(data.verdict);
      return parts("neutral", "Quarantine verdict", [who, " reviewed a quarantined record"], "reviewed a quarantined record", verdict ? quarantineVerdictWords(verdict) : undefined);
    }
    case "memory.quarantine.release_blocked":
      return parts("warn", "Quarantine release blocked", ["A quarantine release by ", who, " was blocked"], "had a quarantine release blocked");
    case "memory.conflict.signal": return parts("warn", "Conflicting memories", [who, " flagged a memory conflict"], "flagged a memory conflict", reason);
    case "memory.budget.denied": return parts("warn", "Memory budget reached", [who, " reached the memory budget"], "reached the memory budget");
    case "memory.hygiene.blocked": return parts("warn", "Memory tidy-up blocked", [who, "'s memory tidy-up was blocked"], "had a memory tidy-up blocked", reason);
    case "memory.distill.timed_out": return parts("warn", "Summarizing timed out", [who, "'s conversation summary timed out"], "had a conversation summary time out");
    case "memory.taint.transition":
      return data.kind === "tainted"
        ? parts("warn", "Session became untrusted", [who, "'s session became untrusted"], "had its session become untrusted")
        : parts("neutral", "Session trust changed", [who, `'s session memory: ${humanizeEnum(textOf(data.kind) || "changed").toLowerCase()}`], "had its session trust change");
    case "memory.harvest.completed": {
      const retired = textValue(data.identity);
      return parts("info", "Retired agent's memory summarized", retired ? [{ agent: retired }, "'s memory was summarized"] : ["A retired agent's memory was summarized"], "had its memory summarized");
    }
    case "memory.promotion.pending_gate":
      return parts("info", "Memory record waiting to move", ["A record waits for approval to move to ", ...memoryScopeSegments(data)], "has a record waiting to move");
    default:
      return parts("neutral", humanizeEnum(event), [who, `: memory ${humanizeEnum(event.slice("memory.".length)).toLowerCase()}`], `memory ${humanizeEnum(event.slice("memory.".length)).toLowerCase()}`);
  }
}

function classifyRouting(frame: ConsoleFrame, data: Data): Parts | null {
  switch (frame.event) {
    case "route_changed":
      return { family: "routing", tone: "info", title: "Route changed", verb: "changed a route", segments: ["Routing changed"], snippet: snippetOf(textOf(data.reason)) };
    case "topology_updated":
      return { family: "routing", tone: "info", title: "Team layout changed", verb: "changed the team layout", segments: ["The team layout changed"] };
    case "members_wired":
    case "members_wired_batch":
    case "external_peer_wired":
      return { family: "routing", tone: "info", title: "Agents connected", verb: "connected agents", segments: ["Agents were connected"] };
    case "members_unwired":
    case "external_peer_unwired":
      return { family: "routing", tone: "info", title: "Agents disconnected", verb: "disconnected agents", segments: ["Agents were disconnected"] };
    default:
      return null;
  }
}

function classifySystem(frame: ConsoleFrame, data: Data, who: FrameSegment): Parts {
  const event = frame.event;
  if (event === "hook_failed" || event === "hook_denied" || event === "hook_launch_refused") {
    const point = humanizeEnum(textValue(data.point)).toLowerCase();
    const title = event === "hook_denied" ? "Hook denied" : event === "hook_launch_refused" ? "Hook refused to start" : "Hook failed";
    return {
      family: "system",
      tone: "warn",
      title,
      verb: title.toLowerCase(),
      segments: [who, `: ${title.toLowerCase()}${point ? ` at ${point}` : ""}`],
      snippet: snippetOf(textValue(data.message) || textOf(data.reason)),
    };
  }
  if (event === "skill_resolution_failed" || event === "model_fallback_target_failed" || event === "operation_observation_failed") {
    return {
      family: "system",
      tone: "warn",
      title: humanizeEnum(event),
      verb: humanizeEnum(event).toLowerCase(),
      segments: [who, `: ${humanizeEnum(event).toLowerCase()}`],
      snippet: snippetOf(describeFailure(data, "") || textOf(data.reason)),
    };
  }
  if (event.startsWith("model_fallback_")) {
    return { family: "system", tone: "info", title: humanizeEnum(event), verb: humanizeEnum(event).toLowerCase(), segments: [who, `: ${humanizeEnum(event).toLowerCase()}`] };
  }
  if (event === "flow_started" || event === "flow_completed" || event === "flow_failed" || event === "flow_canceled" || event === "step_target_failed") {
    const failed = event === "flow_failed" || event === "step_target_failed";
    const words = event === "flow_started" ? "A flow started" : event === "flow_completed" ? "A flow finished"
      : event === "flow_canceled" ? "A flow was cancelled" : event === "flow_failed" ? "A flow failed" : "A flow step failed";
    return {
      family: "system",
      tone: failed ? "warn" : event === "flow_completed" ? "ok" : "neutral",
      title: words.replace(/^A /, "").replace(/^flow/, "Flow"),
      verb: words.replace(/^A /, "").toLowerCase(),
      segments: [words],
      snippet: failed ? snippetOf(describeFailure(data, "") || textOf(data.reason)) : undefined,
    };
  }
  if (event === "live_channel_closed") {
    return { family: "system", tone: "neutral", title: "Live channel closed", verb: "closed its live channel", segments: [who, " closed its live channel"], snippet: snippetOf(humanizeEnum(textValue(data.reason))) };
  }
  if (event.startsWith("workgraph.")) {
    return {
      family: "workgraph",
      tone: "neutral",
      title: `WorkGraph ${humanizeEnum(event.slice("workgraph.".length)).toLowerCase()}`,
      verb: `work graph ${humanizeEnum(event.slice("workgraph.".length)).toLowerCase()}`,
      segments: [who, `: work graph ${humanizeEnum(event.slice("workgraph.".length)).toLowerCase()}`],
    };
  }
  // Anything else names itself; a frame that says it failed is a problem.
  const { message } = summarizeFailureData(data);
  const failed = data.is_error === true || Boolean(recordOf(data.error_report).class);
  return {
    family: "system",
    tone: failed ? "crit" : "neutral",
    title: humanizeEnum(event) || "Event",
    verb: humanizeEnum(event).toLowerCase(),
    segments: [who, `: ${humanizeEnum(event).toLowerCase()}`],
    snippet: failed ? snippetOf(message) : undefined,
  };
}

function classifyUncached(frame: ConsoleFrame, directory: AgentDirectory | undefined): ClassifiedFrame {
  const data = recordOf(frame.data);
  const agent = frameAgent(frame);
  const who: FrameSegment = { agent };
  const event = frame.event;
  let parts: Parts | null = null;
  if (event === "system_notice") parts = classifySystemNotice(frame, data, who, directory);
  else if (event === "gating_decision") parts = classifyApproval(data, who);
  else if (event.startsWith("memory.")) parts = classifyMemory(frame, data, who);
  parts ??= classifyTurn(frame, data, who)
    ?? classifyTool(frame, data, who, directory)
    ?? classifyLifecycle(frame, data, who)
    ?? classifyRouting(frame, data);
  parts ??= classifySystem(frame, data, who);
  const internal = parts.internal ?? INTERNAL_EVENTS.has(event);
  return {
    family: parts.family,
    familyWord: FAMILY_WORD[parts.family],
    severity: parts.severity ?? severityForTone(parts.tone),
    tone: parts.tone,
    title: parts.title,
    verb: parts.verb,
    segments: parts.segments,
    actor: parts.actor !== undefined ? parts.actor : agent,
    ...(parts.target ? { target: parts.target } : {}),
    ...(parts.snippet ? { snippet: parts.snippet } : {}),
    ...(parts.span ? { span: parts.span } : {}),
    ...(parts.durationMs !== undefined ? { durationMs: parts.durationMs } : {}),
    eventKey: parts.eventKey ?? `frame:${frame.id || `${event}:${frame.identity ?? ""}:${frame.timestampMs ?? ""}`}`,
    internal,
  };
}

const CLASSIFIED = new WeakMap<ConsoleFrame, { directory: AgentDirectory | undefined; classified: ClassifiedFrame }>();

/**
 * Read one frame: its family, severity, words, span and key. Memoised per
 * frame object (and directory), so a list classifies each frame once. A
 * `user_input` frame is read afresh: whether "You" sent it can change.
 */
export function classifyFrame(frame: ConsoleFrame, directory?: AgentDirectory): ClassifiedFrame {
  if (frame.event === "user_input") return classifyUncached(frame, directory);
  const known = CLASSIFIED.get(frame);
  if (known && known.directory === directory) return known.classified;
  const classified = classifyUncached(frame, directory);
  CLASSIFIED.set(frame, { directory, classified });
  return classified;
}

/** The sentence in words, agents named by `nameOf` ("System" for none). */
export function frameSentence(classified: Pick<ClassifiedFrame, "segments">, nameOf: (agent: string) => string): string {
  return classified.segments
    .map((segment) => (typeof segment === "string" ? segment : segment.agent === null ? "System" : nameOf(segment.agent)))
    .join("");
}

/** Whether a frame is a problem worth a mark: a warning or a failure. */
export function isProblemFrame(classified: Pick<ClassifiedFrame, "severity" | "internal">): boolean {
  return !classified.internal && classified.severity !== "info";
}
