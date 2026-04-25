import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
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

async function run() {
  const heartbeat = await acquireLock();
  try {
    await rm(OUT_DIR, { recursive: true, force: true });

    await new Promise((resolve, reject) => {
      const child = spawn(
        "wasm-pack",
        ["build", CRATE_DIR, "--target", "web", "--out-dir", OUT_DIR],
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
  } finally {
    clearInterval(heartbeat);
    await releaseLock();
  }
}

await run();
