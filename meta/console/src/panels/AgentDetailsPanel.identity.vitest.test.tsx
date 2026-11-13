/**
 * An agent's details and the changes of its record: a change that is open
 * belongs to the agent it was opened for. A pane that is shown another
 * agent in place carries no confirmation over to it.
 */
import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityInspectViewState } from "@console-core";
import type { IdentityChangesApi } from "../lib/use-identity-changes";
import { AgentDetailsPanel } from "./AgentDetailsPanel";

function inspect(identity: string, displayName: string): IdentityInspectViewState {
  return {
    identity,
    display_name: displayName,
    role: "worker",
    state: "active",
    addressability: "addressable",
    labels: {},
    continuity: { generation: 1, session_id: "session-1", agent_runtime_id: "rt-1" },
    topology_peers: [],
  } as IdentityInspectViewState;
}

beforeEach(() => {
  localStorage.clear();
});

describe("an agent's details and the changes of its record", () => {
  it("shows no confirmation of another agent's change when the pane is shown another agent in place", async () => {
    const loadRecord = vi.fn(async (name: string) => ({
      name, profile: "worker", specDigest: `sha256:${name}`, archived: false, readiness: "dormant", openOperations: [], instructions: [], labels: {}, peers: [],
    }));
    const change = vi.fn(async () => ({ kind: "failed", message: "no answer" }));
    const plan = vi.fn(async () => ({ kind: "unknown" }));
    const api = {
      flags: { available: true, canCreate: true, canChange: true, canRemove: true },
      revision: 0,
      pending: [],
      mintRequestId: () => "request-1",
      loadRecord,
      change,
      plan,
    } as unknown as IdentityChangesApi;
    const identityChanges = { api, controls: { edit: true, archive: true, remove: true } };
    const pane = (identity: string, displayName: string) => (
      <AgentDetailsPanel identity={identity} inspect={inspect(identity, displayName)} stateLabel="active" actions={{}} identityChanges={identityChanges} />
    );
    const view = render(pane("warden", "The warden"));
    fireEvent.click(view.getByTestId("inspect-menu:warden"));
    fireEvent.click(view.getByTestId("inspect-action:warden:remove"));
    expect(view.getByTestId("identity-confirm:warden:remove").textContent).toContain("Remove The warden for good?");

    // The same pane is shown the courier: the warden's question is gone.
    // The question read the warden's record when it opened; nothing was
    // read of the courier, and nothing was sent.
    view.rerender(pane("courier", "The courier"));
    expect(view.container.querySelectorAll("[data-testid^='identity-confirm:']").length).toBe(0);
    expect(loadRecord.mock.calls).toEqual([["warden"]]);
    expect(change).not.toHaveBeenCalled();

    // A removal opened for the courier names the courier and acts on it:
    // its record is read once, when the question opens, and Confirm sends
    // the request of that read.
    fireEvent.click(view.getByTestId("inspect-menu:courier"));
    fireEvent.click(view.getByTestId("inspect-action:courier:remove"));
    const confirm = view.getByTestId("identity-confirm:courier:remove");
    expect(confirm.textContent).toContain("Remove The courier for good?");
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    await waitFor(() => expect(change).toHaveBeenCalledTimes(1));
    expect(loadRecord.mock.calls).toEqual([["warden"], ["courier"]]);
    expect(change.mock.calls[0].slice(0, 2)).toEqual(["remove", { name: "courier", expected_spec_digest: "sha256:courier" }]);
    // The courier's question asked what its removal would take with it, of
    // the courier at the digest it read.
    await waitFor(() => expect(plan.mock.calls.at(-1)).toEqual(["remove", { name: "courier", expected_spec_digest: "sha256:courier" }]));
  });
});
