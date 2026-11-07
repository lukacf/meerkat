// The original requester on Meta callbacks. `callback/call_tool` and
// `callback/job/start` name the original requester of the run they serve,
// typed, as `requester`; the runtime sets it, tool arguments never do. The SDK
// parses it strictly (a malformed value is no requester), passes it to a tool
// handler as its optional second argument, and puts a job's on
// `DetachedJobContext.requester`, so a host can refuse a person-scoped tool for
// anyone but its owner.
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { CallbackDispatcher } from "../src/agent-builder.js";
import { DetachedJobExecution, type DetachedJobContext } from "../src/jobs.js";
import { SessionBuildOptions } from "../src/models.js";
import {
  parseToolCallRequester,
  type RequesterPrincipal,
  type ToolCallRequester,
} from "../src/index.js";

const ISSUER_DOMAIN = "oidc-issuer:https://issuer.example";

const wirePrincipal = (subject: string): Record<string, unknown> => ({
  kind: "human",
  id: subject,
  qualification: { kind: "qualified", trust_domain_id: ISSUER_DOMAIN },
});

const wireRequester = (subject: string, carried = false): Record<string, unknown> =>
  carried
    ? { principal: wirePrincipal(subject), carried: true }
    : { principal: wirePrincipal(subject) };

const person = (subject: string): RequesterPrincipal => ({
  kind: "human",
  id: subject,
  trustDomainId: ISSUER_DOMAIN,
});

async function mailDispatcher(
  owner: RequesterPrincipal,
  seen: Array<[Record<string, unknown>, ToolCallRequester | undefined]>,
): Promise<CallbackDispatcher> {
  const dispatcher = new CallbackDispatcher();
  dispatcher.registerBuilder({
    async buildAgent(options: SessionBuildOptions): Promise<void> {
      options.registerTool("read_mail", async (args, requester) => {
        seen.push([args, requester]);
        if (
          requester === undefined ||
          requester.carried ||
          requester.principal.kind !== owner.kind ||
          requester.principal.id !== owner.id ||
          requester.principal.trustDomainId !== owner.trustDomainId
        ) {
          throw new Error("this mailbox serves its owner only");
        }
        return `${owner.id}'s mail`;
      });
    },
  });
  await dispatcher.handleCallback("callback/build_agent", { options: { scope_id: "s1" } });
  return dispatcher;
}

function call(
  dispatcher: CallbackDispatcher,
  args: Record<string, unknown>,
  requester: Record<string, unknown> | undefined,
): Promise<unknown> {
  const params: Record<string, unknown> = { scope_id: "s1", tool: "read_mail", arguments: args };
  if (requester !== undefined) params.requester = requester;
  return dispatcher.handleCallback("callback/call_tool", params);
}

