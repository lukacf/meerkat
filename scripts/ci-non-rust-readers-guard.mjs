#!/usr/bin/env node
// Fail-closed guard for scripts/ci-non-rust-readers.json, the inventory of
// Rust code that reads repository files outside its own crate directory.
//
// The CI planner (scripts/ci-cargo-lanes.mjs) selects a package's lanes from
// the paths that changed inside its crate directory. A test that reads docs/,
// sdks/, examples/, .claude/, CHANGELOG.md, specs/, scripts/ or another
// crate's files has inputs the planner cannot see that way: a docs-only change
// sets rust_changed=false and skips every Rust lane, so such a reader ran only
// in the pre-push hook. The inventory names every such input and its reader;
// scripts/ci-non-rust-readers.mjs maps a changed path list onto the readers.
//
// This guard walks the module tree of every workspace target (so the
// examples/*-rs files that meerkat and meerkat-mob compile as [[example]]
// targets, and #[path] modules shared across crates, are scanned under the
// package that compiles them), lexes each file (comments are not code), and
// collects references to repository paths outside the referencing crate,
// and to in-crate documentation (.md, .mdx) that include_str!/include_bytes!
// compiles in (the planner otherwise treats in-crate docs as no build input).
// The walk follows `mod`, #[path] modules and include!() of a literal or of
// concat!(env!("CARGO_MANIFEST_DIR"), "<literal>"). Any other include!()
// form, an include!() whose file does not exist, and an
// include_str!/include_bytes! argument it cannot resolve (other than a
// concat! under OUT_DIR, which reads its own build script's output; build
// scripts are scanned) are refused: the guard cannot claim a complete
// inventory over source it did not read. References:
//   - include_str!/include_bytes! of a literal or of
//     concat!(env!("CARGO_MANIFEST_DIR"), "...") (symlinks resolved, so an
//     in-crate link to .claude/skills counts as .claude/skills);
//   - #[path = "..."] modules outside the crate;
//   - cargo:rerun-if-changed=<path> in build scripts;
//   - path literals that climb out of the crate ("../../docs/x.mdx", or a
//     "{}/../../sdks" piece after CARGO_MANIFEST_DIR);
//   - literals naming a repository root entry that belongs to no crate
//     (ROOT_DIRS, ROOT_FILES): a multi-segment one ("docs/api/rpc.mdx")
//     wherever it occurs, a bare one ("docs") when joined onto an expression
//     that evaluates to the repository root (env!("CARGO_MANIFEST_DIR") and
//     parent()/"..", git rev-parse --show-toplevel, a same-file helper, or a
//     repo_root()/workspace_root() helper) or listed beside such a path;
//   - a directory walk (read_dir, *walk*, collect_/visit_/scan_ helpers named
//     for files, dirs, paths, sources or trees) of an expression that
//     evaluates to a directory outside the crate;
//   - any other call handed the repository root itself (a live-workspace
//     audit such as collect_effect_authority_findings(&root), a command run
//     with current_dir(root)), except calls that only compute with a path
//     (NON_READING_CALLS, helpers named *_path/*_dir). Such a reader is keyed
//     "./" in the inventory: every changed path selects it.
// A reference counts when its path exists in the checkout (tracked, or
// untracked and not ignored). .rkat, AGENTS.md and CLAUDE.md are runtime
// conventions every realm reads from its own project root, so a bare literal
// naming them is not a repository read (a climb such as "../../AGENTS.md"
// still is).
//
// Every reference must be covered by an inventory row of the same path,
// package and target that lists its source file, or by a `not_readers` row
// that records why the literal is not a read. Every inventory row must still
// be produced by the scan, with the same source files and scope. Either
// mismatch fails, naming the file and line.
//
// Scope: a reference in test-only code (a test target, a #[cfg(test)]
// module, a #[test] function) is read by that target's tests ("target"); a
// reference in library, binary or build-script code can be reached by every
// test of the package ("package").
//
// usage: ci-non-rust-readers-guard.mjs [--inventory FILE] [--list | --json]
//   --list  print every reference found (for curating the inventory)
//   --json  print the references as JSON

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, realpathSync, statSync } from "node:fs";
import { dirname, posix, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// rust-test-selector.mjs takes the repository root from the working
// directory; pin it to the checkout this script belongs to.
process.chdir(scriptRoot);
const { normalizePath, readMetadata, root, workspacePackages, packageDir } = await import(
  "./rust-test-selector.mjs"
);
const { loadNonRustReaders, INVENTORY_PATH } = await import("./ci-non-rust-readers.mjs");

// Repository root entries that belong to no crate.
export const ROOT_DIRS = [
  ".claude",
  ".codex",
  ".config",
  ".github",
  ".rct",
  "artifacts",
  "docs",
  "examples",
  "scripts",
  "sdks",
  "specs",
  "tests",
  "tools",
];
export const ROOT_FILES = [
  "CHANGELOG.md",
  "Cargo.lock",
  "MODULE.bazel",
  "MODULE.bazel.lock",
  "Makefile",
  "deny.toml",
  "renovate.json",
  "rust-toolchain.toml",
  ".bazelignore",
  ".bazelrc",
  ".bazelversion",
  ".pre-commit-config.yaml",
];
// Names that denote the repository root when nothing in the file says
// otherwise (helpers defined in another file, function parameters).
const STRONG_ROOT_NAMES = new Set([
  "repo_root",
  "repository_root",
  "workspace_root",
  "workspace_dir",
  "repo_dir",
  "checkout_root",
  "git_root",
  "meerkat_root",
]);
const WEAK_ROOT_NAMES = new Set(["root", "repo", "workspace", "toplevel", "top_level", "source_root"]);
// Calls that walk a directory: read_dir, walk*, and collect/visit/scan
// helpers named for files, directories, paths, sources or trees.
const WALK_CALL = /^(read_dir|\w*walk\w*|(collect|visit|scan|list|find)_\w*(files|dirs?|paths|sources|tree)\w*)$/;

// Calls whose result is built from their arguments (a path, a command, its
// output); any other call is opaque to the evaluator.
const PATH_CONSTRUCTORS = new Set([
  "new", "from", "var", "env", "concat", "format", "args", "arg", "output", "to_path_buf", "canonicalize",
  "unwrap", "expect", "unwrap_or_else", "unwrap_or", "to_owned", "into", "as_path", "as_ref", "clone", "trim",
  "from_utf8", "from_utf8_lossy", "to_string", "map_err", "context", "with_context", "ok", "PathBuf", "Path",
]);

// Calls that take the repository root without reading under it.
const NON_READING_CALLS = new Set([
  "Arc", "Box", "Err", "Ok", "Some", "as_path", "as_ref", "assert", "assert_eq", "assert_ne", "canonicalize",
  "clone", "cmp", "contains", "context", "debug", "display", "ends_with", "eprintln", "eq", "exists", "expect",
  "format", "from", "get", "insert", "is_dir", "is_file", "join", "map", "map_err", "ne", "new", "panic",
  "partial_cmp", "print", "println", "push", "relative", "starts_with", "strip_prefix", "to_path_buf", "to_str",
  "to_string_lossy", "unwrap", "unwrap_or", "unwrap_or_else", "with_context", "write", "writeln", "relative_path",
  "rel", "relative_to", "repo_relative", "display_path", "replace", "replacen",
]);

function git(args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
}

// ---------------------------------------------------------------------------
// Rust lexer: enough to separate code from comments and string literals.

export function lexRust(text) {
  const tokens = [];
  const n = text.length;
  let i = 0;
  let line = 1;
  const isIdentStart = (c) => /[A-Za-z_]/.test(c);
  const isIdent = (c) => /[A-Za-z0-9_]/.test(c);
  const readQuoted = (start) => {
    let j = start + 1;
    let out = "";
    while (j < n) {
      const c = text[j];
      if (c === "\\") {
        const next = text[j + 1];
        if (next === "\n") {
          line += 1;
          j += 2;
          while (j < n && /\s/.test(text[j])) {
            if (text[j] === "\n") line += 1;
            j += 1;
          }
          continue;
        }
        const simple = { n: "\n", t: "\t", r: "\r", 0: "\0", "\\": "\\", '"': '"', "'": "'" };
        out += next in simple ? simple[next] : next ?? "";
        j += 2;
        continue;
      }
      if (c === '"') return [out, j + 1];
      if (c === "\n") line += 1;
      out += c;
      j += 1;
    }
    return [out, n];
  };
  while (i < n) {
    const c = text[i];
    if (c === "\n") {
      line += 1;
      i += 1;
      continue;
    }
    if (c === " " || c === "\t" || c === "\r") {
      i += 1;
      continue;
    }
    if (c === "/" && text[i + 1] === "/") {
      while (i < n && text[i] !== "\n") i += 1;
      continue;
    }
    if (c === "/" && text[i + 1] === "*") {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        if (text[i] === "\n") line += 1;
        if (text[i] === "/" && text[i + 1] === "*") {
          depth += 1;
          i += 2;
        } else if (text[i] === "*" && text[i + 1] === "/") {
          depth -= 1;
          i += 2;
        } else {
          i += 1;
        }
      }
      continue;
    }
    // Raw strings: r"..", r#".."#, br#".."#, cr"..".
    const raw = /^(?:br|cr|r)(#*)"/.exec(text.slice(i, i + 64));
    if (raw) {
      const close = `"${raw[1]}`;
      const startLine = line;
      const bodyStart = i + raw[0].length;
      const end = text.indexOf(close, bodyStart);
      const stop = end === -1 ? n : end;
      const body = text.slice(bodyStart, stop);
      line += (body.match(/\n/g) ?? []).length;
      tokens.push({ t: "str", v: body, line: startLine });
      i = stop + close.length;
      continue;
    }
    if ((c === "b" || c === "c") && text[i + 1] === '"') {
      const startLine = line;
      const [content, end] = readQuoted(i + 1);
      tokens.push({ t: "str", v: content, line: startLine });
      i = end;
      continue;
    }
    if (c === '"') {
      const startLine = line;
      const [content, end] = readQuoted(i);
      tokens.push({ t: "str", v: content, line: startLine });
      i = end;
      continue;
    }
    if (c === "b" && text[i + 1] === "'") i += 1;
    if (text[i] === "'") {
      if (text[i + 1] === "\\") {
        let j = i + 2;
        while (j < n && text[j] !== "'" && text[j] !== "\n") j += 1;
        i = j + 1;
        continue;
      }
      const cp = text.codePointAt(i + 1);
      const width = cp !== undefined && cp > 0xffff ? 2 : 1;
      if (text[i + 1 + width] === "'") {
        i += 2 + width;
        continue;
      }
      // A lifetime or a label.
      i += 1;
      while (i < n && isIdent(text[i])) i += 1;
      continue;
    }
    if (isIdentStart(c) || c.charCodeAt(0) > 0x7f) {
      let j = i;
      if (c === "r" && text[i + 1] === "#" && isIdentStart(text[i + 2] ?? "")) j += 2;
      while (j < n && (isIdent(text[j]) || text[j].charCodeAt(0) > 0x7f)) j += 1;
      if (j === i) j += 1;
      tokens.push({ t: "id", v: text.slice(i, j).replace(/^r#/, ""), line });
      i = j;
      continue;
    }
    if (/[0-9]/.test(c)) {
      let j = i;
      while (j < n && (isIdent(text[j]) || (text[j] === "." && /[0-9]/.test(text[j + 1] ?? "")))) j += 1;
      tokens.push({ t: "num", v: text.slice(i, j), line });
      i = j;
      continue;
    }
    tokens.push({ t: "p", v: c, line });
    i += 1;
  }
  return tokens;
}

// The index of the bracket that closes the one at `open`.
function closingBracket(tokens, open) {
  const [left, right] = tokens[open].v === "(" ? ["(", ")"] : tokens[open].v === "{" ? ["{", "}"] : ["[", "]"];
  let level = 0;
  for (let k = open; k < tokens.length; k += 1) {
    if (tokens[k].t !== "p") continue;
    if (tokens[k].v === left) level += 1;
    else if (tokens[k].v === right && --level === 0) return k;
  }
  return tokens.length - 1;
}

// The predicate of a `cfg(...)` whose `(` is at `open`: { op: all|any|not,
// args }, { key, value } (`feature = "x"`), { flag } (`test`), or
// { unknown } for a shape this parser does not read.
export function parseCfg(tokens, open) {
  const close = closingBracket(tokens, open);
  let k = open + 1;
  const predicate = () => {
    const head = tokens[k];
    if (head?.t !== "id") return { unknown: true };
    k += 1;
    if (tokens[k]?.v === "=" && tokens[k + 1]?.t === "str") {
      k += 2;
      return { key: head.v, value: tokens[k - 1].v };
    }
    if (tokens[k]?.v !== "(") return { flag: head.v };
    const end = closingBracket(tokens, k);
    k += 1;
    const args = [];
    while (k < end) {
      const before = k;
      args.push(predicate());
      if (tokens[k]?.v === ",") k += 1;
      if (k === before) return { unknown: true };
    }
    k = end + 1;
    return { op: head.v, args };
  };
  const expr = predicate();
  return k === close ? expr : { unknown: true };
}

// The cfg values of the CI runners (x86_64 Linux, debug test builds).
const CFG_FLAGS = { test: true, unix: true, windows: false, debug_assertions: true, miri: false, doc: false, doctest: false };
const CFG_KEYS = { target_arch: "x86_64", target_os: "linux", target_family: "unix", target_pointer_width: "64", target_endian: "little", target_env: "gnu", panic: "unwind" };

// true, false, or null for a cfg this guard does not model (never a
// refusal: only a predicate known false compiles a test out).
export function evalCfg(expr, features) {
  if (expr.op === "all" || expr.op === "any" || expr.op === "not") {
    const values = expr.args.map((arg) => evalCfg(arg, features));
    if (expr.op === "not") return values.length === 1 && values[0] !== null ? !values[0] : null;
    if (expr.op === "all") return values.includes(false) ? false : values.includes(null) ? null : true;
    return values.includes(true) ? true : values.includes(null) ? null : false;
  }
  if (expr.key === "feature") return features.has(expr.value);
  if (expr.key !== undefined) return Object.hasOwn(CFG_KEYS, expr.key) ? CFG_KEYS[expr.key] === expr.value : null;
  if (expr.flag !== undefined) return Object.hasOwn(CFG_FLAGS, expr.flag) ? CFG_FLAGS[expr.flag] : null;
  return null;
}

export function cfgText(expr) {
  if (expr.op) return `${expr.op}(${expr.args.map(cfgText).join(", ")})`;
  if (expr.key !== undefined) return `${expr.key} = "${expr.value}"`;
  return expr.flag ?? "?";
}

// The features `-p <package> --features <listed>` enables for the
// package's own test build: the listed ones, the defaults and those a
// dev-dependency of the package on itself requests (the repository's way
// to give package-scoped test lanes their features), closed over the
// package's feature table (an optional dependency is a feature of its own
// name). A feature another dev-dependency would turn on through Cargo's
// feature unification is not counted: a row whose test needs one lists it,
// so the row does not depend on what its dev-dependencies happen to enable.
export function enabledFeatures(pkg, listed) {
  const enabled = new Set();
  const queue = ["default", ...listed];
  for (const dep of pkg.dependencies ?? []) {
    if (dep.kind !== "dev" || dep.name !== pkg.name || !dep.path) continue;
    queue.push(...dep.features);
    if (dep.uses_default_features) queue.push("default");
  }
  while (queue.length) {
    const name = queue.pop();
    if (enabled.has(name) || !Object.hasOwn(pkg.features, name)) continue;
    enabled.add(name);
    for (const item of pkg.features[name]) {
      if (item.startsWith("dep:")) continue;
      const slash = item.indexOf("/");
      if (slash < 0) queue.push(item);
      else if (item[slash - 1] !== "?") queue.push(item.slice(0, slash));
    }
  }
  return enabled;
}

// The cfg predicates over each definition of `fn <name>` in a file: the
// file's inner #![cfg(...)], those on every enclosing item, and the fn's
// own. One list per definition.
export function fnCfgs(tokens, name) {
  const sites = [];
  const inner = [];
  const stack = [];
  let pending = [];
  let depth = 0;
  for (let k = 0; k < tokens.length; k += 1) {
    const tok = tokens[k];
    if (tok.t === "p" && tok.v === "#" && (tokens[k + 1]?.v === "[" || (tokens[k + 1]?.v === "!" && tokens[k + 2]?.v === "["))) {
      const isInner = tokens[k + 1].v === "!";
      const open = isInner ? k + 2 : k + 1;
      if (tokens[open + 1]?.v === "cfg" && tokens[open + 2]?.v === "(") {
        const expr = parseCfg(tokens, open + 2);
        if (!isInner) pending.push(expr);
        else if (depth === 0) inner.push(expr);
        else if (stack.length && stack[stack.length - 1].depth === depth) stack[stack.length - 1].cfgs.push(expr);
        else stack.push({ depth, cfgs: [expr] });
      }
      k = closingBracket(tokens, open);
      continue;
    }
    if (tok.t === "p" && tok.v === "{") {
      depth += 1;
      if (pending.length) stack.push({ depth, cfgs: pending });
      pending = [];
    } else if (tok.t === "p" && tok.v === "}") {
      if (stack.length && stack[stack.length - 1].depth === depth) stack.pop();
      depth -= 1;
      pending = [];
    } else if (tok.t === "p" && tok.v === ";") {
      pending = [];
    } else if (tok.t === "id" && tok.v === "fn" && tokens[k + 1]?.v === name) {
      sites.push([...inner, ...stack.flatMap((entry) => entry.cfgs), ...pending]);
    }
  }
  return sites;
}

// Test-only code: the bodies of items marked #[test], #[tokio::test],
// #[cfg(test)] (and cfg(all(test, ...)), not cfg(not(test))), and the `mod`
// declarations so marked. Returns { inTest: Uint8Array per token,
// testMods: Set of `mod` token indices }.
export function testRegions(tokens) {
  const inTest = new Uint8Array(tokens.length);
  const testMods = new Set();
  const testFns = new Set();
  let pendingFn = false;
  const starts = [];
  let depth = 0;
  let pending = false;
  for (let k = 0; k < tokens.length; k += 1) {
    const tok = tokens[k];
    if (tok.v === "#" && tok.t === "p" && (tokens[k + 1]?.v === "[" || (tokens[k + 1]?.v === "!" && tokens[k + 2]?.v === "["))) {
      const open = tokens[k + 1].v === "[" ? k + 1 : k + 2;
      let j = open + 1;
      let level = 1;
      const ids = [];
      for (; j < tokens.length && level > 0; j += 1) {
        if (tokens[j].v === "[") level += 1;
        else if (tokens[j].v === "]") level -= 1;
        else if (tokens[j].t === "id") ids.push(tokens[j].v);
      }
      const head = [];
      for (let h = open + 1; h < j && tokens[h].v !== "(" && tokens[h].v !== "]"; h += 1) {
        if (tokens[h].t === "id") head.push(tokens[h].v);
      }
      const isCfgTest = head[0] === "cfg" && ids.includes("test") && !ids.includes("not");
      const isTestFn = head.length > 0 && (head[head.length - 1] === "test" || head[0] === "rstest" || head[0] === "test_case");
      if (tokens[k + 1].v === "!") {
        // Inner attribute: #![cfg(test)] makes the whole file test-only.
        if (isCfgTest) inTest.fill(1);
      } else if (isCfgTest || isTestFn) {
        pending = true;
        if (isTestFn) pendingFn = true;
      }
      for (let h = k; h < j; h += 1) inTest[h] = starts.length > 0 ? 1 : inTest[h];
      k = j - 1;
      continue;
    }
    if (tok.t === "p" && tok.v === "{") {
      depth += 1;
      if (pending) {
        starts.push(depth);
        pending = false;
      }
    } else if (tok.t === "p" && tok.v === "}") {
      if (starts.length && starts[starts.length - 1] === depth) starts.pop();
      depth -= 1;
    } else if (tok.t === "p" && tok.v === ";" && pending) {
      pending = false;
    } else if (tok.t === "id" && tok.v === "mod" && pending) {
      testMods.add(k);
    } else if (tok.t === "id" && tok.v === "fn" && pendingFn && tokens[k + 1]?.t === "id") {
      testFns.add(k + 1);
      pendingFn = false;
    }
    if (starts.length > 0 || inTest[k]) inTest[k] = 1;
  }
  return { inTest, testMods, testFns };
}

// ---------------------------------------------------------------------------
// Workspace model: every Rust file of every workspace target, by module walk.

// The argument of `include!(...)`, `include_str!(...)` or `include_bytes!(...)`
// whose `(` is at `open`: { kind: "literal", value } for a string literal,
// { kind: "manifest", value } for concat!(env!("CARGO_MANIFEST_DIR"),
// "<literal>"), { kind: "out-dir" } for a concat! under env!("OUT_DIR"),
// else { kind: "unresolved" }. `form` is the argument's source text.
export function includeArgument(tokens, open, match) {
  const close = match[open] > open ? match[open] : tokens.length - 1;
  const args = tokens.slice(open + 1, close);
  const form = args.map((tok) => (tok.t === "str" ? JSON.stringify(tok.v) : tok.v)).join("");
  if (args.length === 1 && args[0].t === "str") return { kind: "literal", value: args[0].v, form, close };
  const shape = args.map((tok) => (tok.t === "str" ? "S" : tok.v)).join(" ");
  if (shape === "concat ! ( env ! ( S ) , S )" || shape === "concat ! ( env ! ( S ) , S , )") {
    if (args[6].v === "CARGO_MANIFEST_DIR") return { kind: "manifest", value: args[9].v, form, close };
  }
  if (/^concat ! \( env ! \( S \)( , S)+ ,? \)$/.test(shape) && args[6].v === "OUT_DIR") return { kind: "out-dir", form, close };
  return { kind: "unresolved", form, close };
}

// The `mod` declarations, #[path] modules and include!() files of one file,
// each with the candidate paths it may resolve to, whether the declaration
// is test-only and the cfg predicates over it (its own and those of the
// inline modules around it); an include!() the walk cannot follow is a child
// with `unresolved` set. Inline `mod name { ... }` blocks nest the
// directory. A file reached through include!() gets `modDir`, the
// directory the including file's `mod` items resolve against at the
// include site; its own `mod` items may resolve against either.
function moduleChildren(file, tokens, regions, index, isRoot, crateDir, modDir = null) {
  const base = dirname(file);
  const stem = file.endsWith("/mod.rs") || isRoot ? base : file.replace(/\.rs$/, "");
  const children = [];
  const inline = [];
  const inlineDepths = [];
  const inlineCfgs = [];
  const fileCfgs = [];
  let pendingCfgs = [];
  let depth = 0;
  let pendingPath = null;
  for (let k = 0; k < tokens.length; k += 1) {
    const tok = tokens[k];
    if (tok.t === "p" && tok.v === "{") {
      depth += 1;
      pendingCfgs = [];
      continue;
    }
    if (tok.t === "p" && tok.v === "}") {
      depth -= 1;
      pendingCfgs = [];
      if (inlineDepths.length && inlineDepths[inlineDepths.length - 1] === depth) {
        inlineDepths.pop();
        inline.pop();
        inlineCfgs.pop();
      }
      continue;
    }
    if (tok.t === "p" && tok.v === ";") {
      pendingCfgs = [];
      continue;
    }
    // An inner #![cfg(...)] of the file gates every module it declares.
    if (tok.v === "#" && depth === 0 && tokens[k + 1]?.v === "!" && tokens[k + 2]?.v === "[" && tokens[k + 3]?.v === "cfg" && tokens[k + 4]?.v === "(") {
      fileCfgs.push(parseCfg(tokens, k + 4));
      k = closingBracket(tokens, k + 2);
      continue;
    }
    if (tok.v === "#" && tokens[k + 1]?.v === "[" && tokens[k + 2]?.v === "cfg" && tokens[k + 3]?.v === "(") {
      pendingCfgs.push(parseCfg(tokens, k + 3));
      k = closingBracket(tokens, k + 1);
      continue;
    }
    if (tok.v === "#" && tokens[k + 1]?.v === "[" && tokens[k + 2]?.v === "path" && tokens[k + 3]?.v === "=" && tokens[k + 4]?.t === "str") {
      pendingPath = { value: tokens[k + 4].v, index: k + 4 };
      k += 4;
      continue;
    }
    if (tok.t === "id" && tok.v === "mod" && tokens[k + 1]?.t === "id") {
      const name = tokens[k + 1].v;
      const after = tokens[k + 2];
      const testOnly = regions.testMods.has(k) || regions.inTest[k] === 1;
      const cfgs = [...fileCfgs, ...inlineCfgs.flat(), ...pendingCfgs];
      pendingCfgs = [];
      if (after?.v === ";") {
        const dir = resolve(stem, ...inline);
        if (pendingPath) {
          children.push({ candidates: [resolve(inline.length ? dir : base, pendingPath.value)], testOnly, cfgs, pathLiteral: pendingPath.index });
        } else {
          const dirs = modDir ? [resolve(modDir, ...inline), resolve(base, ...inline), dir] : [dir];
          const candidates = [...new Set(dirs.flatMap((candidate) => [resolve(candidate, `${name}.rs`), resolve(candidate, name, "mod.rs")]))];
          children.push({ candidates, testOnly, cfgs });
        }
        pendingPath = null;
      } else if (after?.v === "{") {
        inline.push(pendingPath?.value ?? name);
        inlineCfgs.push(cfgs.slice(fileCfgs.length + inlineCfgs.flat().length));
        pendingPath = null;
        inlineDepths.push(depth);
        depth += 1;
        k += 2;
      }
      continue;
    }
    if (tok.t === "id" && tok.v === "include" && tokens[k + 1]?.v === "!" && tokens[k + 2]?.v === "(" && tokens[k - 1]?.v !== "fn") {
      const arg = includeArgument(tokens, k + 2, index.match);
      const testOnly = regions.inTest[k] === 1;
      const includeDir = resolve(stem, ...inline);
      const cfgs = [...fileCfgs, ...inlineCfgs.flat()];
      if (arg.kind === "literal") {
        children.push({ candidates: [resolve(base, arg.value)], testOnly, cfgs, include: { line: tok.line, form: arg.form, modDir: includeDir } });
      } else if (arg.kind === "manifest") {
        children.push({ candidates: [resolve(crateDir, arg.value.replace(/^\//, ""))], testOnly, cfgs, include: { line: tok.line, form: arg.form, modDir: includeDir } });
      } else {
        children.push({ unresolved: { line: tok.line, form: `include!(${arg.form})`, why: "the walk follows include!() of a literal or of concat!(env!(\"CARGO_MANIFEST_DIR\"), \"<literal>\") only" } });
      }
      k = arg.close;
    }
  }
  return children;
}

function targetId(target) {
  const kinds = target.kind;
  if (kinds.includes("custom-build")) return "build";
  if (kinds.includes("test")) return `test:${target.name}`;
  if (kinds.includes("bin")) return `bin:${target.name}`;
  if (kinds.includes("example")) return `example:${target.name}`;
  if (kinds.includes("bench")) return `bench:${target.name}`;
  return "lib";
}

export function workspaceModel() {
  const metadata = readMetadata();
  const packages = workspacePackages(metadata);
  // abs file -> Map<package name, { pkg, targets: Map<target id, testOnly>,
  // gates: Map<target id, cfg predicate lists, one per module path> }>
  const files = new Map();
  const cache = new Map();
  const parse = (file) => {
    if (!cache.has(file)) {
      const tokens = lexRust(readFileSync(file, "utf8"));
      cache.set(file, { tokens, regions: testRegions(tokens), index: buildIndex(tokens) });
    }
    return cache.get(file);
  };
  // Source the walk could not follow, keyed by file and line.
  const unresolved = new Map();
  const refuse = (file, site, pkg, id) => {
    const source = normalizePath(relative(root, file));
    const key = `${source}:${site.line}`;
    const entry = unresolved.get(key) ?? { source, line: site.line, form: site.form, why: site.why, readers: new Set() };
    entry.readers.add(`${pkg.name} ${id}`);
    unresolved.set(key, entry);
  };
  for (const pkg of packages) {
    const crateDir = resolve(root, packageDir(pkg));
    for (const target of pkg.targets) {
      const id = targetId(target);
      const rootTestOnly = id.startsWith("test:") || id.startsWith("bench:");
      // file -> Map<cfg gate list, testOnly>. A file is walked once for each
      // distinct gate list a module path brings to it, so the modules it
      // declares inherit every such list, and again when a non-test path
      // reaches it after a test-only one. A path that returns to a file it
      // already passed through is a module cycle; it stops there.
      const visited = new Map();
      const queue = [[target.src_path, true, rootTestOnly, null, [], []]];
      while (queue.length) {
        const [file, isRoot, testOnly, modDir, gates, chain] = queue.pop();
        if (!existsSync(file) || chain.includes(file)) continue;
        const gatesKey = JSON.stringify(gates);
        const seen = visited.get(file) ?? new Map();
        if (seen.has(gatesKey) && (seen.get(gatesKey) === false || testOnly)) continue;
        const byPkg = files.get(file) ?? new Map();
        const entry = byPkg.get(pkg.name) ?? { pkg, targets: new Map(), gates: new Map() };
        if (!seen.has(gatesKey)) entry.gates.set(id, [...(entry.gates.get(id) ?? []), gates]);
        seen.set(gatesKey, testOnly);
        visited.set(file, seen);
        byPkg.set(pkg.name, entry);
        files.set(file, byPkg);
        entry.targets.set(id, entry.targets.has(id) ? entry.targets.get(id) && testOnly : testOnly);
        const { tokens, regions, index } = parse(file);
        for (const child of moduleChildren(file, tokens, regions, index, isRoot, crateDir, modDir)) {
          if (child.unresolved) {
            refuse(file, child.unresolved, pkg, id);
            continue;
          }
          const found = child.candidates.find((candidate) => existsSync(candidate));
          if (found) {
            queue.push([found, false, testOnly || child.testOnly, child.include ? child.include.modDir : null, [...gates, ...child.cfgs], [...chain, file]]);
          } else if (child.include) {
            refuse(file, { ...child.include, why: `its file ${normalizePath(relative(root, child.candidates[0]))} does not exist` }, pkg, id);
          }
        }
      }
    }
  }
  return { packages, files, parse, unresolved };
}

// ---------------------------------------------------------------------------
// Path expressions: a small evaluator over the token stream.

// Per-file index for the evaluator: matching brackets, the first body of
// each `fn`, and every `let`/`for` binding site with its initializer span.
export function buildIndex(tokens) {
  const match = new Int32Array(tokens.length).fill(-1);
  const stack = [];
  for (let k = 0; k < tokens.length; k += 1) {
    const tok = tokens[k];
    if (tok.t !== "p") continue;
    if (tok.v === "(" || tok.v === "[" || tok.v === "{") {
      stack.push(k);
    } else if (tok.v === ")" || tok.v === "]" || tok.v === "}") {
      const want = tok.v === ")" ? "(" : tok.v === "]" ? "[" : "{";
      let s = stack.length - 1;
      while (s >= 0 && tokens[stack[s]].v !== want) s -= 1;
      if (s >= 0) {
        match[stack[s]] = k;
        match[k] = stack[s];
        stack.length = s;
      }
    }
  }
  for (const k of stack) match[k] = tokens.length - 1;
  const skip = (j) => (match[j] > j ? match[j] : j);
  const fnBodies = new Map();
  const bindings = new Map();
  const bodies = [];
  for (let k = 0; k < tokens.length; k += 1) {
    const tok = tokens[k];
    if (tok.v === "fn" && tokens[k + 1]?.t === "id") {
      let j = k + 2;
      while (j < tokens.length && tokens[j].v !== "{" && tokens[j].v !== ";") {
        if (tokens[j].v === "(") j = skip(j);
        j += 1;
      }
      if (tokens[j]?.v === "{") {
        const body = { start: j + 1, end: skip(j), params: k + 2, nameIndex: k + 1 };
        bodies.push(body);
        if (!fnBodies.has(tokens[k + 1].v)) fnBodies.set(tokens[k + 1].v, body);
      }
      continue;
    }
    if (tok.t !== "id" || (tok.v !== "let" && tok.v !== "for")) continue;
    let j = k + 1;
    if (tokens[j]?.v === "mut") j += 1;
    const name = tokens[j];
    if (name?.t !== "id") continue;
    let span = null;
    if (tok.v === "for") {
      if (tokens[j + 1]?.v !== "in") continue;
      let stop = j + 2;
      while (stop < tokens.length && tokens[stop].v !== "{") {
        if (tokens[stop].v === "(" || tokens[stop].v === "[") stop = skip(stop);
        stop += 1;
      }
      span = { start: j + 2, end: stop };
    } else {
      let eq = j + 1;
      if (tokens[eq]?.v === ":") {
        while (eq < tokens.length && tokens[eq].v !== "=" && tokens[eq].v !== ";") eq += 1;
      }
      if (tokens[eq]?.v !== "=") continue;
      let stop = eq + 1;
      while (stop < tokens.length && tokens[stop].v !== ";") {
        if (tokens[stop].t === "p" && ["(", "[", "{"].includes(tokens[stop].v)) stop = skip(stop);
        stop += 1;
      }
      span = { start: eq + 1, end: stop };
    }
    const list = bindings.get(name.v) ?? [];
    list.push({ at: j, span });
    bindings.set(name.v, list);
  }
  // The innermost function body that holds each token (-1: none), so a
  // binding resolves only within its own function.
  const owner = new Int32Array(tokens.length).fill(-1);
  bodies.sort((a, b) => a.start - b.start);
  for (const body of bodies) owner.fill(body.start, body.start, Math.max(body.start, body.end));
  const bodyByStart = new Map(bodies.map((body) => [body.start, body]));
  return { match, fnBodies, bindings, owner, bodyByStart, memo: new Map() };
}

function matching(ctx, openIndex) {
  const end = ctx.index.match[openIndex];
  return end > openIndex ? end : ctx.tokens.length - 1;
}

// The initializer span of the latest `let name = ...;` or `for name in ...`
// before `before`.
function bindingOf(ctx, name, before) {
  const list = ctx.index.bindings.get(name);
  if (!list) return null;
  const scope = ctx.index.owner[before];
  for (let i = list.length - 1; i >= 0; i -= 1) {
    if (list[i].at < before && ctx.index.owner[list[i].at] === scope) return list[i].span;
  }
  return null;
}

// The body span of `fn name(...) ... { ... }` in this file.
function functionBody(ctx, name) {
  return ctx.index.fnBodies.get(name) ?? null;
}

// Evaluate a path expression to an absolute path, or null when unknown.
// Understands env!("CARGO_MANIFEST_DIR"), git rev-parse --show-toplevel,
// parent(), ancestors().nth(n), join/push of literals, "{}/.." format
// pieces, local `let`/`for` bindings and same-file helper functions.
function evalPath(ctx, span, depth = 0) {
  if (depth > 5 || span.start >= span.end) return null;
  const key = `${span.start}:${span.end}`;
  if (ctx.index.memo.has(key)) return ctx.index.memo.get(key);
  // Mark in progress: a recursive helper evaluates to unknown.
  ctx.index.memo.set(key, null);
  const { tokens } = ctx;
  let result = null;
  let start = span.start;
  for (let j = span.start; j <= span.end; j += 1) {
    const tok = tokens[j];
    if (j < span.end && tok.t === "p" && ["(", "[", "{"].includes(tok.v)) {
      j = Math.min(matching(ctx, j), span.end);
      continue;
    }
    if (j === span.end || (tok.t === "p" && tok.v === ";")) {
      const value = evalChain(ctx, { start, end: j }, depth);
      if (value !== null) result = value;
      start = j + 1;
    }
  }
  ctx.index.memo.set(key, result);
  return result;
}

function evalChain(ctx, span, depth) {
  const { tokens, crateDir } = ctx;
  let cur = null;
  for (let j = span.start; j < span.end; j += 1) {
    const tok = tokens[j];
    if (tok.t === "str") {
      if (tok.v === "CARGO_MANIFEST_DIR") cur = crateDir;
      else if (tok.v === "--show-toplevel") cur = root;
      else if (cur === null && /^\{[^}]*\}(\/\.\.)+\/?$/.test(tok.v) && tokens.slice(j, span.end).some((t) => t.v === "CARGO_MANIFEST_DIR")) {
        cur = resolve(crateDir, tok.v.replace(/^\{[^}]*\}\//, ""));
        return cur;
      }
      continue;
    }
    if (tok.t !== "id") continue;
    const next = tokens[j + 1];
    if (cur !== null) {
      if (tok.v === "parent" && next?.v === "(" && tokens[j + 2]?.v === ")") {
        cur = dirname(cur);
        j += 2;
      } else if ((tok.v === "and_then" || tok.v === "map") && next?.v === "(" && tokens[matching(ctx, j + 1) - 1]?.v === "parent") {
        // .and_then(Path::parent)
        cur = dirname(cur);
        j = matching(ctx, j + 1);
      } else if (tok.v === "ancestors" && next?.v === "(" && tokens[j + 3]?.v === "." && tokens[j + 4]?.v === "nth" && tokens[j + 6]?.t === "num") {
        for (let up = Number.parseInt(tokens[j + 6].v, 10); up > 0; up -= 1) cur = dirname(cur);
        j += 7;
      } else if ((tok.v === "join" || tok.v === "push") && next?.v === "(") {
        const arg = tokens[j + 2];
        if (arg?.t === "str" && tokens[j + 3]?.v === ")") {
          cur = resolve(cur, arg.v);
          j += 3;
        } else {
          return null;
        }
      }
      continue;
    }
    if (tokens[j - 1]?.v === "." || tokens[j - 1]?.v === ":") {
      // A method or path segment before any value: Path::new(...), etc.
      if (next?.v !== "(") continue;
    }
    if (next?.v === "(") {
      if (STRONG_ROOT_NAMES.has(tok.v)) {
        const body = functionBody(ctx, tok.v);
        const inside = body && body.start <= span.start && span.end <= body.end;
        const value = body && !inside ? evalPath(ctx, body, depth + 1) : null;
        cur = value ?? root;
        j = matching(ctx, j + 1);
        continue;
      }
      // Only a free call can name a same-file helper; `Path::new(...)` and
      // `.join(...)` are associated functions and methods.
      const free = tokens[j - 1]?.v !== "." && tokens[j - 1]?.v !== ":";
      const body = free ? functionBody(ctx, tok.v) : null;
      if (body) {
        // A same-file helper: evaluate a zero-argument one (repo_root()),
        // treat one with arguments as opaque (its result is not its input).
        const zeroArgs = tokens[j + 2]?.v === ")";
        const inside = body.start <= span.start && span.end <= body.end;
        const value = zeroArgs && !inside ? evalPath(ctx, body, depth + 1) : null;
        if (value !== null) cur = value;
        j = matching(ctx, j + 1);
      } else if (!PATH_CONSTRUCTORS.has(tok.v)) {
        j = matching(ctx, j + 1);
      }
      continue;
    }
    const binding = bindingOf(ctx, tok.v, j);
    if (binding) {
      const value = evalPath(ctx, binding, depth + 1);
      if (value !== null) cur = value;
      continue;
    }
    if (STRONG_ROOT_NAMES.has(tok.v)) cur = root;
  }
  return cur;
}

// The receiver expression span ending just before index `end` (exclusive).
function receiverBefore(tokens, end) {
  let j = end - 1;
  for (; j >= 0; j -= 1) {
    const tok = tokens[j];
    if (tok.t === "p" && (tok.v === ")" || tok.v === "]")) {
      // Jump back to the matching opener.
      let level = 0;
      for (; j >= 0; j -= 1) {
        if (tokens[j].v === ")" || tokens[j].v === "]") level += 1;
        else if (tokens[j].v === "(" || tokens[j].v === "[") {
          level -= 1;
          if (level === 0) break;
        }
      }
      continue;
    }
    if (tok.t === "p" && ![".", ":", "&", "?", "!"].includes(tok.v)) break;
    if (tok.t === "id" && ["let", "return", "in", "if", "while", "match", "for", "else", "move", "mut"].includes(tok.v)) break;
  }
  return { start: j + 1, end };
}

// ---------------------------------------------------------------------------
// Reference extraction.

function trackedIndex() {
  const listed = git(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
    .split("\0")
    .filter(Boolean)
    .map(normalizePath);
  const files = new Set(listed);
  const dirs = new Set();
  for (const file of listed) {
    let dir = posix.dirname(file);
    while (dir && dir !== "." && !dirs.has(dir)) {
      dirs.add(dir);
      dir = posix.dirname(dir);
    }
  }
  return { files, dirs };
}

// A repository-relative path for an absolute one, following symlinks so an
// embedded link names its target. "" is the repository root; null is
// outside the checkout.
function repoPathOf(abs) {
  let real = abs;
  try {
    real = realpathSync(abs);
  } catch {
    // Dangling or missing: keep the lexical path.
  }
  const rel = normalizePath(relative(root, real));
  if (rel.startsWith("../") || rel === ".." || rel.startsWith("/")) return null;
  return rel;
}

function existsInCheckout(rel, index) {
  if (rel === "") return true;
  return index.files.has(rel) || index.dirs.has(rel.replace(/\/$/, ""));
}

const PATH_SHAPE = /^[A-Za-z0-9_.\-/{}*@+]+$/;

// Interpret one string literal as a path.
function literalCandidates(value) {
  if (!PATH_SHAPE.test(value) || value.length < 2) return [];
  let rest = value;
  let anchored = false;
  const placeholder = /^\{[^}]*\}(?=\/)/.exec(rest);
  if (placeholder) {
    rest = rest.slice(placeholder[0].length);
    anchored = true;
  }
  if (rest.startsWith("/")) {
    // Only a concat!/format! piece after a directory: "/../../docs".
    if (!anchored && !rest.startsWith("/..")) return [];
    rest = rest.slice(1);
    anchored = true;
  }
  rest = rest.replace(/^\.\//, "");
  // A glob names the directory it walks.
  const glob = rest.search(/[*{}]/);
  if (glob !== -1) rest = rest.slice(0, glob).replace(/[^/]*$/, "");
  if (!rest) return [];
  if (rest.startsWith("../") || rest === "..") {
    // A pure climb ("..", "../..") locates a root; the joins after it name
    // the read. A broken climb (wrong depth) still names the root path
    // after it.
    if (/^(\.\.\/?)+$/.test(rest)) return [];
    return [
      { base: "crate", rel: rest },
      { base: "root-fallback", rel: rest.replace(/^(\.\.\/)+/, "") },
    ];
  }
  const first = rest.split("/")[0];
  const trimmed = rest.replace(/\/$/, "");
  if (ROOT_DIRS.includes(first) || ROOT_FILES.includes(trimmed)) {
    return [{ base: anchored ? "anchored" : "root", rel: rest, bare: !trimmed.includes("/") }];
  }
  return [];
}

// The joined path of `.join("a").join("b")...` starting at literal k.
function joinChain(tokens, k) {
  let path = tokens[k].v;
  let j = k + 1;
  while (
    tokens[j]?.v === ")" &&
    tokens[j + 1]?.v === "." &&
    tokens[j + 2]?.v === "join" &&
    tokens[j + 3]?.v === "(" &&
    tokens[j + 4]?.t === "str" &&
    tokens[j + 5]?.v === ")"
  ) {
    path = `${path.replace(/\/$/, "")}/${tokens[j + 4].v}`;
    j += 4;
  }
  return path;
}

// An array of literals one of which is an existing multi-segment
// repository path: `["docs", "docs/internal", ".claude/skills"]`.
function inRepoPathArray(tokens, k, index) {
  const before = tokens[k - 1]?.v;
  const after = tokens[k + 1]?.v;
  if (!((before === "[" || before === ",") && (after === "," || after === "]"))) return false;
  let start = k;
  while (start > 0 && tokens[start - 1].v !== "[") {
    if ([";", "{", "}"].includes(tokens[start - 1].v)) return false;
    start -= 1;
  }
  for (let j = start; j < tokens.length && tokens[j].v !== "]"; j += 1) {
    if (j === k || tokens[j].t !== "str") continue;
    const value = tokens[j].v.replace(/\/$/, "");
    if (value.includes("/") && ROOT_DIRS.includes(value.split("/")[0]) && existsInCheckout(value, index)) return true;
  }
  return false;
}

// The macros that compile a file into the crate.
const COMPILED_IN = new Set(["include_str", "include_bytes"]);

export function scanReferences(model, index) {
  const refs = [];
  for (const [file, byPkg] of model.files) {
    const source = normalizePath(relative(root, file));
    const { tokens, regions, index: fileIndex } = model.parse(file);
    for (const { pkg, targets } of byPkg.values()) {
      const pkgDir = packageDir(pkg);
      const crateDir = resolve(root, pkgDir);
      const ctx = { tokens, crateDir, index: fileIndex };
      fileIndex.memo.clear();
      const consumed = new Set();
      const push = (k, kind, abs, literal) => {
        const path = repoPathOf(abs);
        if (path === null) return;
        // The crate's own files select it, except its documentation (.md,
        // .mdx), which the planner treats as no build input: an in-crate doc
        // that include_str!/include_bytes! compiles in is listed like an
        // outside input.
        const inCrate = path !== "" && (path === pkgDir || path.startsWith(`${pkgDir}/`));
        if (inCrate && !(COMPILED_IN.has(kind) && /\.mdx?$/.test(path))) return;
        if (!existsInCheckout(path, index)) return;
        const isDir = path === "" || !index.files.has(path);
        // The #[test] function that holds the reference, if any.
        const body = fileIndex.bodyByStart.get(fileIndex.owner[k]);
        const testFn = body && regions.testFns.has(body.nameIndex) ? tokens[body.nameIndex].v : null;
        for (const [target, fileTestOnly] of targets) {
          const testOnly = fileTestOnly || regions.inTest[k] === 1;
          refs.push({
            testFn,
            source,
            line: tokens[k].line,
            kind,
            literal,
            path: path === "" ? "./" : isDir ? `${path.replace(/\/$/, "")}/` : path,
            package: pkg.name,
            target,
            scope: testOnly || target.startsWith("example:") ? "target" : "package",
          });
        }
      };
      for (let k = 0; k < tokens.length; k += 1) {
        const tok = tokens[k];
        // include_str!/include_bytes!, and include!() of source outside the
        // crate (the walk scans the included file itself).
        if (tok.t === "id" && ["include_str", "include_bytes", "include"].includes(tok.v) && tokens[k + 1]?.v === "!" && tokens[k + 2]?.v === "(" && tokens[k - 1]?.v !== "fn") {
          const arg = includeArgument(tokens, k + 2, fileIndex.match);
          for (let j = k + 3; j < arg.close; j += 1) {
            if (tokens[j].t === "str") consumed.add(j);
          }
          if (arg.kind === "literal") {
            push(k + 3, tok.v, resolve(dirname(file), arg.value), arg.value);
          } else if (arg.kind === "manifest") {
            push(k + 3, tok.v, resolve(crateDir, arg.value.replace(/^\//, "")), arg.value);
          } else if (arg.kind === "unresolved" && tok.v !== "include") {
            // The walk refuses an unresolved include!() itself.
            const key = `${source}:${tok.line}`;
            const entry = model.unresolved.get(key) ?? {
              source,
              line: tok.line,
              form: `${tok.v}!(${arg.form})`,
              why: "the scan resolves a literal, concat!(env!(\"CARGO_MANIFEST_DIR\"), ...) or an OUT_DIR concat! only",
              readers: new Set(),
            };
            for (const target of targets.keys()) entry.readers.add(`${pkg.name} ${target}`);
            model.unresolved.set(key, entry);
          }
          continue;
        }
        // #[path = "..."] modules.
        if (tok.v === "#" && tokens[k + 1]?.v === "[" && tokens[k + 2]?.v === "path" && tokens[k + 3]?.v === "=" && tokens[k + 4]?.t === "str") {
          consumed.add(k + 4);
          push(k + 4, "mod-path", resolve(dirname(file), tokens[k + 4].v), tokens[k + 4].v);
          continue;
        }
        // Directory walks of an evaluated directory: walk_files(&root, ...).
        if (tok.t === "id" && WALK_CALL.test(tok.v) && tokens[k + 1]?.v === "(" && tokens[k - 1]?.v !== "fn") {
          let j = k + 2;
          if (tokens[j]?.v === "&") j += 1;
          let stop = j;
          while (stop < tokens.length && tokens[stop].v !== "," && tokens[stop].v !== ")") {
            if (["(", "["].includes(tokens[stop].v)) stop = matching(ctx, stop);
            stop += 1;
          }
          const hasLiteral = tokens.slice(j, stop).some((t) => t.t === "str");
          if (!hasLiteral && stop > j) {
            const value = evalPath(ctx, { start: j, end: stop });
            if (value !== null) push(k, "walk", value, tokens.slice(j, stop).map((t) => t.v).join(""));
          }
        }
        // Any other call handed the repository root itself reads under it
        // (a live-workspace audit, a command run in the checkout).
        // A helper named for a path (machine_model_path(root)) computes one;
        // the read that follows it is not resolvable here.
        const pathHelper = /_(path|paths|dir|dirs)$/.test(tok.v);
        if (tok.t === "id" && tokens[k + 1]?.v === "(" && tokens[k - 1]?.v !== "fn" && !NON_READING_CALLS.has(tok.v) && !WALK_CALL.test(tok.v) && !pathHelper) {
          const close = matching(ctx, k + 1);
          let argStart = k + 2;
          for (let j = k + 2; j <= close; j += 1) {
            if (j < close && ["(", "[", "{"].includes(tokens[j].v) && tokens[j].t === "p") {
              j = matching(ctx, j);
              continue;
            }
            if (j === close || tokens[j].v === ",") {
              let a = argStart;
              if (tokens[a]?.v === "&") a += 1;
              const isPlain = j - a === 1 && tokens[a].t === "id" || (j - a === 3 && tokens[a].t === "id" && tokens[a + 1].v === "(" && tokens[a + 2].v === ")");
              if (isPlain && evalPath(ctx, { start: a, end: j }) === root) {
                push(k, "root-arg", root, `${tok.v}(${tokens.slice(a, j).map((t) => t.v).join("")})`);
                break;
              }
              argStart = j + 1;
            }
          }
        }
        if (tok.t !== "str" || consumed.has(k)) continue;
        const rerun = /^cargo::?rerun-if-changed=(.+)$/.exec(tok.v);
        if (rerun) {
          if (!rerun[1].includes("{")) push(k, "rerun-if-changed", resolve(crateDir, rerun[1]), tok.v);
          continue;
        }
        // `.join("a").join("b")` names "a/b".
        const joined = tokens[k - 1]?.v === "(" && ["join", "push"].includes(tokens[k - 2]?.v) && tokens[k - 3]?.v === ".";
        if (joined && tokens[k - 4]?.v === ")" && tokens[k - 5]?.t === "str" && tokens[k - 6]?.v === "(" && tokens[k - 7]?.v === "join") {
          continue; // already part of the chain that starts earlier
        }
        const value = joined ? joinChain(tokens, k) : tok.v;
        let receiver = null;
        let receiverKnown = false;
        if (joined) {
          const span = receiverBefore(tokens, k - 3);
          receiver = evalPath(ctx, span);
          receiverKnown = receiver !== null;
          if (!receiverKnown) {
            // The chain's base names (not field or method names): `paths.root`
            // is a field of `paths`, not the repository root.
            const names = tokens
              .slice(span.start, span.end)
              .filter((t, i) => t.t === "id" && tokens[span.start + i - 1]?.v !== ".")
              .map((t) => t.v);
            if (names.some((name) => STRONG_ROOT_NAMES.has(name) || WEAK_ROOT_NAMES.has(name))) receiver = root;
          }
        }
        if (joined && receiverKnown) {
          // The receiver is known: resolve against it. When that names
          // nothing in the checkout, read the literal on its own below.
          const candidate = value.replace(/^\.\//, "");
          if (PATH_SHAPE.test(candidate) && !/^(\.\.\/?)+$/.test(candidate)) {
            const abs = resolve(receiver, candidate.replace(/[*{].*$/, ""));
            const rel = repoPathOf(abs);
            if (rel !== null && existsInCheckout(rel, index)) {
              push(k, "join", abs, value);
              continue;
            }
          } else {
            continue;
          }
          receiver = null;
        }
        for (const candidate of literalCandidates(value)) {
          if (candidate.base === "crate") {
            const abs = resolve(crateDir, candidate.rel);
            if (existsInCheckout(repoPathOf(abs) ?? "\0", index)) {
              push(k, "path-literal", abs, value);
              break;
            }
            continue;
          }
          if (candidate.base === "root-fallback") {
            if (existsInCheckout(candidate.rel.replace(/\/$/, ""), index)) push(k, "path-literal", resolve(root, candidate.rel), value);
            continue;
          }
          // A crate-local entry of the same name wins: crate tests run with
          // the crate directory as their working directory.
          if (candidate.base === "root" && existsSync(resolve(crateDir, candidate.rel))) continue;
          if (candidate.base === "root") {
            if (joined && receiver === null) continue;
            if (candidate.bare && !joined && !inRepoPathArray(tokens, k, index)) continue;
          }
          push(k, "path-literal", resolve(root, candidate.rel), value);
        }
      }
    }
  }
  refs.sort(
    (a, b) =>
      a.source.localeCompare(b.source) ||
      a.line - b.line ||
      a.path.localeCompare(b.path) ||
      a.package.localeCompare(b.package) ||
      a.target.localeCompare(b.target),
  );
  return refs;
}

// ---------------------------------------------------------------------------
// Inventory check.

// Group the references by (path, package, target).
export function groupReferences(refs) {
  const groups = new Map();
  for (const ref of refs) {
    const key = `${ref.path}\0${ref.package}\0${ref.target}`;
    const group = groups.get(key) ?? {
      path: ref.path,
      package: ref.package,
      target: ref.target,
      scope: "target",
      sources: new Set(),
      first: ref,
    };
    group.sources.add(ref.source);
    if (ref.scope === "package") group.scope = "package";
    groups.set(key, group);
  }
  return groups;
}

export function checkInventory(refs, inventory, model) {
  const problems = [];
  for (const site of [...model.unresolved.values()].sort((a, b) => a.source.localeCompare(b.source) || a.line - b.line)) {
    problems.push(
      `unresolved include: ${site.source}:${site.line} ${site.form} (${[...site.readers].sort().join(", ")}): ${site.why}; the guard did not read that source, so the inventory cannot be complete`,
    );
  }
  const live = [];
  for (const ref of refs) {
    const ignored = inventory.notReaders.some((row) => row.source === ref.source && row.literal === ref.literal);
    if (!ignored) live.push(ref);
  }
  for (const row of inventory.notReaders) {
    if (!refs.some((ref) => ref.source === row.source && ref.literal === row.literal)) {
      problems.push(`stale not_readers row: ${row.source} ${JSON.stringify(row.literal)} is no longer a scanned reference`);
    }
  }
  const groups = groupReferences(live);
  const rows = new Map(inventory.entries.map((entry) => [`${entry.path}\0${entry.package}\0${entry.target}`, entry]));
  for (const [key, group] of groups) {
    const row = rows.get(key);
    const ref = group.first;
    if (!row) {
      problems.push(
        `missing reader: ${ref.source}:${ref.line} reads ${group.path} (${group.package} ${group.target}, scope ${group.scope}) via ${ref.kind} ${JSON.stringify(ref.literal)}; add it under "${group.path}" in ${INVENTORY_PATH}`,
      );
      continue;
    }
    const listed = [...row.sources].sort();
    const scanned = [...group.sources].sort();
    if (listed.join("\n") !== scanned.join("\n")) {
      problems.push(
        `sources differ for ${group.path} (${group.package} ${group.target}): inventory lists [${listed.join(", ")}], scan found [${scanned.join(", ")}]`,
      );
    }
    if ((row.scope ?? "target") !== group.scope) {
      problems.push(`scope differs for ${group.path} (${group.package} ${group.target}): inventory says ${row.scope ?? "target"}, scan found ${group.scope}`);
    }
  }
  for (const [key, row] of rows) {
    if (!groups.has(key)) {
      problems.push(
        `stale inventory entry: "${row.path}" lists ${row.package} ${row.target} (${row.sources.join(", ")}), but no scanned reference there reads it`,
      );
    }
  }
  // Rows must name real packages, targets, features and test functions.
  const byName = new Map(model.packages.map((pkg) => [pkg.name, pkg]));
  const targetFiles = new Map();
  for (const [file, byPkg] of model.files) {
    for (const { pkg, targets } of byPkg.values()) {
      for (const target of targets.keys()) {
        const key = `${pkg.name}\0${target}`;
        if (!targetFiles.has(key)) targetFiles.set(key, []);
        targetFiles.get(key).push(file);
      }
    }
  }
  for (const row of inventory.entries) {
    const pkg = byName.get(row.package);
    if (!pkg) {
      problems.push(`inventory row "${row.path}" names unknown package ${row.package}`);
      continue;
    }
    const target = pkg.targets.find((candidate) => targetId(candidate) === row.target);
    if (!target) {
      problems.push(`inventory row "${row.path}" names unknown target ${row.package} ${row.target}`);
      continue;
    }
    for (const feature of [...row.features, ...(target["required-features"] ?? [])]) {
      if (!Object.hasOwn(pkg.features, feature)) problems.push(`inventory row "${row.path}" (${row.package} ${row.target}) names unknown feature ${feature}`);
    }
    for (const feature of target["required-features"] ?? []) {
      if (!row.features.includes(feature)) problems.push(`inventory row "${row.path}" (${row.package} ${row.target}) must list required feature ${feature}`);
    }
    for (const test of row.tests ?? []) {
      const files = targetFiles.get(`${row.package}\0${row.target}`) ?? [];
      const sites = files.flatMap((file) => {
        const chains = model.files.get(file).get(row.package).gates.get(row.target) ?? [[]];
        return fnCfgs(model.parse(file).tokens, test).flatMap((site) => chains.map((chain) => [...chain, ...site]));
      });
      if (sites.length === 0) {
        problems.push(`inventory row "${row.path}" (${row.package} ${row.target}) names test ${test}, which no file of that target defines`);
        continue;
      }
      // A test the row's build compiles out runs in no lane, and the row's
      // `--no-tests=fail` fails when it names no other test.
      const enabled = enabledFeatures(pkg, row.features);
      if (!sites.some((site) => site.every((cfg) => evalCfg(cfg, enabled) !== false))) {
        const gates = [...new Set(sites.flatMap((site) => site.filter((cfg) => evalCfg(cfg, enabled) === false).map(cfgText)))];
        const build = row.features.length ? `--features ${row.features.join(",")}` : "default features";
        problems.push(`inventory row "${row.path}" (${row.package} ${row.target}) names test ${test}, which its build (${build}) compiles out: cfg(${gates.join("), cfg(")}); list the feature that compiles it in the row's "features" (a feature another dev-dependency turns on is not counted)`);
      }
    }
  }
  return problems;
}

function main() {
  const args = process.argv.slice(2);
  let inventoryPath = resolve(root, INVENTORY_PATH);
  let mode = "check";
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--inventory") inventoryPath = resolve(args[++i] ?? "");
    else if (args[i] === "--list") mode = "list";
    else if (args[i] === "--json") mode = "json";
    else throw new Error(`unknown argument: ${args[i]}`);
  }
  const model = workspaceModel();
  const index = trackedIndex();
  const refs = scanReferences(model, index);
  if (mode !== "check") {
    for (const site of model.unresolved.values()) {
      process.stderr.write(`ci-non-rust-readers: unresolved include: ${site.source}:${site.line} ${site.form}: ${site.why}\n`);
    }
  }
  if (mode === "json") {
    process.stdout.write(`${JSON.stringify(refs, null, 2)}\n`);
    return;
  }
  if (mode === "list") {
    for (const ref of refs) {
      process.stdout.write(`${ref.source}:${ref.line}\t${ref.package}\t${ref.target}\t${ref.scope}\t${ref.kind}\t${ref.path}\t${JSON.stringify(ref.literal)}\n`);
    }
    process.stderr.write(`ci-non-rust-readers: ${refs.length} references in ${model.files.size} Rust files\n`);
    return;
  }
  const inventory = loadNonRustReaders(inventoryPath);
  const problems = checkInventory(refs, inventory, model);
  const summary = `${refs.length} references in ${model.files.size} Rust files; ${inventory.entries.length} inventory rows, ${inventory.notReaders.length} not_readers rows`;
  if (problems.length) {
    for (const problem of problems) process.stderr.write(`ci-non-rust-readers: ${problem}\n`);
    process.stderr.write(`ci-non-rust-readers: FAIL, ${problems.length} problem(s) (${summary})\n`);
    process.exit(1);
  }
  process.stdout.write(`ci-non-rust-readers: OK (${summary})\n`);
}

try {
  main();
} catch (error) {
  process.stderr.write(`ci-non-rust-readers: ${error?.stack ?? error}\n`);
  process.exit(1);
}
