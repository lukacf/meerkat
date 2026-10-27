/**
 * Runtime ownership of gateway cleanup: a transport whose stop rejected stays
 * with its runtime until a later stop fulfils, a reconnect settles it before
 * building another transport, and a failed startup whose stop rejected
 * reaches the same runtime through a typed error.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ChildProcess } from "node:child_process";
import { once } from "node:events";

import {
  GatewayStartupCleanupError,
  Meta,
  MetaRuntime,
  NotConnectedError,
  PersistentTransport,
} from "../dist/index.js";

/**
 * A stand-in gateway answering every request with `result`. With
 * `stubborn`, it ignores stdin EOF and SIGTERM, so only SIGKILL ends it;
 * `shutdownError` answers `meta/shutdown` with that error.
 */
function writeGateway(
  dir: string,
  name: string,
  options: { stubborn?: boolean; result?: unknown; shutdownError?: string } = {},
): string {
  const script = join(dir, `${name}.cjs`);
  writeFileSync(
    script,
    [
      'const readline = require("node:readline");',
      `const result = ${JSON.stringify(options.result ?? {})};`,
      `const shutdownError = ${JSON.stringify(options.shutdownError ?? null)};`,
      "const rl = readline.createInterface({ input: process.stdin });",
      'rl.on("line", (line) => {',
      "  let request;",
      "  try { request = JSON.parse(line); } catch { return; }",
      '  const answer = request.method === "meta/shutdown" && shutdownError !== null',
      '    ? { jsonrpc: "2.0", id: request.id, error: { code: -32000, message: shutdownError } }',
      '    : { jsonrpc: "2.0", id: request.id, result };',
      '  process.stdout.write(JSON.stringify(answer) + "\\n");',
      "});",
      options.stubborn
        ? 'process.on("SIGTERM", () => {}); setInterval(() => {}, 1000);'
        : 'rl.on("close", () => process.exit(0));',
      "",
    ].join("\n"),
  );
  const gateway = join(dir, `${name}.sh`);
  writeFileSync(gateway, `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`, {
    mode: 0o755,
  });
  return gateway;
}

/** A short bounded wait for a child's exit, scaling the stop's waits. */
async function quickWait(child: ChildProcess): Promise<boolean> {
  if (child.exitCode !== null || child.signalCode !== null) return true;
  let handle: NodeJS.Timeout | undefined;
  const timer = new Promise<boolean>((resolve) => {
    handle = setTimeout(() => resolve(child.exitCode !== null || child.signalCode !== null), 100);
  });
  return Promise.race([once(child, "exit").then(() => true), timer]).finally(() =>
    clearTimeout(handle),
  );
}

/**
 * Its gateway survives bounded cleanup while `allowKill` is false: the
 * transport's kill attempts reach the real process only when allowed.
 */
class StubbornTransport extends PersistentTransport {
  allowKill = false;
  failStartup: Error | null = null;

  constructor(gatewayBin: string) {
    super(gatewayBin);
    (this as unknown as { _waitForExit: typeof quickWait })._waitForExit = quickWait;
  }

  override async startAsync(): Promise<void> {
    await super.startAsync();
    const child = (this as unknown as { _process: ChildProcess })._process;
    const kill = child.kill.bind(child);
    child.kill = ((signal?: NodeJS.Signals | number) =>
      this.allowKill ? kill(signal) : true) as ChildProcess["kill"];
    if (this.failStartup !== null) throw this.failStartup;
  }
}

