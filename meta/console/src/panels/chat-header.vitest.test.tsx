import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ConversationTimelineEntry } from "@console-core";
import type { ConsoleAgent, ConsoleFrame } from "../types";
import { ActivityLedgerProvider, createActivityLedger } from "../lib/activity-ledger";
import {
  ChatHeader,
  chatAgentStatus,
  chatLiveActivity,
  chatRoleGroupWords,
  runningToolOf,
  type ChatHeaderProps,
} from "./chat-header";

const NOW = Date.UTC(2026, 10, 1, 9, 30, 0);

function agent(overrides: Partial<ConsoleAgent> = {}): ConsoleAgent {
  return {
    identity: "orders",
    agent_id: "orders",
    member_id: "orders",
    label: "Orders",
    kind: "member",
    role: "specialist",
    group: "Domain",
    state: "active",
    ...overrides,
  };
}

function toolEntry(id: string, blocks: Array<Record<string, unknown>>): ConversationTimelineEntry {
  return {
    id,
    kind: "message",
    variant: "rich",
    identity: { id: "orders", label: "Orders", role: "assistant" },
    blocks: blocks as never,
  } as ConversationTimelineEntry;
}

const labels = {
  inspectLabel: "Details",
  forkSideChatLabel: "Fork to side chat",
  stopRunLabel: "Stop run",
  respawnLabel: "Respawn",
  retireLabel: "Retire",
};

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("chat header status", () => {
  it("reads a busy agent as Working, never Unknown, while its health cannot be judged", () => {
    const busy = agent({ response_phase: "tool-executing", progress: { health: "unknown", run_state: "run_open" } as never });
    expect(chatAgentStatus(busy, "tool-executing", false)).toEqual({ tone: "live", word: "Working", working: true });
    // A run the owner reports open is work too, without a phase.
    expect(chatAgentStatus(agent({ progress: { health: "healthy", run_state: "run_open" } as never }), null, false)?.word).toBe("Working");
  });

  it("names a problem or a retirement before the work, and a quiet agent as Idle", () => {
    expect(chatAgentStatus(agent({ progress: { health: "degraded" } as never }), "generating", false))
      .toEqual({ tone: "warn", word: "Degraded", working: true });
    expect(chatAgentStatus(agent({ state: "broken" }), null, false)).toMatchObject({ tone: "crit", word: "Broken", working: false });
    expect(chatAgentStatus(agent({ state: "retired" }), "generating", false)).toMatchObject({ word: "Retired", working: false });
    expect(chatAgentStatus(agent(), null, false)).toEqual({ tone: "neutral", word: "Idle", working: false });
    expect(chatAgentStatus(agent({ unobserved: true } as never), null, false)?.word).toBe("Not observed");
    // The console's own record of an open run counts when the owner says nothing.
    expect(chatAgentStatus(agent({ response_phase: undefined }), null, true)?.word).toBe("Working");
    expect(chatAgentStatus(null, null, false)).toBeNull();
    expect(chatAgentStatus(null, "waiting", false)?.word).toBe("Working");
  });

  it("says what the agent is doing in words, naming the tool and a message's recipient", () => {
    expect(chatLiveActivity({ phase: "generating", working: true })).toEqual({ text: "Writing a reply" });
    expect(chatLiveActivity({ phase: "waiting", working: true })).toEqual({ text: "Starting a reply" });
    expect(chatLiveActivity({ phase: "tool-executing", working: true, tool: { name: "web_search", callId: "c1" }, toolStartMs: NOW }))
      .toEqual({ text: "Running web search", detail: "web_search", sinceMs: NOW });
    expect(chatLiveActivity({ phase: "tool-executing", working: true, tool: { name: "send_request", callId: "c2", peer: "Billing" } })?.text)
      .toBe("Asking Billing");
    expect(chatLiveActivity({ phase: "tool-executing", working: true, tool: { name: "send_message", callId: "c3" } })?.text)
      .toBe("Messaging another agent");
    expect(chatLiveActivity({ phase: "tool-executing", working: true, tool: null })?.text).toBe("Running a tool");
    expect(chatLiveActivity({ phase: null, working: true, runStartMs: NOW })).toEqual({ text: "Working", sinceMs: NOW });
    expect(chatLiveActivity({ phase: null, working: false })).toBeNull();
  });

  it("finds the tool call still pending, newest first, and skips incoming peer messages", () => {
    const entries = [
      toolEntry("a", [{ type: "tool-call", toolCallId: "done", name: "lookup_order", arguments: "{}", status: "success" }]),
      toolEntry("b", [{ type: "tool-call", toolCallId: "older", name: "lookup_order", arguments: "{}", status: "pending" }]),
      toolEntry("c", [
        { type: "tool-call", toolCallId: "export", name: "pull_export", arguments: "{}", status: "pending" },
        { type: "tool-call", toolCallId: "inbox", name: "send_message", arguments: "{}", status: "pending", peerIncoming: true },
      ]),
    ];
    expect(runningToolOf(entries)).toEqual({ name: "pull_export", callId: "export" });
    expect(runningToolOf([toolEntry("d", [{ type: "tool-call", toolCallId: "q", name: "send_request", arguments: "{}", status: "pending", peerDisplayLabel: "Billing" }])]))
      .toEqual({ name: "send_request", callId: "q", peer: "Billing" });
    expect(runningToolOf(entries.slice(0, 1))).toBeNull();
  });

  it("joins the role and the group, leaving out words that repeat the name or the role", () => {
    expect(chatRoleGroupWords(agent(), "Orders")).toBe("Specialist · Domain");
    expect(chatRoleGroupWords(agent({ role: "coordinator", group: "Coordinators" }), "Intake")).toBe("Coordinator");
    expect(chatRoleGroupWords(agent({ role: "orders", group: undefined, labels: { group: "Domain" } }), "Orders")).toBe("Domain");
    expect(chatRoleGroupWords(null, "Orders")).toBe("");
  });
});

