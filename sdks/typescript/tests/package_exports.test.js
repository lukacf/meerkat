import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = JSON.parse(readFileSync(path.join(packageDir, "package.json"), "utf8"));

/** Every file path the package entry points name: main, types and each exports target. */
function entryTargets() {
  const targets = new Set([manifest.main, manifest.types]);
  const walk = (value) => {
    if (typeof value === "string") targets.add(value);
    else if (value && typeof value === "object") Object.values(value).forEach(walk);
  };
  walk(manifest.exports);
  return [...targets].filter(Boolean).map((target) => path.posix.normalize(target));
}

describe("@rkat/sdk package", () => {
  it("packs every file its entry points reference", () => {
    const npm = process.platform === "win32" ? "npm.cmd" : "npm";
    const pack = spawnSync(npm, ["pack", "--dry-run", "--json", "--ignore-scripts"], {
      cwd: packageDir,
      encoding: "utf8",
      shell: process.platform === "win32",
    });
    assert.equal(pack.status, 0, pack.stderr);
    const packed = new Set(JSON.parse(pack.stdout)[0].files.map((file) => file.path));
    const missing = entryTargets().filter((target) => !packed.has(target));
    assert.deepEqual(missing, [], `entry points name files the package does not ship: ${missing}`);
  });

  it(
    "loads through require() as well as import",
    { skip: !process.features.require_module && "this Node cannot require() an ES module" },
    () => {
      const load = spawnSync(
        process.execPath,
        ["-e", "process.stdout.write(typeof require('@rkat/sdk').MeerkatClient)"],
        { cwd: packageDir, encoding: "utf8" },
      );
      assert.equal(load.status, 0, load.stderr);
      assert.equal(load.stdout, "function");
    },
  );
});
