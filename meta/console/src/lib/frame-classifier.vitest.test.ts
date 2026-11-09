import { describe, expect, it } from "vitest";

import { createAgentDirectory } from "../../../packages/console-components/src/panel-kit/agents";
import { describeTodayFrame } from "../panels/TimelinePanel";
import { createActivityLedger, createAgentResolver } from "./activity-ledger";
import { stripPeerTransportScaffold } from "./adapters";
import { rememberOwnSend } from "./turn-origin";
import type { ConsoleFrame } from "../types";
import { classifyFrame, failureWords, frameSentence, isProblemFrame, plainSnippetText, toolFailureWords } from "./frame-classifier";

let serial = 0;
function frame(event: string, data: Record<string, unknown> = {}, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  serial += 1;
  return { id: `f-${serial}`, event, identity: "domain:billing", timestampMs: 1_000 + serial, data, ...extra };
}

const asKeys = (agent: string) => agent;
const sentence = (f: ConsoleFrame) => frameSentence(classifyFrame(f), asKeys);

describe("the classifier speaks Today's words", () => {
  it("gives the sentence, tone and excerpt Today shows for the events it names", () => {
    rememberOwnSend("classifier-own-send");
    const corpus: ConsoleFrame[] = [
      frame("user_input", { content: "Nightly check", origin: "console:schedule", origin_kind: "scheduler" }),
      frame("user_input", { content: "Customer email", origin: "mail-bridge", origin_kind: "connector" }),
      frame("user_input", { content: "Backlog", origin: "console:pane-1", origin_kind: "operator", idempotency_key: "someone-else" }),
      frame("user_input", { content: "Summary", origin: "console:pane-2", origin_kind: "operator", idempotency_key: "classifier-own-send" }),
      frame("user_input", { content: "Probe", origin: "release-gate", origin_kind: "operator_probe" }),
      frame("interaction_started", { content: "Draft the refund reply" }),
      frame("interaction_complete", { result: "Replied to the refund email." }),
      frame("interaction_failed", { error_report: { class: "llm", message: "LLM error: authentication failed (401)" } }),
      frame("run_failed", { error: "Payment API timed out after 30 s" }),
      frame("gating_decision", { decision: "requested", action_id: "deploy_release" }),
      frame("gating_decision", { decision: "approved", action_id: "deploy_release" }),
      frame("gating_decision", { decision: "rejected" }),
      frame("gating_decision", { decision: "escalated" }),
      frame("gating_decision", { decision: "timed_out" }),
      frame("member_ready"),
      frame("member_retired", { reason: "Finished its task" }),
      frame("state_changed", { state: "paused" }),
      frame("state_changed", {}),
      frame("route_changed", { reason: "New email route" }),
      frame("topology_updated"),
      frame("tool_call_requested", { id: "c1", name: "send_request", args: { display_name: "team/support", body: "Refund 4411?" } }),
      frame("tool_call_requested", { id: "c2", name: "send_message", args: { to: "router", body: "FYI" } }),
      frame("tool_call_requested", { id: "c3", name: "send_response", args: { display_name: "support", result: "Approved" } }),
      frame("tool_call_requested", { id: "c4", name: "send_message", args: { peer_id: "8f1e" } }),
      frame("tool_call_requested", { id: "c5", name: "web_search", args: { query: "refund policy" } }),
      frame("memory.dream.completed"),
      frame("memory.dream.started"),
      frame("memory.dream.skipped", { reason: "Nothing new" }),
      frame("memory.record.promoted", { scope_kind: "mob", scope_key: "main" }),
      frame("memory.write.quarantined", { reason: "session tainted" }),
      frame("memory.quarantine.verdict", { verdict: "tombstone" }),
      frame("memory.quarantine.verdict", { verdict: "open_loop_escalated", record_id: "rec-1" }),
      frame("memory.quarantine.release_blocked", {}),
      frame("memory.conflict.signal", { reason: "Two prices" }),
      frame("memory.budget.denied", {}),
      frame("memory.taint.transition", { kind: "tainted" }),
      frame("memory.taint.transition", { kind: "reset_boundary" }),
      frame("memory.something_new", {}),
      frame("custom_event", { detail: 1 }),
      frame("member_ready", {}, { identity: "_system" }),
    ];
    for (const f of corpus) {
      const today = describeTodayFrame(f);
      const todaySentence = today.segments.map((segment) => (typeof segment === "string" ? segment : segment.agent ?? "System")).join("");
      const classified = classifyFrame(f);
      expect(sentence(f), f.event).toBe(todaySentence);
      expect(classified.tone, f.event).toBe(today.tone);
      expect(classified.snippet, f.event).toBe(today.snippet);
    }
  });
});

