// The products of the repository (ci/products.json): Core owns the root,
// every other product one top-level directory. A path belongs to the product
// whose root holds it, and a workspace package to the product whose root
// holds its manifest; everything else is Core's.
//
// The CI planner (scripts/ci-cargo-lanes.mjs) partitions its rows by
// product: no cargo invocation it plans mixes Core packages with a product's
// (Cargo unifies features across the packages of one invocation, and a
// product's feature requests must not change what Core's rows compile), and
// a change inside a product's root never escalates to Core's lanes.
//
// It also holds each member's test features (testFeaturesOf), which every
// test build of a member passes.
//
// Pure apart from loadProducts(): no git, no cargo, nothing runs at import.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PRODUCTS_PATH = "ci/products.json";
export const CORE = "core";

const NAME_RE = /^[a-z][a-z0-9-]*$/;
const ROOT_RE = /^[a-z][a-z0-9-]*$/;

// Validate the parsed document and return [{ name, root, tagPrefix,
// releaseBranchPrefix, changelog, publishOrder }], Core first. Throws on any
// malformed entry, so a planner that reads it fails closed.
export function parseProducts(document) {
  if (document === null || typeof document !== "object") throw new Error(`${PRODUCTS_PATH}: not a JSON object`);
  if (document.schema_version !== 1) throw new Error(`${PRODUCTS_PATH}: unsupported schema_version ${document.schema_version}`);
  if (!Array.isArray(document.products) || document.products.length === 0) {
    throw new Error(`${PRODUCTS_PATH}: products must be a non-empty list`);
  }
  const products = document.products.map((entry) => {
    const where = `${PRODUCTS_PATH}: product ${JSON.stringify(entry?.name)}`;
    if (typeof entry?.name !== "string" || !NAME_RE.test(entry.name)) throw new Error(`${where}: bad name`);
    if (typeof entry.root !== "string") throw new Error(`${where}: root must be a string`);
    if (entry.name === CORE ? entry.root !== "" : !ROOT_RE.test(entry.root)) {
      throw new Error(`${where}: Core's root is "" and every other product's one top-level directory`);
    }
    for (const field of ["tag_prefix", "release_branch_prefix", "changelog"]) {
      if (typeof entry[field] !== "string" || entry[field] === "") throw new Error(`${where}: ${field} must be a non-empty string`);
    }
    const order = entry.publish_order;
    if (!(typeof order === "string" || (Array.isArray(order) && order.every((name) => typeof name === "string" && name)))) {
      throw new Error(`${where}: publish_order must be a script path or a list of crate names`);
    }
    const stack = entry.test_stack;
    if (stack !== undefined && !(Number.isInteger(stack) && stack >= 2 * 1024 * 1024)) {
      throw new Error(`${where}: test_stack must be a whole number of bytes, at least 2 MiB`);
    }
    return {
      name: entry.name,
      root: entry.root,
      tagPrefix: entry.tag_prefix,
      releaseBranchPrefix: entry.release_branch_prefix,
      changelog: entry.changelog,
      publishOrder: order,
      testStack: stack ?? null,
    };
  });
  if (products[0].name !== CORE) throw new Error(`${PRODUCTS_PATH}: Core comes first`);
  const names = products.map((product) => product.name);
  const roots = products.map((product) => product.root);
  const prefixes = products.map((product) => product.tagPrefix);
  for (const [label, values] of [["name", names], ["root", roots], ["tag_prefix", prefixes]]) {
    if (new Set(values).size !== values.length) throw new Error(`${PRODUCTS_PATH}: duplicate product ${label}`);
  }
  return products;
}

export function loadProducts(file = resolve(dirname(fileURLToPath(import.meta.url)), "..", PRODUCTS_PATH)) {
  return parseProducts(JSON.parse(readFileSync(file, "utf8")));
}

