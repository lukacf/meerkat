#!/usr/bin/env node
// Hook coverage equivalence: does a CI lane plan run every test binary that
// the deterministic workspace lanes (scripts/pre-push-unit.sh, the hook's
// cargo-test body, which the pre-push hook now runs only on demand) run,
// under the feature set they build it with? The push runs these rows; a
// pull-request plan runs none and is checked against its push's rows.
//
// The hook builds every test binary of Core's members in one invocation,
//   cargo nextest list --workspace --exclude <every other product's package>
//     -E 'kind(lib) or kind(test)'
// (scripts/core-workspace-args.mjs derives the selection from
// ci/products.json), so each Core member is compiled with the features
// Core's whole workspace unifies on it (`cargo tree`'s roots for that
// selection; on 8e1f4f21, 18 of the 53 members got more than their
// package-scoped build, meerkat-mcp's test-support among them), and a test
// target whose required-features are outside that set is not built at all.
// The other products' packages are out of the hook: their test binaries run
// in rows that select exactly that product's packages (`-p` each), which
// compile them as the product's own workspace did. It then runs its lanes over those binaries, and
// HeadCanonical from its own package-scoped build. The lanes are read from
// the hook script itself (hookModel), not copied: every nextest command in it
// must have one of the shapes below, or the check fails closed.
//
// The required CI rows run that selection for the plan's test scope: the
// reverse-dependency closure of the changed packages (every member when a
// global input changed) plus the readers of changed non-Rust inputs. This
// module derives the hook's selection for that scope from cargo metadata,
// .config/nextest.toml and the hook script, and reads each planned row the way
// the workflow step that executes it does (CI_ROW_STEPS: the step's exact
// list and run lines, its one selection array, its matrix and env). Any other
// nextest line in such a step, a different selection, or a missing binary
// check is an error. It then reports every hook binary that no row runs under
// the hook's own feature set, and every row Cargo would refuse. The planner
// calls it on its own result and fails the plan on any gap.
//
// Only a row that selects Core's whole workspace (`--workspace` and an
// `--exclude` for exactly every other product's package) is credited for a
// hook binary: Cargo then resolves every crate's features exactly as the
// hook's build does, so the row compiles the hook's own units (a
// package-scoped row compiles its dependencies with fewer features and is not
// the hook's build). A bare `--workspace` row is refused: it would mix the
// products. A product's test binary is credited only to a row that selects
// exactly that product's packages. HeadCanonical is credited for its own,
// package-scoped command.
// A row that runs one count partition (`partition: count:K/N`, the
// nested-Cargo canaries' rows) credits nothing alone: rows of one exact
// selection are credited together, and only when they hold every partition
// 1..N exactly once.
//
// A plan may leave lanes of the hook script without a row
// (hook_retained_lanes, by the lane's label in the hook script): the lib
// binaries such a lane runs need no row, because the push-to-main unit lanes
// run every member's lib tests and nightly's workspace unit lane runs them
// under the workspace feature set. Only lib binaries: whatever else a
// retained lane runs still needs a row, as does HeadCanonical. The planner
// leaves the hook's kind(lib) lane there when no runner can build the
// `--workspace --lib` row in time (--lib-row-runner). A row that names a
// runner must run exactly where its bound job runs.
//
// usage: ci-hook-coverage.mjs [--plan <file>|-] [--map]
//   Reads a plan (ci-cargo-lanes.mjs --format json) and prints the coverage
//   summary; exits 1 on a gap. --map prints every hook binary with the CI row
//   that runs it, or the hook lane the plan leaves to the unit lanes. A
//   pull-request plan holds no rows: check its push plan instead.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  CORE,
  coreWorkspaceArgs,
  loadProducts,
  productPackageArgs,
  workspaceMembersByProduct,
} from "./ci-products.mjs";
import { readMetadata, readSelectionFeatures, root, workspacePackages } from "./rust-test-selector.mjs";

export const HOOK_SCRIPT = "scripts/pre-push-unit.sh";
export const WORKFLOW = ".github/workflows/ci.yml";

