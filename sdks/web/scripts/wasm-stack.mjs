// Shadow-stack size of a wasm32 module built by rustc + wasm-ld.
//
// @rkat/web links its runtime with an 8 MiB wasm stack. At the 1 MiB default
// the first turn overflows it ("RuntimeError: memory access out of bounds"),
// and every @rkat/web release from 0.8.30 through 0.8.44 shipped exactly that:
// the release job's ambient RUSTFLAGS made Cargo ignore the target flags that
// carried `-zstack-size`. This module reads the size back out of the binary
// itself, so the build, the release job and the tests all check the artifact
// users get instead of trusting the flags that were supposed to produce it.
//
// The check is a typed parse of the module's sections, not a text search:
//   - the stack pointer is the defined global named `__stack_pointer` in the
//     name section, or, when names were stripped (wasm-opt does), the single
//     defined mutable i32 global (wasm-ld's other defined globals are
//     immutable addresses); anything else is ambiguous and refused;
//   - the stack size follows from the layout: wasm-ld's `--stack-first` puts
//     the stack below every active data segment, so it spans [0, SP); with
//     data first it spans [end of data, SP).
//
// CLI: node wasm-stack.mjs <module.wasm> [--min-bytes N]

import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** The stack size @rkat/web links with (`-C link-arg=-zstack-size=...`). */
export const REQUIRED_WASM_STACK_BYTES = 8 * 1024 * 1024;

/** The rustc flag that links {@link REQUIRED_WASM_STACK_BYTES}. */
export const WASM_STACK_RUSTFLAGS = ["-C", `link-arg=-zstack-size=${REQUIRED_WASM_STACK_BYTES}`];

const SECTION_CUSTOM = 0;
const SECTION_IMPORT = 2;
const SECTION_GLOBAL = 6;
const SECTION_EXPORT = 7;
const SECTION_DATA = 11;
const EXTERNAL_KIND_FUNCTION = 0;
const EXTERNAL_KIND_TABLE = 1;
const EXTERNAL_KIND_MEMORY = 2;
const EXTERNAL_KIND_GLOBAL = 3;
const EXTERNAL_KIND_TAG = 4;
const VALTYPE_I32 = 0x7f;
const OPCODE_I32_CONST = 0x41;
const OPCODE_END = 0x0b;
const NAME_SUBSECTION_GLOBAL = 7;

class Reader {
  constructor(bytes, offset = 0, end = bytes.length) {
    this.bytes = bytes;
    this.offset = offset;
    this.end = end;
  }

  byte() {
    if (this.offset >= this.end) {
      throw new Error(`wasm: unexpected end of data at offset ${this.offset}`);
    }
    return this.bytes[this.offset++];
  }

  u32() {
    let result = 0;
    let shift = 0;
    for (;;) {
      const byte = this.byte();
      result += (byte & 0x7f) * 2 ** shift;
      if ((byte & 0x80) === 0) {
        return result;
      }
      shift += 7;
      if (shift > 35) {
        throw new Error("wasm: malformed unsigned LEB128");
      }
    }
  }

  i32() {
    let result = 0;
    let shift = 0;
    let byte;
    do {
      byte = this.byte();
      result |= (byte & 0x7f) << shift;
      shift += 7;
    } while (byte & 0x80 && shift < 35);
    if (byte & 0x80) {
      throw new Error("wasm: malformed signed LEB128");
    }
    if (shift < 32 && byte & 0x40) {
      result |= -1 << shift;
    }
    return result | 0;
  }

  skip(length) {
    if (this.offset + length > this.end) {
      throw new Error(`wasm: section runs past its end at offset ${this.offset}`);
    }
    this.offset += length;
  }

  name() {
    const length = this.u32();
    const start = this.offset;
    this.skip(length);
    return new TextDecoder().decode(this.bytes.subarray(start, start + length));
  }

