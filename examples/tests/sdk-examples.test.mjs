import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, join, resolve } from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import Ajv from "ajv";

const exec = promisify(execFile);
// The per-run timeout is a hang detector, not a speed check. A run normally
// ends within seconds, but a busy runner has stretched the first runs of a job
// past 15 s, the old limit. Only a run that never ends should reach this.
const RUN_TIMEOUT_MS = 60_000;
// The hang check below passes this through the same parameter, so it
// exercises the kill without waiting a minute.
const HANG_PROBE_TIMEOUT_MS = 3_000;
const examples = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(examples, "..");
const work = join(examples, ".work", `sdk-tests-${randomUUID()}`);
mkdirSync(work, { recursive: true });
after(() => rmSync(work, { recursive: true, force: true }));
const version = JSON.parse(readFileSync(join(repo, "sdks/typescript/package.json"))).version;
const typescriptExamples = [
  "003-hello-meerkat-ts", "008-structured-output-ts",
  "023-rpc-ide-integration-ts", "027-skills-v21-invoke-ts",
];
const pythonExamples = [
  "002-hello-meerkat-py", "007-multi-turn-sessions-py",
  "021-multi-provider-routing-py", "026-skills-v21-invoke-py",
];
const env = {
  PATH: process.env.PATH,
  HOME: work,
  XDG_CONFIG_HOME: work,
  XDG_DATA_HOME: work,
  TSX_DISABLE_CACHE: "1",
  PYTHONDONTWRITEBYTECODE: "1",
  PYTHONPATH: join(repo, "sdks/python"),
};

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

async function execute(command, args, extraEnv, timeout = RUN_TIMEOUT_MS) {
  try {
    const output = await exec(command, args, {
      cwd: work, env: { ...env, ...extraEnv }, timeout,
    });
    return { ...output, code: 0, killed: false, timeout };
  } catch (error) {
    if (typeof error.code !== "number" && !error.killed) throw error;
    error.timeout = timeout;
    return error;
  }
}

async function runExample(example, mode = "success", extraEnv = {}, timeout = RUN_TIMEOUT_MS) {
  const log = join(work, `${example}-${mode}-${randomUUID()}.jsonl`);
  const ts = example.endsWith("-ts");
  const result = await execute(
    ts ? process.execPath : "python3",
    ts
      ? ["--import", join(examples, "node_modules/tsx/dist/loader.mjs"),
          join(examples, example, "main.ts")]
      : [join(examples, example, "main.py")],
    {
      ...extraEnv,
      EXAMPLE_TEST_LOG: log,
      EXAMPLE_TEST_MODE: mode,
      EXAMPLE_TEST_VERSION: version,
      MEERKAT_BIN_PATH: mode === "missing"
        ? join(work, "nonexistent-rpc")
        : join(examples, "tests/rpc_fixture.py"),
    },
    timeout,
  );
  const records = existsSync(log)
    ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line))
    : [];
  const pids = records.filter((record) => record.pid).map((record) => record.pid);
  const requests = records.filter((record) => record.method);
  // Cleanup is a safety net, never a passing outcome: the example must reap its peer.
  for (let attempt = 0; attempt < 40 && pids.some(alive); attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  const surviving = pids.filter(alive);
  for (const pid of surviving) process.kill(pid, "SIGKILL");
  assert.equal(result.killed, false, `${example}/${mode} timed out after the requests `
    + `[${requests.map((request) => request.method).join(", ")}]: ${result.stderr}`);
  assert.deepEqual(surviving, [], `${example}/${mode} leaked its child`);
  if (mode !== "missing") assert.equal(pids.length, 1, "must execute the real fixture");
  return { ...result, records, requests };
}

function successful(result) {
  assert.equal(result.code, 0, result.stderr);
  assert.equal(result.stderr, "");
  assert.equal(result.timeout, RUN_TIMEOUT_MS, "a run gets the hang detector's timeout unless told otherwise");
}

