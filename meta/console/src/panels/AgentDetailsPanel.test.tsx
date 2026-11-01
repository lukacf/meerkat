import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { identityStateLabel, type IdentityInspectViewState, type IdentityStatusRow } from "@console-core";
import type { ConsoleAgent } from "../types";
import { AgentDetailsPanel, agentStatusChip, reachabilityWords } from "./AgentDetailsPanel";
import { HealthPanel } from "./HealthPanel";
import { SchedulesSection } from "./SchedulesSection";
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

beforeEach(() => {
  localStorage.clear();
});

const agent = { member_id: "identity:coordinator", identity: "identity:coordinator", agent_id: "identity:coordinator", label: "Coordinator", kind: "identity", role: "coordinator", group: "Leads" } as ConsoleAgent;

describe("status words", () => {
  it("lets the repair hold and health problems outrank the lifecycle state", () => {
    expect(agentStatusChip("needs repair", "healthy")).toEqual({ label: "Needs repair", tone: "warn" });
    expect(agentStatusChip("active", "wedged")).toEqual({ label: "Stalled", tone: "crit" });
    expect(agentStatusChip("active", "degraded")).toEqual({ label: "Degraded", tone: "warn" });
    expect(agentStatusChip("active", "healthy")).toEqual({ label: "Healthy", tone: "ok" });
    expect(agentStatusChip("retired", "unknown")).toEqual({ label: "Retired", tone: "neutral" });
    expect(agentStatusChip("active", undefined)).toEqual({ label: "Active", tone: "ok" });
    // A busy agent's health cannot be judged: it reads Working, never Unknown.
    expect(agentStatusChip("ready", "unknown", true)).toEqual({ label: "Working", tone: "live" });
    expect(agentStatusChip("ready", "unknown", false)).toEqual({ label: "Not observed", tone: "locked" });
    expect(agentStatusChip("ready", "healthy", true)).toEqual({ label: "Working", tone: "live" });
    expect(agentStatusChip("active", "wedged", true)).toEqual({ label: "Stalled", tone: "crit" });
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
    expect(panel.querySelector(".cc-panel-head__sub")?.textContent?.startsWith("Coordinator · Leads")).toBe(true);
    expect(view.getByTestId("inspect-state:identity:coordinator").textContent).toBe("Active");
    expect(panel.textContent?.includes("Can receive messages")).toBe(true);
    expect(panel.textContent?.includes("Research Analyst")).toBe(true);
    expect(panel.querySelector(".agent-details__preview")?.textContent).toBe("Drafted the reply and queued it for approval.");
    const tech = panel.querySelector("details.cc-tech") as HTMLDetailsElement | null;
    expect(tech !== null && tech.open === false).toBe(true);
    for (const id of ["identity:coordinator", "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D", "rt-01JBX4N0Q3T9"]) {
      expect(tech?.textContent?.includes(id)).toBe(true);
    }
    // Lifecycle actions live in the overflow menu; Retire asks first.
    expect(view.queryByTestId("inspect-action:identity:coordinator:retire") === null).toBe(true);
    fireEvent.click(view.getByTestId("inspect-menu:identity:coordinator"));
    const action = view.getByTestId("inspect-action:identity:coordinator:retire");
    expect(action.getAttribute("role")).toBe("menuitem");
    expect(action.classList.contains("is-destructive")).toBe(true);
    expect(view.queryByTestId("inspect-action:identity:coordinator:respawn") === null).toBe(true);
    fireEvent.click(action);
    expect(retire.mock.calls.length).toBe(0);
    const confirm = view.getByTestId("inspect-confirm:identity:coordinator:retire");
    expect(confirm.getAttribute("role")).toBe("alertdialog");
    expect(confirm.textContent?.includes("Retire Coordinator?")).toBe(true);
    expect(confirm.textContent?.includes("It stops and leaves the team.")).toBe(true);
    fireEvent.click(view.getByTestId("inspect-confirm-yes:identity:coordinator:retire"));
    expect(retire.mock.calls.length).toBe(1);
    expect(view.queryByTestId("inspect-confirm:identity:coordinator:retire") === null).toBe(true);
  });

  it("asks before a reset and cancels on Escape, and respawns at once", () => {
    const reset = vi.fn();
    const respawn = vi.fn();
    const view = render(
      <AgentDetailsPanel
        identity="identity:coordinator"
        inspect={inspect()}
        agent={agent}
        stateLabel="active"
        actions={{ respawn: { label: "Respawn", onClick: respawn }, reset: { label: "Reset", onClick: reset } }}
      />,
    );
    fireEvent.click(view.getByTestId("inspect-menu:identity:coordinator"));
    fireEvent.click(view.getByTestId("inspect-action:identity:coordinator:respawn"));
    expect(respawn.mock.calls.length).toBe(1);
    fireEvent.click(view.getByTestId("inspect-menu:identity:coordinator"));
    fireEvent.click(view.getByTestId("inspect-action:identity:coordinator:reset"));
    const confirm = view.getByTestId("inspect-confirm:identity:coordinator:reset");
    expect(document.activeElement === view.getByTestId("inspect-confirm-cancel:identity:coordinator:reset")).toBe(true);
    fireEvent.keyDown(confirm, { key: "Escape" });
    expect(view.queryByTestId("inspect-confirm:identity:coordinator:reset") === null).toBe(true);
    expect(reset.mock.calls.length).toBe(0);
  });

  it("reads a busy agent as Working with the live mark, never Unknown or Ready", () => {
    const busy = {
      ...agent,
      state: "active",
      response_phase: "tool-executing",
      progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 30_000, last_progress_event: "became_busy", health: "unknown" },
    } as ConsoleAgent;
    const view = render(
      <AgentDetailsPanel identity="identity:coordinator" inspect={inspect({ state: "ready" })} agent={busy} stateLabel="ready" actions={{}} />,
    );
    expect(view.getByTestId("inspect-state:identity:coordinator").textContent).toBe("Working");
    const pill = view.getByTestId("inspect-status:identity:coordinator");
    expect(pill.getAttribute("data-tone")).toBe("live");
    expect(view.getByTestId("inspect-now:identity:coordinator").querySelector(".agent-details__now")?.textContent?.startsWith("Running a tool")).toBe(true);
    expect(view.getByTestId("inspect-panel:identity:coordinator").querySelector(".cc-panel-head .cc-agent__avatar.is-live") !== null).toBe(true);
  });

  it("shows a glance at the day, the recent turns part and the ego graph of its peers", () => {
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel identity="identity:coordinator" inspect={inspect()} agent={agent} stateLabel="active" actions={{}} />
      </AgentDirectoryProvider>,
    );
    expect(view.getByTestId("inspect-glance:turns").textContent?.includes("Turns today")).toBe(true);
    expect(view.getByTestId("inspect-glance:failures").textContent?.includes("none today")).toBe(true);
    expect(view.getByTestId("inspect-recent-turns").textContent?.includes("No turns in the last 24 hours")).toBe(true);
    const ego = view.getByTestId("inspect-ego-graph");
    expect(ego.getAttribute("aria-roledescription")).toBe("graph");
    expect(ego.getAttribute("aria-label")).toBe("Coordinator and 1 connection");
  });

  it("lists the work it owns or claimed, the item in progress live, ids in tooltips", () => {
    const view = render(
      <AgentDetailsPanel
        identity="identity:coordinator"
        inspect={inspect()}
        agent={agent}
        stateLabel="active"
        actions={{}}
        work={[
          { id: "wi-41", title: "Collect vendor scorecards", status: "in_progress", claimedAt: new Date(NOW - 2 * 3_600_000).toISOString() },
          { id: "wi-44", title: "Summarize the scorecards", status: "ready" },
        ]}
      />,
    );
    const work = view.getByTestId("inspect-work:identity:coordinator");
    const first = view.getByTestId("inspect-work-item:wi-41");
    expect(first.classList.contains("is-live")).toBe(true);
    expect(first.textContent?.includes("Collect vendor scorecards")).toBe(true);
    expect(first.textContent?.includes("claimed 2h ago")).toBe(true);
    expect(first.querySelector('[title="wi-41"]') !== null).toBe(true);
    expect(work.textContent?.includes("wi-44")).toBe(false);
    expect(view.getByTestId("inspect-work-item:wi-44").textContent?.includes("Ready")).toBe(true);
  });

  it("explains an inspect answer it could not read, with Retry", () => {
    const refresh = vi.fn();
    const view = render(
      <AgentDetailsPanel identity="identity:coordinator" inspect={null} agent={agent} stateLabel={null} actions={{}} onRefresh={refresh} />,
    );
    expect(view.getByText("Couldn't load this agent's details").className).toBe("cc-panel-empty__title");
    fireEvent.click(view.getByRole("button", { name: "Retry" }));
    expect(refresh.mock.calls.length).toBe(1);
  });

  it("names an agent without a display name the way the sidebar does", () => {
    const unnamed = { ...agent, member_id: "router:main", identity: "router:main", agent_id: "router:main", label: "router:main", role: "router" } as ConsoleAgent;
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel
          identity="router:main"
          inspect={inspect({ identity: "router:main", display_name: undefined, role: "router" })}
          agent={unnamed}
          stateLabel="active"
          actions={{}}
        />
      </AgentDirectoryProvider>,
    );
    const panel = view.getByTestId("inspect-panel:router:main");
    expect(within(panel).getByRole("heading", { level: 2 }).textContent).toBe("Router main");
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

describe("Agent details headings", () => {
  // Every heading in document order, by its text and level.
  function outline(root: HTMLElement): Array<[string, number]> {
    return Array.from(root.querySelectorAll("h1, h2, h3, h4, h5, h6, [role='heading']")).map((heading) => [
      heading.textContent ?? "",
      Number(heading.getAttribute("aria-level") ?? heading.tagName.slice(1)),
    ]);
  }

  it("titles the panel h2 and each section below it h3, skipping no level", async () => {
    const schedule = {
      schedule_id: "sch_1",
      identity: "identity:coordinator",
      name: "Standup",
      content: "Post the standup summary",
      trigger: { type: "interval", start_at_utc: "2030-01-01T09:00:00Z", every_seconds: 7200 },
      phase: "active",
      revision: 1,
      next_due_at_utc: "2030-01-01T11:00:00Z",
    };
    const run = vi.fn(async (command: string) => (
      command === "listSchedules" ? { identity: "identity:coordinator", schedules: [schedule] } : {}
    ));
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel identity="identity:coordinator" inspect={inspect()} agent={agent} stateLabel="active" actions={{}}>
          <SchedulesSection identity="identity:coordinator" editing={{ canChange: true, note: null }} run={run} timezone="UTC" />
        </AgentDetailsPanel>
      </AgentDirectoryProvider>,
    );
    await waitFor(() => expect(view.queryByTestId("schedule:sch_1") !== null).toBe(true));
    // The schedule form adds no heading of its own.
    fireEvent.click(view.getByTestId("schedules-new:identity:coordinator"));
    const headings = outline(view.getByTestId("inspect-panel:identity:coordinator"));
    expect(headings).toEqual([["Coordinator", 2], ["Overview", 3], ["Latest output", 3], ["Schedules", 3]]);
    for (let index = 1; index < headings.length; index += 1) {
      expect(headings[index][1] <= headings[index - 1][1] + 1).toBe(true);
    }
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
    const summary = view.getByTestId("health-summary");
    expect(summary.classList.contains("cc-panel-head__sub")).toBe(true);
    expect(summary.textContent).toBe("3 agents · 1 healthy · 1 stalled");
    expect(summary.querySelector('[data-tone="crit"]')?.textContent?.includes("1 stalled")).toBe(true);
    const writer = view.getByTestId("health-identity:identity:writer");
    expect(writer.textContent?.includes("Docs Writer")).toBe(true);
    expect(writer.querySelector('.cc-chip[data-tone="crit"]')?.textContent).toBe("Stalled");
    // Since when, in words: the reason carries the age.
    expect(writer.textContent?.includes("stalled 47m")).toBe(true);
    const monitor = view.getByTestId("health-identity:identity:monitor");
    expect(monitor.textContent?.includes("Internal only")).toBe(true);
    expect(monitor.textContent?.includes("internal_only")).toBe(false);
    fireEvent.click(writer);
    expect(onOpen.mock.calls[0]?.[0]).toBe("identity:writer");
  });

  it("titles a nameless agent's Health row the way every surface names it", () => {
    const nameless = { identity: "router:main", state: "active", addressability: "addressable" } as IdentityStatusRow;
    const view = render(<HealthPanel identities={[nameless]} stateLabel={identityStateLabel} />);
    const row = view.getByTestId("health-identity:router:main");
    expect(row.querySelector(".cc-entity__title")?.textContent).toBe("Router main");
    expect(row.querySelector(".cc-entity__title [title]")?.getAttribute("title")).toBe("router:main");
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
    expect(view.getByTestId("health-identity:identity:writer").querySelector("button.cc-entity__hit") !== null).toBe(true);
    expect(view.getByTestId("health-identity:identity:monitor").querySelector("button.cc-entity__hit") === null).toBe(true);
  });

  it("has a designed empty state", () => {
    const view = render(<HealthPanel identities={[]} stateLabel={identityStateLabel} />);
    expect(view.getByText("No agents to report on yet").className).toBe("cc-panel-empty__title");
  });

  it("puts the worst first, counts each health at a glance and filters by a figure", () => {
    const degraded: IdentityStatusRow = { identity: "identity:release", display_name: "Release", state: "active", addressability: "addressable", labels: {}, progress: { run_state: "idle", in_flight_work: 1, last_progress_at_ms: NOW - 18 * 60_000, last_progress_event: "became_idle", health: "degraded" } };
    const view = render(<HealthPanel identities={[...rows, degraded]} stateLabel={identityStateLabel} onOpen={() => {}} />);
    const order = Array.from(view.getByTestId("health-tiles").querySelectorAll("[data-testid^='health-tile:']")).map((tile) => tile.getAttribute("data-testid"));
    expect(order).toEqual(["health-tile:identity:writer", "health-tile:identity:release", "health-tile:identity:coordinator", "health-tile:identity:monitor"]);
    expect(view.getByTestId("health-identity:identity:writer").textContent?.includes("stalled 47m · run open · 1 in flight")).toBe(true);
    expect(view.getByTestId("health-identity:identity:release").textContent?.includes("idle 18m · 1 in flight")).toBe(true);
    expect(view.getByTestId("health-glance:degraded").textContent?.includes("1")).toBe(true);
    fireEvent.click(view.getByTestId("health-glance:degraded"));
    expect(view.getByTestId("health-glance:degraded").getAttribute("aria-pressed")).toBe("true");
    expect(view.queryByTestId("health-tile:identity:writer") === null).toBe(true);
    expect(view.queryByTestId("health-tile:identity:release") !== null).toBe(true);
    expect(view.getByTestId("health-heat").querySelector("svg[role='img']") !== null).toBe(true);
  });

  it("reads a busy agent whose health the team cannot judge as Working, never Unknown", () => {
    const busy: IdentityStatusRow = { identity: "identity:orders", display_name: "Orders", state: "active", addressability: "addressable", labels: {}, progress: { run_state: "run_open", in_flight_work: 0, last_progress_at_ms: NOW - 60_000, last_progress_event: "became_busy", health: "unknown" } };
    const agents = [{ member_id: "identity:orders", identity: "identity:orders", agent_id: "identity:orders", label: "Orders", kind: "identity", response_phase: "tool-executing", progress: busy.progress } as ConsoleAgent];
    const view = render(<HealthPanel identities={[busy]} agents={agents} stateLabel={identityStateLabel} />);
    const tile = view.getByTestId("health-identity:identity:orders");
    expect(tile.querySelector('.cc-chip[data-tone="live"]')?.textContent).toBe("Working");
    expect(/unknown/i.test(tile.textContent || "")).toBe(false);
    expect(view.getByTestId("health-summary").textContent?.includes("1 working")).toBe(true);
  });

  it("says whether the team runs and which modules it loaded", () => {
    const view = render(
      <HealthPanel identities={rows} stateLabel={identityStateLabel} team={{ running: true, loadedModules: ["router", "delivery"], loadedModuleCount: 2 }} />,
    );
    expect(view.getByTestId("health-summary").textContent?.endsWith("team running · 2 modules")).toBe(true);
    const team = view.getByTestId("health-team");
    expect(team.textContent?.includes("Running")).toBe(true);
    expect(team.textContent?.includes("router")).toBe(true);
    expect(team.textContent?.includes("delivery")).toBe(true);
  });
});
