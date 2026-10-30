import { fireEvent, render, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { identityStateLabel, type IdentityInspectViewState, type IdentityStatusRow } from "@console-core";
import type { ConsoleAgent } from "../types";
import { AgentDetailsPanel, agentStatusChip, reachabilityWords } from "./AgentDetailsPanel";
import { HealthPanel } from "./HealthPanel";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

const NOW = Date.now();
const directory = createAgentDirectory([
  { id: "identity:coordinator", label: "Coordinator" },
  { id: "identity:analyst", label: "Research Analyst" },
]);

function inspect(overrides: Partial<IdentityInspectViewState> = {}): IdentityInspectViewState {
  return {
    identity: "identity:coordinator",
    display_name: "Coordinator",
    role: "coordinator",
    state: "active",
    addressability: "addressable",
    labels: {},
    continuity: { generation: 4, session_id: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D", agent_runtime_id: "rt-01JBX4N0Q3T9" },
    topology_peers: ["identity:analyst"],
    output_preview: "Drafted the reply and queued it for approval.",
    ...overrides,
  } as IdentityInspectViewState;
}

const agent = { member_id: "identity:coordinator", identity: "identity:coordinator", agent_id: "identity:coordinator", label: "Coordinator", kind: "identity", role: "coordinator", group: "Leads" } as ConsoleAgent;

describe("status words", () => {
  it("lets the repair hold and health problems outrank the lifecycle state", () => {
    expect(agentStatusChip("needs repair", "healthy")).toEqual({ label: "Needs repair", tone: "warn" });
    expect(agentStatusChip("active", "wedged")).toEqual({ label: "Stalled", tone: "crit" });
    expect(agentStatusChip("active", "degraded")).toEqual({ label: "Degraded", tone: "warn" });
    expect(agentStatusChip("active", "healthy")).toEqual({ label: "Healthy", tone: "ok" });
    expect(agentStatusChip("retired", "unknown")).toEqual({ label: "Retired", tone: "neutral" });
    expect(agentStatusChip("active", undefined)).toEqual({ label: "Active", tone: "ok" });
    expect(reachabilityWords("addressable")).toBe("Can receive messages");
    expect(reachabilityWords("internal_only")).toBe("Internal only");
  });
});

describe("AgentDetailsPanel", () => {
  it("leads with the display name and role and keeps IDs in collapsed technical details", () => {
    const retire = vi.fn();
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel
          identity="identity:coordinator"
          inspect={inspect()}
          agent={agent}
          stateLabel="active"
          actions={{ retire: { label: "Retire", onClick: retire } }}
        />
      </AgentDirectoryProvider>,
    );
    const panel = view.getByTestId("inspect-panel:identity:coordinator");
    expect(within(panel).getByRole("heading", { level: 2 }).textContent).toBe("Coordinator");
    expect(panel.querySelector(".cc-panel-head__sub")?.textContent).toBe("Coordinator · Leads");
    expect(view.getByTestId("inspect-state:identity:coordinator").textContent).toBe("Active");
    expect(panel.textContent?.includes("Can receive messages")).toBe(true);
    expect(panel.textContent?.includes("Research Analyst")).toBe(true);
    expect(panel.querySelector(".agent-details__preview")?.textContent).toBe("Drafted the reply and queued it for approval.");
    const tech = panel.querySelector("details.cc-tech") as HTMLDetailsElement | null;
    expect(tech !== null && tech.open === false).toBe(true);
    for (const id of ["identity:coordinator", "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D", "rt-01JBX4N0Q3T9"]) {
      expect(tech?.textContent?.includes(id)).toBe(true);
    }
    const action = view.getByTestId("inspect-action:identity:coordinator:retire");
    expect(action.classList.contains("cc-btn--danger")).toBe(true);
    fireEvent.click(action);
    expect(retire.mock.calls.length).toBe(1);
    expect(view.queryByTestId("inspect-action:identity:coordinator:respawn") === null).toBe(true);
  });

  it("takes health from the roster when the inspect payload carries no progress", () => {
    const stalled = {
      ...agent,
      progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 47 * 60_000, last_progress_event: "execution_advanced", health: "wedged" },
    } as ConsoleAgent;
    // The gateway's inspect payload has no progress field.
    const view = render(
      <AgentDetailsPanel identity="identity:coordinator" inspect={inspect()} agent={stalled} stateLabel="active" actions={{}} />,
    );
    expect(view.getByTestId("inspect-state:identity:coordinator").textContent).toBe("Stalled");
    const chip = view.getByTestId("inspect-panel:identity:coordinator").querySelector(".cc-panel-head .cc-chip");
    expect(chip?.textContent).toBe("Stalled");
    expect(chip?.getAttribute("data-tone")).toBe("crit");
  });

  it("explains a session repair with both commands, each copyable", () => {
    const repairing = inspect({
      session_repair: {
        session_id: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D",
        hold: "repair",
        diagnose_command: "rkat session diagnose session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D",
        apply_command: "rkat session repair session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D",
        detail: "",
      },
    });
    const view = render(
      <AgentDetailsPanel
        identity="identity:coordinator"
        inspect={repairing}
        agent={agent}
        stateLabel={identityStateLabel(repairing)}
        actions={{}}
      />,
    );
    expect(view.getByTestId("inspect-state:identity:coordinator").textContent).toBe("Needs repair");
    const repair = view.getByTestId("inspect-session-repair:identity:coordinator");
    expect(repair.querySelector('.cc-notice[data-tone="warn"]') !== null).toBe(true);
    expect(view.getByTestId("inspect-session-repair-diagnose").textContent).toBe("rkat session diagnose session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D");
    expect(view.getByTestId("inspect-session-repair-apply").textContent).toBe("rkat session repair session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D");
    expect(within(repair).getByRole("button", { name: "Copy the diagnose command" }).tagName).toBe("BUTTON");
    expect(within(repair).getByRole("button", { name: "Copy the repair command" }).tagName).toBe("BUTTON");
  });

  it("shows a loading skeleton until the details arrive", () => {
    const view = render(
      <AgentDetailsPanel identity="identity:coordinator" inspect={undefined} agent={agent} stateLabel={null} actions={{}} />,
    );
    expect(view.getByRole("status").textContent).toBe("Loading agent details");
    expect(within(view.getByTestId("inspect-panel:identity:coordinator")).getByRole("heading", { level: 2 }).textContent).toBe("Coordinator");
  });
});

