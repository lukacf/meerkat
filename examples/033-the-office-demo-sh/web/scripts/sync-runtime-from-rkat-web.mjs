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

function syncDirectory(source, dest) {
  fs.rmSync(dest, { recursive: true, force: true });
  fs.mkdirSync(dest, { recursive: true });

  for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
    if (entry.name === ".gitignore") continue;
    const src = path.join(source, entry.name);
    const dst = path.join(dest, entry.name);
    if (entry.isDirectory()) {
      syncDirectory(src, dst);
    } else {
      fs.copyFileSync(src, dst);
    }
  }
}

for (const destDir of destDirs) {
  syncDirectory(sourceDir, destDir);
}

console.log(
  `Synced runtime from ${sourceDir} -> ${destDirs.join(", ")}`,
);
