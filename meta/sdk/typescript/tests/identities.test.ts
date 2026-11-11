/**
 * `MetaRuntime.identities`, the calls of the long-term agent family
 * (`meta/identity/*`): each sends its method with exactly the params it
 * names, in the one spelling every surface writes (a create names
 * `agents: [{name, ...}]` flat, a change of one agent names it `name`); each
 * answer is typed; an outcome is one of `done`, `pending_decision`,
 * `pending_preparation` and `refused`; every refusal rejects with the one
 * typed error, whose kinds are a closed vocabulary mirrored from the
 * gateway's; and an advisory value this SDK does not know is kept as the
 * gateway sent it.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  ANNOUNCEMENT_REFUSED_CODE,
  AnnouncementRefusedError,
  ContractMismatchError,
  IDENTITY_REFUSAL_KINDS,
  IDENTITY_REFUSED_CODE,
  IdentityRefusedError,
  RpcError,
  isIdentityRefusalKind,
} from "../src/errors.js";
import {
  AGENT_SEATS,
  IDENTITY_OUTCOMES,
  INSTRUCTION_NEXT_SESSION_REASONS,
  INSTRUCTION_OUTCOME_KINDS,
  parseIdentityOutcome,
} from "../src/identities.js";
import * as sdk from "../src/index.js";
import { runtimeConfig } from "./runtime-config.js";

const DIGEST = `sha256:${"a".repeat(64)}`;
const NEXT_DIGEST = `sha256:${"b".repeat(64)}`;
const OPERATION_ID = `host.${"c".repeat(32)}.req-1`;
const DECISION_ID = `decision:${"d".repeat(64)}`;

type Reply = { result: unknown } | { error: Record<string, unknown> };

async function scripted(replies: Reply[]) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const rt = new MetaRuntime(runtimeConfig());
  const queue = [...replies];
  const sent: Array<{ method: string; params: unknown }> = [];
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (request: Record<string, unknown>) => {
      await Promise.resolve();
      const method = String(request.method);
      sent.push({ method, params: request.params });
      const reply = queue.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return { identities: rt.identities, sent };
}

function summary(name = "scout", overrides: Record<string, unknown> = {}) {
  return {
    name,
    profile: "researcher",
    spec_digest: DIGEST,
    archived: false,
    readiness: "dormant",
    ...overrides,
  };
}

function done(...agents: Record<string, unknown>[]) {
  return { outcome: "done", operation_id: OPERATION_ID, agents };
}

const committed = { result: done({ name: "scout", state: "committed" }) };

function refusal(kind: string, members: Record<string, unknown> = {}): Reply {
  return {
    error: {
      code: IDENTITY_REFUSED_CODE,
      message: `refused: ${kind}`,
      data: { kind, ...members },
    },
  };
}

async function rejection(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  throw new Error("expected a rejection");
}

describe("identity family: request spelling", () => {
  it("adopt, get, list and operation send their params", async () => {
    const { identities, sent } = await scripted([
      committed,
      { result: summary("scout", { instructions: [], labels: {}, peers: [] }) },
      { result: { agents: [] } },
      { result: { agents: [] } },
      committed,
      committed,
    ]);
    await identities.adopt("scout", { expectedSpecDigest: DIGEST, requestId: "req-1" });
    await identities.get("scout");
    await identities.list();
    await identities.list({ profile: "researcher", includeArchived: true, after: "scout" });
    await identities.operation({ requestId: "req-1" });
    await identities.operation({ operationId: OPERATION_ID });
    assert.deepEqual(sent, [
      {
        method: "meta/identity/adopt",
        params: { request_id: "req-1", name: "scout", expected_spec_digest: DIGEST },
      },
      { method: "meta/identity/get", params: { name: "scout" } },
      { method: "meta/identity/list", params: { include_archived: false } },
      {
        method: "meta/identity/list",
        params: { profile: "researcher", include_archived: true, after: "scout" },
      },
      { method: "meta/identity/operation", params: { request_id: "req-1" } },
      { method: "meta/identity/operation", params: { operation_id: OPERATION_ID } },
    ]);
  });

  it("a create names its agents flat, by name", async () => {
    const { identities, sent } = await scripted([committed]);
    await identities.create(
      [
        {
          name: "scout",
          profile: "researcher",
          displayName: "Scout",
          instructions: ["Find sources.", "Cite each one."],
          context: { team: "research" },
          labels: { area: "research" },
          firstMessage: "Introduce yourself.",
          peers: ["editor"],
        },
        { name: "editor", profile: "writer" },
      ],
      {
        requestId: "req-create",
        proposer: "planner",
        sourceEvidence: {
          attestor: "host",
          subject: "ticket-7",
          sourceRef: "tracker:7",
          observedAtMs: 1_900_000_000_000,
        },
      },
    );
    assert.deepEqual(sent, [
      {
        method: "meta/identity/create",
        params: {
          request_id: "req-create",
          agents: [
            {
              name: "scout",
              profile: "researcher",
              display_name: "Scout",
              instructions: ["Find sources.", "Cite each one."],
              context: { team: "research" },
              labels: { area: "research" },
              first_message: "Introduce yourself.",
              peers: ["editor"],
            },
            { name: "editor", profile: "writer" },
          ],
          proposer: "planner",
          source_evidence: {
            attestor: "host",
            subject: "ticket-7",
            source_ref: "tracker:7",
            observed_at_ms: 1_900_000_000_000,
          },
        },
      },
    ]);
  });

  it("an update names the agent, what it sets and its peers", async () => {
    const { identities, sent } = await scripted([committed, committed]);
    await identities.update("scout", {
      expectedSpecDigest: DIGEST,
      requestId: "req-update",
      set: {
        displayName: "Scout II",
        instructions: ["Find primary sources."],
        context: { team: "research" },
        labels: { area: "archives" },
        labelsRemove: ["trial"],
        profile: "senior-researcher",
      },
      peersAdd: ["librarian"],
      peersRemove: ["editor"],
    });
    await identities.update("scout", { expectedSpecDigest: DIGEST, requestId: "req-noop" });
    assert.deepEqual(sent, [
      {
        method: "meta/identity/update",
        params: {
          request_id: "req-update",
          name: "scout",
          expected_spec_digest: DIGEST,
          set: {
            display_name: "Scout II",
            instructions: ["Find primary sources."],
            context: { team: "research" },
            labels: { area: "archives" },
            labels_remove: ["trial"],
            profile: "senior-researcher",
          },
          peers_add: ["librarian"],
          peers_remove: ["editor"],
        },
      },
      {
        method: "meta/identity/update",
        params: {
          request_id: "req-noop",
          name: "scout",
          expected_spec_digest: DIGEST,
          set: {},
        },
      },
    ]);
  });

  for (const change of ["archive", "unarchive", "remove", "adopt"] as const) {
    it(`${change} names the agent and the spec it read`, async () => {
      const { identities, sent } = await scripted([committed, committed]);
      await identities[change]("scout", { expectedSpecDigest: DIGEST, requestId: "req-1" });
      await identities[change]("scout", {
        expectedSpecDigest: DIGEST,
        requestId: "req-2",
        proposer: "planner",
        sourceEvidence: { attestor: "host", subject: "s", sourceRef: "r", observedAtMs: 7 },
      });
      assert.deepEqual(sent, [
        {
          method: `meta/identity/${change}`,
          params: { request_id: "req-1", name: "scout", expected_spec_digest: DIGEST },
        },
        {
          method: `meta/identity/${change}`,
          params: {
            request_id: "req-2",
            name: "scout",
            expected_spec_digest: DIGEST,
            proposer: "planner",
            source_evidence: {
              attestor: "host",
              subject: "s",
              source_ref: "r",
              observed_at_ms: 7,
            },
          },
        },
      ]);
    });
  }

  it("withdraw, changesHold and directory send their params", async () => {
    const withdrawn = { result: { operation_id: OPERATION_ID, decision_id: DECISION_ID } };
    const held = { result: { held: true } };
    const directory = { result: { profiles: [], peers: [] } };
    const { identities, sent } = await scripted([
      withdrawn,
      withdrawn,
      held,
      held,
      directory,
      directory,
    ]);
    await identities.withdraw({ operationId: OPERATION_ID });
    await identities.withdraw({ requestId: "req-1" });
    await identities.changesHold(true, { reason: "deploy window" });
    await identities.changesHold(false);
    await identities.directory();
    await identities.directory({ query: "res", after: "researcher" });
    assert.deepEqual(sent, [
      { method: "meta/identity/withdraw", params: { operation_id: OPERATION_ID } },
      { method: "meta/identity/withdraw", params: { request_id: "req-1" } },
      {
        method: "meta/identity/changes_hold",
        params: { held: true, reason: "deploy window" },
      },
      { method: "meta/identity/changes_hold", params: { held: false } },
      { method: "meta/identity/directory", params: {} },
      { method: "meta/identity/directory", params: { query: "res", after: "researcher" } },
    ]);
  });

  it("plan sends the change it plans", async () => {
    const plan = {
      result: {
        outcome: "require_approval",
        change: "adopt",
        approver_id: "team:owners",
        timeout_ms: 600_000,
        request_digest: DIGEST,
        subject_digest: NEXT_DIGEST,
        rendering: "adopt scout",
      },
    };
    const { identities, sent } = await scripted([plan, plan, plan]);
    await identities.plan("adopt", {
      name: "scout",
      expectedSpecDigest: DIGEST,
      requestId: "req-1",
    });
    await identities.plan("create", {
      agents: [{ name: "scout", profile: "researcher" }],
      requestId: "req-2",
    });
    await identities.plan("update", {
      name: "scout",
      expectedSpecDigest: DIGEST,
      requestId: "req-3",
    });
    assert.deepEqual(sent, [
      {
        method: "meta/identity/plan",
        params: {
          change: "adopt",
          request_id: "req-1",
          name: "scout",
          expected_spec_digest: DIGEST,
        },
      },
      {
        method: "meta/identity/plan",
        params: {
          change: "create",
          request_id: "req-2",
          agents: [{ name: "scout", profile: "researcher" }],
        },
      },
      {
        method: "meta/identity/plan",
        params: {
          change: "update",
          request_id: "req-3",
          name: "scout",
          expected_spec_digest: DIGEST,
          set: {},
        },
      },
    ]);
  });

  it("a call that cannot be spelled sends nothing", async () => {
    const { identities, sent } = await scripted([]);
    const untyped = identities as unknown as Record<
      string,
      (...args: unknown[]) => Promise<unknown>
    >;
    for (const call of [
      () => untyped.operation({}),
      () => untyped.operation({ requestId: "req-1", operationId: OPERATION_ID }),
      () => untyped.withdraw({}),
      () => untyped.withdraw({ requestId: "req-1", operationId: OPERATION_ID }),
      () =>
        untyped.plan("rename", { name: "scout", expectedSpecDigest: DIGEST, requestId: "r" }),
      () => untyped.adopt("scout", { expectedSpecDigest: DIGEST }),
      () => untyped.adopt("scout", { expectedSpecDigest: DIGEST, requestId: " " }),
      () => untyped.create([{ name: "scout", profile: "researcher" }], {}),
    ]) {
      assert.ok((await rejection(call())) instanceof TypeError);
    }
    assert.deepEqual(sent, []);
  });
});

describe("identity family: typed answers", () => {
  it("identity family round trip", async () => {
    const { identities } = await scripted([
      {
        result: summary("scout", {
          display_name: "Scout",
          spec_revision: 3,
          provenance: {
            entered: "adopted",
            requester_kind: "host",
            requester: "rpc",
            at_ms: 1_900_000_000_000,
            consent: { form: "allowed" },
          },
          open_operations: [OPERATION_ID],
          instructions: ["Find sources."],
          context: { team: "research" },
          labels: { area: "research" },
          peers: ["editor"],
        }),
      },
      {
        result: {
          agents: [summary(), summary("editor", { archived: true, readiness: "unseated" })],
          next: "editor",
        },
      },
      {
        result: done({
          name: "scout",
          state: "committed",
          spec_digest: NEXT_DIGEST,
          spec_revision: 1,
          session_id: "session-1",
        }),
      },
      { result: done({ name: "scout", state: "committed", spec_digest: NEXT_DIGEST }) },
    ]);

    const read = await identities.get("scout");
    assert.deepEqual(read, {
      name: "scout",
      profile: "researcher",
      specDigest: DIGEST,
      archived: false,
      readiness: "dormant",
      displayName: "Scout",
      specRevision: 3,
      provenance: {
        entered: "adopted",
        requesterKind: "host",
        requester: "rpc",
        atMs: 1_900_000_000_000,
        consent: { form: "allowed" },
        consentForm: "allowed",
      },
      openOperations: [OPERATION_ID],
      instructions: ["Find sources."],
      context: { team: "research" },
      labels: { area: "research" },
      peers: ["editor"],
    });

    const page = await identities.list();
    assert.deepEqual(page, {
      agents: [
        {
          name: "scout",
          profile: "researcher",
          specDigest: DIGEST,
          archived: false,
          readiness: "dormant",
          displayName: null,
          specRevision: null,
          provenance: null,
          openOperations: [],
        },
        {
          name: "editor",
          profile: "researcher",
          specDigest: DIGEST,
          archived: true,
          readiness: "unseated",
          displayName: null,
          specRevision: null,
          provenance: null,
          openOperations: [],
        },
      ],
      next: "editor",
    });

    const adopted = await identities.adopt("scout", {
      expectedSpecDigest: read.specDigest,
      requestId: "req-1",
    });
    assert.deepEqual(adopted, {
      outcome: "done",
      operationId: OPERATION_ID,
      agents: [
        {
          name: "scout",
          state: "committed",
          specDigest: NEXT_DIGEST,
          specRevision: 1,
          sessionId: "session-1",
          seat: null,
          instructions: null,
        },
      ],
    });

    const again = await identities.operation({ operationId: adopted.operationId });
    assert.equal(again.outcome, "done");
    assert.ok(again.outcome === "done" && again.agents[0].specDigest === NEXT_DIGEST);
  });

  it("each outcome is parsed to its type", () => {
    const seated = parseIdentityOutcome({
      outcome: "done",
      operation_id: OPERATION_ID,
      agents: [
        { name: "scout", state: "committed", seat: "seated" },
        { name: "editor", state: "committed", seat: "owed" },
      ],
    });
    assert.ok(seated.outcome === "done");
    assert.deepEqual(
      seated.agents.map((agent) => [agent.name, agent.seat]),
      [
        ["scout", "seated"],
        ["editor", "owed"],
      ],
    );

    assert.deepEqual(
      parseIdentityOutcome({
        outcome: "pending_decision",
        operation_id: OPERATION_ID,
        decision_id: DECISION_ID,
        deadline_ms: 1_900_000_600_000,
      }),
      {
        outcome: "pending_decision",
        operationId: OPERATION_ID,
        decisionId: DECISION_ID,
        deadlineMs: 1_900_000_600_000,
      },
    );

    const preparing = parseIdentityOutcome({
      outcome: "pending_preparation",
      operation_id: OPERATION_ID,
      agents: [{ name: "scout", state: "pending_preparation" }],
    });
    assert.ok(preparing.outcome === "pending_preparation");
    assert.equal(preparing.operationId, OPERATION_ID);
    assert.equal(preparing.agents[0].state, "pending_preparation");

    assert.deepEqual(
      parseIdentityOutcome({
        outcome: "refused",
        kind: "stale_spec",
        field: "expected_spec_digest",
        retryable: false,
        detail: "The agent changed since you read it.",
        current_digest: NEXT_DIGEST,
        current_revision: 4,
      }),
      {
        outcome: "refused",
        refusal: {
          kind: "stale_spec",
          knownKind: true,
          field: "expected_spec_digest",
          detail: "The agent changed since you read it.",
          retryable: false,
          details: { current_digest: NEXT_DIGEST, current_revision: 4 },
        },
      },
    );

    const cases: Array<[string, string | null]> = [
      ["applies_at_next_run", null],
      ["applies_after_compaction", null],
      ["unchanged", null],
      ["applies_at_next_session", "prepared_authorization_required"],
      ["applies_at_next_session", "slot_limit_exceeded"],
      ["applies_at_next_session", "active_set_budget_exceeded"],
      ["applies_at_next_session", "lowering_unavailable"],
      ["applies_at_next_session", "body_too_large"],
      ["applies_at_next_session", "record_unavailable"],
    ];
    for (const [kind, reason] of cases) {
      const instructions: Record<string, unknown> = { kind };
      if (reason !== null) instructions.reason = reason;
      const updated = parseIdentityOutcome({
        outcome: "done",
        operation_id: OPERATION_ID,
        agents: [{ name: "scout", state: "committed", instructions }],
      });
      assert.ok(updated.outcome === "done");
      assert.deepEqual(updated.agents[0].instructions, { kind, reason });
    }
  });

  it("an answer that is no outcome is a contract mismatch", () => {
    for (const answer of [
      null,
      [],
      {},
      { outcome: "reviewed", operation_id: OPERATION_ID },
      { outcome: "done", agents: [] },
      { outcome: "done", operation_id: OPERATION_ID },
      { outcome: "pending_preparation", operation_id: OPERATION_ID },
      { outcome: "done", operation_id: OPERATION_ID, agents: [{ state: "committed" }] },
      { outcome: "pending_decision", operation_id: OPERATION_ID },
      {
        outcome: "pending_decision",
        operation_id: OPERATION_ID,
        decision_id: DECISION_ID,
        deadline_ms: true,
      },
      { outcome: "refused" },
    ]) {
      assert.throws(() => parseIdentityOutcome(answer), ContractMismatchError);
    }
  });

  it("a held decision is an answer, and the operation is read later", async () => {
    const held = {
      result: {
        outcome: "pending_decision",
        operation_id: OPERATION_ID,
        decision_id: DECISION_ID,
        deadline_ms: 1_900_000_600_000,
      },
    };
    const { identities, sent } = await scripted([
      held,
      held,
      { result: { outcome: "unknown", operation_id: OPERATION_ID } },
      {
        result: {
          outcome: "refused",
          kind: "decision_closed",
          state: "denied",
          retryable: false,
          detail: "The decision has ended: ask again.",
        },
      },
      { result: { outcome: "unknown" } },
    ]);
    const outcome = await identities.adopt("scout", {
      expectedSpecDigest: DIGEST,
      requestId: "req-1",
    });
    assert.ok(outcome.outcome === "pending_decision");
    assert.equal(outcome.decisionId, DECISION_ID);
    const later = await identities.operation({ operationId: outcome.operationId });
    assert.deepEqual(later, outcome);
    assert.deepEqual(sent[1], {
      method: "meta/identity/operation",
      params: { operation_id: OPERATION_ID },
    });

    assert.deepEqual(await identities.operation({ requestId: "req-never-asked" }), {
      outcome: "unknown",
      operationId: OPERATION_ID,
    });

    // A refusal an operation ended with is what the read answers: the read
    // itself succeeded, so nothing rejects.
    const ended = await identities.operation({ operationId: OPERATION_ID });
    assert.ok(ended.outcome === "refused");
    assert.equal(ended.refusal.kind, "decision_closed");
    assert.deepEqual(ended.refusal.details, { state: "denied" });

    assert.ok(
      (await rejection(identities.operation({ requestId: "req-2" }))) instanceof
        ContractMismatchError,
    );
  });

  it("withdraw, changesHold, directory and plan answers are typed", async () => {
    const { identities } = await scripted([
      { result: { operation_id: OPERATION_ID, decision_id: DECISION_ID } },
      { result: { held: true } },
      {
        result: {
          profiles: [
            { profile: "researcher", verdict: "allowed" },
            {
              profile: "operator",
              verdict: "needs_decision",
              lines: ["An operator can run shell commands."],
            },
            { profile: "admin", verdict: "not_allowed", reason: "host only" },
          ],
          peers: [
            { name: "editor", display_name: "Editor", profile: "writer", verdict: "not_allowed" },
          ],
          next: "editor",
        },
      },
      {
        result: {
          outcome: "require_approval",
          change: "adopt",
          approver_id: "team:owners",
          timeout_ms: 600_000,
          request_digest: DIGEST,
          subject_digest: NEXT_DIGEST,
          rendering: "adopt scout",
        },
      },
    ]);
    assert.deepEqual(await identities.withdraw({ operationId: OPERATION_ID }), {
      operationId: OPERATION_ID,
      decisionId: DECISION_ID,
    });
    assert.deepEqual(await identities.changesHold(true), { held: true });
    assert.deepEqual(await identities.directory(), {
      profiles: [
        { profile: "researcher", verdict: "allowed", lines: [], reason: null },
        {
          profile: "operator",
          verdict: "needs_decision",
          lines: ["An operator can run shell commands."],
          reason: null,
        },
        { profile: "admin", verdict: "not_allowed", lines: [], reason: "host only" },
      ],
      peers: [
        {
          name: "editor",
          profile: "writer",
          displayName: "Editor",
          verdict: "not_allowed",
          lines: [],
          reason: null,
        },
      ],
      next: "editor",
    });
    assert.deepEqual(
      await identities.plan("adopt", {
        name: "scout",
        expectedSpecDigest: DIGEST,
        requestId: "r",
      }),
      {
        outcome: "require_approval",
        change: "adopt",
        approverId: "team:owners",
        timeoutMs: 600_000,
        requestDigest: DIGEST,
        subjectDigest: NEXT_DIGEST,
        rendering: "adopt scout",
      },
    );
  });
});

describe("identity family: one refusal error, a closed vocabulary", () => {
  it("every refusal kind rejects with the one typed error", async () => {
    assert.equal(IDENTITY_REFUSED_CODE, -32058);
    assert.equal(IDENTITY_REFUSAL_KINDS.size, 57);
    const kinds = [...IDENTITY_REFUSAL_KINDS].sort();
    const { identities } = await scripted(
      kinds.map((kind) =>
        refusal(kind, {
          field: "agents[1].name",
          retryable: false,
          detail: "What was wrong, and what to send instead.",
        }),
      ),
    );
    for (const kind of kinds) {
      const error = await rejection(
        identities.create([{ name: "scout", profile: "researcher" }], { requestId: "r" }),
      );
      assert.ok(error instanceof IdentityRefusedError, kind);
      assert.ok(error instanceof RpcError);
      assert.equal(error.name, "IdentityRefusedError");
      assert.equal(error.code, IDENTITY_REFUSED_CODE);
      assert.equal(error.kind, kind);
      assert.equal(error.knownKind, true);
      assert.ok(isIdentityRefusalKind(error.kind));
      assert.equal(error.field, "agents[1].name");
      assert.equal(error.detail, "What was wrong, and what to send instead.");
      assert.equal(error.retryable, false);
      assert.deepEqual(error.details, {});
      assert.equal(error.refusal.kind, kind);
      assert.equal(error.method, "meta/identity/create");
    }
  });

  it("a refusal carries its kind's details and whether it is retryable", async () => {
    const { identities } = await scripted([
      refusal("stale_spec", {
        field: "expected_spec_digest",
        retryable: false,
        detail: "The agent changed since you read it.",
        current_digest: NEXT_DIGEST,
        current_revision: 4,
      }),
      refusal("changes_held", { retryable: true, detail: "Retry later." }),
      refusal("owner_read_only"),
      refusal("invalid_request", {
        retryable: false,
        detail: "What was wrong, and what to send instead.",
        problem: "malformed",
      }),
      refusal("invalid_request", {
        field: "request_id",
        retryable: false,
        detail: "A change names its operation: send a request_id.",
        problem: "empty",
      }),
    ]);
    const options = { expectedSpecDigest: DIGEST, requestId: "r" };

    const stale = await rejection(identities.update("scout", options));
    assert.ok(stale instanceof IdentityRefusedError);
    assert.deepEqual(stale.details, { current_digest: NEXT_DIGEST, current_revision: 4 });
    assert.equal(stale.retryable, false);

    const held = await rejection(identities.adopt("scout", options));
    assert.ok(held instanceof IdentityRefusedError);
    assert.equal(held.retryable, true);
    assert.equal(held.field, null);

    // A gateway that sends neither: the detail is the error's message, and
    // whether a retry may succeed is not known.
    const bare = await rejection(identities.update("scout", options));
    assert.ok(bare instanceof IdentityRefusedError);
    assert.equal(bare.detail, "refused: owner_read_only");
    assert.equal(bare.retryable, null);

    // Params the gateway cannot read are the same error, never -32602:
    // `malformed` with no field for an unknown member or a wrong type,
    // `empty` at `request_id` for a change without its request id.
    const malformed = await rejection(identities.adopt("scout", options));
    assert.ok(malformed instanceof IdentityRefusedError);
    assert.equal(malformed.kind, "invalid_request");
    assert.equal(malformed.field, null);
    assert.deepEqual(malformed.details, { problem: "malformed" });
    const unnamed = await rejection(identities.adopt("scout", options));
    assert.ok(unnamed instanceof IdentityRefusedError);
    assert.equal(unnamed.kind, "invalid_request");
    assert.equal(unnamed.field, "request_id");
    assert.deepEqual(unnamed.details, { problem: "empty" });
  });

  it("only the family's code with a kind is an identity refusal", async () => {
    const { identities } = await scripted([
      { error: { code: -32602, message: "Invalid params", data: { kind: "name_taken" } } },
      { error: { code: IDENTITY_REFUSED_CODE, message: "refused" } },
      { error: { code: IDENTITY_REFUSED_CODE, message: "refused", data: "name_taken" } },
      { error: { code: IDENTITY_REFUSED_CODE, message: "refused", data: {} } },
      { error: { code: IDENTITY_REFUSED_CODE, message: "refused", data: { kind: 7 } } },
      {
        error: {
          code: ANNOUNCEMENT_REFUSED_CODE,
          message: "meta/identity/adopt is refused: this gateway is a candidate launch",
          data: { kind: "candidate_launch" },
        },
      },
    ]);
    for (let index = 0; index < 5; index += 1) {
      const error = await rejection(identities.get("scout"));
      assert.ok(error instanceof RpcError);
      assert.ok(!(error instanceof IdentityRefusedError), `reply ${index}`);
    }
    // A candidate launch refuses every writer before the family is reached,
    // with the gateway-wide code: it keeps the class the SDK gives that code.
    const candidate = await rejection(
      identities.adopt("scout", { expectedSpecDigest: DIGEST, requestId: "r" }),
    );
    assert.ok(candidate instanceof AnnouncementRefusedError);
    assert.ok(!(candidate instanceof IdentityRefusedError));
    assert.equal(candidate.kind, "candidate_launch");
  });

  it("a kind newer than this SDK is still the typed error", async () => {
    const { identities } = await scripted([
      refusal("quota_exceeded", { field: "agents", retryable: true, quota: 3 }),
    ]);
    const error = await rejection(
      identities.create([{ name: "scout", profile: "researcher" }], { requestId: "r" }),
    );
    assert.ok(error instanceof IdentityRefusedError);
    assert.equal(error.kind, "quota_exceeded");
    assert.ok(!IDENTITY_REFUSAL_KINDS.has(error.kind));
    assert.equal(error.knownKind, false);
    assert.ok(!isIdentityRefusalKind(error.kind));
    assert.equal(error.field, "agents");
    assert.equal(error.retryable, true);
    assert.deepEqual(error.details, { quota: 3 });
  });

  it("a refused change rejects with the typed error however it is carried", async () => {
    const carried = {
      kind: "not_adoptable",
      field: "name",
      retryable: false,
      detail: "An adoption does not take this agent, for the reason given.",
      reason: "nothing_to_adopt",
    };
    const { identities } = await scripted([
      refusal("not_adoptable", carried),
      { result: { outcome: "refused", ...carried } },
      refusal("identity_unknown", { field: "name", retryable: false, detail: "No agent." }),
    ]);
    const options = { expectedSpecDigest: DIGEST, requestId: "r" };

    const asError = await rejection(identities.adopt("scout", options));
    assert.ok(asError instanceof IdentityRefusedError);
    assert.equal(asError.kind, "not_adoptable");
    assert.equal(asError.field, "name");
    assert.deepEqual(asError.details, { reason: "nothing_to_adopt" });

    // The same refusal carried as the change's outcome is the same error.
    const asOutcome = await rejection(identities.adopt("scout", options));
    assert.ok(asOutcome instanceof IdentityRefusedError);
    assert.equal(asOutcome.kind, "not_adoptable");
    assert.equal(asOutcome.field, "name");
    assert.equal(asOutcome.retryable, false);
    assert.deepEqual(asOutcome.details, { reason: "nothing_to_adopt" });
    assert.equal(asOutcome.method, "meta/identity/adopt");
    assert.equal(
      asOutcome.detail,
      "An adoption does not take this agent, for the reason given.",
    );

    // A read's refusal is the same error.
    const read = await rejection(identities.get("nobody"));
    assert.ok(read instanceof IdentityRefusedError);
    assert.equal(read.kind, "identity_unknown");
  });
});

function rust(name: string): string {
  return readFileSync(
    new URL(`../../../crates/meerkat-meta/src/identity_creation/${name}`, import.meta.url),
    "utf-8",
  );
}

/**
 * The wire tokens of a closed vocabulary, as the family lists them once:
 * `closed_tokens!(<typeName> { Variant => "token", ... });`.
 */
