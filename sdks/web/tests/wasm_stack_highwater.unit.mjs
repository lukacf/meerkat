// The shadow-stack high-water probe the packed-package smoke runs
// (scripts/wasm-stack-highwater.mjs).

import assert from "node:assert/strict";
import test from "node:test";

import {
  TURN_STACK_BUDGET_BYTES,
  TURN_STACK_FLOOR_BYTES,
  assertStackProbeObservedTurn,
  assertStackWithinBudget,
  paintIdleStack,
  stackHighWater,
  turnStackBudget,
} from "../scripts/wasm-stack-highwater.mjs";

const KiB = 1024;

function memory(bytes) {
  return { buffer: new ArrayBuffer(bytes) };
}

test("the deepest overwritten byte below the resting pointer is the high-water", () => {
  const stack = { initialStackPointer: 256 * KiB, stackBytes: 256 * KiB, layout: "stack-first" };
  const mem = memory(512 * KiB);
  paintIdleStack(mem, stack);
  assert.equal(stackHighWater(mem, stack), 0, "nothing ran");
  // A run whose frames reached 100 KiB below the pointer, with holes above.
  const view = new Uint8Array(mem.buffer);
  view[stack.initialStackPointer - 100 * KiB] = 0;
  view[stack.initialStackPointer - 10 * KiB] = 7;
  assert.equal(stackHighWater(mem, stack), 100 * KiB);
});

test("painting stays below the resting stack pointer and inside the free stack", () => {
  const stack = { initialStackPointer: 256 * KiB, stackBytes: 128 * KiB, layout: "stack-first" };
  const mem = memory(512 * KiB);
  const view = new Uint8Array(mem.buffer);
  view.fill(1);
  paintIdleStack(mem, stack);
  const base = stack.initialStackPointer - stack.stackBytes;
  assert.equal(view[base - 1], 1, "nothing below the stack's base is painted");
  assert.notEqual(view[base], 1, "the stack's base is painted");
  // The resting pointer's own byte, everything above it, and the margin just
  // below it (where the next call's first frame lands) keep their contents.
  for (let address = stack.initialStackPointer - 64; address < view.length; address++) {
    assert.equal(view[address], 1, `byte ${address} at or above the unpainted margin is untouched`);
  }
  assert.notEqual(view[stack.initialStackPointer - 65], 1, "the free stack below the margin is painted");
});

test("a data-first stack is refused: its free stack may overlap .bss", () => {
  const stack = { initialStackPointer: 384 * KiB, stackBytes: 128 * KiB, layout: "data-first" };
  const mem = memory(512 * KiB);
  const view = new Uint8Array(mem.buffer);
  view.fill(1);
  assert.throws(() => paintIdleStack(mem, stack), /cannot paint a data-first wasm stack/);
  assert.ok(view.every((byte) => byte === 1), "nothing was painted");
  assert.throws(
    () => paintIdleStack(mem, { initialStackPointer: 384 * KiB, stackBytes: 128 * KiB }),
    /cannot paint a unknown-layout wasm stack/,
  );
});

test("a high-water under the floor means the probe missed the turn", () => {
  assert.equal(assertStackProbeObservedTurn(TURN_STACK_FLOOR_BYTES), TURN_STACK_FLOOR_BYTES);
  assert.throws(() => assertStackProbeObservedTurn(0), /below the 16384-byte floor/);
  assert.throws(() => assertStackProbeObservedTurn(TURN_STACK_FLOOR_BYTES - 1), /below the 16384-byte floor/);
  assert.ok(
    TURN_STACK_FLOOR_BYTES < TURN_STACK_BUDGET_BYTES / 8,
    "the floor sits far below a real release-build turn (about 116 KB)",
  );
});

test("a turn over its budget is refused, at or under it passes", () => {
  assert.equal(assertStackWithinBudget(TURN_STACK_BUDGET_BYTES), TURN_STACK_BUDGET_BYTES);
  assert.throws(
    () => assertStackWithinBudget(TURN_STACK_BUDGET_BYTES + 1),
    /over its 524288-byte budget for this build \(profile release, opt-level s\)/,
  );
});

test("the budget is enforced for the release build at opt-level s, and fails closed without settings", () => {
  const release = turnStackBudget({ profile: "release", opt_level: "s", codegen_units: "256", wasm_opt: true });
  assert.equal(release.enforced, true);
  assert.equal(release.budget, TURN_STACK_BUDGET_BYTES);
  assert.equal(release.label, "profile release, opt-level s");
  const unrecorded = turnStackBudget(null);
  assert.equal(unrecorded.enforced, true, "a package without recorded settings is budgeted as release");
  assert.equal(unrecorded.budget, TURN_STACK_BUDGET_BYTES);
});

test("a dev-profile or overridden build is logged, not budgeted", () => {
  const dev = turnStackBudget({ profile: "dev", opt_level: null, codegen_units: null, wasm_opt: true });
  assert.equal(dev.enforced, false);
  assert.equal(dev.label, "profile dev, opt-level Cargo's profile default");
  const overridden = turnStackBudget({ profile: "release", opt_level: "0", codegen_units: "256", wasm_opt: true });
  assert.equal(overridden.enforced, false);
  assert.equal(overridden.label, "profile release, opt-level 0");
});
