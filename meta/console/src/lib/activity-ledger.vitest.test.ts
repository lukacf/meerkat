import { act, render } from "@testing-library/react";
import React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ConsoleFrame } from "../types";
import {
  ActivityLedgerProvider,
  LEDGER_WINDOW_MS,
  createActivityLedger,
  createAgentResolver,
  useActivityLedger,
} from "./activity-ledger";

const NOW = Date.UTC(2026, 9, 31, 12, 0, 0);
const MIN = 60_000;
let serial = 0;

function frame(event: string, atMs: number, data: Record<string, unknown> = {}, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  serial += 1;
  return { id: `l-${serial}`, event, identity: "domain:billing", timestampMs: atMs, data, ...extra };
}

const ledgerAt = (nowMs = NOW) => createActivityLedger({ now: () => nowMs, notifyMs: 0 });

afterEach(() => {
  vi.useRealTimers();
});

describe("per agent per minute", () => {
  it("counts turns started and finished, failures, tool calls and events in minute bins", () => {
    const ledger = ledgerAt();
    ledger.ingestMany([
      frame("run_started", NOW - 10 * MIN, {}, { runId: "r1" }),
      frame("tool_call_requested", NOW - 10 * MIN + 1000, { id: "t1", name: "web_search" }),
      frame("interaction_complete", NOW - 9 * MIN, { result: "ok" }, { runId: "r1" }),
      frame("run_started", NOW - 3 * MIN, {}, { runId: "r2" }),
      frame("interaction_failed", NOW - 2 * MIN, { error_report: { class: "llm", message: "boom" } }, { runId: "r2" }),
      frame("text_delta", NOW - 2 * MIN, { delta: "x" }),
    ]);
    const window = { bins: 15, endMs: NOW };
    expect(ledger.series("domain:billing", "turnsStarted", window)).toEqual([0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0]);
    expect(ledger.series("domain:billing", "turnsFinished", window)[6]).toBe(1);
    expect(ledger.series("domain:billing", "failures", window)[13]).toBe(1);
    expect(ledger.series("domain:billing", "toolCalls", window)[5]).toBe(1);
    // Plumbing (run starts, deltas) is no event; the tool call, the end and the failure are.
    expect(ledger.totals("domain:billing")).toEqual({ turnsStarted: 2, turnsFinished: 1, failures: 1, toolCalls: 1, messagesSent: 0, events: 3 });
    expect(ledger.series("domain:billing", "turnsStarted", { bins: 3, binMs: 5 * MIN, endMs: NOW })).toEqual([0, 1, 1]);
    expect(ledger.totals(null).turnsStarted).toBe(2);
  });

  it("counts one event once: a second delivery and a saved history copy", () => {
    const ledger = ledgerAt();
    const live = frame("tool_call_requested", NOW - MIN, { id: "t9", name: "web_search" }, { sourceKind: "console_event" });
    ledger.ingest(live);
    ledger.ingest({ ...live });
    ledger.ingest(frame("tool_call_requested", NOW - MIN + 5, { id: "t9", name: "web_search" }, { sourceKind: "session_history" }));
    ledger.ingest(frame("interaction_complete", NOW - MIN, {}, { runId: "r5" }));
    ledger.ingest(frame("run_completed", NOW - MIN, {}, { runId: "r5" }));
    ledger.ingest(frame("user_input", NOW - 2 * MIN, { content: "Go" }, { sourceKind: "send", interactionId: "i-5" }));
    ledger.ingest(frame("user_input", NOW - 2 * MIN + 80, { content: "Go" }, { sourceKind: "session_history", interactionId: "i-5" }));
    expect(ledger.totals("domain:billing").toolCalls).toBe(1);
    expect(ledger.totals("domain:billing").turnsFinished).toBe(1);
    // The tool call, the turn's end and the input: three events.
    expect(ledger.totals("domain:billing").events).toBe(3);
  });

  it("keeps a day: older frames are not counted", () => {
    const ledger = ledgerAt();
    expect(ledger.ingest(frame("interaction_complete", NOW - LEDGER_WINDOW_MS - MIN, {}, { runId: "old" }))).toBe(false);
    expect(ledger.totals(null).turnsFinished).toBe(0);
  });
});

