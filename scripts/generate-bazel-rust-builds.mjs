#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { testSourcePaths as sharedTestSourcePaths } from "./rust-test-selector.mjs";

const checkOnly = process.argv.includes("--check");

const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
}).trim();
const metadata = JSON.parse(
  execFileSync("./scripts/repo-cargo", ["metadata", "--format-version=1"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  }),
);

const workspaceMembers = new Set(metadata.workspace_members);
const localPackages = new Map(
  metadata.packages
    .filter((pkg) => pkg.source === null && workspaceMembers.has(pkg.id))
    .map((pkg) => [pkg.id, pkg]),
);
const byName = new Map(
  [...localPackages.values()].map((pkg) => [pkg.name, pkg]),
);
const externalByName = new Map();
for (const pkg of metadata.packages.filter((pkg) => pkg.source !== null)) {
  if (!externalByName.has(pkg.name)) externalByName.set(pkg.name, []);
  externalByName.get(pkg.name).push(pkg);
}
const packageDir = (pkg) => dirname(pkg.manifest_path);
const packageKey = (pkg) => {
  const dir = relative(root, packageDir(pkg));
  return dir.includes("/") || dir !== pkg.name ? dir : pkg.name;
};
const packageLabel = (pkg) => `//${relative(root, packageDir(pkg))}:${crateName(pkg.name)}`;
const crateName = (name) => name.replaceAll("-", "_");
const q = (value) => JSON.stringify(value);

function defaultFeatures(pkg) {
  return pkg.features?.default ?? [];
}

const activeFeatures = new Map(
  [...localPackages.values()].map((pkg) => [pkg.id, new Set(defaultFeatures(pkg))]),
);
const requiredTargetFeatures = new Map(
  [...localPackages.values()].map((pkg) => [pkg.id, new Set()]),
);
const resolvedFeatures = new Map(
  (metadata.resolve?.nodes ?? []).map((node) => [node.id, new Set(node.features ?? [])]),
);

function addFeature(pkg, feature) {
  if (!feature || feature.startsWith("dep:")) return false;
  const features = activeFeatures.get(pkg.id);
  const previousSize = features.size;
  features.add(feature);
  return features.size !== previousSize;
}

function localPackageForDependency(pkg, depName) {
  const dep = pkg.dependencies.find((candidate) => candidate.name === depName && candidate.source === null);
  return dep ? byName.get(dep.name) : null;
}

for (const pkg of localPackages.values()) {
  for (const target of pkg.targets) {
    if (target.kind.includes("bench") || target.kind.includes("example")) continue;
    for (const feature of target["required-features"] ?? []) {
      requiredTargetFeatures.get(pkg.id).add(feature);
      addFeature(pkg, feature);
    }
  }
}

let changed = true;
while (changed) {
  changed = false;
  for (const pkg of localPackages.values()) {
    for (const dep of pkg.dependencies) {
      if (dep.source !== null) continue;
      const local = byName.get(dep.name);
      if (!local) continue;
      for (const feature of dep.features ?? []) {
        changed = addFeature(local, feature) || changed;
      }
      if (dep.uses_default_features) {
        for (const feature of defaultFeatures(local)) {
          changed = addFeature(local, feature) || changed;
        }
      }
    }

    for (const feature of [...activeFeatures.get(pkg.id)]) {
      for (const expansion of pkg.features?.[feature] ?? []) {
        const [depName, depFeature] = expansion.split("/", 2);
        const local = depFeature ? localPackageForDependency(pkg, depName) : null;
        if (local) {
          changed = addFeature(local, depFeature) || changed;
        } else {
          changed = addFeature(pkg, expansion) || changed;
        }
      }
    }
  }
}

function targetRule(target) {
  if (target.kind.includes("proc-macro")) return "rust_proc_macro";
  if (target.kind.includes("test")) return "rust_test";
  if (target.kind.includes("bin")) return "rust_binary";
  if (target.kind.includes("lib") || target.kind.includes("rlib") || target.kind.includes("cdylib")) return "rust_library";
  return null;
}

function localDependencyLabel(consumerKey, depPackage) {
  return packageLabel(depPackage);
}

function optionalDependencyNamesEnabledByFeatures(pkg, features) {
  if (features === null || features === undefined) return null;

  const optionalNames = new Set(
    pkg.dependencies
      .filter((dep) => dep.optional)
      .map((dep) => dep.name),
  );
  const enabled = new Set();
  const seenFeatures = new Set();

  const visit = (feature) => {
    if (!feature) return;
    if (feature.startsWith("dep:")) {
      const depName = feature.slice(4);
      if (optionalNames.has(depName)) enabled.add(depName);
      return;
    }

    const slashIndex = feature.indexOf("/");
    if (slashIndex !== -1) {
      const depName = feature.slice(0, slashIndex);
      if (optionalNames.has(depName)) enabled.add(depName);
      return;
    }

    if (optionalNames.has(feature)) enabled.add(feature);
    if (seenFeatures.has(feature)) return;
    seenFeatures.add(feature);
    for (const expansion of pkg.features?.[feature] ?? []) {
      visit(expansion);
    }
  };

  for (const feature of features) {
    visit(feature);
  }
  return enabled;
}

function localDeps(
  pkg,
  procMacroOnly,
  includeDev = false,
  consumerKey = packageKey(pkg),
  enabledFeatures = null,
) {
  const enabledOptionalDeps = optionalDependencyNamesEnabledByFeatures(pkg, enabledFeatures);
  const labels = [];
  for (const dep of pkg.dependencies) {
    if (!includeDev && dep.kind === "dev") continue;
    if (dep.optional && enabledOptionalDeps && !enabledOptionalDeps.has(dep.name)) continue;
    if (dep.source !== null) continue;
    const local = byName.get(dep.name);
    if (!local) continue;
    const target = local.targets.find((candidate) =>
      candidate.kind.includes("proc-macro") || candidate.kind.includes("lib")
    );
    const isProc = target?.kind.includes("proc-macro") ?? false;
    if (isProc === procMacroOnly) labels.push(localDependencyLabel(consumerKey, local));
  }
  return [...new Set(labels)].sort();
}

function crateFeaturesFor(key, pkg, extra = []) {
  const features = new Set(activeFeatures.get(pkg.id));
  if (key === "meerkat-mcp-server") {
    features.add("mob");
  }
  if (key === "meerkat-schedule") {
    features.delete("machine-schema-exports");
  }
  for (const feature of extra) {
    features.add(feature);
  }
  return [...features].sort();
}

function publicCoreCrateFeatures(key, pkg) {
  const features = new Set(crateFeaturesFor(key, pkg));
  if (key === "meerkat-core") {
    features.delete("standalone-agent-builder");
    features.delete("__meerkat-facade-agent-factory-build");
  }
  return [...features].sort();
}

function externalCrateLabel(dep) {
  const candidates = externalByName.get(dep.name) ?? [];
  let pkg = candidates.length === 1 ? candidates[0] : null;
  if (!pkg && dep.req?.startsWith("^")) {
    const requested = dep.req.slice(1).split(".");
    pkg = candidates.find((candidate) => {
      const version = candidate.version.split("+", 1)[0].split(".");
      if (requested[0] === "0" && requested[1]) {
        return version[0] === requested[0] && version[1] === requested[1];
      }
      return version[0] === requested[0];
    }) ?? null;
  }
  if (!pkg) return null;
  return `@crates//:${pkg.name}-${pkg.version}`;
}

function optionalExternalDeps(pkg) {
  const labels = new Set();
  const features = requiredTargetFeatures.get(pkg.id) ?? new Set();
  const alreadyResolved = resolvedFeatures.get(pkg.id) ?? new Set();
  for (const feature of features) {
    if (alreadyResolved.has(feature)) continue;
    for (const expansion of pkg.features?.[feature] ?? []) {
      const depName = expansion.startsWith("dep:") ? expansion.slice(4) : null;
      if (!depName) continue;
      const dep = pkg.dependencies.find((candidate) => candidate.name === depName && candidate.source !== null);
      const label = dep ? externalCrateLabel(dep) : null;
      if (label) labels.add(label);
    }
  }
  return [...labels].sort();
}