describe("payload-aware severity", () => {
  it("reads a failed turn's typed class and reason, never the event name", () => {
    expect(classifyFrame(frame("interaction_failed", { error_report: { class: "llm", message: "boom" } })).severity).toBe("crit");
    for (const data of [
      { error_report: { class: "cancelled", message: "stopped" } },
      { reason: "superseded_by_later_run" },
      { reason: "lifecycle_mutation", lifecycle_event: "identity_retired" },
    ]) {
      const quiet = classifyFrame(frame("interaction_failed", data, { runId: "run-q" }));
      expect(quiet.severity).toBe("info");
      expect(quiet.tone).toBe("neutral");
      expect(quiet.span?.outcome).toBe("cancelled");
    }
    const dropped = classifyFrame(frame("interaction_failed", { reason: "queue_overflow", content: "Late request" }));
    expect([dropped.severity, dropped.title, dropped.snippet]).toEqual(["warn", "Turn dropped", "Late request"]);
    expect(sentence(frame("run_cancelled"))).toBe("domain:billing's turn was stopped");
    // A name that says failed is not a failure; the payload decides.
    expect(classifyFrame(frame("routing_rehearsal_failed_over")).severity).toBe("info");
    expect(classifyFrame(frame("custom_event", { is_error: true, error: "Disk full" })).severity).toBe("crit");
  });

  it("marks tool errors, timed-out tools, failed deliveries and memory warnings", () => {
    const toolError = classifyFrame(frame("tool_result_received", { id: "t1", name: "web_search", is_error: true, content: [{ type: "text", text: "rate limited" }] }));
    expect([toolError.severity, toolError.internal, toolError.title, toolError.snippet]).toEqual(["warn", false, "Tool failed", "rate limited"]);
    const toolDone = classifyFrame(frame("tool_execution_completed", { id: "t2", name: "web_search", is_error: false, duration_ms: 4200 }));
    expect([toolDone.severity, toolDone.internal, toolDone.durationMs]).toEqual(["info", true, 4200]);
    // A saved copy names no tool.
    expect(sentence(frame("tool_execution_completed", { id: "t2b", is_error: true, content: "denied" }))).toBe("domain:billing's tool call failed");
    const timedOut = classifyFrame(frame("tool_execution_timed_out", { id: "t-timeout", name: "payment_api", timeout_ms: 30_000 }));
    expect([timedOut.severity, timedOut.snippet, timedOut.span?.outcome]).toEqual(["warn", "No result after 30 s", "timed_out"]);
    const delivery = classifyFrame(frame("message_delivery_failed", { reason: "agent is retired" }));
    expect([delivery.severity, delivery.snippet]).toEqual(["crit", "agent is retired"]);
    expect(sentence(frame("message_delivery_failed", { reason: "x" }))).toBe("A message to domain:billing was not delivered");
    for (const event of ["memory.write.quarantined", "memory.budget.denied", "memory.hygiene.blocked", "memory.quarantine.release_blocked", "memory.conflict.signal"]) {
      expect(classifyFrame(frame(event)).severity, event).toBe("warn");
    }
    expect(classifyFrame(frame("memory.taint.transition", { kind: "tainted" })).severity).toBe("warn");
    expect(classifyFrame(frame("memory.taint.transition", { kind: "rotated_clean" })).severity).toBe("info");
    const refused = classifyFrame(frame("gating_decision", { decision: "denied" }));
    expect([refused.severity, refused.tone, refused.title]).toEqual(["warn", "crit", "Approval refused"]);
    expect(isProblemFrame(refused)).toBe(true);
    expect(isProblemFrame(classifyFrame(frame("interaction_complete")))).toBe(false);
  });
});

