// Shadow-stack frame sizes of a wasm32 module's functions, by name.
//
// rustc + wasm-ld give every function that needs shadow-stack memory a
// prologue that moves the stack pointer down by the frame's size:
//
//   global.get $__stack_pointer
//   i32.const  <frame bytes>
//   i32.sub
//
// This reads that constant out of each function body, and the function's name
// out of the name section. It is meant for the linked module before wasm-opt
// (Cargo's `meerkat_web_runtime.wasm`), which still carries its names; wasm-opt
// strips them from the module the package ships.
//
// The frames that motivated it (issue #1230): at opt-level "s" the largest
// frames were `tokio_with_wasm::glue::task::spawn::{{closure}}` instances, the
// spawn wrapper's poll of the future it was handed, up to 125,440 bytes on a
// turn whose high-water was 230,404 bytes. Every Meerkat spawn now hands that
// wrapper a boxed future (`meerkat_core::tokio::spawn`), so an instance's frame
// no longer grows with the future it wraps (the largest is 4,512 bytes and the
// turn's high-water 115,892); {@link assertSpawnFrameBudget} keeps it that way.
//
// CLI: node wasm-frames.mjs <module.wasm> [--top N] [--filter SUBSTRING]
//                           [--spawn-budget BYTES]

import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { Reader, sections, wasmStack } from "./wasm-stack.mjs";

const SECTION_CUSTOM = 0;
const SECTION_IMPORT = 2;
const SECTION_CODE = 10;
const EXTERNAL_KIND_FUNCTION = 0;
const EXTERNAL_KIND_TABLE = 1;
const EXTERNAL_KIND_MEMORY = 2;
const EXTERNAL_KIND_GLOBAL = 3;
const EXTERNAL_KIND_TAG = 4;
const NAME_SUBSECTION_FUNCTION = 1;
const OPCODE_LOCAL_GET = 0x20;
const OPCODE_LOCAL_SET = 0x21;
const OPCODE_LOCAL_TEE = 0x22;
const OPCODE_GLOBAL_GET = 0x23;
const OPCODE_I32_CONST = 0x41;
const OPCODE_I32_SUB = 0x6b;
const VALTYPE_REF_NULL = 0x63;
const VALTYPE_REF = 0x64;
// Instructions of a body searched for the prologue. wasm-ld's prologue is the
// body's first three instructions; a few local moves may precede the constant.
const PROLOGUE_INSTRUCTIONS = 8;

/**
 * The names of `tokio_with_wasm::spawn` and its closures, one instance per
 * spawned future type. wasm-ld leaves Rust symbols mangled in the name
 * section: rustc's default (legacy) mangling names them
 * `_ZN15tokio_with_wasm4glue4task5spawn...`, and `-C
 * symbol-mangling-version=v0` names them with the spawned future's type
 * spelled out (`_RNC...15tokio_with_wasm4glue4task5spawnNC...`), which is how
 * an instance is traced back to its call site. A demangled name section is
 * matched too.
 */
export const SPAWN_WRAPPER_NAME =
  /^(?:_ZN15tokio_with_wasm4glue4task5spawn|tokio_with_wasm::glue::task::spawn::)|^_R\w*?15tokio_with_wasm4glue4task5spawn/;

/**
 * The largest frame any spawn wrapper instance may have in the release build
 * (profile release, opt-level "s"). Before every Meerkat spawn boxed its
 * future the largest of 464 instances measured 125,440 bytes and 145 were over
 * 4 KiB. Boxed, the wrapper is instantiated per output type and the largest
 * measures 4,512 bytes, carrying a command result of about 2 KB through the
 * join. The budget leaves room for a larger output type and fails on any
 * spawn that hands the wrapper an unboxed future again.
 */
export const SPAWN_FRAME_BUDGET_BYTES = 8 * 1024;

function importedFunctionCount(bytes, section) {
  const reader = new Reader(bytes, section.start, section.end);
  let functions = 0;
  const count = reader.u32();
  for (let index = 0; index < count; index += 1) {
    reader.name();
    reader.name();
    const kind = reader.byte();
    switch (kind) {
      case EXTERNAL_KIND_FUNCTION:
        reader.u32();
        functions += 1;
        break;
      case EXTERNAL_KIND_TABLE:
        reader.byte();
        reader.limits();
        break;
      case EXTERNAL_KIND_MEMORY:
        reader.limits();
        break;
      case EXTERNAL_KIND_GLOBAL:
        reader.byte();
        reader.byte();
        break;
      case EXTERNAL_KIND_TAG:
        reader.byte();
        reader.u32();
        break;
      default:
        throw new Error(`wasm: unknown import kind ${kind}`);
    }
  }
  return functions;
}

function functionNames(bytes, section) {
  const reader = new Reader(bytes, section.start, section.end);
  if (reader.name() !== "name") {
    return null;
  }
  const names = new Map();
  while (reader.offset < reader.end) {
    const subsection = reader.byte();
    const size = reader.u32();
    const end = reader.offset + size;
    if (subsection === NAME_SUBSECTION_FUNCTION) {
      const sub = new Reader(bytes, reader.offset, end);
      const count = sub.u32();
      for (let index = 0; index < count; index += 1) {
        const functionIndex = sub.u32();
        names.set(functionIndex, sub.name());
      }
    }
    reader.skip(size);
  }
  return names;
}

function skipLocals(reader) {
  const groups = reader.u32();
  for (let group = 0; group < groups; group += 1) {
    reader.u32();
    const valtype = reader.byte();
    if (valtype === VALTYPE_REF || valtype === VALTYPE_REF_NULL) {
      reader.i32();
    }
  }
}

