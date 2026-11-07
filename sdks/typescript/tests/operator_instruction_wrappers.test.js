/**
 * Operator instructions on the TypeScript SDK: the session and mob verbs send
 * the exact RPC literals with snake_case params; the status reads parse
 * fail-closed on the closed vocabularies (dispositions, effects, entry kinds,
 * state kinds) and keep an advisory value this SDK does not know (a pending
 * or blocked reason, a residency, a lowering) as `{ unknown: <the value> }`;
 * a refusal's closed code reads typed from the error data; the
 * applied-instructions event is schema-known.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient, MeerkatError, Mob, Session, parseEvent } from "../dist/index.js";

const SESSION_ID = "019a2b3c-0000-7000-8000-000000000001";
const RUN_ID = "019a2b3c-0000-7000-8000-000000000002";
const DIGEST = `sha256:${"a".repeat(64)}`;

const MOB_WRITE = {
  mob_id: "team",
  namespace: "app.example",
  key: "tone",
  revision: 1,
  disposition: "committed",
  generation: 1,
  matched_members: 2,
};

const SESSION_WRITE = {
  session_id: SESSION_ID,
  namespace: "app.example",
  key: "tone",
  revision: 1,
  disposition: "recorded",
  target_generation: 1,
};

function memberStatus({ residency = "dormant", deliveryState, slotState } = {}) {
  return {
    agent_identity: "worker-1",
    profile: "worker",
    residency,
    delivery: { recorded_generation: 1, state: deliveryState ?? { state: "current" } },
    slots: [
      {
        namespace: "app.example",
        key: "tone",
        desired: { revision: 1, effect: "active" },
        recorded: { revision: 1, effect: "active" },
        effective: {
          revision: 1,
          effect: "active",
          entry_sequence: 3,
          kind: "delta",
          run_id: RUN_ID,
          message_index: 7,
        },
        state: slotState ?? { state: "effective" },
      },
    ],
    effective_read: { status: "ok" },
    live_channel_open: false,
  };
}

function mobStatus(member) {
  return {
    mob_id: "team",
    generation: 1,
    desired: [
      {
        namespace: "app.example",
        key: "tone",
        revision: 1,
        effect: "active",
        target: { kind: "profile", profile: "worker" },
        content_sha256: DIGEST,
        body_bytes: 16,
      },
    ],
    members: [member],
    next_offset: 1,
  };
}

function sessionStatus({ lowering = "mid_conversation", reason = "awaiting_run_start" } = {}) {
  return {
    session_id: SESSION_ID,
    owner: { kind: "session" },
    target_generation: 2,
    entry_sequence: 3,
    lowering,
    slots: [
      {
        namespace: "app.example",
        key: "tone",
        recorded: { revision: 2, effect: "active" },
        effective: {
          revision: 1,
          effect: "active",
          entry_sequence: 3,
          kind: "snapshot",
          message_index: 1,
        },
        state: { state: "pending", reason },
      },
    ],
  };
}

function fakeClient(results) {
  const client = new MeerkatClient();
  const calls = [];
  client.request = async (method, params) => {
    calls.push({ method, params });
    const result = results[method];
    if (!result) {
      throw new Error(`no canned result for ${method}`);
    }
    return structuredClone(result);
  };
  return { client, calls };
}

describe("operator instruction wrappers", () => {
  it("exposes the operator-instruction verbs", () => {
    for (const name of [
      "setMobInstruction",
      "revokeMobInstruction",
      "mobInstructionStatus",
      "setSessionInstruction",
      "revokeSessionInstruction",
      "sessionInstructionStatus",
    ]) {
      assert.equal(typeof MeerkatClient.prototype[name], "function", `MeerkatClient.${name}`);
    }
    for (const name of ["setInstruction", "revokeInstruction", "instructionStatus"]) {
      assert.equal(typeof Mob.prototype[name], "function", `Mob.${name}`);
      assert.equal(typeof Session.prototype[name], "function", `Session.${name}`);
    }
  });

  it("the mob verbs send the exact request shapes", async () => {
    const { client, calls } = fakeClient({
      "mob/set_instruction": MOB_WRITE,
      "mob/revoke_instruction": { ...MOB_WRITE, revision: 2, generation: 2 },
    });
    const mob = new Mob(client, "team");

    const written = await mob.setInstruction(
      "app.example",
      "tone",
      1,
      "Answer formally.",
      { kind: "profile", profile: "worker" },
      { contentSha256: DIGEST },
    );
    const revoked = await client.revokeMobInstruction("team", "app.example", "tone", 2);

    assert.deepEqual(calls, [
      {
        method: "mob/set_instruction",
        params: {
          mob_id: "team",
          namespace: "app.example",
          key: "tone",
          revision: 1,
          body: "Answer formally.",
          content_sha256: DIGEST,
          target: { kind: "profile", profile: "worker" },
        },
      },
      {
        method: "mob/revoke_instruction",
        params: { mob_id: "team", namespace: "app.example", key: "tone", revision: 2 },
      },
    ]);
    assert.deepEqual(written, MOB_WRITE);
    assert.equal(revoked.generation, 2);
  });

  it("the mob status read sends its filters and parses the page", async () => {
    const page = mobStatus(memberStatus());
    const { client, calls } = fakeClient({ "mob/instruction_status": page });

    const status = await client.mobInstructionStatus("team", {
      namespace: "app.example",
      key: "tone",
      profile: "worker",
      offset: 0,
      limit: 50,
    });

    assert.deepEqual(calls, [
      {
        method: "mob/instruction_status",
        params: {
          mob_id: "team",
          namespace: "app.example",
          key: "tone",
          profile: "worker",
          offset: 0,
          limit: 50,
        },
      },
    ]);
    assert.deepEqual(status, page);
  });

  it("the mob status read keeps unknown advisory values as the unknown form", async () => {
    const member = memberStatus({
      residency: "orbiting",
      deliveryState: { state: "blocked", reason: "host_on_holiday" },
      slotState: { state: "pending", reason: "awaiting_the_moon" },
    });
    const { client } = fakeClient({ "mob/instruction_status": mobStatus(member) });

    const status = await client.mobInstructionStatus("team");

    assert.deepEqual(status.members[0].residency, { unknown: "orbiting" });
    assert.deepEqual(status.members[0].delivery.state, {
      state: "blocked",
      reason: { unknown: "host_on_holiday" },
    });
    assert.deepEqual(status.members[0].slots[0].state, {
      state: "pending",
      reason: { unknown: "awaiting_the_moon" },
    });
  });

  for (const member of [
    memberStatus({ slotState: { state: "levitating" } }),
    memberStatus({ deliveryState: { state: "teleported" } }),
  ]) {
    it(`the mob status read refuses an unknown closed value ${JSON.stringify(member.delivery.state)}`, async () => {
      const { client } = fakeClient({ "mob/instruction_status": mobStatus(member) });
      await assert.rejects(client.mobInstructionStatus("team"), (error) => {
        assert.ok(error instanceof MeerkatError);
        assert.equal(error.code, "INVALID_RESPONSE");
        return true;
      });
    });
  }

  it("the session verbs send the exact request shapes", async () => {
    const { client, calls } = fakeClient({
      "session/set_instruction": SESSION_WRITE,
      "session/revoke_instruction": { ...SESSION_WRITE, revision: 2, target_generation: 2 },
      "session/instruction_status": sessionStatus(),
    });
    const session = new Session(client, { sessionId: SESSION_ID, sessionRef: undefined });

    const written = await session.setInstruction("app.example", "tone", 1, "Answer formally.");
    const revoked = await session.revokeInstruction("app.example", "tone", 2);
    const status = await session.instructionStatus();

    assert.deepEqual(calls, [
      {
        method: "session/set_instruction",
        params: {
          session_id: SESSION_ID,
          namespace: "app.example",
          key: "tone",
          revision: 1,
          body: "Answer formally.",
        },
      },
      {
        method: "session/revoke_instruction",
        params: { session_id: SESSION_ID, namespace: "app.example", key: "tone", revision: 2 },
      },
      { method: "session/instruction_status", params: { session_id: SESSION_ID } },
    ]);
    assert.deepEqual(written, SESSION_WRITE);
    assert.equal(revoked.target_generation, 2);
    assert.deepEqual(status, sessionStatus());
  });

  it("the session status read keeps unknown advisory values as the unknown form", async () => {
    const { client } = fakeClient({
      "session/instruction_status": sessionStatus({
        lowering: "carrier_pigeon",
        reason: "awaiting_the_tide",
      }),
    });

    const status = await client.sessionInstructionStatus(SESSION_ID);

    assert.deepEqual(status.lowering, { unknown: "carrier_pigeon" });
    assert.deepEqual(status.slots[0].state, {
      state: "pending",
      reason: { unknown: "awaiting_the_tide" },
    });
  });

  it("the session status read refuses an unknown owner kind", async () => {
    const page = sessionStatus();
    page.owner = { kind: "committee" };
    const { client } = fakeClient({ "session/instruction_status": page });
    await assert.rejects(client.sessionInstructionStatus(SESSION_ID), (error) => {
      assert.ok(error instanceof MeerkatError);
      assert.equal(error.code, "INVALID_RESPONSE");
      return true;
    });
  });

  it("a refusal reads its closed instruction code", async () => {
    const { instructionSetCode } = await import("../dist/index.js");
    assert.equal(typeof instructionSetCode, "function", "instructionSetCode is exported");
    const governed = new MeerkatError("-32602", "governed", {
      instruction_set_code: "prepared_authorization_required",
    });
    assert.equal(instructionSetCode(governed), "prepared_authorization_required");
    assert.equal(
      instructionSetCode(new MeerkatError("-32602", "stale", { instruction_set_code: "stale_revision" })),
      "stale_revision",
    );
    assert.equal(
      instructionSetCode(new MeerkatError("-32602", "invented", { instruction_set_code: "invented_code" })),
      undefined,
    );
    assert.equal(instructionSetCode(new MeerkatError("-32602", "plain")), undefined);
    assert.equal(instructionSetCode(null), undefined);
  });

  it("the applied-instructions event is schema-known and keeps its payload", async () => {
    const { KNOWN_AGENT_EVENT_TYPES } = await import("../dist/generated/events.js");
    const raw = {
      type: "operator_instructions_applied",
      session_id: SESSION_ID,
      entry_sequence: 3,
      kind: "delta",
      run_id: RUN_ID,
      active: [
        { namespace: "app.example", key: "tone", revision: 1, content_sha256: DIGEST, body_bytes: 16 },
      ],
      changes: [
        {
          change: "activate",
          namespace: "app.example",
          key: "tone",
          revision: 1,
          content_sha256: DIGEST,
          body_bytes: 16,
        },
      ],
    };
    assert.ok(KNOWN_AGENT_EVENT_TYPES.includes(raw.type), "operator_instructions_applied is known");
    // Schema-known without a handwritten parser case: the raw-preserving path
    // keeps every field, so a kind or change kind this SDK does not know is
    // kept too.
    assert.deepEqual(parseEvent(structuredClone(raw)), raw);
    const future = { ...raw, kind: "replay", changes: [{ change: "suspend" }] };
    assert.deepEqual(parseEvent(structuredClone(future)), future);
  });
});
