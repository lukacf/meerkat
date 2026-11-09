import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";

import type {
  TopologyEndpoint,
  TopologyManagementState,
} from "@console-core";
import { topologyEdgeKey } from "@console-core";

import { ConnectionPicker } from "./connection-picker";

const endpoints: TopologyEndpoint[] = [
  { ref: { id: "commander" }, presentation: { label: "Commander", caption: "Incident lead", section: "Command" } },
  { ref: { id: "triage" }, presentation: { label: "Triage", caption: "Evidence analysis", section: "Response" } },
  { ref: { id: "responder" }, presentation: { label: "Responder", caption: "Remediation", section: "Response" } },
  { ref: { id: "comms" }, presentation: { label: "Communications", caption: "Status updates", section: "Coordination" } },
  { ref: { id: "observer" }, presentation: { label: "Observer", caption: "Protected audit endpoint", section: "Coordination" } },
];

const editable: TopologyManagementState = {
  revision: 42,
  policy: {
    mode: "editable",
    capabilities: {
      connect: { state: "allowed" },
      disconnect: { state: "allowed" },
      reconnect: { state: "allowed" },
    },
  },
  affordances: [
    {
      edge: { from: "commander", to: "triage" },
      state: "connected",
      actions: { disconnect: { state: "allowed" } },
    },
    {
      edge: { from: "commander", to: "responder" },
      state: "disconnected",
      actions: { connect: { state: "approval_required", reason: "Operator approval required" } },
    },
    {
      edge: { from: "commander", to: "comms" },
      state: "degraded",
      actions: { reconnect: { state: "allowed" } },
      message: "One side is missing the edge",
    },
    {
      edge: { from: "commander", to: "observer" },
      state: "connected",
      actions: { disconnect: { state: "denied", reason: "Protected audit route" } },
    },
  ],
};

