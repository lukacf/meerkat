/**
 * Decision cards of changes of long-term agents: each says that a person
 * cannot approve their own request, none holds a control for approving, and
 * the card of a request this page made offers its withdrawal.
 */
import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DecisionCardList } from "./DecisionCard";
import { decisionKindWords, isIdentityChangeDecision, type DecisionCardView } from "./decision-cards";

const NOW = Date.UTC(2030, 0, 1, 9, 0, 0);

function card(overrides: Partial<DecisionCardView> & { decisionId: string; kind: string }): DecisionCardView {
  return { phase: "open", deadlineAtMs: NOW + 3_600_000, statusOnly: false, ...overrides };
}

const whole = card({
  decisionId: "decision-whole",
  kind: "identity.update",
  approverId: "owner",
  approverName: "Owner",
  requester: 'principal:["human","example","carol"]',
  requesterName: "carol",
  rendering: "action: update",
});
const own = card({ decisionId: "decision-own", kind: "identity.create", statusOnly: true });
const other = card({ decisionId: "decision-other", kind: "tool_grant.issue", statusOnly: true });

describe("decision cards of agent changes", () => {
  it("names a change of a long-term agent in words", () => {
    expect(isIdentityChangeDecision("identity.create")).toBe(true);
    expect(isIdentityChangeDecision("tool_grant.issue")).toBe(false);
    expect(decisionKindWords("identity.create")).toBe("New long-term agent");
    expect(decisionKindWords("identity.update")).toBe("Change of a long-term agent");
    expect(decisionKindWords("identity.archive")).toBe("Archive of a long-term agent");
    expect(decisionKindWords("identity.unarchive")).toBe("Unarchive of a long-term agent");
    expect(decisionKindWords("identity.remove")).toBe("Removal of a long-term agent");
    expect(decisionKindWords("identity.adopt")).toBe("Release of an agent from configuration");
    // Another kind reads as before.
    expect(decisionKindWords("tool_grant.issue")).toBe("Tool grant issue");
  });

  it("says a person cannot approve their own request, and holds no control for approving", () => {
    const view = render(<DecisionCardList cards={[whole, own, other]} signedIn />);
    const wholeCard = view.getByTestId("decision-card:decision-whole");
    expect(wholeCard.querySelector(".cc-decision__hint")?.textContent)
      .toBe("Decide where it was presented. A person cannot approve their own request.");
    const ownCard = view.getByTestId("decision-card:decision-own");
    expect(ownCard.querySelector(".cc-decision__note")?.textContent)
      .toBe("You asked for this change. Only its approvers see what it holds, and a person cannot approve their own request.");
    // A decision of another kind reads as it did.
    expect(view.getByTestId("decision-card:decision-other").querySelector(".cc-decision__note")?.textContent)
      .toBe("You asked for this decision. Only its approvers see what it holds.");
    // No card offers a decision, and none a withdrawal the page cannot name.
    for (const element of [wholeCard, ownCard]) {
      expect(within(element).queryByTestId("decision-card-withdraw")).toBeNull();
      expect(Array.from(element.querySelectorAll("button")).some((button) => /approve|reject|decide/i.test(button.textContent || ""))).toBe(false);
    }
  });

  it("offers the withdrawal on the open card of a request this page made, and only there", async () => {
    const withdraw = vi.fn(async () => null as string | null);
    const closed = card({ decisionId: "decision-closed", kind: "identity.create", phase: "withdrawn", statusOnly: true });
    const ownOther = card({ decisionId: "decision-grant", kind: "tool_grant.issue", statusOnly: true });
    const view = render(
      <DecisionCardList
        cards={[whole, own, closed, ownOther]}
        own={{ decisionIds: new Set(["decision-own", "decision-closed", "decision-grant"]), withdraw }}
      />,
    );
    const offered = ["decision-whole", "decision-own", "decision-closed", "decision-grant"].filter((id) =>
      within(view.getByTestId(`decision-card:${id}`)).queryByTestId("decision-card-withdraw") !== null);
    expect(offered).toEqual(["decision-own"]);
    fireEvent.click(within(view.getByTestId("decision-card:decision-own")).getByTestId("decision-card-withdraw"));
    await waitFor(() => expect(withdraw).toHaveBeenCalledWith("decision-own"));
    expect(view.queryByTestId("decision-card-withdraw-refused")).toBeNull();

    // A refusal shows the gateway's sentence on the card.
    withdraw.mockResolvedValueOnce("The decision is closed: it was approved.");
    fireEvent.click(within(view.getByTestId("decision-card:decision-own")).getByTestId("decision-card-withdraw"));
    expect((await view.findByTestId("decision-card-withdraw-refused")).textContent).toBe("The decision is closed: it was approved.");
  });
});