describe("chat header", () => {
  function renderHeader(props: Partial<ChatHeaderProps>, frames: ConsoleFrame[] = []) {
    const ledger = createActivityLedger({ now: () => Date.now(), notifyMs: 0 });
    ledger.ingestMany(frames);
    return render(
      <ActivityLedgerProvider ledger={ledger}>
        <ChatHeader identity="orders" agentLabel="Orders" agent={agent()} phase={null} {...labels} {...props} />
      </ActivityLedgerProvider>,
    );
  }

  it("shows a working agent with its live ring, the tool it runs and for how long", () => {
    vi.useFakeTimers({ now: NOW });
    const view = renderHeader(
      { agent: agent({ response_phase: "tool-executing" }), phase: "tool-executing", toolName: "pull_export", toolCallId: "export-1" },
      [{ id: "f1", event: "tool_execution_started", identity: "orders", timestampMs: NOW - 45_000, data: { id: "export-1", name: "pull_export" } }],
    );
    expect(screen.getByTestId("conv-status:orders")).toHaveTextContent("Working");
    expect(screen.getByTestId("conv-status:orders").getAttribute("data-tone")).toBe("live");
    const live = screen.getByTestId("conv-live:orders");
    expect(live).toHaveAttribute("data-live", "true");
    expect(live).toHaveTextContent("Running pull export");
    expect(live).toHaveTextContent("45 s");
    expect(screen.getByTitle("pull_export")).toHaveTextContent("Running pull export");
    expect(view.container.querySelector(".cc-agent__avatar.is-live")).not.toBeNull();
    expect(view.container.textContent).not.toMatch(/unknown/i);
    // The elapsed time ticks every second in its first minute.
    act(() => { vi.advanceTimersByTime(5_000); });
    expect(screen.getByTestId("conv-live:orders")).toHaveTextContent("50 s");
  });

  it("shows a quiet agent's last activity and no live ring", () => {
    const view = renderHeader({ agent: agent({ last_activity_ms: Date.now() - 4 * 60_000 }) });
    expect(screen.getByTestId("conv-status:orders")).toHaveTextContent("Idle");
    expect(screen.getByTestId("conv-live:orders")).toHaveTextContent("Last active 4m ago");
    expect(screen.getByTestId("conv-live:orders")).not.toHaveAttribute("data-live");
    expect(view.container.querySelector(".cc-agent__avatar.is-live")).toBeNull();
    expect(screen.getByText("Specialist · Domain")).toBeTruthy();
  });

  it("keeps Details, Fork and Stop run in view and puts Respawn and Retire behind the menu, asking first", async () => {
    const onRespawn = vi.fn();
    const onRetire = vi.fn();
    renderHeader({ onInspect: vi.fn(), onForkSideChat: vi.fn(), forkSideChatDisabledReason: "Available when the agent is idle", onStopRun: vi.fn(), onRespawn, onRetire });
    expect(screen.getByTestId("conv-action:details")).toHaveAttribute("title", "Details - orders");
    expect(screen.getByTestId("conv-action:fork-side-chat")).toBeDisabled();
    expect(screen.getByTestId("conv-action:stop-run")).toHaveAttribute("title", "Stop run - orders");
    expect(screen.queryByTestId("conv-action:respawn")).toBeNull();
    fireEvent.click(screen.getByTestId("conv-menu"));
    fireEvent.click(await screen.findByTestId("conv-action:retire"));
    expect(screen.getByTestId("conv-confirm:retire")).toHaveTextContent("Retire Orders? It stops and leaves the team.");
    expect(onRetire).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("conv-confirm-accept:retire"));
    expect(onRetire).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByTestId("conv-menu"));
    fireEvent.click(await screen.findByTestId("conv-action:respawn"));
    expect(screen.getByTestId("conv-confirm:respawn")).toHaveTextContent("Respawn Orders? It restarts and keeps its conversation.");
    fireEvent.click(screen.getByTestId("conv-confirm-cancel:respawn"));
    expect(onRespawn).not.toHaveBeenCalled();
  });

  it("closes a confirmation whose action was withdrawn", async () => {
    const onRetire = vi.fn();
    const ledger = createActivityLedger({ notifyMs: 0 });
    const view = render(
      <ActivityLedgerProvider ledger={ledger}>
        <ChatHeader identity="orders" agentLabel="Orders" agent={agent()} phase={null} {...labels} onRetire={onRetire} />
      </ActivityLedgerProvider>,
    );
    fireEvent.click(screen.getByTestId("conv-menu"));
    fireEvent.click(await screen.findByTestId("conv-action:retire"));
    expect(screen.getByTestId("conv-confirm:retire")).toBeTruthy();
    view.rerender(
      <ActivityLedgerProvider ledger={ledger}>
        <ChatHeader identity="orders" agentLabel="Orders" agent={agent()} phase={null} {...labels} />
      </ActivityLedgerProvider>,
    );
    expect(screen.queryByTestId("conv-confirm:retire")).toBeNull();
    expect(screen.queryByTestId("conv-menu")).toBeNull();
    expect(onRetire).not.toHaveBeenCalled();
  });
});
