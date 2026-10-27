import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Meta } from "../dist/index.js";
import type { DurableAgentSpec } from "../dist/index.js";

const shouldRun =
  process.env.META_AGENT_MEMORY_REAL_API_SMOKE === "1";
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");

function resolveGatewayBin(): string {
  if (process.env.META_RPC_GATEWAY_BIN) {
    return process.env.META_RPC_GATEWAY_BIN;
  }
  try {
    const output = execFileSync(
      join(repoRoot, "scripts", "repo-cargo"),
      ["--print-env", "CARGO_TARGET_DIR"],
      { cwd: repoRoot, encoding: "utf8" },
    );
    const targetDir = output
      .split(/\r?\n/)
      .find((line) => line.startsWith("CARGO_TARGET_DIR="))
      ?.slice("CARGO_TARGET_DIR=".length)
      .trim();
    if (targetDir) {
      return join(targetDir, "debug", "rpc_gateway");
    }
  } catch {
    // Fall back to Cargo's default target dir below; the build step will
    // produce a clearer failure if neither path contains the gateway binary.
  }
  return join(repoRoot, "target", "debug", "rpc_gateway");
}

async function consoleRpc(
  baseUrl: string,
  method: string,
  params: Record<string, unknown>,
): Promise<unknown> {
  const response = await fetch(`${baseUrl}/console/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: `${method}:${Date.now()}`,
      method,
      params,
    }),
  });
  const text = await response.text();
  assert.equal(response.ok, true, `${method} HTTP ${response.status}: ${text}`);
  const payload = JSON.parse(text) as {
    result?: unknown;
    error?: { code: number; message: string };
  };
  assert.equal(
    payload.error,
    undefined,
    payload.error && `${method} RPC ${payload.error.code}: ${payload.error.message}`,
  );
  return payload.result;
}

async function sendAndWaitForToken(
  baseUrl: string,
  idempotencyKey: string,
): Promise<string> {
  const send = await consoleRpc(baseUrl, "meta/console/send", {
    identity: "identity:memory-smoke",
    content: "Reply with only the secret smoke token from your persisted memory.",
    origin: "agent-memory-real-smoke",
    idempotency_key: idempotencyKey,
    handling_mode: "queue",
  });
  assert.equal((send as Record<string, unknown>).status, "accepted");

  let observed = "";
  for (let attempt = 0; attempt < 90; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    const page = await consoleRpc(baseUrl, "meta/console/query_timeline", {
      identity: "identity:memory-smoke",
      mode: "recent",
      limit: 80,
    });
    observed = JSON.stringify(page);
    if (observed.includes("ZEBRA-17")) {
      break;
    }
  }
  return observed;
}

describe("agent memory real API smoke", { skip: !shouldRun }, () => {
  it("injects persisted identity memory into a real gateway-backed turn", { timeout: 240_000 }, async () => {
    assert.ok(
      process.env.OPENAI_API_KEY,
      "OPENAI_API_KEY is required when META_AGENT_MEMORY_REAL_API_SMOKE=1",
    );
    const root = mkdtempSync(join(tmpdir(), "mobkit-agent-memory-smoke-"));
    const configDir = join(root, "config");
    const stateWriteDir = join(root, "state-write");
    const stateRunDir = join(root, "state-run");
    mkdirSync(configDir, { recursive: true });
    mkdirSync(stateWriteDir, { recursive: true });
    mkdirSync(stateRunDir, { recursive: true });
    const gatewayBin = resolveGatewayBin();

    const mobToml = join(configDir, "mob.toml");
    writeFileSync(
      mobToml,
      `
[mob]
id = "agent-memory-smoke"

[skills.memory_smoke]
source = "inline"
content = "You are a terse smoke-test agent. Answer exactly what the user asks for."

[profiles.memory_smoke]
model = "${process.env.META_AGENT_MEMORY_SMOKE_MODEL ?? "gpt-5.5"}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["memory_smoke"]
peer_description = "Single smoke-test identity."

[profiles.memory_smoke.tools]
builtins = true
comms = true
memory = false
`,
    );

    class SmokeRosterProvider {
      async roster(): Promise<DurableAgentSpec[]> {
        return [
          {
            identity: "identity:memory-smoke",
            profile: "memory_smoke",
            addressability: "addressable",
            displayName: null,
            labels: { topic: "smoke" },
            context: null,
            additionalInstructions: ["Keep answers to one line."],
            runtimeModeOverride: "autonomous_host",
          },
        ];
      }
    }

    let runtime: Awaited<ReturnType<ReturnType<typeof Meta.builder>["build"]>> | null = null;
    try {
      runtime = await Meta.builder()
        .mob(mobToml)
        .gateway(gatewayBin)
        .persistentState(stateWriteDir)
        .consoleAuthRequired(false)
        .rosterProvider(new SmokeRosterProvider())
        .agentMemory({
          selection: "always",
          maxEntries: 4,
          recallTimeoutMs: 5_000,
          recallFailurePolicy: "fail",
        })
        .gatewayTimeoutMs(180_000)
        .build();

      assert.ok(runtime.rustHttpBaseUrl, "gateway must expose HTTP console base URL");
      const status = await runtime.status("identity:memory-smoke");
      assert.equal(status.lifecycleState, "active");
      const record = await runtime.mobHandle().rememberAgentMemory("identity:memory-smoke", {
        title: "Smoke token",
        body: "The secret smoke token is ZEBRA-17.",
        tags: ["smoke"],
      });
      assert.equal(record.title, "Smoke token");
      assert.match(record.memoryId, /^mem-/);
      const recalled = await runtime.mobHandle().recallAgentMemory("identity:memory-smoke", {
        queryTerms: ["ZEBRA-17"],
        selection: "contextual",
      });
      assert.equal(recalled.length, 1);
      assert.equal(recalled[0]!.body, "The secret smoke token is ZEBRA-17.");
      const memoryFile = join(
        stateWriteDir,
        "agent-memory",
        "default",
        "identity%3Amemory-smoke.md",
      );
      const writtenMemory = readFileSync(memoryFile, "utf-8");
      assert.match(writtenMemory, /ZEBRA-17/);
      assert.match(
        await sendAndWaitForToken(
          runtime.rustHttpBaseUrl,
          "agent-memory-real-smoke-active",
        ),
        /ZEBRA-17/,
      );
      await runtime.shutdown();
      runtime = null;

      mkdirSync(join(stateRunDir, "agent-memory", "default"), { recursive: true });
      writeFileSync(
        join(stateRunDir, "agent-memory", "default", "identity%3Amemory-smoke.md"),
        writtenMemory,
      );

      runtime = await Meta.builder()
        .mob(mobToml)
        .gateway(gatewayBin)
        .persistentState(stateRunDir)
        .consoleAuthRequired(false)
        .rosterProvider(new SmokeRosterProvider())
        .agentMemory({
          selection: "always",
          maxEntries: 4,
          recallTimeoutMs: 5_000,
          recallFailurePolicy: "fail",
        })
        .gatewayTimeoutMs(180_000)
        .build();
      assert.ok(runtime.rustHttpBaseUrl, "gateway must expose HTTP console base URL");
      const runStatus = await runtime.status("identity:memory-smoke");
      assert.equal(runStatus.lifecycleState, "active");

      assert.match(
        await sendAndWaitForToken(runtime.rustHttpBaseUrl, "agent-memory-real-smoke-restart"),
        /ZEBRA-17/,
      );
      const forgotten = await runtime.mobHandle().forgetAgentMemory(
        "identity:memory-smoke",
        record.memoryId,
      );
      assert.equal(forgotten.memoryId, record.memoryId);
      assert.equal(forgotten.deleted, true);
      const afterForget = await runtime.mobHandle().recallAgentMemory("identity:memory-smoke", {
        selection: "always",
      });
      assert.equal(afterForget.length, 0);
      assert.doesNotMatch(
        readFileSync(
          join(stateRunDir, "agent-memory", "default", "identity%3Amemory-smoke.md"),
          "utf-8",
        ),
        /ZEBRA-17/,
      );
    } finally {
      if (runtime) {
        await runtime.shutdown();
      }
      if (process.env.META_KEEP_AGENT_MEMORY_SMOKE !== "1") {
        rmSync(root, { recursive: true, force: true });
      }
    }
  });
});
