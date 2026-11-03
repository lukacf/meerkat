"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { routeGuard } = require("./route-guard.cjs");

// Dispatches routes the way Playwright does: nothing awaits the handler, so a
// rejection would be unhandled. unrouteAll({ behavior: "wait" }) waits for the
// handlers still running.
function fakePage() {
  const handlers = [];
  const invocations = new Set();
  const page = {
    unrouted: null,
    async route(url, handler) { handlers.push(handler); },
    deliver(options = {}) {
      const route = fakeRoute(options);
      const invocation = handlers[0](route).finally(() => invocations.delete(invocation));
      invocations.add(invocation);
      return { route, invocation };
    },
    async unrouteAll({ behavior }) {
      page.unrouted = behavior;
      handlers.length = 0;
      if (behavior === "wait") await Promise.all([...invocations]);
    },
  };
  return page;
}
function fakeRoute({ method = "POST", url = "http://fixture.test:8080/console/rpc", body = { jsonrpc: "2.0", id: 1, method: "meta/capabilities" } } = {}) {
  const route = {
    handled: null,
    request: () => ({ method: () => method, url: () => url, postData: () => body === null ? null : JSON.stringify(body) }),
    async fulfill() { route.handle("fulfilled"); },
    async abort(code) { route.handle(`aborted:${code}`); },
    handle(outcome) {
      if (route.handled) throw new Error("Route is already handled!");
      route.handled = outcome;
    },
  };
  return route;
}
const capabilities = "POST /console/rpc meta/capabilities";
const capabilitiesHandler = `route handler for ${capabilities}`;
function setup(options) {
  const page = fakePage();
  const lines = [];
  const routes = routeGuard(page, "fixture-scenario", { log: line => lines.push(line), ...options });
  return { page, lines, routes };
}

test("a failing handler is recorded and its request failed instead of an unhandled rejection", async () => {
  const { page, lines, routes } = setup();
  await routes.route("**/console/rpc", async () => {
    throw new Error("route.fetch: Protocol error (Storage.getCookies): Failed to find browser context");
  });
  const { route, invocation } = page.deliver();
  await assert.doesNotReject(invocation);
  assert.equal(route.handled, "aborted:failed");
  assert.equal(routes.failures.length, 1);
  assert.equal(routes.failures[0].source, capabilitiesHandler);
  assert.equal(routes.failures[0].afterEnd, false);
  assert.match(routes.failures[0].error, /Failed to find browser context/);
  assert.match(lines[0], /^fixture-scenario: route handler for POST \/console\/rpc meta\/capabilities failed: Error: route\.fetch/);
  assert.throws(() => routes.check(), /^Error: fixture-scenario: route handler for POST \/console\/rpc meta\/capabilities failed: Error: route\.fetch/);
});

test("a handler that fails after handling its route keeps that outcome", async () => {
  const { page, routes } = setup();
  await routes.route("**/console/rpc", async route => {
    await route.fulfill();
    throw new Error("assertion after fulfill");
  });
  const { route, invocation } = page.deliver();
  await assert.doesNotReject(invocation);
  assert.equal(route.handled, "fulfilled");
  assert.throws(() => routes.check(), /assertion after fulfill/);
});

test("settle waits for a running handler and its failure fails the scenario", async () => {
  const { page, lines, routes } = setup();
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  await routes.route("**/console/rpc", async () => {
    await gate;
    throw new Error("fetch failed once released");
  });
  page.deliver();
  let settled = false;
  const settling = routes.settle().then(() => { settled = true; });
  assert.equal(lines[0], `fixture-scenario: waiting for route handlers still running: ${capabilities}`);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(settled, false, "settle returned while a handler was still running");
  release();
  await settling;
  assert.equal(page.unrouted, "wait");
  assert.equal(routes.failures.length, 1);
  assert.equal(routes.failures[0].afterEnd, false, "a failure while settle waits is the scenario's failure");
  assert.throws(() => routes.check(), /fetch failed once released/);
  assert.equal(routes.settle(), routes.settle(), "settle runs once");
});

test("settle gives up after its bound and records the handlers still running", async () => {
  const { page, lines, routes } = setup({ settleTimeoutMs: 20 });
  await routes.route("**/console/experience", () => new Promise(() => {}));
  page.deliver({ method: "GET", url: "http://fixture.test:8080/console/experience", body: null });
  await routes.settle();
  assert.equal(routes.failures.length, 1);
  assert.equal(routes.failures[0].source, "route handler for GET /console/experience");
  assert.match(routes.failures[0].error, /still running 20 ms after the scenario ended/);
  assert.match(lines[0], /waiting for route handlers still running: GET \/console\/experience$/);
  assert.throws(() => routes.check(), /GET \/console\/experience failed/);
});

test("handlers that complete leave the scenario clean", async () => {
  const { page, lines, routes } = setup();
  await routes.route("**/console/rpc", route => route.fulfill());
  const { route, invocation } = page.deliver();
  await invocation;
  await routes.settle();
  assert.equal(route.handled, "fulfilled");
  assert.deepEqual(routes.failures, []);
  assert.deepEqual(lines, []);
  routes.check();
});
