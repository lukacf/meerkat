import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleWhoami, MetaConsoleTransport } from "./headless";

// Personal approval cards: the console says whether a request waits for the
// signed-in person, and a request decided elsewhere says who decided it.

const identity = "identity:approvals-agent";
const pendingRequest = (id: string, approver?: string) => ({
  pending_id: id, action_id: `action-${id}`, action: `Request ${id}`, origin: { identity }, risk_tier: "r2",
  ...(approver ? { requested_approver: approver } : {}),
});

function gateway(viewer: ConsoleWhoami) {
  let pending = [pendingRequest("p-owners", "team:owners"), pendingRequest("p-finance", "team:finance"), pendingRequest("p-open")];
  let audit: unknown[] = [];
  const fake: MetaConsoleTransport = {
    loadExperience: async () => ({
      runtime_id: "approvals-test", storage_scope: "console-storage:v1:approvals",
      console_config: { layout: { initial_agent: identity } },
      agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Approvals agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/gating/pending", "meta/gating/decide", "meta/gating/audit"] }) as never,
    whoami: async () => viewer,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {},
    send: vi.fn(),
    executeCommand: async (input) => {
      if (input.command === "listGatingPending") return { command: input.command, accepted: true, result: { pending } } as never;
      if (input.command === "listGatingAudit") return { command: input.command, accepted: true, result: { entries: audit } } as never;
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
  return {
    fake,
    decideElsewhere(id: string, decider: string) {
      pending = pending.filter((entry) => entry.pending_id !== id);
      audit = [{ audit_id: `audit-${id}`, pending_id: id, action_id: `action-${id}`, event_type: "approval_decided", outcome: "allowed", risk_tier: "r2", timestamp_ms: Date.now(), detail: { decision: "approve", approver_id: "team:owners", deciding_principal: decider } }, ...audit];
    },
  };
}

const person: ConsoleWhoami = { actorKind: "person", principal: "a@example.test", approverIds: ["team:owners"], canAdminister: false, callerScope: "caller-a" };

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("personal approval cards", () => {
  it("needs_you_counts_only_requests_for_you", async () => {
    const { fake } = gateway(person);
    render(<ConsoleApp baseUrl="" transport={fake} />);
    // The request routed to another approver waits for them, not for you.
    expect(await screen.findByRole("button", { name: "Needs you, 2 pending approvals" })).toBeTruthy();
  });

  it("card_marks_for_you_and_decided_by", async () => {
    const { fake, decideElsewhere } = gateway(person);
    render(<ConsoleApp baseUrl="" transport={fake} />);
    fireEvent.click(await screen.findByRole("button", { name: /^Needs you/ }));
    const routedToYou = await screen.findByTestId("gating-pending:p-owners");
    expect(within(routedToYou).getByText("For you")).toBeVisible();
    await waitFor(() => expect((within(routedToYou).getByTestId("gating-action:p-owners:approve") as HTMLButtonElement).disabled).toBe(false));
    const unrouted = screen.getByTestId("gating-pending:p-open");
    expect(within(unrouted).getByText("For you")).toBeVisible();
    const elsewhere = screen.getByTestId("gating-pending:p-finance");
    expect(within(elsewhere).getByText("Waiting for team:finance")).toBeVisible();
    expect(within(elsewhere).queryByTestId("gating-action:p-finance:approve")).toBeNull();
    // Someone else decides the request routed to you: its card stays, saying who.
    decideElsewhere("p-owners", "b@example.test");
    fireEvent.click(screen.getByTestId("gating-refresh"));
    const tombstone = await screen.findByTestId("gating-departed:p-owners");
    expect(tombstone).toHaveTextContent("Approved by b@example.test");
    expect(within(tombstone).queryByRole("button", { name: "Approve" })).toBeNull();
  });

  it("marks nothing for a caller who cannot decide, and still says whom a routed request waits for", async () => {
    const { fake } = gateway({ actorKind: "anonymous", approverIds: [], canAdminister: false, callerScope: "caller-open" });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    // Only the request routed to no one may need this caller (through the host).
    fireEvent.click(await screen.findByRole("button", { name: "Needs you, 1 pending approval" }));
    const unrouted = await screen.findByTestId("gating-pending:p-open");
    expect(within(unrouted).queryByText("For you")).toBeNull();
    expect(within(screen.getByTestId("gating-pending:p-finance")).getByText("Waiting for team:finance")).toBeVisible();
  });
});
