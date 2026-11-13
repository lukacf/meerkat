// A build must never contain a provider key, whatever the environment holds.
// The page pre-fills its key fields from VITE_-prefixed variables; Vite would
// write every exposed variable into the bundle, so the build exposes none of
// them (see vite.config.ts).
import assert from "node:assert/strict";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
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
  VITE_RKAT_ANTHROPIC_API_KEY: "sentinel-rkat-anthropic-key-c0d5",
  VITE_RKAT_OPENAI_API_KEY: "sentinel-rkat-openai-key-44ae",
  VITE_RKAT_GEMINI_API_KEY: "sentinel-rkat-gemini-key-e913",
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
  const outDir = await mkdtemp(path.join(tmpdir(), "office-build-keys-"));
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
