// The provider proxy's allowed origins and Host check (proxy/index.mjs, proxy/cli.mjs): which
// requests it answers, which it refuses, and which reach the provider. The
// provider is a stub: `fetch` is replaced before the proxy starts, so no row
// sends a request off this process.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { request } from "node:http";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createProxyHandler, startProxy } from "../proxy/index.mjs";

const CLI = fileURLToPath(new URL("../proxy/cli.mjs", import.meta.url));
const PLACEHOLDER_KEY = "unit-test-placeholder";

for (const name of ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "PORT"]) {
  delete process.env[name];
}
process.env.ANTHROPIC_API_KEY = PLACEHOLDER_KEY;

// The stub provider answers with an allow-origin header of its own, which
// the proxy must not pass on.
const upstreamCalls = [];
globalThis.fetch = async (url, init) => {
  upstreamCalls.push({ url: String(url), key: init.headers.get("x-api-key") });
  return new Response('{"stub":true}', {
    status: 200,
    headers: { "content-type": "application/json", "access-control-allow-origin": "*" },
  });
};

const LOOPBACK_ORIGINS = [
  "http://localhost:5173",
  "http://127.0.0.1:8080",
  "http://[::1]:3000",
  "https://localhost",
];
const FOREIGN_ORIGINS = [
  "https://other.example",
  "http://localhost.other.example",
  "http://127.0.0.1.other.example:5173",
  "null",
];

function call(port, { method = "POST", path = "/anthropic/v1/messages", origin, host, fetchSite } = {}) {
  return new Promise((resolve, reject) => {
    const headers = { connection: "close" };
    if (origin !== undefined) headers.origin = origin;
    if (host !== undefined) headers.host = host;
    if (fetchSite !== undefined) headers["sec-fetch-site"] = fetchSite;
    if (method === "POST") headers["content-type"] = "application/json";
    const req = request({ host: "127.0.0.1", port, method, path, headers }, (res) => {
      let body = "";
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body }));
    });
    req.on("error", reject);
    req.end(method === "POST" ? "{}" : undefined);
  });
}

