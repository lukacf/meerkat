import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

import { assertSpawnFrameBudget } from "./wasm-frames.mjs";
import { wasmBuildEnv } from "./wasm-rustflags.mjs";
import { assertWasmStack } from "./wasm-stack.mjs";
import { BUDGETED_BUILD } from "./wasm-stack-highwater.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SDK_DIR = path.resolve(__dirname, "..");
const OUT_DIR = path.resolve(process.env.MEERKAT_WEB_WASM_OUT_DIR || path.join(SDK_DIR, "wasm"));
const LOCK_DIR = path.resolve(
  process.env.MEERKAT_WEB_WASM_LOCK_DIR || path.join(SDK_DIR, ".wasm-build.lock"),
);
const LOCK_OWNER_FILE = path.join(LOCK_DIR, "owner.json");
const LOCK_HEARTBEAT_FILE = path.join(LOCK_DIR, "heartbeat");
const CRATE_DIR = path.resolve(SDK_DIR, "../../crates/meerkat-web-runtime");
const WORKSPACE_DIR = path.resolve(SDK_DIR, "../..");
const CACHE_MANIFEST = path.join(OUT_DIR, ".meerkat-wasm-build.json");
const WASM_PACK_BIN =
  process.env.RKAT_WASM_PACK_BIN || process.env.WASM_PACK || "wasm-pack";
const CARGO_BIN = process.env.CARGO || "cargo";
const CARGO_BIN_DIR = path.isAbsolute(CARGO_BIN) ? path.dirname(CARGO_BIN) : "";
const REQUIRED_OUTPUTS = [
  "meerkat_web_runtime.js",
  "meerkat_web_runtime_bg.wasm",
  "meerkat_web_runtime.d.ts",
];
const FORCE_REBUILD =
  process.env.MEERKAT_WEB_WASM_FORCE_REBUILD === "1" ||
  process.env.MEERKAT_WEB_WASM_CACHE === "0";
const BUILD_PROFILE = (() => {
  const value = process.env.MEERKAT_WEB_WASM_PROFILE ?? "release";
  if (value === "release" || value === "dev" || value === "profiling") {
    return value;
  }
  throw new Error(
    `invalid MEERKAT_WEB_WASM_PROFILE=${value}; expected release, dev, or profiling`,
  );
})();
// MEERKAT_WEB_WASM_OPT=0 skips the wasm-opt pass. wasm-opt spends many
// minutes on the release binary (most of the build); the browser suites that
// only need a working runtime (the example web suites in CI) skip it.
const WASM_OPT = (() => {
  const value = process.env.MEERKAT_WEB_WASM_OPT ?? "1";
  if (value === "0" || value === "1") {
    return value === "1";
  }
  throw new Error(`invalid MEERKAT_WEB_WASM_OPT=${value}; expected 0 or 1`);
})();
// Ambient rustflags are folded in ahead of the runtime's own and the result
// reaches Cargo as a `--config target.wasm32-unknown-unknown.rustflags`
// array, with no rustflags variable left to outrank it
// (scripts/wasm-rustflags.mjs), so no environment can drop `-zstack-size`.
const WASM_BUILD = wasmBuildEnv(process.env);
const WASM_RUSTFLAGS = WASM_BUILD.flags;
// The release build runs at opt-level "s" with 256 codegen units (both
// overridable through the environment). Measured on one tree, one build at a
// time through this script (wasm-opt capped at 8 cores), "s" was best or tied
// on every axis against 0, 1 and 2: a turn's wasm shadow-stack high-water of
// 133 KB (1.46 MB at 0, where LLVM colours no stack slots and every awaited
// future keeps its own slot in its parent's poll frame), a 32.3 MB wasm
// (10.1 MB gzip; 43.1 / 12.7 MB at 0, 60.5 / 16.2 MB at 2) and the shortest
// build, since wasm-opt's time follows the size of its input. The commit that
// chose opt-level 0 states no reason; the likely one is that the generated
// machine catalog then made rustc's optimizer run out of memory, since fixed
// at its root by chunking the catalog.
const RELEASE_CARGO_PROFILE_ENV =
  BUILD_PROFILE === "release"
    ? {
        CARGO_PROFILE_RELEASE_CODEGEN_UNITS:
          process.env.CARGO_PROFILE_RELEASE_CODEGEN_UNITS ?? "256",
        CARGO_PROFILE_RELEASE_OPT_LEVEL:
          process.env.CARGO_PROFILE_RELEASE_OPT_LEVEL ?? "s",
      }
    : {};
