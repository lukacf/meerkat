#!/usr/bin/env node

/**
 * Builds the CJS version: compiles TS to CommonJS in dist-cjs/,
 * then renames all .js files to .cjs and rewrites require() paths.
 */

import { execSync } from "node:child_process";
import { writeFileSync, rmSync, cpSync, readdirSync, readFileSync, renameSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const root = join(__dirname, "..");

// Compile CJS build to temp dir
execSync("npx tsc -p tsconfig.cjs.json", { cwd: root, stdio: "inherit" });

const srcDir = join(root, "dist-cjs");
const destDir = join(root, "dist", "cjs");

// Clean destination
rmSync(destDir, { recursive: true, force: true });

// Copy to destination
cpSync(srcDir, destDir, { recursive: true });

// Rename .js → .cjs and rewrite require("./foo.js") → require("./foo.cjs")
function processDir(dir) {
  for (const entry of readdirSync(dir)) {
    const fullPath = join(dir, entry);
    const stat = statSync(fullPath);
    if (stat.isDirectory()) {
      processDir(fullPath);
      continue;
    }
    if (!entry.endsWith(".js")) continue;

    // Read and rewrite require paths: ./foo.js → ./foo.cjs
    let content = readFileSync(fullPath, "utf-8");
    content = content.replace(/require\("\.([^"]+)\.js"\)/g, 'require(".$1.cjs")');

    // Write as .cjs
    const cjsPath = fullPath.replace(/\.js$/, ".cjs");
    writeFileSync(cjsPath, content, "utf-8");

    // Remove original .js
    if (cjsPath !== fullPath) {
      rmSync(fullPath);
    }
  }
}

processDir(destDir);

// Clean up temp dir
rmSync(srcDir, { recursive: true, force: true });

// Create dist/index.cjs that re-exports from dist/cjs/index.cjs
writeFileSync(
  join(root, "dist", "index.cjs"),
  `"use strict";\nmodule.exports = require("./cjs/index.cjs");\n`,
  "utf-8",
);

console.log("Generated dist/index.cjs");
