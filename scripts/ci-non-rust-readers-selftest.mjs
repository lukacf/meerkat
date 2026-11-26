#!/usr/bin/env node
// Contract test for scripts/ci-non-rust-readers.mjs, the pure mapping from a
// changed path list to the Rust tests that read non-Rust inputs. Each case
// pins a selection the planner relies on; the inventory is the committed
// scripts/ci-non-rust-readers.json (kept complete by
// scripts/ci-non-rust-readers-guard.mjs) or a small synthetic one.
import assert from "node:assert/strict";
import {
  READER_DEFINITION_PATHS,
  WHOLE_CHECKOUT,
  hasIgnoredRunFlag,
  ignoredRunEvidence,
  inventoryPathCovers,
  loadNonRustReaders,
  parseNonRustReaders,
  readerExecution,
  readerPackagesFor,
  readerSelectionsFor,
  readerTestState,
  rustTestAttributes,
} from "./ci-non-rust-readers.mjs";

const inventory = loadNonRustReaders();

// Rows a path selects through its own inventory keys, leaving out the
// readers of the whole checkout, which every path selects.
const pathSpecific = (paths) =>
  readerSelectionsFor(paths, inventory)
    .map((row) => ({ ...row, reads: row.reads.filter((read) => read !== WHOLE_CHECKOUT) }))
    .filter((row) => row.reads.length > 0);
const has = (rows, pkg, target) => rows.some((row) => row.package === pkg && row.target === target);

// A docs-only change selects the rkat doc-example fixture test, xtask's docs
// readers and the machine-schema docs alignment test.
{
  const packages = readerPackagesFor(["docs/guides/skills.mdx"], inventory);
  for (const name of ["rkat", "xtask", "meerkat-machine-schema"]) {
    assert.ok(packages.includes(name), `a docs-only change selects ${name}: ${packages.join(" ")}`);
  }
  const rows = pathSpecific(["docs/guides/skills.mdx"]);
  const rkat = rows.find((row) => row.package === "rkat" && row.target === "test:doc_examples_fixture");
  assert.ok(rkat, "the rkat doc-example fixture test reads docs/ and fails when its fixture drifts");
  assert.equal(rkat.tests, null, "the fixture test target runs whole");
  assert.equal(rkat.scope, "target");
  assert.ok(!has(rows, "rkat", "bin:rkat"), "the rkat binary parses its packaged fixture, not docs/");
  assert.ok(has(rows, "meerkat-machine-schema", "test:docs_machine_authority_alignment"));
  assert.ok(has(rows, "xtask", "test:buildbuddy_static_lanes"));
  assert.ok(!has(rows, "rkat", "test:help_mobpack_contract"), "help_mobpack_contract reads seven named docs, not this one");
}

// A docs file a test embeds with include_str! selects that test target.
{
  const rows = pathSpecific(["docs/guides/mobpack.mdx"]);
  assert.ok(has(rows, "rkat", "test:help_mobpack_contract"));
  // The doc-example fixture walk and the static lane walk read it too.
  assert.ok(has(rows, "rkat", "test:doc_examples_fixture"));
}

// An sdks/ change selects its readers: xtask's version-parity and
// generated-type checks and the integration-test SDK lanes.
{
  const rows = pathSpecific(["sdks/python/meerkat/generated/types.py"]);
  assert.ok(has(rows, "xtask", "test:buildbuddy_static_lanes"), "buildbuddy_static_lanes reads the generated Python types");
  assert.ok(has(rows, "xtask", "lib"), "public_contracts reads sdks/python/meerkat/generated/");
  assert.ok(has(rows, "meerkat-integration-tests", "lib"), "e2e_lanes runs the Python SDK suite from sdks/python");
  const web = pathSpecific(["sdks/web/package.json"]);
  assert.ok(has(web, "xtask", "test:buildbuddy_static_lanes"));
  assert.ok(has(web, "meerkat-integration-tests", "lib"));
}

