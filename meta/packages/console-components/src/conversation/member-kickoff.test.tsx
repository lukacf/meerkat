import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { MEMBER_KICKOFF_PHASES, memberKickoffPhaseLabel, type ConversationRichMemberKickoffBlock } from "@console-core";
import { ConversationRichContent } from "./conversation-rich-content";

describe("member-kickoff status card", () => {
  it("shows the member, role and phase, and names the status for assistive tech", () => {
    for (const phase of MEMBER_KICKOFF_PHASES) {
      const label = memberKickoffPhaseLabel(phase);
      const block: ConversationRichMemberKickoffBlock = {
        type: "member-kickoff",
        phase,
        member: "incident-commander",
        role: "commander",
        copyText: `Kickoff ${label.toLowerCase()}: incident-commander`,
      };
      const { container, unmount } = render(<ConversationRichContent blocks={[block]} />);
      const card = container.querySelector(".cc-member-kickoff");
      expect(card?.getAttribute("data-phase")).toBe(phase);
      expect(card?.getAttribute("aria-label")).toBe(block.copyText);
      expect(card?.querySelector(".cc-member-kickoff__member")?.textContent).toBe("incident-commander");
      expect(card?.querySelector(".cc-member-kickoff__role")?.textContent).toBe("commander");
      expect(card?.querySelector(".cc-member-kickoff__phase")?.textContent).toBe(label);
      unmount();
    }
  });

  it("omits the role when the notice carries none", () => {
    const { container } = render(<ConversationRichContent blocks={[{
      type: "member-kickoff", phase: "pending", member: "scribe", copyText: "Kickoff pending: scribe",
    }]} />);
    expect(container.querySelector(".cc-member-kickoff__role")).toBeNull();
  });
});
