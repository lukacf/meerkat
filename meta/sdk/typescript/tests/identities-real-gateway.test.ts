/**
 * The long-term agent calls against a real gateway.
 *
 * `identities.test.ts` pins the SDK's reading of the wire on a scripted
 * transport. These rows run the same calls through the shipping path,
 * TypeScript SDK -> rpc_gateway -> Meta, so the SDK and the host methods
 * cannot drift apart unnoticed: the request types refuse an unknown member,
 * so each call that answers here is spelled as the gateway reads it.
 *
 * They run when META_RPC_GATEWAY_BIN names a built rpc_gateway, and make no
 * model call: an agent a create commits is dormant, no row sends a first
 * message, and no row delivers anything.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  IDENTITY_REFUSAL_KINDS,
  INSTRUCTION_OUTCOME_KINDS,
  IdentityRefusedError,
  Meta,
} from "../src/index.js";
import type { IdentitiesHandle } from "../src/index.js";

const gatewayBin = process.env.META_RPC_GATEWAY_BIN;
const skip = !gatewayBin && "META_RPC_GATEWAY_BIN is not set";

const MOB_TOML = `[mob]
id = "identity-family-typescript"

[profiles.researcher]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.researcher.tools]
comms = true

[profiles.writer]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.writer.tools]
comms = true
`;

// The host on the stdio surface may make every change a rule may allow
// without a person (an authority change always needs one).
const HOST_MAY_CHANGE_AGENTS = {
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
  // Which peers an agent may list; the default is deny.
  peers: { default: "allow" },
};

// The agents the host's configuration declares: one, `scout`.
const roster = {
  async roster() {
    return [
      {
        identity: "scout",
        profile: "researcher",
        addressability: "addressable",
        displayName: "Scout",
        labels: {},
        context: null,
        additionalInstructions: ["Find sources."],
        runtimeModeOverride: "turn_driven",
      },
    ];
  },
};

async function withGateway(
  run: (identities: IdentitiesHandle, rpc: (m: string, p: unknown) => Promise<unknown>) => Promise<void>,
  policy: Record<string, unknown> | null = HOST_MAY_CHANGE_AGENTS,
): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), "meta-identities-"));
  const mobToml = join(dir, "mob.toml");
  writeFileSync(mobToml, MOB_TOML);
  let builder = Meta.builder()
    .gateway(gatewayBin as string)
    .mob(mobToml)
    .persistentState(join(dir, "state"))
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    .rosterProvider(roster as any);
  if (policy !== null) builder = builder.identityCreation(policy);
  const runtime = await builder.build();
  try {
    await run(runtime.identities, (method, params) =>
      runtime._rpc(method, params as Record<string, unknown>),
    );
  } finally {
    await runtime.shutdown();
    rmSync(dir, { recursive: true, force: true });
  }
}

async function refusedBy(promise: Promise<unknown>): Promise<IdentityRefusedError> {
  try {
    await promise;
  } catch (error) {
    assert.ok(error instanceof IdentityRefusedError, String(error));
    return error;
  }
  throw new Error("expected a refusal");
}

/** Create `name` and return its spec digest. */
async function created(identities: IdentitiesHandle, name = "editor"): Promise<string> {
  const outcome = await identities.create(
    [{ name, profile: "writer", instructions: ["Edit for clarity."] }],
    { requestId: `create-${name}` },
  );
  assert.ok(outcome.outcome === "done", JSON.stringify(outcome));
  assert.deepEqual(outcome.agents.map((agent) => agent.name), [name]);
  const digest = outcome.agents[0].specDigest;
  assert.ok(digest !== null && digest.startsWith("sha256:"));
  return digest;
}