async function withProxy(opts, body) {
  const server = startProxy({ port: 0, ...opts });
  try {
    await once(server, "listening");
    upstreamCalls.length = 0;
    await body(server.address().port);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

test("by default a loopback origin is answered with its own origin", async () => {
  await withProxy({}, async (port) => {
    for (const origin of LOOPBACK_ORIGINS) {
      upstreamCalls.length = 0;
      const res = await call(port, { origin });
      assert.equal(res.status, 200, origin);
      assert.equal(res.headers["access-control-allow-origin"], origin);
      assert.match(res.headers.vary ?? "", /origin/i, origin);
      assert.deepEqual(upstreamCalls, [
        { url: "https://api.anthropic.com/v1/messages", key: PLACEHOLDER_KEY },
      ]);
    }
  });
});

test("by default another origin is refused, and the provider is not called", async () => {
  await withProxy({}, async (port) => {
    for (const origin of FOREIGN_ORIGINS) {
      for (const method of ["POST", "GET"]) {
        const res = await call(port, { origin, method });
        assert.equal(res.status, 403, `${method} ${origin}`);
        assert.equal(res.headers["access-control-allow-origin"], undefined, `${method} ${origin}`);
      }
    }
    assert.deepEqual(upstreamCalls, []);
  });
});

test("by default a preflight gets the allow headers only from a loopback origin", async () => {
  await withProxy({}, async (port) => {
    const local = await call(port, { method: "OPTIONS", origin: "http://localhost:5173" });
    assert.equal(local.status, 204);
    assert.equal(local.headers["access-control-allow-origin"], "http://localhost:5173");
    assert.match(local.headers["access-control-allow-headers"] ?? "", /x-api-key/);

    const foreign = await call(port, { method: "OPTIONS", origin: "https://other.example" });
    assert.equal(foreign.status, 403);
    for (const name of Object.keys(foreign.headers)) {
      assert.doesNotMatch(name, /^access-control-/, `refused preflight carries ${name}`);
    }
    assert.deepEqual(upstreamCalls, []);
  });
});

test("a request without an Origin header is served, with no allow-origin header", async () => {
  await withProxy({}, async (port) => {
    const res = await call(port, {});
    assert.equal(res.status, 200);
    assert.equal(res.body, '{"stub":true}');
    assert.equal(res.headers["access-control-allow-origin"], undefined);
    assert.equal(upstreamCalls.length, 1);
  });
});

test("an allowed origin given by the caller is the only origin served", async () => {
  await withProxy({ allowOrigin: "https://app.example" }, async (port) => {
    const allowed = await call(port, { origin: "https://app.example" });
    assert.equal(allowed.status, 200);
    assert.equal(allowed.headers["access-control-allow-origin"], "https://app.example");
    assert.equal(upstreamCalls.length, 1);

    upstreamCalls.length = 0;
    for (const origin of ["https://other.example", "http://localhost:5173"]) {
      const refused = await call(port, { origin });
      assert.equal(refused.status, 403, origin);
      assert.equal(refused.headers["access-control-allow-origin"], undefined, origin);
    }
    assert.deepEqual(upstreamCalls, []);

    const noOrigin = await call(port, {});
    assert.equal(noOrigin.status, 200);
    assert.equal(upstreamCalls.length, 1);
  });
});

test("an allowed origin of * serves every origin", async () => {
  await withProxy({ allowOrigin: "*" }, async (port) => {
    const res = await call(port, { origin: "https://other.example" });
    assert.equal(res.status, 200);
    assert.equal(res.headers["access-control-allow-origin"], "*");
    assert.equal(upstreamCalls.length, 1);
  });
});

test("createProxyHandler compares the Origin before the provider is called", async () => {
  const send = (origin, method = "POST") => new Request("http://127.0.0.1/anthropic/v1/messages", {
    method,
    headers: origin ? { origin } : {},
    body: method === "POST" ? "{}" : undefined,
  });

  // Without the option a handler allows loopback origins only, like the CLI.
  const byDefault = createProxyHandler("anthropic");
  upstreamCalls.length = 0;
  for (const method of ["POST", "OPTIONS"]) {
    const refused = await byDefault(send("https://other.example", method), "/v1/messages");
    assert.equal(refused.status, 403, method);
    assert.equal(refused.headers.get("access-control-allow-origin"), null, method);
  }
  const preflight = await byDefault(send("http://localhost:5173", "OPTIONS"), "/v1/messages");
  assert.equal(preflight.status, 204);
  assert.equal(preflight.headers.get("access-control-allow-origin"), "http://localhost:5173");
  assert.deepEqual(upstreamCalls, []);
  const local = await byDefault(send("http://localhost:5173"), "/v1/messages");
  assert.equal(local.status, 200);
  assert.equal(local.headers.get("access-control-allow-origin"), "http://localhost:5173");
  assert.equal(upstreamCalls.length, 1);

  const exact = createProxyHandler("anthropic", { allowOrigin: "https://app.example" });
  upstreamCalls.length = 0;
  const refused = await exact(send("https://other.example"), "/v1/messages");
  assert.equal(refused.status, 403);
  assert.equal(refused.headers.get("access-control-allow-origin"), null);
  assert.deepEqual(upstreamCalls, []);
  const served = await exact(send("https://app.example"), "/v1/messages");
  assert.equal(served.status, 200);
  assert.equal(served.headers.get("access-control-allow-origin"), "https://app.example");
  assert.equal(upstreamCalls.length, 1);

  // `*` keeps a handler open to every origin.
  const open = createProxyHandler("anthropic", { allowOrigin: "*" });
  upstreamCalls.length = 0;
  const any = await open(send("https://other.example"), "/v1/messages");
  assert.equal(any.status, 200);
  assert.equal(any.headers.get("access-control-allow-origin"), "*");
  assert.equal(upstreamCalls.length, 1);
});

test("an Origin of null is refused unless every origin is allowed", async () => {
  for (const opts of [{}, { allowOrigin: "https://app.example" }, { allowOrigin: "null" }]) {
    await withProxy(opts, async (port) => {
      for (const method of ["POST", "GET", "OPTIONS"]) {
        const res = await call(port, { origin: "null", method });
        assert.equal(res.status, 403, `${method} ${JSON.stringify(opts)}`);
        assert.equal(res.headers["access-control-allow-origin"], undefined);
      }
      assert.deepEqual(upstreamCalls, []);
    });
  }
  await withProxy({ allowOrigin: "*" }, async (port) => {
    const res = await call(port, { origin: "null" });
    assert.equal(res.status, 200);
    assert.equal(res.headers["access-control-allow-origin"], "*");
    assert.equal(upstreamCalls.length, 1);
  });
});

test("bound to a loopback address, a request with another Host is refused", async () => {
  await withProxy({}, async (port) => {
    const hosts = [
      "other.example",
      `other.example:${port}`,
      "localhost.other.example",
      `127.0.0.1.other.example:${port}`,
    ];
    for (const host of hosts) {
      for (const method of ["GET", "POST", "OPTIONS"]) {
        const res = await call(port, { method, path: "/anthropic/v1/models", host });
        assert.equal(res.status, 403, `${method} ${host}`);
      }
    }
    assert.deepEqual(upstreamCalls, []);
  });
});

test("bound to a loopback address, the loopback Host forms are served", async () => {
  await withProxy({}, async (port) => {
    const hosts = [
      "localhost",
      `localhost:${port}`,
      "127.0.0.1",
      `127.0.0.1:${port}`,
      "[::1]",
      `[::1]:${port}`,
    ];
    for (const host of hosts) {
      upstreamCalls.length = 0;
      const res = await call(port, { method: "GET", path: "/anthropic/v1/models", host });
      assert.equal(res.status, 200, host);
      assert.deepEqual(upstreamCalls, [
        { url: "https://api.anthropic.com/v1/models", key: PLACEHOLDER_KEY },
      ]);
    }
  });
});

test("bound to another address, the Host is not checked", async () => {
  await withProxy({ host: "0.0.0.0" }, async (port) => {
    const res = await call(port, { method: "GET", path: "/anthropic/v1/models", host: "other.example" });
    assert.equal(res.status, 200);
    assert.equal(upstreamCalls.length, 1);
  });
});

test("createProxyHandler checks the Host only when asked to", async () => {
  const get = (host) => new Request(`http://${host}/anthropic/v1/models`, { headers: { host } });

  const checked = createProxyHandler("anthropic", { loopbackHostOnly: true });
  upstreamCalls.length = 0;
  const refused = await checked(get("other.example"), "/v1/models");
  assert.equal(refused.status, 403);
  assert.deepEqual(upstreamCalls, []);
  const served = await checked(get("localhost:3100"), "/v1/models");
  assert.equal(served.status, 200);
  assert.equal(upstreamCalls.length, 1);

  const unchecked = createProxyHandler("anthropic");
  upstreamCalls.length = 0;
  const any = await unchecked(get("other.example"), "/v1/models");
  assert.equal(any.status, 200);
  assert.equal(upstreamCalls.length, 1);
});

// A browser sends Sec-Fetch-Site on every request, and no Origin on some
// GETs. Other callers send neither.
const MODELS = { method: "GET", path: "/anthropic/v1/models" };

test("a cross-site request without an Origin header is refused", async () => {
  for (const opts of [{}, { allowOrigin: "https://app.example" }]) {
    await withProxy(opts, async (port) => {
      for (const method of ["GET", "POST"]) {
        const res = await call(port, { ...MODELS, method, fetchSite: "cross-site" });
        assert.equal(res.status, 403, `${method} ${JSON.stringify(opts)}`);
        assert.equal(res.headers["access-control-allow-origin"], undefined);
      }
      assert.deepEqual(upstreamCalls, []);
    });
  }
});

test("a request without an Origin header from the same origin or site, or with no Sec-Fetch-Site, is served", async () => {
  await withProxy({}, async (port) => {
    for (const fetchSite of ["same-origin", "same-site", "none", undefined]) {
      upstreamCalls.length = 0;
      const res = await call(port, { ...MODELS, fetchSite });
      assert.equal(res.status, 200, String(fetchSite));
      assert.equal(upstreamCalls.length, 1, String(fetchSite));
    }
  });
});

test("the Origin decides a cross-site request that carries one", async () => {
  await withProxy({}, async (port) => {
    const local = await call(port, { origin: "http://127.0.0.1:5173", fetchSite: "cross-site" });
    assert.equal(local.status, 200);
    assert.equal(local.headers["access-control-allow-origin"], "http://127.0.0.1:5173");
    assert.equal(upstreamCalls.length, 1);
  });
});

test("an allowed origin of * serves a cross-site request without an Origin header", async () => {
  await withProxy({ allowOrigin: "*" }, async (port) => {
    const res = await call(port, { ...MODELS, fetchSite: "cross-site" });
    assert.equal(res.status, 200);
    assert.equal(upstreamCalls.length, 1);
  });
});

test("createProxyHandler refuses a cross-site request without an Origin header", async () => {
  const get = (fetchSite) => new Request("http://127.0.0.1/anthropic/v1/models", {
    headers: fetchSite ? { "sec-fetch-site": fetchSite } : {},
  });

  const byDefault = createProxyHandler("anthropic");
  upstreamCalls.length = 0;
  const refused = await byDefault(get("cross-site"), "/v1/models");
  assert.equal(refused.status, 403);
  assert.deepEqual(upstreamCalls, []);
  for (const fetchSite of ["same-origin", "none", undefined]) {
    const served = await byDefault(get(fetchSite), "/v1/models");
    assert.equal(served.status, 200, String(fetchSite));
  }
  assert.equal(upstreamCalls.length, 3);

  const open = createProxyHandler("anthropic", { allowOrigin: "*" });
  upstreamCalls.length = 0;
  const any = await open(get("cross-site"), "/v1/models");
  assert.equal(any.status, 200);
  assert.equal(upstreamCalls.length, 1);
});

// The CLI rows use preflights only: the proxy answers them itself.
async function withCli(args, body) {
  const child = spawn(process.execPath, [CLI, "--port", "0", ...args], {
    env: { ...process.env, ANTHROPIC_API_KEY: PLACEHOLDER_KEY },
    stdio: ["ignore", "pipe", "inherit"],
  });
  const exit = once(child, "close");
  try {
    let stdout = "";
    const port = await new Promise((resolve, reject) => {
      child.stdout.on("data", (chunk) => {
        stdout += chunk;
        const match = stdout.match(/listening on http:\/\/127\.0\.0\.1:(\d+)/);
        if (match) resolve(Number(match[1]));
      });
      exit.then(() => reject(new Error(`the proxy CLI exited before it listened: ${stdout}`)));
    });
    await body(port);
  } finally {
    child.kill();
    await exit;
  }
}

test("the CLI allows loopback origins by default", { timeout: 30_000 }, async () => {
  await withCli([], async (port) => {
    const local = await call(port, { method: "OPTIONS", origin: "http://127.0.0.1:5173" });
    assert.equal(local.status, 204);
    assert.equal(local.headers["access-control-allow-origin"], "http://127.0.0.1:5173");
    const foreign = await call(port, { method: "OPTIONS", origin: "https://other.example" });
    assert.equal(foreign.status, 403);
    assert.equal(foreign.headers["access-control-allow-origin"], undefined);
  });
});

test("--cors-origin names the one allowed origin, or * for any", { timeout: 30_000 }, async () => {
  await withCli(["--cors-origin", "https://app.example"], async (port) => {
    const allowed = await call(port, { method: "OPTIONS", origin: "https://app.example" });
    assert.equal(allowed.status, 204);
    assert.equal(allowed.headers["access-control-allow-origin"], "https://app.example");
    const refused = await call(port, { method: "OPTIONS", origin: "http://localhost:5173" });
    assert.equal(refused.status, 403);
    assert.equal(refused.headers["access-control-allow-origin"], undefined);
  });
  await withCli(["--cors-origin", "*"], async (port) => {
    const res = await call(port, { method: "OPTIONS", origin: "https://other.example" });
    assert.equal(res.status, 204);
    assert.equal(res.headers["access-control-allow-origin"], "*");
  });
});
