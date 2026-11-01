import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "./headless";

// "New workspace" (the tab strip's "+", Cmd+T) opens a blank tab with the
// target picker, as a browser's new tab does, whatever the focused pane shows.

const namespace = "runtime/realm/new-workspace";
const identity = "identity:new-workspace-agent";

function transport(): MetaConsoleTransport {
  return {
    loadExperience: async () => ({
      runtime_id: "new-workspace-test",
      agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "New workspace agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/timeline"] }) as never,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {},
    send: vi.fn(),
    executeCommand: async () => ({ result: {} }) as never,
    upload: async () => ({ blob_id: "blob" }) as never,
    blobUrl: (id) => `/blobs/${id}`,
  };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});

afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("New workspace", () => {
  it("opens a picker from Roster, not a second Roster", async () => {
    window.localStorage.setItem(`meta-console-dock-state:${namespace}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "roster", kind: "roster", title: "Roster" } }],
      activeTabId: "tab-1", focusedPanelId: "panel-1",
    }));
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={transport()} />);
    await screen.findByTestId("roster-panel");
    fireEvent.click(screen.getByTestId("wstab-add"));
    await waitFor(() => expect(screen.getByText("Nothing to show here")).toBeTruthy());
    expect(screen.queryByTestId("roster-panel")).toBeNull();
    expect(document.querySelectorAll('[data-testid^="wstab:"]')).toHaveLength(2);
  });
});