// The workflow steps that execute plan rows, with the exact lines each must
// run (after joining backslash continuations, collapsing blanks and dropping
// comments). In a bound step, every line that mentions nextest, the binary
// check, a selection variable, one of the step's inputs (its env names) or a
// variable it exports from them (exports: the test-thread stack) must be one
// of these, in this order, so the listed selection is the run selection,
// nothing can narrow a run alone, and no line can reassign an input or
// override an export: an assignment, export, read or unset of one names it.
export const CI_ROW_STEPS = [
  {
    field: "closure_test_rows",
    job: "closure-tests",
    runsOn: "ubuntu-24.04",
    step: "Closure tests (the row's targets, workspace feature set)",
    matrix: "closure_test_matrix",
    env: {
      PACKAGES: "${{ matrix.packages }}",
      TESTS: "${{ matrix.tests }}",
      PROFILE: "${{ matrix.profile }}",
      FILTER: "${{ matrix.filter }}",
      BINARIES: "${{ matrix.binaries }}",
      NO_TESTS: "${{ matrix.no_tests }}",
      MIN_STACK: "${{ matrix.rust_min_stack }}",
      PARTITION: "${{ matrix.partition }}",
    },
    exports: ["RUST_MIN_STACK"],
    lines: [
      "if [[ -z \"${PACKAGES}\" || -z \"${TESTS}\" || -z \"${PROFILE}\" || -z \"${BINARIES}\" || -z \"${NO_TESTS}\" ]]; then",
      "read -r -a test_flags <<<\"${TESTS}\"",
      "filter_args=()",
      "if [[ -n \"${FILTER}\" ]]; then filter_args=(-E \"${FILTER}\"); fi",
      "partition_args=()",
      "if [[ -n \"${PARTITION}\" ]]; then partition_args=(--partition \"${PARTITION}\"); fi",
      "selection=(${PACKAGES} \"${test_flags[@]}\" --profile \"${PROFILE}\" \"${filter_args[@]}\" \"${partition_args[@]}\")",
      "if [[ -n \"${MIN_STACK}\" ]]; then export RUST_MIN_STACK=\"${MIN_STACK}\"; fi",
      "fixture=\"$(scripts/mcp-test-server-fixture --if-needed ${PACKAGES})\"",
      "./scripts/repo-cargo nextest list \"${selection[@]}\" --message-format json >\"${RUNNER_TEMP}/closure-test-list.json\"",
      "node scripts/ci-verify-row-binaries.mjs \"${RUNNER_TEMP}/closure-test-list.json\" \"${partition_args[@]}\" ${BINARIES}",
      "./scripts/repo-cargo nextest run \"${selection[@]}\" --no-tests=\"${NO_TESTS}\" --no-fail-fast --status-level slow --final-status-level slow",
    ],
    targets: null,
    profile: null,
    // The rows run in two waves (long rows first, scripts/ci-cargo-lanes.mjs):
    // the rest wave's job is a copy of the job above, bound the same way.
    twins: [{ job: "closure-tests-rest", matrix: "closure_test_rest_matrix" }],
  },
];

// Every job that executes a spec's rows: the spec's own and its twins.
export function rowStepBindings() {
  return CI_ROW_STEPS.flatMap((spec) => [spec, ...(spec.twins ?? []).map((twin) => ({ ...spec, ...twin }))]);
}

// The lines of a bound step its pins govern: nextest, the binary check, the
// selection variables, the step's inputs and its exports, each as a whole
// word.
function boundLine(spec) {
  const names = [...Object.keys(spec.env), ...spec.exports].join("|");
  return new RegExp(`nextest|ci-verify-row-binaries|\\b(selection|test_flags|filter_args|partition_args|${names})\\b`);
}

// Shell text as logical lines: continuations joined, blanks collapsed,
// comments and empty lines dropped. Not a shell parser: bound text must be
// written in the plain form the pins name.
export function logicalLines(text) {
  const lines = [];
  let pending = "";
  for (const raw of text.split("\n")) {
    const trimmed = raw.trim();
    if (pending === "" && (trimmed === "" || trimmed.startsWith("#"))) continue;
    if (trimmed.endsWith("\\")) {
      pending += `${trimmed.slice(0, -1).trim()} `;
      continue;
    }
    lines.push(`${pending}${trimmed}`.replace(/\s+/g, " ").trim());
    pending = "";
  }
  if (pending !== "") lines.push(pending.trim());
  return lines;
}

// The block of an indented YAML mapping entry or list item, by its exact
// header line, up to the next sibling at the same indentation.
function block(text, header, indent) {
  const lines = text.split("\n");
  const starts = lines.flatMap((line, index) => (line === header ? [index] : []));
  if (starts.length !== 1) return { error: `${JSON.stringify(header.trim())} occurs ${starts.length} times` };
  const sibling = new RegExp(`^ {${indent}}\\S`);
  let end = starts[0] + 1;
  while (end < lines.length && !(sibling.test(lines[end]) || /^\S/.test(lines[end]))) end += 1;
  return { text: lines.slice(starts[0], end).join("\n") };
}

