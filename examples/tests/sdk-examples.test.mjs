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

async function execute(command, args, extraEnv) {
  try {
    const output = await exec(command, args, {
      cwd: work, env: { ...env, ...extraEnv }, timeout: 15000,
    });
    return { ...output, code: 0, killed: false };
  } catch (error) {
    if (typeof error.code !== "number" && !error.killed) throw error;
    return error;
  }
}

async function runExample(example, mode = "success", extraEnv = {}) {
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
  );
  const records = existsSync(log)
    ? readFileSync(log, "utf8").trim().split("\n").map((line) => JSON.parse(line))
    : [];
  const pids = records.filter((record) => record.pid).map((record) => record.pid);
  // Cleanup is a safety net, never a passing outcome: the example must reap its peer.
  for (let attempt = 0; attempt < 40 && pids.some(alive); attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  const surviving = pids.filter(alive);
  for (const pid of surviving) process.kill(pid, "SIGKILL");
  assert.equal(result.killed, false, `${example}/${mode} timed out: ${result.stderr}`);
  assert.deepEqual(surviving, [], `${example}/${mode} leaked its child`);
  if (mode !== "missing") assert.equal(pids.length, 1, "must execute the real fixture");
  return {
    ...result, records,
    requests: records.filter((record) => record.method),
  };
}

function successful(result) {
  assert.equal(result.code, 0, result.stderr);
  assert.equal(result.stderr, "");
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
      assert.ok(creates.every((r) => r.params.model === "claude-sonnet-4-6"));
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
          ["claude-sonnet-4-6", "gpt-5.5", "gemini-3.5-flash", "claude-sonnet-4-6", "gpt-5.5"]);
        assert.deepEqual(creates[3].params.provider_params, { thinking_budget_tokens: 5000 });
        assert.deepEqual(creates[4].params.provider_params, {
          provider_tag: { provider: "open_ai", reasoning_effort: "high" },
        });
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
  it("021 with no provider keys prints setup guidance without creating sessions", async () => {
    const result = await runExample("021-multi-provider-routing-py");
    successful(result);
    assert.match(result.stdout, /Set at least one API key/);
    assert.ok(result.requests.every((r) => r.method !== "session/create"));
  });
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