function listExpr(values, indent = 8) {
  if (values.length === 0) return "[]";
  const pad = " ".repeat(indent);
  return `[\n${values.map((v) => `${pad}${q(v)},`).join("\n")}\n${" ".repeat(indent - 4)}]`;
}

function rustTargetVisibility(key) {
  if (key === "meerkat-agent-build-authority") {
    return listExpr([
      "//:__pkg__",
      "//meerkat-core:__pkg__",
      "//meerkat-mob:__pkg__",
      "//meerkat-session:__pkg__",
      "//meerkat:__pkg__",
    ]);
  }
  return `["//visibility:public"]`;
}

function localNormalDependencyPackages(pkg) {
  return pkg.dependencies
    .filter((dep) => dep.source === null && dep.kind !== "dev")
    .map((dep) => byName.get(dep.name))
    .filter(Boolean);
}

function hasRustLibraryTarget(pkg) {
  return pkg.targets.some((target) =>
    target.kind.includes("lib") || target.kind.includes("rlib") || target.kind.includes("cdylib")
  );
}

const agentFactoryGraphKeys = new Set();
const agentFactoryStack = byName.has("meerkat") ? [byName.get("meerkat")] : [];
while (agentFactoryStack.length) {
  const pkg = agentFactoryStack.pop();
  const key = packageKey(pkg);
  if (agentFactoryGraphKeys.has(key)) continue;
  agentFactoryGraphKeys.add(key);
  for (const dep of localNormalDependencyPackages(pkg)) {
    agentFactoryStack.push(dep);
  }
}

function reverseDependencyKeys(includeDev) {
  const reverse = new Map(
    [...localPackages.values()].map((pkg) => [packageKey(pkg), new Set()]),
  );
  for (const pkg of localPackages.values()) {
    const key = packageKey(pkg);
    for (const dep of pkg.dependencies) {
      if (dep.source !== null) continue;
      if (!includeDev && dep.kind === "dev") continue;
      const local = byName.get(dep.name);
      if (!local) continue;
      reverse.get(packageKey(local))?.add(key);
    }
  }
  return reverse;
}

const localNormalReverseDependencyKeys = reverseDependencyKeys(false);
const publicDownstreamFixtureKeys = new Set([
  "test-fixtures/surface-build-fixtures",
]);
const agentFactoryConsumerKeys = new Set();
const agentFactoryConsumerStack = ["meerkat"];
while (agentFactoryConsumerStack.length) {
  const key = agentFactoryConsumerStack.pop();
  if (agentFactoryConsumerKeys.has(key)) continue;
  agentFactoryConsumerKeys.add(key);
  for (const consumerKey of localNormalReverseDependencyKeys.get(key) ?? []) {
    if (publicDownstreamFixtureKeys.has(consumerKey)) continue;
    agentFactoryConsumerStack.push(consumerKey);
  }
}

const agentFactoryDevTestConsumerKeys = new Set();
for (const pkg of localPackages.values()) {
  const key = packageKey(pkg);
  for (const dep of pkg.dependencies) {
    if (dep.source !== null || dep.kind !== "dev") continue;
    const local = byName.get(dep.name);
    if (local && agentFactoryConsumerKeys.has(packageKey(local))) {
      agentFactoryDevTestConsumerKeys.add(key);
    }
  }
}

const agentFactoryTestConsumerKeys = new Set([
  ...agentFactoryConsumerKeys,
  ...agentFactoryDevTestConsumerKeys,
]);

const agentFactoryConsumerDependencyKeys = new Set();
const agentFactoryConsumerDependencyStack = [];
for (const pkg of localPackages.values()) {
  if (!agentFactoryConsumerKeys.has(packageKey(pkg))) continue;
  agentFactoryConsumerDependencyStack.push(...localNormalDependencyPackages(pkg));
}
while (agentFactoryConsumerDependencyStack.length) {
  const pkg = agentFactoryConsumerDependencyStack.pop();
  const key = packageKey(pkg);
  if (agentFactoryConsumerDependencyKeys.has(key)) continue;
  agentFactoryConsumerDependencyKeys.add(key);
  agentFactoryConsumerDependencyStack.push(...localNormalDependencyPackages(pkg));
}

const agentFactoryVariantKeys = new Set([
  ...agentFactoryGraphKeys,
  ...agentFactoryConsumerDependencyKeys,
  ...agentFactoryDevTestConsumerKeys,
]);

function shouldGenerateAgentFactoryVariantForKey(key) {
  return key !== "meerkat"
    && !agentFactoryConsumerKeys.has(key)
    && agentFactoryVariantKeys.has(key);
}

function agentFactoryVariantName(pkg) {
  return `${crateName(pkg.name)}_agent_factory_build`;
}

function agentFactoryActualVariantLabel(pkg) {
  return `//${relative(root, packageDir(pkg))}:${agentFactoryVariantName(pkg)}`;
}

function agentFactoryFacadeVariantAliasLabel(pkg) {
  return `//meerkat:${agentFactoryVariantName(pkg)}`;
}

const agentFactoryVariantPackages = [...localPackages.values()]
  .filter((pkg) =>
    shouldGenerateAgentFactoryVariantForKey(packageKey(pkg))
    && hasRustLibraryTarget(pkg)
  );

const agentFactoryVariantByPublicLabel = new Map(
  agentFactoryVariantPackages
    .map((pkg) => [packageLabel(pkg), agentFactoryFacadeVariantAliasLabel(pkg)]),
);

const agentFactoryActualVariantVisibility = listExpr(["//meerkat:__pkg__"]);

const agentFactoryFacadeVariantAliasVisibility = listExpr(
  [...new Set([
    ...agentFactoryGraphKeys,
    ...agentFactoryConsumerKeys,
    ...agentFactoryConsumerDependencyKeys,
    ...agentFactoryTestConsumerKeys,
  ])]
    .filter((key) => key !== ".")
    .map((key) => `//${key}:__pkg__`)
    .sort(),
);

function rewriteAgentFactoryDeps(deps) {
  return deps.map((dep) => agentFactoryVariantByPublicLabel.get(dep) ?? dep);
}

function shouldRewriteAgentFactoryDepsFor(key, testOnlyDeps = false) {
  if (publicDownstreamFixtureKeys.has(key)) return false;
  return agentFactoryConsumerKeys.has(key)
    || (testOnlyDeps && agentFactoryTestConsumerKeys.has(key));
}