// Binds each CI_ROW_STEPS entry to its job and step in the workflow.
export function bindRowSteps(workflowText) {
  const errors = [];
  for (const spec of rowStepBindings()) {
    const where = `${WORKFLOW} job ${spec.job}`;
    const job = block(workflowText, `  ${spec.job}:`, 2);
    if (job.error) {
      errors.push(`${where}: ${job.error}`);
      continue;
    }
    if (!job.text.includes(`      matrix: \${{ fromJSON(needs.changes.outputs.${spec.matrix}) }}\n`)) {
      errors.push(`${where}: its matrix is not needs.changes.outputs.${spec.matrix}`);
    }
    if (!job.text.includes(`\n    runs-on: ${spec.runsOn}\n`)) errors.push(`${where}: it does not run on ${spec.runsOn}`);
    if (/^\s+NEXTEST_[A-Z0-9_]*:/m.test(job.text)) errors.push(`${where}: sets a NEXTEST_ variable`);
    const step = block(job.text, `      - name: ${spec.step}`, 6);
    if (step.error) {
      errors.push(`${where}: step ${step.error}`);
      continue;
    }
    for (const [name, value] of Object.entries(spec.env)) {
      if (!step.text.includes(`\n          ${name}: ${value}\n`)) errors.push(`${where}: step env ${name} is not ${value}`);
    }
    const run = step.text.split("\n        run: |\n");
    if (run.length !== 2) {
      errors.push(`${where}: step has no single run block`);
      continue;
    }
    const bound = logicalLines(run[1]).filter((line) => boundLine(spec).test(line));
    if (JSON.stringify(bound) !== JSON.stringify(spec.lines)) {
      errors.push(`${where}: the step runs ${JSON.stringify(bound)}, not the bound lines ${JSON.stringify(spec.lines)}`);
    }
  }
  return errors;
}

// The hook's selection, read from scripts/pre-push-unit.sh: the workspace
// build inventory, each lane run over it, and HeadCanonical's own build and
// run. Every nextest command in the script must match one of these shapes.
export function hookModel(text) {
  const lines = logicalLines(text).filter((line) => /nextest (list|run)/.test(line));
  const build = [];
  const headBuild = [];
  const lanes = [];
  const unknown = [];
  for (const line of lines) {
    let match = line.match(/^"\$CARGO" nextest list "\$\{core_workspace\[@\]\}" --list-type binaries-only --message-format json -E '([^']+)' >"\$workspace_binaries_metadata"$/);
    if (match) {
      build.push(match[1]);
      continue;
    }
    match = line.match(/^"\$CARGO" nextest list -p (\S+) --test (\S+) --features (\S+) --profile (\S+) --list-type binaries-only --message-format json >"\$headcanonical_binaries_metadata"$/);
    if (match) {
      headBuild.push({ package: match[1], test: match[2], features: match[3].split(","), profile: match[4] });
      continue;
    }
    match = line.match(/^retry_lane "([^"]+)" "\$[A-Z_]+" "\$CARGO" nextest run --cargo-metadata "\$workspace_cargo_metadata" --binaries-metadata "\$(workspace|headcanonical)_binaries_metadata"(?: --profile (\S+))? -E '([^']+)' --no-tests=fail(?: --no-fail-fast)? --show-progress none --status-level none --final-status-level fail$/);
    if (match) {
      lanes.push({ label: match[1], inventory: match[2], profile: match[3] ?? "default", filter: match[4] });
      continue;
    }
    unknown.push(line);
  }
  if (unknown.length > 0) throw new Error(`${HOOK_SCRIPT} runs nextest in a shape the coverage model does not read: ${unknown.join(" | ")}`);
  if (build.length !== 1) {
    throw new Error(`${HOOK_SCRIPT}: expected one build inventory over Core's members ("\${core_workspace[@]}"), found ${build.length}`);
  }
  if (headBuild.length !== 1) throw new Error(`${HOOK_SCRIPT}: expected one HeadCanonical build, found ${headBuild.length}`);
  const headLanes = lanes.filter((lane) => lane.inventory === "headcanonical");
  if (headLanes.length !== 1) throw new Error(`${HOOK_SCRIPT}: expected one HeadCanonical lane, found ${headLanes.length}`);
  const [head] = headBuild;
  if (headLanes[0].filter !== `binary(${head.test})` || headLanes[0].profile !== head.profile) {
    throw new Error(`${HOOK_SCRIPT}: the HeadCanonical lane does not run exactly its build's ${head.test}`);
  }
  const workspaceLanes = lanes.filter((lane) => lane.inventory === "workspace");
  // e2e-fast: the one workspace lane that runs a single named binary.
  const named = workspaceLanes.filter((lane) => /^binary\([A-Za-z0-9_]+\)$/.test(lane.filter));
  if (named.length !== 1) throw new Error(`${HOOK_SCRIPT}: expected one single-binary workspace lane (e2e-fast), found ${named.length}`);
  return {
    buildFilter: build[0],
    lanes: workspaceLanes,
    headCanonical: head,
    e2eFast: { binary: named[0].filter.slice("binary(".length, -1), profile: named[0].profile, label: named[0].label },
  };
}

const LIB_KINDS = new Set(["lib", "rlib", "dylib", "cdylib", "staticlib"]);

// nextest's binary id of a Cargo target: the package for a lib (or
// proc-macro) test, package::name for an integration test, package::bin/name
// for a bin.
function binaryId(pkg, target) {
  const kind = nextestKind(target);
  if (kind === "lib" || kind === "proc-macro") return pkg.name;
  if (kind === "bin") return `${pkg.name}::bin/${target.name}`;
  return `${pkg.name}::${target.name}`;
}

// nextest binary kind of a Cargo target.
function nextestKind(target) {
  if (target.kind.includes("proc-macro")) return "proc-macro";
  if (target.kind.some((kind) => LIB_KINDS.has(kind))) return "lib";
  if (target.kind.length === 1) return target.kind[0];
  return target.kind.join("+");
}

