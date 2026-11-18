#!/usr/bin/env node
// Focused dry contracts: no Cargo metadata/compiler, provider, Docker or SDK
// execution. The real planner integration blocks and gate shell are exercised
// with injected graph/plan data. Make calls the unmodified repo-cargo in a
// temporary Git repository, with a recording fake Cargo and fake Python.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import {
  EXAMPLE_035, EXAMPLE_035_COMMANDS, EXAMPLE_035_DEPENDENCIES,
  EXAMPLE_035_MANIFEST, EXAMPLE_035_MANIFEST_SHA256, example035Selection, ownsExample035,
  readExample035Inputs, selectExample035Dependencies,
} from "./ci-standalone-example.mjs";

import { loadNonRustReaders, readerSelectionsFor } from "./ci-non-rust-readers.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const text = (path) => readFileSync(resolve(root, path), "utf8");
const manifestText = text(`${EXAMPLE_035}/Cargo.toml`);
const inputs = { manifestText, unsupported: [] };
let checks = 0;
function check(label, fn) { fn(); checks += 1; console.log(`ok ${checks} - ${label}`); }
function pkg(name, dependencies = []) { return { id: name, name, dependencies }; }
const edge = (name, kind = null) => ({ name, kind, source: null });
const packages = [
  ...EXAMPLE_035_DEPENDENCIES.map((name) => pkg(name, name === "meerkat" ? [edge("normal-input"), edge("dev-input", "dev"), edge("build-input", "build")] : [])),
  pkg("normal-input", [edge("optional-input")]), pkg("dev-input"), pkg("build-input"), pkg("optional-input"), pkg("unrelated"), pkg("xtask"),
];
const select = (changed, extra = {}) => example035Selection({ event: "pull_request", packages, changed, inputs, ...extra });

check("complete manifest pin and all path dependency kinds are bound", () => {
  assert.equal(createHash("sha256").update(manifestText).digest("hex"), EXAMPLE_035_MANIFEST_SHA256);
  // Python's standard TOML parser independently inventories path declarations,
  // including target-specific sections, rather than mirroring the JS list.
  const parsed = JSON.parse(execFileSync("python3", ["-I", "-c", `import json,sys,tomllib
x=tomllib.load(open(sys.argv[1],'rb'))
n=[]
def walk(v):
 if isinstance(v,dict):
  for k,a in v.items():
   if isinstance(a,dict) and 'path' in a and k not in ('package',): n.append(k)
   walk(a)
 elif isinstance(v,list):
  for a in v: walk(a)
walk(x)
print(json.dumps(sorted(n)))`, resolve(root, EXAMPLE_035, "Cargo.toml")], { encoding: "utf8" }));
  assert.deepEqual(parsed, EXAMPLE_035_DEPENDENCIES);
});
const exactPaths = ["README.md", "scripts/test-scripts.py", "scripts/tmux-architecture-suite.sh", "scripts/tmux-live-suite.sh", "scripts/tux-prompt-input.sh", "src/bin/kennel.rs", "tests/suite_dispatch.rs"].map((p) => `${EXAMPLE_035}/${p}`);
check("exact PR2413 seven paths select035 without artificial workspace seeds", () => {
  const s = select(exactPaths);
  assert.equal(s.selected, true);
  assert.equal(s.fallback, "");
  assert.equal(s.ownedPaths.length, 6);
  for (const name of [...EXAMPLE_035_DEPENDENCIES, "normal-input", "dev-input", "build-input", "optional-input"]) assert(s.dependencyClosure.includes(name), name);
  assert(!("seeds" in s));
});
check("each Rust, script, lock and Make input selects direct coverage", () => {
  for (const p of ["src/bin/kennel.rs", "tests/suite_dispatch.rs", "scripts/test-scripts.py", "scripts/x.sh", "Cargo.lock", "Makefile"]) assert(select([`${EXAMPLE_035}/${p}`]).selected, p);
});
check("Markdown alone does not invent a native row or seeds", () => {
  const s = select([`${EXAMPLE_035}/README.md`]);
  assert(!s.selected); assert(!("seeds" in s));
});
check("unknown example is not registered; unknown/global direct coverage is push-only", () => {
  assert(!select(["examples/999-unknown/src/main.rs"]).selected);
  assert(!ownsExample035("examples/035-mdm-tux-rs-other/main.rs"));
  assert(!select(null).selected);
  assert(!select([], { globalChange: true }).selected);
  assert(select(null, { event: "push" }).selected);
  assert(select([], { event: "push", globalChange: true }).selected);
  for (const event of [undefined, "", "other", "workflow_dispatch"]) assert.throws(() => select([], { event }), /normalized planner event/);
});
check("manifest feature/dev/target changes preserve workspace plus direct coverage", () => {
  for (const suffix of ['\n[features]\nnew=[]\n', '\n[build-dependencies]\nextra={path="../../crates/extra"}\n', '\n# reviewed registration must change too\n']) {
    const s = select([], { inputs: { ...inputs, manifestText: manifestText + suffix } });
    assert(s.selected); assert(s.fallback); assert.equal(s.ownedPaths.length, 0);
  }
});
check("missing registered dependency and symlink/special input retain workspace", () => {
  assert(select(exactPaths, { packages: packages.filter((p) => p.name !== "meerkat-client") }).fallback);
  for (const path of [`${EXAMPLE_035}/src/link.rs`, `${EXAMPLE_035}/src`]) {
    const s = select([], { inputs: { ...inputs, unsupported: [path] } });
    assert(s.selected); assert(s.fallback); assert.equal(s.ownedPaths.length, 0);
  }
});
check("dependency changes select035 on push, defer on PR, unrelated changes do not", () => {
  for (const name of ["meerkat", "normal-input", "dev-input", "build-input", "optional-input"]) {
    const pr = selectExample035Dependencies(select([]), [name]);
    assert(!pr.selected); assert(pr.deferredReasons.some((r) => r.includes(name)));
    assert(selectExample035Dependencies(select([], { event: "push" }), [name]).selected, name);
  }
  assert(!selectExample035Dependencies(select([], { event: "push" }), ["unrelated"]).selected);
});

