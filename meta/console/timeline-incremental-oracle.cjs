#!/usr/bin/env node
"use strict";

// Runs the stock adapter test suites with src/lib/adapters resolved to
// src/lib/adapters-incremental-oracle.ts, which checks on every call that an
// incrementally extended transcript derivation equals a full one (see that
// file), plus the dedicated incremental-derivation tests.

const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { build } = require("esbuild");

const adapters = path.join(__dirname, "src/lib/adapters.ts");
const oracle = path.join(__dirname, "src/lib/adapters-incremental-oracle.ts");
const entries = [
  "src/lib/adapters.test.ts",
  "src/lib/adapters-markdown.test.ts",
  "src/lib/adapters-completion.test.ts",
  "src/lib/adapters-integrity.test.ts",
  "src/lib/adapters-durable-append.test.ts",
  "src/lib/adapters-assistant-identity.test.ts",
  "src/lib/adapters-realtime-identity.test.ts",
  "src/lib/timeline-derivation.test.ts",
  "src/panels/ChatPane.test.ts",
  "../packages/console-core/src/assistant-message-source.test.ts",
];

const redirectAdapters = {
  name: "incremental-oracle",
  setup(pluginBuild) {
    pluginBuild.onResolve({ filter: /adapters$/ }, async (args) => {
      if (args.importer === oracle || args.pluginData?.oracle) return undefined;
      const resolved = await pluginBuild.resolve(args.path, {
        kind: args.kind,
        resolveDir: args.resolveDir,
        pluginData: { oracle: true },
      });
      return resolved.path === adapters ? { path: oracle } : resolved;
    });
  },
};

(async () => {
  const outdir = path.join(__dirname, ".tmp/timeline-incremental-oracle");
  const files = [];
  for (const [index, entry] of entries.entries()) {
    const outfile = path.join(outdir, `${index}-${path.basename(entry).replace(/\.tsx?$/, "")}.mjs`);
    await build({
      entryPoints: [path.join(__dirname, entry)],
      outfile,
      bundle: true,
      platform: "node",
      format: "esm",
      jsx: "automatic",
      alias: {
        "@console-core": path.resolve(__dirname, "../packages/console-core/src/index.ts"),
        "@console-components": path.resolve(__dirname, "../packages/console-components/src/index.ts"),
      },
      external: ["react", "react-dom", "react-dom/server", "react-markdown", "remark-gfm", "clsx"],
      nodePaths: [path.resolve(__dirname, "node_modules")],
      plugins: [redirectAdapters],
      logLevel: "error",
    });
    files.push(outfile);
  }
  // That test counts JSON.parse calls across two derivations; the oracle's
  // extra prefix derivations change the count, not the result. It runs
  // unwrapped in phase0:types.
  const skip = ["--test-skip-pattern", "never re-JSON.parse"];
  const result = spawnSync(process.execPath, ["--test", ...skip, ...files], { stdio: "inherit", cwd: __dirname });
  process.exitCode = result.status ?? 1;
})().catch((error) => {
  process.stderr.write(`${error.stack}\n`);
  process.exitCode = 1;
});