const nativeE2eSystemTests = [
  {
    packageKey: "meerkat-cli",
    cargoTestTarget: "live_smoke_cli",
    name: "e2e_system_cli_capabilities_and_config_bazel_test",
    testName: "e2e_scenario_28_cli_capabilities_and_config",
  },
  {
    packageKey: "meerkat-cli",
    cargoTestTarget: "system_cli_init",
    name: "e2e_system_cli_init_snapshot_bazel_test",
    testName: "integration_real_rkat_init_snapshot",
  },
  {
    packageKey: "meerkat-cli",
    cargoTestTarget: "system_cli_resume",
    name: "e2e_system_cli_resume_tools_bazel_test",
    testName: "integration_real_cli_resume_tools",
  },
  {
    packageKey: "meerkat-cli",
    cargoTestTarget: "cli_mobpack_live_smoke",
    name: "e2e_system_cli_mobpack_pack_inspect_validate_bazel_test",
    testName: "e2e_smoke_mobpack_pack_inspect_validate",
  },
  {
    packageKey: "meerkat-cli",
    cargoTestTarget: "cli_mobpack_live_smoke",
    name: "e2e_system_cli_wasm_surface_gate_bazel_test",
    testName: "e2e_smoke_wasm_surface_gate",
  },
  {
    packageKey: "meerkat-cli",
    cargoTestTarget: "cli_mobpack_live_smoke",
    name: "e2e_system_cli_wasm_forbidden_capability_bazel_test",
    testName: "e2e_smoke_wasm_forbidden_capability_rejected",
  },
  {
    packageKey: "meerkat-rest",
    cargoTestTarget: "system_rest_resume",
    name: "e2e_system_rest_resume_metadata_bazel_test",
    testName: "integration_real_rest_resume_metadata",
  },
  {
    packageKey: "tests/integration",
    cargoTestTarget: "system_shared_realm",
    name: "e2e_system_sqlite_shared_realm_rpc_rest_rpc_bazel_test",
    testName: "rpc_rest_rpc_default_sqlite_shared_realm_roundtrip",
    data: ["//meerkat-rpc:rkat_rpc_bin", "//meerkat-rest:rkat_rest_bin"],
    env: [
      `        "RKAT_TEST_BIN_RKAT_RPC": "$(rootpath //meerkat-rpc:rkat_rpc_bin)",`,
      `        "RKAT_TEST_BIN_RKAT_REST": "$(rootpath //meerkat-rest:rkat_rest_bin)",`,
    ],
  },
  {
    packageKey: "tests/integration",
    cargoTestTarget: "system_shared_realm",
    name: "e2e_system_sqlite_shared_realm_cli_rpc_cli_bazel_test",
    testName: "cli_rpc_cli_default_sqlite_shared_realm_roundtrip",
    data: ["//meerkat-cli:rkat", "//meerkat-rpc:rkat_rpc_bin"],
    env: [
      `        "RKAT_TEST_BIN_RKAT": "$(rootpath //meerkat-cli:rkat)",`,
      `        "RKAT_TEST_BIN_RKAT_RPC": "$(rootpath //meerkat-rpc:rkat_rpc_bin)",`,
    ],
  },
  {
    packageKey: "tests/integration",
    cargoTestTarget: "system_shared_realm",
    name: "e2e_system_sqlite_shared_realm_cli_rest_cli_bazel_test",
    testName: "cli_rest_cli_default_sqlite_shared_realm_roundtrip",
    data: ["//meerkat-cli:rkat", "//meerkat-rest:rkat_rest_bin"],
    env: [
      `        "RKAT_TEST_BIN_RKAT": "$(rootpath //meerkat-cli:rkat)",`,
      `        "RKAT_TEST_BIN_RKAT_REST": "$(rootpath //meerkat-rest:rkat_rest_bin)",`,
    ],
  },
];

function nativeE2eSystemSpecsFor(pkg, target) {
  const key = packageKey(pkg);
  return nativeE2eSystemTests.filter((spec) =>
    spec.packageKey === key && spec.cargoTestTarget === target.name
  );
}

let staleFileCount = 0;

function writeGenerated(path, contents) {
  contents = contents.replace(/\n+$/u, "") + "\n";
  if (!checkOnly) {
    writeFileSync(path, contents);
    return;
  }
  const existing = existsSync(path) ? readFileSync(path, "utf8") : null;
  if (existing !== contents) {
    staleFileCount += 1;
    console.error(`stale generated Bazel file: ${relative(root, path)}`);
  }
}

function ensureGeneratedBlock(path, marker, block) {
  block = block.replace(/\n+$/u, "") + "\n";
  const existing = existsSync(path) ? readFileSync(path, "utf8") : "";
  if (existing.includes(marker)) return;
  if (checkOnly) {
    staleFileCount += 1;
    console.error(`stale generated Bazel file: ${relative(root, path)}`);
    return;
  }
  const separator = existing.endsWith("\n") ? "\n" : "\n\n";
  writeFileSync(path, `${existing}${separator}${block}`);
}

function needsWorkspaceRunfiles(target) {
  const source = readFileSync(target.src_path, "utf8");
  return [
    "workspace_root",
    "rev-parse",
    ".github/",
    "test-fixtures",
    "scan_for_manual_input_schema_literals",
    "meerkat-runtime/",
    "meerkat-machine-schema/",
    "meerkat-core/",
    "target-codex",
  ].some((needle) => source.includes(needle));
}

function needsPackageRunfiles(target) {
  const source = readFileSync(target.src_path, "utf8");
  return [
    "CARGO_MANIFEST_DIR",
    "WORKSPACE_ROOT",
    "workspace_root",
    "rev-parse",
    "read_to_string",
    "read_dir",
    "tokio::fs::read",
    "std::fs::read",
    "fs::read",
    "SKILL.md",
    "AGENTS.md",
    "Cargo.toml",
    "test-fixtures",
    ".github/",
    "scripts/",
    "docs/",
    "target-codex",
    "walk_rust_sources",
    "scan_for_manual_input_schema_literals",
  ].some((needle) => source.includes(needle));
}

function workspaceDataLabels(target) {
  const source = readFileSync(target.src_path, "utf8");
  const labels = new Set();
  if (source.includes("workspace_root") || source.includes("rev-parse")) {
    labels.add("//:workspace_metadata");
    labels.add("//:workspace_cargo_manifests");
  }
  if (source.includes(".github/")) {
    labels.add("//:github_workflows");
  }
  if (source.includes("scripts/")) {
    labels.add("//:repo_governance_files");
  }
  if (source.includes("tools/buildbuddy/")) {
    labels.add("//tools/buildbuddy:lane_scripts");
  }
  if (source.includes("test-fixtures")) {
    labels.add("//:test_fixtures");
    labels.add("//test-fixtures/machine-dsl-tests:package_runfiles");
    labels.add("//test-fixtures/mcp-test-server:package_runfiles");
    labels.add("//test-fixtures/surface-build-fixtures:package_runfiles");
  }
  if (target.name === "protocol_codegen_drift") {
    for (const label of packageRunfileLabels) labels.add(label);
  }
  if (source.includes("scan_for_manual_input_schema_literals")) {
    labels.add("//:workspace_rust_sources");
  }
  for (const pkg of localPackages.values()) {
    const dir = relative(root, packageDir(pkg));
    if (!dir) continue;
    const quotedDir = source.includes(q(dir)) || source.includes(`'${dir}'`) || source.includes(`\`${dir}\``);
    const pathLikeDir =
      source.includes(`${dir}/`) ||
      source.includes(`./${dir}`) ||
      source.includes(`join(${q(dir)})`);
    if (source.includes("Cargo.toml") && (quotedDir || pathLikeDir)) {
      labels.add(`//${dir}:cargo_manifest`);
    }
    if (pathLikeDir) {
      labels.add(`//${dir}:package_runfiles`);
    }
  }
  return [...labels].sort();
}

function rustSourceFiles(packageRoot, includeTests) {
  const roots = [resolve(packageRoot, "src")];
  if (includeTests) roots.push(resolve(packageRoot, "tests"));
  const files = [];
  for (const start of roots) {
    try {
      const stack = [start];
      while (stack.length) {
        const current = stack.pop();
        const stat = statSync(current);
        if (stat.isDirectory()) {
          for (const entry of readdirSync(current)) stack.push(resolve(current, entry));
        } else if (current.endsWith(".rs")) {
          files.push(current);
        }
      }
    } catch {
      // Not every package has every source root.
    }
  }
  return files.sort();
}

function testSourcePaths(target, pkg, packageRoot) {
  return [...sharedTestSourcePaths(target, pkg)]
    .map((path) => relative(packageRoot, resolve(root, path)).replaceAll("\\", "/"))
    .sort();
}

function compileData(target, packageRoot, includeTests) {
  const paths = new Set(["Cargo.toml"]);
  const labels = new Set();
  const includeRe = /\binclude_(?:str|bytes)!\(\s*"([^"]+)"\s*\)|\binclude!\(\s*"([^"]+)"\s*\)/g;
  const sourceFiles = new Set([target.src_path, ...rustSourceFiles(packageRoot, includeTests)]);
  const embeddedHelpSkillRoots = [
    resolve(root, ".claude/skills/meerkat-cli-reference"),
    resolve(root, ".claude/skills/meerkat-platform"),
  ];
  for (const sourceFile of sourceFiles) {
    const source = readFileSync(sourceFile, "utf8");
    for (const match of source.matchAll(includeRe)) {
      const includePath = match[1] ?? match[2];
      if (!includePath) continue;
      const absolute = resolve(dirname(sourceFile), includePath);
      if (absolute.startsWith(`${packageRoot}/`)) {
        const rel = relative(packageRoot, absolute);
        if (rel && !rel.startsWith("..")) paths.add(rel);
      } else if (
        embeddedHelpSkillRoots.some(
          (skillRoot) => absolute === skillRoot || absolute.startsWith(`${skillRoot}/`),
        )
      ) {
        labels.add("//:meerkat_platform_skill_files");
      }
    }
    if (source.includes("../../test-fixtures/live_smoke/support.rs")) {
      labels.add("//:live_smoke_support");
    }
  }
  return {
    labels: [...labels].sort(),
    paths: [...paths].sort(),
  };
}

