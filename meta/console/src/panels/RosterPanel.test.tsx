import { act, fireEvent, render, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ConsoleAgent } from "../types";
import { agentDirectoryEntry } from "../lib/agents";
import { RosterPanel, rosterActivity, rosterDotTone, rosterHealthChip, rosterStatus } from "./RosterPanel";
import { agentHealth } from "../lib/agent-status";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

const NOW = Date.now();

beforeEach(() => {
  localStorage.clear();
});

function agent(overrides: Partial<ConsoleAgent> & { member_id: string; label: string }): ConsoleAgent {
  return {
    agent_id: overrides.member_id,
    identity: overrides.member_id,
    kind: "identity",
    state: "active",
    ...overrides,
  } as ConsoleAgent;
}

const coordinator = agent({
  member_id: "identity:coordinator",
  label: "Coordinator",
  role: "coordinator",
  session_id: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D",
  generation: 4,
  wired_to: ["identity:analyst", "identity:unknown-peer"],
  progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 2 * 60_000, last_progress_event: "execution_advanced", health: "healthy" },
  affordances: { can_respawn: true, can_retire: true },
});
const analyst = agent({
  member_id: "identity:analyst",
  label: "Research Analyst",
  role: "analyst",
  response_phase: "tool-executing",
  progress: { run_state: "run_open", in_flight_work: 2, last_progress_at_ms: NOW - 40_000, last_progress_event: "execution_advanced", health: "healthy" },
});
const writer = agent({
  member_id: "identity:writer",
  label: "Docs Writer",
  role: "writer",
  progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 47 * 60_000, last_progress_event: "execution_advanced", health: "wedged" },
});
const monitor = agent({ member_id: "identity:monitor", label: "Nightly Monitor", role: "monitor", state: "retired", group: "internal" });
const idler = agent({
  member_id: "identity:idler",
  label: "Idle Ivy",
  role: "assistant",
  progress: { run_state: "idle", in_flight_work: 0, last_progress_at_ms: NOW - 5 * 60_000, last_progress_event: "became_idle", health: "healthy" },
});

function renderRoster(agents: ConsoleAgent[], extra: Partial<Parameters<typeof RosterPanel>[0]> = {}) {
  const handlers = {
    onSelect: vi.fn(),
    onChat: vi.fn(),
    onDetails: vi.fn(),
    onLifecycle: vi.fn(),
  };
  // The directory the console builds: names as the sidebar shows them.
  const directory = createAgentDirectory(agents.map(agentDirectoryEntry));
  const view = render(
    <AgentDirectoryProvider directory={directory}>
      <RosterPanel agents={agents} canResetLifecycle {...handlers} {...extra} />
    </AgentDirectoryProvider>,
  );
  return { view, ...handlers };
}

function hit(row: HTMLElement): HTMLElement {
  return row.querySelector(".cc-entity__hit") as HTMLElement;
}

describe("Roster words", () => {
  it("says what an agent is doing instead of raw run states", () => {
    expect(rosterActivity(coordinator)).toBe("working");
    expect(rosterActivity(analyst)).toBe("running a tool");
    expect(rosterActivity({ ...analyst, response_phase: "generating" })).toBe("writing a reply");
    expect(rosterActivity({ ...coordinator, progress: { ...coordinator.progress!, in_flight_work: 3 } })).toBe("working on 3 tasks");
    expect(rosterActivity({ ...coordinator, progress: { ...coordinator.progress!, run_state: "idle", in_flight_work: 0 } })).toBe("idle");
    expect(rosterActivity(monitor)).toBe("retired");
    // A stalled agent's run is still open; it is not "working".
    expect(rosterActivity(writer)).toBe("stalled");
  });

  it("raises a chip only for health problems and never tones status with the accent", () => {
    expect(rosterHealthChip(coordinator)).toBe(null);
    expect(rosterHealthChip(writer)).toEqual({ label: "Stalled", tone: "crit" });
    expect(rosterHealthChip({ ...coordinator, progress: { ...coordinator.progress!, health: "degraded" } })).toEqual({ label: "Degraded", tone: "warn" });
    expect(rosterHealthChip(monitor)).toBe(null);
    expect(rosterDotTone(coordinator)).toBe("ok");
    expect(rosterDotTone(writer)).toBe("crit");
    expect(rosterDotTone(monitor)).toBe("neutral");
  });

  it("gives a busy agent whose health the team cannot judge the live Working pill, never Unknown", () => {
    const busy = { ...analyst, progress: { ...analyst.progress!, health: "unknown" } };
    expect(rosterStatus(agentHealth(busy), busy, true)).toEqual({ word: "Working", tone: "live" });
    expect(rosterStatus(agentHealth(idler), idler, false)).toEqual({ word: "Idle", tone: "neutral" });
    expect(rosterStatus(agentHealth(writer), writer, false)).toEqual({ word: "Stalled", tone: "crit" });
    expect(rosterStatus(agentHealth(monitor), monitor, false)).toEqual({ word: "Retired", tone: "neutral" });
  });
});