// How the module was built, recorded in the cache manifest the package ships
// (wasm/.meerkat-wasm-build.json). The packed-package smoke reads it to decide
// whether the release build's stack budget applies. The opt-level is the one
// this script sets (release) or `null` for Cargo's own profile default.
const BUILD_SETTINGS = {
  profile: BUILD_PROFILE,
  opt_level: RELEASE_CARGO_PROFILE_ENV.CARGO_PROFILE_RELEASE_OPT_LEVEL ?? null,
  codegen_units: RELEASE_CARGO_PROFILE_ENV.CARGO_PROFILE_RELEASE_CODEGEN_UNITS ?? null,
  wasm_opt: WASM_OPT,
};
// Lock timeout must exceed (wasm_build_seconds * max_parallel_tests). A cold
// wasm-pack build takes ~60s on M-series; the e2e-smoke lane can run ~5 browser
// tests that all compete for this lock. 15 minutes gives comfortable headroom
// without masking genuinely stuck builds.
const LOCK_TIMEOUT_MS = 15 * 60 * 1000;
const LOCK_RETRY_MS = 250;
const LOCK_STALE_MS = 2 * 60 * 1000;
const HEARTBEAT_MS = 5 * 1000;
const OWNER_TOKEN = randomUUID();

function pidIsAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

function wasmPackEnv(extraEnv = {}) {
  const env = {
    ...process.env,
    ...extraEnv,
  };
  if (CARGO_BIN_DIR) {
    env.CARGO = CARGO_BIN;
    env.PATH = `${CARGO_BIN_DIR}${path.delimiter}${env.PATH || ""}`;
  }
  return env;
}

async function readLockOwner() {
  try {
    return JSON.parse(await readFile(LOCK_OWNER_FILE, "utf8"));
  } catch {
    return null;
  }
}

async function heartbeatAgeMs() {
  try {
    const info = await stat(LOCK_HEARTBEAT_FILE);
    return Date.now() - info.mtimeMs;
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}

async function lockIsStale() {
  const owner = await readLockOwner();
  if (!owner) {
    return { stale: true, reason: "missing owner metadata" };
  }
  if (!pidIsAlive(owner.pid)) {
    return { stale: true, reason: `owner pid ${owner.pid} is not alive` };
  }
  const ageMs = await heartbeatAgeMs();
  if (ageMs > LOCK_STALE_MS) {
    return {
      stale: true,
      reason: `owner pid ${owner.pid} heartbeat is ${Math.round(ageMs)}ms old`,
    };
  }
  return { stale: false, reason: "" };
}

async function writeLockOwner() {
  const now = new Date().toISOString();
  await writeFile(
    LOCK_OWNER_FILE,
    JSON.stringify(
      {
        pid: process.pid,
        ppid: process.ppid,
        token: OWNER_TOKEN,
        started_at: now,
        cwd: process.cwd(),
      },
      null,
      2,
    ),
  );
  await writeFile(LOCK_HEARTBEAT_FILE, `${now}\n`);
}

function startHeartbeat() {
  const timer = setInterval(() => {
    writeFile(LOCK_HEARTBEAT_FILE, `${new Date().toISOString()}\n`).catch(() => {});
  }, HEARTBEAT_MS);
  timer.unref();
  return timer;
}

async function acquireLock() {
  const deadline = Date.now() + LOCK_TIMEOUT_MS;
  while (true) {
    try {
      await mkdir(LOCK_DIR);
      await writeLockOwner();
      return startHeartbeat();
    } catch (error) {
      if (error?.code !== "EEXIST") {
        throw error;
      }
      const { stale, reason } = await lockIsStale();
      if (stale) {
        console.warn(`removing stale wasm build lock: ${reason}`);
        await rm(LOCK_DIR, { recursive: true, force: true });
        continue;
      }
      if (Date.now() >= deadline) {
        throw new Error(`timed out waiting for wasm build lock at ${LOCK_DIR}`);
      }
      await delay(LOCK_RETRY_MS);
    }
  }
}

async function releaseLock() {
  const owner = await readLockOwner();
  if (owner?.token !== OWNER_TOKEN) {
    return;
  }
  await rm(LOCK_DIR, { recursive: true, force: true });
}

async function fileExists(filePath) {
  try {
    await stat(filePath);
    return true;
  } catch {
    return false;
  }
}

async function runCapture(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      ...options,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      reject(
        new Error(
          `failed to run ${command}: ${error.message}. Install wasm-pack or set RKAT_WASM_PACK_BIN/WASM_PACK to a wasm-pack binary.`,
        ),
      );
    });
    child.on("exit", (code, signal) => {
      if (code === 0) {
        resolve(stdout);
        return;
      }
      reject(
        new Error(
          signal
            ? `${command} terminated by signal ${signal}`
            : `${command} exited with code ${code ?? "unknown"}\n${stderr}`,
        ),
      );
    });
  });
}