describe("run spans", () => {
  it("pairs a run's start and end by run id, or by interaction when the end names only that", () => {
    const ledger = ledgerAt();
    ledger.ingest(frame("run_started", NOW - 5 * MIN, {}, { runId: "r1", interactionId: "i1" }));
    expect(ledger.hasOpenRun("domain:billing")).toBe(true);
    expect(ledger.liveState("domain:billing").openRun).toMatchObject({ runId: "r1", startMs: NOW - 5 * MIN });
    ledger.ingest(frame("interaction_failed", NOW - 4 * MIN, { reason: "lifecycle_mutation" }, { interactionId: "i1" }));
    expect(ledger.hasOpenRun("domain:billing")).toBe(false);
    expect(ledger.runSpans("domain:billing")).toEqual([
      expect.objectContaining({ runId: "r1", interactionId: "i1", startMs: NOW - 5 * MIN, endMs: NOW - 4 * MIN, outcome: "cancelled" }),
    ]);
  });

  it("merges an end that arrives before its start, and ends open runs on a lifecycle change", () => {
    const ledger = ledgerAt();
    ledger.ingest(frame("interaction_complete", NOW - 2 * MIN, {}, { runId: "r2" }));
    ledger.ingest(frame("run_started", NOW - 3 * MIN, {}, { runId: "r2" }));
    ledger.ingest(frame("run_started", NOW - MIN, {}, { runId: "r3" }));
    ledger.ingest(frame("tool_call_requested", NOW - MIN + 10, { id: "t1", name: "web_search" }));
    expect(ledger.liveState("domain:billing").openRun?.toolCalls).toBe(1);
    ledger.ingest(frame("identity_respawned", NOW - 30_000, { generation: 2 }));
    const spans = ledger.runSpans("domain:billing", { sinceMs: NOW - 10 * MIN, untilMs: NOW });
    expect(spans.map((span) => [span.runId, span.startMs, span.endMs, span.outcome])).toEqual([
      ["r2", NOW - 3 * MIN, NOW - 2 * MIN, "ok"],
      ["r3", NOW - MIN, NOW - 30_000, "cancelled"],
    ]);
    expect(ledger.hasOpenRun("domain:billing")).toBe(false);
  });

  it("keeps tool executions with their duration, and model calls while open", () => {
    const ledger = ledgerAt();
    ledger.ingest(frame("tool_execution_completed", NOW - MIN, { id: "t1", name: "web_search", is_error: false, duration_ms: 4200 }));
    ledger.ingest(frame("tool_execution_started", NOW - 20_000, { id: "t2", name: "payment_api" }));
    ledger.ingest(frame("turn_started", NOW - 10_000, { turn_number: 1 }, { runId: "r9" }));
    expect(ledger.toolSpans("domain:billing")[0]).toMatchObject({ startMs: NOW - MIN - 4200, endMs: NOW - MIN, durationMs: 4200, outcome: "ok" });
    const live = ledger.liveState("domain:billing");
    expect(live.openTool).toMatchObject({ key: "tool:domain:billing:t2", startMs: NOW - 20_000 });
    expect(live.callingModel).toBe(true);
    expect(live.lastActivityMs).toBe(NOW - 10_000);
    ledger.ingest(frame("tool_execution_timed_out", NOW - 5_000, { id: "t2", name: "payment_api", timeout_ms: 15_000 }));
    ledger.ingest(frame("turn_completed", NOW - 4_000, { stop_reason: "end_turn" }, { runId: "r9" }));
    expect(ledger.liveState("domain:billing")).toMatchObject({ openTool: null, callingModel: false });
  });
});

