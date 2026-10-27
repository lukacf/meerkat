// Actual HTTP loopback fixtures. No Home Assistant instance, credential, or physical effect claim.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import net from "node:net";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import {
  HomeAssistantOperations, HomeAssistantError, HomeAssistantCancelled,
} from "@rkat/toolkit/connectors/home-assistant";

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
async function bounded(promise, label, milliseconds = 5_000) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`fixture deadline: ${label}`)), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
}
function reply(response, value, { status = 200, headers = {}, raw } = {}) {
  const body = raw ?? Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "Content-Type": "application/json", "Content-Length": String(body.length),
    Connection: "close", ...headers,
  });
  response.end(body);
}
function state(entity = "light.kitchen", value = "on") {
  return {
    entity_id: entity, state: value,
    attributes: { nested: { unicode: "kök", values: [1, false, null] } },
    last_changed: "2026-09-30T12:00:00Z", context: { id: "provider-context", parent_id: null },
  };
}

class Server {
  requests = [];
  tasks = [];
  sockets = new Set();
  socketClosures = [];
  releases = [];
  joined = false;
  closing = false;
  accepted = deferred();
  constructor(handler) {
    this.server = http.createServer((request, response) => {
      // Retain every actual handler immediately, including requests whose bodies fail.
      const task = (async () => {
        const chunks = [];
        for await (const chunk of request) chunks.push(chunk);
        const item = { method: request.method, target: request.url, headers: request.headers,
          body: Buffer.concat(chunks).toString("utf8") };
        this.requests.push(item);
        await handler(item, response);
      })();
      // Observe rejection immediately, then rethrow all errors after joining every owner.
      this.tasks.push(task.then(() => null, (error) => error));
    });
    this.server.on("connection", (socket) => {
      this.sockets.add(socket);
      this.socketClosures.push(new Promise((resolve) => {
        socket.once("close", () => { this.sockets.delete(socket); resolve(); });
      }));
      socket.on("error", () => {}); // Disconnect is an intentional input in several cases.
      this.accepted.resolve();
      if (this.closing) socket.destroy();
    });
  }
  async start() {
    this.server.listen(0, "127.0.0.1");
    await once(this.server, "listening");
    this.url = `http://127.0.0.1:${this.server.address().port}/`;
    return this;
  }
  async close() {
    this.closing = true;
    const errors = [];
    // Stop admission before waiting. All previously accepted sockets are already retained.
    const closed = new Promise((resolve, reject) => this.server.close((error) => error ? reject(error) : resolve()));
    for (const release of this.releases) release.resolve();
    for (const socket of this.sockets) socket.destroy();
    try { await bounded(closed, "listener and socket retirement"); } catch (error) { errors.push(error); }
    try { await bounded(Promise.all(this.socketClosures), "accepted socket joins"); }
    catch (error) { errors.push(error); }
    try {
      const outcomes = await bounded(Promise.all(this.tasks), "handler joins");
      errors.push(...outcomes.filter((error) => error !== null));
    } catch (error) { errors.push(error); }
    this.joined = errors.length === 0 && this.sockets.size === 0;
    if (errors.length) throw new AggregateError(errors, "fixture cleanup failures");
  }
}

async function fixture(handler, body) {
  const server = await new Server(handler).start();
  let failure;
  try { await body(server); } catch (error) { failure = error; }
  try { await server.close(); } catch (error) {
    throw new AggregateError(failure ? [failure, error] : [error], "fixture body and cleanup");
  }
  if (failure) throw failure;
  assert.equal(server.joined, true);
  return server;
}
function context(server, account = "first", prefix = "") {
  // The host selects the account. These are local fixture values, not real credentials.
  return { baseUrl: server.url + prefix, fetch: (request) => {
    assert.equal(request.redirect, "manual");
    assert.equal(request.credentials, "omit");
    const headers = new Headers(request.headers);
    headers.set("Authorization", `Bearer fixture-${account}`);
    return fetch(new Request(request, { headers }));
  } };
}
function errorIs(kind, outcome = "unknown", status = null) {
  return (error) => {
    assert.ok(error instanceof HomeAssistantError);
    assert.equal(error.kind, kind);
    assert.equal(error.writeOutcome, outcome);
    assert.equal(error.statusCode, status);
    assert.equal(error.cause, undefined);
    return true;
  };
}

