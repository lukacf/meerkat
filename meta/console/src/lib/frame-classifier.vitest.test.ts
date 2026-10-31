import { describe, expect, it } from "vitest";

import { createAgentDirectory } from "../../../packages/console-components/src/panel-kit/agents";
import { describeTodayFrame } from "../panels/TimelinePanel";
import { rememberOwnSend } from "./turn-origin";
import type { ConsoleFrame } from "../types";
import { classifyFrame, frameSentence, isProblemFrame } from "./frame-classifier";

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
