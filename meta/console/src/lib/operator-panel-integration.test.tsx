/**
 * Operator panels through the whole console: the sidebar button opens a
 * docked panel, and a saved dock layout restores it from the current static
 * configuration only. The panel path is never read from saved layout data.
 */
import React from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { buildOperatorPanelTarget } from "./adapters";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { MetaConsoleTransport } from "./headless";

const RUNTIME = "operator-panel-runtime";

function experience(panel: string) {
  return {
    contract_version: "test",
    runtime_id: RUNTIME,
    console_config: { sidebar: { buttons: [{ id: "reviews", label: "Reviews", panel }] } },
    console_policy: {},
    agent_sidebar: { live_snapshot: { agents: [] } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
  };
}

function transport(panel: string): MetaConsoleTransport {
  return {
    loadExperience: async () => experience(panel) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => {},
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => ({ command: input.command, accepted: true, result: {} }) as never,
  } as MetaConsoleTransport;
}

function seedSavedPanel(): void {
  // A saved layout that also carries a tampered path; only the id may count.
  // Saved layouts persist only under a host storage scope, so the restore
  // cases render with `storageNamespace={RUNTIME}`.
  const target = { ...buildOperatorPanelTarget({ id: "reviews", label: "Reviews" }), panel: "/tampered" };
  window.localStorage.setItem(`meta-console-dock-state:${RUNTIME}`, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", mode: "console", target }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
}

async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 300));
  });
}

describe("operator panels in the console", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => window.localStorage.clear());

  it("opens the configured panel from the sidebar", async () => {
    const view = render(<ConsoleApp baseUrl="" transport={transport("/host/reviews")} />);
    await settle();
    await act(async () => { fireEvent.click(view.getByTestId("nav-panel:reviews")); });
    await settle();
    expect(view.container.querySelector("iframe")?.getAttribute("src")).toBe("/host/reviews");
    view.unmount();
  }, 30_000);

  it("restores a saved panel from the current configuration only", async () => {
    seedSavedPanel();
    const view = render(<ConsoleApp baseUrl="" storageNamespace={RUNTIME} transport={transport("/host/reviews-v2")} />);
    await settle();
    expect(view.container.querySelector("iframe")?.getAttribute("src")).toBe("/host/reviews-v2");
    view.unmount();
  }, 30_000);

  it("shows a restored panel as unconfigured when its current path is invalid", async () => {
    seedSavedPanel();
    const view = render(<ConsoleApp baseUrl="" storageNamespace={RUNTIME} transport={transport("/./host")} />);
    await settle();
    expect(view.container.querySelector("iframe")).toBeNull();
    expect(view.getByTestId("operator-panel-missing:reviews")).toBeTruthy();
    view.unmount();
  }, 30_000);
});