function closedTokens(source: string, typeName: string): string[] {
  const listed = source.split(`closed_tokens!(${typeName} {`);
  assert.equal(listed.length, 2, `closed_tokens!(${typeName}) is not listed exactly once`);
  const body = listed[1].split("});")[0];
  const tokens = [...body.matchAll(/^\s*[A-Z][A-Za-z]+ => "([a-z_]+)",$/gm)].map(
    (match) => match[1],
  );
  assert.ok(tokens.length > 0, `closed_tokens!(${typeName}) lists no token`);
  assert.equal(new Set(tokens).size, tokens.length, `${typeName}: a token is listed twice`);
  return tokens;
}

describe("identity family: the closed vocabularies are the gateway's", () => {
  it("the refusal kinds are the gateway's closed vocabulary", () => {
    const tokens = closedTokens(rust("refusal.rs"), "IdentityRefusalKind");
    assert.equal(tokens.length, 57);
    assert.equal(new Set(tokens).size, 57);
    assert.ok(tokens.includes("owner_read_only"));
    assert.deepEqual(
      [...IDENTITY_REFUSAL_KINDS].sort(),
      [...tokens].sort(),
      "IDENTITY_REFUSAL_KINDS and IdentityRefusalKind::token differ: a kind is added or " +
        "removed in both, and the change is Breaking",
    );
  });

  it("the outcome, instruction and seat vocabularies are the gateway's", () => {
    const outcome = rust("outcome.rs");
    assert.deepEqual(
      [...IDENTITY_OUTCOMES].sort(),
      closedTokens(outcome, "IdentityOutcome").sort(),
    );
    assert.deepEqual(
      [...IDENTITY_OUTCOMES].sort(),
      ["done", "pending_decision", "pending_preparation", "refused"],
    );
    assert.deepEqual(
      [...INSTRUCTION_OUTCOME_KINDS].sort(),
      closedTokens(outcome, "InstructionOutcome").sort(),
    );
    assert.equal(INSTRUCTION_OUTCOME_KINDS.length, 4);
    assert.deepEqual(
      [...INSTRUCTION_NEXT_SESSION_REASONS].sort(),
      closedTokens(outcome, "InstructionNextSessionReason").sort(),
    );
    assert.equal(INSTRUCTION_NEXT_SESSION_REASONS.length, 6);
    assert.deepEqual([...AGENT_SEATS].sort(), closedTokens(outcome, "AgentSeat").sort());
    assert.deepEqual([...AGENT_SEATS].sort(), ["owed", "seated"]);
  });
});

