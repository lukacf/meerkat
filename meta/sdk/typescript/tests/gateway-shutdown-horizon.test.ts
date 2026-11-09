/**
 * The runtime reports the gateway shutdown horizon its transport accepted
 * (twin of the Python test_gateway_shutdown_horizon_accessor).
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { Meta, MetaRuntime, PersistentTransport } from "../dist/index.js";

async function initialized(result: Record<string, unknown>): Promise<PersistentTransport> {
  const transport = new PersistentTransport("unused-test-gateway");
  (transport as any)._sendAsyncWithTimeout = async () => ({ jsonrpc: "2.0", id: "init", result });
  await transport.sendAsync({ jsonrpc: "2.0", id: "init", method: "meta/init", params: {} });
  return transport;
}

function runtimeWith(transport?: PersistentTransport): MetaRuntime {
  return new MetaRuntime((Meta.builder() as any)._config, transport);
}

describe("gateway shutdown horizon", () => {
  it("reports the horizon the transport accepted, and nothing it refused", async () => {
    const cases: Array<[Record<string, unknown>, number | null]> = [
      [{ stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 392_000 }, 392_000],
      [{ stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 432_000 }, 432_000],
      [{ stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: "392000" }, null],
      [{ stdio_shutdown_handshake: true }, null],
      [{ stdio_shutdown_horizon_ms: 392_000 }, null],
    ];
    for (const [result, expected] of cases) {
      const runtime = runtimeWith(await initialized(result));
      assert.equal((runtime as any).gatewayShutdownHorizonMs, expected, JSON.stringify(result));
    }
  });

  it("does not let a replacement gateway inherit the previous horizon", async () => {
    const transport = await initialized({ stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 432_000 });
    assert.equal((runtimeWith(transport) as any).gatewayShutdownHorizonMs, 432_000);
    // The replacement child has negotiated nothing yet (this one cannot start).
    transport.start();
    assert.equal((runtimeWith(transport) as any).gatewayShutdownHorizonMs, null);
    await new Promise((resolve) => setTimeout(resolve, 50));
  });

  it("reports no horizon before a gateway is negotiated", () => {
    assert.equal((runtimeWith() as any).gatewayShutdownHorizonMs, null);
    assert.equal(
      (runtimeWith(new PersistentTransport("unused-test-gateway")) as any).gatewayShutdownHorizonMs,
      null,
    );
  });
});
