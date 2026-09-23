#!/usr/bin/env node
// Fail-closed contract test for scripts/ci-cargo-lanes.mjs, the changed-path
// classifier behind the GitHub-hosted Cargo PR CI. Each fixture is a changed
// path list; the assertions pin the property the old `changed-paths`
// BuildBuddy mode violated: a build-relevant change always yields lanes.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const classifier = resolve(here, "ci-cargo-lanes.mjs");
const root = resolve(here, "..");

function run(args, { input } = {}) {
  const result = spawnSync("node", [classifier, ...args], {
    cwd: root,
    encoding: "utf8",
    input,
    maxBuffer: 64 * 1024 * 1024,
  });
  return result;
}

function planFor(paths, extra = []) {
  const result = run(["--format", "json", ...extra, "--", ...paths]);
  assert.equal(result.status, 0, `classifier failed: ${result.stderr}`);
  return JSON.parse(result.stdout);
}

function assertLanes(plan, label) {
  assert.equal(plan.rust_changed, true, `${label}: rust_changed`);
  assert.ok(plan.shards.length >= 1, `${label}: at least one clippy/unit shard`);
  const covered = new Set(plan.shards.flatMap((shard) => shard.packages));
  for (const name of plan.packages) {
    assert.ok(covered.has(name), `${label}: ${name} is covered by a shard`);
  }
  for (const shard of plan.shards) {
    assert.match(shard.packages_flags ?? shard.package_flags, /^-p \S+( -p \S+)*$/, `${label}: shard flags`);
  }
  assert.ok(plan.closure.length >= plan.packages.length, `${label}: closure contains the packages`);
  for (const name of plan.packages) {
    assert.ok(plan.closure.includes(name), `${label}: closure includes ${name}`);
  }
}

// Core touch: the direct lane is meerkat-core alone; the closure is nearly
// the whole workspace (everything depends on core).
{
  const plan = planFor(["meerkat-core/src/lib.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-core"]);
  assertLanes(plan, "core touch");
  assert.ok(plan.closure.length >= 40, `core closure spans the workspace (${plan.closure.length})`);
  assert.ok(plan.closure.includes("meerkat-mob"), "core closure includes meerkat-mob");
  assert.ok(plan.closure.includes("rkat"), "core closure includes rkat");
  assert.equal(plan.shards.length, 1);
  assert.equal(plan.wasm, true, "core is in the wasm runtime closure");
  assert.equal(plan.machine_authority, false);
}