// CHANGELOG.md is read by xtask's static lane scans only. The CHANGELOG
// mentions in meerkat-mob (cold_resume_digest_cost.rs) and meerkat-session
// (event_store.rs) are comments, not reads, so they select nothing.
{
  const rows = pathSpecific(["CHANGELOG.md"]);
  assert.deepEqual(
    [...new Set(rows.map((row) => row.package))],
    ["xtask"],
    `CHANGELOG.md selects xtask only: ${JSON.stringify(rows.map((row) => `${row.package} ${row.target}`))}`,
  );
  assert.ok(has(rows, "xtask", "test:buildbuddy_static_lanes"));
}

// An unmapped non-Rust path selects nothing new: only the readers of the
// whole checkout, which every path selects.
{
  for (const path of ["notes/unmapped-for-selftest.txt", "renovate.json"]) {
    assert.deepEqual(pathSpecific([path]), [], `${path} selects no path-specific reader`);
    for (const row of readerSelectionsFor([path], inventory)) {
      assert.deepEqual(row.reads, [WHOLE_CHECKOUT], `${path}: ${row.package} ${row.target} reads the whole checkout`);
    }
  }
  const whole = readerSelectionsFor(["notes/unmapped-for-selftest.txt"], inventory);
  assert.ok(has(whole, "xtask", "test:buildbuddy_static_lanes"), "the deprecated-backend scan walks the whole checkout");
  assert.ok(has(whole, "meerkat-integration-tests", "test:phase6_cleanup_sweep"));
  assert.deepEqual(readerSelectionsFor([], inventory), [], "no changed path, no selection");
}

// A deleted input still selects its readers: the path list carries the
// deletion (and both sides of a rename, as the planner's --no-renames diff
// lists them), and selection never consults the working tree.
{
  const deleted = pathSpecific(["docs/docs.json"]);
  assert.ok(has(deleted, "rkat", "test:help_mobpack_contract"), "deleting docs/docs.json selects its include_str! reader");
  const renamed = pathSpecific(["tests/fixtures/live_smoke/support.rs", "tests/fixtures/live_smoke/support_renamed.rs"]);
  for (const [pkg, target] of [["meerkat-mcp-server", "test:live_mcp_matrix"], ["meerkat-rest", "test:live_rest_matrix"], ["meerkat-rpc", "test:live_rpc_regression"], ["meerkat-rpc", "test:live_smoke_rpc"]]) {
    assert.ok(has(renamed, pkg, target), `renaming the live smoke support module away selects ${pkg} ${target}`);
  }
  const row = renamed.find((candidate) => candidate.package === "meerkat-rest");
  assert.deepEqual(row.paths, ["tests/fixtures/live_smoke/support.rs"], "the selecting path is the old side");
}

// Cross-crate inputs select the reader, not only the owning crate.
{
  const rows = pathSpecific(["crates/meerkat-machine-kernels/src/generated/meerkat.rs"]);
  assert.ok(has(rows, "meerkat-machine-codegen", "test:runtime_schema_parity"), "meerkat-machine-codegen's parity test embeds meerkat-machine-kernels' generated kernel");
  const specs = pathSpecific(["specs/machines/meerkat_machine/model.tla"]);
  assert.ok(has(specs, "meerkat-runtime", "test:core_apply_terminal_truth"));
  assert.ok(has(specs, "xtask", "lib"));
  const skill = pathSpecific([".claude/skills/meerkat-platform/SKILL.md"]);
  const facade = skill.find((candidate) => candidate.package === "meerkat" && candidate.target === "lib");
  assert.equal(facade?.scope, "package", "the facade embeds the platform skill in library code");
  assert.ok(has(skill, "meerkat-machine-schema", "test:docs_machine_authority_alignment"));
}

// A change to the inventory or its model selects every reader it names.
{
  const all = new Set(inventory.entries.map((entry) => `${entry.package} ${entry.target}`));
  for (const path of READER_DEFINITION_PATHS) {
    const rows = readerSelectionsFor([path], inventory);
    assert.deepEqual(new Set(rows.map((row) => `${row.package} ${row.target}`)), all, `${path} selects every reader`);
  }
}