describe("peer traffic per ordered pair", () => {
  const agents = [
    { identity: "domain:billing", member_id: "billing-member", agent_id: "billing-member", label: "Billing" },
    { identity: "domain:support", member_id: "support-member", agent_id: "support-member", label: "Support" },
    { identity: "router:main", member_id: "router-member", agent_id: "router-member", label: "Router" },
  ];

  /** A send tool's receipt: the envelope id the recipient's notice carries as its delivery id. */
  const receipt = (callId: string, name: string, envelope: string, atMs: number, identity = "domain:billing") => frame("tool_result_received", atMs, {
    id: callId, name, is_error: false,
    content: [{ type: "text", text: JSON.stringify({ kind: "peer_message", receipt: { envelope_id: envelope, kind: "peer_message_sent" } }) }],
  }, { identity });
  const notice = (identity: string, atMs: number, block: Record<string, unknown>, extra: Partial<ConsoleFrame> = {}) =>
    frame("system_notice", atMs, { blocks: [{ type: "comms", direction: "incoming", ...block }] }, { identity, ...extra });

  it("counts a message once however many of its ends are seen", () => {
    const ledger = ledgerAt();
    ledger.setResolver(createAgentResolver(agents));
    // Billing asks a peer by its opaque id; the receipt names the envelope.
    ledger.ingest(frame("tool_call_requested", NOW - 5 * MIN, { id: "m1", name: "send_request", args: { peer_id: "peer-7", body: "Refund?" } }));
    ledger.ingest(receipt("m1", "send_request", "env-1", NOW - 5 * MIN + 100));
    // Support's notice of it, live and as the session's saved copy.
    const request = { kind: "request", peer: { id: "peer-1", display_name: "reference-mob/lead/billing-member" }, delivery_id: "env-1" };
    ledger.ingest(notice("domain:support", NOW - 5 * MIN + 800, request, { sourceKind: "console_event" }));
    ledger.ingest(notice("domain:support", NOW - 5 * MIN + 900, request, { sourceKind: "session_history" }));
    // Support replies; Billing's notice of the reply arrives before Support's receipt.
    ledger.ingest(frame("tool_call_requested", NOW - 4 * MIN, { id: "m2", name: "send_response", args: { peer_id: "peer-1" } }, { identity: "domain:support" }));
    ledger.ingest(notice("domain:billing", NOW - 4 * MIN + 500, { kind: "response_terminal", peer: { id: "peer-7", display_name: "reference-mob/lead/support-member" }, delivery_id: "env-2" }));
    ledger.ingest(receipt("m2", "send_response", "env-2", NOW - 4 * MIN + 600, "domain:support"));
    // Two more messages: one seen by its sender only, one from an agent outside the console.
    ledger.ingest(frame("tool_call_requested", NOW - 2 * MIN, { id: "m3", name: "send_message", args: { to: "Support" } }));
    ledger.ingest(notice("domain:billing", NOW - 90_000, { kind: "message", peer: { id: "peer-x", display_name: "partner-mob/lead/carrier" }, delivery_id: "env-9" }));
    // A kickoff notice is no message.
    ledger.ingest(notice("domain:billing", NOW - MIN, { kind: "lifecycle", intent: "mob.kickoff_started", peer: { id: "peer-9", display_name: "router-member" } }));
    expect(ledger.traffic({ sinceMs: NOW - 60 * MIN })).toEqual([
      { from: "domain:billing", to: "domain:support", count: 2, lastMs: NOW - 2 * MIN },
      { from: "carrier", to: "domain:billing", count: 1, lastMs: NOW - 90_000 },
      { from: "domain:support", to: "domain:billing", count: 1, lastMs: NOW - 4 * MIN },
    ]);
    expect(ledger.totals("domain:billing").messagesSent).toBe(2);
    expect(ledger.totals("domain:support").messagesSent).toBe(1);
    // Busiest first, then the most recent.
    expect(ledger.traffic({ sinceMs: NOW - 3 * MIN })).toEqual([
      { from: "carrier", to: "domain:billing", count: 1, lastMs: NOW - 90_000 },
      { from: "domain:billing", to: "domain:support", count: 1, lastMs: NOW - 2 * MIN },
    ]);
    expect(ledger.series("domain:billing", "messagesSent", { bins: 6, endMs: NOW })).toEqual([0, 1, 0, 0, 1, 0]);
  });

  it("learns an opaque peer id from the notice its message became", () => {
    const ledger = ledgerAt();
    ledger.setResolver(createAgentResolver(agents));
    // No peers list and no name: only the message ties peer-5 to Router.
    ledger.ingest(frame("tool_call_requested", NOW - 3 * MIN, { id: "m1", name: "send_message", args: { peer_id: "peer-5" } }));
    ledger.ingest(receipt("m1", "send_message", "env-5", NOW - 3 * MIN + 50));
    ledger.ingest(notice("router:main", NOW - 3 * MIN + 400, { kind: "message", peer: { id: "peer-1", display_name: "billing-member" }, delivery_id: "env-5" }));
    // A later message to the same peer id, seen from its sender only.
    ledger.ingest(frame("tool_call_requested", NOW - MIN, { id: "m2", name: "send_message", args: { peer_id: "peer-5" } }));
    expect(ledger.traffic()).toEqual([{ from: "domain:billing", to: "router:main", count: 2, lastMs: NOW - MIN }]);
  });

  it("merges a pair recorded under a name once the agents resolve it", () => {
    const ledger = ledgerAt();
    ledger.ingest(frame("tool_call_requested", NOW - MIN, { id: "m1", name: "send_message", args: { to: "team/router-member" } }));
    expect(ledger.traffic()[0]).toMatchObject({ from: "domain:billing", to: "router-member" });
    ledger.setResolver(createAgentResolver(agents));
    ledger.ingest(frame("tool_call_requested", NOW - 30_000, { id: "m2", name: "send_message", args: { to: "Router" } }));
    expect(ledger.traffic()).toEqual([{ from: "domain:billing", to: "router:main", count: 2, lastMs: NOW - 30_000 }]);
  });
});