describe("HealthPanel", () => {
  const rows: IdentityStatusRow[] = [
    { identity: "identity:coordinator", display_name: "Coordinator", state: "active", addressability: "addressable", labels: {}, progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 120_000, last_progress_event: "execution_advanced", health: "healthy" } },
    { identity: "identity:writer", display_name: "Docs Writer", state: "active", addressability: "addressable", labels: {}, progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 47 * 60_000, last_progress_event: "execution_advanced", health: "wedged" } },
    { identity: "identity:monitor", display_name: "Nightly Monitor", state: "retired", addressability: "internal_only", labels: {} },
  ];

  it("is a status board: names, status words with tone, reachability in words, counts", () => {
    const onOpen = vi.fn();
    const view = render(<HealthPanel identities={rows} stateLabel={identityStateLabel} onOpen={onOpen} />);
    expect(view.getByRole("heading", { level: 2 }).textContent).toBe("Health");
    expect(view.getByText("3 agents · 1 healthy · 1 stalled").className).toBe("cc-panel-head__sub");
    const writer = view.getByTestId("health-identity:identity:writer");
    expect(writer.textContent?.includes("Docs Writer")).toBe(true);
    expect(writer.querySelector('.cc-chip[data-tone="crit"]')?.textContent).toBe("Stalled");
    expect(writer.textContent?.includes("47m ago")).toBe(true);
    const monitor = view.getByTestId("health-identity:identity:monitor");
    expect(monitor.textContent?.includes("Internal only")).toBe(true);
    expect(monitor.textContent?.includes("internal_only")).toBe(false);
    fireEvent.click(writer);
    expect(onOpen.mock.calls[0]?.[0]).toBe("identity:writer");
  });

  it("makes a row a button only when its agent's details can open", () => {
    const view = render(
      <HealthPanel
        identities={rows}
        stateLabel={identityStateLabel}
        onOpen={() => {}}
        canOpen={(identity) => identity !== "identity:monitor"}
      />,
    );
    expect(view.getByTestId("health-identity:identity:writer").tagName).toBe("BUTTON");
    expect(view.getByTestId("health-identity:identity:monitor").tagName).toBe("DIV");
  });

  it("has a designed empty state", () => {
    const view = render(<HealthPanel identities={[]} stateLabel={identityStateLabel} />);
    expect(view.getByText("No agents to report on yet").className).toBe("cc-panel-empty__title");
  });
});