describe("TypeScript SDK entrypoint lifecycle", { concurrency: 4 }, () => {
  for (const example of typescriptExamples) {
    for (const [mode, diagnostic] of [
      ["missing", /BINARY_NOT_FOUND/],
      ["create-error", /Synthetic session refusal/],
      ["mismatch", /VERSION_MISMATCH/],
      ["exit-before-reply", /PROCESS_EXITED/],
    ]) {
      it(`${example}: ${mode} fails nonzero and leaves no child`, async () => {
        const result = await runExample(example, mode);
        assert.notEqual(result.code, 0);
        assert.match(result.stderr, diagnostic);
        if (mode === "mismatch") {
          assert.deepEqual(result.requests.map((r) => r.method), ["initialize"]);
          assert.ok(result.records.some((r) => r.eof || r.exit_signal));
        }
      });
    }
    it(`${example}: successful protocol flow`, async () => {
      const result = await runExample(example);
      successful(result);
      assert.deepEqual(result.requests.slice(0, 2).map((r) => r.method),
        ["initialize", "capabilities/get"]);
      const creates = result.requests.filter((r) => r.method === "session/create");
      assert.equal(creates.length, example.startsWith("008") ? 4 : 1);
      assert.ok(creates.every((r) => r.params.model === "claude-sonnet-5-5"));
      if (example.startsWith("008")) {
        assert.equal(result.stdout.match(/Confidence:  50%/g)?.length, 4);
        const ajv = new Ajv();
        for (const request of creates) {
          const schema = request.params.output_schema;
          assert.equal(schema.properties.confidence.minimum, 0);
          assert.equal(schema.properties.confidence.maximum, 1);
          assert.equal(request.params.structured_output_retries, 3);
          const validate = ajv.compile(schema);
          for (const confidence of [-0.1, 1.1, 0, 0.5, 1]) {
            assert.equal(validate({
              sentiment: "positive", confidence,
              key_phrases: ["synthetic"], summary: "Synthetic sentiment",
            }), confidence >= 0 && confidence <= 1, `confidence=${confidence}`);
          }
        }
      } else {
        assert.match(result.stdout, /Synthetic answer/);
      }
      if (example.startsWith("023")) {
        assert.deepEqual(result.requests.slice(2).map((r) => r.method),
          ["config/get", "session/create", "turn/start", "turn/start",
            "session/list", "session/read", "session/archive"]);
        assert.ok(result.records[0].args.includes("--isolated"));
        assert.match(result.stdout, /Archived\./);
      }
      if (example.startsWith("027")) checkSkill(result);
    });
  }
});

function checkSkill(result) {
  const turn = result.requests.find((r) => r.method === "turn/start");
  assert.deepEqual(turn.params.skill_refs, [{
    kind: "structured",
    source_uuid: "00000000-0000-4b11-8111-000000000002",
    skill_name: "shell-patterns",
  }]);
  for (const flag of ["--isolated", "--context-root", "--state-root", "--user-config-root"]) {
    assert.ok(result.records[0].args.includes(flag), flag);
  }
  assert.match(result.stdout, /Skill: 00000000-0000-4b11-8111-000000000002\/shell-patterns/);
}

