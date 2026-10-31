/**
 * Host-provided attention items through the whole console: configured sources
 * reach the "Needs you" count and the inbox, and an aggregate console (no
 * gating surface) still opens a view that lists them.
 */
import React from "react";
import { act, fireEvent, render, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { MetaConsoleTransport } from "./headless";
import { SHARED_CONSOLE_RUNTIME_ID } from "./panel-runtime";

const SOURCE = "/host/attention";

function experience(runtimeId: string, options: { readOnly?: boolean; decideHref?: string } = {}) {
  return {
    contract_version: "test",
    runtime_id: runtimeId,
    console_config: { attention: { sources: [{ id: "host", label: "Host", url: SOURCE, ...(options.decideHref ? { decide_href: options.decideHref } : {}) }] } },
    console_policy: options.readOnly ? { read_only: true } : {},
    agent_sidebar: { live_snapshot: { agents: [] } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
  };
}

function transport(runtimeId: string, options: { readOnly?: boolean; decideHref?: string; pending?: unknown[] } = {}): MetaConsoleTransport {
  return {
    loadExperience: async () => experience(runtimeId, options) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => {},
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => ({ command: input.command, accepted: true, result: { pending: options.pending || [] } }) as never,
  } as MetaConsoleTransport;
}

async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 300));
  });
}

describe("host-provided attention in the console", () => {
  let realFetch: typeof fetch;
  beforeEach(() => {
    window.localStorage?.clear?.();
    realFetch = window.fetch;
    window.fetch = vi.fn(async (url: RequestInfo | URL) => String(url) === SOURCE
      ? new Response(JSON.stringify({ items: [{ id: "a", title: "Confirm a setting", href: "/host/items/a" }] }), { status: 200 })
      : new Response("not found", { status: 404 })) as typeof fetch;
  });
  afterEach(() => {
    window.fetch = realFetch;
    window.localStorage?.clear?.();
  });

  it("an aggregate console counts host items and opens a view that lists them", async () => {
    const view = render(<ConsoleApp baseUrl="" transport={transport(SHARED_CONSOLE_RUNTIME_ID)} />);
    await settle();
    const attention = view.getByTestId("approval-attention");
    expect(attention.querySelector(".cc-approval-attention__count")?.textContent).toBe("1");
    await act(async () => { fireEvent.click(within(attention).getByRole("button")); });
    await settle();
    const card = view.getByTestId("host-attention:host:a");
    expect(within(card).getByRole("link", { name: "Open: Confirm a setting" }).getAttribute("href")).toBe("/host/items/a");
    expect(view.getByText("Approvals are not available for this connection")).toBeTruthy();
    expect(view.getByText("Not available on this connection")).toBeTruthy();
    // The inbox, not the view a runtime-only panel shows on this connection.
    expect(view.queryByText("Not available on this runtime") === null).toBe(true);
    expect(vi.mocked(window.fetch).mock.calls.some(([url]) => String(url) === SOURCE)).toBe(true);
    view.unmount();
  }, 30_000);

  it("a runtime console shows the configured source's items in Needs you and in the inbox", async () => {
    const view = render(<ConsoleApp baseUrl="" transport={transport("host-attention-runtime")} />);
    await settle();
    const attention = view.getByTestId("approval-attention");
    expect(within(attention).getByRole("button").getAttribute("aria-label")).toContain("1 item from the host");
    await act(async () => { fireEvent.click(within(attention).getByRole("button")); });
    await settle();
    expect(view.getByText("No pending approvals.")).toBeTruthy();
    const host = view.getByTestId("host-attention-list");
    expect(within(host).getByRole("link", { name: "Open: Confirm a setting" }).getAttribute("href")).toBe("/host/items/a");
    view.unmount();
  }, 30_000);

  it.each([
    ["a runtime", "host-attention-runtime"],
    ["an aggregate", SHARED_CONSOLE_RUNTIME_ID],
  ])("Needs you brings %s console's inbox open on another tab back to Pending", async (_kind, runtimeId) => {
    const view = render(<ConsoleApp baseUrl="" transport={transport(runtimeId)} />);
    await settle();
    const needsYou = () => within(view.getByTestId("approval-attention")).getByRole("button");
    await act(async () => { fireEvent.click(needsYou()); });
    await settle();
    await act(async () => { fireEvent.click(view.getByTestId("gating-tab:audit")); });
    expect(view.queryByTestId("host-attention-list") === null).toBe(true);
    await act(async () => { fireEvent.click(needsYou()); });
    await settle();
    expect(view.getByTestId("gating-tab:pending").getAttribute("aria-selected")).toBe("true");
    expect(within(view.getByTestId("host-attention-list")).getByRole("link", { name: "Open: Confirm a setting" })).toBeTruthy();
    view.unmount();
  }, 30_000);

  it("an open aggregate inbox stays when the host items drop to zero", async () => {
    let answered = 0;
    window.fetch = vi.fn(async (url: RequestInfo | URL) => {
      if (String(url) !== SOURCE) return new Response("not found", { status: 404 });
      answered += 1;
      const items = answered === 1 ? [{ id: "a", title: "Confirm a setting", href: "/host/items/a" }] : [];
      return new Response(JSON.stringify({ items }), { status: 200 });
    }) as typeof fetch;
    const view = render(<ConsoleApp baseUrl="" transport={transport(SHARED_CONSOLE_RUNTIME_ID)} />);
    await settle();
    await act(async () => { fireEvent.click(within(view.getByTestId("approval-attention")).getByRole("button")); });
    await settle();
    expect(view.getByTestId("host-attention:host:a")).toBeTruthy();
    await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
    await settle();
    expect(answered).toBeGreaterThanOrEqual(2);
    expect(view.queryByTestId("host-attention:host:a") === null).toBe(true);
    expect(view.getByText("Approvals are not available for this connection")).toBeTruthy();
    expect(view.getByText("Not available on this connection")).toBeTruthy();
    expect(view.queryByText("Not available on this runtime") === null).toBe(true);
    view.unmount();
  }, 30_000);

  it("a read-only runtime console links its pending approval to the source's decide page", async () => {
    const view = render(<ConsoleApp baseUrl="" transport={transport("host-attention-runtime", {
      readOnly: true,
      decideHref: "/host/decide?decision={decision_id}",
      pending: [{ pending_id: "p 1", action_id: "a1", action: "Deploy release" }],
    })} />);
    await settle();
    await act(async () => { fireEvent.click(within(view.getByTestId("approval-attention")).getByRole("button")); });
    await settle();
    const link = view.getByRole("link", { name: "Decide in Host: Deploy release" });
    expect(link.getAttribute("href")).toBe("/host/decide?decision=p%201");
    view.unmount();
  }, 30_000);
});