/** A gateway that exits at once: an inert child for startup failures. */
function writeInertGateway(dir: string): string {
  const gateway = join(dir, "inert.sh");
  writeFileSync(gateway, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  return gateway;
}

/**
 * A host-composed transport whose own private cleanup rejects the first
 * time: its stop lets the base transport stop the gateway, then closes a
 * private channel. Its startup fails once its child exited or, with `spawn`
 * false, before any child exists.
 */
class PrivateCloseTransport extends PersistentTransport {
  closes = 0;

  constructor(
    gatewayBin: string,
    private readonly startupError: unknown,
    private readonly closeError: unknown,
    private readonly spawn: boolean,
  ) {
    super(gatewayBin);
  }

  override async startAsync(): Promise<void> {
    if (this.spawn) {
      await super.startAsync();
      const child = (this as unknown as { _process: ChildProcess })._process;
      if (child.exitCode === null && child.signalCode === null) await once(child, "exit");
    }
    throw this.startupError;
  }

  override async stop(): Promise<void> {
    await super.stop();
    this.closes += 1;
    if (this.closes === 1) throw this.closeError;
  }
}

describe("runtime gateway cleanup ownership", () => {
  it("a startup failure whose cleanup does not settle keeps the gateway with the runtime", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
    const stubbornGateway = writeGateway(dir, "stubborn", { stubborn: true });
    const plainGateway = writeGateway(dir, "plain");
    const made: PersistentTransport[] = [];
    let first: StubbornTransport | null = null;
    const builder = Meta.builder()
      .gateway(stubbornGateway)
      .gatewayTransport(({ gatewayBin }) => {
        if (first === null) {
          first = new StubbornTransport(gatewayBin);
          first.failStartup = new Error("host bootstrap refused");
          made.push(first);
          return first;
        }
        const transport = new PersistentTransport(plainGateway);
        made.push(transport);
        return transport;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      const error = await rt.connect().then(() => null, (failure: unknown) => failure);
      assert.ok(error instanceof GatewayStartupCleanupError, String(error));
      assert.match((error.startupError as Error).message, /host bootstrap refused/);
      assert.match((error.cleanupError as Error).message, /did not terminate after bounded cleanup/);
      assert.equal(error.cause, error.startupError);
      assert.deepEqual(error.cleanup.lastTermination(), {
        state: "unknown",
        exitCode: null,
        signal: null,
        escalation: "sigkill",
        nativeShutdown: "not_negotiated",
      });
      const stubborn = first as unknown as StubbornTransport;
      assert.equal(stubborn.isRunning(), true, "the live child stays owned");

      // A reconnect settles the retained owner first and, while it cannot,
      // never builds another transport.
      await assert.rejects(rt.connect(), /did not terminate after bounded cleanup/);
      assert.equal(made.length, 1);

      // The cleanup capability rejoins the same stop on the same child.
      stubborn.allowKill = true;
      await error.cleanup.shutdown();
      assert.equal(stubborn.isRunning(), false);
      assert.deepEqual(rt.lastGatewayTermination(), {
        state: "exited",
        exitCode: null,
        signal: "SIGKILL",
        escalation: "sigkill",
        nativeShutdown: "not_negotiated",
      });

      await rt.connect();
      assert.equal(made.length, 2, "once settled, a reconnect builds a new transport");
      assert.equal(rt.isRunning, true);
    } finally {
      if (first !== null) (first as StubbornTransport).allowKill = true;
      await rt.shutdown().catch(() => undefined);
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a shutdown whose stop does not settle keeps the transport for the next shutdown", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
    let stubborn: StubbornTransport | null = null;
    const builder = Meta.builder()
      .gateway(writeGateway(dir, "stubborn", { stubborn: true }))
      .gatewayTransport(({ gatewayBin }) => {
        stubborn = new StubbornTransport(gatewayBin);
        return stubborn;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      await rt.connect();
      const transport = stubborn as unknown as StubbornTransport;
      const original = (transport as unknown as { _process: ChildProcess })._process;
      await assert.rejects(rt.shutdown(), /did not terminate after bounded cleanup/);
      assert.equal(rt.isRunning, false, "public admission closed");
      await assert.rejects((rt as any)._rpc("meta/status", {}), NotConnectedError);
      assert.equal(transport.isRunning(), true, "the live child stays owned");
      assert.equal(
        (transport as unknown as { _process: ChildProcess })._process,
        original,
        "the transport's finally kept the exact original live child",
      );
      assert.equal(original.exitCode, null);
      assert.equal(original.signalCode, null);
      assert.equal(rt.lastGatewayTermination()?.state, "unknown");

      // The second shutdown rejoins the stop of that same child.
      transport.allowKill = true;
      await rt.shutdown();
      assert.equal(transport.isRunning(), false);
      assert.equal(original.signalCode, "SIGKILL", "the same original child ended");
      assert.equal(rt.lastGatewayTermination()?.state, "exited");
      assert.equal(rt.lastGatewayTermination()?.signal, "SIGKILL");
      await rt.shutdown();
    } finally {
      if (stubborn !== null) (stubborn as StubbornTransport).allowKill = true;
      await rt.shutdown().catch(() => undefined);
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("a rejoined shutdown keeps the gateway's one shutdown answer", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
    // One result answers meta/init (negotiating the handshake) and
    // meta/shutdown (acknowledging the runtime cleanup).
    const gateway = writeGateway(dir, "stubborn", {
      stubborn: true,
      result: {
        stdio_shutdown_handshake: true,
        stdio_shutdown_horizon_ms: 300,
        shutdown: true,
        runtime_cleanup_completed: true,
      },
    });
    let stubborn: StubbornTransport | null = null;
    const builder = Meta.builder()
      .gateway(gateway)
      .gatewayTransport(({ gatewayBin }) => {
        stubborn = new StubbornTransport(gatewayBin);
        return stubborn;
      });
    const rt = new MetaRuntime((builder as any)._config);
    try {
      await rt.connect();
      const transport = stubborn as unknown as StubbornTransport;
      await assert.rejects(rt.shutdown(), /did not terminate after bounded cleanup/);
      assert.deepEqual(rt.lastGatewayTermination(), {
        state: "unknown",
        exitCode: null,
        signal: null,
        escalation: "sigkill",
        nativeShutdown: "acknowledged",
      });

      // The rejoin finishes the same child without a second shutdown request
      // on the stdin the first stop ended: the answer stands.
      transport.allowKill = true;
      await rt.shutdown();
      assert.deepEqual(rt.lastGatewayTermination(), {
        state: "exited",
        exitCode: null,
        signal: "SIGKILL",
        escalation: "sigkill",
        nativeShutdown: "acknowledged",
      });
    } finally {
      if (stubborn !== null) (stubborn as StubbornTransport).allowKill = true;
      await rt.shutdown().catch(() => undefined);
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("builder.build() keeps a settled startup failure's own error and reaches an unsettled one", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
    const plainGateway = writeGateway(dir, "plain");
    const stubbornGateway = writeGateway(dir, "stubborn", { stubborn: true });
    let stubborn: StubbornTransport | null = null;
    try {
      // The cleanup settles: the original startup error, unchanged.
      const settled = await Meta.builder()
        .gateway(plainGateway)
        .gatewayTransport(({ gatewayBin }) => {
          const transport = new StubbornTransport(gatewayBin);
          transport.allowKill = true;
          transport.failStartup = new Error("host bootstrap refused");
          return transport;
        })
        .build()
        .then(() => null, (failure: unknown) => failure);
      assert.ok(settled instanceof Error);
      assert.ok(!(settled instanceof GatewayStartupCleanupError));
      assert.match(settled.message, /host bootstrap refused/);

      // The cleanup does not settle: no runtime is returned, but the typed
      // error reaches the same runtime's cleanup.
      const unsettled = await Meta.builder()
        .gateway(stubbornGateway)
        .gatewayTransport(({ gatewayBin }) => {
          stubborn = new StubbornTransport(gatewayBin);
          stubborn.failStartup = new Error("host bootstrap refused");
          return stubborn;
        })
        .build()
        .then(() => null, (failure: unknown) => failure);
      assert.ok(unsettled instanceof GatewayStartupCleanupError);
      assert.equal(unsettled.cleanup.lastTermination()?.state, "unknown");
      (stubborn as unknown as StubbornTransport).allowKill = true;
      await unsettled.cleanup.shutdown();
      assert.equal(unsettled.cleanup.lastTermination()?.state, "exited");
      assert.equal((stubborn as unknown as StubbornTransport).isRunning(), false);
    } finally {
      if (stubborn !== null) {
        (stubborn as StubbornTransport).allowKill = true;
        await (stubborn as StubbornTransport).stop().catch(() => undefined);
      }
      rmSync(dir, { recursive: true, force: true });
    }
  });

  for (const spawn of [true, false]) {
    const after = spawn ? "after its child exited" : "with no child";
    it(`builder.build() keeps a private cleanup rejection ${after}, and only the returned cleanup retries it`, async () => {
      const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
      const startupError = new Error("host bootstrap refused");
      const closeError = new Error("private channel close failed");
      let made = 0;
      let owner: PrivateCloseTransport | null = null;
      try {
        const failure = await Meta.builder()
          .gateway(writeInertGateway(dir))
          .gatewayTransport(({ gatewayBin }) => {
            made += 1;
            owner = new PrivateCloseTransport(gatewayBin, startupError, closeError, spawn);
            return owner;
          })
          .build()
          .then(() => null, (error: unknown) => error);
        // The process outcome does not attest the transport's own cleanup:
        // its rejection reaches the caller with both exact errors.
        assert.ok(failure instanceof GatewayStartupCleanupError, String(failure));
        assert.equal(failure.startupError, startupError);
        assert.equal(failure.cause, startupError);
        assert.equal(failure.cleanupError, closeError);
        assert.deepEqual(failure.cleanup.lastTermination(), {
          state: spawn ? "exited" : "not_started",
          exitCode: spawn ? 0 : null,
          signal: null,
          escalation: "none",
          nativeShutdown: "not_negotiated",
        });
        const transport = owner as unknown as PrivateCloseTransport;
        assert.equal(transport.closes, 1);

        // The returned cleanup retries that same owner's stop and settles it.
        await failure.cleanup.shutdown();
        assert.equal(transport.closes, 2, "the retry ran the same private cleanup");
        await failure.cleanup.shutdown();
        assert.equal(transport.closes, 2, "a settled cleanup is not repeated");
        assert.equal(made, 1, "no transport was made again");
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  }

  /** An Error whose `message` getter throws. */
  function errorWithThrowingMessage(): Error {
    const error = new Error("placeholder");
    Object.defineProperty(error, "message", {
      get() {
        throw new Error("unprintable message");
      },
    });
    return error;
  }

  for (const [label, makeStartupError, makeCloseError] of [
    [
      "a rejection without a prototype",
      () => new Error("host bootstrap refused"),
      () => Object.create(null),
    ],
    [
      "a rejection whose message getter throws",
      () => new Error("host bootstrap refused"),
      errorWithThrowingMessage,
    ],
    [
      "a startup failure without a prototype",
      () => Object.create(null),
      () => new Error("private channel close failed"),
    ],
  ] as const) {
    it(`builder.build() keeps ${label} exactly, and its cleanup still settles`, async () => {
      const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
      const startupError: unknown = makeStartupError();
      const closeError: unknown = makeCloseError();
      let made = 0;
      let owner: PrivateCloseTransport | null = null;
      try {
        const failure = await Meta.builder()
          .gateway(writeInertGateway(dir))
          .gatewayTransport(({ gatewayBin }) => {
            made += 1;
            owner = new PrivateCloseTransport(gatewayBin, startupError, closeError, true);
            return owner;
          })
          .build()
          .then(() => null, (error: unknown) => error);
        // An unprintable failure cannot make the typed error itself throw.
        assert.ok(failure instanceof GatewayStartupCleanupError, "the typed error is thrown");
        assert.equal(failure.startupError, startupError);
        assert.equal(failure.cause, startupError);
        assert.equal(failure.cleanupError, closeError);
        const transport = owner as unknown as PrivateCloseTransport;
        await failure.cleanup.shutdown();
        assert.equal(transport.closes, 2, "the same cleanup settled");
        assert.equal(made, 1, "no transport was made again");
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    });
  }

  it("a refused native shutdown after the gateway exited stays visible, and the next shutdown settles", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-cleanup-"));
    const gateway = writeGateway(dir, "refusing", {
      result: { stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 2_000 },
      shutdownError: "cleanup refused",
    });
    const rt = new MetaRuntime((Meta.builder().gateway(gateway) as any)._config);
    try {
      await rt.connect();
      await assert.rejects(rt.shutdown(), /gateway shutdown failed after bounded cleanup: .*cleanup refused/);
      assert.deepEqual(rt.lastGatewayTermination(), {
        state: "exited",
        exitCode: 0,
        signal: null,
        escalation: "none",
        nativeShutdown: "refused",
      });
      // The child is already released; the retained stop fulfils at once.
      await rt.shutdown();
      assert.equal(rt.lastGatewayTermination()?.nativeShutdown, "refused");
    } finally {
      await rt.shutdown().catch(() => undefined);
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