test("fixture joins an accepted socket even before HTTP headers", { timeout: 10_000 }, async () => {
  const server = await fixture(() => assert.fail("no HTTP request expected"), async (server) => {
    const socket = net.connect({ host: "127.0.0.1", port: server.server.address().port });
    socket.on("error", () => {});
    const closed = new Promise((resolve) => socket.once("close", resolve));
    try { await bounded(server.accepted.promise, "accept before headers"); }
    finally { socket.destroy(); await bounded(closed, "client socket join"); }
  });
  assert.equal(server.requests.length, 0);
  assert.ok(server.socketClosures.length > 0);
});

test("state and discovery preserve exact path, selected context, nested provider data", async () => {
  const metadata = [{ domain: "light", services: { turn_on: {
    name: "On", target: { entity: [{ domain: "light" }] },
    fields: { brightness: { selector: { number: { min: 0, max: 255 } } } },
  } }, extra: false }];
  await fixture((request, response) => reply(response,
    request.target.endsWith("/services") ? metadata : state()), async (server) => {
    const selected = context(server, "selected", "proxy/");
    const borrowedFetch = selected.fetch;
    const operations = new HomeAssistantOperations(selected);
    selected.baseUrl = "http://127.0.0.1:1/wrong/";
    selected.fetch = () => { throw new Error("must not replace selected transport"); };
    assert.deepEqual(await operations.getState("light.kitchen"), state());
    assert.deepEqual(await operations.listServices(), metadata);
    assert.deepEqual(server.requests.map((item) => item.target),
      ["/proxy/api/states/light.kitchen", "/proxy/api/services"]);
    for (const item of server.requests) {
      assert.equal(item.method, "GET");
      assert.equal(item.headers.authorization, "Bearer fixture-selected");
      assert.equal(item.headers["accept-encoding"], "identity");
    }
    // The borrowed transport remains usable; operations never close it.
    const borrowedResponse = await borrowedFetch(new Request(server.url, { redirect: "manual", credentials: "omit" }));
    assert.equal(borrowedResponse.status, 200);
    assert.deepEqual(await borrowedResponse.json(), state());
  });
});

test("two concurrent account closures on one instance keep request and receipt association", async () => {
  const firstEntered = deferred();
  const secondEntered = deferred();
  await fixture(async (request, response) => {
    if (request.headers.authorization === "Bearer fixture-first") {
      firstEntered.resolve(); await bounded(secondEntered.promise, "second account entered");
      reply(response, [state("light.first", "first")]);
    } else {
      secondEntered.resolve(); await bounded(firstEntered.promise, "first account entered");
      reply(response, [state("light.second", "second")]);
    }
  }, async (server) => {
    const first = new HomeAssistantOperations(context(server, "first"));
    const second = new HomeAssistantOperations(context(server, "second"));
    const outcomes = await Promise.allSettled([
      first.callService("light", "turn_on", { data: { entity_id: "light.first" } }),
      second.callService("light", "turn_on", { data: { entity_id: "light.second" } }),
    ]);
    const errors = outcomes.filter((result) => result.status === "rejected");
    if (errors.length) throw new AggregateError(errors.map((result) => result.reason), "account operations failed");
    const [a, b] = outcomes.map((result) => result.value);
    assert.equal(a.changedStates[0].state, "first");
    assert.equal(b.changedStates[0].state, "second");
    assert.equal(server.requests.length, 2);
    for (const item of server.requests) {
      const id = JSON.parse(item.body).entity_id;
      assert.equal(item.headers.authorization, `Bearer fixture-${id.split(".")[1]}`);
    }
  });
});

