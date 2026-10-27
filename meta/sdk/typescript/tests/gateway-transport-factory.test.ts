/**
 * Host-composed gateway transport (twin of the Python
 * test_gateway_transport_factory): extra arguments and stdio slots on the
 * gateway process, a transport factory honored on every connect, and
 * host-owned startup that completes before `meta/init`.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ChildProcess } from "node:child_process";
import { once } from "node:events";
import type { Readable } from "node:stream";

import {
  GatewayRestartRefusedError,
  Meta,
  MetaRuntime,
  PersistentTransport,
} from "../dist/index.js";

/**
 * A stand-in gateway that answers every JSON-RPC request with `{}`. With
 * `META_TEST_SLOTS=1` it first writes its arguments to descriptor 3 and a
 * marker to descriptor 4.
 */
function writeSlotGateway(dir: string): string {
  const script = join(dir, "slot_gateway.cjs");
  writeFileSync(
    script,
    [
      'const fs = require("node:fs");',
      'const readline = require("node:readline");',
      'if (process.env.META_TEST_SLOTS === "1") {',
      '  fs.writeSync(3, JSON.stringify(process.argv.slice(2)) + "\\n");',
      '  fs.writeSync(4, "inherited\\n");',
      "}",
      "const rl = readline.createInterface({ input: process.stdin });",
      'rl.on("line", (line) => {',
      "  let request;",
      "  try { request = JSON.parse(line); } catch { return; }",
      '  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: {} }) + "\\n");',
      "});",
      'rl.on("close", () => process.exit(Number(process.env.META_TEST_EXIT_CODE ?? 0)));',
      "",
    ].join("\n"),
  );
  const gateway = join(dir, "slot_gateway.sh");
  writeFileSync(gateway, `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`, {
    mode: 0o755,
  });
  return gateway;
}

function readLine(stream: Readable): Promise<string> {
  return new Promise((resolve, reject) => {
    let buffered = "";
    stream.setEncoding("utf8");
    stream.on("data", (chunk: string) => {
      buffered += chunk;
      const end = buffered.indexOf("\n");
      if (end >= 0) resolve(buffered.slice(0, end));
    });
    stream.on("error", reject);
    stream.on("end", () => reject(new Error(`stream ended before a line: ${buffered}`)));
  });
}

/**
 * End the transport's gateway process (it exits on stdin EOF) and wait until
 * it is gone.
 */
async function killGateway(transport: PersistentTransport): Promise<void> {
  const child = (transport as unknown as { _process: ChildProcess })._process;
  const exited = once(child, "exit");
  child.stdin?.end();
  await exited;
  assert.equal(transport.isRunning(), false);
}

const STATUS = { jsonrpc: "2.0", id: "status-1", method: "meta/status", params: {} };

/** Records host startup and every request in order. */
class RecordingTransport extends PersistentTransport {
  readonly events: string[] = [];
  startupDone = false;
  failStartup: Error | null = null;

  override async startAsync(): Promise<void> {
    await super.startAsync();
    await new Promise((resolve) => setTimeout(resolve, 50));
    if (this.failStartup !== null) throw this.failStartup;
    this.startupDone = true;
    this.events.push("startup");
  }

  override async sendAsync(
    request: Record<string, unknown>,
    options?: { timeoutMs?: number },
  ): Promise<unknown> {
    this.events.push(`${String(request.method)} after startup=${this.startupDone}`);
    return super.sendAsync(request, options);
  }
}

/** Its own startup as a class field, assigned after the base constructor. */
class FieldStartupTransport extends PersistentTransport {
  startups = 0;
  startAsync = async (): Promise<void> => {
    this.start();
    this.startups += 1;
  };
}