// Docs-only: no Cargo lanes, and the plan says so explicitly rather than
// erroring or emitting a placeholder shard.
{
  const plan = planFor(["docs/guides/skills.mdx", "CHANGELOG.md", "README.md"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.mode, "none");
  assert.deepEqual(plan.shards, []);
  assert.deepEqual(plan.packages, []);
  assert.equal(plan.docs_only, true);
  assert.equal(plan.generated_contract, false);
}

// Documentation inside a crate directory is not a build input either.
{
  const plan = planFor(["meerkat-core/README.md"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.mode, "none");
}

// Cargo.lock-only: the lock changes what every crate compiles against, so
// the plan is the whole workspace, packed into bounded shards that cover
// every member.
{
  const plan = planFor(["Cargo.lock"], ["--workspace-shards", "8"]);
  assert.equal(plan.mode, "workspace");
  assertLanes(plan, "Cargo.lock");
  assert.ok(plan.packages.length >= 45, `workspace mode lists every member (${plan.packages.length})`);
  assert.deepEqual(plan.closure, plan.packages);
  assert.ok(plan.shards.length <= 8 && plan.shards.length >= 2, `bounded shard count (${plan.shards.length})`);
  // Cost-balanced, not count-balanced: no shard carries more than a quarter
  // of the estimated workspace cost, so the heaviest crates spread out.
  const total = plan.shards.reduce((sum, shard) => sum + shard.estimated_cost, 0);
  for (const shard of plan.shards) {
    assert.ok(shard.estimated_cost <= total / 4, `${shard.name} is not overloaded`);
  }
  const covered = plan.shards.flatMap((shard) => shard.packages).sort();
  assert.deepEqual(covered, plan.packages, "shards partition the workspace exactly once");
  assert.equal(plan.wasm, true);
}

// Root manifest, Cargo config, nextest config, the toolchain pin, the build
// wrapper, and the workflow itself are all global.
for (const path of [
  "Cargo.toml",
  ".cargo/config.toml",
  ".config/nextest.toml",
  "rust-toolchain.toml",
  "scripts/repo-cargo",
  "scripts/cargo-agent-gate",
  "scripts/ci-cargo-lanes.mjs",
  ".github/workflows/ci.yml",
  ".github/actions/setup-rust-ci/action.yml",
]) {
  const plan = planFor([path]);
  assert.equal(plan.mode, "workspace", `${path} escalates to the workspace`);
  assertLanes(plan, path);
}

// Machine-authority-only: TLA+ models and machine specs are not Cargo
// inputs, but the governance flag must fire so machine-check-drift runs.
{
  const plan = planFor(["specs/machines/meerkat_machine/model.tla", "specs/compositions/meerkat_mob_seam/ci.cfg"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.mode, "none");
  assert.equal(plan.machine_authority, true);
  assert.deepEqual(plan.shards, []);
}

// A generated machine kernel is both machine authority and a Rust source
// owned by meerkat-machine-kernels.
{
  const plan = planFor(["meerkat-machine-kernels/src/generated/meerkat.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-machine-kernels"]);
  assert.equal(plan.machine_authority, true);
  assertLanes(plan, "generated kernel");
}

// A Rust source outside every workspace package cannot be attributed, so it
// fails closed into the workspace instead of into nothing.
{
  const plan = planFor(["examples/001-hello-meerkat-rs/main.rs"]);
  assert.equal(plan.mode, "workspace");
  assert.deepEqual(plan.unmapped_rust_paths, ["examples/001-hello-meerkat-rs/main.rs"]);
  assertLanes(plan, "unmapped .rs");
}

// A crate manifest selects its crate (and the closure through dependents).
{
  const plan = planFor(["meerkat-openai/Cargo.toml", "meerkat/src/help.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat", "meerkat-openai"]);
  assertLanes(plan, "manifest + facade");
  assert.ok(plan.closure.includes("meerkat-mob"));
  assert.equal(plan.shards.length, 2, "one shard per changed package below the shard cap");
}

// Many changed packages are packed into the shard cap, never dropped.
{
  const paths = [
    "meerkat-core/src/lib.rs",
    "meerkat-runtime/src/lib.rs",
    "meerkat-mob/src/lib.rs",
    "meerkat-rpc/src/lib.rs",
    "meerkat-rest/src/lib.rs",
    "meerkat-cli/src/main.rs",
    "meerkat-session/src/lib.rs",
    "meerkat-store/src/lib.rs",
    "xtask/src/main.rs",
  ];
  const plan = planFor(paths, ["--max-shards", "4"]);
  assert.equal(plan.mode, "packages");
  assert.equal(plan.packages.length, 9);
  assert.equal(plan.shards.length, 4);
  assertLanes(plan, "nine packages in four shards");
}

// Embedded inputs the facade compiles in through include macros select the
// facade even though they are Markdown outside its directory.
{
  const plan = planFor([".claude/skills/meerkat-platform/SKILL.md"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat"]);
  assertLanes(plan, "embedded skill doc");
}

// Generated-contract and SDK-host flags.
{
  const plan = planFor(["artifacts/schemas/version.json", "sdks/python/meerkat/client.py"]);
  assert.equal(plan.generated_contract, true);
  assert.equal(plan.sdk_host, true);
}

// No diff base at all: the plan is the whole workspace, never nothing.
{
  const result = run(["--format", "json"]);
  assert.equal(result.status, 0, result.stderr);
  const plan = JSON.parse(result.stdout);
  assert.equal(plan.mode, "workspace");
  assertLanes(plan, "no base");
}
{
  const result = run(["--format", "json", "--base", "0000000000000000000000000000000000000000"]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).mode, "workspace");
}
{
  const result = run(["--format", "json", "--base", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "--head", "HEAD"]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).mode, "workspace", "unknown base escalates to the workspace");
}

// GitHub output: every key the workflow reads, and a well-formed matrix.
{
  const result = run(["--format", "github", "--", "meerkat-core/src/lib.rs"]);
  assert.equal(result.status, 0, result.stderr);
  const lines = Object.fromEntries(
    result.stdout.trim().split("\n").map((line) => {
      const index = line.indexOf("=");
      return [line.slice(0, index), line.slice(index + 1)];
    }),
  );
  for (const key of [
    "rust_changed",
    "mode",
    "reason",
    "generated_contract",
    "machine_authority",
    "wasm",
    "sdk_host",
    "docs_only",
    "package_count",
    "closure_count",
    "closure_flags",
    "shard_count",
    "shard_matrix",
  ]) {
    assert.ok(key in lines, `github output has ${key}`);
  }
  assert.equal(lines.rust_changed, "true");
  const matrix = JSON.parse(lines.shard_matrix);
  assert.deepEqual(matrix.include, [{ name: "core", packages: "-p meerkat-core" }]);
  assert.match(lines.closure_flags, /-p meerkat-core/);
}
{
  const result = run(["--format", "github", "--", "docs/index.mdx"]);
  const matrixLine = result.stdout.split("\n").find((line) => line.startsWith("shard_matrix="));
  assert.deepEqual(JSON.parse(matrixLine.slice("shard_matrix=".length)).include, [{ name: "none", packages: "" }]);
  assert.ok(result.stdout.includes("rust_changed=false"));
}

// Bad arguments fail loudly.
{
  const result = run(["--format", "yaml", "--", "meerkat-core/src/lib.rs"]);
  assert.notEqual(result.status, 0, "invalid format is an error");
}

console.log("ci-cargo-lanes fail-closed contracts hold");
