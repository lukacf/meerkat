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
  // Unit plan: every changed package is either in a pull-request unit shard
  // or named as deferred to the push-to-main run, never dropped.
  const unitCovered = plan.unit_shards.flatMap((shard) => shard.packages);
  assert.deepEqual(
    [...unitCovered, ...plan.unit_deferred].sort(),
    [...plan.packages].sort(),
    `${label}: unit shards plus deferred packages partition the changed packages`,
  );
  for (const name of plan.unit_deferred) {
    assert.equal(plan.package_model[name].heavy_chain, true, `${label}: ${name} is deferred only because it compiles meerkat-mob`);
  }
  for (const shard of plan.unit_shards) {
    assert.ok(
      shard.estimated_minutes <= plan.pr_unit_budget_minutes,
      `${label}: pull-request unit lane ${shard.name} models ${shard.estimated_minutes} min, over the ${plan.pr_unit_budget_minutes} min budget`,
    );
    for (const name of shard.packages) {
      assert.equal(plan.package_model[name].heavy_chain, false, `${label}: ${name} must not run its unit tests in the pull request`);
    }
  }
  // The push-to-main plan covers the whole workspace: the packed shards
  // plus the archived (build once, partitioned run) lanes, never both.
  const archivedPackages = plan.main_archive_builds
    .filter((build) => !build.package_flags.includes("--features"))
    .map((build) => build.package_flags.replace(/^-p /, ""));
  const packed = plan.main_unit_shards.flatMap((shard) => shard.packages);
  for (const name of archivedPackages) {
    assert.ok(!packed.includes(name), `${label}: archived package ${name} is not also packed`);
  }
  assert.deepEqual(
    [...packed, ...archivedPackages].sort(),
    Object.keys(plan.package_model).sort(),
    `${label}: main unit shards and archived lanes cover every workspace package`,
  );
  assertArchivedLanes(plan, label);
}

// Every archive build is executed by exactly its partitions, 1..n of n, and
// names unique archives (they name matrix rows and artifacts).
function assertArchivedLanes(plan, label) {
  const archives = plan.main_archive_builds.map((build) => build.archive);
  assert.equal(new Set(archives).size, archives.length, `${label}: archive names are unique`);
  for (const build of plan.main_archive_builds) {
    assert.match(build.archive, /^[A-Za-z0-9_-]+$/, `${label}: archive ${build.archive} is artifact-safe`);
    assert.ok(build.partitions >= 2, `${label}: ${build.name} fans out`);
    const partitions = plan.main_archive_runs
      .filter((run) => run.archive === build.archive)
      .map((run) => run.partition);
    assert.deepEqual(
      partitions,
      Array.from({ length: build.partitions }, (_, index) => `hash:${index + 1}/${build.partitions}`),
      `${label}: ${build.name} runs every partition exactly once`,
    );
  }
  assert.equal(
    plan.main_archive_runs.length,
    plan.main_archive_builds.reduce((sum, build) => sum + build.partitions, 0),
    `${label}: no partition run without its archive build`,
  );
}

// Core touch: the direct lane is meerkat-core alone; the closure is nearly
// the whole workspace (everything depends on core).
{
  const plan = planFor(["crates/meerkat-core/src/lib.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-core"]);
  assertLanes(plan, "core touch");
  assert.ok(plan.closure.length >= 40, `core closure spans the workspace (${plan.closure.length})`);
  assert.ok(plan.closure.includes("meerkat-mob"), "core closure includes meerkat-mob");
  assert.ok(plan.closure.includes("rkat"), "core closure includes rkat");
  assert.equal(plan.shards.length, 1);
  assert.deepEqual(plan.unit_shards.map((shard) => shard.packages), [["meerkat-core"]], "core runs its unit tests in the pull request");
  assert.deepEqual(plan.unit_deferred, []);
  assert.equal(plan.wasm, true, "core is in the wasm runtime closure");
  assert.equal(plan.machine_authority, false);
}

