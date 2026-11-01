import React from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "./headless";

// The first pane of a first load: with no saved layout and no first view the
// config names, it opens Today instead of an empty pane. Saved layouts and
// configured first views keep their meaning.

const namespace = "runtime/realm/first-pane";
const identity = "identity:first-pane-agent";

function transport(layout?: Record<string, unknown>): MetaConsoleTransport {
  return {
    loadExperience: async () => ({
      runtime_id: "first-pane-test",
      ...(layout ? { console_config: { layout } } : {}),
      agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "First pane agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
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

describe("the first pane of a first load", () => {
  it("opens Today when no layout is saved and the config names no first view", async () => {
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={transport()} />);
    await screen.findByTestId("timeline-panel");
    expect(screen.queryByText("Nothing to show here")).toBeNull();
  });

  it("keeps a first view the config names", async () => {
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={transport({ initial_control: "logs" })} />);
    await screen.findByTestId("logs-panel");
    expect(screen.queryByTestId("timeline-panel")).toBeNull();
  });

  it("restores a saved layout as it was", async () => {
    window.localStorage.setItem(`meta-console-dock-state:${namespace}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "roster", kind: "roster", title: "Roster" } }],
      activeTabId: "tab-1", focusedPanelId: "panel-1",
    }));
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={transport()} />);
    await screen.findByTestId("roster-panel");
    expect(screen.queryByTestId("timeline-panel")).toBeNull();
  });
});