function testTags(pkg, target) {
  const haystack = [
    packageKey(pkg),
    target.name,
    relative(root, target.src_path),
    ...(target["required-features"] ?? []),
  ].join(" ").toLowerCase();
  const tags = [];
  for (const tag of ["e2e", "system", "live", "integration", "trybuild", "snapshot", "fixture", "slow"]) {
    if (haystack.includes(tag)) tags.push(tag);
  }
  if (target["required-features"]?.length) tags.push("required-feature");
  if (!tags.length) tags.push("fast");
  if (packageKey(pkg) === "meerkat" && target.name === "agent_builder_policy_canary") {
    tags.push("unit");
  }
  return [...new Set(tags)].sort();
}

const packageRunfileLabels = [...localPackages.values()]
  .map((pkg) => relative(root, packageDir(pkg)))
  .filter((dir) => dir !== "")
  .sort()
  .map((dir) => `//${dir}:package_runfiles`);
packageRunfileLabels.sort();

function writeRootBuild(fastTestLabels, e2eSystemTestLabels, surfaceFeatureMatrixLabels) {
  const lines = [
    `load("@rules_shell//shell:sh_test.bzl", "sh_test")`,
    ``,
    `# Root package marker for Bazel module extension labels.`,
    ``,
    `filegroup(`,
    `    name = "workspace_metadata",`,
    `    srcs = [`,
    `        "Cargo.lock",`,
    `        "Cargo.toml",`,
    `        "MODULE.bazel",`,
    `        "MODULE.bazel.lock",`,
    `    ],`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "workspace_cargo_manifests",`,
    `    srcs = glob(["Cargo.toml", "*/Cargo.toml", "*/*/Cargo.toml"], allow_empty = True),`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "github_workflows",`,
    `    srcs = glob([".github/workflows/*"], allow_empty = True),`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "workspace_rust_sources",`,
    `    srcs = glob(`,
    `        ["**/*.rs"],`,
    `        exclude = [`,
    `            ".git/**",`,
    `            "bazel-*",`,
    `            "bazel-*/**",`,
    `            "target/**",`,
    `        ],`,
    `        allow_empty = True,`,
    `    ),`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "live_smoke_support",`,
    `    srcs = ["test-fixtures/live_smoke/support.rs"],`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "test_fixtures",`,
    `    srcs = glob(`,
    `        ["test-fixtures/**"],`,
    `        exclude = [`,
    `            "test-fixtures/**/BUILD",`,
    `            "test-fixtures/**/BUILD.bazel",`,
    `        ],`,
    `        allow_empty = True,`,
    `    ),`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "meerkat_platform_skill_files",`,
    `    srcs = [`,
    `        ".claude/skills/meerkat-cli-reference/SKILL.md",`,
    `        ".claude/skills/meerkat-platform/SKILL.md",`,
    `        ".claude/skills/meerkat-platform/references/api_reference.md",`,
    `        ".claude/skills/meerkat-platform/references/migration_0_5.md",`,
    `        ".claude/skills/meerkat-platform/references/mobs.md",`,
    `    ],`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "repo_governance_files",`,
    `    srcs = glob(["Makefile", "scripts/*"], allow_empty = True),`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `filegroup(`,
    `    name = "workspace_runfiles",`,
    `    srcs = glob(`,
    `        [`,
    `            ".github/workflows/*",`,
    `            "**/*",`,
    `        ],`,
    `        exclude = [`,
    `            ".git/**",`,
    `            "bazel-*",`,
    `            "bazel-*/**",`,
    `            "**/node_modules/**",`,
    `            "target/**",`,
    `            "secrets.env",`,
    `            "**/secrets.env",`,
    `            "*credential*.json",`,
    `            "**/*credential*.json",`,
    `            "*credentials*.json",`,
    `            "**/*credentials*.json",`,
    `            "*service-account*.json",`,
    `            "**/*service-account*.json",`,
    `            "*service_account*.json",`,
    `            "**/*service_account*.json",`,
    `            "**/BUILD",`,
    `            "**/BUILD.bazel",`,
    `        ],`,
    `        allow_empty = True,`,
    `    ) + ${listExpr(packageRunfileLabels, 8)},`,
    `    visibility = ["//visibility:public"],`,
    `)`,
    ``,
    `sh_test(`,
    `    name = "audit_effect_authority_test",`,
    `    srcs = ["scripts/audit-effect-authority.sh"],`,
    `    data = [":workspace_runfiles"],`,
    `    size = "small",`,
    `    tags = [`,
    `        "fast",`,
    `    ],`,
    `)`,
    ``,
    `sh_test(`,
    `    name = "e2e_smoke_remote_test",`,
    `    srcs = ["scripts/buildbuddy-e2e-smoke-remote-test"],`,
    `    data = [`,
    `        ":workspace_runfiles",`,
    `        "@node_darwin_arm64//:bin/node",`,
    `        "@node_darwin_arm64//:lib/node_modules/npm/bin/npm-cli.js",`,
    `        "@node_darwin_arm64//:lib/node_modules/npm/bin/npx-cli.js",`,
    `        "@node_darwin_arm64//:node",`,
    `        "@node_darwin_arm64//:npm_runtime",`,
    `        "@python_darwin_arm64//:python/bin/python3",`,
    `        "@python_darwin_arm64//:python",`,
    `        "@rules_rust//rust/toolchain:current_cargo_files",`,
    `        "@rules_rust//rust/toolchain:current_rust_stdlib_files",`,
    `        "@rules_rust//rust/toolchain:current_rust_toolchain",`,
    `        "@rules_rust//rust/toolchain:current_rustc_files",`,
    `        "@rules_rust//rust/toolchain:current_rustc_lib_files",`,
    `        "@rust_std_wasm32_unknown_unknown_1_94_0//:rust_std",`,
    `        "@wasm_pack_darwin_arm64//:wasm-pack",`,
    `        "//meerkat-cli:cli_mobpack_live_smoke_test",`,
    `        "//meerkat-cli:live_smoke_cli_test",`,
    `        "//meerkat-cli:rkat",`,
    `        "//meerkat-mcp-server:rkat_mcp_bin",`,
    `        "//meerkat-mob:smoke_mob_flow_runtime_test",`,
    `        "//meerkat-mob:smoke_mob_pictionary_test",`,
    `        "//meerkat-mob:smoke_mob_resume_test",`,
    `        "//meerkat-rest:rkat_rest_bin",`,
    `        "//meerkat-rpc:live_smoke_rpc_test",`,
    `        "//meerkat-rpc:rkat_rpc_bin",`,
    `        "//tests/integration:e2e_artifacts_bin",`,
    `        "//tests/integration:e2e_smoke_lane_test",`,
    `        "//tests/integration:smoke_shared_realm_test",`,
    `    ],`,
    `    env = {`,
    `        "MEERKAT_E2E_ARTIFACTS_BIN": "$(rootpath //tests/integration:e2e_artifacts_bin)",`,
    `        "MEERKAT_E2E_NODE_BIN": "$(rootpath @node_darwin_arm64//:bin/node)",`,
    `        "MEERKAT_E2E_NPM_CLI": "$(rootpath @node_darwin_arm64//:lib/node_modules/npm/bin/npm-cli.js)",`,
    `        "MEERKAT_E2E_NPX_CLI": "$(rootpath @node_darwin_arm64//:lib/node_modules/npm/bin/npx-cli.js)",`,
    `        "MEERKAT_E2E_PYTHON_BIN": "$(rootpath @python_darwin_arm64//:python/bin/python3)",`,
    `        "MEERKAT_E2E_WASM_PACK_BIN": "$(rootpath @wasm_pack_darwin_arm64//:wasm-pack)",`,
    `    },`,
    `    size = "enormous",`,
    `    tags = [`,
    `        "buildbuddy",`,
    `        "e2e",`,
    `        "integration",`,
    `        "live",`,
    `    ],`,
    `    timeout = "eternal",`,
    `)`,
    ``,
  ];
  if (fastTestLabels.length) {
    lines.push(
      `test_suite(`,
      `    name = "fast_tests",`,
      `    tests = ${listExpr(fastTestLabels, 8)},`,
      `)`,
      ``,
    );
  }
  if (e2eSystemTestLabels.length) {
    lines.push(
      `test_suite(`,
      `    name = "e2e_system_tests",`,
      `    tests = ${listExpr(e2eSystemTestLabels, 8)},`,
      `)`,
      ``,
    );
  }
  if (surfaceFeatureMatrixLabels.length) {
    lines.push(
      `filegroup(`,
      `    name = "surface_feature_matrix_builds",`,
      `    srcs = ${listExpr(surfaceFeatureMatrixLabels, 8)},`,
      `    visibility = ["//visibility:public"],`,
      `)`,
      ``,
    );
  }
  writeGenerated(resolve(root, "BUILD.bazel"), lines.join("\n"));
}

const surfaceFeatureVariantSpecs = [
  { name: "surface_min", packageKey: "meerkat-rpc", features: [], targetNames: null },
  { name: "surface_comms_mcp", packageKey: "meerkat-rpc", features: ["comms", "mcp"], targetNames: null },
  { name: "surface_mini", packageKey: "meerkat-rpc", features: ["mini-surface"], targetNames: ["rkat-rpc-mini"] },
  { name: "surface_min", packageKey: "meerkat-rest", features: [], targetNames: null },
  { name: "surface_comms", packageKey: "meerkat-rest", features: ["comms"], targetNames: null },
  { name: "surface_min", packageKey: "meerkat-mcp-server", features: [], targetNames: null },
  { name: "surface_comms", packageKey: "meerkat-mcp-server", features: ["comms"], targetNames: null },
  { name: "surface_session_store", packageKey: "meerkat-cli", features: ["session-store"], targetNames: null },
  { name: "surface_session_store_mcp", packageKey: "meerkat-cli", features: ["session-store", "mcp"], targetNames: null },
  {
    name: "surface_mini_providers",
    packageKey: "meerkat-cli",
    features: ["anthropic", "openai", "gemini", "jsonl-store", "session-store"],
    targetNames: ["rkat-mini"],
  },
  {
    name: "surface_mini_providers_skills",
    packageKey: "meerkat-cli",
    features: ["anthropic", "openai", "gemini", "jsonl-store", "session-store", "skills"],
    targetNames: ["rkat-mini"],
  },
  { name: "surface_session_store_comms_mcp", packageKey: "meerkat-cli", features: ["session-store", "comms", "mcp"], targetNames: null },
];

const fastTestLabels = [];
const e2eSystemTestLabels = [];
const surfaceFeatureMatrixLabels = [];
const agentFactoryBridgeSymbolSuffix = "bazel_private_agent_factory_build";

for (const pkg of localPackages.values()) {
  const dir = packageDir(pkg);
  if (!dir.startsWith(root)) continue;

  const rules = [];
  const packageFastTests = [];
  const loads = new Set(["rust_binary", "rust_library", "rust_proc_macro", "rust_test"]);
  let needsShellTestLoad = false;
  const libOrMacro = pkg.targets.find((target) =>
    target.kind.includes("proc-macro") || target.kind.includes("lib")
  );
  const key = packageKey(pkg);

  if (key === "meerkat") {
    for (const variantPkg of agentFactoryVariantPackages) {
      rules.push(`alias(
    name = ${q(agentFactoryVariantName(variantPkg))},
    actual = ${q(agentFactoryActualVariantLabel(variantPkg))},
    visibility = ${agentFactoryFacadeVariantAliasVisibility},
)`);
    }
  }

  for (const target of pkg.targets) {
    const rule = targetRule(target);
    if (!rule) continue;
    if (target.kind.includes("bench") || target.kind.includes("example")) {
      continue;
    }

    const isTest = target.kind.includes("test");
    const hasLibrary = Boolean(libOrMacro);
    const name =
      isTest
        ? `${crateName(target.name)}_test`
        :
      target.kind.includes("bin") && (target.name !== pkg.name || hasLibrary)
        ? `${crateName(target.name)}_bin`
        : crateName(pkg.name);
    const optionalExternal = optionalExternalDeps(pkg);
    const selfLibraryLabel = packageLabel(pkg);
    const rawDeps = isTest
      ? [...new Set([...(libOrMacro ? [selfLibraryLabel] : []), ...localDeps(pkg, false, true)])].sort()
      : target.kind.includes("bin") && libOrMacro
      ? [...new Set([selfLibraryLabel, ...localDeps(pkg, false, true)])].sort()
      : target.kind.includes("bin")
      ? localDeps(pkg, false, true)
      : localDeps(pkg, false);
    const deps = shouldRewriteAgentFactoryDepsFor(key, isTest)
      ? rewriteAgentFactoryDeps(rawDeps)
      : rawDeps;
    const procMacroDeps = localDeps(pkg, true, isTest);
    const isCorePublicLibrary = key === "meerkat-core" && rule === "rust_library";
    const targetSourceText = isTest ? readFileSync(target.src_path, "utf8") : "";
    let targetDeps = deps;
    if (key === "meerkat-machine-codegen" && target.name === "runtime_schema_parity") {
      const scheduleMachineSchemaExports = shouldRewriteAgentFactoryDepsFor(key, isTest)
        ? "//meerkat-schedule:meerkat_schedule_machine_schema_exports_agent_factory_build"
        : "//meerkat-schedule:meerkat_schedule_machine_schema_exports";
      targetDeps = deps
        .filter((dep) => dep !== "//meerkat-schedule:meerkat_schedule")
        .filter((dep) => dep !== "//meerkat-schedule:meerkat_schedule_agent_factory_build")
        .filter((dep) => dep !== "//meerkat:meerkat_schedule_agent_factory_build")
        .concat(scheduleMachineSchemaExports)
        .sort();
    }
    const externalNormal = `all_crate_deps(\n        package_name = ${q(key)},\n        normal = True,\n    )`;
    const externalNormalWithDev = `all_crate_deps(\n        package_name = ${q(key)},\n        normal = True,\n        normal_dev = True,\n    )`;
    const externalProc = `all_crate_deps(\n        package_name = ${q(key)},\n        proc_macro = True,\n    )`;
    const externalProcWithDev = `all_crate_deps(\n        package_name = ${q(key)},\n        proc_macro = True,\n        proc_macro_dev = True,\n    )`;

    const depsWithOptionalExternal = [...new Set([...targetDeps, ...optionalExternal])].sort();
    const depsExpr = depsWithOptionalExternal.length
      ? `${listExpr(depsWithOptionalExternal)} + ${isTest ? externalNormalWithDev : externalNormal}`
      : isTest ? externalNormalWithDev : externalNormal;
    const procExpr = procMacroDeps.length
      ? `${listExpr(procMacroDeps)} + ${isTest ? externalProcWithDev : externalProc}`
      : isTest ? externalProcWithDev : externalProc;
    const aliasesExpr = isTest
      ? `aliases(\n        package_name = ${q(key)},\n        normal = True,\n        normal_dev = True,\n        proc_macro = True,\n        proc_macro_dev = True,\n    )`
      : `aliases(package_name = ${q(key)})`;
    const extraData = isTest ? workspaceDataLabels(target) : [];
    const usesTrybuild = targetSourceText.includes("trybuild::");
    const scansWorkspaceRustSources = targetSourceText.includes("walk_rust_sources(&root)");
    const needsLiveWorkspaceRunfiles = targetSourceText.includes("LIVE_WORKSPACE_RUNFILES");
    const isE2eLaneHarness = target.name.startsWith("e2e_") && target.name.endsWith("_lane");
    const targetCompileData = compileData(target, dir, isTest);
    const compileDataExpr = `${listExpr(targetCompileData.paths, 8)}${
      targetCompileData.labels.length ? ` + ${listExpr(targetCompileData.labels, 8)}` : ""
    }`;
    const srcsExpr = isTest
      ? listExpr(testSourcePaths(target, pkg, dir), 8)
      : `glob(["src/**/*.rs"])`;

    const attrs = [
      `    name = ${q(name)},`,
      `    aliases = ${aliasesExpr},`,
      `    crate_name = ${q(crateName(target.name))},`,
      `    crate_root = ${q(relative(dir, target.src_path))},`,
      `    crate_features = ${listExpr(isCorePublicLibrary ? publicCoreCrateFeatures(key, pkg) : crateFeaturesFor(key, pkg))},`,
      `    edition = "2024",`,
      `    compile_data = ${compileDataExpr},`,
      `    srcs = ${srcsExpr},`,
      `    visibility = ${rustTargetVisibility(key)},`,
      `    deps = ${depsExpr},`,
    ];
    if (rule === "rust_binary" || rule === "rust_test") {
      const packageRunfilesDir = relative(root, dir) || ".";
      const cargoManifestDir = isTest && packageRunfilesDir !== "."
        ? `./${packageRunfilesDir}`
        : packageRunfilesDir;
      const rustcEnv = [
        `        "CARGO_BIN_NAME": ${q(target.name)},`,
        `        "CARGO_MANIFEST_DIR": ${q(cargoManifestDir)},`,
      ];
      if (rule === "rust_test" && key === "meerkat-rpc") {
        rustcEnv.push(`        "CARGO_BIN_EXE_rkat-rpc": "$(rootpath //meerkat-rpc:rkat_rpc_bin)",`);
        rustcEnv.push(`        "CARGO_BIN_EXE_rkat-rpc-mini": "$(rootpath //meerkat-rpc:rkat_rpc_mini_bin)",`);
      }
      attrs.splice(attrs.length - 1, 0, `    rustc_env = {\n${rustcEnv.join("\n")}\n    },`);
    }
    if (rule === "rust_library" && (key === "meerkat" || key === "meerkat-core")) {
      const packageRunfilesDir = relative(root, dir) || ".";
      const cargoManifestDir = packageRunfilesDir !== "."
        ? `./${packageRunfilesDir}`
        : packageRunfilesDir;
      const rustcEnv = [
        `        "CARGO_MANIFEST_DIR": ${q(cargoManifestDir)},`,
      ];
      if (key === "meerkat") {
        rustcEnv.push(
          `        "MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX": ${q(agentFactoryBridgeSymbolSuffix)},`,
        );
      }
      attrs.splice(
        attrs.length - 1,
        0,
        `    rustc_env = {\n${rustcEnv.join("\n")}\n    },`,
      );
    }
    if (rule === "rust_library" && key === "meerkat") {
      attrs.splice(attrs.length - 1, 0, `    data = [":package_runfiles"],`);
    }
    let rustTestBaseAttrs = null;
    let filteredNativeTests = [];
    if (rule === "rust_test") {
      rustTestBaseAttrs = [...attrs];
      const tags = testTags(pkg, target);
      if (usesTrybuild) tags.push("local");
      if (tags.includes("fast")) {
        fastTestLabels.push(`//${relative(root, dir)}:${name}`);
        packageFastTests.push(`:${name}`);
      }
      const currentPackageRunfiles = `//${relative(root, dir)}:package_runfiles`;
      const data = [
        ...extraData.filter((label) => label !== currentPackageRunfiles),
      ];
      if (needsPackageRunfiles(target) || extraData.includes(currentPackageRunfiles) || usesTrybuild) {
        data.unshift(":package_runfiles");
      }
      const env = [`        "RUST_MIN_STACK": "16777216",`];
      attrs.splice(attrs.length - 1, 0, `    tags = ${listExpr([...new Set(tags)].sort())},`);
      if (key === "meerkat" && target.name === "agent_builder_policy_canary") {
        attrs.splice(attrs.length - 1, 0, `    size = "large",`);
      } else if (tags.includes("fast")) {
        attrs.splice(attrs.length - 1, 0, `    size = "small",`);
      }
      if (usesTrybuild) {
        data.push("//:workspace_runfiles");
        data.push("@rules_rust//rust/toolchain:current_cargo_files");
        data.push("@rules_rust//rust/toolchain:current_rust_stdlib_files");
        data.push("@rules_rust//rust/toolchain:current_rustc_files");
        data.push("@rules_rust//rust/toolchain:current_rustc_lib_files");
        env.push(`        "CARGO_TARGET_DIR": "trybuild-target",`);
      }
      if (scansWorkspaceRustSources) {
        data.push("//:workspace_rust_sources");
      }
      if (key === "meerkat" && target.name === "agent_builder_policy_canary") {
        data.push(
          "//:workspace_runfiles",
          "//meerkat-rest:package_runfiles",
          "//meerkat-rpc:package_runfiles",
          "//meerkat-runtime:package_runfiles",
          "@rules_rust//rust/toolchain:current_cargo_files",
          "@rules_rust//rust/toolchain:current_rust_stdlib_files",
          "@rules_rust//rust/toolchain:current_rustc_files",
          "@rules_rust//rust/toolchain:current_rustc_lib_files",
        );
      }
      if (key === "meerkat-machine-codegen" && target.name === "runtime_schema_parity") {
        data.push("//:workspace_runfiles");
        env.push(`        "WORKSPACE_ROOT": ".",`);
      }
      if (key === "xtask" && target.name === "buildbuddy_static_lanes") {
        data.push("BUILD.bazel");
      }
      if (needsLiveWorkspaceRunfiles || isE2eLaneHarness) {
        data.push("//:workspace_runfiles");
        env.push(`        "WORKSPACE_ROOT": ".",`);
      }
      if (key === "meerkat-cli") {
        data.push("//meerkat-cli:rkat", "//meerkat-cli:rkat_mini_bin");
        env.push(`        "CARGO_BIN_EXE_rkat": "$(rootpath //meerkat-cli:rkat)",`);
        env.push(`        "CARGO_BIN_EXE_rkat-mini": "$(rootpath //meerkat-cli:rkat_mini_bin)",`);
      }
      if (key === "meerkat-rpc") {
        data.push("//meerkat-rpc:rkat_rpc_bin", "//meerkat-rpc:rkat_rpc_mini_bin");
        env.push(`        "CARGO_BIN_EXE_rkat-rpc": "$(rootpath //meerkat-rpc:rkat_rpc_bin)",`);
        env.push(`        "CARGO_BIN_EXE_rkat-rpc-mini": "$(rootpath //meerkat-rpc:rkat_rpc_mini_bin)",`);
      }
      if (key === "xtask") {
        const rustfmt = "@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustfmt_bin";
        const rustfmtLib = "@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustc_lib";
        data.push(rustfmt);
        data.push(rustfmtLib);
        env.push(`        "RUSTFMT": "$(rootpath ${rustfmt})",`);
        if (readFileSync(target.src_path, "utf8").includes("rev-parse")) {
          env.push(`        "WORKSPACE_ROOT": ".",`);
        }
      }
      attrs.splice(attrs.length - 1, 0, `    data = ${listExpr([...new Set(data)])},`);
      if (env.length) {
        attrs.splice(attrs.length - 1, 0, `    env = {\n${env.join("\n")}\n    },`);
      }
      filteredNativeTests = nativeE2eSystemSpecsFor(pkg, target).map((spec) => ({
        ...spec,
        data: [...new Set([...data, ...(spec.data ?? [])])].sort(),
        env: [...env, ...(spec.env ?? [])],
        tags: [...new Set([...tags, "e2e", "integration", "system"])].sort(),
      }));
    }
    attrs.splice(attrs.length - 1, 0, `    proc_macro_deps = ${procExpr},`);
    rules.push(`${rule}(\n${attrs.join("\n")}\n)`);
    const shouldGenerateAgentFactoryVariant =
      rule === "rust_library"
      && shouldGenerateAgentFactoryVariantForKey(key)
      && hasRustLibraryTarget(pkg);
    if (shouldGenerateAgentFactoryVariant) {
      const internalDepsWithOptionalExternal = [
        ...new Set([...rewriteAgentFactoryDeps(targetDeps), ...optionalExternal]),
      ].sort();
      const internalDepsExpr = internalDepsWithOptionalExternal.length
        ? `${listExpr(internalDepsWithOptionalExternal)} + ${externalNormal}`
        : externalNormal;
      const internalAttrs = attrs.map((line) => {
        if (line === `    name = ${q(name)},`) {
          return `    name = ${q(agentFactoryVariantName(pkg))},`;
        }
        if (line === `    visibility = ${rustTargetVisibility(key)},`) {
          return `    visibility = ${agentFactoryActualVariantVisibility},`;
        }
        if (line === `    deps = ${depsExpr},`) {
          return `    deps = ${internalDepsExpr},`;
        }
        return line;
      });
      if (key === "meerkat-core") {
        const editionIndex = internalAttrs.indexOf(`    edition = "2024",`);
        internalAttrs.splice(
          editionIndex + 1,
          0,
          `    rustc_flags = ["--cfg=meerkat_internal_agent_factory_build"],`,
        );
        const rustcEnvIndex = internalAttrs.findIndex((line) => line.startsWith("    rustc_env = {"));
        if (rustcEnvIndex >= 0) {
          internalAttrs[rustcEnvIndex] = `    rustc_env = {\n        "CARGO_MANIFEST_DIR": "./meerkat-core",\n        "MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX": ${q(agentFactoryBridgeSymbolSuffix)},\n    },`;
        }
      }
      if (key === "meerkat-machine-schema" || key === "meerkat-runtime") {
        const editionIndex = internalAttrs.indexOf(`    edition = "2024",`);
        internalAttrs.splice(
          editionIndex + 1,
          0,
          `    rustc_flags = ["-Copt-level=0"],`,
        );
      }
      rules.push(`rust_library(\n${internalAttrs.join("\n")}\n)`);
    }
    if (rule === "rust_library") {
      const unitName = `${crateName(target.name)}_unit_test`;
      const packageRunfilesDir = relative(root, dir) || ".";
      const cargoManifestDir = packageRunfilesDir !== "."
        ? `./${packageRunfilesDir}`
        : packageRunfilesDir;
      const unitLocalDeps = shouldRewriteAgentFactoryDepsFor(key, true)
        ? rewriteAgentFactoryDeps(localDeps(pkg, false, true))
        : localDeps(pkg, false, true);
      const unitBaseDeps = shouldRewriteAgentFactoryDepsFor(key, true)
        ? rewriteAgentFactoryDeps(depsWithOptionalExternal)
        : depsWithOptionalExternal;
      const unitDeps = [
        ...new Set([
          ...unitBaseDeps,
          ...unitLocalDeps,
        ]),
      ].sort();
      const unitProcDeps = [...new Set([...procMacroDeps, ...localDeps(pkg, true, true)])].sort();
      const unitDepsExpr = unitDeps.length
        ? `${listExpr(unitDeps)} + ${externalNormalWithDev}`
        : externalNormalWithDev;
      const unitProcExpr = unitProcDeps.length
        ? `${listExpr(unitProcDeps)} + ${externalProcWithDev}`
        : externalProcWithDev;
      const currentPackageRunfiles = `//${relative(root, dir)}:package_runfiles`;
      const unitData = [
        ":package_runfiles",
        ...workspaceDataLabels(target).filter((label) => label !== currentPackageRunfiles),
      ];
      const unitEnv = [`        "RUST_MIN_STACK": "16777216",`];
      const unitSize = key === "meerkat-mob" ? "large" : key === "xtask" ? "medium" : "small";
      if (key === "xtask") {
        const rustfmt = "@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustfmt_bin";
        const rustfmtLib = "@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustc_lib";
        unitData.push("//:workspace_runfiles");
        unitData.push(rustfmt);
        unitData.push(rustfmtLib);
        unitEnv.push(`        "RUSTFMT": "$(rootpath ${rustfmt})",`);
        unitEnv.push(`        "WORKSPACE_ROOT": ".",`);
      }
      const unitRustcEnv = [
        `        "CARGO_MANIFEST_DIR": ${q(cargoManifestDir)},`,
      ];
      if (key === "meerkat") {
        unitRustcEnv.push(
          `        "MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX": ${q(agentFactoryBridgeSymbolSuffix)},`,
        );
      }
      const unitAttrs = [
        `    name = ${q(unitName)},`,
        `    aliases = ${aliasesExpr.replace(`aliases(package_name = ${q(key)})`, `aliases(\n        package_name = ${q(key)},\n        normal = True,\n        normal_dev = True,\n        proc_macro = True,\n        proc_macro_dev = True,\n    )`)},`,
        `    crate_name = ${q(crateName(target.name))},`,
        `    crate_root = ${q(relative(dir, target.src_path))},`,
        `    crate_features = ${listExpr(crateFeaturesFor(key, pkg))},`,
        `    edition = "2024",`,
        `    compile_data = ${compileDataExpr},`,
        `    srcs = ${srcsExpr},`,
        `    visibility = ${rustTargetVisibility(key)},`,
        `    rustc_env = {\n${unitRustcEnv.join("\n")}\n    },`,
        `    tags = ${listExpr(["fast", "unit"])},`,
        `    size = ${q(unitSize)},`,
        `    data = ${listExpr([...new Set(unitData)].sort())},`,
        `    env = {\n${unitEnv.join("\n")}\n    },`,
        `    proc_macro_deps = ${unitProcExpr},`,
        `    deps = ${unitDepsExpr},`,
      ];
      rules.push(`rust_test(\n${unitAttrs.join("\n")}\n)`);
      fastTestLabels.push(`//${relative(root, dir)}:${unitName}`);
      packageFastTests.push(`:${unitName}`);
    }
    for (const spec of filteredNativeTests) {
      const filteredAttrs = [...rustTestBaseAttrs];
      filteredAttrs[0] = `    name = ${q(spec.name)},`;
      filteredAttrs.splice(
        filteredAttrs.length - 1,
        0,
        `    args = ${listExpr(["--exact", spec.testName, "--ignored", "--nocapture"])},`,
        `    tags = ${listExpr(spec.tags)},`,
        `    size = "large",`,
        `    data = ${listExpr(spec.data)},`,
        `    env = {\n${spec.env.join("\n")}\n    },`,
        `    proc_macro_deps = ${procExpr},`,
      );
      rules.push(`${rule}(\n${filteredAttrs.join("\n")}\n)`);
      e2eSystemTestLabels.push(`//${relative(root, dir)}:${spec.name}`);
    }
  }

  const packageSurfaceSpecs = surfaceFeatureVariantSpecs.filter((spec) => spec.packageKey === key);
  if (packageSurfaceSpecs.length) {
    const externalNormal = `all_crate_deps(\n        package_name = ${q(key)},\n        normal = True,\n    )`;
    const externalProc = `all_crate_deps(\n        package_name = ${q(key)},\n        proc_macro = True,\n    )`;
    const optionalExternal = optionalExternalDeps(pkg);
    const procMacroDeps = localDeps(pkg, true);
    const procExpr = procMacroDeps.length
      ? `${listExpr(procMacroDeps)} + ${externalProc}`
      : externalProc;
    const aliasesExpr = `aliases(package_name = ${q(key)})`;
    const packageBuildTargets = pkg.targets.filter((target) => {
      const rule = targetRule(target);
      return (rule === "rust_library" || rule === "rust_binary") &&
        !target.kind.includes("bench") &&
        !target.kind.includes("example");
    });
    for (const spec of packageSurfaceSpecs) {
      const explicitNames = spec.targetNames ? new Set(spec.targetNames) : null;
      const selectedTargets = packageBuildTargets.filter((target) =>
        explicitNames ? explicitNames.has(target.name) : true
      );
      if (explicitNames && libOrMacro && !selectedTargets.some((target) => target.name === libOrMacro.name)) {
        selectedTargets.unshift(libOrMacro);
      }
      const libVariantNames = new Map();
      for (const target of selectedTargets) {
        if (target.kind.includes("proc-macro") || target.kind.includes("lib")) {
          libVariantNames.set(target.name, `${crateName(target.name)}_${spec.name}`);
        }
      }
      const primaryLibVariantName = libOrMacro ? libVariantNames.get(libOrMacro.name) : null;
      for (const target of selectedTargets) {
        const rule = targetRule(target);
        if (rule !== "rust_library" && rule !== "rust_binary") continue;
        const targetCompileData = compileData(target, dir, false);
        const compileDataExpr = `${listExpr(targetCompileData.paths, 8)}${
          targetCompileData.labels.length ? ` + ${listExpr(targetCompileData.labels, 8)}` : ""
        }`;
        const name = target.kind.includes("proc-macro") || target.kind.includes("lib")
          ? `${crateName(target.name)}_${spec.name}`
          : `${crateName(target.name)}_${spec.name}_bin`;
        const localDependencyLabels = target.kind.includes("bin") && primaryLibVariantName
          ? [...new Set([`:${primaryLibVariantName}`, ...localDeps(pkg, false, true, key, spec.features)])]
          : target.kind.includes("bin")
          ? localDeps(pkg, false, true, key, spec.features)
          : localDeps(pkg, false, false, key, spec.features);
        const targetDeps = shouldRewriteAgentFactoryDepsFor(key, false)
          ? rewriteAgentFactoryDeps(localDependencyLabels)
          : localDependencyLabels;
        const depsWithOptionalExternal = [...new Set([...targetDeps, ...optionalExternal])].sort();
        const depsExpr = depsWithOptionalExternal.length
          ? `${listExpr(depsWithOptionalExternal)} + ${externalNormal}`
          : externalNormal;
        const attrs = [
          `    name = ${q(name)},`,
          `    aliases = ${aliasesExpr},`,
          `    crate_name = ${q(crateName(target.name))},`,
          `    crate_root = ${q(relative(dir, target.src_path))},`,
          `    crate_features = ${listExpr(spec.features)},`,
          `    edition = "2024",`,
          `    compile_data = ${compileDataExpr},`,
          `    srcs = glob(["src/**/*.rs"]),`,
          `    visibility = ${rustTargetVisibility(key)},`,
        ];
        if (rule === "rust_binary") {
          const rustcEnv = [
            `        "CARGO_BIN_NAME": ${q(target.name)},`,
            `        "CARGO_MANIFEST_DIR": ${q(relative(root, dir) || ".")},`,
          ];
          if (key === "meerkat") {
            rustcEnv.push(
              `        "MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX": ${q(agentFactoryBridgeSymbolSuffix)},`,
            );
          }
          attrs.push(
            `    rustc_env = {\n${rustcEnv.join("\n")}\n    },`,
          );
        } else if (key === "meerkat") {
          attrs.push(
            `    rustc_env = {\n        "CARGO_MANIFEST_DIR": ${q(relative(root, dir) || ".")},\n        "MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX": ${q(agentFactoryBridgeSymbolSuffix)},\n    },`,
          );
        }
        attrs.push(
          `    tags = ${listExpr(["buildbuddy", "feature-matrix", "surface"])},`,
          `    proc_macro_deps = ${procExpr},`,
          `    deps = ${depsExpr},`,
        );
        rules.push(`${rule}(\n${attrs.join("\n")}\n)`);
        surfaceFeatureMatrixLabels.push(`//${relative(root, dir)}:${name}`);
      }
    }
  }

  if (key === "meerkat-schedule" && libOrMacro) {
    rules.push(`rust_library(
    name = "meerkat_schedule_machine_schema_exports",
    aliases = aliases(package_name = "meerkat-schedule"),
    crate_name = "meerkat_schedule",
    crate_root = "src/lib.rs",
    crate_features = ${listExpr(crateFeaturesFor(key, pkg, ["machine-schema-exports"]))},
    edition = "2024",
    compile_data = [
        "Cargo.toml",
    ],
    srcs = glob(["src/**/*.rs"]),
    visibility = ["//meerkat-machine-codegen:__pkg__"],
    proc_macro_deps = [
        "//meerkat-machine-dsl:meerkat_machine_dsl",
    ] + all_crate_deps(
        package_name = "meerkat-schedule",
        proc_macro = True,
    ),
    deps = [
        "${packageLabel(byName.get("meerkat-core"))}",
        "//meerkat-machine-schema:meerkat_machine_schema",
    ] + all_crate_deps(
        package_name = "meerkat-schedule",
        normal = True,
    ),
)`);
    rules.push(`rust_library(
    name = "meerkat_schedule_machine_schema_exports_agent_factory_build",
    aliases = aliases(package_name = "meerkat-schedule"),
    crate_name = "meerkat_schedule",
    crate_root = "src/lib.rs",
    crate_features = ${listExpr(crateFeaturesFor(key, pkg, ["machine-schema-exports"]))},
    edition = "2024",
    compile_data = [
        "Cargo.toml",
    ],
    srcs = glob(["src/**/*.rs"]),
    visibility = ["//meerkat-machine-codegen:__pkg__"],
    proc_macro_deps = [
        "//meerkat-machine-dsl:meerkat_machine_dsl",
    ] + all_crate_deps(
        package_name = "meerkat-schedule",
        proc_macro = True,
    ),
    deps = [
        "//meerkat:meerkat_core_agent_factory_build",
        "//meerkat:meerkat_machine_schema_agent_factory_build",
    ] + all_crate_deps(
        package_name = "meerkat-schedule",
        normal = True,
    ),
)`);
  }

  if (key === "xtask") {
    needsShellTestLoad = true;
    const rustfmt = "@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustfmt_bin";
    const rustfmtLib = "@@rules_rust++rust+rustfmt_nightly-2026-04-16__aarch64-apple-darwin_tools//:rustc_lib";
    rules.push(`sh_test(
    name = "machine_verify_all_tlc_test",
    srcs = ["tests/machine_verify_all_tlc_test.sh"],
    args = ["$(rootpath :xtask_bin)"],
    data = [
        ":xtask_bin",
        "//:workspace_runfiles",
        "${rustfmtLib}",
        "${rustfmt}",
    ],
    env = {
        "RUSTFMT": "$(rootpath ${rustfmt})",
    },
    size = "enormous",
    timeout = "eternal",
    tags = [
        "buildbuddy",
        "required-feature",
    ],
)`);
  }

  if (rules.length === 0) continue;
  if (packageFastTests.length) {
    rules.push(`test_suite(\n    name = "fast_tests",\n    tests = ${listExpr(packageFastTests.sort())},\n)`);
  }
  if (!checkOnly) mkdirSync(dir, { recursive: true });
  writeGenerated(
    resolve(dir, "BUILD.bazel"),
    [
      `load("@crates//:defs.bzl", "aliases", "all_crate_deps")`,
      `load("@rules_rust//rust:defs.bzl", ${[...loads].sort().map(q).join(", ")})`,
      ...(needsShellTestLoad ? [`load("@rules_shell//shell:sh_test.bzl", "sh_test")`] : []),
      "",
      `filegroup(`,
      `    name = "cargo_manifest",`,
      `    srcs = ["Cargo.toml"],`,
      `    visibility = ["//visibility:public"],`,
      `)`,
      "",
      `filegroup(`,
      `    name = "package_runfiles",`,
      `    srcs = glob(["Cargo.toml", "**/*"], exclude = ["BUILD", "BUILD.bazel"], allow_empty = True),`,
      `    visibility = ["//visibility:public"],`,
      `)`,
      "",
      ...rules,
      "",
    ].join("\n\n"),
  );
}

writeRootBuild(
  [...new Set(fastTestLabels)].sort(),
  [...new Set(e2eSystemTestLabels)].sort(),
  [...new Set(surfaceFeatureMatrixLabels)].sort(),
);
if (checkOnly && staleFileCount > 0) {
  console.error(`${staleFileCount} generated Bazel file(s) are stale; run node scripts/generate-bazel-rust-builds.mjs`);
  process.exit(1);
}