// Execute the actual changed-file loop, not a second implementation. The
// existing generic path/product functions are injected; their own broad tests
// remain the normal Core CI responsibility. This catches interceptor placement
// relative to globals, workspace linked readers and the unmapped fallback.
const planner = text("scripts/ci-cargo-lanes.mjs");
function between(source, first, last) {
  const start = source.indexOf(first); assert(start >= 0, first);
  const end = source.indexOf(last, start + first.length); assert(end >= 0, last);
  return source.slice(start, end);
}
const changedLoop = between(planner, "    for (const path of changed) {", "    if (result.unmapped_rust_paths.length > 0)");
function changedPlan(paths, standalone, linked = false) {
  const result = { unmapped_rust_paths: [], lock_scope: null, packages: [], closure: [], rust_changed: false };
  const seeds = new Set();
  const context = {
    changed: paths, standalone, result, seeds, workspaceReason: standalone.fallback,
    isGlobalPath: (p) => ["Cargo.toml", ".github/workflows/ci.yml"].includes(p),
    productOfPath: () => "core", CORE: "core", products: [],
    isProductBuildConfigPath: () => false, productPackages: () => [], productEscalations: {},
    packagesForFile: (p) => p.startsWith("crates/unrelated/") ? [pkg("unrelated")] : linked && p.endsWith("kennel.rs") ? [pkg("xtask")] : [],
    packageDir: (p) => `crates/${p.name}`, dirs: [], compiledInDocs: new Set(),
    isRustSourcePath: (p) => p.endsWith(".rs") || p.endsWith("/Cargo.toml") || p === "Cargo.toml",
  };
  vm.runInNewContext(changedLoop, context);
  const escalation = between(planner, "    if (result.unmapped_rust_paths.length > 0)", "  }\n\n  if (releaseProjection) {\n    // What a version projection");
  vm.runInNewContext(escalation, context);
  const mode = between(planner, "  if (releaseProjection) {\n    result.mode = RELEASE_PROJECTION_MODE;", "  // Core's own lanes");
  Object.assign(context, {
    releaseProjection: false, deferToPush: standalone.event === "pull_request",
    EXAMPLE_035_COMMANDS, EXAMPLE_035_MANIFEST,
    allNames: packages.map((p) => p.name),
    byName: new Map(packages.map((p) => [p.name, p])),
    byId: new Map(packages.map((p) => [p.id, p])),
    reverse: new Map(packages.map((p) => [p.id, new Set(packages.filter((q) => q.dependencies.some((d) => d.name === p.name)).map((q) => q.id))])),
    selectExample035Dependencies,
  });
  vm.runInNewContext(mode, context);
  return context;
}
check("actual planner loop narrows registered Rust and keeps workspace linked reader", () => {
  const c = changedPlan(exactPaths, select(exactPaths), true);
  assert.equal(c.result.unmapped_rust_paths.length, 0);
  assert.equal(c.result.mode, "packages"); assert.equal(c.result.example_035, true);
  assert(c.seeds.has("xtask")); assert(!c.seeds.has("meerkat"));
  assert.deepEqual(Array.from(c.result.packages), ["xtask"]);
});
check("actual planner loop preserves unknown and mixed/global inputs", () => {
  const unknown = "examples/999-unknown/src/main.rs";
  let paths = [...exactPaths, unknown, "crates/unrelated/src/lib.rs"];
  let c = changedPlan(paths, select(paths));
  assert.deepEqual(Array.from(c.result.unmapped_rust_paths), [unknown]); assert(c.seeds.has("unrelated"));
  assert.equal(c.result.mode, "workspace"); assert.equal(c.result.packages.length, packages.length);
  paths = [...exactPaths, "Cargo.toml"];
  c = changedPlan(paths, select(paths, { globalChange: true }));
  assert.match(c.workspaceReason, /global build configuration/); assert.equal(c.result.mode, "workspace");
  c = changedPlan(exactPaths, select(exactPaths, { inputs: { ...inputs, manifestText: manifestText + "\n" } }));
  assert.equal(c.result.unmapped_rust_paths.length, 2);
});
check("actual PR leaf mode has no workspace seeds; push preserves unowned Rust fallback", () => {
  const own = [`${EXAMPLE_035}/src/bin/kennel.rs`];
  const pr = changedPlan(own, select(own));
  assert.equal(pr.result.mode, "none"); assert.equal(pr.result.example_035, true);
  assert.equal(pr.result.packages.length, 0); assert.match(pr.result.reason, /workspace-owned.*standalone035/);
  const push = changedPlan(own, select(own, { event: "push" }));
  assert.equal(push.result.mode, "workspace"); assert.equal(push.result.example_035, true);
  for (const event of ["pull_request", "push"]) {
    const paths = [`${EXAMPLE_035}/scripts/test-scripts.py`, `${EXAMPLE_035}/Makefile`, `${EXAMPLE_035}/Cargo.lock`];
    const c = changedPlan(paths, select(paths, { event }));
    assert.equal(c.result.mode, "none"); assert.equal(c.result.packages.length, 0); assert(c.result.example_035);
  }
  const unknown = ["examples/999-unknown/src/main.rs"];
  assert.equal(changedPlan(unknown, select(unknown)).result.example_035, false);
  assert.equal(changedPlan(unknown, select(unknown, { event: "push" })).result.example_035, true);
});
check("actual final flag and escaped GitHub metadata describe selected/deferred commands", () => {
  const block = between(planner, "  if (!releaseProjection) {\n    selectExample035Dependencies", "  // Core's own lanes");
  for (const event of ["pull_request", "push"]) {
    const context = { releaseProjection: false, standalone: select([], { event }), result: { mode: "packages", packages: ["normal-input"] }, selectExample035Dependencies, EXAMPLE_035_COMMANDS, EXAMPLE_035_MANIFEST };
    vm.runInNewContext(block, context); assert.equal(context.result.example_035, event === "push");
    const lines = planner.split("\n").filter((l) => /scalar\("example_035(?:_|"|,)/.test(l));
    assert.equal(lines.length, 4);
    const output = {}; vm.runInNewContext(lines.join("\n"), { result: context.result, scalar: (k, v) => output[k] = v });
    assert.equal(output.example_035, String(event === "push"));
    assert.equal(output.example_035_manifest, `${EXAMPLE_035}/Cargo.toml`);
    assert.deepEqual(JSON.parse(output.example_035_commands), event === "push" ? EXAMPLE_035_COMMANDS : []);
    assert(JSON.parse(output.example_035_reasons).some((r) => r.includes(event === "push" ? "dependency" : "deferred to push")));
    const reused = { ...context, releaseProjection: true, standalone: select(null, { event }), result: { mode: "release-projection", packages: [] } };
    vm.runInNewContext(block, reused); assert.equal(reused.result.example_035, false);
  }
  const malicious = select([`${EXAMPLE_035}/scripts/$(touch SHOULD_NOT_EXIST)\nscript.sh`]);
  const encoded = JSON.stringify(malicious.reasons);
  assert(!encoded.includes("\n")); assert(encoded.includes("\\n"));
});
const workflow = text(".github/workflows/ci.yml");
check("workflow output, immediate scheduling, Make commands and aggregate need agree", () => {
  for (const key of ["example_035", "example_035_reasons", "example_035_manifest", "example_035_commands"]) {
    assert(workflow.includes(key + ": ${{ steps.plan.outputs." + key + " }}"));
  }
  const job = between(workflow, "  example-035:\n", "  example-web:\n");
  assert(job.includes("needs: [changes]"));
  assert(job.includes("if: ${{ needs.changes.outputs.example_035 == 'true' }}"));
  for (const cmd of EXAMPLE_035_COMMANDS) assert(job.includes(`run: ${cmd}`));
  assert(workflow.slice(workflow.indexOf("  gate:\n")).includes("      - example-035\n"));
  assert(workflow.includes("EXAMPLE_035_RESULT: ${{ needs.example-035.result }}"));
  assert(!job.includes("continue-on-error"));
});
check("actual gate rejects failed, cancelled and skipped selected035", () => {
  const block = between(workflow, '          case "${EXAMPLE_035}" in', '          if [[ "${MACHINE_AUTHORITY}" == "true" ]]; then');
  const prelude = 'failed=0; plan_errors=0; flagged_lanes=();\n' + between(workflow, "          require_ran() {", "          # The classifier is the fail-closed authority");
  for (const status of ["success", "failure", "cancelled", "skipped"]) {
    const r = spawnSync("bash", ["-euc", prelude + block + '\nexit "$failed"\n'], { env: { PATH: "/usr/bin:/bin", EXAMPLE_035: "true", EXAMPLE_035_RESULT: status }, encoding: "utf8" });
    assert.equal(r.status === 0, status === "success", status);
  }
  const r = spawnSync("bash", ["-euc", prelude + block + '\nexit "$failed"\n'], { env: { PATH: "/usr/bin:/bin", EXAMPLE_035: "false", EXAMPLE_035_RESULT: "skipped" } }); assert.equal(r.status, 0);
  for (const value of ["", "TRUE", "1", "false\ntrue", "unknown"]) {
    for (const result of ["success", "skipped"]) {
      const invalid = spawnSync("bash", ["-euc", prelude + block + '\nexit "$failed"\n'], { env: { PATH: "/usr/bin:/bin", EXAMPLE_035: value, EXAMPLE_035_RESULT: result }, encoding: "utf8" });
      assert.notEqual(invalid.status, 0, `${JSON.stringify(value)} / ${result}`);
    }
  }
});
check("actual affected-tests block prints both exact CI commands", () => {
  const source = text("scripts/affected-tests.mjs");
  const block = between(source, "  if (push.example_035) {", "  // SDK suites");
  for (const selected of [true, false]) {
    const lines = []; vm.runInNewContext(block, { push: { example_035: selected }, EXAMPLE_035_COMMANDS, add: (_section, line) => lines.push(line) });
    assert.deepEqual(lines, selected ? EXAMPLE_035_COMMANDS : []);
  }
});
check("shared readers, unknown fallback, browser selection and budget values remain", () => {
  const readers = JSON.parse(text("scripts/ci-non-rust-readers.json"));
  assert(JSON.stringify(readers).includes('"examples/":[{"package":"xtask","target":"lib"'));
  assert(planner.includes('result.unmapped_rust_paths.push(path)'));
  assert(planner.includes('path.startsWith("examples/") && !/\\.(md|mdx)$/.test(path)'));
  assert(workflow.includes('CI_MAX_EXEC_SECONDS: "1080"'));
  assert(workflow.includes('CI_MAX_TERMINAL_SECONDS: "1200"'));
  assert(text("scripts/ci-product-evidence.mjs").includes('"Example035 standalone Rust + Python": "core"'));
});