// The meerkat-mob compile chain: a mob touch gets its clippy lane in the pull
// request and no pull-request unit lane; its unit tests are named as
// deferred to the push-to-main run. The chain is computed from metadata.
{
  const plan = planFor(["crates/meerkat-mob/src/lib.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-mob"]);
  assert.equal(plan.shards.length, 1, "clippy lane for mob");
  assert.deepEqual(plan.unit_shards, [], "no pull-request unit lane may compile meerkat-mob");
  assert.deepEqual(plan.unit_deferred, ["meerkat-mob"]);
  assert.ok(plan.package_model["meerkat-mob"].estimated_minutes > plan.pr_unit_budget_minutes, "mob's own lane models over budget");
  assertLanes(plan, "mob touch");
  for (const name of ["meerkat-mob", "meerkat-rpc", "meerkat-rest", "rkat", "meerkat-mob-mcp", "meerkat-integration-tests", "meerkat-web-runtime", "meerkat-machine-codegen"]) {
    assert.ok(plan.unit_deferred_chain.includes(name), `${name} compiles meerkat-mob in its unit lane and is in the deferred chain`);
  }
  // xtask reaches meerkat-mob only through meerkat-machine-codegen's
  // dev-dependency, which Cargo never builds for xtask: not in the chain.
  for (const name of ["meerkat-core", "meerkat-runtime", "meerkat-session", "meerkat", "xtask", "machine-dsl-tests"]) {
    assert.ok(!plan.unit_deferred_chain.includes(name), `${name} does not compile meerkat-mob in its unit lane`);
  }
}
{
  const plan = planFor(["crates/meerkat-cli/src/main.rs", "crates/meerkat-session/src/lib.rs"]);
  assert.deepEqual(plan.unit_deferred, ["rkat"]);
  assert.deepEqual(plan.unit_shards.map((shard) => shard.packages), [["meerkat-session"]]);
  assertLanes(plan, "cli + session");
}

// Runtime's ordinary unit tests remain in the pull request even when its
// authorization dependency has native facade fixtures in its own dev graph.
{
  const plan = planFor(["crates/meerkat-runtime/src/lib.rs"]);
  assert.equal(plan.pr_unit_budget_minutes, 16, "runtime uses the unchanged pull-request unit budget");
  assert.deepEqual(plan.unit_shards.map((shard) => shard.packages), [["meerkat-runtime"]], "runtime retains its pull-request unit lane");
  assert.deepEqual(plan.unit_deferred, [], "runtime unit tests are not deferred");
  assert.ok(!plan.unit_deferred_chain.includes("meerkat-runtime"), "runtime remains outside the mob build chain");
  assert.ok(plan.unit_shards[0].estimated_minutes <= plan.pr_unit_budget_minutes, "runtime's unit lane fits the budget");
}