describe("identity family: tolerance", () => {
  it("an unknown advisory value is kept raw", async () => {
    const { identities } = await scripted([
      {
        result: {
          agents: [
            summary("scout", {
              readiness: "hibernating",
              provenance: {
                entered: "forked",
                requester_kind: "service",
                requester: "importer",
                at_ms: 5,
                consent: { form: "reviewed", reviewer: "editor" },
              },
              badge: "new",
            }),
          ],
          total: 1,
        },
      },
      {
        result: {
          profiles: [{ profile: "researcher", verdict: "needs_review", reviewer: "editor" }],
          peers: [],
        },
      },
    ]);
    const page = await identities.list();
    const agent = page.agents[0];
    assert.equal(agent.readiness, "hibernating");
    assert.equal(agent.provenance?.entered, "forked");
    assert.equal(agent.provenance?.requesterKind, "service");
    assert.deepEqual(agent.provenance?.consent, { form: "reviewed", reviewer: "editor" });
    assert.equal(agent.provenance?.consentForm, "reviewed");

    const directory = await identities.directory();
    assert.deepEqual(directory.profiles[0], {
      profile: "researcher",
      verdict: "needs_review",
      lines: [],
      reason: null,
    });

    const outcome = parseIdentityOutcome({
      outcome: "done",
      operation_id: OPERATION_ID,
      agents: [{ name: "scout", state: "hibernated", note: "x" }],
      notice: "kept",
    });
    assert.ok(outcome.outcome === "done");
    assert.equal(outcome.agents[0].state, "hibernated");
  });

  it("a closed value newer than this SDK is kept as its token", () => {
    // A committed change stays readable: a seat or an instruction outcome
    // this SDK does not know never turns a `done` answer into a throw. The
    // vocabulary test is what fails.
    const outcome = parseIdentityOutcome({
      outcome: "done",
      operation_id: OPERATION_ID,
      agents: [
        {
          name: "scout",
          state: "committed",
          seat: "reserved",
          instructions: { kind: "applies_at_next_wake", reason: "asleep" },
        },
      ],
    });
    assert.ok(outcome.outcome === "done");
    const agent = outcome.agents[0];
    assert.equal(agent.seat, "reserved");
    assert.ok(!(AGENT_SEATS as readonly string[]).includes(agent.seat ?? ""));
    assert.deepEqual(agent.instructions, { kind: "applies_at_next_wake", reason: "asleep" });
  });
});

