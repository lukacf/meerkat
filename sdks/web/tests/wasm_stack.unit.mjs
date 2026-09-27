// The wasm stack guard and the build's rustflags folding
// (scripts/wasm-stack.mjs, scripts/wasm-rustflags.mjs). Every @rkat/web from
// 0.8.30 through 0.8.44 shipped a 1 MiB wasm stack because an ambient
// RUSTFLAGS replaced the flags that carried `-zstack-size`.

import assert from "node:assert/strict";
import test from "node:test";

import {
  REQUIRED_WASM_STACK_BYTES,
  assertWasmStack,
  wasmStack,
} from "../scripts/wasm-stack.mjs";
import {
  REQUIRED_WASM_RUSTFLAGS,
  effectiveWasmRustflags,
  wasmBuildEnv,
} from "../scripts/wasm-rustflags.mjs";

const MiB = 1024 * 1024;

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

// { mutable, value } -> a defined i32 global with `i32.const value`.
function globalSection(globals) {
  return section(
    6,
    vector(globals.map(({ mutable, value }) => [0x7f, mutable ? 1 : 0, 0x41, ...i32(value), 0x0b])),
  );
}

function exportGlobals(exports) {
  return section(7, vector(exports.map(([exportName, index]) => [...name(exportName), 3, ...u32(index)])));
}

function importGlobal() {
  return section(2, vector([[...name("env"), ...name("imported"), 3, 0x7f, 0]]));
}

function globalNames(names) {
  const sub = vector(names.map(([index, globalName]) => [...u32(index), ...name(globalName)]));
  return section(0, [...name("name"), 7, ...u32(sub.length), ...sub]);
}

// [offset, length] active segments in memory 0.
function dataSection(segments) {
  return section(
    11,
    vector(segments.map(([offset, length]) => [0, 0x41, ...i32(offset), 0x0b, ...u32(length), ...new Array(length).fill(0)])),
  );
}

function module(...sections) {
  return new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, ...sections.flat()]);
}

// The shape of every published @rkat/web module: stack first, the stack
// pointer the only mutable global, wasm-bindgen's exported immutable globals.
function publishedShape(stackBytes) {
  return module(
    globalSection([
      { mutable: true, value: stackBytes },
      { mutable: false, value: stackBytes + 64 },
      { mutable: false, value: stackBytes + 128 },
    ]),
    exportGlobals([
      ["__abort_handler", 1],
      ["__instance_terminated", 2],
    ]),
    dataSection([[stackBytes, 16]]),
  );
}

test("the 1 MiB stack every release from 0.8.30 to 0.8.44 shipped is refused", () => {
  const bytes = publishedShape(1 * MiB);
  assert.deepEqual(wasmStack(bytes), {
    globalIndex: 0,
    initialStackPointer: 1 * MiB,
    layout: "stack-first",
    stackBytes: 1 * MiB,
  });
  assert.throws(() => assertWasmStack(bytes), /1048576 bytes .* below the required 8388608/);
});

test("an 8 MiB stack-first module passes", () => {
  assert.equal(assertWasmStack(publishedShape(8 * MiB)).stackBytes, REQUIRED_WASM_STACK_BYTES);
});

test("with data first the stack is what lies between the data and the stack pointer", () => {
  // 7.5 MiB of data then a 1 MiB stack: the pointer is past 8 MiB, the stack is not.
  const dataBytes = 7.5 * MiB;
  const bytes = module(
    globalSection([{ mutable: true, value: dataBytes + 1 * MiB }]),
    dataSection([
      [1024, 16],
      [dataBytes - 16, 16],
    ]),
  );
  const stack = wasmStack(bytes);
  assert.equal(stack.layout, "data-first");
  assert.equal(stack.stackBytes, 1 * MiB);
  assert.throws(() => assertWasmStack(bytes), /below the required/);
});

test("the name section identifies __stack_pointer among several mutable globals", () => {
  const bytes = module(
    importGlobal(),
    globalSection([
      { mutable: true, value: 64 },
      { mutable: true, value: 8 * MiB },
    ]),
    globalNames([
      [1, "__tls_base"],
      [2, "__stack_pointer"],
    ]),
  );
  // Global indices count the imported global first.
  const stack = wasmStack(bytes);
  assert.equal(stack.globalIndex, 2);
  assert.equal(stack.stackBytes, 8 * MiB);
});

test("an unnamed module with no single stack-pointer candidate is refused, not guessed", () => {
  const twoMutable = module(
    globalSection([
      { mutable: true, value: 8 * MiB },
      { mutable: true, value: 64 },
    ]),
  );
  assert.throws(() => wasmStack(twoMutable), /2 unexported mutable i32 globals/);
  const noMutable = module(globalSection([{ mutable: false, value: 8 * MiB }]));
  assert.throws(() => wasmStack(noMutable), /0 unexported mutable i32 globals/);
  assert.throws(() => wasmStack(module()), /defines no globals/);
  assert.throws(() => wasmStack(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])), /bad magic/);
});

test("the release job's ambient RUSTFLAGS no longer replaces the stack flag", () => {
  // The exact environment of release.yml's "Build and pack Web SDK" step.
  const release = { RUSTFLAGS: '--cfg getrandom_backend="wasm_js"', PATH: "/usr/bin" };
  const { env, flags, cargoArgs } = wasmBuildEnv(release);
  assert.deepEqual(flags.slice(-REQUIRED_WASM_RUSTFLAGS.length), REQUIRED_WASM_RUSTFLAGS);
  assert.ok(flags.includes("link-arg=-zstack-size=8388608"));
  // No rustflags variable is left to outrank the flags, or to leak the
  // wasm-only stack flag into wasm-pack's host `cargo install` fallback.
  for (const key of ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS"]) {
    assert.equal(env[key], undefined, key);
  }
  assert.equal(env.PATH, "/usr/bin");
  // The flags reach Cargo as a --config array it joins with config-file
  // target rustflags; the value is a TOML array of the folded flags.
  assert.equal(cargoArgs[0], "--config");
  const [key, value] = [cargoArgs[1].slice(0, cargoArgs[1].indexOf("=")), cargoArgs[1].slice(cargoArgs[1].indexOf("=") + 1)];
  assert.equal(key, "target.wasm32-unknown-unknown.rustflags");
  assert.deepEqual(JSON.parse(value), flags);
});

test("every ambient rustflags source is folded in ahead of the required flags", () => {
  const flags = effectiveWasmRustflags({
    CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS: "-C opt-level=s",
    RUSTFLAGS: "  -C  debuginfo=0 ",
    CARGO_ENCODED_RUSTFLAGS: "-C\x1fembed-bitcode=no",
  });
  assert.deepEqual(flags, [
    "-C",
    "opt-level=s",
    "-C",
    "debuginfo=0",
    "-C",
    "embed-bitcode=no",
    ...REQUIRED_WASM_RUSTFLAGS,
  ]);
  assert.deepEqual(effectiveWasmRustflags({}), REQUIRED_WASM_RUSTFLAGS);
});