// Budget by construction: every package outside the chain models under the
// pull-request unit budget on its own, so no changed-package plan can
// exceed it; every package inside the chain is deferred.
{
  const plan = planFor(["Cargo.lock"]);
  for (const [name, model] of Object.entries(plan.package_model)) {
    if (!model.heavy_chain) {
      assert.ok(model.estimated_minutes <= plan.pr_unit_budget_minutes, `${name} models ${model.estimated_minutes} min alone, over budget`);
    }
  }
  assert.equal(plan.unit_deferred.length, plan.unit_deferred_chain.length, "workspace mode defers exactly the chain");
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
  const plan = planFor(["crates/meerkat-core/README.md"]);
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

// Poster-only: the poster gate runs, but the change is not machine
// authority, so it cannot start the machine-authority lanes (TLC).
{
  const plan = planFor(["docs/internal/machine-posters/mob_machine.html", "scripts/machine-posters/generate-machine-posters.mjs"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.machine_authority, false);
  assert.equal(plan.machine_posters, true);
}

// A generated machine kernel is both machine authority and a Rust source
// owned by meerkat-machine-kernels.
{
  const plan = planFor(["crates/meerkat-machine-kernels/src/generated/meerkat.rs"]);
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
  const plan = planFor(["crates/meerkat-openai/Cargo.toml", "crates/meerkat/src/help.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat", "meerkat-openai"]);
  assertLanes(plan, "manifest + facade");
  assert.ok(plan.closure.includes("meerkat-mob"));
  assert.equal(plan.shards.length, 2, "one shard per changed package below the shard cap");
}

// Many changed packages are packed into the shard cap, never dropped.
{
  const paths = [
    "crates/meerkat-core/src/lib.rs",
    "crates/meerkat-runtime/src/lib.rs",
    "crates/meerkat-mob/src/lib.rs",
    "crates/meerkat-rpc/src/lib.rs",
    "crates/meerkat-rest/src/lib.rs",
    "crates/meerkat-cli/src/main.rs",
    "crates/meerkat-session/src/lib.rs",
    "crates/meerkat-store/src/lib.rs",
    "crates/xtask/src/main.rs",
  ];
  const plan = planFor(paths, ["--max-shards", "4"]);
  assert.equal(plan.mode, "packages");
  assert.equal(plan.packages.length, 9);
  assert.equal(plan.shards.length, 4);
  assertLanes(plan, "nine packages in four shards");
}

// A pull request that changes crates/xtask runs xtask's unit lane and its
// machine-authority feature suite (main went red through #1362 because both
// were deferred).
{
  const plan = planFor(["crates/xtask/src/machines.rs"]);
  assert.deepEqual(plan.unit_deferred, [], "xtask is not deferred to main");
  assert.deepEqual(plan.unit_shards.map((shard) => shard.packages), [["xtask"]]);
  assert.ok(
    plan.unit_feature_shards.some((suite) => suite.packages[0] === "xtask" && suite.features.includes("machine-authority")),
    "the xtask machine-authority suite runs in the pull request",
  );
  for (const shard of [...plan.unit_shards, ...plan.unit_feature_shards]) {
    assert.ok(shard.estimated_minutes <= plan.pr_unit_budget_minutes, `${shard.name} fits the pull-request budget`);
  }
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
  const result = run(["--format", "github", "--", "crates/meerkat-core/src/lib.rs"]);
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
    "machine_posters",
    "wasm",
    "sdk_host",
    "bazel_graph",
    "examples_browser",
    "example_web",
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
  for (const key of ["unit_shard_count", "unit_shard_matrix", "unit_deferred", "unit_deferred_count", "main_unit_shard_count", "main_unit_shard_matrix"]) {
    assert.ok(key in lines, `github output has ${key}`);
  }
  assert.equal(lines.unit_shard_count, "1");
  assert.equal(lines.unit_deferred, "");
  assert.ok(Number(lines.main_unit_shard_count) >= 2);
}
{
  const result = run(["--format", "github", "--", "crates/meerkat-mob/src/lib.rs"]);
  const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.unit_shard_count, "0");
  assert.equal(lines.unit_deferred, "meerkat-mob");
  assert.deepEqual(JSON.parse(lines.unit_shard_matrix).include, [{ name: "none", packages: "" }]);
  assert.equal(lines.shard_count, "1");
}
{
  const result = run(["--format", "github", "--", "docs/index.mdx"]);
  const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.deepEqual(JSON.parse(lines.shard_matrix).include, [{ name: "none", packages: "" }]);
  assert.equal(lines.rust_changed, "false");
  // A docs-only push to main has no unit lanes: the main-unit matrix is
  // empty (no "none" shard for the job to trip over) and its count is 0.
  assert.deepEqual(JSON.parse(lines.main_unit_shard_matrix), { include: [] });
  assert.equal(lines.main_unit_shard_count, "0");
  assert.equal(lines.unit_shard_count, "0");
  assert.equal(lines.unit_deferred, "");
}
{
  const plan = planFor(["CHANGELOG.md"]);
  assert.equal(plan.rust_changed, false);
  assert.deepEqual(plan.main_unit_shards, [], "a CHANGELOG-only merge yields no main unit lanes");
  assert.deepEqual(plan.main_archive_builds, [], "a CHANGELOG-only merge yields no archived lanes");
  assert.deepEqual(plan.main_archive_runs, []);
}

// Feature-gated unit suites: the default-feature unit lanes cannot run a
// test behind a non-default feature, so every Rust-relevant push to main
// carries every suite as an extra main unit row, and a pull request carries
// the suites of a changed package that runs its unit tests there.
{
  const featureFlags = /^-p (\S+) --features [a-z0-9-]+(,[a-z0-9-]+)*$/;
  const facade = planFor(["crates/meerkat/src/experimental_gpt_live.rs"]);
  assert.ok(facade.main_feature_unit_shards.length >= 2, "main carries the feature suites");
  for (const suite of facade.main_feature_unit_shards) {
    const match = featureFlags.exec(suite.package_flags);
    assert.ok(match, `feature suite ${suite.name} flags: ${suite.package_flags}`);
    assert.deepEqual(suite.packages, [match[1]], `${suite.name} runs exactly its package`);
    assert.ok(suite.packages[0] in facade.package_model, `${suite.name} names a workspace package`);
  }
  const names = facade.main_feature_unit_shards.map((suite) => suite.name);
  assert.equal(new Set(names).size, names.length, "feature suite names are unique (they name matrix rows and artifacts)");
  assert.ok(
    facade.main_feature_unit_shards.some((suite) => suite.packages[0] === "meerkat" && suite.features.includes("openai-live")),
    "the facade's public GPT Live suite is on main",
  );
  // The facade runs its unit tests in the pull request, so its suites do too.
  assert.deepEqual(
    facade.unit_feature_shards.map((suite) => suite.packages[0]).filter((name) => name !== "meerkat"),
    [],
    "only the changed package's suites join the pull request",
  );
  assert.ok(facade.unit_feature_shards.some((suite) => suite.features.includes("openai-live")), "the facade touch runs its GPT Live suite in the pull request");
  for (const suite of facade.unit_feature_shards) {
    assert.ok(suite.estimated_minutes <= facade.pr_unit_budget_minutes, `${suite.name} fits the pull-request budget`);
  }
  const github = run(["--format", "github", "--", "crates/meerkat/src/experimental_gpt_live.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  const unitRows = JSON.parse(lines.unit_shard_matrix).include;
  const mainRows = JSON.parse(lines.main_unit_shard_matrix).include;
  assert.equal(Number(lines.unit_shard_count), unitRows.length);
  assert.equal(Number(lines.main_unit_shard_count), mainRows.length);
  assert.equal(unitRows.length, facade.unit_shards.length + facade.unit_feature_shards.length);
  assert.equal(mainRows.length, facade.main_unit_shards.length + facade.main_feature_unit_shards.length);
  for (const suite of facade.main_feature_unit_shards) {
    assert.ok(mainRows.some((row) => row.name === suite.name && row.packages === suite.package_flags), `main matrix row for ${suite.name}`);
  }

  // A heavy-chain package defers its suites with its unit tests.
  const mob = planFor(["crates/meerkat-mob/src/lib.rs"]);
  assert.deepEqual(mob.unit_feature_shards, [], "no pull-request feature suite may compile meerkat-mob");
  // meerkat-mob's default lane and its feature suite run as archived lanes:
  // built once, executed in partitions.
  assert.ok(
    !mob.main_feature_unit_shards.some((suite) => suite.packages[0] === "meerkat-mob"),
    "mob's suite leaves the packed feature rows",
  );
  assert.deepEqual(
    mob.main_archive_builds.map((build) => build.package_flags),
    ["-p meerkat-mob", "-p meerkat-mob --features experimental-gpt-live,schema"],
    "mob's default lane and feature suite build archives on main",
  );
  const mobGithub = run(["--format", "github", "--", "crates/meerkat-mob/src/lib.rs"]);
  assert.equal(mobGithub.status, 0, mobGithub.stderr);
  const mobLines = Object.fromEntries(mobGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(Number(mobLines.main_archive_build_count), JSON.parse(mobLines.main_archive_build_matrix).include.length);
  assert.equal(Number(mobLines.main_archive_run_count), JSON.parse(mobLines.main_archive_run_matrix).include.length);
  // An unrelated leaf change carries no pull-request suite; docs carry none anywhere.
  assert.deepEqual(planFor(["crates/meerkat-sqlite/src/lib.rs"]).unit_feature_shards, []);
  const docs = planFor(["docs/index.mdx"]);
  assert.deepEqual(docs.unit_feature_shards, []);
  assert.deepEqual(docs.main_feature_unit_shards, []);
}

// Integration suites: a directly changed trigger package runs the suite's
// tests/*.rs binaries; a leaf change and docs run none; workspace mode runs
// every suite; the github output carries the matrix rows.
{
  const names = (plan) => plan.integration_suites.map((suite) => suite.packages[0]);
  assert.deepEqual(names(planFor(["crates/meerkat-runtime/src/lib.rs"])), ["meerkat-runtime", "meerkat-machine-codegen", "meerkat-authorization"]);
  assert.deepEqual(
    names(planFor(["crates/meerkat-machine-schema/src/lib.rs"])),
    ["meerkat-runtime", "meerkat-machine-codegen", "meerkat-authorization"],
    "a machine schema or DSL change runs all affected integration suites",
  );
  assert.deepEqual(names(planFor(["crates/meerkat-mob/src/lib.rs"])), ["meerkat-machine-codegen"], "mob runs the codegen parity suite");
  assert.deepEqual(names(planFor(["crates/meerkat-machine-codegen/tests/runtime_alphabet_parity.rs"])), ["meerkat-machine-codegen"]);
  assert.deepEqual(names(planFor(["crates/meerkat-sqlite/src/lib.rs"])), []);
  assert.deepEqual(names(planFor(["docs/index.mdx"])), []);
  assert.deepEqual(names(planFor(["Cargo.toml"])), ["meerkat-runtime", "meerkat-machine-codegen", "xtask", "meerkat-authorization"], "workspace mode runs every suite");
  // Authorization's native model/tool loops and ordinary cost controls are
  // integration binaries, so its default-feature unit row cannot run them.
  for (const path of [
    "crates/meerkat-authorization/src/grant_policy.rs",
    "crates/meerkat-authorization/tests/native_governed_loop.rs",
    "crates/meerkat-authorization/tests/native_cost.rs",
    "crates/meerkat-authorization-contracts/src/grant.rs",
    "crates/meerkat-core/src/agent/state.rs",
    "crates/meerkat-runtime/src/store/execution_custody.rs",
    "crates/meerkat/src/session_factory.rs",
    "crates/meerkat-tools/src/lib.rs",
    "crates/meerkat-llm-core/src/lib.rs",
    "crates/meerkat-anthropic/src/lib.rs",
    "crates/meerkat-auth-core/src/lib.rs",
    "crates/meerkat-models/src/lib.rs",
    "crates/meerkat-machine-schema/src/lib.rs",
    "crates/meerkat-machine-dsl/src/lib.rs",
    "crates/meerkat-machine-dsl-core/src/lib.rs",
    "crates/meerkat-machine-derive/src/lib.rs",
    "crates/meerkat-machine-kernels/src/lib.rs",
  ]) {
    const authorization = planFor([path]);
    const selected = authorization.integration_suites.filter((suite) => suite.packages[0] === "meerkat-authorization");
    assert.equal(selected.length, 1, `${path} selects exactly one native authorization suite`);
    assert.equal(selected[0].package_flags, "-p meerkat-authorization", "ordinary native fixtures keep default features and ignored acceptance opt-in");
  }
  const authorizationGithub = run(["--format", "github", "--", "crates/meerkat-authorization/tests/native_governed_loop.rs"]);
  assert.equal(authorizationGithub.status, 0, authorizationGithub.stderr);
  const authorizationLines = Object.fromEntries(authorizationGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(authorizationLines.integration_count, "1");
  assert.deepEqual(JSON.parse(authorizationLines.integration_matrix).include.map((row) => row.packages), ["-p meerkat-authorization"]);
  // xtask's integration tests pin the workflows: an xtask change or a
  // workflow-only edit (a plan with no Rust change) runs them, with the
  // feature its machines_contracts target requires.
  const xtask = planFor(["crates/xtask/src/machines.rs"]);
  assert.deepEqual(names(xtask), ["xtask"]);
  assert.equal(xtask.integration_suites[0].package_flags, "-p xtask --features machine-authority");
  const workflow = planFor([".github/workflows/nightly.yml"]);
  assert.equal(workflow.rust_changed, false);
  assert.deepEqual(names(workflow), ["xtask"], "a workflow-only edit runs the xtask workflow pins");
  const github = run(["--format", "github", "--", "crates/meerkat-runtime/src/lib.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  const rows = JSON.parse(lines.integration_matrix).include;
  assert.equal(Number(lines.integration_count), 3);
  assert.deepEqual(rows.map((row) => row.packages), ["-p meerkat-runtime", "-p meerkat-machine-codegen", "-p meerkat-authorization"]);
  const docsGithub = run(["--format", "github", "--", "docs/index.mdx"]);
  const docsLines = Object.fromEntries(docsGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(docsLines.integration_count, "0");
  assert.deepEqual(JSON.parse(docsLines.integration_matrix).include, [{ name: "none", packages: "" }]);
}

// Bazel graph check selection: Bazel-relevant paths, any Cargo manifest, and
// moved or deleted Rust files select it; documentation does not.
{
  const plan = planFor(["crates/meerkat-core/BUILD.bazel"]);
  assert.equal(plan.bazel_graph, true, "a BUILD-only diff selects the Bazel graph check");
  assert.equal(plan.examples_browser, false);
}
{
  for (const path of ["MODULE.bazel", "MODULE.bazel.lock", ".bazelrc", ".bazelversion", "tools/bazel/defs.bzl", "tools/buildbuddy/README.md", "scripts/generate-bazel-rust-builds.mjs", "Cargo.lock"]) {
    assert.equal(planFor([path]).bazel_graph, true, `${path} selects the Bazel graph check`);
  }
}
{
  const plan = planFor(["docs/index.mdx", "README.md"]);
  assert.equal(plan.bazel_graph, false, "a docs-only diff does not select the Bazel graph check");
  assert.equal(plan.examples_browser, false, "a docs-only diff does not select the example suites");
  assert.equal(plan.example_web, false, "a docs-only diff does not select the example web suites");
}
{
  const plan = planFor(["crates/meerkat-core/Cargo.toml"]);
  assert.equal(plan.bazel_graph, true, "a Cargo.toml change selects the Bazel graph check");
}
{
  const plan = planFor(["crates/meerkat-core/src/lib.rs"]);
  assert.equal(plan.bazel_graph, false, "an in-place Rust edit leaves the graph unchanged");
}
{
  // A rename inside crates/: the old path no longer exists, the new one does.
  const plan = planFor(["crates/meerkat-core/src/renamed_away_for_selftest.rs", "crates/meerkat-core/src/lib.rs"]);
  assert.equal(plan.bazel_graph, true, "a crates/ rename selects the Bazel graph check");
  assert.deepEqual(plan.removed_paths, ["crates/meerkat-core/src/renamed_away_for_selftest.rs"]);
}
{
  const result = run(["--format", "github", "--", "crates/meerkat-core/BUILD.bazel"]);
  const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.bazel_graph, "true");
  assert.equal(lines.examples_browser, "false");
}

// Example suites selection: the TypeScript and Python SDKs and example
// sources select them; example documentation does not.
{
  for (const path of ["sdks/typescript/src/index.ts", "sdks/python/meerkat/__init__.py", "examples/037-live-webrtc-web/app.js", "examples/tests/sdk-examples.test.mjs", "examples/package-lock.json"]) {
    assert.equal(planFor([path]).examples_browser, true, `${path} selects the example suites`);
  }
  assert.equal(planFor(["examples/037-live-webrtc-web/README.md"]).examples_browser, false);
  assert.equal(planFor(["crates/meerkat-core/src/lib.rs"]).examples_browser, false);
}

// Example web suites selection: the sdks/web runtime, the crates it builds
// from, and the sources of examples 031, 032 and 033 select them; their
// documentation, other examples and unrelated crates do not.
{
  // Dependency-only Rust changes must run both the browser examples and
  // WASM timer tests, whose workflow jobs share the example_web flag.
  for (const path of [
    "crates/meerkat-core/src/time_compat/wasm.rs",
    "crates/meerkat-core/src/lib.rs",
    "crates/meerkat-runtime/src/lib.rs",
    "crates/meerkat-session/src/lib.rs",
    "crates/meerkat-mob/src/lib.rs",
    "crates/meerkat-machine-kernels/src/lib.rs",
  ]) {
    const plan = planFor([path]);
    assert.equal(plan.mode, "packages", `${path} selects its owning package`);
    assert.equal(plan.packages.length, 1, `${path} does not escalate to the workspace`);
    assert.ok(plan.closure.includes("meerkat-web-runtime"), `${path} reaches the web runtime through dependencies`);
    assert.equal(plan.wasm, true, `${path} selects the WASM build`);
    assert.equal(plan.example_web, true, `${path} selects the browser and WASM timer suites`);
  }
  for (const path of [
    "sdks/web/scripts/build-wasm.mjs",
    "sdks/web/src/index.ts",
    "crates/meerkat-web-runtime/src/lib.rs",
    "crates/meerkat-contracts/src/lib.rs",
    "examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs",
    "examples/032-wasm-webcm-agent/web/package.json",
    "examples/033-the-office-demo-sh/web/tests/regression.cjs",
  ]) {
    assert.equal(planFor([path]).example_web, true, `${path} selects the example web suites`);
  }
  for (const path of [
    "examples/033-the-office-demo-sh/README.md",
    "examples/037-live-webrtc-web/app.js",
    "sdks/typescript/src/index.ts",
    "crates/meerkat-core/README.md",
    "CHANGELOG.md",
  ]) {
    assert.equal(planFor([path]).example_web, false, `${path} does not select the example web suites`);
  }
  for (const path of ["crates/meerkat-rpc/src/lib.rs", "crates/meerkat-cli/src/main.rs"]) {
    const plan = planFor([path]);
    assert.equal(plan.mode, "packages", `${path} still selects its Rust package`);
    assert.ok(!plan.closure.includes("meerkat-web-runtime"), `${path} has no reverse dependency on the web runtime`);
    assert.equal(plan.example_web, false, `${path} does not select the browser and WASM timer suites`);
  }
  for (const path of ["Cargo.lock", ".cargo/config.toml"]) {
    const plan = planFor([path]);
    assert.equal(plan.mode, "workspace", `${path} selects the workspace`);
    assert.equal(plan.example_web, true, `${path} selects the browser and WASM timer suites`);
  }
  for (const path of ["sdks/web/src/index.ts", "crates/meerkat-core/src/time_compat/wasm.rs"]) {
    const result = run(["--format", "github", "--", path]);
    assert.equal(result.status, 0, result.stderr);
    const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
    assert.equal(lines.example_web, "true", `${path} selects the suites in GitHub output`);
  }
}

// A change to the lane definitions runs the lanes they define.
{
  for (const path of [".github/workflows/ci.yml", "scripts/ci-cargo-lanes.mjs", "scripts/ci-cargo-lanes-selftest.mjs"]) {
    const plan = planFor([path]);
    assert.equal(plan.bazel_graph, true, `${path} selects the Bazel graph check`);
    assert.equal(plan.examples_browser, true, `${path} selects the example suites`);
    assert.equal(plan.example_web, true, `${path} selects the example web suites`);
  }
}

// Bad arguments fail loudly.
{
  const result = run(["--format", "yaml", "--", "crates/meerkat-core/src/lib.rs"]);
  assert.notEqual(result.status, 0, "invalid format is an error");
}

console.log("ci-cargo-lanes fail-closed contracts hold");