// The `default-filter` of each nextest profile. nextest's `inherits` does not
// carry a parent's default-filter (measured on 0.9.143; see
// .config/nextest.toml), so a profile without its own selects every binary.
export function profileFilters(text) {
  const filters = new Map();
  const profiles = new Set(["default"]);
  let section = null;
  for (const line of text.split("\n")) {
    const header = line.match(/^\s*(\[\[?)([^\]]+)\]\]?\s*(#.*)?$/);
    if (header) {
      const name = header[2].trim();
      const profile = header[1] === "[" ? name.match(/^profile\.([A-Za-z0-9_-]+)$/) : null;
      section = profile ? profile[1] : null;
      if (section) profiles.add(section);
      continue;
    }
    if (section === null || !/^\s*default-filter\s*=/.test(line)) continue;
    const value = line.match(/^\s*default-filter\s*=\s*'([^']*)'\s*(#.*)?$/);
    if (!value) throw new Error(`unsupported default-filter syntax in profile ${section}: ${line.trim()}`);
    filters.set(section, value[1]);
  }
  return { filters, profiles };
}

export function profileFilter(nextestText, profile) {
  const { filters, profiles } = profileFilters(nextestText);
  if (!profiles.has(profile)) throw new Error(`.config/nextest.toml has no profile ${profile}`);
  return filters.get(profile) ?? "all()";
}

// A binary-level subset of nextest's filterset language: kind(), binary(),
// package(), all(), none(), not/!, and/&, or/|/+, and parentheses, with
// exact (`=name` or plain) name matchers. A test() predicate or any other
// matcher cannot be decided per binary, so it throws instead of guessing.
export function parseFilter(expr) {
  const tokens = [];
  const pattern = /\s*(\(|\)|!|&|\||\+|[A-Za-z_]+\([^()]*\)|[A-Za-z_]+)/y;
  let index = 0;
  const source = expr.trim();
  while (index < source.length) {
    pattern.lastIndex = index;
    const match = pattern.exec(source);
    if (!match) throw new Error(`unsupported filter syntax at ${JSON.stringify(source.slice(index))}`);
    tokens.push(match[1]);
    index = pattern.lastIndex;
    while (index < source.length && /\s/.test(source[index])) index += 1;
  }
  let position = 0;
  const peek = () => tokens[position];
  const next = () => tokens[position++];
  const parseOr = () => {
    let left = parseAnd();
    while (["or", "|", "+"].includes(peek())) {
      next();
      const right = parseAnd();
      const l = left;
      left = (binary) => l(binary) || right(binary);
    }
    return left;
  };
  const parseAnd = () => {
    let left = parseUnary();
    while (["and", "&"].includes(peek())) {
      next();
      const right = parseUnary();
      const l = left;
      left = (binary) => l(binary) && right(binary);
    }
    return left;
  };
  const parseUnary = () => {
    const token = next();
    if (token === "not" || token === "!") {
      const inner = parseUnary();
      return (binary) => !inner(binary);
    }
    if (token === "(") {
      const inner = parseOr();
      if (next() !== ")") throw new Error(`unbalanced parentheses in filter ${JSON.stringify(expr)}`);
      return inner;
    }
    const call = token?.match(/^([A-Za-z_]+)\(([^()]*)\)$/);
    if (!call) throw new Error(`unsupported filter term ${JSON.stringify(token)} in ${JSON.stringify(expr)}`);
    const [, name, rawArg] = call;
    const arg = rawArg.trim();
    if (name === "all" || name === "none") {
      if (arg !== "") throw new Error(`${name}() takes no argument in ${JSON.stringify(expr)}`);
      return () => name === "all";
    }
    if (!["kind", "binary", "package"].includes(name)) {
      throw new Error(`filter predicate ${name}() cannot be decided per binary: ${JSON.stringify(expr)}`);
    }
    const exact = arg.startsWith("=") ? arg.slice(1) : arg;
    if (!/^[A-Za-z0-9_.-]+$/.test(exact)) {
      throw new Error(`unsupported name matcher ${JSON.stringify(arg)} in ${JSON.stringify(expr)}`);
    }
    return (binary) => binary[name] === exact;
  };
  const predicate = parseOr();
  if (position !== tokens.length) throw new Error(`trailing filter tokens in ${JSON.stringify(expr)}`);
  return predicate;
}

// Features a package has with `enabled` requested, following its own feature
// table (`dep:x` enables no feature; `x/y` and `x?/y` name a dependency).
export function featureClosure(pkg, enabled) {
  const result = new Set();
  const stack = [...enabled];
  while (stack.length) {
    const feature = stack.pop();
    if (result.has(feature)) continue;
    if (!Object.hasOwn(pkg.features ?? {}, feature)) {
      throw new Error(`${pkg.name} has no feature ${feature}`);
    }
    result.add(feature);
    for (const entry of pkg.features[feature]) {
      if (entry.startsWith("dep:")) continue;
      const own = entry.includes("/") ? entry.split("/", 1)[0] : entry;
      if (own.endsWith("?")) continue;
      if (Object.hasOwn(pkg.features, own)) stack.push(own);
    }
  }
  return result;
}

