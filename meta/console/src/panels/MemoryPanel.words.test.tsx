/**
 * The Memory panel in plain words: grants as sentences, team keys as team
 * names, the store's own terms (superseded, tombstoned, tainted, realm,
 * injection history) as everyday words, with the raw term or key kept as
 * the tooltip.
 */
import React from "react";
import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type {
  MemoryDreamRunSheet,
  MemoryFullRecord,
  MemoryLedgerEntry,
  MemoryPanelRecord,
  MemoryPendingPromotion,
  MemoryProposalEntry,
} from "../types";
import { MemoryPanel } from "./MemoryPanel";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

const NOW = Date.now();
const HOUR = 60 * 60 * 1000;

function record(
  overrides: Partial<MemoryPanelRecord> & Pick<MemoryPanelRecord, "id" | "title">,
): MemoryPanelRecord {
  return {
    kind: "fact",
    trust: "agent_observed",
    status: { status: "active" },
    scope: { scope: "identity", realm: "ops", identity: "identity:coordinator" },
    updated_at_ms: NOW - HOUR,
    ...overrides,
  };
}

const current = record({ id: "mem-current", title: "Release train cutoff is Thursday" });
const replaced = record({
  id: "mem-replaced",
  title: "Release train cutoff is Wednesday",
  status: { status: "superseded", by: "mem-current" },
});
const discarded = record({
  id: "mem-discarded",
  title: "Billing questions go to finance",
  status: { status: "tombstoned" },
});
const teamRecord = record({
  id: "mem-team",
  title: "Escalation path for the on-call rotation",
  scope: { scope: "mob", realm: "ops", mob: "ops-team" },
});

type PanelProps = Partial<React.ComponentProps<typeof MemoryPanel>>;

function renderPanel(props: PanelProps = {}) {
  const directory = createAgentDirectory([{ id: "identity:coordinator", label: "Coordinator" }]);
  return render(
    <AgentDirectoryProvider directory={directory}>
      <MemoryPanel
        records={[current, replaced, discarded, teamRecord]}
        realms={["ops"]}
        quarantineRecords={[]}
        pendingPromotions={[]}
        dreams={[]}
        detail={null}
        onRefresh={() => undefined}
        onSelectRecord={() => undefined}
        onClearDetail={() => undefined}
        onQueryRecords={async () => null}
        {...props}
      />
    </AgentDirectoryProvider>,
  );
}

function text(view: ReturnType<typeof render>, testId: string): string {
  return view.getByTestId(testId).textContent ?? "";
}

function hasTitle(view: ReturnType<typeof render>, testId: string, title: string): boolean {
  return Array.from(view.getByTestId(testId).querySelectorAll("[title]")).some(
    (element) => element.getAttribute("title") === title,
  );
}

function openTab(view: ReturnType<typeof render>, tab: string): void {
  fireEvent.click(view.getByTestId(`memory-tab:${tab}`));
}

