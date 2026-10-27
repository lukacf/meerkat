import React from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { type ConsolePanelDefinition, type ConsoleFrame } from "@console-core";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "./headless";
import { CONSOLE_RPC_METHODS } from "./contract";
const identity = { id: "identity:demo", label: "Demo", role: "assistant" as const };

describe("stock custom panels", () => {
  beforeEach(() => window.localStorage.clear());
  it("opens an application panel after the last selected conversation leaves visibility", async () => {
    let live: (frame: ConsoleFrame) => void = () => {};
    let visible = true;
    const agent = { identity: identity.id, member_id: identity.id, agent_id: identity.id,
      label: "Demo", kind: "member", role: "worker", state: "idle", addressable: true, affordances: {} };
    const transport: MetaConsoleTransport = {
      loadExperience: async () => ({ contract_version: "test", runtime_id: "panel-selection-test", storage_scope: "panel-selection-subject",
        console_config: {}, console_policy: {}, agent_sidebar: { live_snapshot: { agents: visible ? [agent] : [] } },
        activity_feed: { filter_presets: [], active_preset_id: "all" } }) as never,
      loadModules: async () => ({ modules: [] }) as never,
      capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
      queryTimeline: async () => ({ frames: [], exhausted: true, available: true }),
      subscribeTimeline: (_input, onFrame) => { live = onFrame; return () => {}; },
      send: async () => ({}) as never, executeCommand: async () => ({ accepted: true, result: {} }) as never,
    };
    const mount = vi.fn((element: HTMLElement, context) => {
      element.textContent = context.conversation?.identity ?? "Application overview";
      return { dispose() {} };
    });
    const customPanels: ConsolePanelDefinition[] = [{ id: "demo/overview", title: "Overview", mount }];
    render(<ConsoleApp baseUrl="" transport={transport} customPanels={customPanels} />);
    await waitFor(() => expect(screen.getByTestId(`sidebar-agent:${identity.id}`)).toBeVisible());
    fireEvent.click(screen.getByTestId(`sidebar-agent:${identity.id}`));
    fireEvent.click(screen.getByTestId("nav-custom-panel:demo/overview"));
    await waitFor(() => expect(mount.mock.calls.at(-1)?.[1].conversation?.identity).toBe(identity.id));
    visible = false;
    await act(async () => live({ id: "member-left", event: "member_retired", identity: identity.id,
      timestampMs: Date.now(), cursor: "console:member-left", data: {} }));
    await waitFor(() => expect(screen.queryByTestId(`sidebar-agent:${identity.id}`)).toBeNull());
    fireEvent.click(screen.getByTestId("nav-custom-panel:demo/overview"));
    await waitFor(() => expect(screen.getByText("Application overview")).toBeVisible());
    expect(mount.mock.calls.at(-1)?.[1].conversation).toBeNull();
  });

  it("opens a developer panel from the sidebar, cleans up, and restores it", async () => {
    let live: (frame: ConsoleFrame) => void = () => {};
    const experience = { contract_version: "test", runtime_id: "extension-test", storage_scope: "extension-test-subject", console_config: { layout: { initial_agent: identity.id } }, console_policy: {},
      agent_sidebar: { live_snapshot: { agents: [{ identity: identity.id, member_id: identity.id, agent_id: identity.id,
        label: "Demo", kind: "member", role: "worker", state: "idle", addressable: true, affordances: {} }] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" } };
    const transport: MetaConsoleTransport = {
      loadExperience: async () => experience as never, loadModules: async () => ({ modules: [] }) as never,
      capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
      queryTimeline: async () => ({ frames: [], exhausted: true, available: true }),
      subscribeTimeline: (_input, onFrame) => { live = onFrame; return () => {}; },
      send: async () => ({}) as never, executeCommand: async () => ({ accepted: true, result: {} }) as never,
    };
    const dispose = vi.fn();
    const extensionService = vi.fn(async () => ({ ready: true }));
    const mount = vi.fn((element: HTMLElement, context, signal: AbortSignal) => {
      element.textContent = "Custom overview";
      void context.request({ path: "/application/status" }, signal);
      return { dispose };
    });
    const customPanels: ConsolePanelDefinition[] = [{ id: "demo/overview", title: "Overview", mount }];
    const view = render(<ConsoleApp baseUrl="" transport={transport} customPanels={customPanels} panelService={extensionService} />);
    await waitFor(() => expect(screen.getByTestId("nav-custom-panel:demo/overview")).toBeVisible());
    fireEvent.click(screen.getByTestId("nav-custom-panel:demo/overview"));
    await waitFor(() => expect(screen.getByText("Custom overview")).toBeVisible());
    expect(view.container.querySelectorAll(".pane")).toHaveLength(1);
    expect(mount).toHaveBeenCalledTimes(1);
    expect(extensionService).toHaveBeenCalledWith({ path: "/application/status" }, {
      authority: { key: JSON.stringify(["", "extension-test-subject"]), runtimeId: "extension-test" },
      conversation: { scopeKey: JSON.stringify(["", "extension-test-subject"]), identity: identity.id },
      readOnly: false,
    }, expect.any(AbortSignal));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 100)); });
    view.unmount();
    expect(dispose).toHaveBeenCalledTimes(1);
    const restored = render(<ConsoleApp baseUrl="" transport={transport} customPanels={customPanels} panelService={extensionService} />);
    await waitFor(() => expect(screen.getByText("Custom overview")).toBeVisible());
    expect(restored.container.querySelectorAll(".pane")).toHaveLength(1);
    restored.unmount();
    experience.storage_scope = "different-principal";
    render(<ConsoleApp baseUrl="" transport={transport} customPanels={customPanels} panelService={extensionService} />);
    await waitFor(() => expect(screen.getByTestId("nav-custom-panel:demo/overview")).toBeVisible());
    expect(screen.queryByText("Custom overview")).toBeNull();
  });
});
