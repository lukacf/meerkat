#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const webDir = path.resolve(__dirname, "..");

const runtimeFiles = [
  "meerkat_web_runtime.js",
  "meerkat_web_runtime_bg.wasm",
];

const sourceDirs = [
  path.resolve(webDir, "../../../sdks/web/wasm"),
  path.join(webDir, "node_modules", "@rkat", "web", "wasm"),
];

const sourceDir = sourceDirs.find((dir) =>
  runtimeFiles.every((file) => fs.existsSync(path.join(dir, file))),
);

if (!sourceDir) {
  const looked = sourceDirs.map((dir) => ` - ${dir}`).join("\n");
  console.error(
    "Unable to find @rkat/web runtime artifacts.\n" +
    "Looked in:\n" + looked + "\n" +
    "Install deps with `npm install` in this web directory, or build sdks/web wasm assets first.",
  );
  process.exit(1);
}

const destDirs = [
  path.join(webDir, "public", "meerkat-pkg"),
  path.join(webDir, "dist", "meerkat-pkg"),
];

for (const destDir of destDirs) {
  fs.mkdirSync(destDir, { recursive: true });
  for (const file of runtimeFiles) {
    const src = path.join(sourceDir, file);
    const dest = path.join(destDir, file);
    fs.copyFileSync(src, dest);
  }
}

console.log(
  `Synced runtime from ${sourceDir} -> ${destDirs.join(", ")}`,
);
