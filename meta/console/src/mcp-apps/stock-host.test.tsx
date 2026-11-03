import React from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { ConsoleFrame } from "@console-core";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "../lib/headless";
import { CONSOLE_RPC_METHODS } from "../lib/contract";

it("routes a live native app locator through the mounted stock Console before history or turn completion", async () => {
  window.localStorage.clear();
  const identity = "member:app";
  let live: ((frame: ConsoleFrame) => void) | undefined;
  const queryTimeline = vi.fn(async () => ({ frames: [], exhausted: true, available: true }));
  const transport: MetaConsoleTransport = {
    loadExperience: async () => ({
      contract_version: "test", runtime_id: "app-routing-test", storage_scope: "app-routing-subject",
      console_config: {}, console_policy: {},
      agent_sidebar: { live_snapshot: { agents: [{
        identity, member_id: identity, agent_id: identity, label: "App member", kind: "member",
        role: "worker", state: "idle", addressable: true, affordances: {},
      }] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
    queryTimeline,
    subscribeTimeline: (_input, onFrame) => { live = onFrame; return () => {}; },
    send: async () => ({}) as never,
    executeCommand: async () => ({ accepted: true, result: {} }) as never,
  };
  const resolve = vi.fn(async () => null);
  const view = render(<ConsoleApp baseUrl="" transport={transport}
    mcpAppsHost={{ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve }} />);
  await waitFor(() => expect(screen.getByTestId(`sidebar-agent:${identity}`)).toBeVisible());
  fireEvent.click(screen.getByTestId(`sidebar-agent:${identity}`));
  await waitFor(() => expect(live).toBeTypeOf("function"));
  expect(resolve).not.toHaveBeenCalled();
  await act(async () => live!({
    id: "native-live-app", event: "mcp_app", sourceKind: "tool_application",
    identity, sessionId: "session:app", runtimeKey: "app-routing-test",
    timestampMs: Date.now(), cursor: "console:1",
    data: { session_id: "session:app", tool_call_id: "call:app" },
  }));
  await waitFor(() => expect(resolve).toHaveBeenCalledWith(
    { identity, sessionId: "session:app", toolCallId: "call:app" }, expect.any(AbortSignal),
  ));
  expect(resolve).toHaveBeenCalledTimes(1);
  expect(queryTimeline.mock.results.every(result => result.type === "return")).toBe(true);
  view.unmount();
});
