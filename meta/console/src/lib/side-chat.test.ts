import assert from "node:assert/strict";
import test from "node:test";

import type { ConsoleAgent } from "../types";
import {
  FORK_INSTRUCTION_REFUSED_CODE,
  SIDE_CHAT_FORK_REFUSED_CODE,
  findSideChatAgent,
  inactiveInstructionNotice,
  parseSideChatForkResult,
  restartOffered,
  sideChatAlias,
  sideChatForkBlockedReason,
  sideChatForkErrorMessage,
  sideChatForkOffered,
  sideChatSourceMemberId,
  sideChatsOf,
} from "./side-chat";

function agent(memberId: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent {
  return {
    agent_id: memberId,
    member_id: memberId,
    identity: memberId,
    label: memberId,
    kind: "mob_agent",
    ...extra,
  };
}

function rpcError(code: number, data?: unknown): Error {
  return Object.assign(new Error(`rpc ${code}`), { rpcError: { code, message: `rpc ${code}`, data } });
}

test("a side chat alias names its source and stays out of runtime-owned namespaces", () => {
  assert.equal(sideChatAlias("alpha", "K3x9q2"), "alpha-side-k3x9q2");
  assert.equal(sideChatAlias("domain:calendar", "ab12"), "domain:calendar-side-ab12");
  assert.equal(sideChatAlias("rt:review:main:3", "ab12"), "review:main:3-side-ab12");
  assert.equal(sideChatAlias("mk--alpha", "ab12"), "alpha-side-ab12");
  assert.equal(sideChatAlias("  team lead / east ", "ab12"), "team-lead-east-side-ab12");
  assert.equal(sideChatAlias("", "!!"), "agent-side-0");
  const long = sideChatAlias("x".repeat(200), "n".repeat(40));
  assert.ok(long.length <= 40 + "-side-".length + 12, long);
});

test("the fork names the agent by its identity, else its member id", () => {
  assert.equal(sideChatSourceMemberId(agent("worker-1", { identity: "domain:calendar" })), "domain:calendar");
  assert.equal(sideChatSourceMemberId(agent("worker-1", { identity: undefined })), "worker-1");
});

test("fork waits for an idle agent", () => {
  const idle = { busy: false, phase: null, responsePhase: null, queued: 0 };
  assert.equal(sideChatForkBlockedReason(idle), null);
  assert.equal(sideChatForkBlockedReason({ ...idle, responsePhase: undefined }), null);
  assert.notEqual(sideChatForkBlockedReason({ ...idle, busy: true }), null);
  assert.notEqual(sideChatForkBlockedReason({ ...idle, phase: "generating" }), null);
  assert.notEqual(sideChatForkBlockedReason({ ...idle, responsePhase: "waiting" }), null);
  assert.notEqual(sideChatForkBlockedReason({ ...idle, queued: 1 }), null);
});

test("the fork result is read typed and fails closed", () => {
  const parsed = parseSideChatForkResult({
    agent_identity: "alpha-side-ab12",
    side_chat_of: "alpha",
    display_name: "alpha (side chat)",
    session_id: "s-1",
    retention: "until_closed",
    instructions: [
      { namespace: "app.skills", key: "calendar", state: "inherited" },
      { namespace: "app.skills", key: "billing", state: "inactive", reason: { kind: "not_effective_in_source" } },
      { namespace: "", key: "broken", state: "inactive" },
    ],
  });
  assert.deepEqual(parsed, {
    agentIdentity: "alpha-side-ab12",
    sideChatOf: "alpha",
    displayName: "alpha (side chat)",
    sessionId: "s-1",
    instructions: [
      { namespace: "app.skills", key: "calendar", state: "inherited" },
      { namespace: "app.skills", key: "billing", state: "inactive", reason: "not_effective_in_source" },
    ],
  });
  assert.throws(() => parseSideChatForkResult({ side_chat_of: "alpha" }));
  assert.throws(() => parseSideChatForkResult(null));
});

test("instructions a fork could not inherit are named, with why", () => {
  assert.equal(inactiveInstructionNotice([]), null);
  assert.equal(
    inactiveInstructionNotice([{ namespace: "app.skills", key: "calendar", state: "inherited" }]),
    null,
  );
  assert.equal(
    inactiveInstructionNotice([
      { namespace: "app.skills", key: "calendar", state: "inherited" },
      { namespace: "app.skills", key: "billing", state: "inactive", reason: "not_effective_in_source" },
    ]),
    "Not inherited: app.skills/billing (not active in the source). This side chat cannot answer until it is activated.",
  );
  assert.equal(
    inactiveInstructionNotice([
      { namespace: "app.skills", key: "calendar", state: "inactive", reason: "head_body_retired" },
    ]),
    "Not inherited: app.skills/calendar (a compaction of the source retired its text). This side chat cannot answer until it is activated.",
  );
});

test("typed fork refusals read as what the operator can do", () => {
  assert.match(
    sideChatForkErrorMessage(rpcError(SIDE_CHAT_FORK_REFUSED_CODE, { kind: "fork_source_unavailable", cause: "running" })),
    /when it is idle/,
  );
  assert.match(
    sideChatForkErrorMessage(rpcError(SIDE_CHAT_FORK_REFUSED_CODE, {
      kind: "unwired_seat_unavailable",
      wiring: { rule: "seated_with_peers", peers: ["lead"] },
      retired: true,
    })),
    /closed again/,
  );
  assert.match(
    sideChatForkErrorMessage(rpcError(SIDE_CHAT_FORK_REFUSED_CODE, {
      kind: "unwired_seat_unavailable",
      wiring: { rule: "seated_with_peers", peers: ["lead"] },
      retired: false,
    })),
    /could not be closed/,
  );
  assert.match(
    sideChatForkErrorMessage(rpcError(SIDE_CHAT_FORK_REFUSED_CODE, {
      kind: "fork_source_changed",
      agent_identity: "alpha-side",
      source_member_id: "alpha",
      retired: true,
    })),
    /closed again\. Fork it again/,
  );
  assert.match(
    sideChatForkErrorMessage(rpcError(SIDE_CHAT_FORK_REFUSED_CODE, {
      kind: "fork_source_changed",
      retired: false,
    })),
    /could not be closed/,
  );
  assert.match(sideChatForkErrorMessage(rpcError(FORK_INSTRUCTION_REFUSED_CODE)), /instructions/);
  assert.equal(sideChatForkErrorMessage(new Error("network down")), "network down");
});

test("a pane offers the fork only where the viewer may fork the agent", () => {
  const sendable = agent("alpha", { affordances: { can_send_message: true } });
  const unsendable = agent("alpha", { affordances: { can_send_message: false } });
  const offered = { readOnly: false, offered: true, canSpawnMembers: true, accessEnforcing: true };
  assert.equal(sideChatForkOffered({ ...offered, agent: sendable }), true);
  assert.equal(
    sideChatForkOffered({ ...offered, agent: unsendable }),
    false,
    "under enforced access a viewer who may not send to the agent may not fork it",
  );
  assert.equal(
    sideChatForkOffered({ ...offered, accessEnforcing: false, agent: unsendable }),
    true,
    "without enforcement can_send_message only says the agent is not addressable",
  );
  assert.equal(sideChatForkOffered({ ...offered, agent: agent("alpha") }), true);
  assert.equal(sideChatForkOffered({ ...offered, readOnly: true, agent: sendable }), false);
  assert.equal(sideChatForkOffered({ ...offered, offered: false, agent: sendable }), false);
  assert.equal(sideChatForkOffered({ ...offered, canSpawnMembers: false, agent: sendable }), false);
  assert.equal(sideChatForkOffered({ ...offered, agent: null }), false);
});

test("a side chat offers no respawn or reset", () => {
  assert.equal(restartOffered(agent("alpha")), true);
  assert.equal(restartOffered(agent("alpha-side-ab12", { side_chat_of: "alpha" })), false);
  assert.equal(restartOffered(null), true);
});

test("side chats are found by alias and listed under their source", () => {
  const source = agent("alpha");
  const side = agent("alpha-side-ab12", { side_chat_of: "alpha" });
  const other = agent("beta-side-cd34", { side_chat_of: "beta" });
  const agents = [source, side, other, agent("beta")];
  assert.equal(findSideChatAgent(agents, "ALPHA-side-ab12"), side);
  assert.equal(findSideChatAgent(agents, "gamma"), null);
  assert.deepEqual(sideChatsOf(agents, source), [side]);
  assert.deepEqual(sideChatsOf(agents, side), []);
});
