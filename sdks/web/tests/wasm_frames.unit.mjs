// The shadow-stack frame reader and the spawn frame budget
// (scripts/wasm-frames.mjs, issue #1230). At opt-level "s" the largest frames
// of the runtime were `tokio_with_wasm::spawn` wrapper instances, each about
// the size of the future it was handed, up to 125,440 bytes.

import assert from "node:assert/strict";
import test from "node:test";

import {
  SPAWN_FRAME_BUDGET_BYTES,
  assertSpawnFrameBudget,
  spawnWrapperFrames,
  wasmFrames,
} from "../scripts/wasm-frames.mjs";

function u32(value) {
  const bytes = [];
  do {
    let byte = value & 0x7f;
    value = Math.floor(value / 128);
    if (value !== 0) byte |= 0x80;
    bytes.push(byte);
  } while (value !== 0);
  return bytes;
}

function i32(value) {
  const bytes = [];
  for (;;) {
    const byte = value & 0x7f;
    value >>= 7;
    const done = (value === 0 && (byte & 0x40) === 0) || (value === -1 && (byte & 0x40) !== 0);
    bytes.push(done ? byte : byte | 0x80);
    if (done) return bytes;
  }
}

function name(text) {
  const encoded = [...new TextEncoder().encode(text)];
  return [...u32(encoded.length), ...encoded];
}

function section(id, body) {
  return [id, ...u32(body.length), ...body];
}

function vector(items) {
  return [...u32(items.length), ...items.flat()];
}

const STACK_POINTER = 0;
const EIGHT_MIB = 8 * 1024 * 1024;

// One imported function, so defined function indices start at 1.
const importFunction = section(2, vector([[...name("env"), ...name("host"), 0, ...u32(0)]]));

// Global 0 is the stack pointer; global 1 an immutable address.
const globals = section(
  6,
  vector([
    [0x7f, 1, 0x41, ...i32(EIGHT_MIB), 0x0b],
    [0x7f, 0, 0x41, ...i32(EIGHT_MIB + 64), 0x0b],
  ]),
);

// wasm-ld's prologue: global.get $sp; i32.const N; i32.sub; local.tee 0;
// global.set $sp.
function framed(frameBytes) {
  return [
    ...vector([[1, 0x7f]]),
    0x23, ...u32(STACK_POINTER),
    0x41, ...i32(frameBytes),
    0x6b,
    0x22, 0,
    0x24, ...u32(STACK_POINTER),
    0x0b,
  ];
}

// A frame that moves its stack pointer copy through a local first.
function framedThroughLocal(frameBytes) {
  return [
    ...vector([[2, 0x7f]]),
    0x23, ...u32(STACK_POINTER),
    0x21, 1,
    0x20, 1,
    0x41, ...i32(frameBytes),
    0x6b,
    0x0b,
  ];
}

// A body that subtracts from something other than the stack pointer.
const notAFrame = [...vector([]), 0x23, ...u32(1), 0x41, ...i32(4096), 0x6b, 0x1a, 0x0b];
const leaf = [...vector([]), 0x41, ...i32(7), 0x1a, 0x0b];

function code(bodies) {
  return section(10, vector(bodies.map((body) => [...u32(body.length), ...body])));
}

function names(functions) {
  const functionSub = vector(functions.map(([index, text]) => [...u32(index), ...name(text)]));
  const globalSub = vector([[...u32(STACK_POINTER), ...name("__stack_pointer")]]);
  return section(0, [
    ...name("name"),
    1, ...u32(functionSub.length), ...functionSub,
    7, ...u32(globalSub.length), ...globalSub,
  ]);
}

function module(...sections) {
  return new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, ...sections.flat()]);
}

// Names as wasm-ld leaves them: legacy-mangled, and v0 with the spawned type.
const LEGACY_SPAWN_CLOSURE =
  "_ZN15tokio_with_wasm4glue4task5spawn28_$u7b$$u7b$closure$u7d$$u7d$17hd88a10a57481c50bE";
