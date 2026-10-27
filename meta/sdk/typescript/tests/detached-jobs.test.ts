import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { CallbackDispatcher } from "../src/agent-builder.js";
import {
  DetachedJobExecution,
  DetachedJobResult,
  type DetachedJobContext,
  type DetachedJobHandler,
} from "../src/jobs.js";
import { SessionBuildOptions } from "../src/models.js";

const AUTHORITY_1 = { job_id: "job-1", attempt_id: "attempt-1", fence: 7 };
const AUTHORITY_2 = { job_id: "job-1", attempt_id: "attempt-2", fence: 8 };

function startParams(
  authority = AUTHORITY_1,
  runnerHandle = "callback:job-1:attempt:1",
): Record<string, unknown> {
  return {
    authority: { ...authority },
    runner: { name: "example.security_scan", version: "1" },
    restart_class: "non_resumable",
    runner_handle: runnerHandle,
    runner_specification_ref: "blob-args",
    arguments: { target: "lan" },
    credential_scopes: ["network.read"],
  };
}

async function register(
  dispatcher: CallbackDispatcher,
  runner: DetachedJobHandler,
  profileName = "network",
  scopeId = "build-1",
): Promise<Record<string, unknown>> {
  dispatcher.registerBuilder({
    async buildAgent(options: SessionBuildOptions): Promise<void> {
      options.profileName = profileName;
      options.registerTool("security_scan", () => null, {
        execution: new DetachedJobExecution({
          runner: "example.security_scan",
          version: "1",
          restartClass: "non_resumable",
          idempotencyScope: "interaction_and_arguments",
          submissionTimeoutMs: 30_000,
          credentialScopes: ["network.read"],
          handler: runner,
        }),
      });
    },
  });
  return await dispatcher.handleCallback("callback/build_agent", {
    options: { scope_id: scopeId },
  }) as Record<string, unknown>;
}

