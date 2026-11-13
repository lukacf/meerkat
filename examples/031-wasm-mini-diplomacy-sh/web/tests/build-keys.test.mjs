// A build must never contain a provider key, whatever the environment holds.
// The page pre-fills its key fields from VITE_-prefixed variables; Vite would
// write every exposed variable into the bundle, so the build exposes none of
// them (see vite.config.ts).
import assert from "node:assert/strict";
import { mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SENTINELS = {
  VITE_ANTHROPIC_API_KEY: "sentinel-anthropic-key-7f3a",
  VITE_OPENAI_API_KEY: "sentinel-openai-key-91c2",
  VITE_GEMINI_API_KEY: "sentinel-gemini-key-b64e",
  // The page reads its variables by name from the whole environment object,
  // so any exposed variable would be written out, not only the three above.
  VITE_UNRELATED_SETTING: "sentinel-unrelated-value-2d18",
};

async function filesUnder(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await filesUnder(full)));
    else out.push(full);
  }
  return out;
}

test("vite build inlines no provider key from the environment", async () => {
  const outDir = await mkdtemp(path.join(tmpdir(), "diplomacy-build-keys-"));
  const saved = {};
  for (const [name, value] of Object.entries(SENTINELS)) {
    saved[name] = process.env[name];
    process.env[name] = value;
  }
  try {
    await build({
      root: webRoot,
      logLevel: "silent",
      // The test needs the bundle only; skip copying the public assets.
      publicDir: false,
      build: { outDir, emptyOutDir: true },
    });
    const files = await filesUnder(outDir);
    assert.ok(files.some((file) => file.endsWith(".js")), "the build wrote a script");
    for (const file of files) {
      const text = await readFile(file, "latin1");
      for (const [name, value] of Object.entries(SENTINELS)) {
        assert.ok(!text.includes(value), `${name} is inlined in ${path.relative(outDir, file)}`);
      }
    }
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    await rm(outDir, { recursive: true, force: true });
  }
});

test("vite build inlines no provider key from an env file", async () => {
  // Vite also reads `.env`, `.env.local` and the mode's files. Point it at a
  // scratch directory holding one, so the developer's own files are untouched.
  const outDir = await mkdtemp(path.join(tmpdir(), "diplomacy-build-keys-"));
  const envDir = await mkdtemp(path.join(tmpdir(), "diplomacy-build-env-"));
  const saved = {};
  for (const name of Object.keys(SENTINELS)) {
    saved[name] = process.env[name];
    delete process.env[name];
  }
  try {
    const lines = Object.entries(SENTINELS).map(([name, value]) => `${name}=${value}`);
    await writeFile(path.join(envDir, ".env.local"), lines.join("\n") + "\n");
    await build({
      root: webRoot,
      envDir,
      logLevel: "silent",
      publicDir: false,
      build: { outDir, emptyOutDir: true },
    });
    const files = await filesUnder(outDir);
    assert.ok(files.some((file) => file.endsWith(".js")), "the build wrote a script");
    for (const file of files) {
      const text = await readFile(file, "latin1");
      for (const [name, value] of Object.entries(SENTINELS)) {
        assert.ok(!text.includes(value), `${name} from .env.local is inlined in ${path.relative(outDir, file)}`);
      }
    }
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value !== undefined) process.env[name] = value;
    }
    await rm(outDir, { recursive: true, force: true });
    await rm(envDir, { recursive: true, force: true });
  }
});