describe("Memory panel words", () => {
  it("says which permission is missing in a sentence and keeps the grant name in the tooltip", () => {
    const view = renderPanel({ canReviewQuarantine: false, mobScopeDenied: true, operatorScopeDenied: true });

    const team = text(view, "memory-holdings-scope-denied:mob");
    expect(team.includes("You need permission to read team memory")).toBe(true);
    expect(team.includes("mob.memory.read")).toBe(false);
    expect(hasTitle(view, "memory-holdings-scope-denied:mob", "Grant: mob.memory.read")).toBe(true);
    const operator = text(view, "memory-holdings-scope-denied:operator");
    expect(operator.includes("You need permission to read operator memory")).toBe(true);
    expect(hasTitle(view, "memory-holdings-scope-denied:operator", "Grant: operator.memory.read")).toBe(true);

    const holdings = text(view, "memory-holdings");
    expect(holdings.includes("You need permission to review quarantined memory")).toBe(true);
    expect(holdings.includes("memory.quarantine.review")).toBe(false);
    expect(hasTitle(view, "memory-holdings", "Grant: memory.quarantine.review")).toBe(true);

    openTab(view, "pipeline");
    expect(text(view, "memory-pipeline-no-grant")).toBe(
      "Quarantine: no grant. You need permission to review quarantined memory to see the queue.",
    );
    expect(hasTitle(view, "memory-pipeline-no-grant", "Grant: memory.quarantine.review")).toBe(true);
  });

  it("names a team from its key and keeps the key as the tooltip", () => {
    const proposal: MemoryProposalEntry = {
      realm: "ops",
      proposal_id: "prop-1",
      scope_kind: "mob",
      scope_key: "ops-team",
      title: "Share the vendor contact list",
      kind: "reference",
      tainted: true,
    };
    const promotion: MemoryPendingPromotion = {
      realm: "ops",
      pending_id: "gate-1",
      record_id: "mem-team",
      scope_kind: "mob",
      scope_key: "ops-team",
    };
    const view = renderPanel({ canReviewQuarantine: true, proposals: [proposal], pendingPromotions: [promotion] });

    const row = "memory-holdings-scope:mob:ops:ops-team";
    expect(text(view, row).includes("Ops team")).toBe(true);
    expect(text(view, row).includes("ops-team")).toBe(false);
    expect(hasTitle(view, row, "ops-team")).toBe(true);

    openTab(view, "records");
    const group = "memory-group:mob:ops:ops-team";
    expect(text(view, group).includes("Team memory · Ops team")).toBe(true);
    expect(text(view, group).includes("ops-team")).toBe(false);
    expect(hasTitle(view, group, "ops-team")).toBe(true);

    openTab(view, "pipeline");
    const proposed = text(view, "memory-proposal:prop-1");
    expect(proposed.includes("Reference for Ops team")).toBe(true);
    expect(proposed.includes("ops-team")).toBe(false);
    expect(hasTitle(view, "memory-proposal:prop-1", "ops-team")).toBe(true);
    const waiting = text(view, "memory-pending:gate-1");
    expect(waiting.includes("To Ops team")).toBe(true);
    expect(waiting.includes("ops-team")).toBe(false);
    // A proposal from an untrusted session reads so; "tainted" is the tooltip.
    const taint = view.getByTestId("memory-proposal-taint:prop-1");
    expect(taint.textContent).toBe("Untrusted source");
    expect((taint.getAttribute("title") ?? "").startsWith("Tainted:")).toBe(true);
  });

  it("says replaced and discarded, with the store's terms in the tooltips", () => {
    const view = renderPanel();
    // Holdings counts the agent's records in the same words.
    expect(
      text(view, "memory-holdings-scope:identity:ops:identity:coordinator").includes("1 active, 1 replaced, 1 discarded"),
    ).toBe(true);

    openTab(view, "records");
    const replacedRow = text(view, "memory-record:mem-replaced");
    expect(replacedRow.includes("Replaced")).toBe(true);
    expect(/superseded|replaced by a newer version/i.test(replacedRow)).toBe(false);
    expect(hasTitle(view, "memory-record:mem-replaced", "Superseded: a newer version replaced this record.")).toBe(true);
    const discardedRow = text(view, "memory-record:mem-discarded");
    expect(discardedRow.includes("Discarded")).toBe(true);
    expect(/tombstone/i.test(discardedRow)).toBe(false);
    expect(
      hasTitle(view, "memory-record:mem-discarded", "Tombstoned: kept only as a deletion marker; it is never recalled."),
    ).toBe(true);

    const statuses = Array.from(
      view.getByTestId("memory-filter:status").querySelectorAll("option"),
      (option) => option.textContent,
    );
    expect(statuses.join(",")).toBe("All,Active,Quarantined,Replaced,Discarded");
    expect(
      Array.from(view.getByTestId("memory-filter:status").querySelectorAll("option"), (option) => option.value).join(","),
    ).toBe(",active,quarantined,superseded,tombstoned");
  });

  it("offers Discard, never Tombstone, on a quarantined record", () => {
    const quarantined: MemoryFullRecord = {
      ...record({
        id: "mem-q",
        title: "Upstream API token pasted into the chat",
        status: { status: "quarantined", reason: "session tainted by configured untrusted tool" },
      }),
      body: "The token is in the transcript.",
      content_hash: "c0ffee",
    };
    const view = renderPanel({
      detail: { realm: "ops", record: quarantined, chain: [], injections: [] },
      onDecideQuarantine: async () => {
        throw new Error("not decided in this test");
      },
    });
    const block = text(view, "memory-quarantine-decide");
    expect(/tombstone/i.test(block)).toBe(false);
    expect(block.includes("Discard marks it deleted for good, so it is never recalled.")).toBe(true);
    const discard = view.getByTestId("memory-quarantine-tombstone");
    expect(discard.textContent).toBe("Discard");
    expect((discard.getAttribute("title") ?? "").startsWith("Tombstone:")).toBe(true);
    expect(view.getByTestId("memory-quarantine-rationale").getAttribute("placeholder")).toBe(
      "Why you release or discard it",
    );
  });

  it("calls realms spaces and keeps the realm in the tooltip", () => {
    const lab = record({
      id: "mem-lab",
      title: "Lab runs start at noon",
      scope: { scope: "identity", realm: "lab", identity: "identity:coordinator" },
    });
    const view = renderPanel({ records: [current, lab], realms: ["ops", "lab"] });
    const panel = text(view, "memory-panel");
    expect(panel.includes("2 spaces")).toBe(true);
    expect(/realm/i.test(panel)).toBe(false);
    const scope = "memory-holdings-scope:identity:ops:identity:coordinator";
    expect(text(view, scope).includes("ops space")).toBe(true);
    expect(hasTitle(view, scope, "Realm ops")).toBe(true);

    openTab(view, "records");
    const realmFilter = view.getByTestId("memory-filter:realm");
    expect(realmFilter.querySelector("option")?.textContent).toBe("All spaces");
    expect(view.getByLabelText("Space").getAttribute("data-testid")).toBe("memory-filter:realm");
    expect(text(view, "memory-multi-realm-note")).toBe(
      "This page combines records from several spaces. Pick a space to page through its records.",
    );
    const group = "memory-group:identity:lab:identity:coordinator";
    expect(text(view, group).includes("lab space")).toBe(true);
    expect(hasTitle(view, group, "Realm lab")).toBe(true);
    expect(/realm/i.test(text(view, "memory-panel"))).toBe(false);
  });

  it("calls the injection history the memory given to agents", () => {
    const given: MemoryLedgerEntry = {
      realm: "ops",
      record_id: "mem-current",
      identity: "identity:coordinator",
      surface: "turn",
      at_ms: NOW - HOUR,
    };
    const view = renderPanel({ injections: [given] });
    openTab(view, "knowledge");
    const section = text(view, "memory-knowledge-history");
    expect(section.includes("Memory given to agents")).toBe(true);
    expect(/injection history/i.test(section)).toBe(false);
    expect(hasTitle(view, "memory-knowledge-history", "Injection history")).toBe(true);
  });

  it("finds a team's records by the team name the headings show", async () => {
    const queries: Array<Record<string, unknown>> = [];
    const view = renderPanel({
      onQueryRecords: async (params) => {
        queries.push(params);
        return { records: [teamRecord], realms: ["ops"] };
      },
    });
    openTab(view, "records");
    fireEvent.change(view.getByTestId("memory-filter:scope"), { target: { value: "mob" } });
    fireEvent.change(view.getByTestId("memory-filter-input"), { target: { value: "Ops team" } });
    fireEvent.keyDown(view.getByTestId("memory-filter-input"), { key: "Enter" });
    expect(JSON.stringify(queries.at(-1))).toBe(JSON.stringify({ scope: "mob", scope_key: "ops-team" }));
  });

  it("names a team's dream run and its changes in plain words", () => {
    const run: MemoryDreamRunSheet = {
      realm: "ops",
      run_id: "run-1",
      partition: "mob:ops-team",
      started_at_ms: NOW - 2 * HOUR,
      completed_at_ms: NOW - HOUR,
      ops_committed: 2,
      detail: { verdicts: { quarantine_tombstoned: 2 } },
    };
    const view = renderPanel({ dreamRuns: [run] });
    openTab(view, "dreams");
    const row = text(view, "memory-dream-run:run-1");
    expect(row.includes("Dream for Ops team")).toBe(true);
    expect(row.includes("2 quarantine discarded")).toBe(true);
    expect(/ops-team|tombston/i.test(row)).toBe(false);
    // The steward's own counter name is the chip's tooltip.
    expect(hasTitle(view, "memory-dream-run:run-1", "quarantine_tombstoned")).toBe(true);
  });

  it("keeps an evidence reference's generation in the technical details, never on the link", () => {
    const withEvidence: MemoryFullRecord = {
      ...record({
        id: "mem-evidence",
        title: "Standup moves to 09:30",
        provenance: { author: { author: "operator" }, evidence: [{ generation: 3, range: [1, 2] }] },
      }),
      body: "Standup moves to 09:30 on Fridays.",
    };
    const view = renderPanel({
      detail: { realm: "ops", record: withEvidence, chain: [], injections: [] },
      onLoadEvidence: async () => null,
    });
    expect(text(view, "memory-evidence:0")).toBe("Conversation with Coordinator, messages 1-2");
    const technical = view.getByTestId("memory-detail").querySelector("details")?.textContent ?? "";
    expect(technical.includes("generation 3")).toBe(true);
  });
});
