// A build must never contain a provider key, whatever the environment holds.
import assert from "node:assert/strict";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SENTINELS = {
  ANTHROPIC_API_KEY: "sentinel-anthropic-key-7f3a",
  OPENAI_API_KEY: "sentinel-openai-key-91c2",
  GEMINI_API_KEY: "sentinel-gemini-key-b64e",
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
  const outDir = await mkdtemp(path.join(tmpdir(), "webcm-build-keys-"));
  const saved = {};
  for (const [name, value] of Object.entries(SENTINELS)) {
    saved[name] = process.env[name];
    process.env[name] = value;
  }
  try {
    await build({
      root: webRoot,
      logLevel: "silent",
      // The test needs the bundle only; skip copying the large public assets.
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