// The frame size in the body's prologue, or 0 when the body has none (it uses
// no shadow-stack memory, or only the caller's).
function prologueFrameBytes(reader, stackPointer) {
  let sawStackPointer = false;
  let constant = null;
  for (let step = 0; step < PROLOGUE_INSTRUCTIONS && reader.offset < reader.end; step += 1) {
    const opcode = reader.byte();
    switch (opcode) {
      case OPCODE_GLOBAL_GET:
        sawStackPointer = reader.u32() === stackPointer;
        constant = null;
        break;
      case OPCODE_LOCAL_GET:
      case OPCODE_LOCAL_SET:
      case OPCODE_LOCAL_TEE:
        reader.u32();
        break;
      case OPCODE_I32_CONST:
        constant = reader.i32();
        break;
      case OPCODE_I32_SUB:
        return sawStackPointer && constant !== null && constant > 0 ? constant : 0;
      default:
        return 0;
    }
  }
  return 0;
}

/**
 * Every defined function of the module with a shadow-stack frame, largest
 * first: `{ index, name, frameBytes }`. `name` is `null` when the module has
 * no name for the function.
 */
export function wasmFrames(bytes) {
  const found = sections(bytes);
  const byId = (id) => found.filter((section) => section.id === id);
  const imports = byId(SECTION_IMPORT)[0];
  const firstDefined = imports ? importedFunctionCount(bytes, imports) : 0;
  const stackPointer = wasmStack(bytes).globalIndex;

  let names = null;
  for (const custom of byId(SECTION_CUSTOM)) {
    names = functionNames(bytes, custom) ?? names;
  }

  const code = byId(SECTION_CODE)[0];
  if (!code) {
    return [];
  }
  const reader = new Reader(bytes, code.start, code.end);
  const count = reader.u32();
  const frames = [];
  for (let offset = 0; offset < count; offset += 1) {
    const size = reader.u32();
    const end = reader.offset + size;
    const body = new Reader(bytes, reader.offset, end);
    skipLocals(body);
    const frameBytes = prologueFrameBytes(body, stackPointer);
    if (frameBytes > 0) {
      const index = firstDefined + offset;
      frames.push({ index, name: names?.get(index) ?? null, frameBytes });
    }
    reader.skip(size);
  }
  return frames.sort((a, b) => b.frameBytes - a.frameBytes || a.index - b.index);
}

/** The spawn wrapper instances among `frames` (from {@link wasmFrames}). */
export function spawnWrapperFrames(frames) {
  return frames.filter((frame) => frame.name !== null && SPAWN_WRAPPER_NAME.test(frame.name));
}

/**
 * Throws unless every spawn wrapper instance of the module has a frame of at
 * most `budget` bytes. Also throws when the module has no function names or
 * no spawn wrapper instance at all: a module this check cannot see into would
 * otherwise pass any budget. Returns the largest instance.
 */
export function assertSpawnFrameBudget(bytes, budget = SPAWN_FRAME_BUDGET_BYTES, label = "module") {
  const frames = wasmFrames(bytes);
  if (!frames.some((frame) => frame.name !== null)) {
    throw new Error(
      `${label}: no function names, so its spawn frames cannot be found; measure the linked ` +
        `module before wasm-opt strips its name section`,
    );
  }
  const spawns = spawnWrapperFrames(frames);
  if (spawns.length === 0) {
    throw new Error(`${label}: no \`tokio_with_wasm::spawn\` frame found; is this the runtime module?`);
  }
  const over = spawns.filter((frame) => frame.frameBytes > budget);
  if (over.length > 0) {
    const listed = over
      .slice(0, 5)
      .map((frame) => `  ${frame.frameBytes} bytes: ${frame.name}`)
      .join("\n");
    throw new Error(
      `${label}: ${over.length} spawn wrapper frame(s) over the ${budget}-byte budget:\n${listed}\n` +
        `A spawn wrapper's frame follows the size of the future it polls; spawn through ` +
        `\`meerkat_core::tokio::spawn\` (the crate's \`tokio::spawn\` alias), which boxes it.`,
    );
  }
  return { largest: spawns[0], instances: spawns.length };
}

async function main(argv) {
  const args = argv.slice(2);
  let top = 20;
  let filter = null;
  let spawnBudget = null;
  const files = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--top") {
      top = Number(args[++index]);
    } else if (args[index] === "--filter") {
      filter = args[++index];
    } else if (args[index] === "--spawn-budget") {
      spawnBudget = Number(args[++index]);
      if (!Number.isInteger(spawnBudget) || spawnBudget <= 0) {
        throw new Error(`--spawn-budget needs a positive integer, got ${args[index]}`);
      }
    } else {
      files.push(args[index]);
    }
  }
  if (files.length !== 1 || !Number.isInteger(top) || top <= 0) {
    throw new Error(
      "usage: wasm-frames.mjs <module.wasm> [--top N] [--filter SUBSTRING] [--spawn-budget BYTES]",
    );
  }
  const bytes = new Uint8Array(await readFile(files[0]));
  const frames = wasmFrames(bytes);
  const shown = filter ? frames.filter((frame) => frame.name?.includes(filter)) : frames;
  const spawns = spawnWrapperFrames(frames);
  console.log(
    `${files[0]}: ${frames.length} functions with frames; ${spawns.length} spawn wrapper instances, ` +
      `largest ${spawns[0]?.frameBytes ?? 0} bytes`,
  );
  for (const frame of shown.slice(0, top)) {
    console.log(`${String(frame.frameBytes).padStart(9)}  ${frame.name ?? `<function ${frame.index}>`}`);
  }
  if (spawnBudget !== null) {
    const { largest } = assertSpawnFrameBudget(bytes, spawnBudget, files[0]);
    console.log(`largest spawn wrapper frame ${largest.frameBytes} bytes, within ${spawnBudget}`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv).catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
