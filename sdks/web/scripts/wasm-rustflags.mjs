// The rustc flags the @rkat/web wasm runtime is built with, folded with
// whatever flags the calling environment already carries.
//
// Cargo takes rustflags from exactly ONE source: CARGO_ENCODED_RUSTFLAGS,
// else RUSTFLAGS, else every matching `target.<triple>.rustflags` and
// `target.'cfg(..)'.rustflags` entry joined (config files, the
// CARGO_TARGET_<TRIPLE>_RUSTFLAGS variable, and `--config`), else
// `build.rustflags`. build-wasm.mjs used to pass its flags through
// CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS, so any ambient RUSTFLAGS
// silently replaced them: the release job set RUSTFLAGS for the getrandom
// cfg, and every published @rkat/web from 0.8.30 through 0.8.44 lost
// `-zstack-size` and shipped a 1 MiB wasm stack.
//
// Now every ambient environment source is folded ahead of the required flags
// and the result reaches Cargo as `cargo build --config
// target.wasm32-unknown-unknown.rustflags=[...]` (wasm-pack's extra options),
// with no rustflags variable left in the build's environment:
//   - Cargo joins that array with the config files' `target.wasm32-...` and
//     `target.'cfg(..)'` rustflags, so config-file flags are kept, not dropped;
//   - no environment variable outranks it. A config-file
//     `target.'cfg(..)'.rustflags` entry is still appended after it, so a
//     cfg-keyed `-zstack-size` there wins at link; the post-build stack guard
//     reports that as "overridden or lost";
//   - wasm-pack's `cargo install wasm-bindgen-cli` fallback (a host build)
//     inherits no wasm-only flag, so it never sees `-zstack-size`.
// One limit, Cargo's own rule: `build.rustflags` applies only when no
// target rustflags do, so it is not used for this build (it was not before
// either: the target variable had the same effect).

import { WASM_STACK_RUSTFLAGS } from "./wasm-stack.mjs";

const UNIT_SEPARATOR = "\x1f";
const WASM_TARGET = "wasm32-unknown-unknown";

/** Flags the runtime needs regardless of the environment. */
export const REQUIRED_WASM_RUSTFLAGS = ['--cfg', 'getrandom_backend="wasm_js"', ...WASM_STACK_RUSTFLAGS];

/** Environment variables that carry rustflags, in the order they are folded. */
export const RUSTFLAGS_ENV_KEYS = [
  "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS",
  "RUSTFLAGS",
  "CARGO_ENCODED_RUSTFLAGS",
];

function whitespaceFlags(value) {
  return (value ?? "").split(/\s+/).filter(Boolean);
}

/**
 * The flags to build with: every ambient rustflags variable (target-specific,
 * then RUSTFLAGS, then CARGO_ENCODED_RUSTFLAGS, each split the way Cargo
 * splits it), followed by {@link REQUIRED_WASM_RUSTFLAGS}.
 */
export function effectiveWasmRustflags(env) {
  const encoded = env.CARGO_ENCODED_RUSTFLAGS;
  return [
    ...whitespaceFlags(env.CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS),
    ...whitespaceFlags(env.RUSTFLAGS),
    ...(encoded ? encoded.split(UNIT_SEPARATOR).filter(Boolean) : []),
    ...REQUIRED_WASM_RUSTFLAGS,
  ];
}

/** `flags` as a TOML array of basic strings (JSON strings are valid TOML). */
function tomlStringArray(flags) {
  return `[${flags.map((flag) => JSON.stringify(flag)).join(", ")}]`;
}

/**
 * How to run the wasm build: an environment with no rustflags variable left,
 * and the Cargo arguments that carry the folded flags. wasm-pack passes
 * `cargoArgs` to `cargo build` after `--`.
 */
export function wasmBuildEnv(env) {
  const flags = effectiveWasmRustflags(env);
  const child = { ...env };
  for (const key of RUSTFLAGS_ENV_KEYS) {
    delete child[key];
  }
  return {
    env: child,
    flags,
    cargoArgs: ["--config", `target.${WASM_TARGET}.rustflags=${tomlStringArray(flags)}`],
  };
}
