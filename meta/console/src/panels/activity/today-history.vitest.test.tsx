/**
 * Today loads the most active agents' history, and that history reaches
 * the activity ledger: the day's turns are spans the ledger pairs, so a
 * turn that only the history holds still draws. The team's global
 * timeline leaves out a retired member's frames (its retire event lives
 * only in its own log), so the retired member's lane and its retire row
 * come from that backfill.
 */
import React from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { ConsoleApp } from "../../ConsoleApp";
import type { ConsoleTimelineQueryInput, MetaConsoleTransport } from "../../lib/headless";
import type { ConsoleFrame, ConsoleTimelinePage } from "../../types";

const SCOPE = "today-history-principal";
const RUN = "01900000-0000-7000-8000-00000000c0a1";
const TURN = "01900000-0000-7000-8000-00000000c0b1";
const RETIRED_RUN = "01900000-0000-7000-8000-00000000c0a2";
const RETIRED_TURN = "01900000-0000-7000-8000-00000000c0b2";

let serial = 0;
function frame(identity: string, event: string, minutesAgo: number, data: Record<string, unknown>, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  serial += 1;
  return {
    id: `history-${serial}`,
    event,
    identity,
    cursor: `console:${serial}`,
    timestampMs: Date.now() - minutesAgo * 60_000,
    sourceKind: "console_event",
    data,
    ...extra,
  };
}

function pages(): Record<string, ConsoleTimelinePage> {
  return {
    billing: {
      available: true,
      exhausted: true,
      frames: [
        frame("billing", "user_input", 12, { content: "Refund order 77310.", origin: "console:pane-1", origin_kind: "operator" }, { interactionId: TURN }),
        frame("billing", "run_started", 12, { run_id: RUN }, { interactionId: TURN, runId: RUN }),
        frame("billing", "interaction_complete", 11, { result: "Refunded 1,240 EUR." }, { interactionId: TURN, runId: RUN }),
      ],
    },
    "weekend-cover": {
      available: true,
      exhausted: true,
      frames: [
        frame("weekend-cover", "run_started", 9, { run_id: RETIRED_RUN }, { interactionId: RETIRED_TURN, runId: RETIRED_RUN }),
        frame("weekend-cover", "interaction_failed", 9, { error_report: { class: "cancelled", message: "Run was cancelled" } }, { interactionId: RETIRED_TURN, runId: RETIRED_RUN }),
        frame("weekend-cover", "identity_retired", 8, {}),
      ],
    },
  };
}

function transport(queries: ConsoleTimelineQueryInput[]): MetaConsoleTransport {
  const history = pages();
  return {
    loadExperience: async () => ({
      runtime_id: "today-history-test",
      storage_scope: SCOPE,
      console_config: {},
      agent_sidebar: {
        live_snapshot: {
          agents: [
            { identity: "billing", member_id: "billing", agent_id: "billing", label: "Billing", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true } },
            { identity: "weekend-cover", member_id: "weekend-cover", agent_id: "weekend-cover", label: "Weekend cover", kind: "member", state: "retired", addressable: false },
          ],
        },
      },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ methods: [] }) as never,
    queryTimeline: async (input) => {
      queries.push(input);
      // The team's timeline (the stream's seed) leaves out every one of
      // these frames: only each agent's own history holds them.
      if (!input.identity) return { available: true, exhausted: true, frames: [] };
      return history[input.identity] ?? { available: true, exhausted: true, frames: [] };
    },
    subscribeTimeline: () => () => {},
    send: vi.fn(),
    executeCommand: async (input) => ({ command: input.command, accepted: true, result: {} }) as never,
    upload: vi.fn(),
    blobUrl: (id) => `/blobs/${id}`,
  };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  const target = { id: "timeline", kind: "timeline", title: "Today" };
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", SCOPE])}`, JSON.stringify({
    tabs: [{ id: "tab", presetId: "single", layout: { kind: "panel", panelId: "panel" } }],
    panels: [{ id: "panel", mode: "console", target }],
    activeTabId: "tab",
    focusedPanelId: "panel",
  }));
});

afterEach(async () => {
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
  cleanup();
  vi.unstubAllGlobals();
});

it("loads each agent's day into the ledger, the retired member's included", async () => {
  const queries: ConsoleTimelineQueryInput[] = [];
  render(<ConsoleApp baseUrl="" transport={transport(queries)} />);
  await screen.findByTestId("timeline-panel");

  // Today asks for each agent's recent history, up to a day's worth.
  await vi.waitFor(() => {
    const loaded = queries.filter((query) => query.identity).map((query) => [query.identity, query.mode, query.limit]);
    expect(loaded).toEqual(expect.arrayContaining([["billing", "recent", 1000], ["weekend-cover", "recent", 1000]]));
  });

  // The ledger pairs the history's run ends: the turns are spans in the
  // day lanes, though no live frame ever showed them.
  await vi.waitFor(() => {
    expect(document.querySelector(`[data-testid="timeline-span:${RUN}"]`)).not.toBeNull();
    expect(document.querySelector(`[data-testid="timeline-span:${RETIRED_RUN}"]`)).not.toBeNull();
  }, { timeout: 3_000 });
  expect(document.querySelector('[data-testid="timeline-lane:weekend-cover"]')).not.toBeNull();
  expect(document.querySelector(`[data-testid="timeline-span:${RUN}"]`)?.getAttribute("data-state")).toBe("done");
  expect(document.querySelector(`[data-testid="timeline-span:${RETIRED_RUN}"]`)?.getAttribute("data-state")).toBe("cancelled");

  // The retire row reads as a sentence, and the turn as one row.
  const story = screen.getByRole("list", { name: "The day's events, newest first" });
  expect(story.textContent).toContain("Weekend cover was retired");
  expect(story.textContent).toContain("An operator asked Billing");
  expect(story.textContent).not.toMatch(/identity_retired|interaction_complete/);
});
