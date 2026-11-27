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
  targetRunEvidence,
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

  // SELECTED, RUN NOT SHOWN: credited only on a file that holds an
  // ignored-run flag AND the test's own name; the evidence is the file,
  // both lines and how they are paired. The state says that a file selects
  // the test, never that something runs that file.
  {
    const lane = { path: "scripts/sdk-lane", text: "#!/bin/sh\nset -e\ncargo test -p x --test sdk -- --exact runs_the_sdk_script \\\n  --ignored\n" };
    const evidence = ignoredRunEvidence("runs_the_sdk_script", [lane]);
    assert.deepEqual(evidence, [
      {
        path: "scripts/sdk-lane",
        covers: "test",
        pairing: "command",
        flag_line: 4,
        flag: "--ignored",
        selector_line: 3,
        selector: "cargo test -p x --test sdk -- --exact runs_the_sdk_script \\",
      },
    ]);
    assert.equal(readerExecution(stateOf(["runs_the_sdk_script"], ignored), () => evidence).execution, "selected, run not shown");
    // HOW THE FLAG AND THE NAME ARE PAIRED is part of the evidence. On one
    // line; in one command; or only in one file: a lane table whose entry
    // names the test and whose command builder holds the flag, beside the
    // flag of a command for something else. For a pairing per file the
    // first flag line and the number of flag lines are cited, not the
    // nearest line, which may be another command's.
    const pairingOf = (text) => ignoredRunEvidence("runs_the_sdk_script", [{ path: "lanes/table", text }]).map((item) => [item.pairing, item.flag_line, item.selector_line, item.flag_lines]);
    assert.deepEqual(pairingOf("cargo test -- --exact runs_the_sdk_script --ignored\n"), [["line", 1, 1, undefined]]);
    assert.deepEqual(pairingOf("cargo test -- --exact runs_the_sdk_script \\\n  --nocapture \\\n  --ignored\n"), [["command", 3, 1, undefined]]);
    const table = [
      "fn command(entry) {", //                              1
      '    args.push("--ignored");', //                       2: the builder's flag
      "}",
      'Entry { test_name: "some_other_test" },',
      'Entry { test_name: "runs_the_sdk_script" },', //       5: the entry
      'let raw = ["-p", "tool", "--test", "resume", "--",',
      '    "--ignored",', //                                  7: another command's flag, the nearest
      "];",
    ].join("\n");
    assert.deepEqual(pairingOf(table), [["file", 2, 5, 2]], "the first flag line of the file, and how many it has");
    // A name in a comparison, in a script whose flag runs another test, is
    // such a pairing per file too, and says so.
    const sibling = 'SELECTOR = "its_own_test"\nassert original == "runs_the_sdk_script"\nos.execve(binary, [binary, "--exact", SELECTOR, "--ignored"], env)\n';
    assert.deepEqual(pairingOf(sibling), [["file", 3, 2, 1]]);
    // No credit for a file with the flag and ANOTHER test's name, for one
    // that names the test and asks for no ignored test, for a longer name
    // that only contains it, for a lane's name, for a runner that skips
    // the test (on one line, or as the next item of an argument list
    // written one item a line), or for a comment that names the test or
    // the flag.
    for (const text of [
      "cargo test -- --exact some_other_test --ignored\n",
      "cargo test -- --exact runs_the_sdk_script\n",
      "cargo test -- --exact runs_the_sdk_script_twice --ignored\n",
      "# lane: sdk (ignored tests)\ncargo test -p x --test sdk -- --ignored\n",
      "cargo test -p x --test sdk -- --ignored --skip runs_the_sdk_script\n",
      "cargo test -p x --test sdk -- --ignored --skip=sdk::runs_the_sdk_script\n",
      'let args = [\n    "--ignored",\n    "--skip",\n    "runs_the_sdk_script",\n];\n',
      'let args = [\n    "--ignored",\n    "--skip",\n\n    // not this one\n    "sdk::runs_the_sdk_script",\n];\n',
      "# runs_the_sdk_script is not run here\ncargo test -p x --test sdk -- --ignored\n",
      "// to run it: cargo test -- --ignored\nconst NAME: &str = \"runs_the_sdk_script\";\n",
    ]) {
      assert.deepEqual(ignoredRunEvidence("runs_the_sdk_script", [{ path: "scripts/other", text }]), [], text.trim());
    }
    // An item after `--skip` on the line above is skipped; the item after
    // that is a selector again.
    assert.equal(ignoredRunEvidence("runs_the_sdk_script", [{ path: "scripts/other", text: 'let args = [\n    "--ignored",\n    "--skip",\n    "some_other_test",\n    "runs_the_sdk_script",\n];\n' }]).length, 1);
  }

  // A FILE THAT ONLY ASSERTS A RUNNER'S ARGUMENTS (a test of a launcher's
  // argument list, a check of a recorded one) issues no command. It is
  // evidence of a claim, marked, and never credits a test by itself: a test
  // named only there is UNPROVEN. Beside a file that issues the command,
  // the credit is the issuing file's.
  {
    const asserting = {
      path: "scripts/test_launchers.py",
      text: 'CASES = [("late", "runs_the_sdk_script")]\n\ndef test_argv(self):\n    self.assertEqual(actual["argv"], ["--exact", selector, "--ignored", "--quiet"])\n',
    };
    const runner = { path: "scripts/launcher.py", text: 'SELECTOR = "runs_the_sdk_script"\nos.execve(binary, [binary, "--exact", SELECTOR, "--ignored"], env)\n' };
    const marks = (files) => ignoredRunEvidence("runs_the_sdk_script", files).map((item) => [item.path, item.pairing, item.asserted ?? false]);
    assert.deepEqual(marks([asserting]), [["scripts/test_launchers.py", "file", true]]);
    assert.deepEqual(marks([runner, asserting]), [
      ["scripts/launcher.py", "file", false],
      ["scripts/test_launchers.py", "file", true],
    ]);
    const state = stateOf(["runs_the_sdk_script"], ignored);
    const alone = readerExecution(state, (name) => ignoredRunEvidence(name, [asserting]));
    assert.deepEqual([alone.execution, alone.ignored_tests[0].execution, alone.ignored_tests[0].evidence.length], ["unproven", "unproven", 1], "the asserting file alone credits nothing");
    assert.equal(readerExecution(state, (name) => ignoredRunEvidence(name, [runner, asserting])).execution, "selected, run not shown");
    // The true runner no longer carries the literal name (it takes it from
    // elsewhere) and only the asserting file does: unproven, not credit.
    const indirect = { path: "scripts/launcher.py", text: "SELECTOR = cases.SELECTOR\nos.execve(binary, [binary, \"--exact\", SELECTOR, \"--ignored\"], env)\n" };
    assert.equal(readerExecution(state, (name) => ignoredRunEvidence(name, [indirect, asserting])).execution, "unproven");
    // Other shapes of a statement about arguments: a Rust assertion, a
    // membership check. A file that both asserts and issues is an issuer.
    for (const line of ['assert_eq!(argv, ["--exact", name, "--ignored"]);', 'ok = "--exact" in argv and "--ignored" in argv', 'expect(argv).toContain("--ignored");']) {
      assert.deepEqual(marks([{ path: "scripts/check", text: `NAME = "runs_the_sdk_script"\n${line}\n` }]), [["scripts/check", "file", true]], line);
    }
    assert.deepEqual(marks([{ path: "scripts/both", text: `${asserting.text}${runner.text}` }]), [["scripts/both", "file", false]]);
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
      ["unproven", ["ignored, not executed", "selected, run not shown"]],
    );
  }

  // A RUNNER THAT NAMES NO TEST: a cargo test command that asks for ignored
  // tests of the row's whole target, or of its whole package, covers every
  // test of the row. The evidence is the command, with its file and lines.
  {
    const row = { package: "sdk", target: "test:contract" };
    const places = (text, of = row) => {
      const found = targetRunEvidence(of, [{ path: "scripts/lane", text }]);
      return [found.covering.map((item) => [item.line, item.flag_line, item.covers]), found.unclear.map((item) => item.line)];
    };
    // The target selector, in the shapes a lane writes it.
    for (const text of [
      "cargo test -p sdk --test contract --locked -- --ignored\n",
      "./scripts/repo-cargo nextest run -p sdk --test contract --run-ignored all --no-fail-fast\n",
      'lane = "nextest run -p sdk --test contract --run-ignored ignored-only --test-threads=1"\n',
      'args = ["test", "--package", "sdk", "--test", "contract", "--", "--include-ignored"]\n',
      "cargo test --package=sdk --test=contract -- --ignored --nocapture 2>&1 | tee lane.log\n",
    ]) {
      assert.deepEqual(places(text), [[[1, 1, "target"]], []], text.trim());
    }
    assert.deepEqual(places("set -e\ncargo test -p sdk \\\n  --test contract -- \\\n  --ignored\n"), [[[2, 4, "target"]], []], "a command over several lines");
    assert.deepEqual(targetRunEvidence(row, [{ path: "scripts/lane", text: "set -e\ncargo test -p sdk --test contract -- --ignored\n" }]).covering, [
      { path: "scripts/lane", line: 2, flag_line: 2, command: "cargo test -p sdk --test contract -- --ignored", covers: "target" },
    ]);
    // The package-wide run: no target selector, every test target, every
    // target, or the workspace.
    for (const text of [
      "cargo test -p sdk -- --ignored\n",
      "cargo nextest run -p sdk --run-ignored all\n",
      "cargo test -p sdk --tests -- --include-ignored\n",
      "cargo test -p sdk --all-targets --all-features -- --ignored\n",
      "cargo test --workspace -- --ignored\n",
    ]) {
      assert.deepEqual(places(text), [[[1, 1, "package"]], []], text.trim());
    }
    // TOLD APART, no evidence and nothing unclear: another package, another
    // target, the library only, the flag in another command, the package
    // excluded, a comment, no flag, git's own flag.
    for (const text of [
      "cargo test -p other --test contract -- --ignored\n",
      "cargo test -p sdk --test another -- --ignored\n",
      "cargo test -p sdk --lib -- --ignored\n",
      "cargo test -p sdk --test contract\ncargo test -p other -- --ignored\n",
      "cargo test --workspace --exclude sdk -- --ignored\n",
      "# cargo test -p sdk --test contract -- --ignored\n",
      "cargo test -p sdk --test contract -- --nocapture\n",
      "git status --porcelain --ignored && cargo test -p sdk --test contract\n",
    ]) {
      assert.deepEqual(places(text), [[], []], text.trim());
    }
    // NOT TOLD APART, no credit and the row unproven: the target and no
    // package, a package the text does not hold, a filter, a nextest
    // profile, an unknown value, and a flag on a line of its own in a file
    // that selects the target.
    for (const text of [
      "cargo test --test contract -- --ignored\n",
      'cargo test -p "${PACKAGE}" --test contract -- --ignored\n',
      "cargo test -p sdk --test contract -- --ignored --exact one_test\n",
      "cargo nextest run -p sdk --test contract --run-ignored all -E 'test(one)'\n",
      "cargo nextest run -p sdk --test contract --profile live --run-ignored all\n",
      "cargo test -p sdk --test contract $EXTRA -- --ignored\n",
      "cargo test -p sdk --test contract -- --ignored one_test\n",
    ]) {
      assert.deepEqual(places(text), [[], [1]], text.trim());
    }
    assert.deepEqual(places('command = ["cargo", "test", "-p", "sdk", "--test", "contract", "--",\n    "--ignored"]\n'), [[], [2]], "an argument list over two lines");
    assert.deepEqual(places('ARGS = ["--exact", name, "--ignored"]\n'), [[], []], "a flag in a file that does not select the target says nothing of it");
    // A statement of a runner's arguments (a test of a launcher) is no
    // command: it never covers, and leaves the row unproven.
    assert.deepEqual(places('assert argv == ["cargo", "test", "-p", "sdk", "--test", "contract", "--", "--ignored"]\n'), [[], [1]], "an asserted command covers nothing");
    // The features a row needs: a command without them may compile the
    // tests out.
    const featured = { ...row, features: ["python"] };
    assert.deepEqual(places("cargo test -p sdk --test contract -- --ignored\n", featured), [[], [1]]);
    assert.deepEqual(places("cargo test -p sdk --features python,other --test contract -- --ignored\n", featured), [[[1, 1, "target"]], []]);
    assert.deepEqual(places("cargo test -p sdk --all-features --test contract -- --ignored\n", featured), [[[1, 1, "target"]], []]);
    // A library row: --lib names its target; a test target's run is not it.
    const library = { package: "sdk", target: "lib" };
    assert.deepEqual(places("cargo test -p sdk --lib -- --ignored\n", library), [[[1, 1, "target"]], []]);
    assert.deepEqual(places("cargo test -p sdk -- --ignored\n", library), [[[1, 1, "package"]], []]);
    assert.deepEqual(places("cargo test -p sdk --test contract -- --ignored\n", library), [[], []]);

    // The row's state: a covering command credits EVERY ignored test of the
    // row, which no test's own evidence did; an unclear one credits none
    // and leaves the row unproven, not "not executed".
    const second = '#[test]\n#[ignore]\nfn runs_the_other_script() {}\n';
    const two = stateOf(["runs_the_other_script", "runs_the_sdk_script"], ignored, second);
    const whole = (text) => targetRunEvidence(row, [{ path: "scripts/lane", text }]);
    const covered = readerExecution(two, none, whole("cargo test -p sdk --test contract -- --ignored\n"));
    assert.deepEqual(
      [covered.execution, covered.ignored_tests.map((test) => [test.execution, test.evidence]), covered.unclear_runners],
      [
        "selected, run not shown",
        ["runs_the_other_script", "runs_the_sdk_script"].map(() => [
          "selected, run not shown",
          [
            {
              path: "scripts/lane",
              covers: "target",
              pairing: "command",
              flag_line: 1,
              flag: "cargo test -p sdk --test contract -- --ignored",
              selector_line: 1,
              selector: "cargo test -p sdk --test contract -- --ignored",
            },
          ],
        ]),
        [],
      ],
    );
    const unsettled = readerExecution(two, none, whole("cargo test --test contract -- --ignored\n"));
    assert.deepEqual(
      [unsettled.execution, unsettled.ignored_tests.map((test) => [test.execution, test.evidence]), unsettled.unclear_runners.map((item) => [item.path, item.line, item.why])],
      ["unproven", [["unproven", []], ["unproven", []]], [["scripts/lane", 1, "selects --test contract and names no package"]]],
    );
    assert.equal(readerExecution(two, none, whole("cargo test -p other --test contract -- --ignored\n")).execution, "ignored, not executed", "another package's run is no runner of the row");
    // In a target with plain tests too, the covering command credits the
    // ignored one.
    const part = readerExecution(stateOf(null, `${plain}${ignored}`), none, whole("cargo test -p sdk -- --ignored\n"));
    assert.deepEqual([part.execution, part.ignored_tests.map((test) => [test.name, test.execution, test.evidence[0].covers])], ["executed in part", [["runs_the_sdk_script", "selected, run not shown", "package"]]]);
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