// The product a repository path belongs to: the product whose root holds it,
// else Core.
export function productOfPath(path, products) {
  const normalized = String(path).replaceAll("\\", "/").replace(/^\.\//, "");
  for (const product of products) {
    if (product.root && (normalized === product.root || normalized.startsWith(`${product.root}/`))) return product.name;
  }
  return CORE;
}

// The roots of the products other than Core, as path prefixes ending in "/".
export function productRootPrefixes(products) {
  return products.filter((product) => product.name !== CORE).map((product) => `${product.root}/`);
}

// The workspace's member packages by product: each member belongs to the
// product whose root holds its manifest (cargo metadata, Core first).
export function workspaceMembersByProduct(metadata, products) {
  const members = new Set(metadata.workspace_members ?? []);
  const workspaceRoot = String(metadata.workspace_root ?? "");
  const byProduct = new Map(products.map((product) => [product.name, []]));
  for (const pkg of metadata.packages ?? []) {
    if (!members.has(pkg.id)) continue;
    const manifestDir = String(pkg.manifest_path).replaceAll("\\", "/").replace(/\/Cargo\.toml$/, "");
    const base = workspaceRoot.replaceAll("\\", "/").replace(/\/$/, "");
    if (manifestDir !== base && !manifestDir.startsWith(`${base}/`)) {
      throw new Error(`workspace member ${pkg.name} lies outside the workspace root ${base}`);
    }
    const relativeDir = manifestDir === base ? "" : manifestDir.slice(base.length + 1);
    byProduct.get(productOfPath(relativeDir, products)).push(pkg.name);
  }
  for (const names of byProduct.values()) names.sort();
  return byProduct;
}

// The cargo package selection of Core's whole-workspace lanes: `--workspace`
// and an `--exclude` for every package of the other products. Cargo unifies
// features across the packages of one invocation, so `--workspace` alone
// would compile Core's crates with the features the products request of
// them, and Core's lanes would no longer build what Core's own tests ran
// against.
export function coreWorkspaceArgs(byProduct) {
  const excluded = [...byProduct]
    .filter(([name]) => name !== CORE)
    .flatMap(([, names]) => names)
    .sort();
  return ["--workspace", ...excluded.flatMap((name) => ["--exclude", name])];
}

// The cargo package selection of one product's own lanes: `-p` for each of
// its packages, so they resolve together as they did in the product's own
// workspace and no Core member's features join in.
export function productPackageArgs(byProduct, product) {
  const names = byProduct.get(product) ?? [];
  if (names.length === 0) throw new Error(`product ${product} has no workspace member`);
  return names.flatMap((name) => ["-p", name]);
}

// Test features. A workspace member's own tests build with the features its
// [package.metadata.meerkat.standalone-tests] lists, plus `workspace-tests`
// when it declares that feature (the unit tests that need a crate its
// published package cannot depend on). No dev-dependency turns them on: a
// path-only self dev-dependency did, and it made the crate's lib test wait
// for its own library. So every command that builds a member's test targets
// in this repository passes them (testFeatureArgs), and a crate's test build
// without them fails to compile there (its lib.rs canary, which
// .cargo/config.toml [env] MEERKAT_WORKSPACE_TEST_FEATURES arms). A
// non-test build (build, check without test targets, doc) passes none.
export const WORKSPACE_TESTS_FEATURE = "workspace-tests";

// The test features of a member (a cargo metadata package): its declared
// standalone-test features, then `workspace-tests` when it declares that
// feature. Throws on a declaration the published test contract refuses.
export function testFeaturesOf(pkg) {
  const declared = pkg.metadata?.meerkat?.["standalone-tests"]?.features ?? [];
  if (!Array.isArray(declared) || declared.some((feature) => typeof feature !== "string" || feature === "")) {
    throw new Error(`${pkg.name}: [package.metadata.meerkat.standalone-tests] features must list feature names`);
  }
  const features = pkg.features ?? {};
  for (const feature of declared) {
    if (feature === WORKSPACE_TESTS_FEATURE) {
      throw new Error(`${pkg.name}: standalone-tests features must not include ${WORKSPACE_TESTS_FEATURE}`);
    }
    if (!Object.hasOwn(features, feature)) {
      throw new Error(`${pkg.name}: standalone-tests feature ${feature} is not declared in [features]`);
    }
  }
  const result = [...new Set(declared)];
  if (Object.hasOwn(features, WORKSPACE_TESTS_FEATURE)) result.push(WORKSPACE_TESTS_FEATURE);
  return result;
}

// The `--features` arguments of a test build that selects `packages` (cargo
// metadata packages): every selected member's test features, each named by
// its package (a bare name would reach every selected member that declares
// it), in package order; none when no selected member has any.
export function testFeatureArgs(packages) {
  const features = [...packages]
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((pkg) => testFeaturesOf(pkg).map((feature) => `${pkg.name}/${feature}`));
  return features.length > 0 ? ["--features", features.join(",")] : [];
}

// The workspace's default members must be exactly Core's members: a bare
// cargo command, and the .cargo/config.toml aliases (which cannot run a
// script to derive the selection), build them. Returns a description of the
// difference, or null when they agree.
export function defaultMembersProblem(metadata, byProduct) {
  const names = new Map((metadata.packages ?? []).map((pkg) => [pkg.id, pkg.name]));
  const defaults = (metadata.workspace_default_members ?? []).map((id) => names.get(id) ?? id).sort();
  const core = [...(byProduct.get(CORE) ?? [])].sort();
  const missing = core.filter((name) => !defaults.includes(name));
  const extra = defaults.filter((name) => !core.includes(name));
  if (missing.length === 0 && extra.length === 0) return null;
  return `the workspace's default-members must be exactly Core's members (Cargo.toml [workspace] default-members): ` +
    `missing ${missing.join(", ") || "none"}; not Core's ${extra.join(", ") || "none"}`;
}