describe("ConnectionPicker", () => {
  test("emits revisioned connect, disconnect, and reconnect intents", () => {
    const onRequestMutation = vi.fn();
    const { rerender } = render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[{ from: "commander", to: "triage" }, { from: "commander", to: "comms" }]}
        management={editable}
        sourceId="commander"
        onRequestMutation={onRequestMutation}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Disconnect Triage from Commander" }));
    expect(onRequestMutation).toHaveBeenLastCalledWith({
      action: "disconnect",
      edge: { from: "commander", to: "triage" },
      expectedRevision: 42,
      origin: "picker",
    });

    fireEvent.click(screen.getByRole("button", { name: "Request approval Responder to Commander" }));
    expect(onRequestMutation).toHaveBeenLastCalledWith({
      action: "connect",
      edge: { from: "commander", to: "responder" },
      expectedRevision: 42,
      origin: "picker",
    });

    fireEvent.click(screen.getByRole("button", { name: "Reconnect Communications to Commander" }));
    expect(onRequestMutation).toHaveBeenLastCalledWith({
      action: "reconnect",
      edge: { from: "commander", to: "comms" },
      expectedRevision: 42,
      origin: "picker",
    });
  });

  test("preserves bilateral authority revisions in host callbacks", () => {
    const onRequestMutation = vi.fn();
    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[{ from: "commander", to: "triage" }]}
        management={{
          ...editable,
          authorityRevisions: {
            "mob/alpha": 11,
            "mob/beta": 19,
          },
        }}
        sourceId="commander"
        onRequestMutation={onRequestMutation}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Disconnect Triage from Commander" }));
    expect(onRequestMutation).toHaveBeenCalledWith({
      action: "disconnect",
      edge: { from: "commander", to: "triage" },
      expectedRevision: 42,
      expectedAuthorityRevisions: {
        "mob/alpha": 11,
        "mob/beta": 19,
      },
      origin: "picker",
    });
  });

  test("uses the host-selected repair action for an ambiguous conflict", () => {
    const onRequestMutation = vi.fn();
    const conflict: TopologyManagementState = {
      ...editable,
      affordances: [{
        edge: { from: "commander", to: "triage" },
        state: "conflict",
        preferredAction: "disconnect",
        actions: { disconnect: { state: "allowed" } },
        message: "The suppressed edge is still physically present",
      }],
    };

    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[{ from: "commander", to: "triage" }]}
        management={conflict}
        sourceId="commander"
        onRequestMutation={onRequestMutation}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Disconnect Triage from Commander" }));
    expect(onRequestMutation).toHaveBeenCalledWith({
      action: "disconnect",
      edge: { from: "commander", to: "triage" },
      expectedRevision: 42,
      origin: "picker",
    });
  });

  test("shows denied pair reasons and fails closed", () => {
    const onRequestMutation = vi.fn();
    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[{ from: "commander", to: "observer" }]}
        management={editable}
        sourceId="commander"
        onRequestMutation={onRequestMutation}
      />,
    );

    expect(screen.getByText("Protected audit route")).toBeInTheDocument();
    const denied = screen.getByRole("button", { name: "Disconnect Observer from Commander" });
    expect(denied).toBeDisabled();
    fireEvent.click(denied);
    expect(onRequestMutation).not.toHaveBeenCalled();
  });

  test("requires an explicit check before a host prepares an unresolved pair", () => {
    const onRequestPairInspection = vi.fn();
    const onRequestMutation = vi.fn();
    render(
      <ConnectionPicker
        endpoints={endpoints.slice(0, 2)}
        edges={[]}
        management={{ ...editable, affordances: [] }}
        sourceId="commander"
        onRequestMutation={onRequestMutation}
        onRequestPairInspection={onRequestPairInspection}
      />,
    );

    expect(screen.getByText("Not inspected")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", {
      name: "Check Triage connection availability with Commander",
    }));
    expect(onRequestPairInspection).toHaveBeenCalledWith({ from: "commander", to: "triage" });
    expect(onRequestMutation).not.toHaveBeenCalled();
  });

  test("shows honest automatic resolution state without exposing Check", () => {
    const pairKey = topologyEdgeKey({ from: "commander", to: "triage" });
    const { rerender } = render(
      <ConnectionPicker
        endpoints={endpoints.slice(0, 2)}
        edges={[]}
        management={{ ...editable, affordances: [] }}
        sourceId="commander"
        interactionMode="direct"
        resolvingPairKeys={new Set([pairKey])}
        onRequestPairInspection={vi.fn()}
        onRequestMutation={vi.fn()}
      />,
    );

    expect(screen.getByText("Loading…")).toBeInTheDocument();
    expect(screen.getByText("Loading current connection status from Meta.")).toBeInTheDocument();
    expect(screen.queryByText("Not inspected")).toBeNull();
    expect(screen.queryByRole("button", { name: /Check .* connection availability/u })).toBeNull();
    expect(screen.queryByRole("button", { name: /Connect Triage to Commander/u })).toBeNull();

    rerender(
      <ConnectionPicker
        endpoints={endpoints.slice(0, 2)}
        edges={[]}
        management={{ ...editable, affordances: [] }}
        sourceId="commander"
        interactionMode="direct"
        resolvingPairKeys={new Set()}
        onRequestMutation={vi.fn()}
      />,
    );
    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    expect(screen.getByText("Current connection status is unavailable.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Connect Triage to Commander/u })).toBeNull();
  });

  test("uses connection language before a direct-mode source is selected", () => {
    render(
      <ConnectionPicker
        endpoints={endpoints.slice(0, 2)}
        edges={[]}
        management={{ ...editable, affordances: [] }}
        interactionMode="direct"
      />,
    );

    expect(screen.getByText("Pick an endpoint below to see its connections.")).toBeInTheDocument();
    expect(screen.queryByText(/inspect/iu)).toBeNull();
  });

  test("keeps reconnect semantics behind direct Connect and Disconnect controls", () => {
    const onRequestMutation = vi.fn();
    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[{ from: "commander", to: "triage" }, { from: "commander", to: "comms" }]}
        management={{
          ...editable,
          affordances: editable.affordances.map((affordance) => (
            affordance.edge.to === "comms"
              ? {
                  ...affordance,
                  message: "Disconnected by operator. Reconnection restores the declared relationship after inspection.",
                }
              : affordance
          )),
        }}
        sourceId="commander"
        interactionMode="direct"
        onRequestMutation={onRequestMutation}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Disconnect Triage from Commander" }));
    expect(onRequestMutation).toHaveBeenLastCalledWith(expect.objectContaining({
      action: "disconnect",
      edge: { from: "commander", to: "triage" },
    }));

    fireEvent.click(screen.getByRole("button", { name: "Connect Communications to Commander" }));
    expect(onRequestMutation).toHaveBeenLastCalledWith(expect.objectContaining({
      action: "reconnect",
      edge: { from: "commander", to: "comms" },
    }));
    expect(screen.queryByRole("button", { name: /Reconnect/u })).toBeNull();
    expect(screen.queryByText(/reconnect/iu)).toBeNull();
    expect(screen.queryByText(/inspection/iu)).toBeNull();
    expect(screen.getByText("This connection needs repair.")).toBeInTheDocument();
  });

  test("does not expose raw inspection or reconnection failures in direct mode", () => {
    render(
      <ConnectionPicker
        endpoints={endpoints.slice(0, 2)}
        edges={[]}
        management={{
          ...editable,
          affordances: [{
            edge: { from: "commander", to: "triage" },
            state: "disconnected",
            actions: {
              connect: {
                state: "denied",
                reason: "Pair inspection denied; reconnection requires Meta policy approval.",
              },
            },
            message: "Inspection denied during reconnection preparation.",
          }],
        }}
        sourceId="commander"
        interactionMode="direct"
        onRequestMutation={vi.fn()}
      />,
    );

    expect(screen.getByText("Not connected")).toBeInTheDocument();
    expect(screen.getByText("This connection cannot be changed right now.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect Triage to Commander" })).toBeDisabled();
    expect(screen.queryByText(/inspect|reconnect/iu)).toBeNull();
    expect(screen.getByRole("button", { name: "Connect Triage to Commander" }))
      .toHaveAttribute("title", "This connection cannot be changed right now.");
  });

  test("shows pending approval and blocks duplicate requests", () => {
    const onRequestMutation = vi.fn();
    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[]}
        management={{
          ...editable,
          operations: [{
            operationId: "approval-1",
            action: "connect",
            edge: { from: "commander", to: "responder" },
            status: "pending_approval",
            message: "Waiting for incident lead",
          }],
        }}
        sourceId="commander"
        onRequestMutation={onRequestMutation}
      />,
    );

    expect(screen.getByText("Pending approval")).toBeInTheDocument();
    expect(screen.getByText("Waiting for incident lead")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Awaiting approval Responder to Commander" })).toBeDisabled();
    expect(onRequestMutation).not.toHaveBeenCalled();
  });

  test("shows conflict, partial, degraded, and retry states", () => {
    const onRetryOperation = vi.fn();
    const receipt = {
      operationId: "partial-1",
      action: "reconnect" as const,
      edge: { from: "commander", to: "comms" },
      status: "partial" as const,
      message: "Remote endpoint did not acknowledge",
      retryable: true,
    };
    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[{ from: "commander", to: "comms" }]}
        management={{
          ...editable,
          health: "conflict",
          message: "Topology revision changed while applying the plan",
          operations: [receipt],
        }}
        sourceId="commander"
        onRetryOperation={onRetryOperation}
      />,
    );

    expect(screen.getByText("Topology conflict")).toBeInTheDocument();
    expect(screen.getByText(/revision changed/)).toBeInTheDocument();
    expect(screen.getByText("Partial")).toBeInTheDocument();
    expect(screen.getByText("Remote endpoint did not acknowledge")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry Communications to Commander" }));
    expect(onRetryOperation).toHaveBeenCalledWith(receipt);
  });

  test("labels transport-ambiguous recovery as resolution rather than a fresh retry", () => {
    const onRetryOperation = vi.fn();
    const receipt = {
      operationId: "topology-stable-key",
      idempotencyKey: "topology-stable-key",
      action: "connect" as const,
      edge: { from: "commander", to: "responder" },
      status: "failed" as const,
      message: "The apply response was lost",
      retryable: true,
      retryMode: "resolve_ambiguous" as const,
    };
    const { rerender } = render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[]}
        management={{ ...editable, operations: [receipt] }}
        sourceId="commander"
        onRetryOperation={onRetryOperation}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Resolve Responder to Commander" }));
    expect(onRetryOperation).toHaveBeenCalledWith(receipt);

    const rebaseReceipt = {
      ...receipt,
      status: "conflict" as const,
      retryMode: "revision_rebase" as const,
    };
    rerender(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[]}
        management={{ ...editable, operations: [rebaseReceipt] }}
        sourceId="commander"
        onRetryOperation={onRetryOperation}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Rebase Responder to Commander" }));
    expect(onRetryOperation).toHaveBeenLastCalledWith(rebaseReceipt);
  });

  test("has no bulk control by default and only runs an explicitly bounded host action", () => {
    const onRequestBulkAction = vi.fn();
    const { rerender } = render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[]}
        management={editable}
        sourceId="commander"
      />,
    );
    expect(screen.queryByTestId("connection-picker-bulk-actions")).toBeNull();

    const withBulk: TopologyManagementState = {
      ...editable,
      policy: {
        ...editable.policy,
        maxBatchSize: 4,
        capabilities: { ...editable.policy.capabilities, bulk: { state: "allowed" } },
      },
    };
    const bounded = {
      id: "connect-response-team",
      label: "Connect response team",
      operationCount: 3,
      maxOperations: 4,
      capability: { state: "allowed" as const },
    };
    rerender(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[]}
        management={withBulk}
        sourceId="commander"
        bulkActions={[bounded]}
        onRequestBulkAction={onRequestBulkAction}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Connect response team/ }));
    expect(onRequestBulkAction).toHaveBeenCalledWith(bounded);
  });

  test("rejects an unbounded or oversized host bulk action", () => {
    const onRequestBulkAction = vi.fn();
    render(
      <ConnectionPicker
        endpoints={endpoints}
        edges={[]}
        management={{
          ...editable,
          policy: {
            ...editable.policy,
            maxBatchSize: 2,
            capabilities: { ...editable.policy.capabilities, bulk: { state: "allowed" } },
          },
        }}
        sourceId="commander"
        bulkActions={[{
          id: "too-many",
          label: "Connect selected",
          operationCount: 5,
          maxOperations: Number.POSITIVE_INFINITY,
          capability: { state: "allowed" },
        }]}
        onRequestBulkAction={onRequestBulkAction}
      />,
    );

    const button = screen.getByRole("button", { name: /Connect selected/ });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(onRequestBulkAction).not.toHaveBeenCalled();
  });

  test("searches a capped roster of hundreds without rendering every row", async () => {
    const largeRoster: TopologyEndpoint[] = Array.from({ length: 260 }, (_, index) => ({
      ref: { id: `endpoint-${index}` },
      presentation: {
        label: `Responder ${index}`,
        caption: index === 259 ? "Rare database specialist" : "General response",
        section: "Responders",
      },
    }));
    render(
      <ConnectionPicker
        endpoints={largeRoster}
        edges={[]}
        management={{ ...editable, affordances: [] }}
        sourceId="endpoint-0"
        visibleLimit={25}
      />,
    );

    expect(screen.getByText(/234 more endpoints/)).toBeInTheDocument();
    expect(screen.queryByTestId("connection-picker-row:endpoint-259")).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "Search endpoints" }), {
      target: { value: "Rare database" },
    });
    await waitFor(() => {
      expect(screen.getByTestId("connection-picker-row:endpoint-259")).toBeInTheDocument();
    });
    expect(screen.queryByText(/more endpoints/)).toBeNull();
  });
});
