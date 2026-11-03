import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const examplesRoot = resolve(here, "..");

const result = spawnSync(
  process.execPath,
  [
    join(examplesRoot, "node_modules/tsx/dist/cli.mjs"),
    join(here, "run.ts"),
    "--smoke",
    "--allow-empty-targets",
    "--demo-llm",
    "--skip-build",
  ],
  {
    cwd: examplesRoot,
    env: process.env,
    encoding: "utf8",
  },
);

process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
if (result.status !== 0) {
  throw new Error(`MDM smoke failed with status ${result.status}`);
}
if (!result.stdout.includes("[mdm-smoke] ok")) {
  throw new Error("MDM smoke did not print success marker");
}