// The features each workspace member is compiled with in a whole-workspace
// build: cargo metadata's resolve graph (equal to `cargo tree --workspace`'s
// per-member features on 8e1f4f21).
export function workspaceFeatures(metadata, packages) {
  const nodes = new Map((metadata.resolve?.nodes ?? []).map((node) => [node.id, node]));
  return new Map(packages.map((pkg) => {
    const node = nodes.get(pkg.id);
    if (!node) throw new Error(`cargo metadata has no resolve node for ${pkg.name}`);
    return [pkg.name, new Set(node.features ?? [])];
  }));
}

function sameSet(a, b) {
  return a.size === b.size && [...a].every((item) => b.has(item));
}

// The lane selections of the workspace: Core's (`--workspace` and an
// `--exclude` for every other product's package) and each other product's
// (`-p` each of its packages), with the features each selection compiles its
// members with (`cargo tree`'s roots for exactly that selection).
export function laneSelections(metadata, { products = loadProducts(), selectionFeatures = readSelectionFeatures } = {}) {
  const byProduct = workspaceMembersByProduct(metadata, products);
  const productOf = new Map();
  for (const [product, names] of byProduct) for (const name of names) productOf.set(name, product);
  const selections = new Map();
  for (const [product, names] of byProduct) {
    if (names.length === 0) continue;
    const args = product === CORE ? coreWorkspaceArgs(byProduct) : productPackageArgs(byProduct, product);
    const resolved = selectionFeatures(args);
    const features = new Map(names.map((name) => [name, new Set(resolved.get(name) ?? [])]));
    selections.set(product, { product, args, flags: args.join(" "), packages: [...names], features });
  }
  if (!selections.has(CORE)) throw new Error("the workspace has no Core member");
  return { byProduct, productOf, selections };
}

// Reads a row's package flags the way Cargo does. `--workspace` must come
// with an `--exclude` for exactly every other product's package (Core's
// selection, the hook's own resolution); `-p NAME`, `--features LIST` and
// `--no-default-features` as for a multi-package invocation (a feature must
// name its package unless exactly one package is selected), where `-p` for
// exactly one product's packages, with no feature flag, is that product's
// selection.
function parsePackageFlags(flags, byName, selections) {
  const tokens = flags.split(/\s+/).filter(Boolean);
  if (tokens[0] === "--workspace") {
    const excluded = new Set();
    for (let index = 1; index < tokens.length; index += 2) {
      if (tokens[index] !== "--exclude" || !tokens[index + 1]) {
        throw new Error(`a whole-workspace row takes only --exclude NAME pairs after --workspace: ${JSON.stringify(flags)}`);
      }
      excluded.add(tokens[index + 1]);
    }
    const others = [...selections.productOf].filter(([, product]) => product !== CORE).map(([name]) => name).sort();
    if (!sameSet(excluded, new Set(others))) {
      throw new Error(
        `a whole-workspace row must exclude exactly the other products' packages (${others.join(", ") || "none"}), ` +
          `not ${[...excluded].sort().join(", ") || "none"}: ${JSON.stringify(flags)}`,
      );
    }
    const core = selections.selections.get(CORE);
    return { packages: [...core.packages], enabled: new Map(core.features), selection: CORE };
  }
  const packages = [];
  const requested = [];
  let defaults = true;
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token === "--workspace") {
      throw new Error(`--workspace cannot be combined with other package flags in ${JSON.stringify(flags)}`);
    } else if (token === "-p" || token === "--package") {
      packages.push(tokens[++index]);
    } else if (token === "--features" || token === "-F") {
      requested.push(...tokens[++index].split(",").filter(Boolean));
    } else if (token === "--no-default-features") {
      defaults = false;
    } else {
      throw new Error(`unsupported package flag ${token} in ${JSON.stringify(flags)}`);
    }
  }
  for (const name of packages) {
    if (!byName.has(name)) throw new Error(`row selects unknown package ${name}`);
  }
  if (packages.length > 0 && requested.length === 0 && defaults) {
    const owner = selections.productOf.get(packages[0]);
    const selection = selections.selections.get(owner);
    if (owner !== CORE && selection && sameSet(new Set(packages), new Set(selection.packages))) {
      return { packages: [...packages].sort(), enabled: new Map(selection.features), selection: owner };
    }
  }
  const explicit = new Map(packages.map((name) => [name, new Set(defaults ? ["default"] : [])]));
  for (const feature of requested) {
    const [owner, name] = feature.includes("/") ? feature.split("/", 2) : [null, feature];
    if (owner === null && packages.length !== 1) {
      throw new Error(`unqualified feature ${feature} in a multi-package row: ${JSON.stringify(flags)}`);
    }
    const target = owner ?? packages[0];
    if (!explicit.has(target)) throw new Error(`feature ${feature} names a package the row does not select`);
    explicit.get(target).add(name);
  }
  const enabled = new Map();
  for (const [name, features] of explicit) {
    const pkg = byName.get(name);
    // A package without a `default` feature simply has no default set.
    if (!Object.hasOwn(pkg.features ?? {}, "default")) features.delete("default");
    enabled.set(name, featureClosure(pkg, features));
  }
  return { packages, enabled, selection: null };
}

