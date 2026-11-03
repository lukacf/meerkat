import { describe, expect, it } from "vitest";

import { memberKickoffNotice } from "./member-kickoff";

const notice = (overrides: Record<string, unknown>) => ({
  type: "comms",
  kind: "lifecycle",
  peer: { id: "peer-1", display_name: "mob/commander/incident-commander" },
  intent: "mob.kickoff_started",
  payload: { peer: "incident-commander", role: "commander" },
  ...overrides,
});

describe("memberKickoffNotice", () => {
  it("reads the typed phase, member and role from either notice form", () => {
    expect(memberKickoffNotice(notice({}))).toEqual({
      phase: "started", member: "incident-commander", role: "commander", peerIdentity: "peer-1",
    });
    expect(memberKickoffNotice(notice({ kind: "request", intent: "mob.kickoff_callback_pending" }))?.phase)
      .toBe("callback_pending");
  });

  it("falls back to the sender's display name when the payload names no member", () => {
    expect(memberKickoffNotice(notice({ payload: {} }))?.member).toBe("incident-commander");
  });

  it("is null for anything that is not a known kickoff phase on a comms notice", () => {
    expect(memberKickoffNotice(notice({ intent: "mob.kickoff_exploded" }))).toBeNull();
    expect(memberKickoffNotice(notice({ intent: "mob.member_paused" }))).toBeNull();
    expect(memberKickoffNotice(notice({ kind: "message" }))).toBeNull();
    expect(memberKickoffNotice(notice({ type: "background_job" }))).toBeNull();
  });
});
