/**
 * Side chats in the stock console: "Fork to side chat" forks the agent of a
 * conversation (`meta/fork_member`) into a new member, opens it beside the
 * source, and "Close side chat" retires it and closes its pane. The source's
 * conversation stays where it was. The fork is offered only to a viewer who
 * may spawn members, and waits while the agent works.
 */
import React from "react";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { MetaConsoleTransport } from "./headless";

const SOURCE = "alpha";
const SCOPE = "side-chat-principal";

type Row = Record<string, unknown>;

function agentRow(identity: string, extra: Row = {}): Row {
  return {
    identity,
    member_id: identity,
    agent_id: identity,
    label: identity === SOURCE ? "Alpha" : `${identity} label`,
    kind: "mob_agent",
    role: "worker",
    state: "active",
    addressable: true,
    affordances: { can_send_message: true, can_retire: true },
    model_capabilities: { image_input: false },
    ...extra,
  };
}

interface Harness extends MetaConsoleTransport {
  canSpawn: boolean;
  sourcePhase: string | null;
  sideChats: string[];
  commands: Array<{ command: string; params: Record<string, unknown> }>;
}

function transport(): Harness {
  const harness: Harness = {
    canSpawn: true,
    sourcePhase: null,
    sideChats: [],
    commands: [],
    loadExperience: async () => ({
      contract_version: "test",
      runtime_id: "side-chat-runtime",
      storage_scope: SCOPE,
      console_config: {},
      console_policy: {},
      runtime_capabilities: {
        can_spawn_members: harness.canSpawn,
        can_send_messages: true,
        can_retire_members: true,
      },
      agent_sidebar: {
        live_snapshot: {
          agents: [
            agentRow(SOURCE, harness.sourcePhase ? { response_phase: harness.sourcePhase } : {}),
            ...harness.sideChats.map((alias) =>
              agentRow(alias, { label: "Alpha (side chat)", side_chat_of: SOURCE }),
            ),
          ],
        },
      },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => undefined,
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => {
      const params = (input.params || {}) as Record<string, unknown>;
      harness.commands.push({ command: input.command, params });
      if (input.command === "forkMember") {
        const alias = String(params.agent_identity);
        harness.sideChats.push(alias);
        return {
          command: input.command,
          accepted: true,
          result: {
            agent_identity: alias,
            session_id: "side-session",
            display_name: "Alpha (side chat)",
            side_chat_of: SOURCE,
            source_member_id: SOURCE,
            retention: "until_closed",
            instructions: [
              { namespace: "app.skills", key: "billing", state: "inactive", reason: { kind: "not_effective_in_source" } },
            ],
          },
        } as never;
      }
      if (input.command === "retireIdentity") {
        harness.sideChats = harness.sideChats.filter((alias) => alias !== params.identity);
        return { command: input.command, accepted: true, result: { identity: params.identity } } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
  return harness;
}

function seedDock(): void {
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", SCOPE])}`, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{
      id: "panel-1",
      mode: "console",
      target: { id: `chat:${SOURCE}`, kind: "agent-chat", title: "Alpha", identity: SOURCE, memberId: SOURCE },
    }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
}

async function wait(ms: number): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

describe("side chats", () => {
  beforeEach(() => {
    window.localStorage.clear();
    seedDock();
  });
  afterEach(() => window.localStorage.clear());

  it("forks the conversation into a side pane and closes it, leaving the source as it was", async () => {
    const harness = transport();
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await view.findByTestId(`chat-pane:${SOURCE}`);
    const fork = await view.findByTestId("conv-action:fork-side-chat");
    await waitFor(() => expect((fork as HTMLButtonElement).disabled).toBe(false));

    fireEvent.click(fork);
    await waitFor(() => expect(harness.sideChats).toHaveLength(1));
    const alias = harness.sideChats[0];
    expect(alias).toMatch(/^alpha-side-[a-z0-9]+$/);
    expect(harness.commands.find((entry) => entry.command === "forkMember")?.params).toEqual({
      source_member_id: SOURCE,
      agent_identity: alias,
    });

    await view.findByTestId(`chat-pane:${alias}`, undefined, { timeout: 5_000 });
    expect(view.getByTestId(`chat-pane:${SOURCE}`)).toBeTruthy();
    expect(view.getByTestId(`side-chat-banner:${alias}`).textContent).toContain("Side chat of Alpha");
    expect(view.getByTestId(`side-chat-notice:${alias}`).textContent).toContain("app.skills/billing");
    expect(view.getByTestId(`sidebar-side-chat:${alias}`)).toBeTruthy();

    fireEvent.click(view.getByTestId(`side-chat-close:${alias}`));
    await waitFor(() => expect(view.queryByTestId(`chat-pane:${alias}`)).toBeNull());
    expect(harness.commands.find((entry) => entry.command === "retireIdentity")?.params).toEqual({ identity: alias });
    expect(view.getByTestId(`chat-pane:${SOURCE}`)).toBeTruthy();
    expect(harness.commands.filter((entry) => entry.params.identity === SOURCE)).toHaveLength(0);
    view.unmount();
  }, 30_000);

  it("waits while the agent works", async () => {
    const harness = transport();
    harness.sourcePhase = "generating";
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    const fork = await view.findByTestId("conv-action:fork-side-chat");
    expect((fork as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(fork);
    await wait(200);
    expect(harness.commands.filter((entry) => entry.command === "forkMember")).toHaveLength(0);
    view.unmount();
  }, 30_000);

  it("offers no fork to a viewer who may not spawn members", async () => {
    const harness = transport();
    harness.canSpawn = false;
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await view.findByTestId(`chat-pane:${SOURCE}`);
    await wait(300);
    expect(view.queryByTestId("conv-action:fork-side-chat")).toBeNull();
    view.unmount();
  }, 30_000);
});