describe("Python SDK example controls", { concurrency: 4 }, () => {
  for (const example of pythonExamples) {
    it(`${example}: successful protocol flow`, async () => {
      const result = await runExample(example, "success", {
        ANTHROPIC_API_KEY: "synthetic-not-a-key",
        OPENAI_API_KEY: "synthetic-not-a-key",
        GEMINI_API_KEY: "synthetic-not-a-key",
      });
      successful(result);
      assert.match(result.stdout, /Synthetic answer/);
      if (example.startsWith("007")) {
        assert.match(result.stdout, /Synthetic stream/);
        assert.match(result.stdout, /archived\./);
        assert.deepEqual(result.requests.slice(2).map((r) => r.method),
          ["session/create", "turn/start", "turn/start", "turn/start",
            "session/read", "session/list", "session/archive"]);
      }
      if (example.startsWith("021")) {
        const creates = result.requests.filter((r) => r.method === "session/create");
        assert.deepEqual(creates.map((r) => r.params.model),
          ["claude-sonnet-5-5", "gpt-6-astra", "gemini-4-hydrogen",
            "claude-sonnet-5-5", "gpt-6-astra", "gemini-4-hydrogen", "claude-sonnet-5-5"]);
        // One provider-neutral level for all three; no provider-specific effort.
        for (const create of creates.slice(3, 6)) {
          assert.equal(create.params.reasoning_effort, "high");
          assert.equal(create.params.provider_params, undefined);
        }
        // The provider-specific setting is its own request and carries no level.
        assert.deepEqual(creates[6].params.provider_params,
          { provider_tag: { provider: "anthropic", thinking: { type: "adaptive" } } });
        assert.equal(creates[6].params.reasoning_effort, undefined);
        assert.equal(result.stdout.match(/Sent to synthetic as high/g)?.length, 3);
        assert.match(result.stdout, /Comparison \(3 providers\)/);
      }
      if (example.startsWith("026")) checkSkill(result);
    });
  }
  it("021 retains intentional per-provider comparison error handling", async () => {
    const result = await runExample("021-multi-provider-routing-py", "comparison-error", {
      ANTHROPIC_API_KEY: "synthetic-not-a-key",
      OPENAI_API_KEY: "synthetic-not-a-key",
      GEMINI_API_KEY: "synthetic-not-a-key",
    });
    successful(result);
    assert.equal(result.stdout.match(/Error: .*Synthetic session refusal/g)?.length, 3);
    assert.match(result.stdout, /Routing Strategies/);
  });
  it("021 reports a refused reasoning effort level per provider and carries on", async () => {
    const result = await runExample("021-multi-provider-routing-py", "effort-refusal", {
      ANTHROPIC_API_KEY: "synthetic-not-a-key",
      OPENAI_API_KEY: "synthetic-not-a-key",
      GEMINI_API_KEY: "synthetic-not-a-key",
    });
    successful(result);
    assert.equal(
      result.stdout.match(/Refused: unsupported_level; this model accepts: low, medium/g)?.length, 3);
    assert.match(result.stdout, /Anthropic with adaptive thinking/);
    assert.match(result.stdout, /Routing Strategies/);
  });
  for (const [mode, line] of [
    ["effort-refusal-no-level", /Refused: unsupported_level; this model takes no reasoning-effort level/g],
    ["effort-refusal-unknown", /Refused: supported_levels_unknown; the levels this model accepts are not known$/gm],
    ["effort-refusal-unavailable", /Refused: supported_levels_unavailable; the levels this model accepts are not known; try again later/g],
  ]) {
    it(`021 tells an empty list of levels from an unknown one (${mode})`, async () => {
      const result = await runExample("021-multi-provider-routing-py", mode, {
        ANTHROPIC_API_KEY: "synthetic-not-a-key",
        OPENAI_API_KEY: "synthetic-not-a-key",
        GEMINI_API_KEY: "synthetic-not-a-key",
      });
      successful(result);
      assert.equal(result.stdout.match(line)?.length, 3);
      assert.doesNotMatch(result.stdout, /this model accepts:/);
      assert.match(result.stdout, /Routing Strategies/);
    });
  }
  it("021 reports an error that is not a refusal as an error and carries on", async () => {
    const result = await runExample("021-multi-provider-routing-py", "effort-error", {
      ANTHROPIC_API_KEY: "synthetic-not-a-key",
      OPENAI_API_KEY: "synthetic-not-a-key",
      GEMINI_API_KEY: "synthetic-not-a-key",
    });
    successful(result);
    assert.equal(
      result.stdout.match(/Error \(MeerkatError, code -32000\): Synthetic provider failure/g)?.length, 3);
    assert.doesNotMatch(result.stdout, /Refused:/);
    assert.match(result.stdout, /Anthropic with adaptive thinking/);
  });
  it("021 with no provider keys prints setup guidance without creating sessions", async () => {
    const result = await runExample("021-multi-provider-routing-py");
    successful(result);
    assert.match(result.stdout, /Set at least one API key/);
    assert.ok(result.requests.every((r) => r.method !== "session/create"));
  });
});

