import assert from "node:assert/strict";
import { it } from "node:test";

import { MetaBuilder } from "../src/builder.js";
import { MetaRuntime } from "../src/runtime.js";

it("uses the canonical job and monitor domain methods", async () => {
  const runtime = new MetaRuntime(new MetaBuilder()._config);
  const calls: Array<[string, Record<string, unknown>]> = [];
  (runtime as unknown as { _rpc: Function })._rpc = async (
    method: string,
    params: Record<string, unknown>,
  ) => {
    calls.push([method, params]);
    return { method };
  };

  await runtime.jobs.get("job-1");
  await runtime.jobs.list({ sessionId: "session-1", limit: 25 });
  await runtime.jobs.cancel("job-1");
  await runtime.jobs.progress("job-1");
  await runtime.jobs.result("job-1");
  await runtime.jobs.artifacts("job-1");
  await runtime.jobs.retry("job-1", 123);
  await runtime.jobs.health();
  await runtime.jobs.subscribe("job-1", {
    subscriptionId: "sub-1",
    sessionId: "session-1",
    delivery: { kind: "event", handling_mode: "steer" },
  });
  await runtime.jobs.unsubscribe("job-1", "sub-1");
  await runtime.monitors.start({
    sessionId: "session-1",
    submissionKey: "monitor:lan",
    command: "./scan --watch",
    timeoutSecs: 600,
    restartClass: "non_resumable",
    delivery: { kind: "notification" },
    protocol: "framed_jsonl",
    workingDir: "/srv/example",
    maxLineBytes: 4096,
  });

  assert.deepEqual(calls, [
    ["jobs/get", { job_id: "job-1" }],
    ["jobs/list", { session_id: "session-1", limit: 25 }],
    ["jobs/cancel", { job_id: "job-1" }],
    ["jobs/progress", { job_id: "job-1" }],
    ["jobs/result", { job_id: "job-1" }],
    ["jobs/artifacts", { job_id: "job-1" }],
    ["jobs/retry", { job_id: "job-1", retry_due_at_ms: 123 }],
    ["jobs/health", {}],
    ["jobs/subscribe", {
      job_id: "job-1",
      subscription_id: "sub-1",
      session_id: "session-1",
      delivery: { kind: "event", handling_mode: "steer" },
    }],
    ["jobs/unsubscribe", { job_id: "job-1", subscription_id: "sub-1" }],
    ["monitors/start", {
      session_id: "session-1",
      submission_key: "monitor:lan",
      command: "./scan --watch",
      timeout_secs: 600,
      protocol: "framed_jsonl",
      restart_class: "non_resumable",
      delivery: { kind: "notification" },
      working_dir: "/srv/example",
      max_line_bytes: 4096,
    }],
  ]);
});
