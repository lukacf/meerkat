import React from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it } from "vitest";

import type { ConversationCouncilEntry } from "@console-core";

import { CouncilCard, __councilCardUiState } from "./council-card";

function entryFixture(overrides: Partial<ConversationCouncilEntry> = {}): ConversationCouncilEntry {
  return {
    kind: "council",
    id: "council:c-1",
    identity: { id: "planner", label: "Planner", role: "assistant" },
    councilId: "c-1",
    topic: "Ship or hold 0.8.28?",
    status: "completed",
    exitReason: "completed",
    roundsCompleted: 2,
    participants: [
      {
        order: 0,
        role: "critic",
        sourceMobId: "m1",
        sourceIdentity: "alice",
        targetIdentity: "alice#fork",
        seated: true,
      },
      {
        order: 1,
        role: "advocate",
        sourceMobId: "m1",
        sourceIdentity: "bob",
        targetIdentity: "bob#fork",
        seated: true,
      },
    ],
    exchanges: [
      {
        round: 0,
        sequence: 0,
        participantOrder: 0,
        targetIdentity: "alice#fork",
        status: "completed",
        text: "ship it",
      },
    ],
    mergeKind: "bounded_text_summary",
    mergeFinalizer: "bob#fork",
    mergeText: "Consensus: ship.",
    ...overrides,
  };
}

beforeEach(() => {
  __councilCardUiState.reset();
});

it("renders the topic, participant count and the merge summary", () => {
  render(<CouncilCard entry={entryFixture()} />);
  expect(screen.getByText("Ship or hold 0.8.28?")).toBeTruthy();
  expect(screen.getByText("Consensus: ship.")).toBeTruthy();
  expect(screen.getByText(/2 participants/)).toBeTruthy();
});

it("offers no action affordance on a participant row", () => {
  // Council participants are forked contexts destroyed before the tool
  // returns. A button here would offer to act on something that is gone.
  const { container } = render(<CouncilCard entry={entryFixture()} />);
  const participantButtons = container.querySelectorAll(".cc-council__participant button");
  expect(participantButtons.length).toBe(0);
  // The only control on the card is the collapse toggle.
  expect(container.querySelectorAll("button").length).toBe(1);
});

it("renders a failed council distinctly and keeps the reason visible when collapsed", () => {
  const { container } = render(<CouncilCard entry={entryFixture({
    status: "failed",
    exitReason: "participant_seating_failed",
    exitDetail: "slot 1 · capability expired",
  })} />);
  const card = container.querySelector("[data-council-card]");
  expect(card?.getAttribute("data-status")).toBe("failed");
  expect(card?.className).toContain("is-failed");
  expect(screen.getByText("Failed")).toBeTruthy();
  expect(screen.getByText(/capability expired/)).toBeTruthy();

  // Collapsing hides the body but must NOT hide why it failed.
  fireEvent.click(screen.getByRole("button", { name: "Hide" }));
  expect(screen.getByText(/capability expired/)).toBeTruthy();
  expect(screen.queryByText("Consensus: ship.")).toBeNull();
});

it("separates a budget stop from a failure", () => {
  const { container } = render(<CouncilCard entry={entryFixture({
    status: "bounded",
    exitReason: "max_exchanges_reached",
  })} />);
  expect(container.querySelector("[data-council-card]")?.getAttribute("data-status")).toBe("bounded");
  expect(screen.getByText("Stopped at budget")).toBeTruthy();
  // A budget stop is not a failure and must not render the failure note.
  expect(container.querySelector(".cc-council__failure")).toBeNull();
});

it("renders an artifact claim as a claim and never as a link", () => {
  const { container } = render(<CouncilCard entry={entryFixture({
    artifactClaims: [{ uri: "blob://report.md", mediaType: "text/markdown" }],
  })} />);
  expect(screen.getByText("blob://report.md")).toBeTruthy();
  expect(screen.getByText("claimed")).toBeTruthy();
  // The council resolves nothing: no lookup, no fetch, no existence check.
  // An anchor would assert reachability nobody verified.
  expect(container.querySelectorAll(".cc-council__claims a").length).toBe(0);
});

it("shows cleanup debt without downgrading a good result", () => {
  render(<CouncilCard entry={entryFixture({
    cleanupDebts: [{ subject: "mob-tmp-1", detail: "destroy refused" }],
    cleanupBudgetExhausted: true,
  })} />);
  expect(screen.getByText("Concluded")).toBeTruthy();
  expect(screen.getByText(/destroy refused/)).toBeTruthy();
  expect(screen.getByText("budget exhausted")).toBeTruthy();
});

it("marks an unseated participant rather than leaving it as an absence", () => {
  render(<CouncilCard entry={entryFixture({
    participants: [{
      order: 0,
      role: "critic",
      sourceMobId: "m1",
      sourceIdentity: "alice",
      targetIdentity: "alice#fork",
      seated: false,
    }],
  })} />);
  expect(screen.getByText("never seated")).toBeTruthy();
  expect(screen.getByText(/0\/1 seated/)).toBeTruthy();
});

it("surfaces a pending exchange rather than hiding it", () => {
  // A receipt with no observed terminal is what a coordinator crash looks
  // like; it has to be visible.
  render(<CouncilCard entry={entryFixture({
    exchanges: [{
      round: 0,
      sequence: 0,
      participantOrder: 0,
      targetIdentity: "alice#fork",
      status: "pending",
    }],
  })} />);
  expect(screen.getByText("No terminal observed")).toBeTruthy();
});

it("reports the overflow count so a capped list never reads as complete", () => {
  render(<CouncilCard entry={entryFixture({ exchangeOverflowCount: 5 })} />);
  expect(screen.getByText("+5 more exchanges")).toBeTruthy();
  // The header still reports the TRUE total, not the rendered count.
  expect(screen.getByText(/6 exchanges/)).toBeTruthy();
});

it("marks a replayed council so a cached answer is not read as a fresh run", () => {
  render(<CouncilCard entry={entryFixture({ replayed: true })} />);
  expect(screen.getByText("replayed")).toBeTruthy();
});