test("service calls preserve false/null data, both response profiles, exact return_response query", async () => {
  const changed = [state("light.second"), state("light.first", "off")];
  const payload = { entity_id: "light.kitchen", toggle: false, missing: null, number: 3.5, items: [false, null] };
  const serviceResponse = { "weather.home": { forecast: [{ condition: null, sunny: false }] } };
  let index = 0;
  await fixture((request, response) => {
    assert.deepEqual(JSON.parse(request.body), payload);
    reply(response, index++ === 0 ? changed : { changed_states: changed, service_response: index === 2 ? serviceResponse : null });
  }, async (server) => {
    const operations = new HomeAssistantOperations(context(server));
    assert.deepEqual(await operations.callService("light", "turn_on", { data: payload }),
      { changedStates: changed, responseRequested: false, serviceResponse: null });
    assert.deepEqual(await operations.callService("light", "turn_on", { data: payload, returnResponse: true }),
      { changedStates: changed, responseRequested: true, serviceResponse });
    assert.deepEqual(await operations.callService("light", "turn_on", { data: payload, returnResponse: true }),
      { changedStates: changed, responseRequested: true, serviceResponse: null });
    assert.deepEqual(server.requests.map((item) => item.target), [
      "/api/services/light/turn_on", "/api/services/light/turn_on?return_response", "/api/services/light/turn_on?return_response",
    ]);
    assert.ok(server.requests.every((item) => item.method === "POST"));
  });
});

test("runtime input refusals produce zero transport attempts and do not invoke getters", async () => {
  await fixture(() => assert.fail("invalid input sent HTTP"), async (server) => {
    let attempts = 0;
    const selected = context(server);
    const operations = new HomeAssistantOperations({ baseUrl: selected.baseUrl,
      fetch: (request) => { attempts++; return selected.fetch(request); } });
    for (const name of ["", "Light", "light/other", "light?x", "light\n", "light\r", "light%2e", "a.b"]) {
      await assert.rejects(operations.callService(name, "turn_on"), TypeError);
      await assert.rejects(operations.callService("light", name), TypeError);
    }
    for (const name of ["light", "light..x", "light.X", "light.x\n", "../light.x"]) {
      await assert.rejects(operations.getState(name), TypeError);
    }
    const cyclic = {}; cyclic.self = cyclic;
    let deep = {}; for (let index = 0; index < 129; index++) deep = { child: deep };
    const sparse = []; sparse.length = 2; sparse[1] = null;
    const extraArray = []; extraArray.extra = "lost";
    let getterCalls = 0;
    const getter = Object.defineProperty({}, "secret", { enumerable: true, get() { getterCalls++; throw new Error("secret"); } });
    const nonEnumerable = Object.defineProperty({}, "lost", { value: true });
    for (const data of [null, [], false, { x: undefined }, { x: Infinity }, { x: NaN }, { x: -0 },
      { x: 9_007_199_254_740_992 }, { x: 1n }, { x: () => {} }, { x: Symbol() }, { [Symbol()]: "lost" },
      { x: sparse }, { x: extraArray }, getter, nonEnumerable, { x: new Date() }, cyclic, deep,
      { toJSON() { throw new Error("must not execute"); } }]) {
      await assert.rejects(operations.callService("light", "turn_on", { data }), TypeError);
    }
    for (const returnResponse of [null, 0, 1, "false"]) {
      await assert.rejects(operations.callService("light", "turn_on", { returnResponse }), TypeError);
    }
    for (const signal of [null, {}, "cancel"]) {
      await assert.rejects(operations.callService("light", "turn_on", { signal }), TypeError);
    }
    assert.equal(getterCalls, 0);
    assert.equal(attempts, 0);
    assert.equal(server.requests.length, 0);
    assert.throws(() => new HomeAssistantOperations({ ...selected, baseUrl: server.url + "?return_response" }), TypeError);
    assert.throws(() => new HomeAssistantOperations({ ...selected, baseUrl: server.url.replace("http://", "http://user:secret@") }), TypeError);
    assert.throws(() => new HomeAssistantOperations({ ...selected, baseUrl: server.url + "x/../" }), TypeError);
    for (const timeoutMs of [0, -1, NaN, Infinity, null, true, 2_147_483_648]) {
      assert.throws(() => new HomeAssistantOperations(selected, { timeoutMs }), TypeError);
    }
    for (const maxResponseBytes of [0, -1, 1.1, null, true, Infinity]) {
      assert.throws(() => new HomeAssistantOperations(selected, { maxResponseBytes }), TypeError);
    }
  });
});

test("redirect refusal never enters the other instance and never retries", async () => {
  await fixture(() => assert.fail("redirect destination entered"), async (target) => {
    await fixture((_request, response) => reply(response, null, { status: 307, headers: { Location: target.url } }), async (source) => {
      const operations = new HomeAssistantOperations(context(source));
      await assert.rejects(operations.callService("light", "turn_on"), errorIs("redirect", "unknown", 307));
      assert.equal(source.requests.length, 1);
      assert.equal(target.requests.length, 0);
    });
  });
});

