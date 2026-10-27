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
