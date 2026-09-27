// Smoke-test the @rkat/web package exactly as npm ships it.
//
// Every @rkat/web from 0.8.30 through 0.8.44 passed its tests and still
// crashed on its first turn for users: the tests ran against a runtime built
// with an 8 MiB wasm stack, while the release job's ambient RUSTFLAGS made the
// published one fall back to 1 MiB. This smoke takes the packed tarball, so it
// checks the artifact users install:
//   1. the tarball carries the runtime, the TypeScript entry and the proxy;
//   2. the packed wasm has the required stack (typed parse, scripts/wasm-stack.mjs);
//   3. one turn runs end to end in Node through the packed JS and wasm, with
//      `fetch` stubbed to an Anthropic SSE stream (as browser_contract.rs does).
//
// Usage:
//   node scripts/smoke-packed-package.mjs <rkat-web-X.Y.Z.tgz>
//   node scripts/smoke-packed-package.mjs --pack   (packs sdks/web first)

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { access, mkdir, mkdtemp, readFile, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { assertWasmStack } from "./wasm-stack.mjs";

const SDK_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REQUIRED_FILES = [
  "wasm/meerkat_web_runtime_bg.wasm",
  "wasm/meerkat_web_runtime.js",
  "dist/index.js",
  "proxy/cli.mjs",
];
const REPLY = "packed runtime turn ok";

function anthropicReplyStream(text) {
  return [
    { type: "message_start", message: { usage: { input_tokens: 1, output_tokens: 0 } } },
    { type: "content_block_start", content_block: { type: "text", text: "" } },
    { type: "content_block_delta", delta: { type: "text_delta", text } },
    { type: "content_block_stop" },
    { type: "message_delta", usage: { output_tokens: 4 }, delta: { stop_reason: "end_turn" } },
    { type: "message_stop" },
  ]
    .map((event) => `data: ${JSON.stringify(event)}\n\n`)
    .join("");
}

function installFetchStub() {
  const requests = [];
  globalThis.fetch = async (input, init) => {
    requests.push({ url: String(input?.url ?? input), method: init?.method ?? "GET" });
    const response = new Response(anthropicReplyStream(REPLY), {
      status: 200,
      headers: { "content-type": "text/event-stream" },
    });
    Object.defineProperty(response, "url", {
      value: "https://example.test/anthropic/v1/messages",
    });
    return response;
  };
  return requests;
}

async function packSdk(destination) {
  const output = execFileSync("npm", ["pack", "--ignore-scripts", "--pack-destination", destination], {
    cwd: SDK_DIR,
    encoding: "utf8",
  });
  const lines = output.split("\n").map((line) => line.trim()).filter(Boolean);
  const packfile = lines.at(-1);
  assert.ok(packfile, "npm pack reported no tarball");
  return path.join(destination, packfile);
}

async function unpack(tarball, root) {
  execFileSync("tar", ["-xzf", tarball, "-C", root]);
  const scope = path.join(root, "node_modules", "@rkat");
  await mkdir(scope, { recursive: true });
  const packageDir = path.join(scope, "web");
  await rename(path.join(root, "package"), packageDir);
  return packageDir;
}

async function main(argv) {
  const args = argv.slice(2);
  const root = await mkdtemp(path.join(tmpdir(), "rkat-web-packed-"));
  try {
    const tarball = args[0] === "--pack" ? await packSdk(root) : path.resolve(args[0] ?? "");
    if (!args[0]) {
      throw new Error("usage: smoke-packed-package.mjs <rkat-web-X.Y.Z.tgz> | --pack");
    }
    const packageDir = await unpack(tarball, root);

    for (const file of REQUIRED_FILES) {
      await access(path.join(packageDir, file)).catch(() => {
        throw new Error(`${path.basename(tarball)} is missing package/${file}`);
      });
    }

    const wasmPath = path.join(packageDir, "wasm", "meerkat_web_runtime_bg.wasm");
    const wasmBytes = await readFile(wasmPath);
    const stack = assertWasmStack(new Uint8Array(wasmBytes), undefined, `${path.basename(tarball)} wasm`);
    console.log(`packed wasm stack: ${stack.stackBytes} bytes (${stack.layout})`);

    const web = await import(pathToFileURL(path.join(packageDir, "dist", "index.js")).href);
    assert.ok(web.MeerkatRuntime && web.Session, "missing @rkat/web exports");
    const rawWasm = await import(
      pathToFileURL(path.join(packageDir, "wasm", "meerkat_web_runtime.js")).href
    );
    const wasm = { ...rawWasm, default: async () => rawWasm.default({ module_or_path: wasmBytes }) };

    const requests = installFetchStub();
    const runtime = await web.MeerkatRuntime.init(wasm, {
      anthropicApiKey: "sk-test",
      anthropicBaseUrl: "https://example.test/anthropic",
      model: "claude-sonnet-4-5",
    });
    try {
      const session = runtime.createSession({
        model: "claude-sonnet-4-5",
        apiKey: "sk-test",
        anthropicBaseUrl: "https://example.test/anthropic",
      });
      const result = await session.turn("Say the smoke phrase.");
      assert.equal(result.text, REPLY, "the packed runtime's turn returned the stubbed reply");
      assert.ok(requests.length >= 1, "the turn reached the provider through fetch");
    } finally {
      runtime.destroy();
    }
    console.log(`packed @rkat/web ran one turn end to end (${requests.length} provider request(s))`);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

main(process.argv).catch((error) => {
  console.error(`packed @rkat/web smoke failed: ${error?.stack ?? error}`);
  process.exit(1);
});
