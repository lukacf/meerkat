import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SDK_DIR = path.resolve(__dirname, "..");
const LOCK_DIR = path.join(SDK_DIR, ".wasm-build.lock");
const LOCK_OWNER_FILE = path.join(LOCK_DIR, "owner.json");
const LOCK_HEARTBEAT_FILE = path.join(LOCK_DIR, "heartbeat");
const OUT_DIR = path.join(SDK_DIR, "wasm");
const CRATE_DIR = path.resolve(SDK_DIR, "../../meerkat-web-runtime");
const WORKSPACE_DIR = path.resolve(SDK_DIR, "../..");
const CACHE_MANIFEST = path.join(OUT_DIR, ".meerkat-wasm-build.json");
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
    child.on("error", reject);
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
  const metadataText = await runCapture("cargo", ["metadata", "--format-version", "1"], {
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
  hash.update(`rustflags=--cfg getrandom_backend="wasm_js"\n`);
  hash.update(`profile=${BUILD_PROFILE}\n`);
  hash.update(`wasm-pack=${(await runCapture("wasm-pack", ["--version"])).trim()}\n`);
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
    return manifest.source_hash === sourceHash;
  } catch {
    return false;
  }
}

async function run() {
  const heartbeat = await acquireLock();
  try {
    const source = await computeSourceHash();
    if (await cacheIsValid(source.hash)) {
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
        "wasm-pack",
        [
          "build",
          CRATE_DIR,
          "--target",
          "web",
          "--out-dir",
          OUT_DIR,
          ...profileArgs,
        ],
        {
          cwd: SDK_DIR,
          stdio: "inherit",
          env: {
            ...process.env,
            RUSTFLAGS: '--cfg getrandom_backend="wasm_js"',
          },
        },
      );
      child.on("error", reject);
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
    await writeFile(
      CACHE_MANIFEST,
      JSON.stringify(
        {
          source_hash: source.hash,
          input_count: source.inputCount,
          built_at: new Date().toISOString(),
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
