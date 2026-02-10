/**
 * E2E smoke tests for the TypeScript SDK against a live rkat rpc server.
 *
 * Requirements:
 *   - rkat binary on PATH (cargo build -p meerkat-cli)
 *   - ANTHROPIC_API_KEY set for live API tests
 *
 * Run with: node --test tests/e2e_smoke.test.ts
 */

import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn, execSync, type ChildProcess } from "node:child_process";
import { createInterface, type Interface } from "node:readline";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function rkatAvailable(): boolean {
  try {
    execSync("which rkat", { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasApiKey(): boolean {
  return !!(
    process.env.RKAT_ANTHROPIC_API_KEY || process.env.ANTHROPIC_API_KEY
  );
}

interface RpcClient {
  proc: ChildProcess;
  rl: Interface;
  nextId: number;
  request(method: string, params?: Record<string, unknown>): Promise<unknown>;
  close(): void;
}

function createRpcClient(): RpcClient {
  const proc = spawn("rkat", ["rpc"], {
    stdio: ["pipe", "pipe", "pipe"],
  });

  const rl = createInterface({ input: proc.stdout! });
  const pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void }
  >();

  rl.on("line", (line: string) => {
    try {
      const response = JSON.parse(line);
      if ("id" in response && pending.has(response.id)) {
        const p = pending.get(response.id)!;
        pending.delete(response.id);
        if (response.error) {
          p.reject(
            new Error(
              `RPC error ${response.error.code}: ${response.error.message}`,
            ),
          );
        } else {
          p.resolve(response.result ?? {});
        }
      }
    } catch {
      // ignore non-JSON lines (notifications, etc.)
    }
  });

  const client: RpcClient = {
    proc,
    rl,
    nextId: 1,
    request(method: string, params: Record<string, unknown> = {}) {
      const id = this.nextId++;
      const request = { jsonrpc: "2.0", id, method, params };
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        proc.stdin!.write(JSON.stringify(request) + "\n");
      });
    },
    close() {
      proc.kill();
    },
  };

  return client;
}

// ---------------------------------------------------------------------------
// Scenario: Full TypeScript SDK lifecycle
// ---------------------------------------------------------------------------

describe("E2E Smoke: TypeScript SDK", { skip: !rkatAvailable() }, () => {
  let client: RpcClient;

  before(() => {
    client = createRpcClient();
  });

  after(() => {
    client?.close();
  });

  it("should initialize and get capabilities", async () => {
    const init = (await client.request("initialize")) as Record<
      string,
      unknown
    >;
    assert.ok(init.server_info, "Should have server_info");

    const caps = (await client.request("capabilities/get")) as Record<
      string,
      unknown
    >;
    assert.ok(caps.contract_version, "Should have contract_version");

    const capabilities = caps.capabilities as Array<Record<string, unknown>>;
    assert.ok(capabilities.length > 0, "Should have capabilities");

    const capIds = capabilities.map((c) => c.id);
    assert.ok(capIds.includes("sessions"), "Should have sessions capability");

    // Verify Available status uses capital A (our fix)
    const sessionsCap = capabilities.find((c) => c.id === "sessions");
    assert.equal(
      sessionsCap?.status,
      "Available",
      "Status should be 'Available' (capital A)",
    );
  });

  it(
    "should create session and do multi-turn",
    { skip: !hasApiKey() },
    async () => {
      // Create session
      const create = (await client.request("session/create", {
        prompt: "My name is TsBot. Remember that.",
      })) as Record<string, unknown>;

      const sessionId = String(create.session_id ?? "");
      assert.ok(sessionId, "Should have session_id");
      assert.ok(create.text, "Should have response text");

      // Follow-up turn
      const turn = (await client.request("turn/start", {
        session_id: sessionId,
        prompt: "What is my name?",
      })) as Record<string, unknown>;

      const text = String(turn.text ?? "");
      assert.ok(text.includes("TsBot"), `Should recall TsBot, got: ${text}`);

      // List sessions
      const list = (await client.request("session/list")) as Record<
        string,
        unknown
      >;
      const sessions = (list.sessions ?? []) as Array<Record<string, unknown>>;
      const ids = sessions.map((s) => s.session_id);
      assert.ok(ids.includes(sessionId), "Session should be in list");

      // Archive
      await client.request("session/archive", { session_id: sessionId });

      // Verify gone
      const list2 = (await client.request("session/list")) as Record<
        string,
        unknown
      >;
      const sessions2 = (list2.sessions ?? []) as Array<
        Record<string, unknown>
      >;
      const ids2 = sessions2.map((s) => s.session_id);
      assert.ok(
        !ids2.includes(sessionId),
        "Session should be gone after archive",
      );
    },
  );

  it(
    "should handle errors gracefully and recover",
    { skip: !hasApiKey() },
    async () => {
      // Try non-existent session
      await assert.rejects(
        () =>
          client.request("turn/start", {
            session_id: "00000000-0000-0000-0000-000000000000",
            prompt: "hello",
          }),
        /error/i,
        "Should reject for non-existent session",
      );

      // Recovery: create a real session
      const create = (await client.request("session/create", {
        prompt: "What is 2+2? Reply with just the number.",
      })) as Record<string, unknown>;

      assert.ok(create.session_id, "Should get session_id after recovery");
      const text = String(create.text ?? "");
      assert.ok(text.includes("4"), `Should contain '4', got: ${text}`);
    },
  );
});
