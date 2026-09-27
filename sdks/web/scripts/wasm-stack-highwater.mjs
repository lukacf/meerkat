// Shadow-stack high-water of a wasm32 module's run, by stack painting.
//
// The @rkat/web runtime links an 8 MiB wasm stack (scripts/wasm-stack.mjs).
// A change to the agent loop can grow a turn's stack by hundreds of KiB: at
// the release build's former opt-level 0, where every awaited future kept its
// own slot in its parent's poll frame, model fallback took a turn from 880 KB
// to 1.3 MB between 0.8.36 and 0.8.37. This is the web analogue of the native
// worker-stack canaries: the packed-package smoke paints the idle stack, runs
// one turn, and measures how deep the turn reached.
//
// Between calls into the module nothing runs on the shadow stack, so the stack
// pointer rests at its initial value. Everything below it is free and can be
// painted with a pattern; after the run, the lowest address whose pattern was
// overwritten is how deep the stack went.

/**
 * The release build's turn stack budget: fail the smoke when one turn needs
 * more. The release build (profile release, opt-level "s") measures about
 * 133 KB; the budget leaves roughly four times that before a frame's growth
 * fails the smoke. Other builds (the dev profile, or an overridden
 * opt-level) have much larger frames; see {@link turnStackBudget}.
 */
export const TURN_STACK_BUDGET_BYTES = 512 * 1024;

/** The profile and opt-level {@link TURN_STACK_BUDGET_BYTES} was measured for. */
export const BUDGETED_BUILD = Object.freeze({ profile: "release", opt_level: "s" });

/**
 * Which budget applies to a module built with `build` (the `build` settings
 * build-wasm.mjs records in wasm/.meerkat-wasm-build.json):
 *   - the release build (profile release, opt-level "s"): the budget is
 *     enforced;
 *   - no recorded settings (a package from before they were recorded): also
 *     enforced, so a release package whose record went missing still fails
 *     closed;
 *   - any other build: nothing is enforced, and the high-water is only
 *     logged. At opt-level 0 a turn alone needs about 1.46 MB.
 * `label` names the build in messages.
 */
export function turnStackBudget(build) {
  if (build == null) {
    return {
      enforced: true,
      budget: TURN_STACK_BUDGET_BYTES,
      label: "a build without recorded settings, budgeted as the release build",
    };
  }
  const label = `profile ${build.profile}, opt-level ${build.opt_level ?? "Cargo's profile default"}`;
  if (build.profile === BUDGETED_BUILD.profile && build.opt_level === BUDGETED_BUILD.opt_level) {
    return { enforced: true, budget: TURN_STACK_BUDGET_BYTES, label };
  }
  return { enforced: false, budget: null, label };
}

const PAINT = 0xa5;
// Bytes just below the resting stack pointer are left unpainted: the next
// call into the module writes its first frame there at once.
const TOP_MARGIN = 64;

/** The free stack of `stack` (from `wasmStack`): `[base, top)`. */
function freeStack(stack) {
  const top = stack.initialStackPointer;
  return { base: top - stack.stackBytes, top };
}

/** Paint the free stack of `stack` in `memory` (a `WebAssembly.Memory`). */
export function paintIdleStack(memory, stack) {
  const { base, top } = freeStack(stack);
  new Uint8Array(memory.buffer).fill(PAINT, base, top - TOP_MARGIN);
}

/**
 * How many bytes below its resting pointer the stack reached since it was
 * painted. A result equal to the whole stack means the run used all of it.
 */
export function stackHighWater(memory, stack) {
  const { base, top } = freeStack(stack);
  const bytes = new Uint8Array(memory.buffer);
  for (let address = base; address < top - TOP_MARGIN; address++) {
    if (bytes[address] !== PAINT) {
      return top - address;
    }
  }
  return 0;
}

/**
 * Throws when `highWater` exceeds `budget`. `build` names the build the budget
 * applies to (profile and opt-level).
 */
export function assertStackWithinBudget(
  highWater,
  budget = TURN_STACK_BUDGET_BYTES,
  label = "turn",
  build = `profile ${BUDGETED_BUILD.profile}, opt-level ${BUDGETED_BUILD.opt_level}`,
) {
  if (highWater > budget) {
    throw new Error(
      `${label}: shadow-stack high-water ${highWater} bytes is over its ${budget}-byte budget ` +
        `for this build (${build}); a frame on the turn's path grew ` +
        "(see scripts/wasm-stack-highwater.mjs)",
    );
  }
  return highWater;
}
