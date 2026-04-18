import { mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SDK_DIR = path.resolve(__dirname, "..");
const LOCK_DIR = path.join(SDK_DIR, ".wasm-build.lock");
const OUT_DIR = path.join(SDK_DIR, "wasm");
const CRATE_DIR = path.resolve(SDK_DIR, "../../meerkat-web-runtime");
// Lock timeout must exceed (wasm_build_seconds * max_parallel_tests). A cold
// wasm-pack build takes ~60s on M-series; the e2e-smoke lane can run ~5 browser
// tests that all compete for this lock. 15 minutes gives comfortable headroom
// without masking genuinely stuck builds.
const LOCK_TIMEOUT_MS = 15 * 60 * 1000;
const LOCK_RETRY_MS = 250;

async function acquireLock() {
  const deadline = Date.now() + LOCK_TIMEOUT_MS;
  while (true) {
    try {
      await mkdir(LOCK_DIR);
      return;
    } catch (error) {
      if (error?.code !== "EEXIST") {
        throw error;
      }
      if (Date.now() >= deadline) {
        throw new Error(`timed out waiting for wasm build lock at ${LOCK_DIR}`);
      }
      await delay(LOCK_RETRY_MS);
    }
  }
}

async function releaseLock() {
  await rm(LOCK_DIR, { recursive: true, force: true });
}

async function run() {
  await acquireLock();
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
    await releaseLock();
  }
}

await run();