const V0_SPAWN_CLOSURE =
  "_RNCINvNtNtCshZnAL0cBRH_15tokio_with_wasm4glue4task5spawnNCNvNtCsbVvnB02KdSl_15meerkat_runtime11comms_drain17spawn_comms_drain0uE0BY_";
const SPAWN_BLOCKING = "_ZN15tokio_with_wasm4glue4task14spawn_blocking17h0123456789abcdefE";
const JOIN_SET_SPAWN = "_ZN15tokio_with_wasm4glue4task8join_set16JoinSet$LT$T$GT$5spawn17h0123456789abcdefE";
const AGENT_POLL = "_ZN12meerkat_core5agent3run28_$u7b$$u7b$closure$u7d$$u7d$17h0123456789abcdefE";

function runtimeShaped(spawnFrame) {
  return module(
    importFunction,
    globals,
    code([
      framed(spawnFrame),
      framedThroughLocal(300),
      framed(90_000),
      framed(200_000),
      framed(200_000),
      notAFrame,
      leaf,
    ]),
    names([
      [0, "host"],
      [1, LEGACY_SPAWN_CLOSURE],
      [2, V0_SPAWN_CLOSURE],
      [3, AGENT_POLL],
      [4, SPAWN_BLOCKING],
      [5, JOIN_SET_SPAWN],
      [6, "not_a_frame"],
      [7, "leaf"],
    ]),
  );
}

test("frames are read from each prologue, named, and listed largest first", () => {
  const frames = wasmFrames(runtimeShaped(125_440));
  // Function 0 is the import; 6 subtracts from another global, 7 has no frame.
  assert.deepEqual(frames, [
    { index: 4, name: SPAWN_BLOCKING, frameBytes: 200_000 },
    { index: 5, name: JOIN_SET_SPAWN, frameBytes: 200_000 },
    { index: 1, name: LEGACY_SPAWN_CLOSURE, frameBytes: 125_440 },
    { index: 3, name: AGENT_POLL, frameBytes: 90_000 },
    { index: 2, name: V0_SPAWN_CLOSURE, frameBytes: 300 },
  ]);
});

test("only tokio_with_wasm::spawn and its closures count as spawn wrappers", () => {
  const spawns = spawnWrapperFrames(wasmFrames(runtimeShaped(125_440)));
  assert.deepEqual(
    spawns.map((frame) => frame.name),
    [LEGACY_SPAWN_CLOSURE, V0_SPAWN_CLOSURE],
  );
});

test("the unboxed spawn wrapper frame of issue #1230 is over budget", () => {
  assert.throws(
    () => assertSpawnFrameBudget(runtimeShaped(125_440), undefined, "runtime"),
    new RegExp(
      `runtime: 1 spawn wrapper frame\\(s\\) over the ${SPAWN_FRAME_BUDGET_BYTES}-byte budget:\\n {2}125440 bytes: _ZN15tokio_with_wasm`,
    ),
  );
});

test("boxed spawn wrapper frames pass, whatever the frames of other functions", () => {
  const result = assertSpawnFrameBudget(runtimeShaped(208));
  assert.equal(result.largest.frameBytes, 300);
  assert.equal(result.instances, 2);
  // The largest boxed instance of the release build.
  assert.ok(SPAWN_FRAME_BUDGET_BYTES >= 4512);
  assert.equal(assertSpawnFrameBudget(runtimeShaped(5000), 5000).largest.frameBytes, 5000);
});

test("a module the budget cannot see into fails instead of passing", () => {
  const unnamed = module(importFunction, globals, code([framed(125_440)]));
  assert.throws(() => assertSpawnFrameBudget(unnamed), /no function names/);
  const noSpawn = module(
    importFunction,
    globals,
    code([framed(64)]),
    names([[1, AGENT_POLL]]),
  );
  assert.throws(() => assertSpawnFrameBudget(noSpawn), /no `tokio_with_wasm::spawn` frame found/);
});