test("all attempted HTTP refusals have unknown effect, secret-safe status and retry metadata", async () => {
  for (const [status, kind] of [[400, "http_status"], [401, "authentication"], [403, "permission"],
    [404, "not_found"], [429, "rate_limited"], [500, "http_status"]]) {
    await fixture((_request, response) => reply(response, { secret: "provider-body-secret" },
      { status, headers: { "Retry-After": "17" } }), async (server) => {
      const operations = new HomeAssistantOperations(context(server));
      await assert.rejects(operations.callService("light", "turn_on"), (error) => {
        errorIs(kind, "unknown", status)(error);
        assert.equal(error.retryAfterSeconds, 17);
        assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes("provider-body-secret"));
        assert.ok(!JSON.stringify(error).includes(server.url));
        return true;
      });
      assert.equal(server.requests.length, 1);
    });
  }
});

test("read failure is not a write outcome and opaque client exception is not retained", async () => {
  await fixture((_request, response) => reply(response, null, { status: 404 }), async (server) => {
    await assert.rejects(new HomeAssistantOperations(context(server)).getState("light.kitchen"),
      errorIs("not_found", "not_applicable", 404));
    const operations = new HomeAssistantOperations({ baseUrl: server.url, fetch: () => {
      throw new Error("host-credential-secret", { cause: new Error("nested-secret") });
    } });
    await assert.rejects(operations.callService("light", "turn_on"), (error) => {
      errorIs("transport")(error);
      assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes("secret"));
      return true;
    });
    assert.equal(server.requests.length, 1);
  });
});

test("malformed or incompatible success receipts refuse after one actual provider entry", async () => {
  const rows = [
    { raw: Buffer.from("[{") }, { raw: Buffer.from('[{"entity_id":"light.a","state":"on","attributes":{"x":1,"x":2}}]') },
    { raw: Buffer.from('[{"entity_id":"light.a","state":"on","attributes":{"x":1,"\\u0078":2}}]') },
    { raw: Buffer.from('[{"entity_id":"light.a","state":"on","attributes":{"x":1e999}}]') },
    { raw: Buffer.from('[{"entity_id":"light.a","state":"on","attributes":{"x":9007199254740993}}]') },
    { raw: Buffer.from("[".repeat(129) + "]".repeat(129)) },
    { raw: Buffer.from("[/*comment*/]") }, { raw: Buffer.from("[1,]") }, { raw: Buffer.from("[] trailing") },
    { raw: Buffer.from([0xff]) }, { raw: Buffer.from([0xef, 0xbb, 0xbf, 91, 93]) },
    { value: null }, { value: [false] }, { value: [{ entity_id: "light.a", state: "on", attributes: [] }] },
    { value: { changed_states: [], service_response: false }, requested: true },
    { value: { changed_states: [] }, requested: true }, { value: { changed_states: [], service_response: [] }, requested: true },
    { value: { changed_states: null, service_response: null }, requested: true },
    { value: [], headers: { "Content-Type": "text/html" } },
  ];
  for (const row of rows) {
    await fixture((_request, response) => reply(response, Object.hasOwn(row, "value") ? row.value : [], row), async (server) => {
      const operations = new HomeAssistantOperations(context(server));
      await assert.rejects(operations.callService("light", "turn_on", { returnResponse: row.requested ?? false }),
        errorIs("invalid_response", "unknown", row.raw || row.headers ? 200 : null));
      assert.equal(server.requests.length, 1);
    });
  }
});

test("state identity and service-discovery shape are checked without flattening", async () => {
  for (const value of [state("light.other"), { ...state(), attributes: null }, false]) {
    await fixture((_request, response) => reply(response, value), async (server) => {
      await assert.rejects(new HomeAssistantOperations(context(server)).getState("light.kitchen"),
        errorIs("invalid_response", "not_applicable"));
    });
  }
  for (const value of [{}, [{ domain: "light", services: [] }], [{ services: {} }]]) {
    await fixture((_request, response) => reply(response, value), async (server) => {
      await assert.rejects(new HomeAssistantOperations(context(server)).listServices(),
        errorIs("invalid_response", "not_applicable"));
    });
  }
});