// Pure: the same input gives the same output and is not mutated.
{
  const paths = ["docs/index.mdx", "./sdks/web/package.json", "docs/index.mdx"];
  const copy = [...paths];
  const first = readerSelectionsFor(paths, inventory);
  assert.deepEqual(readerSelectionsFor(paths, inventory), first);
  assert.deepEqual(paths, copy);
  assert.ok(first.some((row) => row.paths.includes("sdks/web/package.json")), "./ prefixes are normalized");
}

// Synthetic inventory: prefix and exact keys, the whole-checkout key, test
// filters (a reader without one needs its whole target), features, scope.
{
  const synthetic = parseNonRustReaders({
    schema_version: 1,
    paths: {
      "./": [{ package: "audit", target: "test:live", scope: "target", tests: ["live_scan"], sources: ["crates/audit/tests/live.rs"] }],
      "docs/": [{ package: "cli", target: "bin:cli", scope: "target", tests: ["doc_examples"], sources: ["crates/cli/src/main.rs"] }],
      "docs/a.mdx": [
        { package: "cli", target: "bin:cli", scope: "target", sources: ["crates/cli/src/main.rs"] },
        { package: "lib", target: "lib", scope: "package", features: ["extra"], sources: ["crates/lib/src/lib.rs"] },
      ],
    },
    not_readers: [],
  });
  assert.equal(inventoryPathCovers("docs/", "docs/b/c.mdx"), true);
  assert.equal(inventoryPathCovers("docs/", "docsx/b.mdx"), false);
  assert.equal(inventoryPathCovers("docs/a.mdx", "docs/a.mdx.bak"), false);
  assert.equal(inventoryPathCovers(WHOLE_CHECKOUT, "anything/at/all"), true);
  const only = readerSelectionsFor(["docs/b.mdx"], synthetic);
  assert.deepEqual(only.map((row) => [row.package, row.target, row.tests]), [
    ["audit", "test:live", ["live_scan"]],
    ["cli", "bin:cli", ["doc_examples"]],
  ]);
  const both = readerSelectionsFor(["docs/a.mdx"], synthetic);
  assert.equal(both.find((row) => row.package === "cli").tests, null, "an unfiltered reader widens the row to the whole target");
  assert.deepEqual(both.find((row) => row.package === "lib"), {
    package: "lib",
    target: "lib",
    scope: "package",
    tests: null,
    features: ["extra"],
    paths: ["docs/a.mdx"],
    reads: ["docs/a.mdx"],
  });
  assert.deepEqual(readerPackagesFor(["docs/a.mdx"], synthetic), ["audit", "cli", "lib"]);
}

// A malformed inventory fails loudly instead of selecting nothing.
{
  const row = { package: "cli", target: "bin:cli", scope: "target", sources: ["crates/cli/src/main.rs"] };
  const bad = [
    ["unsorted keys", { schema_version: 1, paths: { "docs/b": [row], "docs/a": [row] } }],
    ["bad target", { schema_version: 1, paths: { "docs/": [{ ...row, target: "unit" }] } }],
    ["no sources", { schema_version: 1, paths: { "docs/": [{ ...row, sources: [] }] } }],
    ["bad scope", { schema_version: 1, paths: { "docs/": [{ ...row, scope: "workspace" }] } }],
    ["climbing key", { schema_version: 1, paths: { "../docs/": [row] } }],
    ["duplicate row", { schema_version: 1, paths: { "docs/": [row, row] } }],
    ["wrong schema", { schema_version: 2, paths: {} }],
    ["not_readers without why", { schema_version: 1, paths: {}, not_readers: [{ source: "a.rs", literal: "x" }] }],
  ];
  for (const [label, document] of bad) {
    assert.throws(() => parseNonRustReaders(document), Error, label);
  }
}