describe("identity family on a real gateway", () => {
  it("list names every agent the same way", { skip }, async () => {
    await withGateway(async (identities) => {
      await created(identities);
      const page = await identities.list();
      const byName = new Map(page.agents.map((agent) => [agent.name, agent]));
      assert.deepEqual([...byName.keys()].sort(), ["editor", "scout"]);
      for (const agent of byName.values()) {
        assert.ok(agent.specDigest.startsWith("sha256:"));
        assert.equal(agent.archived, false);
        assert.equal(typeof agent.readiness, "string");
      }
      assert.equal(byName.get("scout")?.profile, "researcher");
      assert.equal(byName.get("editor")?.profile, "writer");
      const only = await identities.list({ profile: "writer" });
      assert.deepEqual(only.agents.map((agent) => agent.name), ["editor"]);
    });
  });

  it("an agent without a display name has none", { skip }, async () => {
    // The wire leaves `display_name` out for an agent that has none: it is
    // never the agent's name in its place.
    await withGateway(async (identities) => {
      await created(identities);
      assert.equal((await identities.get("editor")).displayName, null);
      const listed = new Map((await identities.list()).agents.map((agent) => [agent.name, agent]));
      assert.equal(listed.get("editor")?.displayName, null);
      assert.equal(listed.get("scout")?.displayName, "Scout");
      const peers = new Map((await identities.directory()).peers.map((peer) => [peer.name, peer]));
      assert.equal(peers.get("editor")?.displayName, null);
      assert.equal(peers.get("scout")?.displayName, "Scout");
    });
  });

  it("get reads one agent in full", { skip }, async () => {
    await withGateway(async (identities) => {
      const record = await identities.get("scout");
      assert.equal(record.name, "scout");
      assert.equal(record.profile, "researcher");
      assert.equal(record.displayName, "Scout");
      assert.deepEqual(record.instructions, ["Find sources."]);
      const listed = (await identities.list()).agents.find((agent) => agent.name === "scout");
      assert.equal(listed?.specDigest, record.specDigest);

      const refused = await refusedBy(identities.get("nobody"));
      assert.equal(refused.kind, "identity_unknown");
      assert.equal(refused.knownKind, true);
    });
  });

  it("adopt releases a configured agent, and replays", { skip }, async () => {
    await withGateway(async (identities) => {
      const record = await identities.get("scout");
      const digest = record.specDigest;

      // Declared by configuration: a run-time change is refused until it is adopted.
      const before = await refusedBy(
        identities.update("scout", {
          expectedSpecDigest: digest,
          requestId: "update-before-adopt",
          set: { displayName: "Scout II" },
        }),
      );
      assert.equal(before.kind, "owner_read_only");

      const adopted = await identities.adopt("scout", {
        expectedSpecDigest: digest,
        requestId: "adopt-scout",
      });
      assert.ok(adopted.outcome === "done", JSON.stringify(adopted));
      assert.deepEqual(adopted.agents.map((agent) => agent.name), ["scout"]);
      // One spec digest on every surface; an adoption does not change it.
      assert.equal(adopted.agents[0].specDigest, digest);
      assert.equal((await identities.get("scout")).specDigest, digest);

      // The same id and request return the same state.
      assert.deepEqual(
        await identities.adopt("scout", { expectedSpecDigest: digest, requestId: "adopt-scout" }),
        adopted,
      );

      // From now on it is changed at run time.
      const updated = await identities.update("scout", {
        expectedSpecDigest: digest,
        requestId: "update-after-adopt",
        set: { displayName: "Scout II" },
      });
      assert.equal(updated.outcome, "done");
      assert.equal((await identities.get("scout")).displayName, "Scout II");
    });
  });

  it("operation reads a change by either name", { skip }, async () => {
    await withGateway(async (identities) => {
      const record = await identities.get("scout");
      const adopted = await identities.adopt("scout", {
        expectedSpecDigest: record.specDigest,
        requestId: "adopt-scout",
      });
      assert.ok(adopted.outcome === "done");
      assert.deepEqual(await identities.operation({ requestId: "adopt-scout" }), adopted);
      assert.deepEqual(
        await identities.operation({ operationId: adopted.operationId }),
        adopted,
      );
      const unknown = await identities.operation({ requestId: "never-asked" });
      assert.ok(unknown.outcome === "unknown");
      assert.ok(unknown.operationId.length > 0);
    });
  });

  it("create commits a dormant agent", { skip }, async () => {
    await withGateway(async (identities) => {
      const outcome = await identities.create(
        [
          {
            name: "editor",
            profile: "writer",
            displayName: "Editor",
            instructions: ["Edit for clarity."],
            context: { desk: "features" },
            labels: { area: "features" },
            peers: ["scout"],
          },
          { name: "proofreader", profile: "writer", peers: ["editor"] },
        ],
        { requestId: "create-desk" },
      );
      assert.ok(outcome.outcome === "done", JSON.stringify(outcome));
      assert.deepEqual(outcome.agents.map((agent) => agent.name), ["editor", "proofreader"]);
      for (const agent of outcome.agents) {
        assert.equal(agent.seat, "seated");
        assert.ok(agent.specDigest?.startsWith("sha256:"));
      }
      const record = await identities.get("editor");
      assert.equal(record.displayName, "Editor");
      assert.deepEqual(record.instructions, ["Edit for clarity."]);
      assert.deepEqual(record.context, { desk: "features" });
      assert.equal(record.labels.area, "features");
      assert.deepEqual(record.peers, ["scout"]);
      assert.equal(record.specDigest, outcome.agents[0].specDigest);

      // A name is taken once: by an agent of the mob, or by another agent
      // of the same create. Either way the refusal names the agent of the
      // request whose name it is.
      const taken = await refusedBy(
        identities.create([{ name: "editor", profile: "writer" }], { requestId: "create-again" }),
      );
      assert.equal(taken.kind, "name_taken");
      assert.equal(taken.field, "agents[0].name");
      assert.equal(taken.details.by, "identity");
      const twins = await refusedBy(
        identities.create(
          [
            { name: "twin", profile: "writer" },
            { name: "twin", profile: "writer" },
          ],
          { requestId: "create-twins" },
        ),
      );
      assert.equal(twins.kind, "name_taken");
      assert.equal(twins.field, "agents[1].name");
    });
  });

  it("update changes an agent and says when instructions apply", { skip }, async () => {
    await withGateway(async (identities) => {
      const digest = await created(identities);
      const updated = await identities.update("editor", {
        expectedSpecDigest: digest,
        requestId: "update-editor",
        set: {
          displayName: "Chief editor",
          instructions: ["Edit for clarity.", "Keep the author's voice."],
          labels: { area: "features" },
        },
      });
      assert.ok(updated.outcome === "done", JSON.stringify(updated));
      const agent = updated.agents[0];
      assert.notEqual(agent.specDigest, digest);
      // What became of the instructions change is on the outcome when the
      // launch carries instruction changes to its agents; it is one of the
      // closed kinds then.
      if (agent.instructions !== null) {
        assert.ok(
          (INSTRUCTION_OUTCOME_KINDS as readonly string[]).includes(agent.instructions.kind),
        );
      }
      const record = await identities.get("editor");
      assert.equal(record.displayName, "Chief editor");
      assert.deepEqual(record.instructions, ["Edit for clarity.", "Keep the author's voice."]);
      assert.equal(record.labels.area, "features");
      assert.equal(record.specDigest, agent.specDigest);

      // The spec the caller read is no longer the current one.
      const stale = await refusedBy(
        identities.update("editor", {
          expectedSpecDigest: digest,
          requestId: "update-stale",
          set: { displayName: "Editor" },
        }),
      );
      assert.equal(stale.kind, "stale_spec");
      assert.equal(stale.details.current_digest, agent.specDigest);
    });
  });

  it("archive and unarchive keep the agent", { skip }, async () => {
    await withGateway(async (identities) => {
      const digest = await created(identities);
      const early = await refusedBy(
        identities.unarchive("editor", { expectedSpecDigest: digest, requestId: "unarchive-early" }),
      );
      assert.equal(early.kind, "not_archived");

      const archived = await identities.archive("editor", {
        expectedSpecDigest: digest,
        requestId: "archive-editor",
      });
      assert.equal(archived.outcome, "done");
      assert.ok(!(await identities.list()).agents.some((agent) => agent.name === "editor"));
      const all = await identities.list({ includeArchived: true });
      assert.equal(all.agents.find((agent) => agent.name === "editor")?.archived, true);

      const restored = await identities.unarchive("editor", {
        expectedSpecDigest: digest,
        requestId: "unarchive-editor",
      });
      assert.equal(restored.outcome, "done");
      assert.equal((await identities.get("editor")).archived, false);
    });
  });

  it("remove retires the name for good", { skip }, async () => {
    await withGateway(async (identities) => {
      const digest = await created(identities);
      const removed = await identities.remove("editor", {
        expectedSpecDigest: digest,
        requestId: "remove-editor",
      });
      assert.equal(removed.outcome, "done");
      const all = await identities.list({ includeArchived: true });
      assert.ok(!all.agents.some((agent) => agent.name === "editor"));
      const reuse = await refusedBy(
        identities.create([{ name: "editor", profile: "writer" }], { requestId: "create-reuse" }),
      );
      assert.equal(reuse.kind, "name_taken");
      assert.equal(reuse.field, "agents[0].name");
      assert.equal(reuse.details.by, "removed");
    });
  });

  it("changesHold refuses every change until released", { skip }, async () => {
    await withGateway(async (identities) => {
      assert.deepEqual(await identities.changesHold(true, { reason: "deploy" }), { held: true });
      const agents = [{ name: "editor", profile: "writer" }];
      const held = await refusedBy(identities.create(agents, { requestId: "create-held" }));
      assert.equal(held.kind, "changes_held");
      assert.equal(held.retryable, true);
      assert.deepEqual(await identities.changesHold(false), { held: false });
      // The same request, unchanged, commits once the hold is released.
      const outcome = await identities.create(agents, { requestId: "create-held" });
      assert.equal(outcome.outcome, "done");
    });
  });

  it("withdraw refuses an operation no decision is held for", { skip }, async () => {
    await withGateway(async (identities) => {
      const refused = await refusedBy(identities.withdraw({ requestId: "never-asked" }));
      assert.equal(refused.kind, "decision_not_found");
    });
  });

  it("directory lists what a change may name", { skip }, async () => {
    await withGateway(async (identities) => {
      const directory = await identities.directory();
      assert.deepEqual(
        directory.profiles.map((entry) => entry.profile).sort(),
        ["researcher", "writer"],
      );
      assert.ok(directory.profiles.every((entry) => entry.verdict === "allowed"));
      assert.ok(directory.peers.some((peer) => peer.name === "scout"));
      const narrowed = await identities.directory({ query: "writ" });
      assert.deepEqual(narrowed.profiles.map((entry) => entry.profile), ["writer"]);
    });
  });

  it("plan answers the outcome and writes nothing", { skip }, async () => {
    await withGateway(async (identities) => {
      const record = await identities.get("scout");
      const plan = await identities.plan("adopt", {
        name: "scout",
        expectedSpecDigest: record.specDigest,
        requestId: "plan-adopt",
      });
      assert.equal(plan.outcome, "allow");
      assert.equal(plan.change, "adopt");
      assert.ok(plan.requestDigest.startsWith("sha256:"));
      assert.ok(plan.subjectDigest.startsWith("sha256:"));
      assert.ok(plan.rendering.includes("scout"));
      // Nothing was written: the operation is unknown, and the agent is
      // still declared by configuration.
      assert.equal((await identities.operation({ requestId: "plan-adopt" })).outcome, "unknown");
      const still = await refusedBy(
        identities.update("scout", {
          expectedSpecDigest: record.specDigest,
          requestId: "update-after-plan",
          set: { displayName: "Scout II" },
        }),
      );
      assert.equal(still.kind, "owner_read_only");
    });
  });

  it("a policy the gateway cannot read refuses the launch", { skip }, async () => {
    // The SDK sends the policy unchanged: the gateway owns its shape, and a
    // launch with a policy it cannot read fails at init, naming where.
    const allowByHost = { requester: { kind: "host", surface: "rpc" }, outcome: { kind: "allow" } };
    const cases: Array<[Record<string, unknown>, string]> = [
      // A rule names the changes it governs.
      [
        { enabled: true, approval: { rules: [allowByHost] } },
        "identity_creation.approval.rules[0].change",
      ],
      // An authority change always needs a person's decision.
      [
        {
          enabled: true,
          approval: { rules: [{ change: ["authority"], outcome: { kind: "allow" } }] },
        },
        "identity_creation.approval.rules[0]",
      ],
      // A member the policy does not have.
      [{ enabled: true, custody: "store" }, "identity_creation"],
    ];
    for (const [policy, where] of cases) {
      let failure: unknown;
      try {
        await withGateway(async () => {}, policy);
      } catch (error) {
        failure = error;
      }
      assert.ok(failure instanceof Error, JSON.stringify(policy));
      const startup = (failure as { startupError?: unknown }).startupError ?? failure;
      assert.ok(String((startup as Error).message).includes(where), String(startup));
    }
  });

  // The family's reads answer; a change, and the plan of one, is refused
  // `change_denied` and writes nothing.
  async function readsAnswerAndEveryChangeIsDenied(identities: IdentitiesHandle): Promise<void> {
    const record = await identities.get("scout");
    assert.ok(record.specDigest.startsWith("sha256:"));
    assert.ok((await identities.list()).agents.some((agent) => agent.name === "scout"));
    assert.ok(Array.isArray((await identities.directory()).profiles));
    const request = {
      name: "scout",
      expectedSpecDigest: record.specDigest,
      requestId: "adopt-scout",
    };
    for (const call of [
      () => identities.adopt("scout", request),
      () => identities.plan("adopt", request),
      () =>
        identities.create([{ name: "editor", profile: "writer" }], { requestId: "create-editor" }),
    ]) {
      const refused = await refusedBy(call());
      assert.equal(refused.kind, "change_denied");
      assert.ok(IDENTITY_REFUSAL_KINDS.has(refused.kind));
      assert.equal(refused.retryable, false);
    }
    assert.equal((await identities.operation({ requestId: "adopt-scout" })).outcome, "unknown");
    assert.deepEqual(
      (await identities.list()).agents.map((agent) => agent.name),
      ["scout"],
    );
  }

  it("a launch without the policy refuses every change", { skip }, async () => {
    // Never method not found: on a launch that gives no policy the reads
    // answer and every change is the family's refusal.
    await withGateway(readsAnswerAndEveryChangeIsDenied, null);
  });

  it("a disabled policy refuses every change", { skip }, async () => {
    await withGateway(readsAnswerAndEveryChangeIsDenied, { enabled: false });
  });

  it("a policy without a matching rule denies the change", { skip }, async () => {
    // An enabled policy governs the changes its rules name; a change no
    // rule matches gets the default, deny.
    const adoptOnly = {
      enabled: true,
      approval: {
        rules: [
          {
            change: ["adopt"],
            requester: { kind: "host", surface: "rpc" },
            outcome: { kind: "allow" },
          },
        ],
      },
    };
    await withGateway(async (identities) => {
      const record = await identities.get("scout");
      const denied = await refusedBy(
        identities.create([{ name: "editor", profile: "writer" }], { requestId: "create-editor" }),
      );
      assert.equal(denied.kind, "change_denied");
      assert.equal(denied.details.change, "create");
      const adopted = await identities.adopt("scout", {
        expectedSpecDigest: record.specDigest,
        requestId: "adopt-scout",
      });
      assert.equal(adopted.outcome, "done");
    }, adoptOnly);
  });

  it("params the gateway cannot read are the family's refusal", { skip }, async () => {
    await withGateway(async (_identities, rpc) => {
      const digest = `sha256:${"0".repeat(64)}`;
      const cases: Array<[Record<string, unknown>, string | null, string]> = [
        // A change names its operation.
        [{ name: "scout", expected_spec_digest: digest }, "request_id", "empty"],
        // A member is missing.
        [{ request_id: "r", name: "scout" }, null, "malformed"],
        // A member the request does not have.
        [
          { request_id: "r", name: "scout", expected_spec_digest: digest, custody: "store" },
          null,
          "malformed",
        ],
      ];
      for (const [params, field, problem] of cases) {
        const refused = await refusedBy(rpc("meta/identity/adopt", params));
        assert.equal(refused.kind, "invalid_request", JSON.stringify(params));
        assert.equal(refused.details.problem, problem, JSON.stringify(params));
        assert.equal(refused.field, field, JSON.stringify(params));
      }
    });
  });
});
