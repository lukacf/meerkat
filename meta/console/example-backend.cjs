"use strict";

// How an e2e suite starts a Rust example backend.
//
// By default this is exactly what the suites have always done: ask cargo to
// build and run the example. CI wants to build the meerkat-meta dependency
// graph ONCE and hand the resulting binaries to several parallel jobs, which
// requires the suites to be able to exec a path instead.
//
// Handing `cargo run --example X` a bare prebuilt binary does not work: cargo
// recomputes freshness from target/.fingerprint before running, so given only
// the output file it rebuilds and the uploaded artifact is paid for and then
// ignored. The suite has to bypass cargo, not be given a warmer cargo.
//
// META_EXAMPLE_BIN_DIR - directory of prebuilt example binaries. Unset, which
// is the local-development default, changes nothing.

const fs = require("node:fs");
const path = require("node:path");

function exampleBackendSpec(repoRoot, exampleName) {
  const dir = process.env.META_EXAMPLE_BIN_DIR;
  if (!dir) {
    return {
      command: path.join(repoRoot, "scripts", "repo-cargo"),
      args: ["run", "-p", "meerkat-meta", "--example", exampleName],
      prebuilt: false,
    };
  }

  const binary = path.join(
    dir,
    process.platform === "win32" ? `${exampleName}.exe` : exampleName,
  );

  // Fail rather than fall back. A silent fallback to cargo would still pass the
  // suite, just slowly, so a mis-wired artifact download would look like a
  // successful compile-once run instead of a broken one - and the measurement
  // it exists to produce would be quietly wrong.
  if (!fs.existsSync(binary)) {
    throw new Error(
      `META_EXAMPLE_BIN_DIR=${dir} is set but ${binary} is missing. ` +
        `Refusing to fall back to 'cargo run', which would hide the missing ` +
        `artifact behind a slow pass.`,
    );
  }

  return { command: binary, args: [], prebuilt: true };
}

module.exports = { exampleBackendSpec };