describe("host-composed gateway transport", () => {
  it("passes extra arguments, a parent pipe and an inherited descriptor to the gateway", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const inheritedPath = join(dir, "inherited.txt");
    const inherited = openSync(inheritedPath, "w");
    const transport = new PersistentTransport(writeSlotGateway(dir), {
      env: { META_TEST_SLOTS: "1" },
      extraArgs: ["--host-channel", "a b"],
      extraStdio: ["pipe", inherited],
    });
    try {
      await transport.startAsync();
      // The creator keeps ownership of its descriptor and closes its copy.
      closeSync(inherited);
      const endpoint = transport.extraStdioEndpoint(0);
      assert.ok(endpoint !== null, "a pipe slot has a parent endpoint");
      const argv = JSON.parse(await readLine(endpoint as Readable));
      assert.deepEqual(argv, ["--persistent", "--host-channel", "a b"]);
      assert.equal(transport.extraStdioEndpoint(1), null, "an inherited slot has no parent endpoint");
      assert.throws(() => transport.extraStdioEndpoint(2), RangeError);
    } finally {
      await transport.stop();
    }
    assert.equal(transport.extraStdioEndpoint(0), null, "no endpoint once the process is gone");
    assert.equal(readFileSync(inheritedPath, "utf8"), "inherited\n");
    rmSync(dir, { recursive: true, force: true });
  });

  it("refuses invalid arguments and stdio slots before any spawn", () => {
    const invalid: Array<Record<string, unknown>> = [
      { extraArgs: ["a\0b"] },
      { extraStdio: [1] },
      { extraStdio: [3.5] },
      { extraStdio: [5, 5] },
      { extraStdio: ["ipc"] },
    ];
    for (const options of invalid) {
      assert.throws(
        () => new PersistentTransport("/nonexistent/gateway", options as never),
        /extraArgs|extraStdio/,
        JSON.stringify(options),
      );
    }
  });

  it("builds a fresh transport on every connect and finishes its startup before meta/init", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const gateway = writeSlotGateway(dir);
    const made: RecordingTransport[] = [];
    const seen: Array<{ gatewayBin: string; timeoutMs: number | undefined }> = [];
    const builder = Meta.builder()
      .gateway(gateway)
      .gatewayTimeoutMs(5_000)
      .gatewayTransport((args) => {
        seen.push(args);
        const transport = new RecordingTransport(args.gatewayBin, { timeout: args.timeoutMs });
        made.push(transport);
        return transport;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      await rt.connect();
      assert.equal(made.length, 1);
      assert.deepEqual(seen[0], { gatewayBin: gateway, timeoutMs: 5_000 });
      assert.deepEqual(made[0].events, ["startup", "meta/init after startup=true"]);
      assert.equal(made[0].lastTermination(), null, "no record while the gateway runs");
      await rt.shutdown();
      assert.equal(made[0].isRunning(), false, "shutdown stops the factory's transport");
      assert.deepEqual(made[0].lastTermination(), {
        state: "exited",
        exitCode: 0,
        signal: null,
        escalation: "none",
        nativeShutdown: "not_negotiated",
      });

      await rt.connect();
      assert.equal(made.length, 2, "the factory runs again on the next connect");
      assert.notEqual(made[1], made[0]);
      assert.equal(made[1].isRunning(), true);
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
    assert.equal(made[1].isRunning(), false);
  });

  it("a failed host startup fails connect, sends nothing and stops the transport", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    let made: RecordingTransport | null = null;
    const builder = Meta.builder()
      .gateway(writeSlotGateway(dir))
      .gatewayTransport(({ gatewayBin }) => {
        made = new RecordingTransport(gatewayBin);
        made.failStartup = new Error("host bootstrap refused");
        return made;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      await assert.rejects(rt.connect(), /host bootstrap refused/);
      const transport = made as RecordingTransport | null;
      assert.ok(transport !== null);
      assert.deepEqual(transport.events, [], "no request reached the gateway");
      assert.equal(transport.isRunning(), false, "the failed connect stopped the transport");
      assert.equal(rt.isRunning, false);
      assert.deepEqual(transport.lastTermination(), {
        state: "exited",
        exitCode: 0,
        signal: null,
        escalation: "none",
        nativeShutdown: "not_negotiated",
      });
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a host startup that fails before spawning records not_started", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    let made: PersistentTransport | null = null;
    class RefusingTransport extends PersistentTransport {
      override async startAsync(): Promise<void> {
        throw new Error("host refused before spawning");
      }
    }
    const builder = Meta.builder()
      .gateway(writeSlotGateway(dir))
      .gatewayTransport(({ gatewayBin }) => {
        made = new RefusingTransport(gatewayBin);
        return made;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      await assert.rejects(rt.connect(), /host refused before spawning/);
      const transport = made as PersistentTransport | null;
      assert.ok(transport !== null);
      assert.deepEqual(transport.lastTermination(), {
        state: "not_started",
        exitCode: null,
        signal: null,
        escalation: "none",
        nativeShutdown: "not_negotiated",
      });
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("records a nonzero gateway exit, and a respawned child starts without a record", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const gateway = writeSlotGateway(dir);
    const failing = new PersistentTransport(gateway, { env: { META_TEST_EXIT_CODE: "3" } });
    const plain = new PersistentTransport(gateway);
    try {
      await failing.startAsync();
      await failing.stop();
      assert.deepEqual(failing.lastTermination(), {
        state: "exited",
        exitCode: 3,
        signal: null,
        escalation: "none",
        nativeShutdown: "not_negotiated",
      });

      plain.start();
      await plain.stop();
      assert.equal(plain.lastTermination()?.exitCode, 0);
      plain.start();
      assert.equal(plain.lastTermination(), null, "a new child never inherits a record");
      await plain.stop();
      assert.equal(plain.lastTermination()?.state, "exited");
    } finally {
      await failing.stop();
      await plain.stop();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a gateway whose spawn failed records not_started", async () => {
    const transport = new PersistentTransport("/nonexistent/meta-gateway");
    transport.start();
    await transport.stop();
    assert.equal(transport.lastTermination()?.state, "not_started");
    assert.equal(transport.lastTermination()?.exitCode, null);
  });

  it("uses a transport given to the runtime constructor", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const gateway = writeSlotGateway(dir);
    const injected = new RecordingTransport(gateway);
    const rt = new MetaRuntime((Meta.builder().gateway(gateway) as any)._config, injected);
    try {
      await rt.connect();
      assert.deepEqual(injected.events, ["startup", "meta/init after startup=true"]);
      assert.equal(injected.isRunning(), true);
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
    assert.equal(injected.isRunning(), false);
  });

  it("a factory-built transport refuses to respawn its gateway until the runtime reconnects", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const made: PersistentTransport[] = [];
    const builder = Meta.builder()
      .gateway(writeSlotGateway(dir))
      .gatewayTransport(({ gatewayBin }) => {
        // A plain transport: the factory alone makes it host-composed.
        const transport = new PersistentTransport(gatewayBin);
        made.push(transport);
        return transport;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      await rt.connect();
      await killGateway(made[0]);
      await assert.rejects(
        (rt as any)._rpcUnchecked("meta/status", {}),
        (error: unknown) =>
          error instanceof GatewayRestartRefusedError && /exited/.test(error.message),
      );
      assert.equal(made[0].isRunning(), false, "no silent respawn without the host startup");

      await rt.shutdown();
      await rt.connect();
      assert.equal(made.length, 2, "the reconnect went through the factory");
      assert.deepEqual(await (rt as any)._rpcUnchecked("meta/status", {}), {});
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a transport with its own startup refuses to respawn; a default transport still respawns", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const gateway = writeSlotGateway(dir);
    const hosted = new RecordingTransport(gateway);
    const plain = new PersistentTransport(gateway);
    try {
      await assert.rejects(
        hosted.sendAsync(STATUS),
        (error: unknown) =>
          error instanceof GatewayRestartRefusedError && /not started/.test(error.message),
      );
      await hosted.startAsync();
      await killGateway(hosted);
      await assert.rejects(hosted.sendAsync(STATUS), GatewayRestartRefusedError);
      assert.equal(hosted.isRunning(), false);
      // Its own startup is still the way back.
      await hosted.startAsync();
      assert.deepEqual(await hosted.sendAsync(STATUS), { jsonrpc: "2.0", id: "status-1", result: {} });

      plain.start();
      await killGateway(plain);
      assert.deepEqual(await plain.sendAsync(STATUS), { jsonrpc: "2.0", id: "status-1", result: {} });
      assert.equal(plain.isRunning(), true, "the default transport keeps its respawn");
    } finally {
      await hosted.stop();
      await plain.stop();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a gateway killed by a signal counts as exited: a default transport respawns it, a host-composed one refuses", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const gateway = writeSlotGateway(dir);
    const hosted = new RecordingTransport(gateway);
    const plain = new PersistentTransport(gateway);
    const sigkill = async (transport: PersistentTransport): Promise<void> => {
      const child = (transport as unknown as { _process: ChildProcess })._process;
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
      assert.equal(child.exitCode, null, "a signal-killed child has no exit code");
      assert.equal(transport.isRunning(), false);
    };
    try {
      plain.start();
      const first = (plain as unknown as { _process: ChildProcess })._process;
      await sigkill(plain);
      assert.deepEqual(await plain.sendAsync(STATUS), { jsonrpc: "2.0", id: "status-1", result: {} });
      assert.notEqual((plain as unknown as { _process: ChildProcess })._process, first);
      assert.equal(plain.isRunning(), true, "the default transport respawned the gateway");

      await hosted.startAsync();
      await sigkill(hosted);
      await assert.rejects(
        hosted.sendAsync(STATUS),
        (error: unknown) =>
          error instanceof GatewayRestartRefusedError && /exited/.test(error.message),
      );
      assert.equal(hosted.isRunning(), false);
      await hosted.stop();
      // Killed from outside: the signal is the child's own end, not an
      // escalation this stop attempted.
      assert.deepEqual(hosted.lastTermination(), {
        state: "exited",
        exitCode: null,
        signal: "SIGKILL",
        escalation: "none",
        nativeShutdown: "not_negotiated",
      });
    } finally {
      await hosted.stop();
      await plain.stop();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a startAsync class field makes a constructor-injected transport host-composed", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-slot-gateway-"));
    const gateway = writeSlotGateway(dir);
    const injected = new FieldStartupTransport(gateway);
    const rt = new MetaRuntime((Meta.builder().gateway(gateway) as any)._config, injected);
    try {
      await rt.connect();
      assert.equal(injected.startups, 1, "connect ran the field startup");
      await killGateway(injected);
      await assert.rejects(
        (rt as any)._rpcUnchecked("meta/status", {}),
        (error: unknown) =>
          error instanceof GatewayRestartRefusedError && /exited/.test(error.message),
      );
      assert.equal(injected.isRunning(), false, "no respawn that skips the field startup");
      assert.equal(injected.startups, 1);

      await rt.shutdown();
      await rt.connect();
      assert.equal(injected.startups, 2, "the reconnect ran the field startup again");
      assert.deepEqual(await (rt as any)._rpcUnchecked("meta/status", {}), {});
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("the builder validates the factory", async () => {
    assert.throws(() => Meta.builder().gatewayTransport("nope" as never), TypeError);
    await assert.rejects(
      Meta.builder()
        .gatewayTransport(({ gatewayBin }) => new PersistentTransport(gatewayBin))
        .build(),
      /gatewayTransport\(\) requires gateway\(\)/,
    );
  });
});
