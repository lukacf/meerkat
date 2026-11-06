import React from "react";
import { cleanup, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GatingInboxPanel } from "./GatingInboxPanel";

afterEach(cleanup);

const RENDERING = [
  "Release build 7 to production.",
  "Change log: 14 commits, 2 migrations.",
  "Rollback: redeploy build 6.",
].join("\n");

const HELD = {
  decisionId: `decision:${"c".repeat(64)}`,
  kind: "route.release",
  phase: "open",
  approverId: "team:owners",
  approverName: "Owners",
  requester: 'principal:["human","oidc-issuer:https://id.example","carol"]',
  requesterName: "Carol",
  deadlineAtMs: Date.UTC(2030, 0, 1),
  rendering: RENDERING,
  presentations: [{ channelRef: "chat:owners", messageRef: "message-1", atMs: Date.UTC(2029, 0, 1) }],
  statusOnly: false,
};

describe("GatingInboxPanel decision cards", () => {
  it("decision_card_renders_the_whole_rendering_and_hides_actions_without_sign_in", () => {
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} decisions={[HELD]} signedIn={false} />,
    );
    const card = view.getByTestId(`decision-card:${HELD.decisionId}`);
    const shown = within(card).getByTestId("decision-card-rendering");
    expect(shown.textContent).toBe(RENDERING);
    expect(within(card).getByText("Owners")).toBeTruthy();
    expect(within(card).queryByRole("button", { name: /approve/i })).toBeNull();
    expect(within(card).queryByRole("button", { name: /reject/i })).toBeNull();
    expect(within(card).getByText(/decide where it was presented/i)).toBeTruthy();
  });

  it("a_requesters_card_shows_status_only", () => {
    const status = {
      decisionId: HELD.decisionId,
      kind: HELD.kind,
      phase: "open",
      deadlineAtMs: HELD.deadlineAtMs,
      statusOnly: true,
    };
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} decisions={[status]} signedIn />,
    );
    const card = view.getByTestId(`decision-card:${HELD.decisionId}`);
    expect(within(card).getByTestId("decision-card-status").textContent).toMatch(/open/i);
    expect(within(card).queryByTestId("decision-card-rendering")).toBeNull();
    expect(card.textContent).not.toContain("Release build 7");
    expect(within(card).queryByRole("button", { name: /approve/i })).toBeNull();
  });

  it("a_status_card_says_whose_request_it_is_and_points_to_no_decision", () => {
    const status = {
      decisionId: HELD.decisionId,
      kind: HELD.kind,
      phase: "open",
      deadlineAtMs: HELD.deadlineAtMs,
      statusOnly: true,
    };
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} decisions={[status]} signedIn />,
    );
    const card = view.getByTestId(`decision-card:${HELD.decisionId}`);
    expect(within(card).getByText(/you asked for this decision/i)).toBeTruthy();
    expect(within(card).queryByText(/decide where it was presented/i)).toBeNull();
  });
});
