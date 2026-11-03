/**
 * Deterministic lifecycle-policy tests for PersistentTransport.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import type { ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";

import {
  PersistentTransport,
  PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS,
  PROVIDER_CALLBACK_COMPLETION_MS,
  stopChildProcess,
} from "../dist/transport.js";

interface FakeChild {
  readonly child: ChildProcess;
  readonly signals: NodeJS.Signals[];
  stdinEnded: number;
}

function fakeChild(): FakeChild {
  const signals: NodeJS.Signals[] = [];
  const state = {
    stdinEnded: 0,
  };
  const child = Object.assign(new EventEmitter(), {
    stdin: {
      end: () => {
        state.stdinEnded += 1;
      },
    },
    exitCode: null,
    signalCode: null,
    // A spawned child has a pid; only a failed spawn lacks one.
    pid: 4242,
    kill: (signal: NodeJS.Signals) => {
      signals.push(signal);
      return true;
    },
  }) as unknown as ChildProcess;

  return {
    child,
    signals,
    get stdinEnded() {
      return state.stdinEnded;
    },
    set stdinEnded(value: number) {
      state.stdinEnded = value;
    },
  };
}

describe("persistent transport shutdown", () => {
  it("aborts a stalled provider callback and suppresses its late response", async () => {
    const transport = new PersistentTransport("unused-test-gateway");
    const writes: Array<Record<string, unknown>> = [];
    let observedSignal: AbortSignal | null = null;
    let committed = false;
    let releaseProvider!: () => void;
    const providerGate = new Promise<void>((resolve) => {
      releaseProvider = resolve;
    });

    assert.equal(PROVIDER_CALLBACK_COMPLETION_MS, 125_000);
    (transport as any)._providerCallbackCompletionMs = 10;
    (transport as any)._writeLine = (value: Record<string, unknown>) => {
      writes.push(value);
    };
    transport.setCallbackHandler(async (_method, _params, context) => {
      observedSignal = context.signal;
      await providerGate;
      context.signal.throwIfAborted();
      committed = true;
      return [];
    });

    (transport as any)._handleCallback({
      jsonrpc: "2.0",
      id: "cb-timeout",
      method: "callback/roster_provider/roster",
      params: {},
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 25));

    // Assigned in the callback, which control-flow narrowing does not see.
    assert.equal((observedSignal as AbortSignal | null)?.aborted, true);
    assert.equal(writes.length, 1);
    assert.match(
      String((writes[0].error as Record<string, unknown>).message),
      /125s host completion deadline/,
    );

    releaseProvider();
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.equal(committed, false, "an aborted provider must not commit authority");
    assert.equal(writes.length, 1, "late provider completion must not write a response");
  });

  it("negotiates and validates the advertised shutdown horizon", async () => {
    const transport = new PersistentTransport("unused-test-gateway");
    (transport as any)._sendAsyncWithTimeout = async () => ({
      jsonrpc: "2.0",
      id: "init-horizon",
      result: {
        stdio_shutdown_handshake: true,
        stdio_shutdown_horizon_ms: 321_000,
      },
    });

    await transport.sendAsync({
      jsonrpc: "2.0",
      id: "init-horizon",
      method: "meta/init",
      params: {},
    });

    assert.equal((transport as any)._supportsShutdownHandshake, true);
    assert.equal((transport as any)._shutdownHorizonMs, 321_000);

    for (const invalid of [undefined, true, 0, -1, 1.5, "335000"]) {
      (transport as any)._sendAsyncWithTimeout = async () => ({
        jsonrpc: "2.0",
        id: "init-invalid-horizon",
        result: {
          stdio_shutdown_handshake: true,
          stdio_shutdown_horizon_ms: invalid,
        },
      });
      await transport.sendAsync({
        jsonrpc: "2.0",
        id: "init-invalid-horizon",
        method: "meta/init",
        params: {},
      });
      assert.equal(
        (transport as any)._shutdownHorizonMs,
        PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS,
      );
    }
  });

  it("rejects new RPC admission while a stop is in flight", async () => {
    const transport = new PersistentTransport("unused-test-gateway");
    (transport as any)._stopping = new Promise<void>(() => {});

    await assert.rejects(
      transport.sendAsync({
        jsonrpc: "2.0",
        id: "during-stop",
        method: "meta/status",
        params: {},
      }),
      /persistent transport is stopping/,
    );
    assert.throws(
      () => transport.start(),
      /persistent transport is stopping/,
    );
  });

  it("honors the negotiated horizon and keeps stdin open for a gated callback", async () => {
    const fake = fakeChild();
    const transport = new PersistentTransport("unused-test-gateway");
    (transport as any)._process = fake.child;
    (transport as any)._supportsShutdownHandshake = true;
    (transport as any)._shutdownHorizonMs = 321_000;

    let handshakeObserved = false;
    let releaseHandshake!: () => void;
    let markHandshakeEntered!: () => void;
    const handshakeEntered = new Promise<void>((resolve) => {
      markHandshakeEntered = resolve;
    });
    const handshakeGate = new Promise<void>((resolve) => {
      releaseHandshake = resolve;
    });
    (transport as any)._sendAsyncWithTimeout = async (
      request: Record<string, unknown>,
      timeoutMs: number,
      expectedChild: ChildProcess,
    ) => {
      assert.equal(fake.stdinEnded, 0);
      assert.equal(request.method, "meta/shutdown");
      assert.match(String(request.id), /^meta-shutdown-/);
      assert.equal(timeoutMs, 321_000);
      assert.equal(expectedChild, fake.child);
      handshakeObserved = true;
      markHandshakeEntered();
      await handshakeGate;
      (fake.child as any).exitCode = 0;
      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          shutdown: true,
          runtime_cleanup_completed: true,
        },
      };
    };

    const stopping = transport.stop();
    await handshakeEntered;
    assert.equal(fake.stdinEnded, 0);
    releaseHandshake();
    await stopping;

    assert.equal(handshakeObserved, true);
    assert.equal(fake.stdinEnded, 1);
    assert.equal((transport as any)._process, null);
  });

  it("keeps older gateways on the EOF shutdown protocol", async () => {
    const fake = fakeChild();
    const transport = new PersistentTransport("unused-test-gateway");
    (transport as any)._process = fake.child;
    (fake.child as any).exitCode = 0;

    let handshakeAttempts = 0;
    (transport as any)._sendAsyncWithTimeout = async () => {
      handshakeAttempts += 1;
    };

    await transport.stop();

    assert.equal(handshakeAttempts, 0);
    assert.equal(fake.stdinEnded, 1);
    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: 0,
      signal: null,
      escalation: "none",
      nativeShutdown: "not_negotiated",
    });
  });

  it("rejects every non-true cleanup attestation only after reaping", async () => {
    const invalidResults: Array<Record<string, unknown>> = [
      { shutdown: true },
      { shutdown: true, runtime_cleanup_completed: null },
      { shutdown: true, runtime_cleanup_completed: "true" },
      { shutdown: true, runtime_cleanup_completed: false },
      { shutdown: true, runtime_cleanup_completed: 1 },
      { shutdown: false, runtime_cleanup_completed: true },
      { shutdown: false, runtime_cleanup_completed: false },
      {
        shutdown: false,
        runtime_cleanup_completed: false,
        runtime_cleanup_retained_until_exit: "true",
      },
      {
        shutdown: false,
        runtime_cleanup_completed: null,
        runtime_cleanup_retained_until_exit: true,
      },
    ];

    for (const result of invalidResults) {
      const fake = fakeChild();
      const transport = new PersistentTransport("unused-test-gateway");
      (transport as any)._process = fake.child;
      (transport as any)._supportsShutdownHandshake = true;
      (transport as any)._shutdownHorizonMs = 321_000;
      (transport as any)._sendAsyncWithTimeout = async () => {
        return {
          jsonrpc: "2.0",
          id: "shutdown-failed",
          result,
        };
      };

      let rejected = false;
      const stopping = transport.stop();
      void stopping.catch(() => {
        rejected = true;
      });
      await new Promise<void>((resolve) => setImmediate(resolve));

      assert.equal(fake.stdinEnded, 1);
      assert.equal(rejected, false);
      (fake.child as any).exitCode = 0;
      (fake.child as any).emit("exit", 0, null);
      await assert.rejects(
        stopping,
        /gateway shutdown failed after bounded cleanup/,
      );

      assert.equal(rejected, true);
      assert.equal((transport as any)._process, null);
      // The process outcome and the gateway's refusal stay separate facts.
      assert.deepEqual(transport.lastTermination(), {
        state: "exited",
        exitCode: 0,
        signal: null,
        escalation: "none",
        nativeShutdown: "refused",
      });
    }
  });

  it("reaps without failing on process-exit child mob retention", async () => {
    const fake = fakeChild();
    const transport = new PersistentTransport("unused-test-gateway");
    (transport as any)._process = fake.child;
    (transport as any)._supportsShutdownHandshake = true;
    (transport as any)._shutdownHorizonMs = 321_000;
    (transport as any)._sendAsyncWithTimeout = async () => {
      (fake.child as any).exitCode = 0;
      // Cleanup is still incomplete; the only cause is durable child mobs
      // held until the gateway exits, which the reap releases.
      return {
        jsonrpc: "2.0",
        id: "shutdown-retained",
        result: {
          shutdown: false,
          runtime_cleanup_completed: false,
          runtime_cleanup_retained_until_exit: true,
        },
      };
    };

    await transport.stop();

    assert.equal(fake.stdinEnded, 1);
    assert.equal((transport as any)._process, null);
    assert.equal(transport.lastTermination()?.nativeShutdown, "retained_until_exit");
    assert.equal(transport.lastTermination()?.state, "exited");
  });

  it("allows the gateway its full negotiated-safe drain without signaling it", async () => {
    const fake = fakeChild();
    const waits: number[] = [];

    const escalation = await stopChildProcess(fake.child, async (_child, timeoutMs) => {
      waits.push(timeoutMs);
      return true;
    });

    assert.equal(PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS, 337_000);
    assert.equal(fake.stdinEnded, 1);
    assert.deepEqual(waits, [337_000]);
    assert.deepEqual(fake.signals, []);
    assert.equal(escalation, "none");
  });

  it("uses bounded SIGTERM then SIGKILL fallback when drain stalls", async () => {
    const fake = fakeChild();
    const waits: number[] = [];
    const outcomes = [false, false, true];

    const escalation = await stopChildProcess(fake.child, async (_child, timeoutMs) => {
      waits.push(timeoutMs);
      return outcomes.shift() ?? true;
    });

    assert.equal(fake.stdinEnded, 1);
    assert.deepEqual(waits, [337_000, 5_000, 5_000]);
    assert.deepEqual(fake.signals, ["SIGTERM", "SIGKILL"]);
    assert.equal(escalation, "sigkill");
  });

  it("rejects when the gateway remains live after bounded SIGKILL cleanup", async () => {
    const fake = fakeChild();
    const waits: number[] = [];
    const progress = { escalation: "none" as "none" | "sigterm" | "sigkill" };

    await assert.rejects(
      stopChildProcess(
        fake.child,
        async (_child, timeoutMs) => {
          waits.push(timeoutMs);
          return false;
        },
        PERSISTENT_TRANSPORT_SHUTDOWN_GRACE_MS,
        progress,
      ),
      /gateway process did not terminate after bounded cleanup/,
    );

    assert.equal(fake.stdinEnded, 1);
    assert.deepEqual(waits, [337_000, 5_000, 5_000]);
    assert.deepEqual(fake.signals, ["SIGTERM", "SIGKILL"]);
    assert.equal(progress.escalation, "sigkill", "the attempt is known when cleanup fails");
  });

  it("does not force-kill a gateway that exits after SIGTERM", async () => {
    const fake = fakeChild();
    const waits: number[] = [];
    const outcomes = [false, true];

    const escalation = await stopChildProcess(fake.child, async (_child, timeoutMs) => {
      waits.push(timeoutMs);
      return outcomes.shift() ?? true;
    });

    assert.deepEqual(waits, [337_000, 5_000]);
    assert.deepEqual(fake.signals, ["SIGTERM"]);
    assert.equal(escalation, "sigterm");
  });
});

describe("persistent transport termination record", () => {
  /** A transport holding `fake` as its started child. */
  function holding(fake: FakeChild): PersistentTransport {
    const transport = new PersistentTransport("unused-test-gateway");
    (transport as any)._process = fake.child;
    return transport;
  }

  it("records an acknowledged shutdown and a clean exit", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    (transport as any)._supportsShutdownHandshake = true;
    (transport as any)._sendAsyncWithTimeout = async () => {
      (fake.child as any).exitCode = 0;
      return {
        jsonrpc: "2.0",
        id: "shutdown-ok",
        result: { shutdown: true, runtime_cleanup_completed: true },
      };
    };
    assert.equal(transport.lastTermination(), null, "no record before a stop");

    await transport.stop();

    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: 0,
      signal: null,
      escalation: "none",
      nativeShutdown: "acknowledged",
    });
  });

  it("records an unanswered shutdown apart from the observed exit", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    (transport as any)._supportsShutdownHandshake = true;
    (transport as any)._sendAsyncWithTimeout = async () => {
      (fake.child as any).exitCode = 0;
      throw new Error("persistent transport: timeout after 1ms");
    };

    await assert.rejects(transport.stop(), /gateway shutdown failed after bounded cleanup/);

    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: 0,
      signal: null,
      escalation: "none",
      nativeShutdown: "unanswered",
    });
  });

  it("records a nonzero exit and a signal exit exactly", async () => {
    const nonzero = fakeChild();
    (nonzero.child as any).exitCode = 3;
    const first = holding(nonzero);
    await first.stop();
    assert.deepEqual(first.lastTermination(), {
      state: "exited",
      exitCode: 3,
      signal: null,
      escalation: "none",
      nativeShutdown: "not_negotiated",
    });

    const signalled = fakeChild();
    (signalled.child as any).signalCode = "SIGSEGV";
    const second = holding(signalled);
    await second.stop();
    assert.deepEqual(second.lastTermination(), {
      state: "exited",
      exitCode: null,
      signal: "SIGSEGV",
      escalation: "none",
      nativeShutdown: "not_negotiated",
    });
  });

  it("records the attempted escalation with the exit it observed", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    const outcomes = [false, true];
    (transport as any)._waitForExit = async () => {
      const exited = outcomes.shift() ?? true;
      if (exited) (fake.child as any).signalCode = "SIGTERM";
      return exited;
    };

    await transport.stop();

    assert.deepEqual(fake.signals, ["SIGTERM"]);
    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: null,
      signal: "SIGTERM",
      escalation: "sigterm",
      nativeShutdown: "not_negotiated",
    });
  });

  it("keeps an unterminated child, records unknown, and a later stop records its exit", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    (transport as any)._waitForExit = async () => false;

    await assert.rejects(transport.stop(), /did not terminate after bounded cleanup/);

    assert.deepEqual(transport.lastTermination(), {
      state: "unknown",
      exitCode: null,
      signal: null,
      escalation: "sigkill",
      nativeShutdown: "not_negotiated",
    });
    assert.equal((transport as any)._process, fake.child, "the live child stays owned");
    assert.equal(transport.isRunning(), true);

    // The same stop owner, retried, observes the same child's exit; the
    // earlier attempt against this child is still reported.
    (fake.child as any).exitCode = 0;
    (transport as any)._waitForExit = async () => true;
    await transport.stop();

    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: 0,
      signal: null,
      escalation: "sigkill",
      nativeShutdown: "not_negotiated",
    });
    assert.equal((transport as any)._process, null);
  });

  it("a later stop finishes the same child without asking it to shut down again", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    (transport as any)._supportsShutdownHandshake = true;
    (transport as any)._shutdownHorizonMs = 30_000;
    let asked = 0;
    (transport as any)._sendAsyncWithTimeout = async () => {
      asked += 1;
      return {
        jsonrpc: "2.0",
        id: "shutdown-ok",
        result: { shutdown: true, runtime_cleanup_completed: true },
      };
    };
    const waits: number[] = [];
    (transport as any)._waitForExit = async (_child: ChildProcess, timeoutMs: number) => {
      waits.push(timeoutMs);
      return false;
    };

    await assert.rejects(transport.stop(), /did not terminate after bounded cleanup/);
    assert.ok(waits[0] > 29_000, "the first stop gives the child its horizon");
    assert.deepEqual(transport.lastTermination(), {
      state: "unknown",
      exitCode: null,
      signal: null,
      escalation: "sigkill",
      nativeShutdown: "acknowledged",
    });

    // The request already used (and ended) the child's stdin, and the child
    // already had its horizon: the retried stop keeps the answer and goes
    // straight to the signals.
    waits.length = 0;
    (transport as any)._waitForExit = async (child: ChildProcess, timeoutMs: number) => {
      waits.push(timeoutMs);
      if (waits.length > 1) (child as any).signalCode = "SIGKILL";
      return waits.length > 1;
    };
    await transport.stop();

    assert.equal(asked, 1, "the child was asked once");
    assert.equal(waits[0], 0, "the retried stop does not wait out the horizon again");
    assert.deepEqual(fake.signals, ["SIGTERM", "SIGKILL", "SIGTERM"]);
    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: null,
      signal: "SIGKILL",
      escalation: "sigkill",
      nativeShutdown: "acknowledged",
    });
    assert.equal((transport as any)._process, null);
  });

  it("a refused shutdown is reported by the stop that reaps the child", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    (transport as any)._supportsShutdownHandshake = true;
    let asked = 0;
    (transport as any)._sendAsyncWithTimeout = async () => {
      asked += 1;
      return {
        jsonrpc: "2.0",
        id: "shutdown-refused",
        error: { code: -32000, message: "cleanup refused" },
      };
    };
    (transport as any)._waitForExit = async () => false;

    await assert.rejects(transport.stop(), /did not terminate after bounded cleanup/);
    assert.equal(transport.lastTermination()?.state, "unknown");
    assert.equal(transport.lastTermination()?.nativeShutdown, "refused");

    (fake.child as any).exitCode = 0;
    (transport as any)._waitForExit = async () => true;
    await assert.rejects(
      transport.stop(),
      /gateway shutdown failed after bounded cleanup: .*cleanup refused/,
    );
    const reaped = {
      state: "exited",
      exitCode: 0,
      signal: null,
      escalation: "sigkill",
      nativeShutdown: "refused",
    };
    assert.equal(asked, 1, "the child was asked once");
    assert.deepEqual(transport.lastTermination(), reaped);

    // The child is released: a further stop has nothing left to stop.
    await transport.stop();
    assert.deepEqual(transport.lastTermination(), reaped);
  });

  it("records an exit that lands after the last bounded wait as that exit", async () => {
    const fake = fakeChild();
    const transport = holding(fake);
    let waits = 0;
    (transport as any)._waitForExit = async () => {
      waits += 1;
      // The process ends just after the SIGKILL wait gave up.
      if (waits === 3) (fake.child as any).signalCode = "SIGKILL";
      return false;
    };

    await assert.rejects(transport.stop(), /did not terminate after bounded cleanup/);

    assert.deepEqual(transport.lastTermination(), {
      state: "exited",
      exitCode: null,
      signal: "SIGKILL",
      escalation: "sigkill",
      nativeShutdown: "not_negotiated",
    });
    assert.equal((transport as any)._process, null, "an exited child is released");
  });

  it("a stop with no started child records not_started and never overwrites a real record", async () => {
    const never = new PersistentTransport("unused-test-gateway");
    await never.stop();
    assert.deepEqual(never.lastTermination(), {
      state: "not_started",
      exitCode: null,
      signal: null,
      escalation: "none",
      nativeShutdown: "not_negotiated",
    });

    const fake = fakeChild();
    (fake.child as any).exitCode = 0;
    const transport = holding(fake);
    await transport.stop();
    const recorded = transport.lastTermination();
    assert.equal(recorded?.state, "exited");
    await transport.stop();
    assert.equal(transport.lastTermination(), recorded, "a redundant stop keeps the record");
  });

  it("a child whose spawn failed records not_started", async () => {
    const fake = fakeChild();
    (fake.child as any).pid = undefined;
    // Node reports a failed spawn's errno as a negative exit code.
    (fake.child as any).exitCode = -2;
    const transport = holding(fake);

    await transport.stop();

    assert.equal(transport.lastTermination()?.state, "not_started");
    assert.equal(transport.lastTermination()?.exitCode, null);
  });
});
