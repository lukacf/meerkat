import { test } from "node:test";
import assert from "node:assert/strict";
import { request } from "./state.mjs";
import { createDeploymentSetupReader } from "./deployment-setup-read.mjs";

const url = "/api/guide/example/deployment/options?build_id=accepted";
const settle = () => new Promise(resolve => setImmediate(resolve));

test("RED: the ordinary 30s read abandons a successful 40s destination check", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let signal;
  t.mock.method(globalThis, "fetch", (_url, options) => {
    signal = options.signal;
    return new Promise(resolve => setTimeout(() => resolve({ ok: true,
      json: async () => ({ supported: true }) }), 40000));
  });
  const pending = request(url);
  const refused = assert.rejects(pending, error => error.code === "read_timeout");
  t.mock.timers.tick(30000);
  await refused;
  assert.equal(signal.aborted, true);
  t.mock.timers.tick(10000);
  await settle();
});

test("GREEN: a 40s setup check completes and retries share its pending read", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let count = 0, signal;
  const options = { supported: true, console_port: 4110 };
  t.mock.method(globalThis, "fetch", (_url, actual) => {
    count++; signal = actual.signal;
    return new Promise(resolve => setTimeout(() => resolve({ ok: true,
      json: async () => options }), 40000));
  });
  const read = createDeploymentSetupReader();
  const pending = read(url);
  await settle();
  t.mock.timers.tick(30000);
  await settle();
  assert.equal(signal.aborted, false);
  assert.equal(read(url), pending);
  assert.equal(count, 1);
  t.mock.timers.tick(10000);
  assert.equal(await pending, options);
  assert.equal(signal.aborted, false);
  const fresh = read(url);
  assert.notEqual(fresh, pending, "A completed observation is never cached");
  await settle();
  assert.equal(count, 2);
  t.mock.timers.tick(40000);
  await fresh;
});

test("different ports serialize per build, share identical queries and return their own observation", async () => {
  const calls = [], finish = [];
  const read = createDeploymentSetupReader((url, options) => {
    calls.push({ url, options });
    return new Promise(resolve => finish.push(resolve));
  });
  const first = read(url);
  await settle();
  const second = read(url + "&console_port=4111");
  assert.equal(read("/api/guide/example/deployment/options?console_port=4111&build_id=accepted"), second);
  await settle();
  assert.equal(calls.length, 1);
  finish[0]({ console_port: 4110 });
  assert.deepEqual(await first, { console_port: 4110 });
  await settle();
  assert.equal(calls.length, 2);
  assert.match(calls[1].url, /console_port=4111/);
  assert.deepEqual(calls[1].options, { timeoutMs: 120000 });
  finish[1]({ console_port: 4111 });
  assert.deepEqual(await second, { console_port: 4111 });
});

test("unrelated guides and builds do not share reads, including native-team revisions", async () => {
  const calls = [], finish = [];
  const read = createDeploymentSetupReader(url => {
    calls.push(url);
    return new Promise(resolve => finish.push(resolve));
  });
  const pending = [url, url.replace("example", "other"), url.replace("accepted", "new"),
    "/api/guide/example/deployment/options?source_kind=native-team&version=5",
    "/api/guide/example/deployment/options?source_kind=native-team&version=6"].map(read);
  await settle();
  assert.equal(calls.length, 5);
  finish.forEach((resolve, index) => resolve(index));
  assert.deepEqual(await Promise.all(pending), [0, 1, 2, 3, 4]);
});

test("setup deadline stays bounded, releases the pending read and preserves refusal details", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let signal;
  t.mock.method(globalThis, "fetch", (_url, actual) => {
    signal = actual.signal;
    return new Promise(() => {});
  });
  const read = createDeploymentSetupReader();
  const pending = read(url);
  const refused = assert.rejects(pending, error => error.code === "read_timeout" && /Try again/.test(error.message));
  await settle();
  t.mock.timers.tick(120000);
  await refused;
  assert.equal(signal.aborted, true);
  t.mock.method(globalThis, "fetch", async () => ({ ok: false, status: 409,
    json: async () => ({ detail: "The accepted build changed.", code: "build_changed" }) }));
  await assert.rejects(read(url), error => error.status === 409 && error.code === "build_changed");
});