  // A constant expression; only `i32.const N end` has a value here.
  constI32Expr() {
    const opcode = this.byte();
    if (opcode !== OPCODE_I32_CONST) {
      // Skip to the terminating `end`: not a value this check can use.
      while (this.byte() !== OPCODE_END) {
        // keep consuming
      }
      return null;
    }
    const value = this.i32();
    if (this.byte() !== OPCODE_END) {
      return null;
    }
    return value;
  }

  limits() {
    const flags = this.byte();
    this.u32();
    if (flags & 0x01) {
      this.u32();
    }
  }
}

function sections(bytes) {
  const magic = [0x00, 0x61, 0x73, 0x6d];
  if (bytes.length < 8 || magic.some((byte, index) => bytes[index] !== byte)) {
    throw new Error("wasm: not a wasm module (bad magic)");
  }
  const found = [];
  const reader = new Reader(bytes, 8);
  while (reader.offset < reader.end) {
    const id = reader.byte();
    const size = reader.u32();
    const start = reader.offset;
    reader.skip(size);
    found.push({ id, start, end: start + size });
  }
  return found;
}

function importedGlobalCount(bytes, section) {
  const reader = new Reader(bytes, section.start, section.end);
  let globals = 0;
  const count = reader.u32();
  for (let index = 0; index < count; index += 1) {
    reader.name();
    reader.name();
    const kind = reader.byte();
    switch (kind) {
      case EXTERNAL_KIND_FUNCTION:
        reader.u32();
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
        globals += 1;
        break;
      case EXTERNAL_KIND_TAG:
        reader.byte();
        reader.u32();
        break;
      default:
        throw new Error(`wasm: unknown import kind ${kind}`);
    }
  }
  return globals;
}

function definedGlobals(bytes, section, firstIndex) {
  const reader = new Reader(bytes, section.start, section.end);
  const count = reader.u32();
  const globals = [];
  for (let index = 0; index < count; index += 1) {
    const valtype = reader.byte();
    const mutable = reader.byte() === 1;
    const init = reader.constI32Expr();
    globals.push({ index: firstIndex + index, valtype, mutable, init });
  }
  return globals;
}

function exportedGlobalIndices(bytes, section) {
  const reader = new Reader(bytes, section.start, section.end);
  const count = reader.u32();
  const exported = new Set();
  for (let index = 0; index < count; index += 1) {
    reader.name();
    const kind = reader.byte();
    const target = reader.u32();
    if (kind === EXTERNAL_KIND_GLOBAL) {
      exported.add(target);
    }
  }
  return exported;
}

function globalNames(bytes, section) {
  const reader = new Reader(bytes, section.start, section.end);
  if (reader.name() !== "name") {
    return null;
  }
  const names = new Map();
  while (reader.offset < reader.end) {
    const subsection = reader.byte();
    const size = reader.u32();
    const end = reader.offset + size;
    if (subsection === NAME_SUBSECTION_GLOBAL) {
      const sub = new Reader(bytes, reader.offset, end);
      const count = sub.u32();
      for (let index = 0; index < count; index += 1) {
        const globalIndex = sub.u32();
        names.set(globalIndex, sub.name());
      }
    }
    reader.skip(size);
  }
  return names;
}

// [start, end) of every active data segment in memory 0 with a constant offset.
function activeDataRanges(bytes, section) {
  const reader = new Reader(bytes, section.start, section.end);
  const count = reader.u32();
  const ranges = [];
  for (let index = 0; index < count; index += 1) {
    const flags = reader.u32();
    let offset = null;
    if (flags === 0) {
      offset = reader.constI32Expr();
    } else if (flags === 2) {
      reader.u32();
      offset = reader.constI32Expr();
    } else if (flags !== 1) {
      throw new Error(`wasm: unknown data segment flags ${flags}`);
    }
    const length = reader.u32();
    reader.skip(length);
    if (offset !== null) {
      ranges.push({ start: offset >>> 0, end: (offset >>> 0) + length });
    }
  }
  return ranges;
}

/**
 * The module's shadow-stack pointer and the stack size its layout implies.
 * Throws when the module has no identifiable stack pointer.
 */
