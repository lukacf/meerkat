/**
 * A conversation opens on its newest timeline page, and the notice snapshot
 * on that page carries every notice of the session, including notices older
 * than anything the page loaded. Rendered among the loaded rows, such a
 * notice sat directly above much newer rows and hid the unloaded history
 * between them; "Load older history" then filled in rows newer than it. It
 * must wait for the older pages that contain its time, then appear once, in
 * its time order.
 */
import React from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleTimelineQueryInput, MetaConsoleTransport } from "./headless";
import type { ConsoleFrame, ConsoleTimelinePage } from "../types";

const identity = "history-agent";
const SESSION = "01900000-0000-7000-8000-0000000000a1";
const RUN = "01900000-0000-7000-8000-0000000000a2";
const INPUT = "01900000-0000-7000-8000-0000000000a3";
const JOB = "nightly-export";
// Minutes after 15:00Z: the notice is from 15:52 and the newest page starts
// at 16:09.
const at = (minute: number, second = 0) => Date.parse("2026-10-14T15:00:00Z") + (minute * 60 + second) * 1000;

function frame(id: string, event: string, cursor: number, timestampMs: number, data: Record<string, unknown>,
  extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return {
    id, event, identity, runtimeKey: "runtime-a", sessionId: SESSION, cursor: `console:${cursor}`,
    timestampMs, sourceKind: "console_event", status: "completed", data, ...extra,
  };
}

function prompt(id: string, cursor: number, timestampMs: number, text: string, turn: string): ConsoleFrame {
  return frame(id, "user_input", cursor, timestampMs, { content: text }, { interactionId: turn });
}

const job = {
  role: "system_notice", kind: "background_job", body: "Background job completed.",
  blocks: [{ type: "background_job", job_id: JOB, display_name: "Nightly export", status: "completed",
    detail: "Exported 42 files." }],
  created_at: new Date(at(52)).toISOString(),
  runtime_origin: { session_id: SESSION, run_id: RUN, input_id: INPUT, append_ordinal: 0 },
};

// Keyed by the `before` cursor "Load older history" pages back from.
const history: Record<string, ConsoleTimelinePage> = {
  newest: {
    available: true, exhausted: false, latestCursor: "console:902", frames: [
      prompt("asked", 900, at(69), "Did the export finish?", "01900000-0000-7000-8000-0000000000b3"),
      frame("answered", "interaction_complete", 901, at(69, 30), { text: "Checking the export now." },
        { interactionId: "01900000-0000-7000-8000-0000000000b3" }),
      frame("notice-image", "runtime_notice_snapshot", 902, at(70), {
        session_id: SESSION, complete: true, observed_through: "console:901",
        notices: [{ offset: 1, message: job }], settled_attempts: [{ run_id: RUN, input_id: INPUT }],
      }, { sourceKind: "session_history" }),
    ],
  },
  "console:900": {
    available: true, exhausted: false, frames: [
      prompt("nudged", 500, at(65), "Any update on the export?", "01900000-0000-7000-8000-0000000000b2"),
    ],
  },
  "console:500": {
    available: true, exhausted: true, frames: [
      prompt("started", 100, at(50), "Start the nightly export.", "01900000-0000-7000-8000-0000000000b1"),
      // The notice's own live application: the same row, never a second card.
      frame("applied", "boundary_append_applied", 101, at(52, 1), {
        type: "boundary_append_applied", run_id: RUN, input_id: INPUT, append_count: 1, transcript_start: 1,
        notices: [job],
      }, { runId: RUN }),
    ],
  },
};

function transport(queries: ConsoleTimelineQueryInput[], pages = history): MetaConsoleTransport {
  return {
    loadExperience: async () => ({ runtime_id: "history-test", storage_scope: "history-principal", console_config: {},
      agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "History agent",
        kind: "member", state: "running", addressable: true, affordances: { can_send_message: true } }] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" } }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ methods: [] }) as never,
    queryTimeline: async (input) => {
      // The stream's unscoped seed is not this conversation's history.
      if (input.identity !== identity) return { available: true, frames: [] };
      queries.push(input);
      return pages[input.before ?? "newest"] ?? { available: true, exhausted: true, frames: [] };
    },
    subscribeTimeline: () => () => {}, send: vi.fn(),
    executeCommand: async (input) => ({ command: input.command, accepted: true, result: {} }) as never,
    upload: vi.fn(), blobUrl: (id) => `/blobs/${id}`,
  };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear(); window.sessionStorage.clear();
  const target = { id: `chat:${identity}`, kind: "agent-chat", title: "History agent", identity, memberId: identity };
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", "history-principal"])}`, JSON.stringify({
    tabs: [{ id: "tab", presetId: "single", layout: { kind: "panel", panelId: "panel" } }],
    panels: [{ id: "panel", mode: "console", target }], activeTabId: "tab", focusedPanelId: "panel",
  }));
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
  cleanup(); vi.unstubAllGlobals();
});

const transcript = async () => within(await screen.findByRole("feed", { name: "Conversation transcript" }));
const cards = () => document.querySelectorAll(`[role="feed"] [data-job-id="${JOB}"]`);
const precedes = (first: Element, second: Element) =>
  Boolean(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING);

async function loadOlder(queries: ConsoleTimelineQueryInput[], before: string): Promise<void> {
  const count = queries.length;
  await act(async () => { fireEvent.click(await screen.findByRole("button", { name: "Load older history" })); });
  await vi.waitFor(() => expect(queries.slice(count).flatMap((query) => query.before ? [query.before] : []))
    .toEqual([before]));
}

it("holds a snapshot notice older than the newest page until older history reaches its time", async () => {
  const queries: ConsoleTimelineQueryInput[] = [];
  render(<ConsoleApp baseUrl="" transport={transport(queries)} />);
  const asked = await (await transcript()).findByText("Did the export finish?");
  await screen.findByRole("button", { name: "Load older history" });
  expect(cards(), "a 15:52 notice must not render directly above the 16:09 page").toHaveLength(0);

  // 16:05 is still newer than the notice: the history between stays unloaded.
  await loadOlder(queries, "console:900");
  const nudged = await (await transcript()).findByText("Any update on the export?");
  expect(precedes(nudged, asked)).toBe(true);
  expect(cards()).toHaveLength(0);

  // Reaching 15:50 places the notice between its neighbours, exactly once.
  await loadOlder(queries, "console:500");
  const started = await (await transcript()).findByText("Start the nightly export.");
  await vi.waitFor(() => expect(cards()).toHaveLength(1));
  const [card] = cards();
  expect(card.textContent).toContain("Exported 42 files.");
  expect(precedes(started, card)).toBe(true);
  expect(precedes(card, nudged)).toBe(true);
  expect(screen.queryByRole("button", { name: "Load older history" })).toBeNull();
});

it("releases a held notice when the older history turns out to be empty", async () => {
  const queries: ConsoleTimelineQueryInput[] = [];
  render(<ConsoleApp baseUrl="" transport={transport(queries, { newest: history.newest })} />);
  const asked = await (await transcript()).findByText("Did the export finish?");
  await screen.findByRole("button", { name: "Load older history" });
  expect(cards()).toHaveLength(0);

  // An empty last page adds no frame; nothing older exists after all, so the
  // notice is the oldest row of a complete transcript.
  await loadOlder(queries, "console:900");
  await vi.waitFor(() => expect(cards()).toHaveLength(1));
  expect(precedes(cards()[0], asked)).toBe(true);
  expect(screen.queryByRole("button", { name: "Load older history" })).toBeNull();
});
