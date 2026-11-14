// The standalone provider proxy's listener (proxy/index.mjs, proxy/cli.mjs):
// the address it binds by default, the `host` option and `--host` flag, and
// the address it prints. No row sends a request the proxy forwards.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { request } from "node:http";
import { createServer } from "node:net";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { startProxy } from "../proxy/index.mjs";

const CLI = fileURLToPath(new URL("../proxy/cli.mjs", import.meta.url));
const PLACEHOLDER_KEY = "unit-test-placeholder";
const PROVIDER_KEYS = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"];

// The proxy needs one configured provider to start; it never gets a real key.
for (const name of PROVIDER_KEYS) delete process.env[name];
delete process.env.PORT;
process.env.ANTHROPIC_API_KEY = PLACEHOLDER_KEY;

async function closed(server) {
  await new Promise((resolve) => server.close(resolve));
}

function startCli(args) {
  const env = { ...process.env, ANTHROPIC_API_KEY: PLACEHOLDER_KEY };
  const child = spawn(process.execPath, [CLI, ...args], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const output = { stdout: "", stderr: "" };
  let exited = false;
  let waiters = [];
  const wake = () => {
    const pending = waiters;
    waiters = [];
    for (const resolve of pending) resolve();
  };
  child.stdout.on("data", (chunk) => { output.stdout += chunk; wake(); });
  child.stderr.on("data", (chunk) => { output.stderr += chunk; wake(); });
  child.on("close", () => { exited = true; wake(); });
  return {
    output,
    async until(stream, pattern) {
      for (;;) {
        const match = output[stream].match(pattern);
        if (match) return match;
        if (exited) {
          assert.fail(`the proxy CLI exited before ${stream} matched ${pattern}: ${JSON.stringify(output)}`);
        }
        await new Promise((resolve) => waiters.push(resolve));
      }
    },
    async stop() {
      if (exited) return;
      child.kill();
      await once(child, "close");
    },
  };
}

// An unknown path is answered by the proxy itself, without a provider call.
function statusOf(url) {
  return new Promise((resolve, reject) => {
    const req = request(new URL("/unknown-provider", url), (res) => {
      res.resume();
      res.on("end", () => resolve(res.statusCode));
    });
    req.on("error", reject);
    req.end();
  });
}

test("startProxy listens on the loopback interface by default", async () => {
  const server = startProxy({ port: 0 });
  try {
    await once(server, "listening");
    const bound = server.address();
    assert.equal(bound.address, "127.0.0.1");
    assert.notEqual(bound.port, 0);
  } finally {
    await closed(server);
  }
});

test("startProxy binds the address its host option names", async () => {
  const server = startProxy({ port: 0, host: "0.0.0.0" });
  try {
    await once(server, "listening");
    assert.equal(server.address().address, "0.0.0.0");
  } finally {
    await closed(server);
  }
});

function freePort() {
  return new Promise((resolve) => {
    const probe = createServer();
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

test("startProxy binds a port given as a string of digits", async () => {
  const port = await freePort();
  for (const [given, expected] of [[String(port), port], [` ${port} `, port]]) {
    const server = startProxy({ port: given });
    try {
      await once(server, "listening");
      assert.equal(server.address().port, expected, JSON.stringify(given));
    } finally {
      await closed(server);
    }
  }
  const any = startProxy({ port: "0" });
  try {
    await once(any, "listening");
    assert.notEqual(any.address().port, 0);
    assert.notEqual(any.address().port, 3100);
  } finally {
    await closed(any);
  }
});

test("the CLI prints the loopback address it bound, and no warning", { timeout: 30_000 }, async () => {
  const cli = startCli(["--port", "0"]);
  try {
    const [, printed] = await cli.until("stdout", /listening on (\S+)/);
    const url = new URL(printed);
    assert.equal(url.hostname, "127.0.0.1");
    assert.ok(Number(url.port) > 0, `printed URL has the bound port: ${printed}`);
    assert.equal(await statusOf(url), 404);
    assert.doesNotMatch(cli.output.stderr, /does not check who calls it/);
  } finally {
    await cli.stop();
  }
});

test("--host binds the given address, and a non-loopback address prints a warning", { timeout: 30_000 }, async () => {
  const cli = startCli(["--port", "0", "--host", "0.0.0.0"]);
  try {
    const [, printed] = await cli.until("stdout", /listening on (\S+)/);
    const url = new URL(printed);
    assert.equal(url.hostname, "0.0.0.0");
    assert.ok(Number(url.port) > 0, `printed URL has the bound port: ${printed}`);
    await cli.until("stderr", /does not check who calls it/);
  } finally {
    await cli.stop();
  }
});
