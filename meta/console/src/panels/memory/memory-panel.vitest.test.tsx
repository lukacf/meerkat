/**
 * The console v2 Memory panel (spec B3): the not-set-up state, dream runs
 * withheld by access, proposal authors in words, the landscape's record
 * ticks, the Records views and the biography drawer.
 */
import React from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type {
  MemoryDreamRun,
  MemoryFullRecord,
  MemoryPanelRecord,
  MemoryProposalEntry,
  MemoryScopeOverview,
} from "../../types";
import { MemoryPanel } from "../MemoryPanel";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

const NOW = Date.now();
const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

function record(overrides: Partial<MemoryPanelRecord> & Pick<MemoryPanelRecord, "id" | "title">): MemoryPanelRecord {
  return {
    kind: "fact",
    trust: "agent_observed",
    status: { status: "active" },
    scope: { scope: "identity", realm: "ops", identity: "billing" },
    created_at_ms: NOW - 2 * DAY,
    updated_at_ms: NOW - 2 * DAY,
    ...overrides,
  };
}

const tip = record({ id: "mem-v3", title: "Refund approval threshold", supersedes: "mem-v2", kind: "procedure" });
const middle = record({
  id: "mem-v2",
  title: "Refund approval threshold",
  supersedes: "mem-v1",
  kind: "procedure",
  status: { status: "superseded", by: "mem-v3" },
  created_at_ms: NOW - 5 * DAY,
  updated_at_ms: NOW - 5 * DAY,
});
const oldest = record({
  id: "mem-v1",
  title: "Refund approval threshold",
  kind: "procedure",
  status: { status: "superseded", by: "mem-v2" },
  created_at_ms: NOW - 9 * DAY,
  updated_at_ms: NOW - 9 * DAY,
});
const quarantined = record({
  id: "mem-q",
  title: "Regional carrier API access",
  status: { status: "quarantined", reason: "matches a secret pattern" },
  created_at_ms: NOW - HOUR,
  updated_at_ms: NOW - HOUR,
});
const team = record({
  id: "mem-team",
  title: "Escalation map",
  kind: "relationship",
  scope: { scope: "mob", realm: "ops", mob: "customer-ops" },
  created_at_ms: NOW - 40 * DAY,
  updated_at_ms: NOW - 40 * DAY,
});
const records = [tip, middle, oldest, quarantined, team];

const overview: MemoryScopeOverview[] = [
  { realm: "ops", scope_kind: "identity", scope_key: "billing", active: 1, quarantined: 1, superseded: 2, tombstoned: 0, body_bytes: 1400 },
  { realm: "ops", scope_kind: "mob", scope_key: "customer-ops", active: 1, quarantined: 0, superseded: 0, tombstoned: 0, body_bytes: 734, floor_pressure: true },
];

type PanelProps = Partial<React.ComponentProps<typeof MemoryPanel>>;

function renderPanel(props: PanelProps = {}) {
  const directory = createAgentDirectory([{ id: "billing", label: "Billing" }, { id: "shipping", label: "Shipping" }]);
  return render(
    <AgentDirectoryProvider directory={directory}>
      <MemoryPanel
        records={records}
        realms={["ops"]}
        quarantineRecords={[quarantined]}
        pendingPromotions={[]}
        dreams={[]}
        detail={null}
        overview={{ scopes: overview, realms: ["ops"], floors: { records: 4000, bytes: 32 * 1024 * 1024 } }}
        canReviewQuarantine
        onRefresh={() => undefined}
        onSelectRecord={() => undefined}
        onClearDetail={() => undefined}
        {...props}
      />
    </AgentDirectoryProvider>,
  );
}

function text(view: ReturnType<typeof render>, testId: string): string {
  return view.getByTestId(testId).textContent ?? "";
}