describe("RosterPanel", () => {
  it("lists agents by name with role and activity, and keeps keys out of the rows", () => {
    const { view } = renderRoster([coordinator, analyst, writer, monitor]);
    const row = view.getByTestId("roster-row:identity:analyst");
    expect(hit(row).tagName).toBe("BUTTON");
    expect(row.textContent?.includes("Research Analyst")).toBe(true);
    expect(row.textContent?.includes("Analyst · running a tool")).toBe(true);
    expect(row.textContent?.includes("identity:analyst")).toBe(false);
    expect(row.querySelector(".cc-entity__title [title]")?.getAttribute("title")).toBe("identity:analyst");
    expect(view.getByTestId("roster-row:identity:writer").textContent?.includes("Stalled")).toBe(true);
    expect(view.getByTestId("roster-row:identity:writer").getAttribute("data-health")).toBe("wedged");
    const summary = view.getByTestId("roster-summary");
    expect(summary.classList.contains("cc-panel-head__sub")).toBe(true);
    expect(summary.textContent).toBe("3 agents · 2 working · 1 needs attention · 1 retired");
    expect(summary.querySelector('[data-tone="live"]')?.textContent?.includes("2 working")).toBe(true);
  });

  it("answers the first question at a glance and filters by a figure", () => {
    const { view } = renderRoster([coordinator, analyst, writer, idler, monitor]);
    expect(view.getByTestId("roster-glance:agents").textContent?.includes("4")).toBe(true);
    expect(view.getByTestId("roster-glance:working").textContent?.includes("2")).toBe(true);
    expect(view.getByTestId("roster-glance:idle").textContent?.includes("1")).toBe(true);
    const attention = view.getByTestId("roster-glance:attention");
    expect(attention.getAttribute("data-tone")).toBe("crit");
    expect(attention.textContent?.includes("1 stalled")).toBe(true);

    fireEvent.click(view.getByTestId("roster-glance:working"));
    expect(view.getByTestId("roster-glance:working").getAttribute("aria-pressed")).toBe("true");
    expect(view.queryByTestId("roster-row:identity:idler") === null).toBe(true);
    expect(view.queryByTestId("roster-row:identity:analyst") !== null).toBe(true);
    // The filter shows as a chip the viewer can clear.
    fireEvent.click(view.getByTestId("roster-filter:working"));
    expect(view.queryByTestId("roster-row:identity:idler") !== null).toBe(true);
  });

  it("switches between cards and a list and keeps the choice", () => {
    const { view } = renderRoster([coordinator, analyst, writer]);
    fireEvent.click(view.getByTestId("roster-view:cards"));
    expect(view.getByTestId("roster-view:cards").getAttribute("aria-pressed")).toBe("true");
    const card = view.getByTestId("roster-card:identity:analyst");
    expect(card.classList.contains("cc-entity--card")).toBe(true);
    expect(card.classList.contains("is-live")).toBe(true);
    expect(card.querySelector(".cc-entity__now")?.textContent?.startsWith("Running a tool")).toBe(true);
    expect(card.querySelector('.cc-chip[data-tone="live"]')?.textContent).toBe("Working");
    expect(within(card).getByRole("button", { name: "Open chat: Research Analyst" }).tagName).toBe("BUTTON");
    expect(view.getByTestId("roster-card:identity:writer").querySelector('.cc-chip[data-tone="crit"]')?.textContent).toBe("Stalled");
    expect(view.queryByTestId("roster-row:identity:analyst") === null).toBe(true);
    expect(localStorage.getItem("meta-console-roster-view:default")).toBe("cards");
    fireEvent.click(view.getByTestId("roster-view:list"));
    expect(view.queryByTestId("roster-row:identity:analyst") !== null).toBe(true);
  });

  it("selects with the keyboard and opens the detail with words first and IDs collapsed", () => {
    const { view, onSelect } = renderRoster([coordinator, analyst]);
    const row = view.getByTestId("roster-row:identity:coordinator");
    fireEvent.click(hit(row));
    expect(onSelect.mock.calls[0]?.[0]?.member_id).toBe("identity:coordinator");
    expect(hit(row).getAttribute("aria-current")).toBe("true");
    const layout = view.getByTestId("roster-layout");
    expect(layout.classList.contains("has-drawer")).toBe(true);

    const detail = view.getByTestId("roster-detail:identity:coordinator");
    expect(within(detail).getByRole("heading", { name: "Coordinator" }).tagName).toBe("H3");
    expect(detail.textContent?.includes("Working")).toBe(true);
    expect(detail.textContent?.includes("1 task")).toBe(true);
    const tech = detail.querySelector("details.cc-tech") as HTMLDetailsElement | null;
    expect(tech !== null && tech.open === false).toBe(true);
    expect(tech?.textContent?.includes("session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D")).toBe(true);
    expect(within(detail).getByRole("button", { name: "Copy session" }).classList.contains("cc-copy-btn")).toBe(true);
    // The day's activity and the hour-by-hour strip are in the drawer.
    expect(view.getByTestId("roster-detail-activity").querySelector("svg[role='img']") !== null).toBe(true);
    expect(view.getByTestId("roster-detail-heat-strip").querySelectorAll("rect.cc-heat__cell").length).toBe(96);

    fireEvent.click(view.getByRole("button", { name: "All agents" }));
    expect(layout.classList.contains("has-drawer")).toBe(false);
  });

  it("names an agent without a display name the way the sidebar does", () => {
    const router = agent({ member_id: "router:main", label: "router:main", role: "router" });
    const generated = agent({ member_id: "agent:01HXYZ8Q9ABCDEF", label: "agent:01HXYZ8Q9ABCDEF", role: "reviewer" });
    const { view } = renderRoster([router, generated, coordinator]);
    const row = view.getByTestId("roster-row:router:main");
    expect(row.querySelector(".cc-entity__title")?.textContent).toBe("Router main");
    expect(row.querySelector(".cc-entity__title [title]")?.getAttribute("title")).toBe("router:main");
    expect(view.getByTestId("roster-row:agent:01HXYZ8Q9ABCDEF").querySelector(".cc-entity__title")?.textContent).toBe("Reviewer abcdef");
    fireEvent.click(hit(row));
    const detail = view.getByTestId("roster-detail:router:main");
    expect(within(detail).getByRole("heading", { level: 3 }).textContent).toBe("Router main");
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "reviewer abc" } });
    expect(view.queryByTestId("roster-row:agent:01HXYZ8Q9ABCDEF") !== null).toBe(true);
    expect(view.queryByTestId("roster-row:router:main") === null).toBe(true);
  });

  it("shows peers by name and selects a known peer", () => {
    const { view, onSelect } = renderRoster([coordinator, analyst]);
    fireEvent.click(hit(view.getByTestId("roster-row:identity:coordinator")));
    const detail = view.getByTestId("roster-detail:identity:coordinator");
    expect(detail.textContent?.includes("Unknown peer")).toBe(true);
    expect(detail.textContent?.includes("identity:unknown-peer")).toBe(false);
    const peer = within(detail).getByRole("button", { name: "Research Analyst" });
    expect(peer.getAttribute("title")).toBe("identity:analyst");
    fireEvent.click(peer);
    expect(onSelect.mock.calls.at(-1)?.[0]?.member_id).toBe("identity:analyst");
  });

  it("never says a stalled agent is working", () => {
    const stalled = agent({ ...writer, progress: { ...writer.progress!, last_progress_event: "became_busy" } });
    const { view } = renderRoster([stalled]);
    const row = view.getByTestId("roster-row:identity:writer");
    fireEvent.click(hit(row));
    const detail = view.getByTestId("roster-detail:identity:writer");
    expect(/working/i.test(row.textContent || "")).toBe(false);
    expect(/working/i.test(detail.textContent || "")).toBe(false);
    expect(detail.textContent?.includes("Stalled")).toBe(true);
    expect(detail.textContent?.includes("Last progress")).toBe(true);
  });

  it("offers chat first and groups lifecycle actions, Retire as danger behind a confirmation", () => {
    const { view, onChat, onLifecycle } = renderRoster([coordinator]);
    fireEvent.click(hit(view.getByTestId("roster-row:identity:coordinator")));
    const detail = view.getByTestId("roster-detail:identity:coordinator");
    fireEvent.click(within(detail).getByRole("button", { name: "Open chat" }));
    expect(onChat.mock.calls.length).toBe(1);
    expect(within(detail).getByRole("button", { name: "Open chat" }).classList.contains("cc-btn--primary")).toBe(true);
    expect(within(detail).getByRole("group", { name: "Lifecycle" }).textContent).toBe("RespawnResetRetire");
    const retire = within(detail).getByRole("button", { name: "Retire" });
    expect(retire.classList.contains("cc-btn--danger")).toBe(true);
    fireEvent.click(retire);
    // Nothing happens until the viewer confirms.
    expect(onLifecycle.mock.calls.length).toBe(0);
    const confirm = view.getByTestId("roster-retire-confirm:identity:coordinator");
    expect(confirm.getAttribute("role")).toBe("alertdialog");
    expect(confirm.getAttribute("aria-modal")).toBe("true");
    expect(within(confirm).getByText("Retire Coordinator?").id).toBe(confirm.getAttribute("aria-labelledby"));
    expect(confirm.textContent?.includes("It stops and leaves the team.")).toBe(true);
    fireEvent.click(view.getByTestId("roster-retire-confirm-yes:identity:coordinator"));
    expect(onLifecycle.mock.calls[0]).toEqual(["identity:coordinator", "meta/retire"]);
    expect(view.queryByTestId("roster-retire-confirm:identity:coordinator") === null).toBe(true);
  });

  it("keeps focus inside the Retire confirmation, cancels on Escape and returns focus to Retire", () => {
    const { view, onLifecycle } = renderRoster([coordinator]);
    fireEvent.click(hit(view.getByTestId("roster-row:identity:coordinator")));
    const detail = view.getByTestId("roster-detail:identity:coordinator");
    const retire = within(detail).getByRole("button", { name: "Retire" });
    act(() => retire.focus());
    fireEvent.click(retire);
    const confirm = view.getByTestId("roster-retire-confirm:identity:coordinator");
    const yes = view.getByTestId("roster-retire-confirm-yes:identity:coordinator");
    const cancel = view.getByTestId("roster-retire-confirm-cancel:identity:coordinator");
    // The safe choice takes focus; Tab and Shift+Tab stay inside.
    expect(document.activeElement === cancel).toBe(true);
    fireEvent.keyDown(confirm, { key: "Tab" });
    expect(document.activeElement === yes).toBe(true);
    fireEvent.keyDown(confirm, { key: "Tab", shiftKey: true });
    expect(document.activeElement === cancel).toBe(true);
    fireEvent.keyDown(cancel, { key: "Escape" });
    expect(view.queryByTestId("roster-retire-confirm:identity:coordinator") === null).toBe(true);
    expect(onLifecycle.mock.calls.length).toBe(0);
    expect(document.activeElement === retire).toBe(true);
    // The drawer stays open: Escape cancelled only the confirmation.
    expect(view.queryByTestId("roster-detail:identity:coordinator") !== null).toBe(true);
  });

  it("asks before a reset too, and respawns at once", () => {
    const { view, onLifecycle } = renderRoster([coordinator]);
    fireEvent.click(hit(view.getByTestId("roster-row:identity:coordinator")));
    const detail = view.getByTestId("roster-detail:identity:coordinator");
    fireEvent.click(within(detail).getByRole("button", { name: "Respawn" }));
    expect(onLifecycle.mock.calls[0]).toEqual(["identity:coordinator", "meta/respawn"]);
    fireEvent.click(within(detail).getByRole("button", { name: "Reset" }));
    expect(view.getByTestId("roster-reset-confirm:identity:coordinator").textContent?.includes("Reset Coordinator?")).toBe(true);
    fireEvent.click(view.getByTestId("roster-reset-confirm-yes:identity:coordinator"));
    expect(onLifecycle.mock.calls[1]).toEqual(["identity:coordinator", "meta/reset"]);
  });

  it("filters by the sidebar's groups and explains an empty result with a Clear action", () => {
    const { view } = renderRoster([coordinator, analyst, monitor]);
    const groups = view.getByRole("group", { name: "Agent groups" });
    expect(within(groups).getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(within(groups).getByRole("button", { name: "Coordinators" }));
    expect(view.queryByTestId("roster-row:identity:analyst") === null).toBe(true);
    expect(view.queryByTestId("roster-row:identity:coordinator") !== null).toBe(true);

    fireEvent.change(view.getByRole("searchbox", { name: "Search agents" }), { target: { value: "zzz" } });
    expect(view.getByText('No agents match "zzz"').className).toBe("cc-panel-empty__title");
    fireEvent.click(view.getByRole("button", { name: "Clear" }));
    expect(view.queryByTestId("roster-row:identity:analyst") !== null).toBe(true);
  });

  it("has a designed empty state", () => {
    const { view } = renderRoster([]);
    expect(view.getByText("No agents yet").className).toBe("cc-panel-empty__title");
  });

  it("renders only the rows in view for a very large team", () => {
    const many = Array.from({ length: 320 }, (_, index) => agent({ member_id: `identity:a${index}`, label: `Agent ${index}`, role: "specialist" }));
    const { view } = renderRoster(many);
    const list = view.getByTestId("roster-list");
    expect(list.getAttribute("data-windowed")).toBe("true");
    expect(list.querySelectorAll('[data-testid^="roster-row:"]').length < 120).toBe(true);
    expect(view.getByTestId("roster-glance:agents").textContent?.includes("320")).toBe(true);
  });
});