describe("plumbing frames are internal", () => {
  it("hides snapshots, deltas, updates and bootstraps", () => {
    for (const event of [
      "runtime_notice_snapshot",
      "assistant_history_snapshot",
      "runtime_bootstrapped",
      "boundary_append_applied",
      "boundary_appends_discarded",
      "text_delta",
      "reasoning_delta",
      "reasoning_complete",
      "frame_updated",
      "keep-alive",
      "snapshot_complete",
    ]) {
      expect(classifyFrame(frame(event)).internal, event).toBe(true);
    }
    for (const event of ["interaction_complete", "interaction_failed", "user_input", "gating_decision", "identity_retired"]) {
      expect(classifyFrame(frame(event)).internal, event).toBe(false);
    }
  });
});

describe("lifecycle words", () => {
  it("names what Meta records for an identity", () => {
    expect(sentence(frame("identity_retired"))).toBe("domain:billing was retired");
    const respawned = classifyFrame(frame("identity_respawned", { generation: 3 }));
    expect([frameSentence(respawned, asKeys), respawned.snippet, respawned.family]).toEqual(["domain:billing was respawned", "Generation 3", "lifecycle"]);
    expect(classifyFrame(frame("identity_respawned", { generation: 2, live_respawn_warning: "Peers were not restored" })).tone).toBe("warn");
    expect(sentence(frame("identity_reset"))).toBe("domain:billing was reset");
    expect(sentence(frame("identity_deleted"))).toBe("domain:billing was deleted");
    expect(sentence(frame("member_retired"))).toBe("domain:billing was retired");
    expect(classifyFrame(frame("member_retired")).familyWord).toBe("Lifecycle");
  });
});

describe("spans", () => {
  it("pairs a run's start and end by run id, else by interaction", () => {
    const start = classifyFrame(frame("run_started", {}, { runId: "run-1", interactionId: "i-1" }));
    const end = classifyFrame(frame("interaction_complete", { result: "Done" }, { runId: "run-1", interactionId: "i-1" }));
    expect(start.internal).toBe(true);
    expect(start.span).toEqual({ kind: "run", key: "run:domain:billing:run-1", edge: "start", runId: "run-1", interactionId: "i-1" });
    expect(end.span).toMatchObject({ key: "run:domain:billing:run-1", edge: "end", outcome: "ok" });
    const failed = classifyFrame(frame("interaction_failed", { error: "boom", run_id: "run-2" }));
    expect(failed.span).toMatchObject({ key: "run:domain:billing:run-2", edge: "end", outcome: "failed" });
    expect(classifyFrame(frame("interaction_failed", { reason: "lifecycle_mutation" }, { interactionId: "i-9" })).span)
      .toMatchObject({ key: "run:domain:billing:interaction:i-9", outcome: "cancelled" });
  });

  it("gives tool executions and model calls their own spans", () => {
    expect(classifyFrame(frame("tool_execution_started", { id: "t1", name: "web_search" })).span)
      .toEqual({ kind: "tool", key: "tool:domain:billing:t1", edge: "start" });
    expect(classifyFrame(frame("tool_execution_completed", { id: "t1", name: "web_search", duration_ms: 12, is_error: false })).span)
      .toEqual({ kind: "tool", key: "tool:domain:billing:t1", edge: "end", outcome: "ok" });
    expect(classifyFrame(frame("turn_started", { turn_number: 1 }, { runId: "run-1" })).span)
      .toMatchObject({ kind: "turn", key: "turn:domain:billing:run-1", edge: "start" });
    expect(classifyFrame(frame("turn_completed", { stop_reason: "end_turn" }, { runId: "run-1" })).span)
      .toMatchObject({ kind: "turn", key: "turn:domain:billing:run-1", edge: "end" });
  });

  it("gives one key to the copies of one event", () => {
    const failedResult = classifyFrame(frame("tool_result_received", { id: "t7", name: "x", is_error: true }));
    const failedExecution = classifyFrame(frame("tool_execution_completed", { id: "t7", name: "x", is_error: true, duration_ms: 3 }));
    expect(failedResult.eventKey).toBe(failedExecution.eventKey);
    const completed = classifyFrame(frame("interaction_complete", {}, { runId: "run-5" }));
    const projected = classifyFrame(frame("run_completed", {}, { runId: "run-5" }));
    expect(completed.eventKey).toBe(projected.eventKey);
    // An input's send record and the session's saved copy of it.
    const sent = classifyFrame(frame("user_input", { content: "Reconcile" }, { sourceKind: "send", interactionId: "i-7" }));
    const savedInput = classifyFrame(frame("user_input", { content: "Reconcile" }, { sourceKind: "session_history", interactionId: "i-7" }));
    expect(sent.eventKey).toBe(savedInput.eventKey);
    const live = classifyFrame(frame("tool_call_requested", { id: "c9", name: "web_search" }, { sourceKind: "console_event" }));
    const saved = classifyFrame(frame("tool_call_requested", { id: "c9", name: "web_search" }, { sourceKind: "session_history" }));
    expect(live.eventKey).toBe(saved.eventKey);
  });
});