async function collectPackageInputs(packageRoot) {
  const inputs = [path.join(packageRoot, "Cargo.toml")];
  const buildRs = path.join(packageRoot, "build.rs");
  if (await fileExists(buildRs)) {
    inputs.push(buildRs);
  }
  const srcDir = path.join(packageRoot, "src");
  if (!(await fileExists(srcDir))) {
    return inputs;
  }
  async function walk(dir) {
    const entries = await readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      const entryPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        await walk(entryPath);
      } else if (entry.isFile()) {
        inputs.push(entryPath);
      }
    }
  }
  await walk(srcDir);
  return inputs;
}

async function localCargoGraphInputs() {
  const metadataText = await runCapture(CARGO_BIN, ["metadata", "--format-version", "1"], {
    cwd: WORKSPACE_DIR,
  });
  const metadata = JSON.parse(metadataText);
  const packagesById = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const rootPackage = metadata.packages.find(
    (pkg) => path.resolve(pkg.manifest_path) === path.join(CRATE_DIR, "Cargo.toml"),
  );
  if (!rootPackage) {
    throw new Error(`could not find meerkat-web-runtime in cargo metadata from ${WORKSPACE_DIR}`);
  }
  const nodesById = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const seen = new Set();
  const stack = [rootPackage.id];
  const localPackageRoots = new Set();
  while (stack.length > 0) {
    const packageId = stack.pop();
    if (seen.has(packageId)) {
      continue;
    }
    seen.add(packageId);
    const pkg = packagesById.get(packageId);
    if (!pkg || pkg.source !== null) {
      continue;
    }
    localPackageRoots.add(path.dirname(pkg.manifest_path));
    const node = nodesById.get(packageId);
    for (const dep of node?.deps ?? []) {
      stack.push(dep.pkg);
    }
  }

  const inputs = [
    path.join(WORKSPACE_DIR, "Cargo.toml"),
    path.join(WORKSPACE_DIR, "Cargo.lock"),
  ];
  for (const packageRoot of [...localPackageRoots].sort()) {
    inputs.push(...(await collectPackageInputs(packageRoot)));
  }
  return [...new Set(inputs)].sort();
}

async function computeSourceHash() {
  const hash = createHash("sha256");
  hash.update("meerkat-web-runtime-wasm-v1\n");
  hash.update(`rustflags=${JSON.stringify(WASM_RUSTFLAGS)}\n`);
  hash.update(`profile=${BUILD_PROFILE}\n`);
  if (!WASM_OPT) {
    hash.update("wasm-opt=0\n");
  }
  for (const [key, value] of Object.entries(RELEASE_CARGO_PROFILE_ENV).sort()) {
    hash.update(`${key}=${value}\n`);
  }
  hash.update(`wasm-pack=${(await runCapture(WASM_PACK_BIN, ["--version"])).trim()}\n`);
  const inputs = await localCargoGraphInputs();
  for (const filePath of inputs) {
    const relativePath = path.relative(WORKSPACE_DIR, filePath);
    hash.update(`path:${relativePath}\n`);
    hash.update(await readFile(filePath));
    hash.update("\n");
  }
  return { hash: hash.digest("hex"), inputCount: inputs.length };
}