// The binaries a row runs: Cargo's target selection (with its
// required-features refusal for named targets), then the profile's
// default-filter and the row's own filter, as its bound step runs them.
function rowBinaries(row, spec, byName, nextestText, selections) {
  const { packages, enabled, selection } = parsePackageFlags(row.package_flags, byName, selections);
  const targetTokens = spec.targets ?? (row.test_flags ?? "").split(/\s+/).filter(Boolean);
  const profile = spec.profile ?? row.profile;
  if (!profile) throw new Error(`row ${row.name} names no nextest profile`);
  const profileFilterFn = parseFilter(profileFilter(nextestText, profile));
  const rowFilterFn = row.filter ? parseFilter(row.filter) : () => true;
  const selected = [];
  const errors = [];
  const add = (pkg, target) => selected.push({ pkg, target });
  for (let index = 0; index < targetTokens.length; index += 1) {
    const token = targetTokens[index];
    if (token === "--lib") {
      for (const name of packages) {
        const pkg = byName.get(name);
        for (const target of pkg.targets.filter((candidate) => ["lib", "proc-macro"].includes(nextestKind(candidate)))) {
          add(pkg, target);
        }
      }
    } else if (token === "--bins") {
      for (const name of packages) {
        const pkg = byName.get(name);
        for (const target of pkg.targets.filter((candidate) => candidate.kind.includes("bin"))) {
          if ((target["required-features"] ?? []).every((feature) => enabled.get(name).has(feature))) add(pkg, target);
        }
      }
    } else if (token === "--test") {
      const wanted = targetTokens[++index];
      if (!/^[A-Za-z0-9_-]+$/.test(wanted ?? "")) {
        errors.push(`row ${row.name}: unsupported --test pattern ${wanted}`);
        continue;
      }
      let matched = 0;
      for (const name of packages) {
        const pkg = byName.get(name);
        for (const target of pkg.targets) {
          if (!target.kind.includes("test") || target.name !== wanted) continue;
          matched += 1;
          const missing = (target["required-features"] ?? []).filter((feature) => !enabled.get(name).has(feature));
          if (missing.length > 0) {
            errors.push(`row ${row.name}: ${name} test target ${target.name} requires ${missing.join(",")}; Cargo refuses the row`);
            continue;
          }
          add(pkg, target);
        }
      }
      if (matched === 0) errors.push(`row ${row.name}: no test target named ${wanted} in ${packages.join(" ")}`);
    } else {
      errors.push(`row ${row.name}: unsupported target flag ${token}`);
    }
  }
  const runs = selected.filter(({ pkg, target }) => {
    const binary = { kind: nextestKind(target), binary: target.name, package: pkg.name };
    return profileFilterFn(binary) && rowFilterFn(binary);
  });
  return { packages, enabled, selection, runs, errors, profile };
}