check("existing CLI default and dispatch normalization stay explicit", () => {
  const parse = between(planner, "function parseArgs(argv) {", "\nfunction git(argsList");
  const context = {}; vm.runInNewContext(parse, context);
  assert.equal(context.parseArgs([]).event, "push");
  assert.equal(context.parseArgs(["--event", "workflow_dispatch"]).event, "push");
  assert.equal(context.parseArgs(["--event", "pull_request"]).event, "pull_request");
  assert.throws(() => context.parseArgs(["--event", "other"]));
});
check("normal CI owns real-planner fixture receipts, distinct from actual event timing", () => {
  const broad = text("scripts/ci-cargo-lanes-selftest.mjs");
  assert(broad.includes('planFor(paths, ["--event", event])'));
  assert(broad.includes('kind: "explicit-path-classification-fixture-not-event-or-timing"'));
  for (const key of ["source_head", "source_tree", "explicit_paths", "requested_event", "reasons", "manifest", "commands"]) assert(broad.includes(key + ":"), key);
  assert(text("Makefile").includes("node scripts/ci-cargo-lanes-selftest.mjs"));
  assert(workflow.includes("run: make ci-lanes-selftest"));
});
check("hosted fixture consumes actual reader_rows test_flags, not stripped targets", () => {
  const broad = text("scripts/ci-cargo-lanes-selftest.mjs");
  const line = broad.split("\n").find((row) => row.includes("actual xtask lib reader runs"));
  assert(line);
  const returned = between(planner, "  const unique = [...new Map(rows.map", "\n}\n");
  assert(returned.includes(".map(({ targets, filtered, ...row }) => row)"));
  const run = (reader_rows) => vm.runInNewContext(line, { assert, plan: { reader_rows }, label: "dry returned row" });
  run([{ packages: ["xtask"], test_flags: "--lib --bins" }]);
  for (const rows of [[], [{ packages: ["other"], test_flags: "--lib" }], [{ packages: ["xtask"], test_flags: "--test lib" }], [{ packages: ["xtask"], targets: ["--lib"] }]]) assert.throws(() => run(rows));
});
check("projection fixture and actual gate require standalone035 false", () => {
  // Run only the existing fixture's literal environment and workflow shell,
  // never its metadata, worktree or release-evidence setup.
  const source = text("scripts/ci-release-projection-selftest.mjs");
  const context = { BUMP_PARENT: "a".repeat(40), RUN_ID: 4242 };
  vm.runInNewContext(between(source, "  const gateEnv = {", "  // The gate reads each two-wave lane") + "\nthis.env = gateEnv;", context);
  const block = between(workflow, '          if [[ "${RELEASE_PROJECTION}" == "true" ]]; then', '          case "${EXAMPLE_035}" in');
  const prelude = 'failed=0; plan_errors=0; flagged_lanes=();\n' + between(workflow, "          require_ran() {", "          # The classifier is the fail-closed authority");
  for (const value of ["false", "true", "", "FALSE"]) {
    const result = spawnSync("bash", ["-euc", prelude + block + '\nexit "$failed"\n'], { env: { PATH: "/usr/bin:/bin", ...context.env, EXAMPLE_035: value }, encoding: "utf8" });
    assert.equal(result.status, value === "false" ? 0 : 1, result.stdout + result.stderr);
  }
  assert.equal(context.env.EXAMPLE_035, "false");
  assert.equal(context.env.EXAMPLE_035_RESULT, "skipped");
});
check("F1 exact Rust workflow job list equals every current YAML job id", () => {
  const rust = text("crates/xtask/tests/ci_gate_requires_rmat.rs");
  const expected = between(rust, "        vec![\n", "        ],\n");
  const names = [...expected.matchAll(/"([a-z0-9-]+)"/g)].map((m) => m[1]);
  const actual = [...workflow.slice(workflow.indexOf("jobs:\n")).matchAll(/^  ([a-z0-9-]+):$/gm)].map((m) => m[1]).sort();
  assert.deepEqual(names, actual); assert(names.includes("example-035"));
});
check("F3 zero-seed PR selects the full xtask suite and does not defer its audits", () => {
  const definition = planner.split("\n").find((line) => line.includes('{ package: "xtask", features: ["machine-authority"], triggers:'));
  const suite = vm.runInNewContext("[" + definition + "]")[0];
  const xtask = { name: "xtask", features: { "machine-authority": [] }, targets: [
    { name: "xtask", kind: ["lib"] }, { name: "xtask", kind: ["bin"] },
    { name: "runtime_authority_bypass", kind: ["test"] }, { name: "effect_authority", kind: ["test"] },
  ] };
  function selected(paths, includeRegistration = true) {
    const context = {
      INTEGRATION_SUITES: [{ ...suite, paths: includeRegistration ? suite.paths : suite.paths.filter((p) => p !== `${EXAMPLE_035}/`) }],
      byName: new Map([["xtask", xtask]]), changed: paths, deferToPush: true, heavyChain: new Set(), deferred: { reader_rows: [] },
      result: { rust_changed: false, packages: [], main_unit_shards: [], unit_packages: [], unit_shards: [] },
      shortName: (name) => name, withTestFeatures: (flags) => flags,
      LIVE_ONLY_FEATURES: [], WHOLE_CHECKOUT: "__whole_checkout__",
    };
    vm.runInNewContext(between(planner, "  for (const suite of INTEGRATION_SUITES) {", "  // The closure check compiles"), context);
    vm.runInNewContext(between(planner, "function targetFlags(", "\nfunction bazelTestRow("), context);
    vm.runInNewContext(between(planner, "export function readerRows(", "// Longest-processing-time first").replace("export function", "function"), context);
    const selections = readerSelectionsFor(paths, loadNonRustReaders()).filter((row) => row.package === "xtask" && row.scope === "package");
    context.result.reader_rows = context.readerRows({ selections, result: context.result, byName: context.byName, heavyChain: context.heavyChain, deferToPush: true, deferred: context.deferred, compiledInDocs: new Set(), unified: new Map() });
    return context;
  }
  const actual = selected(exactPaths);
  assert.equal(actual.result.packages.length, 0); assert.equal(actual.result.rust_changed, false);
  assert.equal(actual.result.integration_suites.length, 1);
  assert.equal(actual.result.integration_suites[0].package_flags, "-p xtask --features machine-authority");
  assert.equal(actual.result.integration_suites[0].test_flags, undefined);
  assert(!actual.deferred.reader_rows.some((row) => row.includes("test targets")));
  assert.equal(actual.deferred.integration_suites.length, 0);
  assert(actual.result.reader_rows.some((row) => row.test_flags === "--lib --bins"));
  const before = selected(exactPaths, false);
  assert.equal(before.result.integration_suites.length, 0);
  assert(before.deferred.reader_rows.some((row) => row.includes("test targets")), "removing the registration reproduces F3");
  assert.equal(selected(["examples/035-mdm-tux-rs-other/src/lib.rs"]).result.integration_suites.length, 0);
  assert.equal(selected([`${EXAMPLE_035}/README.md`]).result.integration_suites.length, 1, "the exact prefix conservatively includes Markdown");
  const output = {}; actual.scalar = (key, value) => output[key] = value;
  vm.runInNewContext(between(planner, "function matrixOf(", "// The closure-tests job's rows"), actual);
  vm.runInNewContext(between(planner, "  const integrationRows =", '  scalar("test_scope_count"'), actual);
  assert.equal(output.integration_count, "2");
  const row = JSON.parse(output.integration_matrix).include.find((item) => item.name === "xtask");
  assert.equal(row.packages, "-p xtask --features machine-authority"); assert.equal(row.tests, undefined);
  const integration = between(workflow, "  integration:\n", "  closure-tests:\n");
  assert(integration.includes("if: ${{ needs.changes.result == 'success' && needs.changes.outputs.integration_count != '0' }}"));
  const flags = between(integration, '          if [[ -n "${TESTS}" ]]; then', '          # --no-tests=fail:');
  const parsed = spawnSync("bash", ["-euc", flags + '\nprintf "%s\\n" "${test_flags[@]}"'], { env: { PATH: "/usr/bin:/bin", TESTS: row.tests ?? "" }, encoding: "utf8" });
  assert.equal(parsed.status, 0); assert.equal(parsed.stdout, "--test\n*\n");
  assert(integration.includes('--profile ci-pr')); assert(integration.includes('--no-tests=fail'));
  const gate = between(workflow, '          if [[ "${INTEGRATION_COUNT}" =~', '          # Closure tests:');
  const prelude = 'failed=0; plan_errors=0; flagged_lanes=();\n' + between(workflow, "          require_ran() {", "          # The classifier is the fail-closed authority");
  for (const status of ["success", "failure", "cancelled", "skipped"]) {
    const checked = spawnSync("bash", ["-euc", prelude + gate + '\nexit "$failed"'], { env: { PATH: "/usr/bin:/bin", INTEGRATION_COUNT: output.integration_count, INTEGRATION_RESULT: status, CHANGES_RESULT: "success" }, encoding: "utf8" });
    assert.equal(checked.status, status === "success" ? 0 : 1, status);
  }
});
check("F3 selected xtask test targets contain both unignored live example audits", () => {
  for (const [binary, name, collector] of [
    ["runtime_authority_bypass", "live_workspace_runtime_authority_bypass_audit_is_clean", "collect_runtime_authority_bypass_findings"],
    ["effect_authority", "live_workspace_effect_authority_audit_is_clean", "collect_effect_authority_findings"],
  ]) {
    const source = text(`crates/xtask/tests/${binary}.rs`);
    assert(source.includes(`#[test]\nfn ${name}()`));
    assert(between(source, `fn ${name}()`, "\n}\n").includes(`${collector}(&root)`));
  }
  assert(text("crates/xtask/src/runtime_authority_bypass.rs").includes('["crates", "meta", "connect", "tests", "examples"]'));
  assert(text("crates/xtask/src/effect_authority.rs").includes('rel.starts_with("examples/")'));
  const profiles = JSON.parse(execFileSync("python3", ["-I", "-c", "import json,sys,tomllib;print(json.dumps(tomllib.load(open(sys.argv[1],'rb'))['profile']))", resolve(root, ".config/nextest.toml")], { encoding: "utf8" }));
  assert(!("default-filter" in profiles["ci-pr"])); assert(!("default-filter" in profiles.default));
  assert.equal(profiles["ci-pr"].inherits, "fast");
  const excluded = ["e2e_auth_lane", "e2e_build_lane", "e2e_fast_lane", "e2e_system_lane", "e2e_live_lane", "e2e_smoke_lane", "e2e_models_lane"];
  assert.equal(profiles.fast["default-filter"], excluded.map((name) => `not binary(${name})`).join(" and "));
  for (const binary of ["runtime_authority_bypass", "effect_authority"]) assert(!excluded.includes(binary));
});
check("actual shared readers still select examples, workflow invariant and planner readers", () => {
  const inventory = loadNonRustReaders();
  for (const paths of [exactPaths, [`${EXAMPLE_035}/README.md`]]) {
    const rows = readerSelectionsFor(paths, inventory);
    assert(rows.some((r) => r.package === "xtask" && r.target === "lib" && r.scope === "package" && r.reads.includes("examples/")));
  }
  const workflowRows = readerSelectionsFor([".github/workflows/ci.yml"], inventory);
  assert(workflowRows.some((r) => r.package === "xtask" && r.target === "test:ci_gate_requires_rmat"));
  const plannerRows = readerSelectionsFor(["scripts/ci-cargo-lanes.mjs"], inventory);
  assert(plannerRows.some((r) => r.package === "xtask"));
});