async function cacheIsValid(sourceHash) {
  if (FORCE_REBUILD) {
    return false;
  }
  for (const output of REQUIRED_OUTPUTS) {
    if (!(await fileExists(path.join(OUT_DIR, output)))) {
      return false;
    }
  }
  try {
    const manifest = JSON.parse(await readFile(CACHE_MANIFEST, "utf8"));
    // A module whose manifest records other (or no) build settings is
    // rebuilt, so the settings the package ships are the module's own.
    return (
      manifest.source_hash === sourceHash &&
      JSON.stringify(manifest.build ?? null) === JSON.stringify(BUILD_SETTINGS)
    );
  } catch {
    return false;
  }
}

// The built module must carry the stack it was linked for; the flags are not
// trusted to have reached the linker.
async function verifyWasmStack() {
  const wasmPath = path.join(OUT_DIR, "meerkat_web_runtime_bg.wasm");
  const stack = assertWasmStack(new Uint8Array(await readFile(wasmPath)), undefined, wasmPath);
  console.log(`meerkat web wasm stack: ${stack.stackBytes} bytes (${stack.layout})`);
}

// The release build's spawn wrapper frames must stay within budget
// (scripts/wasm-frames.mjs, issue #1230). They are read from the linked
// module Cargo wrote, before wasm-opt strips the function names they are
// found by; other builds' frames are not budgeted.
async function verifySpawnFrames() {
  if (
    BUILD_SETTINGS.profile !== BUDGETED_BUILD.profile ||
    BUILD_SETTINGS.opt_level !== BUDGETED_BUILD.opt_level
  ) {
    return;
  }
  const metadata = JSON.parse(
    await runCapture(CARGO_BIN, ["metadata", "--format-version", "1", "--no-deps"], {
      cwd: WORKSPACE_DIR,
    }),
  );
  const linked = path.join(
    metadata.target_directory,
    "wasm32-unknown-unknown",
    "release",
    "meerkat_web_runtime.wasm",
  );
  const { largest, instances } = assertSpawnFrameBudget(
    new Uint8Array(await readFile(linked)),
    undefined,
    linked,
  );
  console.log(
    `meerkat web wasm spawn frames: largest ${largest.frameBytes} bytes of ${instances} instances`,
  );
}

async function run() {
  const heartbeat = await acquireLock();
  try {
    const source = await computeSourceHash();
    if (await cacheIsValid(source.hash)) {
      await verifyWasmStack();
      console.log(
        `meerkat web wasm already current (${source.inputCount} source inputs, ${source.hash.slice(0, 12)})`,
      );
      return;
    }

    await rm(OUT_DIR, { recursive: true, force: true });

    await new Promise((resolve, reject) => {
      const profileArgs =
        BUILD_PROFILE === "release" ? [] : [`--${BUILD_PROFILE}`];
      const child = spawn(
        WASM_PACK_BIN,
        [
          "build",
          CRATE_DIR,
          "--target",
          "web",
          "--out-dir",
          OUT_DIR,
          ...profileArgs,
          ...(WASM_OPT ? [] : ["--no-opt"]),
          "--",
          ...WASM_BUILD.cargoArgs,
        ],
        {
          cwd: SDK_DIR,
          stdio: "inherit",
          env: wasmBuildEnv(wasmPackEnv(RELEASE_CARGO_PROFILE_ENV)).env,
        },
      );
      child.on("error", (error) => {
        reject(
          new Error(
            `failed to run ${WASM_PACK_BIN}: ${error.message}. Install wasm-pack or set RKAT_WASM_PACK_BIN/WASM_PACK to a wasm-pack binary.`,
          ),
        );
      });
      child.on("exit", (code, signal) => {
        if (code === 0) {
          resolve();
          return;
        }
        reject(
          new Error(
            signal
              ? `wasm-pack terminated by signal ${signal}`
              : `wasm-pack exited with code ${code ?? "unknown"}`,
          ),
        );
      });
    });

    await rm(path.join(OUT_DIR, ".gitignore"), { force: true });
    // Before the cache manifest: a module that fails is rebuilt next time.
    await verifyWasmStack();
    await verifySpawnFrames();
    await writeFile(
      CACHE_MANIFEST,
      JSON.stringify(
        {
          source_hash: source.hash,
          input_count: source.inputCount,
          built_at: new Date().toISOString(),
          build: BUILD_SETTINGS,
        },
        null,
        2,
      ),
    );
  } finally {
    clearInterval(heartbeat);
    await releaseLock();
  }
}

await run();
