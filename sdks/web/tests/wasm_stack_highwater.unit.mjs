// The shadow-stack high-water probe the packed-package smoke runs
// (scripts/wasm-stack-highwater.mjs).

import assert from "node:assert/strict";
import test from "node:test";

import {
  TURN_STACK_BUDGET_BYTES,
  assertStackWithinBudget,
  paintIdleStack,
  stackHighWater,
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
    /over its 2097152-byte budget/,
  );
});