describe("Memory panel v2", () => {
  it("says memory is not set up, with what it would show and how to set it up", () => {
    const view = renderPanel({ unavailable: true });
    const state = text(view, "memory-not-set-up");
    expect(state.includes("Memory isn't set up on this runtime.")).toBe(true);
    expect(state.includes("Agents remember facts, preferences and procedures here")).toBe(true);
    expect(view.getByText("How to set up memory").closest("a")?.getAttribute("href")?.startsWith("https://")).toBe(true);
    expect(view.queryByTestId("memory-unavailable") === null).toBe(false);
    expect(view.queryByTestId("memory-tab:holdings")).toBeNull();
    expect(/no access/i.test(text(view, "memory-panel"))).toBe(false);
  });

  it("shows dream runs withheld by access calmly, never as no dream yet", () => {
    const view = renderPanel({ dreamsWithheld: true, dreamRunsWithheld: true });
    const tile = view.getByTestId("memory-verdict:dreams");
    expect(tile.getAttribute("data-status")).toBe("no-grant");
    expect((tile.textContent ?? "").includes("Dream history is withheld by access")).toBe(true);
    fireEvent.click(view.getByTestId("memory-tab:dreams"));
    expect(text(view, "memory-dream-runs-withheld").includes("Dream history needs access to all memory in this space.")).toBe(true);
    expect(text(view, "memory-dreams-withheld").includes("Withheld by access.")).toBe(true);
    const dreams = text(view, "memory-panel");
    expect(/no dream (run is recorded|runs recorded|ran this week)/i.test(dreams)).toBe(false);
    expect(/realm/i.test(dreams)).toBe(false);
  });

  it("keeps the empty dream history copy when the server sends no flag", () => {
    const view = renderPanel();
    fireEvent.click(view.getByTestId("memory-tab:dreams"));
    expect(view.queryByTestId("memory-dream-runs-withheld")).toBeNull();
    expect(text(view, "memory-dream-runs").includes("No dream run is recorded yet.")).toBe(true);
  });

  it("names a proposal's author in words, never the server's Debug text", () => {
    const proposals: MemoryProposalEntry[] = [
      { realm: "ops", proposal_id: "p-1", scope_kind: "mob", scope_key: "customer-ops", title: "Weekend refund cap", kind: "procedure", author: "Agent { identity: \"billing\" }" },
      { realm: "ops", proposal_id: "p-2", scope_kind: "mob", scope_key: "customer-ops", title: "Merged escalation", kind: "relationship", author: "Steward { run_id: \"dream-7\" }" },
    ];
    const view = renderPanel({ proposals });
    fireEvent.click(view.getByTestId("memory-tab:pipeline"));
    const first = text(view, "memory-proposal:p-1");
    expect(first.includes("proposed by Billing")).toBe(true);
    expect(/Agent \{|identity:|"/.test(first)).toBe(false);
    const second = text(view, "memory-proposal:p-2");
    expect(second.includes("proposed by the memory steward")).toBe(true);
    expect(/Steward \{|dream-7/.test(second)).toBe(false);
  });

  it("draws each record of a scope as a tick that opens it, with earlier records counted", () => {
    const opened: string[] = [];
    const view = renderPanel({ onSelectRecord: (_realm, id) => opened.push(id) });
    const landscape = view.getByTestId("memory-landscape");
    expect(landscape.querySelectorAll('[data-testid^="memory-landscape-tick:"]').length).toBe(4);
    const tick = view.getByTestId("memory-landscape-tick:mem-q");
    expect(tick.getAttribute("data-shape")).toBe("triangle");
    expect((tick.getAttribute("aria-label") ?? "").startsWith("Regional carrier API access")).toBe(true);
    // The strip is one tab stop: focus a mark, Enter opens its record.
    fireEvent.focus(tick);
    fireEvent.keyDown(tick, { key: "Enter" });
    expect(opened).toEqual(["mem-q"]);
    // The team record is older than the window: counted, not drawn.
    const teamRow = text(view, "memory-holdings-scope:mob:ops:customer-ops");
    expect(teamRow.includes("+1")).toBe(true);
    expect(view.queryByTestId("memory-landscape-tick:mem-team")).toBeNull();
    expect(view.getByTestId("memory-holdings-floor:mob:ops:customer-ops").textContent).toBe("Near limit");
    // The ticks carry no text of their own: row text never grows keys.
    expect(/mem-/.test(teamRow)).toBe(false);
  });

  it("switches the Records view between List, Timeline and Clusters", () => {
    const view = renderPanel();
    fireEvent.click(view.getByTestId("memory-tab:records"));
    expect(view.getByTestId("memory-records-view:list").getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(view.getByTestId("memory-records-view:timeline"));
    expect(view.queryByTestId("memory-records-timeline") === null).toBe(false);
    expect(view.container.querySelectorAll('[data-testid^="memory-record:"]').length).toBe(records.length);
    fireEvent.click(view.getByTestId("memory-records-view:clusters"));
    const procedures = text(view, "memory-cluster:procedure");
    expect(procedures.includes("Procedures")).toBe(true);
    expect(procedures.includes("3")).toBe(true);
    fireEvent.click(view.getByTestId("memory-cluster:procedure").querySelector("button") as HTMLElement);
    expect(view.getByTestId("memory-records-view:list").getAttribute("aria-pressed")).toBe("true");
    expect(view.getByTestId("memory-records-kind:procedure").getAttribute("aria-pressed")).toBe("true");
    expect(view.container.querySelectorAll('[data-testid^="memory-record:"]').length).toBe(3);
  });

  it("finds loaded records by text in titles, descriptions and tags, highlighted", () => {
    const view = renderPanel();
    fireEvent.click(view.getByTestId("memory-tab:records"));
    fireEvent.change(view.getByTestId("memory-records-search"), { target: { value: "carrier" } });
    const rows = view.container.querySelectorAll('[data-testid^="memory-record:"]');
    expect(rows.length).toBe(1);
    expect(rows[0].querySelector("mark")?.textContent).toBe("carrier");
  });

  it("opens a record's biography in the drawer with its versions oldest first", async () => {
    const detail = {
      realm: "ops",
      record: { ...tip, body: "Refunds above 250 EUR need a team lead's sign-off.", content_hash: "abc" } as MemoryFullRecord,
      chain: [oldest, middle, tip],
      injections: [{ record_id: tip.id, identity: "billing", surface: "turn" as const, at_ms: NOW - HOUR }],
    };
    const cleared: number[] = [];
    const view = renderPanel({ detail, onClearDetail: () => cleared.push(1) });
    await act(async () => undefined);
    const drawer = view.getByTestId("memory-detail");
    // The versions graph (the kit's GraphView): one node per version, the
    // open one selected.
    const graph = view.getByTestId("memory-biography-versions");
    const nodes = Array.from(graph.querySelectorAll('[data-testid^="memory-versions-node:"]'), (node) => node.getAttribute("data-testid"));
    expect(nodes.sort()).toEqual(["memory-versions-node:mem-v1", "memory-versions-node:mem-v2", "memory-versions-node:mem-v3"]);
    expect(view.getByTestId("memory-versions-node:mem-v3").getAttribute("data-current")).toBe("true");
    expect(view.getByTestId("memory-versions-node:mem-v3").getAttribute("aria-pressed")).toBe("true");
    const chain = Array.from(drawer.querySelectorAll('[data-testid^="memory-chain:"]'), (row) => row.getAttribute("data-testid"));
    expect(chain).toEqual(["memory-chain:mem-v1", "memory-chain:mem-v2", "memory-chain:mem-v3"]);
    expect(view.getByTestId("memory-chain:mem-v3").getAttribute("data-current")).toBe("true");
    expect(text(view, "memory-detail-life").includes("Given to Billing during a turn")).toBe(true);
    // The drawer opens beside the tab the reader is on.
    expect(view.getByTestId("memory-tab:holdings").getAttribute("aria-selected")).toBe("true");
    fireEvent.click(view.getByTestId("memory-detail-back"));
    expect(cleared.length).toBe(1);
  });

  it("counts memory given from the ledger rows it lists, in the server's wire shape", async () => {
    // The record read's injections as the server serializes them
    // (InjectionLogEntry): no session key on a build row, at_ms in epoch ms.
    // The record's own counter disagrees, as on a store whose rows were
    // written without it; USE follows the rows it lists.
    const wire = JSON.parse(
      JSON.stringify({
        realm: "ops",
        record: { ...tip, body: "Refunds above 250 EUR need a team lead's sign-off.", usage: { injected_count: 0, explicit_recall_count: 2 } },
        chain: [tip],
        injections: [
          { record_id: tip.id, identity: "billing", surface: "build", at_ms: NOW - 2 * HOUR },
          { record_id: tip.id, identity: "shipping", session_key: "sess-7", surface: "turn", at_ms: NOW - 5 * HOUR },
        ],
      }),
    );
    const view = renderPanel({ detail: wire });
    await act(async () => undefined);
    const life = text(view, "memory-detail-life");
    expect(life.includes("given to agents 2 · recalled 2")).toBe(true);
    expect(life.includes("1970")).toBe(false);
    const row = text(view, "memory-detail-injection:0");
    expect(row.startsWith("Given to Billing at startup")).toBe(true);
    expect(row.includes("2h ago")).toBe(true);
    expect(text(view, "memory-detail-injection:1").startsWith("Given to Shipping during a turn")).toBe(true);
  });

  it("reads 50+ given when the record read carries the server's 50 ledger rows", async () => {
    const injections = Array.from({ length: 50 }, (_, index) => ({
      record_id: tip.id,
      identity: "billing",
      surface: "turn",
      session_key: `sess-${index}`,
      at_ms: NOW - (index + 1) * HOUR,
    }));
    const view = renderPanel({ detail: { realm: "ops", record: { ...tip, body: "Body.", usage: { injected_count: 120 } } as MemoryFullRecord, chain: [tip], injections } });
    await act(async () => undefined);
    expect(text(view, "memory-detail-life").includes("given to agents 50+")).toBe(true);
  });

  it("leaves out proposals of a scope kind the probes denied", () => {
    const proposals: MemoryProposalEntry[] = [
      { realm: "ops", proposal_id: "p-op", scope_kind: "operator", scope_key: "lead@example.test", title: "Operator note", kind: "fact" },
      { realm: "ops", proposal_id: "p-team", scope_kind: "mob", scope_key: "customer-ops", title: "Team note", kind: "fact" },
    ];
    const view = renderPanel({ proposals, operatorScopeDenied: true });
    fireEvent.click(view.getByTestId("memory-tab:pipeline"));
    expect(view.queryByTestId("memory-proposal:p-op")).toBeNull();
    expect(view.queryByTestId("memory-proposal:p-team") === null).toBe(false);
  });

  it("draws the week of dream runs as bars that open their run", () => {
    const dreams: MemoryDreamRun[] = [
      { realm: "ops", run_id: "run-7", first_op_at_ms: NOW - 3 * HOUR, last_op_at_ms: NOW - 3 * HOUR + 60_000, ops: 4, quarantined_ops: 1 },
    ];
    const view = renderPanel({
      dreams,
      dreamRuns: [{ realm: "ops", run_id: "run-7", partition: "team", started_at_ms: NOW - 3 * HOUR - 90_000, completed_at_ms: NOW - 3 * HOUR + 60_000, ops_committed: 4 }],
    });
    fireEvent.click(view.getByTestId("memory-tab:dreams"));
    const bar = view.getByTestId("memory-dreams-bar:run-7");
    expect((bar.getAttribute("aria-label") ?? "").includes("Dream for the team")).toBe(true);
    expect((bar.getAttribute("aria-label") ?? "").includes("1 quarantined")).toBe(true);
    fireEvent.click(bar);
    expect(view.queryByTestId("memory-dream-run-detail:run-7") === null).toBe(false);
  });
});
