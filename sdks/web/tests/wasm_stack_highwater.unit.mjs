// The shadow-stack high-water probe the packed-package smoke runs
// (scripts/wasm-stack-highwater.mjs).

import assert from "node:assert/strict";
import test from "node:test";

import {
  TURN_STACK_BUDGET_BYTES,
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

test("a data-first stack is measured from its own base, not address zero", () => {
  const stack = { initialStackPointer: 384 * KiB, stackBytes: 128 * KiB, layout: "data-first" };
  const mem = memory(512 * KiB);
  const view = new Uint8Array(mem.buffer);
  view.fill(1, 0, 256 * KiB); // data below the stack is left alone
  paintIdleStack(mem, stack);
  assert.equal(view[256 * KiB - 1], 1, "the data below the stack is untouched");
  assert.equal(stackHighWater(mem, stack), 0);
  view[stack.initialStackPointer - 64 * KiB] = 0;
  assert.equal(stackHighWater(mem, stack), 64 * KiB);
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