// The hook's selection for the plan's test scope, and the CI rows that cover
// it.
export function checkHookCoverage(
  plan,
  { metadata = readMetadata(), nextestText, workflowText, hookText, products, selectionFeatures } = {},
) {
  const nextest = nextestText ?? readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  const workflow = workflowText ?? readFileSync(resolve(root, WORKFLOW), "utf8");
  const errors = bindRowSteps(workflow);
  const hook = hookModel(hookText ?? readFileSync(resolve(root, HOOK_SCRIPT), "utf8"));
  const packages = workspacePackages(metadata);
  const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
  const selections = laneSelections(metadata, {
    products: products ?? loadProducts(),
    selectionFeatures: selectionFeatures ?? readSelectionFeatures,
  });
  const productOf = (name) => selections.productOf.get(name) ?? CORE;
  // The hook's build: Core's members, with Core's own resolution.
  const unified = selections.selections.get(CORE).features;
  const scope = [...(plan.test_scope ?? [])].sort();
  for (const name of scope) {
    if (!byName.has(name)) errors.push(`test scope names unknown package ${name}`);
  }

  // The lane script's workspace binaries for the scope, keyed
  // package::kind::binary: built by the workspace inventory, run by at least
  // one of its lanes. A lib binary of a lane the plan leaves without a row
  // runs in the unit lanes; every other one needs a row.
  const builtFilter = parseFilter(hook.buildFilter);
  const lanes = hook.lanes.map((lane) => ({
    label: lane.label,
    filter: parseFilter(lane.filter),
    profileFilter: parseFilter(profileFilter(nextest, lane.profile)),
  }));
  const retainedLabels = [...new Set(plan.hook_retained_lanes ?? [])].sort();
  for (const label of retainedLabels) {
    if (!lanes.some((lane) => lane.label === label)) errors.push(`the plan leaves "${label}" without a row, but the lane script has no such workspace lane`);
  }
  const expected = new Map();
  const retained = new Map();
  for (const name of scope.filter((candidate) => byName.has(candidate) && productOf(candidate) === CORE)) {
    const pkg = byName.get(name);
    for (const target of pkg.targets) {
      // `test = false` targets get no test binary; unmet required-features
      // keep a target out of the workspace build.
      if (target.test === false) continue;
      if (!(target["required-features"] ?? []).every((feature) => unified.get(name).has(feature))) continue;
      const binary = { kind: nextestKind(target), binary: target.name, package: name };
      if (!builtFilter(binary)) continue;
      const runs = (candidate) => candidate.filter(binary) && candidate.profileFilter(binary);
      const lane = lanes.find(runs);
      if (!lane) continue;
      const key = `${name}::${binary.kind}::${target.name}`;
      const keeper = binary.kind === "lib"
        ? lanes.find((candidate) => retainedLabels.includes(candidate.label) && runs(candidate))
        : undefined;
      if (keeper) retained.set(key, { lane: keeper.label, package: name });
      else expected.set(key, { lane: lane.label, package: name });
    }
  }
  // The other products' test binaries in scope: every tests/*.rs target the
  // product's own build builds (its selection's features) that the hook's
  // integration lane would select, credited only to a row that selects that
  // product's packages. Their lib tests run in the main unit lane's product
  // rows, as Core's retained lib lane's do.
  const productExpected = new Map();
  for (const name of scope.filter((candidate) => byName.has(candidate) && productOf(candidate) !== CORE)) {
    const product = productOf(name);
    const features = selections.selections.get(product).features.get(name) ?? new Set();
    for (const target of byName.get(name).targets) {
      if (target.test === false || !(target.kind.length === 1 && target.kind[0] === "test")) continue;
      if (!(target["required-features"] ?? []).every((feature) => features.has(feature))) continue;
      const binary = { kind: "test", binary: target.name, package: name };
      const lane = lanes.find((candidate) => candidate.filter(binary) && candidate.profileFilter(binary));
      if (!lane) continue;
      productExpected.set(`${name}::test::${target.name}`, { lane: lane.label, package: name, product });
    }
  }
  const hc = hook.headCanonical;
  const headCanonicalExpected = scope.includes(hc.package);
  if (headCanonicalExpected && !byName.get(hc.package)?.targets.some((target) => target.kind.includes("test") && target.name === hc.test)) {
    errors.push(`HeadCanonical target ${hc.test} is missing from ${hc.package}`);
  }

  // What the planned rows run under the hook's feature set (a Core binary,
  // from a row with Core's selection) or under its product's own selection
  // (a product's binary, from a row with that product's selection).
  const covered = new Map();
  const credits = (selection, key) => selection !== null && selection === productOf(key.split("::", 1)[0]);
  // A partitioned row runs one count partition (`--partition count:K/N`) of
  // its selection. It credits nothing alone: a group of rows with one exact
  // selection credits its binaries only when it holds every partition 1..N
  // exactly once, since together those run every test of the selection.
  const partitionGroups = new Map();
  let headCanonical = null;
  for (const spec of CI_ROW_STEPS) {
    for (const row of plan[spec.field] ?? []) {
      // The job runs every row where it runs (no row routes elsewhere yet).
      if (row.runner != null && row.runner !== spec.runsOn) {
        errors.push(`row ${row.name} names runner ${row.runner}, but job ${spec.job} runs on ${spec.runsOn}`);
      }
      let read;
      try {
        read = rowBinaries(row, spec, byName, nextest, selections);
      } catch (error) {
        errors.push(`row ${row.name} (${spec.job}): ${error.message}`);
        continue;
      }
      errors.push(...read.errors);
      // The row's own list of binaries is what the job checks nextest
      // against; it must be exactly what the row runs.
      const runIds = [...new Set(read.runs.map(({ pkg, target }) => binaryId(pkg, target)))].sort();
      const listed = [...new Set(row.binaries ?? [])].sort();
      const unlisted = runIds.filter((id) => !listed.includes(id));
      const unrun = listed.filter((id) => !runIds.includes(id));
      if (unlisted.length > 0 || unrun.length > 0) {
        errors.push(`row ${row.name}: runs binaries it does not list (${unlisted.join(", ") || "none"}) or lists binaries it does not run (${unrun.join(", ") || "none"})`);
      }
      const keys = read.runs.map(({ pkg, target }) => `${pkg.name}::${nextestKind(target)}::${target.name}`);
      if (row.partition != null) {
        const part = /^count:([1-9][0-9]*)\/([1-9][0-9]*)$/.exec(String(row.partition));
        if (!part || Number(part[1]) > Number(part[2]) || Number(part[2]) < 2) {
          errors.push(`row ${row.name}: unsupported partition ${JSON.stringify(row.partition)}; expected count:K/N with 1 <= K <= N, N >= 2`);
          continue;
        }
        const selection = [spec.job, row.package_flags, row.test_flags, row.profile, row.filter ?? "", listed.join(" ")].join("\n");
        const group = partitionGroups.get(selection) ?? { counts: new Set(), indices: [], names: [], keys, selection: read.selection, spec };
        group.counts.add(Number(part[2]));
        group.indices.push(Number(part[1]));
        group.names.push(row.name);
        partitionGroups.set(selection, group);
        continue;
      }
      for (const key of keys) {
        if (credits(read.selection, key) && !covered.has(key)) {
          covered.set(key, `${spec.job}: ${row.name}`);
        }
      }
      const runsHeadCanonical = read.runs.some(({ pkg, target }) => pkg.name === hc.package && target.name === hc.test);
      if (runsHeadCanonical && read.packages.length === 1 && read.packages[0] === hc.package) {
        const hookProfile = parseFilter(profileFilter(nextest, hc.profile));
        const scoped = featureClosure(byName.get(hc.package), new Set(["default", ...hc.features]));
        const exact = sameSet(read.enabled.get(hc.package), scoped)
          && hookProfile({ kind: "test", binary: hc.test, package: hc.package })
          && read.runs.length === 1;
        if (exact) headCanonical = `${spec.job}: ${row.name}`;
      }
    }
  }
  for (const group of partitionGroups.values()) {
    const [count] = [...group.counts];
    const indices = [...group.indices].sort((a, b) => a - b);
    const complete = group.counts.size === 1
      && indices.length === count
      && indices.every((index, position) => index === position + 1);
    if (!complete) {
      errors.push(`partitioned rows ${group.names.join(", ")} hold partitions ${indices.join(",")} of count ${[...group.counts].join("/")}; every partition 1..N of one selection is required`);
      continue;
    }
    for (const key of group.keys) {
      if (credits(group.selection, key) && !covered.has(key)) {
        covered.set(key, `${group.spec.job}: ${group.names.join(", ")}`);
      }
    }
  }

  const headKey = `HeadCanonical::${hc.package}::${hc.test}`;
  // A binary no hosted runner can judge (HOSTED_RUNNER_EXCLUDED_BINARIES in
  // scripts/ci-cargo-lanes.mjs) is named by the plan instead of run by a row.
  const excludedKeys = new Set((plan.hosted_runner_excluded ?? []).map(({ binary }) => {
    const [name, test] = String(binary).split("::");
    return `${name}::test::${test}`;
  }));
  for (const key of excludedKeys) {
    if (covered.has(key)) errors.push(`${key} is excluded from hosted runners but a row runs it (${covered.get(key)})`);
  }
  const missing = [...expected.keys(), ...productExpected.keys()]
    .filter((key) => !covered.has(key) && !excludedKeys.has(key))
    .sort();
  if (headCanonicalExpected && !headCanonical) missing.push(headKey);
  const byLane = {};
  for (const { lane } of [...expected.values(), ...retained.values()]) byLane[lane] = (byLane[lane] ?? 0) + 1;
  if (headCanonicalExpected) byLane.HeadCanonical = 1;
  return {
    scope_packages: scope.length,
    hook_binaries: expected.size + retained.size + (headCanonicalExpected ? 1 : 0),
    hook_retained: retained.size,
    product_binaries: productExpected.size,
    by_lane: byLane,
    missing,
    errors,
    map: Object.fromEntries([
      ...[...expected.keys(), ...retained.keys(), ...productExpected.keys()].sort().map((key) => [
        key,
        covered.get(key)
          ?? (retained.has(key) ? `unit lanes (main, nightly): ${retained.get(key).lane}` : null)
          ?? (excludedKeys.has(key) ? "no hosted runner (hosted_runner_excluded)" : null),
      ]),
      ...(headCanonicalExpected ? [[headKey, headCanonical]] : []),
    ]),
  };
}

function main() {
  const args = process.argv.slice(2);
  let planPath = "-";
  let printMap = false;
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--plan") planPath = args[++index] ?? "-";
    else if (args[index] === "--map") printMap = true;
    else throw new Error(`unknown argument: ${args[index]}`);
  }
  const plan = JSON.parse(readFileSync(planPath === "-" ? 0 : planPath, "utf8"));
  if (plan.event === "pull_request" && plan.deferred_to_push) {
    throw new Error("a pull-request plan leaves its closure-tests rows to the push; plan the same change with --event push");
  }
  const report = checkHookCoverage(plan);
  const summary = {
    scope_packages: report.scope_packages,
    hook_binaries: report.hook_binaries,
    hook_retained: report.hook_retained,
    product_binaries: report.product_binaries,
    by_lane: report.by_lane,
    missing: report.missing,
    errors: report.errors,
  };
  process.stdout.write(`${JSON.stringify(printMap ? { ...summary, map: report.map } : summary, null, 2)}\n`);
  if (report.missing.length > 0 || report.errors.length > 0) process.exit(1);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`ci-hook-coverage: ${error?.message ?? error}\n`);
    process.exit(2);
  }
}