describe("identity family: the launch policy", () => {
  const hostMayChangeAgents = () => ({
    enabled: true,
    approval: {
      rules: [
        {
          change: [
            "create",
            "descriptive",
            "capability",
            "peers",
            "profile",
            "archive",
            "remove",
            "adopt",
          ],
          requester: { kind: "host", surface: "rpc" },
          outcome: { kind: "allow" },
        },
      ],
    },
  });
  const runtimeOptions = async (builder: unknown): Promise<Record<string, unknown>> => {
    const { MetaRuntime } = await import("../src/runtime.js");
    const config = (builder as { _config: unknown })._config;
    const rt = new MetaRuntime(config as ConstructorParameters<typeof MetaRuntime>[0]);
    return (rt as unknown as { _buildInitParams(): { runtime_options: Record<string, unknown> } })
      ._buildInitParams().runtime_options;
  };

  it("the builder sends the launch policy as given", async () => {
    // Without a policy every change of the family is refused `change_denied`;
    // the builder is how an SDK host launches with one.
    assert.equal("identity_creation" in (await runtimeOptions(sdk.Meta.builder())), false);

    const policy = hostMayChangeAgents();
    const builder = sdk.Meta.builder();
    assert.equal(builder.identityCreation(policy), builder);
    assert.deepEqual((await runtimeOptions(builder)).identity_creation, hostMayChangeAgents());
    // The builder keeps its own copy: a later edit of the caller's object
    // does not change what the launch sends.
    policy.approval.rules.length = 0;
    policy.enabled = false;
    assert.deepEqual((await runtimeOptions(builder)).identity_creation, hostMayChangeAgents());

    assert.throws(
      () => (sdk.Meta.builder() as unknown as { identityCreation(p: unknown): unknown })
        .identityCreation(true),
      TypeError,
    );
  });
});