export function wasmStack(bytes) {
  const found = sections(bytes);
  const byId = (id) => found.filter((section) => section.id === id);
  const imports = byId(SECTION_IMPORT)[0];
  const firstDefined = imports ? importedGlobalCount(bytes, imports) : 0;
  const globalSection = byId(SECTION_GLOBAL)[0];
  if (!globalSection) {
    throw new Error("wasm: module defines no globals, so it has no shadow-stack pointer");
  }
  const globals = definedGlobals(bytes, globalSection, firstDefined);
  const exportSection = byId(SECTION_EXPORT)[0];
  const exported = exportSection ? exportedGlobalIndices(bytes, exportSection) : new Set();

  let names = null;
  for (const custom of byId(SECTION_CUSTOM)) {
    names = globalNames(bytes, custom) ?? names;
  }

  let stackPointer;
  const namedIndex = names
    ? [...names].find(([, name]) => name === "__stack_pointer")?.[0]
    : undefined;
  if (namedIndex !== undefined) {
    stackPointer = globals.find((global) => global.index === namedIndex);
    if (!stackPointer) {
      throw new Error("wasm: `__stack_pointer` is imported, not defined by this module");
    }
  } else {
    const candidates = globals.filter(
      (global) =>
        global.valtype === VALTYPE_I32 && global.mutable && !exported.has(global.index),
    );
    if (candidates.length !== 1) {
      throw new Error(
        `wasm: cannot identify the shadow-stack pointer: no \`__stack_pointer\` name and ` +
          `${candidates.length} unexported mutable i32 globals`,
      );
    }
    [stackPointer] = candidates;
  }
  if (stackPointer.valtype !== VALTYPE_I32 || !stackPointer.mutable || stackPointer.init === null) {
    throw new Error("wasm: the shadow-stack pointer is not a mutable i32 with a constant initial value");
  }

  const pointer = stackPointer.init >>> 0;
  const dataSection = byId(SECTION_DATA)[0];
  const ranges = dataSection ? activeDataRanges(bytes, dataSection) : [];
  const below = ranges.filter((range) => range.start < pointer);
  const stackFirst = below.length === 0;
  const stackBase = stackFirst ? 0 : Math.max(...below.map((range) => range.end));
  return {
    globalIndex: stackPointer.index,
    initialStackPointer: pointer,
    layout: stackFirst ? "stack-first" : "data-first",
    stackBytes: Math.max(0, pointer - stackBase),
  };
}

/** Throws unless the module's wasm stack is at least `minBytes`. */
export function assertWasmStack(bytes, minBytes = REQUIRED_WASM_STACK_BYTES, label = "module") {
  const stack = wasmStack(bytes);
  if (stack.stackBytes < minBytes) {
    throw new Error(
      `${label}: wasm stack is ${stack.stackBytes} bytes (${stack.layout}, initial stack pointer ` +
        `${stack.initialStackPointer}), below the required ${minBytes}; the runtime overflows it ` +
        `on its first turn. The build's \`-C link-arg=-zstack-size=${minBytes}\` was ` +
        `overridden (a later -zstack-size, e.g. a config-file target.'cfg(..)'.rustflags ` +
        `entry) or lost.`,
    );
  }
  return stack;
}

async function main(argv) {
  const args = argv.slice(2);
  let minBytes = REQUIRED_WASM_STACK_BYTES;
  const files = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--min-bytes") {
      minBytes = Number(args[++index]);
      if (!Number.isInteger(minBytes) || minBytes <= 0) {
        throw new Error(`--min-bytes needs a positive integer, got ${args[index]}`);
      }
    } else {
      files.push(args[index]);
    }
  }
  if (files.length === 0) {
    throw new Error("usage: wasm-stack.mjs <module.wasm>... [--min-bytes N]");
  }
  for (const file of files) {
    const stack = assertWasmStack(new Uint8Array(await readFile(file)), minBytes, file);
    console.log(
      `${file}: wasm stack ${stack.stackBytes} bytes (${stack.layout}, initial stack pointer ${stack.initialStackPointer})`,
    );
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv).catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