describe("detached callback jobs", () => {
  it("serializes the exact private execution declaration", () => {
    const options = new SessionBuildOptions();
    options.profileName = "network";
    options.registerTool("security_scan", () => null, {
      description: "Scan the LAN",
      inputSchema: { type: "object" },
      execution: new DetachedJobExecution({
        runner: "example.security_scan",
        version: "1",
        restartClass: "non_resumable",
        idempotencyScope: "interaction_and_arguments",
        submissionTimeoutMs: 30_000,
        credentialScopes: ["network.read"],
        handler: async () => undefined,
      }),
    });

    assert.deepEqual(options.toDict().tools, [{
      name: "security_scan",
      description: "Scan the LAN",
      input_schema: { type: "object" },
      execution: {
        mode: "detached",
        runner: { name: "example.security_scan", version: "1" },
        restart_class: "non_resumable",
        idempotency_scope: "interaction_and_arguments",
        submission_timeout_ms: 30_000,
        credential_scopes: ["network.read"],
      },
    }]);
  });

  it("returns from start before work and reports exact authority", async () => {
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => {
      release = resolve;
    });
    let entered!: () => void;
    const started = new Promise<void>((resolve) => {
      entered = resolve;
    });
    const calls: Array<[string, Record<string, unknown>]> = [];
    const resolverCalls: Array<[string | null, readonly string[]]> = [];

    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async (method, params) => {
      calls.push([method, params]);
      return { job: { job_id: "job-1" } };
    });
    dispatcher.registerJobCredentialResolver(async (profile, scopes) => {
      resolverCalls.push([profile, scopes]);
      return { token: "secret-current-attempt" };
    });
    await register(dispatcher, async (context: DetachedJobContext) => {
      assert.deepEqual(context.arguments, { target: "lan" });
      assert.deepEqual(context.credentials, { token: "secret-current-attempt" });
      await context.progress(1, "started", { observedAtMs: 101 });
      entered();
      await blocked;
      return new DetachedJobResult("artifact:scan");
    });

    const result = await dispatcher.handleCallback(
      "callback/job/start",
      startParams(),
    );
    assert.deepEqual(result, {
      accepted: true,
      runner_handle: "callback:job-1:attempt:1",
    });
    await started;
    assert.deepEqual(resolverCalls, [["network", ["network.read"]]]);
    assert.deepEqual(calls[0], [
      "meta/jobs/progress",
      {
        authority: AUTHORITY_1,
        cursor: 1,
        detail: "started",
        observed_at_ms: 101,
      },
    ]);

    release();
    await dispatcher.waitForJobTasks();
    assert.equal(calls.at(-1)?.[0], "meta/jobs/complete");
    assert.deepEqual(calls.at(-1)?.[1].authority, AUTHORITY_1);
    assert.equal(calls.at(-1)?.[1].result_ref, "artifact:scan");
    assert.equal(JSON.stringify(calls).includes("secret-current-attempt"), false);
  });

  it("deduplicates an exact start and rejects an older fence after a later claim", async () => {
    const starts: number[] = [];
    const controllers = new Map<number, AbortSignal>();
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async () => null);
    dispatcher.registerJobCredentialResolver(async () => ({}));
    await register(dispatcher, async (context: DetachedJobContext) => {
      starts.push(context.authority.fence);
      controllers.set(context.authority.fence, context.signal);
      await new Promise<void>((resolve) => {
        context.signal.addEventListener("abort", () => resolve(), { once: true });
      });
    });

    const first = await dispatcher.handleCallback("callback/job/start", startParams());
    const duplicate = await dispatcher.handleCallback("callback/job/start", startParams());
    await Promise.resolve();
    assert.deepEqual(first, duplicate);
    assert.deepEqual(starts, [7]);

    await dispatcher.handleCallback(
      "callback/job/start",
      startParams(AUTHORITY_2, "callback:job-1:attempt:2"),
    );
    await Promise.resolve();
    assert.deepEqual(starts, [7, 8]);
    assert.equal(controllers.get(7)?.aborted, true);

    assert.deepEqual(
      await dispatcher.handleCallback("callback/job/start", startParams()),
      { accepted: false, runner_handle: "callback:job-1:attempt:1" },
    );
    await dispatcher.handleCallback("callback/job/cancel", {
      authority: AUTHORITY_2,
    });
    await dispatcher.waitForJobTasks();
  });

  it("serializes concurrent duplicate starts before credential resolution", async () => {
    let releaseResolver!: () => void;
    const resolverBlocked = new Promise<void>((resolve) => {
      releaseResolver = resolve;
    });
    let releaseRunner!: () => void;
    const runnerBlocked = new Promise<void>((resolve) => {
      releaseRunner = resolve;
    });
    let resolverEntered!: () => void;
    const resolverStarted = new Promise<void>((resolve) => {
      resolverEntered = resolve;
    });
    let resolverCalls = 0;
    let starts = 0;

    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async () => null);
    dispatcher.registerJobCredentialResolver(async () => {
      resolverCalls += 1;
      resolverEntered();
      await resolverBlocked;
      return { token: "fresh" };
    });
    await register(dispatcher, async () => {
      starts += 1;
      await runnerBlocked;
    });

    const first = dispatcher.handleCallback("callback/job/start", startParams());
    await resolverStarted;
    const duplicate = dispatcher.handleCallback("callback/job/start", startParams());
    await Promise.resolve();
    releaseResolver();
    const [firstResult, duplicateResult] = await Promise.all([first, duplicate]);
    await Promise.resolve();

    assert.deepEqual(firstResult, duplicateResult);
    assert.equal(resolverCalls, 1);
    assert.equal(starts, 1);
    releaseRunner();
    await dispatcher.waitForJobTasks();
  });

  it("reconciles only exact live or runner-adopted attempts without starting work", async () => {
    let runs = 0;
    const rpcCalls: Array<[string, Record<string, unknown>]> = [];
    const runner = {
      async run(): Promise<void> {
        runs += 1;
        throw new Error("reconcile must not start work");
      },
      async reconcile(attempt: Record<string, unknown>): Promise<boolean> {
        return attempt.runner_handle === "external:live";
      },
      async cancel(): Promise<void> {},
    };
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async (method, params) => {
      rpcCalls.push([method, params]);
      return null;
    });
    await register(dispatcher, runner);

    const result = await dispatcher.handleCallback("callback/job/reconcile", {
      attempts: [
        {
          authority: AUTHORITY_1,
          runner: { name: "example.security_scan", version: "1" },
          restart_class: "adoptable",
          runner_handle: "external:live",
          lease_expires_at_ms: 999,
        },
        {
          authority: {
            job_id: "job-replay",
            attempt_id: "attempt-replay",
            fence: 1,
          },
          runner: { name: "example.security_scan", version: "1" },
          restart_class: "replayable",
          runner_handle: "external:live",
          lease_expires_at_ms: 999,
        },
      ],
    });
    assert.deepEqual(result, { live_attempts: [AUTHORITY_1] });
    assert.equal(runs, 0);
    assert.equal(rpcCalls.at(-1)?.[0], "meta/jobs/heartbeat");
    assert.deepEqual(rpcCalls.at(-1)?.[1].authority, AUTHORITY_1);
    assert.ok(
      Number(rpcCalls.at(-1)?.[1].lease_expires_at_ms) >
        Number(rpcCalls.at(-1)?.[1].heartbeat_at_ms),
    );
    assert.deepEqual(
      await dispatcher.handleCallback("callback/job/cancel", {
        authority: AUTHORITY_2,
      }),
      { accepted: false },
    );
    assert.deepEqual(
      await dispatcher.handleCallback("callback/job/cancel", {
        authority: AUTHORITY_1,
      }),
      { accepted: true },
    );
  });

  it("re-resolves credentials per attempt and rejects runner/profile ambiguity", async () => {
    const resolved: string[] = [];
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async () => null);
    dispatcher.registerJobCredentialResolver(async (profile) => {
      const token = `${profile}-${resolved.length + 1}`;
      resolved.push(token);
      return { token };
    });
    await register(dispatcher, async () => undefined);
    await dispatcher.handleCallback("callback/job/start", startParams());
    await dispatcher.waitForJobTasks();
    await dispatcher.handleCallback(
      "callback/job/start",
      startParams(AUTHORITY_2, "callback:job-1:attempt:2"),
    );
    await dispatcher.waitForJobTasks();
    assert.deepEqual(resolved, ["network-1", "network-2"]);

    await assert.rejects(
      register(dispatcher, async () => undefined, "security", "build-2"),
      /already bound to profile "network"/,
    );
  });

  it("rejects cleanly when scoped credentials cannot resolve", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerJobRpc(async () => null);
    await register(dispatcher, async () => undefined);
    assert.deepEqual(
      await dispatcher.handleCallback("callback/job/start", startParams()),
      {
        accepted: false,
        runner_handle: "callback:job-1:attempt:1",
      },
    );
  });
});