test("declared and streamed response size limits apply, compressed bodies are refused", async () => {
  for (const chunked of [false, true]) {
    await fixture((_request, response) => {
      const body = "[" + " ".repeat(200) + "]";
      if (!chunked) reply(response, null, { raw: Buffer.from(body) });
      else { response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
        response.write(body.slice(0, 30)); response.end(body.slice(30)); }
    }, async (server) => {
      await assert.rejects(new HomeAssistantOperations(context(server), { maxResponseBytes: 64 })
        .callService("light", "turn_on"), errorIs("response_too_large", "unknown", 200));
      assert.equal(server.requests.length, 1);
    });
  }
  await fixture((_request, response) => reply(response, [], { headers: { "Content-Encoding": "gzip" } }), async (server) => {
    await assert.rejects(new HomeAssistantOperations(context(server)).callService("light", "turn_on"),
      errorIs("unsupported_encoding", "unknown", 200));
  });
});

test("caller cancellation after provider entry remains unknown, service can finish after disconnect", async () => {
  const entered = deferred(); const release = deferred(); const effect = deferred(); const disconnected = deferred();
  await fixture(async (_request, response) => {
    response.once("close", () => disconnected.resolve());
    entered.resolve();
    await release.promise;
    effect.resolve("provider continued");
    if (!response.destroyed) reply(response, []);
  }, async (server) => {
    server.releases.push(release);
    const controller = new AbortController();
    const promise = new HomeAssistantOperations(context(server)).callService("light", "turn_on", { signal: controller.signal });
    const settled = promise.then((value) => ({ value }), (error) => ({ error }));
    try {
      await bounded(entered.promise, "provider entered before cancellation");
      controller.abort(new Error("secret-user-abort-reason"));
      const result = await bounded(settled, "operation cancellation");
      assert.ok(result.error instanceof HomeAssistantCancelled);
      errorIs("cancelled")(result.error);
      assert.ok(!`${result.error.stack} ${JSON.stringify(result.error)}`.includes("secret-user-abort-reason"));
      await bounded(disconnected.promise, "native fetch connection teardown");
      release.resolve();
      assert.equal(await bounded(effect.promise, "remote effect after cancellation"), "provider continued");
      assert.equal(server.requests.length, 1);
    } finally { controller.abort(); release.resolve(); await bounded(settled, "cancelled operation joined"); }
  });
});

test("total deadline survives progress chunks and times out after actual provider entry", { timeout: 10_000 }, async () => {
  const entered = deferred(); const closed = deferred();
  await fixture(async (_request, response) => {
    response.once("close", () => closed.resolve());
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.write("["); entered.resolve();
    // Data arrives every 200ms. An incorrectly reset per-chunk deadline would succeed.
    for (let index = 0; index < 10 && !response.destroyed; index++) {
      await delay(200);
      if (!response.destroyed) response.write(" ");
    }
    if (!response.destroyed) response.end("]");
  }, async (server) => {
    const promise = new HomeAssistantOperations(context(server), { timeoutMs: 1_000 }).callService("light", "turn_on");
    const settled = promise.then((value) => ({ value }), (error) => ({ error }));
    await bounded(entered.promise, "provider entered before total deadline");
    const result = await bounded(settled, "operation total deadline");
    errorIs("timeout")(result.error);
    await bounded(closed.promise, "timed-out response retired");
    assert.equal(server.requests.length, 1);
  });
});

test("connection loss after provider entry cannot establish no effect", async () => {
  let effect = false;
  await fixture((_request, response) => { effect = true; response.destroy(); }, async (server) => {
    await assert.rejects(new HomeAssistantOperations(context(server)).callService("light", "turn_on"), errorIs("transport"));
    assert.equal(effect, true);
    assert.equal(server.requests.length, 1);
  });
});

test("already-aborted signal sends no request and is safe conservative cancellation", async () => {
  await fixture(() => assert.fail("cancelled operation entered"), async (server) => {
    const controller = new AbortController(); controller.abort("secret-reason");
    await assert.rejects(new HomeAssistantOperations(context(server)).callService("light", "turn_on", { signal: controller.signal }),
      errorIs("cancelled"));
    assert.equal(server.requests.length, 0);
  });
});