describe("messages between agents", () => {
  const directory = createAgentDirectory([
    { id: "domain:support", label: "Support", aliases: ["support-member"] },
    { id: "router:main", label: "Router" },
  ]);

  it("names a sent message's recipient through the directory", () => {
    // Comms names a member <team>/<profile>/<member>.
    const sent = classifyFrame(frame("tool_call_requested", { id: "m1", name: "send_request", args: { display_name: "reference-mob/lead/support-member", body: "Refund 4411?" } }), directory);
    expect([sent.family, sent.actor, sent.target, sent.snippet]).toEqual(["message", "domain:billing", "domain:support", "Refund 4411?"]);
    expect(frameSentence(sent, (key) => directory.lookup(key)?.label ?? key)).toBe("domain:billing asked Support");
    // A name the directory does not know stays as written.
    expect(classifyFrame(frame("tool_call_requested", { id: "m2", name: "send_message", args: { to: "team/outsider" } }), directory).target).toBe("outsider");
  });

  it("reads a comms notice from its sender's side", () => {
    const incoming = classifyFrame(frame("system_notice", {
      blocks: [{ type: "comms", kind: "message", direction: "incoming", peer: { id: "p-1", display_name: "reference-mob/lead/router:main" }, content: [{ type: "text", text: "New ticket" }] }],
    }), directory);
    expect([incoming.family, incoming.actor, incoming.target, incoming.snippet]).toEqual(["message", "router:main", "domain:billing", "New ticket"]);
    expect(frameSentence(incoming, asKeys)).toBe("router:main messaged domain:billing");
    const outgoing = classifyFrame(frame("system_notice", {
      blocks: [{ type: "comms", kind: "response_terminal", direction: "outgoing", peer: { id: "p-2", display_name: "domain:support" } }],
    }), directory);
    expect([outgoing.actor, outgoing.target, frameSentence(outgoing, asKeys)]).toEqual(["domain:billing", "domain:support", "domain:billing replied to domain:support"]);
  });

  it("reads a member kickoff as lifecycle, not a message", () => {
    const kickoff = classifyFrame(frame("system_notice", {
      blocks: [{ type: "comms", kind: "lifecycle", direction: "incoming", intent: "mob.kickoff_failed", peer: { id: "p-3", display_name: "team/delivery" }, payload: { peer: "delivery", role: "delivery_lead" } }],
    }));
    expect([kickoff.family, kickoff.severity, kickoff.actor, kickoff.snippet]).toEqual(["lifecycle", "warn", "delivery", "Delivery lead"]);
  });
});

