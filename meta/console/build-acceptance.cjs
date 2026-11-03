#!/usr/bin/env node
"use strict";
const path = require("node:path");
const { build } = require("esbuild");
const { fontLoaders } = require("./build.cjs");
build({
  entryPoints: { host: path.join(__dirname, "fixtures/shared-conversation-host.tsx"), scoped: path.join(__dirname, "fixtures/scoped-stock-host.tsx") },
  outdir: path.join(__dirname, ".tmp/acceptance"), bundle: true,
  format: "iife", platform: "browser", target: ["es2020"], jsx: "automatic",
  define: { "process.env.NODE_ENV": '"production"' },
  alias: {
    "@console-core": path.resolve(__dirname, "../packages/console-core/src/index.ts"),
    "@console-components": path.resolve(__dirname, "../packages/console-components/src/index.ts"),
    "@console-components/styles": path.resolve(__dirname, "../packages/console-components/src/styles/index.ts"),
  },
  nodePaths: [path.join(__dirname, "node_modules")],
  loader: fontLoaders,
}).catch(error => { process.stderr.write(`${error.stack}\n`); process.exitCode = 1; });