describe("bounds", () => {
  it("keeps at most 2000 agents, dropping the least recently active", () => {
    const ledger = ledgerAt();
    const frames: ConsoleFrame[] = [];
    for (let i = 0; i < 2001; i++) {
      frames.push(frame("interaction_complete", NOW - 3 * 60 * MIN + i * 1000, {}, { identity: `agent:${i}`, runId: `r${i}` }));
    }
    ledger.ingestMany(frames);
    const identities = ledger.identities();
    expect(identities).toHaveLength(2000);
    expect(identities).not.toContain("agent:0");
    expect(identities[0]).toBe("agent:2000");
  });
});

describe("change notices", () => {
  it("publishes a burst of changes once per interval", () => {
    vi.useFakeTimers();
    const ledger = createActivityLedger({ now: () => NOW, notifyMs: 250 });
    const heard = vi.fn();
    ledger.subscribe(heard);
    const before = ledger.getVersion();
    ledger.ingest(frame("interaction_complete", NOW - MIN, {}, { runId: "a" }));
    ledger.ingest(frame("interaction_complete", NOW - MIN, {}, { runId: "b" }));
    expect(heard).not.toHaveBeenCalled();
    expect(ledger.getVersion()).toBe(before);
    vi.advanceTimersByTime(250);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(ledger.getVersion()).toBeGreaterThan(before);
    // Nothing changed: nothing to publish.
    ledger.ingest(frame("interaction_complete", NOW - MIN, {}, { runId: "a" }));
    vi.advanceTimersByTime(250);
    expect(heard).toHaveBeenCalledTimes(1);
  });

  it("re-renders a panel that reads it, and reads an empty ledger without a provider", () => {
    const ledger = ledgerAt();
    function Turns({ identity }: { identity: string }) {
      const view = useActivityLedger();
      return React.createElement("span", { "data-testid": "turns" }, String(view.totals(identity).turnsFinished));
    }
    const lone = render(React.createElement(Turns, { identity: "domain:billing" }));
    expect(lone.getByTestId("turns").textContent).toBe("0");
    lone.unmount();
    const view = render(React.createElement(ActivityLedgerProvider, { ledger }, React.createElement(Turns, { identity: "domain:billing" })));
    expect(view.getByTestId("turns").textContent).toBe("0");
    act(() => { ledger.ingest(frame("interaction_complete", Date.now() - MIN, {}, { runId: "x" })); });
    expect(view.getByTestId("turns").textContent).toBe("1");
  });
});

describe("agent resolver", () => {
  it("matches identities, member ids, labels and comms names", () => {
    const resolve = createAgentResolver([
      { identity: "domain:support", member_id: "support-member", agent_id: "agent-1", label: "Support Triage", labels: { agent_identity: "identity:support" } },
    ]);
    for (const reference of ["domain:support", "support-member", "agent-1", "identity:support", "Support Triage", "support triage", "reference-mob/lead/support-member", "SUPPORT_MEMBER"]) {
      expect(resolve(reference), reference).toBe("domain:support");
    }
    expect(resolve("billing")).toBeUndefined();
    expect(resolve("")).toBeUndefined();
  });
});
