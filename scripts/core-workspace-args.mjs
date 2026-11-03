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
// for each of its packages).
//
// It also refuses a workspace whose default-members are not exactly Core's
// members: a bare cargo command and the .cargo/config.toml aliases (which
// cannot run a script) build the default members.
//
// usage: core-workspace-args.mjs [--product NAME]
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
  workspaceMembersByProduct,
} from "./ci-products.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function main(argv) {
  let product = null;
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--product" && argv[index + 1]) {
      product = argv[++index];
    } else {
      throw new Error("usage: core-workspace-args.mjs [--product NAME]");
    }
  }
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
  const args = product === null ? coreWorkspaceArgs(byProduct) : productPackageArgs(byProduct, product);
  process.stdout.write(`${args.join("\n")}\n`);
}

try {
  main(process.argv.slice(2));
} catch (error) {
  console.error(`core-workspace-args: ${error.message}`);
  process.exit(1);
}
