import React from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "./headless";
import { acceptanceMarkdownUrlPolicy } from "../../fixtures/markdown-url-policy";

const identity = "policy-agent";
const source = "[Review](/release/review) [App review](meta:review) [Denied](https://denied.invalid/link) ![Approved image](https://host.test/blobs/approved) ![Denied image](https://denied.invalid/image.png)";

function transport(): MetaConsoleTransport {
  return {
    loadExperience: async () => ({ runtime_id: "policy-test", storage_scope: "policy-principal", console_config: {}, agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Policy agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true } }] } }, activity_feed: { filter_presets: [], active_preset_id: "all" } }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ methods: [] }) as never,
    queryTimeline: async () => ({ available: true, frames: [{ id: "policy-frame", event: "interaction_complete", identity, interactionId: "policy-turn", timestampMs: 1, data: { text: source } }] }),
    subscribeTimeline: () => () => {}, send: vi.fn(),
    executeCommand: async () => ({ result: {} }) as never,
    upload: vi.fn(), blobUrl: id => `/blobs/${id}`,
  };
}
beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear(); window.sessionStorage.clear();
  const target = { id: "chat:policy-agent", kind: "agent-chat", title: "Policy agent", identity, memberId: identity };
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", "policy-principal"])}`, JSON.stringify({
    tabs: [{ id: "tab", presetId: "single", layout: { kind: "panel", panelId: "panel" } }],
    panels: [{ id: "panel", mode: "console", target }], activeTabId: "tab", focusedPanelId: "panel",
  }));
});
afterEach(async () => { await act(async () => { await new Promise(resolve => setTimeout(resolve, 20)); }); cleanup(); vi.unstubAllGlobals(); });

it("applies a host URL policy through the composed stock console and restores defaults when removed", async () => {
  const api = transport();
  const policy = acceptanceMarkdownUrlPolicy({ origin: "https://host.test", search: "?markdownPolicy=custom&approvedImage=approved" });
  const view = render(<ConsoleApp baseUrl="" transport={api} markdownUrlPolicy={policy} />);
  expect(await screen.findByRole("link", { name: "Review", exact: true })).toHaveAttribute("href", "https://host.test/console#release-review");
  expect(screen.getByRole("link", { name: "App review", exact: true })).toHaveAttribute("href", "https://host.test/console#release-review");
  expect(screen.getByRole("img", { name: "Approved image", exact: true })).toHaveAttribute("src", "https://host.test/blobs/approved");
  expect(screen.queryByRole("link", { name: "Denied", exact: true })).toBeNull();
  expect(screen.queryByRole("img", { name: "Denied image", exact: true })).toBeNull();
  view.rerender(<ConsoleApp baseUrl="" transport={api} />);
  expect(screen.queryByRole("link", { name: "Review", exact: true })).toBeNull();
  expect(screen.queryByRole("link", { name: "App review", exact: true })).toBeNull();
  expect(screen.queryByRole("img", { name: "Approved image", exact: true })).toBeNull();
  expect(screen.getByRole("link", { name: "Denied", exact: true })).toHaveAttribute("href", "https://denied.invalid/link");
});
