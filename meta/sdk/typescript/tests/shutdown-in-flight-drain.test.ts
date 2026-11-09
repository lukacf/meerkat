/**
 * Tests for `shutdownInFlightDrainMs`: the builder carries the window as the
 * top-level `meta/init` parameter `shutdown_in_flight_drain_ms`, sends
 * nothing by default, and refuses a value the gateway would refuse.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { Meta, MetaRuntime } from "../dist/index.js";

function initParams(builder: unknown): Record<string, any> {
  const config = (builder as { _config: unknown })._config;
  const rt = new MetaRuntime(config as ConstructorParameters<typeof MetaRuntime>[0]);
  return (rt as any)._buildInitParams();
}

describe("shutdownInFlightDrainMs", () => {
  it("is omitted by default", () => {
    const params = initParams(Meta.builder());
    assert.equal(params.shutdown_in_flight_drain_ms, undefined);
  });

  it("is sent as a top-level init parameter", () => {
    const builder = Meta.builder();
    assert.equal(builder.shutdownInFlightDrainMs(60_000), builder);
    const params = initParams(builder);
    assert.equal(params.shutdown_in_flight_drain_ms, 60_000);
    assert.equal(params.runtime_options.shutdown_in_flight_drain_ms, undefined);
  });

  it("sends an explicit 0 and the 5 minute maximum", () => {
    assert.equal(
      initParams(Meta.builder().shutdownInFlightDrainMs(0)).shutdown_in_flight_drain_ms,
      0,
    );
    assert.equal(
      initParams(Meta.builder().shutdownInFlightDrainMs(300_000)).shutdown_in_flight_drain_ms,
      300_000,
    );
  });

  it("refuses a negative, fractional or too large window", () => {
    for (const windowMs of [-1, 1.5, 300_001, Number.NaN]) {
      assert.throws(
        () => Meta.builder().shutdownInFlightDrainMs(windowMs),
        /shutdownInFlightDrainMs must be an integer of milliseconds in 0\.\.=300000/,
        String(windowMs),
      );
    }
  });
});