// What a lane does with a reader row's tests: one fixture for each state,
// the file that holds ignored and plain tests, and the change of a source.
{
  const plain = '#[test]\nfn reads_the_docs() {}\n';
  const ignored = '#[test]\n#[ignore] // needs a Python environment\nfn runs_the_sdk_script() {}\n';
  const laneIgnored = '#[tokio::test(flavor = "multi_thread")]\n#[ignore = "lane:e2e-smoke"]\nasync fn e2e_scenario_one() {}\n';
  const stateOf = (tests, ...texts) => readerTestState(tests, texts.map((text, index) => ({ path: `tests/file_${index}.rs`, text })));
  const none = () => [];

  // The test attributes of a source: the kinds of test attribute, an ignore
  // with a literal reason and one with a comment, an ignore that holds on
  // some builds only, an attribute over several lines, comments and blank
  // lines between an attribute and its function, and a function that is no
  // test.
  assert.deepEqual(
    rustTestAttributes(
      [
        "#[test]",
        "fn first() {}",
        "",
        "#[tokio::test(",
        '    flavor = "multi_thread",',
        ")]",
        '#[ignore = "lane:e2e-live"]',
        "// a comment between the attributes and the item",
        "",
        "pub(crate) async fn second() {}",
        "#[test]",
        '#[cfg_attr(not(target_os = "linux"), ignore)]',
        "fn third() {}",
        "#[rstest]",
        "#[ignore] // by hand only",
        "fn fourth() {}",
        "#[cfg(test)]",
        "fn helper() {}",
        "#[derive(Debug)]",
        "struct NotAFunction;",
        "fn after_a_struct() {}",
      ].join("\n"),
    ),
    [
      { name: "first", line: 2, ignored: "no", reason: null },
      { name: "second", line: 10, ignored: "always", reason: "lane:e2e-live" },
      { name: "third", line: 13, ignored: "conditional", reason: null },
      { name: "fourth", line: 16, ignored: "always", reason: "by hand only" },
    ],
  );

  // An ignored-run flag is a runner's; git's own --ignored and nextest's
  // default mode are not.
  for (const line of [
    'cargo test -p x --test y -- --ignored --nocapture',
    "os.execve(binary, [binary, '--exact', selector, '--ignored'], env)",
    "nextest run --run-ignored all -E 'test(x)'",
    "nextest run --run-ignored=ignored-only",
    "cargo test -- --include-ignored",
    "          run_ignored: all",
  ]) {
    assert.ok(hasIgnoredRunFlag(line), `${line} asks for ignored tests`);
  }
  for (const line of [
    'subprocess.run(["git", "status", "--porcelain", "--ignored"])',
    "git ls-files --others --ignored --exclude-standard",
    "nextest run --run-ignored default",
    "cargo test -- --ignored-by-design",
    "# these tests are ignored",
  ]) {
    assert.ok(!hasIgnoredRunFlag(line), `${line} asks for no ignored test`);
  }

  // EXECUTED: a test the row names carries no ignore attribute.
  assert.equal(readerExecution(stateOf(["reads_the_docs"], plain), none).execution, "executed");

  // IGNORED, NOT EXECUTED: every named test is ignored, no file shows a
  // runner, and the ignore reason names no lane.
  {
    const report = readerExecution(stateOf(["runs_the_sdk_script"], ignored), none);
    assert.deepEqual(
      [report.execution, report.tests, report.ignored, report.ignored_tests.map((test) => [test.name, test.reason, test.execution, test.evidence])],
      ["ignored, not executed", 1, 1, [["runs_the_sdk_script", "needs a Python environment", "ignored, not executed", []]]],
    );
  }

  // SEPARATELY EXECUTED: credited only on a file that holds an ignored-run
  // flag AND the test's own name; the evidence is the file and both lines.
  {
    const lane = { path: "scripts/sdk-lane", text: "#!/bin/sh\nset -e\ncargo test -p x --test sdk -- --exact runs_the_sdk_script \\\n  --ignored\n" };
    const evidence = ignoredRunEvidence("runs_the_sdk_script", [lane]);
    assert.deepEqual(evidence, [
      {
        path: "scripts/sdk-lane",
        flag_line: 4,
        flag: "--ignored",
        selector_line: 3,
        selector: "cargo test -p x --test sdk -- --exact runs_the_sdk_script \\",
      },
    ]);
    assert.equal(readerExecution(stateOf(["runs_the_sdk_script"], ignored), () => evidence).execution, "separately executed");
    // No credit for a file with the flag and ANOTHER test's name, for one
    // that names the test and asks for no ignored test, for a longer name
    // that only contains it, for a lane's name, for a runner that skips
    // the test, or for a comment that names the test or the flag.
    for (const text of [
      "cargo test -- --exact some_other_test --ignored\n",
      "cargo test -- --exact runs_the_sdk_script\n",
      "cargo test -- --exact runs_the_sdk_script_twice --ignored\n",
      "# lane: sdk (ignored tests)\ncargo test -p x --test sdk -- --ignored\n",
      "cargo test -p x --test sdk -- --ignored --skip runs_the_sdk_script\n",
      "cargo test -p x --test sdk -- --ignored --skip=sdk::runs_the_sdk_script\n",
      "# runs_the_sdk_script is not run here\ncargo test -p x --test sdk -- --ignored\n",
      "// to run it: cargo test -- --ignored\nconst NAME: &str = \"runs_the_sdk_script\";\n",
    ]) {
      assert.deepEqual(ignoredRunEvidence("runs_the_sdk_script", [{ path: "scripts/other", text }]), [], text.trim());
    }
  }

  // UNPROVEN: a runner is claimed and not shown. The ignore reason names a
  // lane; or the evidence could not be read; or only some of the row's
  // ignored tests have evidence. Never "executed", never "not executed".
  assert.equal(readerExecution(stateOf(["e2e_scenario_one"], laneIgnored), none).execution, "unproven");
  assert.equal(readerExecution(stateOf(["e2e_scenario_one"], laneIgnored), none).ignored_tests[0].claimed_lane, "e2e-smoke");
  assert.equal(readerExecution(stateOf(["runs_the_sdk_script"], ignored), () => null).execution, "unproven", "an unread search is no proof of absence");
  {
    const some = (name) => (name === "runs_the_sdk_script" ? [{ path: "scripts/sdk-lane", flag_line: 1, flag: "--ignored", selector_line: 1, selector: name }] : []);
    const second = '#[test]\n#[ignore]\nfn runs_the_other_script() {}\n';
    const report = readerExecution(stateOf(["runs_the_other_script", "runs_the_sdk_script"], ignored, second), some);
    assert.deepEqual(
      [report.execution, report.ignored_tests.map((test) => test.execution)],
      ["unproven", ["ignored, not executed", "separately executed"]],
    );
  }

  // THE MIXED FILE: a target with a plain and an ignored test is executed
  // in part, and the ignored one is reported by itself.
  {
    const report = readerExecution(stateOf(null, `${plain}${ignored}`), none);
    assert.deepEqual(
      [report.execution, report.tests, report.ignored, report.ignored_tests.map((test) => [test.name, test.execution])],
      ["executed in part", 2, 1, [["runs_the_sdk_script", "ignored, not executed"]]],
    );
    // A row that names only the plain test of that file is executed; one
    // that names only the ignored test is not.
    assert.equal(readerExecution(stateOf(["reads_the_docs"], `${plain}${ignored}`), none).execution, "executed");
    assert.equal(readerExecution(stateOf(["runs_the_sdk_script"], `${plain}${ignored}`), none).execution, "ignored, not executed");
  }

  // THE SOURCE CHANGES, THE REPORT FOLLOWS: the same test without its
  // ignore attribute is executed; with a conditional ignore it is not
  // counted as ignored; ignored in one module and plain in another, it
  // runs.
  assert.equal(readerExecution(stateOf(["runs_the_sdk_script"], ignored.replace("#[ignore] // needs a Python environment\n", "")), none).execution, "executed");
  assert.equal(
    readerExecution(stateOf(["runs_the_sdk_script"], ignored.replace("#[ignore]", '#[cfg_attr(windows, ignore)]')), none).execution,
    "executed",
  );
  assert.equal(readerExecution(stateOf(["runs_the_sdk_script"], ignored, ignored.replace("#[ignore] // needs a Python environment\n", "")), none).execution, "executed");

  // NOT JUDGED: a named test no source holds, or a target without a test.
  {
    const report = readerExecution(stateOf(["renamed_away"], plain), none);
    assert.deepEqual([report.execution, report.unknown], ["not judged", ["renamed_away"]]);
    assert.equal(readerExecution(stateOf(null, "fn main() {}\n"), none).execution, "not judged");
  }
}

console.log("ci-non-rust-readers selection contracts hold");
