import { describe, expect, it } from "vitest";

import { CONSOLE_SCHEDULE_COMMANDS, scheduleErrorMessage, scheduleErrorOfAnotherTeam } from "./schedules";

// The refusal "no schedule would be sent" has two reasons under one kind:
// no schedule host sends this gateway's schedules at all, or the gateway
// sends schedules for its own team's agents only and the agent is of
// another team. For an agent of another team the console cannot tell the
// two apart (the kind is the same, and the gateway's message names the
// agent and its team by their ids), so it says one sentence that is true
// of both: this gateway sends no schedules for agents of that team.

const refused = () => Object.assign(new Error("meta/schedule/create RPC error"), {
  rpcError: {
    code: -32004,
    message: "this gateway sends the console schedules of its default mob's agents only, so a console schedule for reviewer of mob support would never be sent",
    data: { kind: "schedule_host_unavailable" },
  },
});

describe("the sentence for a schedule that would never be sent", () => {
  it("is the one about the schedule host for an agent of the console's own team, as before", () => {
    for (const command of [CONSOLE_SCHEDULE_COMMANDS.create, CONSOLE_SCHEDULE_COMMANDS.update, CONSOLE_SCHEDULE_COMMANDS.resume]) {
      expect(scheduleErrorMessage(refused(), command)).toBe("No schedule host sends this gateway's schedules, so a schedule would never be sent.");
    }
  });

  it("for an agent of another team says what is true whichever the reason: no schedules are sent for that team's agents", () => {
    const create = scheduleErrorMessage(scheduleErrorOfAnotherTeam(refused(), "Support team"), CONSOLE_SCHEDULE_COMMANDS.create);
    expect(create).toBe("This gateway sends no schedules for agents of Support team, so a schedule for this agent would never be sent. Nothing was saved.");
    // It does not say that schedules work for anyone: that is not known from this refusal.
    expect(create).not.toMatch(/only|for now|Ops team/);
    for (const command of [CONSOLE_SCHEDULE_COMMANDS.update, CONSOLE_SCHEDULE_COMMANDS.resume]) {
      const said = scheduleErrorMessage(scheduleErrorOfAnotherTeam(refused(), "Support team"), command);
      expect(said).toBe("This gateway sends no schedules for agents of Support team, so a schedule for this agent would never be sent. Nothing was changed.");
      // The team by its name for people, never by its id as the gateway's message has it.
      expect(said).not.toContain("of mob support");
    }
    // No name for the agent's team: still no id.
    expect(scheduleErrorMessage(scheduleErrorOfAnotherTeam(refused(), null), CONSOLE_SCHEDULE_COMMANDS.create))
      .toBe("This gateway sends no schedules for agents of this agent's team, so a schedule for this agent would never be sent. Nothing was saved.");
  });

  it("leaves every other refusal of such an agent as it reads for any agent", () => {
    const denied = Object.assign(new Error("denied"), { rpcError: { code: -32030, message: "denied", data: { kind: "read_only" } } });
    expect(scheduleErrorMessage(scheduleErrorOfAnotherTeam(denied, "Support team"), CONSOLE_SCHEDULE_COMMANDS.create))
      .toBe("This console is read-only, so schedules cannot be changed.");
  });
});