describe("Mob control examples", { concurrency: 2 }, () => {
  const mobExamples = ["038-mob-control-py", "039-mob-control-ts"];
  for (const example of mobExamples) {
    it(`${example}: drives the synthetic mob, stops one run and cancels the rest`, async () => {
      const result = await runExample(example, "success",
        { ANTHROPIC_API_KEY: "synthetic-not-a-key" });
      successful(result);
      const requests = result.requests.slice(2);
      // Status reads are polls; everything else has one order.
      assert.deepEqual(
        requests.map((r) => r.method).filter((method) => method !== "mob/member_status"),
        ["mob/create", "mob/spawn", "mob/spawn", "mob/wire", "mob/wait_kickoff",
          "mob/members", "mob/member_history", "mob/stream_open",
          "mob/member_send",
          "mob/member_send", "mob/stop_member_run", "mob/stop_member_run",
          "mob/member_send", "mob/member_send", "mob/member_send", "mob/cancel_all_work",
          "mob/member_send", "mob/stream_close", "mob/destroy"]);
      assert.ok(requests.some((r) => r.method === "mob/member_status"));

      const { profiles } = requests[0].params.definition;
      assert.deepEqual(Object.keys(profiles), ["editor", "researcher"]);
      for (const profile of Object.values(profiles)) {
        assert.equal(profile.model, "claude-sonnet-5-5");
        assert.equal(profile.tools.comms, true);
      }
      // One entry point: only the editor takes work from outside the mob,
      // and every send goes to it through the mob, never to its session.
      assert.equal(profiles.editor.external_addressable, true);
      assert.equal(profiles.researcher.external_addressable, undefined);
      const sends = requests.filter((r) => r.method === "mob/member_send");
      assert.ok(sends.every((r) => r.params.agent_identity === "editor-1"));
      assert.ok(result.requests.every((r) => !/^(turn|session)\//.test(r.method)));
      for (const spawn of requests.filter((r) => r.method === "mob/spawn")) {
        assert.match(spawn.params.initial_message, /^You are the /);
      }

      // The stop names the run that the long job opened (the second run the
      // fixture started), and the second stop repeats that id.
      const stops = requests.filter((r) => r.method === "mob/stop_member_run");
      assert.deepEqual(stops.map((r) => r.params.run_id),
        Array(2).fill("019d1234-0000-7000-8000-000000002002"));
      assert.ok(stops.every((r) => r.params.agent_identity === "editor-1" && r.params.reason));
      // The cancel names the member by the reference its spawn returned, with
      // two sends waiting behind the run it ends.
      const cancel = requests.find((r) => r.method === "mob/cancel_all_work");
      assert.deepEqual(cancel.params, { member_ref: "synthetic-ref-editor-1" });
      assert.deepEqual(result.records.filter((r) => "cancelled_queued" in r),
        [{ cancelled_queued: 2 }]);

      assert.match(result.stdout, /system prompt: \d+ characters, skills \[.mob-communication.\]/);
      assert.match(result.stdout, /stop: stopped, inputs ended: \[\{/);
      assert.match(result.stdout, /the same stop again: not_current/);
      assert.equal(result.stdout.match(/run ended: cancelled/g)?.length, 2);
      assert.match(result.stdout, /editor-1 answered: .Synthetic answer./);
      assert.match(result.stdout, /Mob destroyed\./);
    });
    it(`${example}: a step that fails after the create still destroys the mob`, async () => {
      const result = await runExample(example, "send-error",
        { ANTHROPIC_API_KEY: "synthetic-not-a-key" });
      assert.notEqual(result.code, 0);
      assert.match(result.stderr, /Synthetic send refusal/);
      // The stream the example opened is closed, then the mob is destroyed.
      assert.deepEqual(result.requests.slice(-3).map((r) => r.method),
        ["mob/member_send", "mob/stream_close", "mob/destroy"]);
      assert.match(result.stdout, /Mob destroyed\./);
    });
    it(`${example}: a refused mob fails nonzero and leaves no child`, async () => {
      const result = await runExample(example, "create-error",
        { ANTHROPIC_API_KEY: "synthetic-not-a-key" });
      assert.notEqual(result.code, 0);
      assert.match(result.stderr, /Synthetic mob refusal/);
      assert.equal(result.requests.at(-1).method, "mob/create");
    });
  }
});

describe("022 REST subprocess", () => {
  for (const mode of ["success", "http-503", "connection-refused"]) {
    it(`${mode} has a truthful exit status and diagnostic`, async () => {
      const requests = [];
      const server = createServer(async (request, response) => {
        let body = "";
        for await (const chunk of request) body += chunk;
        requests.push({ method: request.method, path: request.url,
          body: body ? JSON.parse(body) : null });
        response.writeHead(mode === "http-503" ? 503 : 200,
          { "Content-Type": "application/json" });
        response.end(JSON.stringify(mode === "http-503"
          ? { error: "Synthetic service unavailable" }
          : request.method === "POST"
            ? { session_id: "synthetic-rest-session", text: "Synthetic answer",
                usage: { input_tokens: 1, output_tokens: 2 } }
            : { message_count: 4, total_tokens: 6 }));
      });
      await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
      const url = `http://127.0.0.1:${server.address().port}`;
      if (mode === "connection-refused") await new Promise((resolve) => server.close(resolve));
      try {
        const result = await execute("python3",
          [join(examples, "022-rest-api-client-py/main.py")], { MEERKAT_REST_URL: url });
        assert.equal(result.killed, false);
        assert.match(result.stdout, /REST API Reference/);
        if (mode === "success") {
          successful(result);
          assert.match(result.stdout, /Response: Synthetic answer/);
          assert.deepEqual(requests.map((r) => [r.method, r.path]), [
            ["POST", "/sessions"],
            ["POST", "/sessions/synthetic-rest-session/messages"],
            ["GET", "/sessions/synthetic-rest-session"],
          ]);
        } else {
          assert.notEqual(result.code, 0);
          if (mode === "http-503") {
            assert.match(result.stderr, /Session creation failed: HTTP 503/);
            assert.doesNotMatch(result.stdout + result.stderr, /Failed to connect/);
            assert.match(result.stdout, /Synthetic service unavailable/);
            assert.equal(requests.length, 1);
          } else {
            assert.match(result.stderr, /Failed to connect/);
          }
        }
      } finally {
        if (server.listening) await new Promise((resolve) => server.close(resolve));
      }
    });
  }
});

describe("Run timeout", () => {
  // The fixture's never-reply mode leaves initialize unanswered, and the SDK
  // has no timeout of its own there, so only the per-run timeout ends the run.
  // The request list tells it apart from a run killed while it was closing.
  it("a run whose peer never replies is killed and reported as timed out",
    { timeout: HANG_PROBE_TIMEOUT_MS + 30_000 }, async () => {
      const started = Date.now();
      // A run stalled before it sends initialize is killed with no request.
      await assert.rejects(
        runExample("003-hello-meerkat-ts", "never-reply", {}, HANG_PROBE_TIMEOUT_MS),
        /003-hello-meerkat-ts\/never-reply timed out after the requests \[(initialize)?\]:/,
      );
      const elapsed = Date.now() - started;
      assert.ok(elapsed >= HANG_PROBE_TIMEOUT_MS && elapsed < HANG_PROBE_TIMEOUT_MS + 10_000,
        `the run is killed at its timeout (${elapsed} ms)`);
    });
});
