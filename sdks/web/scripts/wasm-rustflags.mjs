// The rustc flags the @rkat/web wasm runtime is built with, folded with
// whatever flags the calling environment already carries.
//
// Cargo takes rustflags from exactly ONE source, the first of
// CARGO_ENCODED_RUSTFLAGS, RUSTFLAGS, and target.<triple>.rustflags (which
// CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS sets). build-wasm.mjs used to
// pass its flags through the last of those, so any ambient RUSTFLAGS silently
// replaced them: the release job set RUSTFLAGS for the getrandom cfg, and every
// published @rkat/web from 0.8.30 through 0.8.44 lost `-zstack-size` and
// shipped a 1 MiB wasm stack. The flags are now passed through the source
// Cargo reads first, with every ambient source folded in ahead of them, so the
// required flags are always present and always last.

import { WASM_STACK_RUSTFLAGS } from "./wasm-stack.mjs";

const UNIT_SEPARATOR = "\x1f";

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
 * The flags to build with: every ambient rustflags source (target-specific,
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

/**
 * The child environment for the wasm build: the folded flags in
 * CARGO_ENCODED_RUSTFLAGS (the source Cargo reads first) and no other
 * rustflags source left to compete with it. wasm-pack builds with an explicit
 * `--target`, so these reach the wasm artifacts only, never host build
 * scripts or proc macros.
 */
export function wasmBuildEnv(env) {
  const flags = effectiveWasmRustflags(env);
  const child = { ...env };
  for (const key of RUSTFLAGS_ENV_KEYS) {
    delete child[key];
  }
  child.CARGO_ENCODED_RUSTFLAGS = flags.join(UNIT_SEPARATOR);
  return { env: child, flags };
}
