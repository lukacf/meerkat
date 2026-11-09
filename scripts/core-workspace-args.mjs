#!/usr/bin/env node
// Prints the cargo package selection of Core's whole-workspace lanes, one
// argument per line: `--workspace` and an `--exclude NAME` for every package
// of the other products (ci/products.json: the members whose manifests lie
// under a product root, from `cargo metadata --no-deps`). Cargo unifies
// features across the packages of one invocation, so a lane that named
// `--workspace` alone would compile Core's crates with the features the
// products request of them. The list is derived on every call, never
// written down.
//
// With --product NAME it prints that product's own selection instead (`-p`
// for each of its packages), with -p NAME (repeatable) exactly those
// packages, and with --exclude NAME (repeatable) Core's selection without
// those members.
//
// With --tests it also prints the `--features` that every test build of the
// selected members passes: each member's test features (ci-products.mjs
// testFeaturesOf: its [package.metadata.meerkat.standalone-tests] features,
// and `workspace-tests` when it declares that feature), named by package, or
// by their bare names for one -p package. A test build without them fails to
// compile in this repository (the crate's test-feature canary). Leave --tests
// off for a build that compiles no test target (build, doc, a check without
// test targets).
//
// It also refuses a workspace whose default-members are not exactly Core's
// members: a bare cargo command and the .cargo/config.toml aliases (which
// cannot run a script) build the default members.
//
// usage: core-workspace-args.mjs [--tests] [--product NAME | -p NAME ... | --exclude NAME ...]
//   CARGO selects the cargo to ask (default: the repository's
//   scripts/repo-cargo).

import { execFileSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  CORE,
  coreWorkspaceArgs,
  defaultMembersProblem,
  loadProducts,
  productPackageArgs,
  testFeatureArgs,
  testFeaturesOf,
  workspaceMembersByProduct,
} from "./ci-products.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const USAGE = "usage: core-workspace-args.mjs [--tests] [--product NAME | -p NAME ... | --exclude NAME ...]";

function main(argv) {
  let product = null;
  let tests = false;
  const named = [];
  const excluded = [];
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--product" && argv[index + 1]) {
      product = argv[++index];
    } else if ((argv[index] === "-p" || argv[index] === "--package") && argv[index + 1]) {
      named.push(argv[++index]);
    } else if (argv[index] === "--exclude" && argv[index + 1]) {
      excluded.push(argv[++index]);
    } else if (argv[index] === "--tests") {
      tests = true;
    } else {
      throw new Error(USAGE);
    }
  }
  if ([product !== null, named.length > 0, excluded.length > 0].filter(Boolean).length > 1) throw new Error(USAGE);
  const products = loadProducts();
  if (product !== null && (product === CORE || !products.some((entry) => entry.name === product))) {
    throw new Error(`--product names no product other than Core: ${product}`);
  }
  const cargo = process.env.CARGO || resolve(root, "scripts/repo-cargo");
  const metadata = JSON.parse(
    execFileSync(cargo, ["metadata", "--format-version=1", "--no-deps"], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "inherit"],
    }),
  );
  const byProduct = workspaceMembersByProduct(metadata, products);
  const problem = defaultMembersProblem(metadata, byProduct);
  if (problem) throw new Error(problem);
  const memberIds = new Set(metadata.workspace_members ?? []);
  const members = new Map((metadata.packages ?? []).filter((pkg) => memberIds.has(pkg.id)).map((pkg) => [pkg.name, pkg]));
  const args = named.length > 0
    ? [...new Set(named)].flatMap((name) => ["-p", name])
    : product === null
      ? [...coreWorkspaceArgs(byProduct), ...[...new Set(excluded)].flatMap((name) => ["--exclude", name])]
      : productPackageArgs(byProduct, product);
  // The selected members: the named packages, the product's, or Core's less
  // the excluded ones. A name that is no member gets no features here; Cargo
  // itself refuses the selection.
  const selected = named.length > 0
    ? [...new Set(named)]
    : byProduct.get(product ?? CORE).filter((name) => !excluded.includes(name));
  const known = selected.filter((name) => members.has(name)).map((name) => members.get(name));
  // One named package takes its features by their bare names, as a
  // package-scoped Cargo command spells them.
  if (tests && named.length === 1 && known.length === 1) {
    const features = testFeaturesOf(known[0]);
    if (features.length > 0) args.push("--features", features.join(","));
  } else if (tests) {
    args.push(...testFeatureArgs(known));
  }
  process.stdout.write(`${args.join("\n")}\n`);
}

try {
  main(process.argv.slice(2));
} catch (error) {
  console.error(`core-workspace-args: ${error.message}`);
  process.exit(1);
}