describe("words for seeded frames", () => {
  const names = createAgentDirectory([
    { id: "billing", label: "Billing" },
    { id: "shipping", label: "Shipping" },
    { id: "intake", label: "Intake" },
  ]);
  const named = (f: ConsoleFrame) => frameSentence(classifyFrame(f, names), (key) => names.lookup(key)?.label ?? key);

  it("reads a budget warning's share as a percentage of its token budget", () => {
    const warning = classifyFrame(frame("budget_warning", { budget_type: "tokens", used: 1_690_423, limit: 2_000_000, percent: 0.8452115058898926 }));
    expect([sentence(frame("budget_warning", { budget_type: "tokens", percent: 0.8452 })), warning.snippet]).toEqual([
      "domain:billing has used most of its token budget",
      "85% used",
    ]);
    // Without a share: used of limit.
    const calls = classifyFrame(frame("budget_warning", { budget_type: "tool_calls", used: 45, limit: 50 }));
    expect([frameSentence(calls, asKeys), calls.snippet]).toEqual(["domain:billing has used most of its tool call budget", "90% used"]);
  });

  it("names the recipient of a send that names it by peer id, once the console saw that id", () => {
    const agents = [
      { identity: "domain:billing", member_id: "billing-member", label: "Billing" },
      { identity: "domain:support", member_id: "support-member", label: "Support" },
    ];
    const ledger = createActivityLedger({ notifyMs: 0 });
    ledger.setResolver(createAgentResolver(agents));
    const directoryNow = () => {
      const ids = ledger.peerIds();
      return createAgentDirectory(agents.map((agent) => ({ id: agent.identity, label: agent.label, aliases: [agent.member_id, ...(ids.get(agent.identity) ?? [])] })));
    };
    const send = frame("tool_call_requested", { id: "demo-call-1", name: "send_message", args: { body: "Status?", handling_mode: "queue", peer_id: "0799c446-313b" } });
    const before = directoryNow();
    expect(frameSentence(classifyFrame(send, before), (key) => before.lookup(key)?.label ?? key)).toBe("Billing messaged a peer");
    // A notice from Support names its peer id and its comms name.
    const version = ledger.getPeersVersion();
    ledger.ingest(frame("system_notice", {
      blocks: [{ type: "comms", kind: "message", direction: "incoming", delivery_id: "env-1", peer: { id: "0799c446-313b", display_name: "customer-ops/specialist/support-member" } }],
    }, { identity: "domain:billing" }));
    expect(ledger.getPeersVersion()).toBeGreaterThan(version);
    expect(ledger.peerIds().get("domain:support")).toEqual(["0799c446-313b"]);
    const after = directoryNow();
    const sent = classifyFrame(send, after);
    expect([sent.target, frameSentence(sent, (key) => after.lookup(key)?.label ?? key)]).toEqual(["domain:support", "Billing messaged Support"]);
  });

  it("names a memory event by its payload, never \"System\"", () => {
    const system = { identity: "_system" };
    const dream = classifyFrame(frame("memory.dream.completed", { detail: { phase: "consolidate" }, ops_committed: 2, realm: "default" }, system));
    expect([named(frame("memory.dream.completed", { ops_committed: 2 }, system)), dream.snippet]).toEqual(["A memory dream finished", "2 changes"]);
    expect(named(frame("memory.dream.started", { realm: "default" }, system))).toBe("A memory dream started");
    const quarantined = classifyFrame(frame("memory.write.quarantined", { author: "agent shipping", realm: "default", reason: "matches the 'credential-assignment' secret pattern class" }, system), names);
    expect([named(frame("memory.write.quarantined", { author: "agent shipping" }, system)), quarantined.actor, quarantined.snippet]).toEqual([
      "A memory write from Shipping was quarantined",
      "shipping",
      "matches the 'credential-assignment' secret pattern class",
    ]);
    // The runtime's own author form.
    expect(named(frame("memory.write.quarantined", { author: "Agent { identity: \"shipping\" }" }, system))).toBe("A memory write from Shipping was quarantined");
    expect(named(frame("memory.write.quarantined", { author: "Operator" }, system))).toBe("A memory write from the operator was quarantined");
    const conflict = classifyFrame(frame("memory.conflict.signal", { entity: "billing", realm: "default", reason: "a 200 EUR threshold is still quoted next to the 250 EUR policy", topic: "refund approval threshold" }, system));
    expect([frameSentence(conflict, asKeys), conflict.snippet]).toEqual([
      "Conflicting memories about billing, refund approval threshold",
      "a 200 EUR threshold is still quoted next to the 250 EUR policy",
    ]);
    const moved = classifyFrame(frame("memory.record.promoted", { gated: false, realm: "default", scope_key: "billing", scope_kind: "identity" }, system), names);
    expect([named(frame("memory.record.promoted", { scope_key: "billing", scope_kind: "identity" }, system)), moved.actor]).toEqual([
      "A memory record moved to Billing's memory",
      "billing",
    ]);
    expect(named(frame("memory.quarantine.verdict", { verdict: "unverifiable" }, system))).toBe("A quarantined record was reviewed");
    for (const event of ["memory.dream.skipped", "memory.quarantine.release_blocked", "memory.budget.denied", "memory.hygiene.blocked", "memory.distill.timed_out", "memory.something_new"]) {
      expect(named(frame(event, {}, system)), event).not.toContain("System");
    }
  });

  it("reads a tool's JSON error as its kind and its message", () => {
    const denied = classifyFrame(frame("tool_result_received", {
      id: "demo-call-9-tool", name: "workgraph_ready", is_error: true,
      content: [{ type: "text", text: "{\"error\":\"access_denied\",\"message\":\"Tool 'workgraph_ready' is not allowed by policy\"}" }],
    }));
    expect(denied.snippet).toBe("Access denied: Tool 'workgraph_ready' is not allowed by policy");
    expect(toolFailureWords("{\"error\":\"timeout\",\"message\":\"Tool 'query_carrier' timed out after 4000ms\"}")).toBe("Timeout: Tool 'query_carrier' timed out after 4000ms");
    expect(toolFailureWords("rate limited")).toBe("rate limited");
  });

  it("reads a model failure as its human part, the provider as context", () => {
    const chain = "Terminal failure: Failed (LlmFailure): LLM failure terminal turn: LLM error (anthropic): Invalid request: The model provider rejected the request: the input is 412,880 tokens and exceeds the 200,000 token context window (the full carrier rate export was attached).";
    expect(failureWords(chain)).toBe("Invalid request (Anthropic): The model provider rejected the request: the input is 412,880 tokens and exceeds the 200,000 token context window (the full carrier rate export was attached).");
    expect(failureWords("Terminal failure: Failed (ToolFailure): tool failure terminal turn: payment API unreachable")).toBe("payment API unreachable");
    expect(failureWords("LLM error (openai): Rate limited")).toBe("OpenAI: Rate limited");
    const failed = classifyFrame(frame("interaction_failed", {
      error: chain,
      error_report: { class: "llm", message: chain, reason: { cause_kind: "llm_failure", outcome: "failed", reason_type: "turn_terminal_cause" } },
      reason: "turn_terminal_cause",
      terminal_cause_kind: "llm_failure",
    }, { identity: "shipping" }));
    expect(failed.snippet?.startsWith("Invalid request (Anthropic): The model provider rejected the request: the input is 412,880 tokens")).toBe(true);
    expect([failed.severity, failed.snippet?.includes("Terminal failure"), failed.snippet?.includes("turn_terminal_cause")]).toEqual(["crit", false, false]);
  });

  it("drops the envelope line of a peer message notice", () => {
    const peer = { display_name: "customer-ops/coordinator/intake", id: "512386b1-2cda-526f-94ef-3d3388ef8ceb" };
    const text = "Peer message from customer-ops/coordinator/intake:\nTicket 88101 for Billing: payment failed twice on enterprise order 77310.";
    const notice = classifyFrame(frame("system_notice", {
      blocks: [{ type: "comms", kind: "message", direction: "incoming", delivery_id: "cbfb673c", peer, summary: "Peer message", content: [{ type: "text", text }] }],
    }, { identity: "billing", sourceKind: "session_history" }), names);
    expect([frameSentence(notice, (key) => names.lookup(key)?.label ?? key), notice.snippet]).toEqual([
      "Intake messaged Billing",
      "Ticket 88101 for Billing: payment failed twice on enterprise order 77310.",
    ]);
    // Only an envelope that names the notice's own peer goes; without the
    // peer's names, text that merely starts that way stays.
    expect(stripPeerTransportScaffold("Peer message from customer-ops/coordinator/dispatch: Morning. Status?", ["customer-ops/coordinator/dispatch"])).toBe("Morning. Status?");
    expect(stripPeerTransportScaffold("[COMMS REQUEST from dispatch] Carrier status?", ["dispatch"])).toBe("Carrier status?");
    expect(stripPeerTransportScaffold("Peer message from someone-else:\nHi", ["customer-ops/coordinator/dispatch"])).toBe("Peer message from someone-else:\nHi");
    expect(stripPeerTransportScaffold("Peer request from fugue/peer-a:\nDone.")).toBe("Peer request from fugue/peer-a:\nDone.");
  });

  it("reads model markdown in a snippet as plain words", () => {
    const done = classifyFrame(frame("interaction_complete", { result: "**Ops Assistant** here. On it.\n\n> Remind me what is next on my calendar." }));
    expect(done.snippet).toBe("Ops Assistant here. On it. Remind me what is next on my calendar.");
    const standUp = classifyFrame(frame("interaction_complete", { result: "## Morning stand-up\n\n| Desk | Open |\n|---|---|\n| Billing | 3 |" }));
    expect(standUp.snippet).toBe("Morning stand-up Desk Open Billing 3");
    const flat = (text: string) => plainSnippetText(text).replace(/\s+/g, " ").trim();
    expect(flat("- first\n* second\n1. third\n\n---\n```ts\nconst x = 1;\n```\nUse `refund_policy` and __this__")).toBe("first second third const x = 1; Use refund_policy and this");
    // Single marks are words: snake_case names, arithmetic.
    expect(flat("a snake_case_name and 2 * 3")).toBe("a snake_case_name and 2 * 3");
  });

  it("gives a name ending in s the bare apostrophe", () => {
    const directory = createAgentDirectory([{ id: "accounts", label: "Accounts" }, { id: "billing", label: "Billing" }]);
    const name = (key: string) => directory.lookup(key)?.label ?? key;
    const kickoff = (member: string) => frame("system_notice", {
      blocks: [{ type: "comms", kind: "lifecycle", direction: "incoming", intent: "mob.kickoff_started", peer: { display_name: `customer-ops/specialist/${member}`, id: `p-${member}` }, payload: { peer: member } }],
    });
    expect(frameSentence(classifyFrame(kickoff("accounts"), directory), name)).toBe("Accounts' kickoff started");
    expect(frameSentence(classifyFrame(kickoff("billing"), directory), name)).toBe("Billing's kickoff started");
    expect(frameSentence(classifyFrame(frame("run_failed", { error: "x" }, { identity: "accounts" }), directory), name)).toBe("Accounts' turn failed");
    expect(frameSentence(classifyFrame(frame("memory.record.promoted", { scope_kind: "identity", scope_key: "accounts" }, { identity: "_system" }), directory), name))
      .toBe("A memory record moved to Accounts' memory");
  });

  it("lists one kickoff once, whichever of the member's peers got a copy", () => {
    const kickoff = (receiver: string, atMs: number, peerId = "0799c446-313b") => frame("system_notice", {
      blocks: [{ type: "comms", kind: "lifecycle", direction: "incoming", intent: "mob.kickoff_started", summary: "Peer lifecycle: mob.kickoff_started", peer: { display_name: "customer-ops/specialist/shipping", id: peerId }, payload: { peer: "shipping", role: "specialist" } }],
    }, { identity: receiver, sourceKind: "session_history", timestampMs: atMs });
    const at = Date.UTC(2026, 10, 1, 8, 0, 30);
    const keys = [kickoff("incident", at + 3), kickoff("dispatch", at), kickoff("orders", at + 7)].map((f) => classifyFrame(f).eventKey);
    expect(new Set(keys).size).toBe(1);
    expect(sentence(kickoff("orders", at))).toBe("shipping's kickoff started");
    // The member started again (a new comms peer id): another kickoff.
    expect(classifyFrame(kickoff("incident", at + 600_000, "5a1d9e07-77aa")).eventKey).not.toBe(keys[0]);
  });
});

describe("memoised", () => {
  it("reads a frame object once, and a user turn afresh", () => {
    const f = frame("interaction_complete", { result: "Done" });
    expect(classifyFrame(f)).toBe(classifyFrame(f));
    const input = frame("user_input", { content: "Hi", origin_kind: "operator", idempotency_key: "later-own-send" });
    expect(sentence(input)).toBe("An operator asked domain:billing");
    rememberOwnSend("later-own-send");
    expect(sentence(input)).toBe("You asked domain:billing");
  });
});