describe("identity family: one agent model", () => {
  it("no answer names where an agent's record lives", async () => {
    const banned = ["custody", "origin", "storeOwned", "intentManaged", "declared", "createdBy"];
    const { identities } = await scripted([
      {
        result: summary("scout", {
          custody: "store",
          origin: "created",
          store_owned: true,
          intent_managed: false,
          instructions: [],
          labels: {},
          peers: [],
        }),
      },
      { result: done({ name: "scout", state: "committed", origin: "created" }) },
    ]);
    const record = await identities.get("scout");
    const outcome = await identities.adopt("scout", {
      expectedSpecDigest: DIGEST,
      requestId: "r",
    });
    const keys = [
      ...Object.keys(record),
      ...(outcome.outcome === "done" ? Object.keys(outcome.agents[0]) : []),
    ];
    for (const key of keys) {
      assert.ok(!banned.some((word) => key.includes(word)), key);
    }
  });

  it("the family is exported from the package", () => {
    for (const name of [
      "IdentitiesHandle",
      "IdentityRefusedError",
      "IDENTITY_REFUSAL_KINDS",
      "IDENTITY_REFUSED_CODE",
      "isIdentityRefusalKind",
      "parseIdentityRefusal",
      "IDENTITY_OUTCOMES",
      "INSTRUCTION_OUTCOME_KINDS",
      "INSTRUCTION_NEXT_SESSION_REASONS",
      "AGENT_SEATS",
      "IDENTITY_CHANGES",
      "parseIdentityOutcome",
    ]) {
      assert.ok(name in sdk, name);
    }
  });
});