const scratch = mkdtempSync(resolve(tmpdir(), "meerkat-035-argv-"));
try {
  const example = resolve(scratch, EXAMPLE_035); const bin = resolve(scratch, "bin");
  mkdirSync(example, { recursive: true }); mkdirSync(bin); mkdirSync(resolve(scratch, "scripts"));
  copyFileSync(resolve(root, "scripts/repo-cargo"), resolve(scratch, "scripts/repo-cargo"));
  copyFileSync(resolve(root, EXAMPLE_035, "Makefile"), resolve(example, "Makefile"));
  writeFileSync(resolve(example, "Cargo.toml"), manifestText);
  execFileSync("git", ["init", "-q", scratch], { env: { PATH: "/usr/bin:/bin", HOME: scratch } });
  execFileSync("git", ["-C", scratch, "add", "--", `${EXAMPLE_035}/Cargo.toml`], { env: { PATH: "/usr/bin:/bin", HOME: scratch } });
  const record = resolve(scratch, "calls.jsonl");
  const fake = `#!${process.execPath}\nimport fs from 'node:fs'; fs.appendFileSync(process.env.RECORD, JSON.stringify({tool:process.argv[1].split('/').pop(),cwd:process.cwd(),argv:process.argv.slice(2)})+'\\n');\n`;
  for (const name of ["cargo-fake.mjs", "python3"]) writeFileSync(resolve(bin, name), fake, { mode: 0o755 });
  const env = { PATH: `${bin}:/usr/bin:/bin`, HOME: scratch, XDG_CACHE_HOME: resolve(scratch, "cache"), CARGO_HOME: resolve(scratch, "cargo-home"), CARGO_TARGET_DIR: resolve(scratch, "output"), CARGO_BIN: resolve(bin, "cargo-fake.mjs"), RECORD: record };
  check("real Make and unchanged repo-cargo route every subcommand to035, Python stays local", () => {
    const result = spawnSync("make", ["-C", example, "build", "check", "lint", "test", "clean"], { env, encoding: "utf8" });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    const rows = readFileSync(record, "utf8").trim().split("\n").map(JSON.parse);
    const cargo = rows.filter((r) => r.tool === "cargo-fake.mjs");
    assert.deepEqual(cargo.map((r) => r.argv[0]), ["build", "check", "clippy", "test", "clean"]);
    for (const row of cargo) {
      assert.equal(row.cwd, scratch);
      const index = row.argv.indexOf("--manifest-path"); assert(index > 0);
      assert.equal(row.argv[index + 1], resolve(example, "Cargo.toml"));
    }
    const python = rows.find((r) => r.tool === "python3"); assert.equal(python.cwd, example); assert.deepEqual(python.argv, ["scripts/test-scripts.py"]);
  });
  check("live file adapter refuses missing and symlink manifest; linked source prevents narrowing", () => {
    assert.equal(readExample035Inputs(scratch).manifestText, manifestText);
    mkdirSync(resolve(example, "src")); symlinkSync("../../../../outside.rs", resolve(example, "src/link.rs"));
    const i = readExample035Inputs(scratch, [`${EXAMPLE_035}/src/link.rs`]); assert(i.unsupported.includes(`${EXAMPLE_035}/src/link.rs`)); assert(select([], { inputs: i }).fallback);
    rmSync(resolve(example, "Cargo.toml")); assert.throws(() => readExample035Inputs(scratch));
    writeFileSync(resolve(scratch, "manifest-copy"), manifestText); symlinkSync("../../manifest-copy", resolve(example, "Cargo.toml")); assert.throws(() => readExample035Inputs(scratch), /regular file/);
  });
} finally { rmSync(scratch, { recursive: true, force: true }); }
console.log(`${checks} focused dry contracts passed; no native or standalone test execution.`);