describe("callback requester", () => {
  it("parses a requester and reads a malformed one as no requester", () => {
    assert.deepEqual(parseToolCallRequester(wireRequester("alice")), {
      principal: person("alice"),
      carried: false,
    });
    assert.deepEqual(parseToolCallRequester(wireRequester("alice", true)), {
      principal: person("alice"),
      carried: true,
    });
    assert.deepEqual(
      parseToolCallRequester({ principal: { kind: "service_account", id: "svc-1" } }),
      { principal: { kind: "service_account", id: "svc-1" }, carried: false },
    );
    for (const raw of [
      undefined,
      null,
      "alice",
      [],
      {},
      { principal: null },
      { principal: { kind: "human" } },
      { principal: { kind: "human", id: "" } },
      { principal: { kind: 7, id: "alice" } },
      { principal: { ...wirePrincipal("alice"), qualification: { kind: "qualified" } } },
      { principal: { ...wirePrincipal("alice"), qualification: "issuer" } },
      { ...wireRequester("alice"), carried: "yes" },
    ]) {
      assert.equal(parseToolCallRequester(raw), undefined, JSON.stringify(raw));
    }
  });

  it("reads every requester the runtime refuses as no requester", () => {
    const qualified = { kind: "qualified", trust_domain_id: ISSUER_DOMAIN };
    const alice = wirePrincipal("alice");
    const refused: Record<string, Record<string, unknown>> = {
      "an unknown qualification kind": { ...alice, qualification: { kind: "maybe" } },
      "an unknown qualification kind with a domain": {
        ...alice,
        qualification: { kind: "maybe", trust_domain_id: ISSUER_DOMAIN },
      },
      "a blank qualification kind": { ...alice, qualification: { kind: "  " } },
      "a null qualification": { ...alice, qualification: null },
      "a trust domain on an unqualified principal": {
        ...alice,
        qualification: { kind: "unqualified", trust_domain_id: ISSUER_DOMAIN },
      },
      "a qualification field the runtime does not have": {
        ...alice,
        qualification: { ...qualified, via: "proxy" },
      },
      "a blank trust domain": {
        ...alice,
        qualification: { kind: "qualified", trust_domain_id: "  " },
      },
      "a trust domain with a control character": {
        ...alice,
        qualification: { kind: "qualified", trust_domain_id: `${ISSUER_DOMAIN}\u0000` },
      },
      "a blank principal kind": { ...alice, kind: "  " },
      "an unknown principal kind": { ...alice, kind: "robot" },
      "a qualified id with a control character": wirePrincipal("ali\u0007ce"),
    };
    const accepted = Object.entries(refused)
      .filter(([, principal]) => parseToolCallRequester({ principal }) !== undefined)
      .map(([name]) => name);
    assert.deepEqual(accepted, [], "the SDK reads a requester the runtime refuses");
  });

  it("lets a host refuse a person-scoped tool for another requester", async () => {
    const seen: Array<[Record<string, unknown>, ToolCallRequester | undefined]> = [];
    const dispatcher = await mailDispatcher(person("alice"), seen);
    const served = await call(dispatcher, {}, wireRequester("alice")).catch(
      (error: unknown) => error,
    );
    assert.deepEqual(served, { content: "alice's mail" }, "the owner's direct call is served");
    for (const requester of [wireRequester("bob"), wireRequester("alice", true), undefined]) {
      await assert.rejects(call(dispatcher, {}, requester), /owner only/);
    }
    assert.deepEqual(
      seen.map(([, requester]) => requester),
      [
        { principal: person("alice"), carried: false },
        { principal: person("bob"), carried: false },
        { principal: person("alice"), carried: true },
        undefined,
      ],
    );
  });

  it("never takes the requester from the arguments", async () => {
    const seen: Array<[Record<string, unknown>, ToolCallRequester | undefined]> = [];
    const dispatcher = await mailDispatcher(person("alice"), seen);
    const forged = { requester: wireRequester("alice") };
    await assert.rejects(call(dispatcher, forged, undefined), /owner only/);
    await assert.rejects(call(dispatcher, forged, wireRequester("bob")), /owner only/);
    assert.deepEqual(seen, [
      [forged, undefined],
      [forged, { principal: person("bob"), carried: false }],
    ]);
  });

  it("puts a detached job's requester on its context", async () => {
    const contexts: DetachedJobContext[] = [];
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async () => ({}));
    dispatcher.registerBuilder({
      async buildAgent(options: SessionBuildOptions): Promise<void> {
        options.profileName = "network";
        options.registerTool("security_scan", () => null, {
          execution: new DetachedJobExecution({
            runner: "example.security_scan",
            version: "1",
            restartClass: "non_resumable",
            idempotencyScope: "interaction_and_arguments",
            submissionTimeoutMs: 30_000,
            handler: async (context: DetachedJobContext) => {
              contexts.push(context);
            },
          }),
        });
      },
    });
    await dispatcher.handleCallback("callback/build_agent", { options: { scope_id: "b1" } });

    for (const [job, requester] of [
      ["job-person", wireRequester("alice")],
      ["job-carried", wireRequester("alice", true)],
      ["job-none", undefined],
    ] as const) {
      const params: Record<string, unknown> = {
        authority: { job_id: job, attempt_id: `${job}-1`, fence: 1 },
        runner: { name: "example.security_scan", version: "1" },
        restart_class: "non_resumable",
        runner_handle: `callback:${job}:attempt:1`,
        arguments: { target: "lan" },
      };
      if (requester !== undefined) params.requester = requester;
      assert.deepEqual(await dispatcher.handleCallback("callback/job/start", params), {
        accepted: true,
        runner_handle: `callback:${job}:attempt:1`,
      });
    }
    await dispatcher.waitForJobTasks();
    assert.deepEqual(
      contexts.map((context) => context.requester),
      [
        { principal: person("alice"), carried: false },
        { principal: person("alice"), carried: true },
        undefined,
      ],
    );
  });
});
