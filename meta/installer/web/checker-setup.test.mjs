import { test } from "node:test";
import assert from "node:assert/strict";
import React, { act } from "react";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";

const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const require = createRequire(import.meta.url);
const built = await build({ entryPoints: [fileURLToPath(new URL("checker-setup.jsx", import.meta.url))],
  bundle: true, write: false, platform: "node", format: "esm", loader: { ".css": "empty" },
  plugins: [{ name: "real-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
const { CheckerSetup } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
const status = (value, canPrepare = value === "needs_preparation" || value === "failed", detail = "Checked host prerequisite.") =>
  ({ status: value, can_prepare: canPrepare, detail });
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

async function mount(read, check, props = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/" });
  const names = ["window", "document", "navigator", "HTMLElement", "IS_REACT_ACT_ENVIRONMENT", "fetch",
    "localStorage", "sessionStorage", "setTimeout", "clearTimeout"];
  const old = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  const originalTimeout = globalThis.setTimeout, originalClear = globalThis.clearTimeout;
  const timers = new Set(), calls = [], readiness = [];
  let closes = 0, unmounted = false;
  for (const name of ["window", "document", "navigator", "HTMLElement"])
    Object.defineProperty(globalThis, name, { configurable: true, value: dom.window[name] || dom.window });
  Object.defineProperty(globalThis, "IS_REACT_ACT_ENVIRONMENT", { configurable: true, value: true });
  for (const name of ["localStorage", "sessionStorage"])
    Object.defineProperty(globalThis, name, { configurable: true, get() { throw new Error("Checker setup cannot persist browser settings"); } });
  Object.defineProperty(globalThis, "setTimeout", { configurable: true, value(fn, ms, ...args) {
    if (ms !== 2000) return originalTimeout(fn, ms, ...args);
    const timer = { fn: () => fn(...args) }; timers.add(timer); return timer;
  } });
  Object.defineProperty(globalThis, "clearTimeout", { configurable: true, value(timer) {
    if (timers.delete(timer)) return;
    originalClear(timer);
  } });
  Object.defineProperty(globalThis, "fetch", { configurable: true, value: async (url, options = {}) => {
    const call = { url, method: options.method || "GET", body: options.body };
    calls.push(call);
    const result = await read(call);
    if (result instanceof Error) throw result;
    return { ok: true, json: async () => result };
  } });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  const tick = async () => act(async () => new Promise(done => originalTimeout(done, 1)));
  const find = label => [...dom.window.document.querySelectorAll("button")].find(button => button.textContent === label);
  const click = async label => {
    const button = find(label); assert.ok(button, label);
    await act(async () => button.click()); await tick(); return button;
  };
  const poll = async () => {
    const timer = timers.values().next().value; assert.ok(timer, "A read-only poll is scheduled");
    timers.delete(timer);
    await act(async () => { timer.fn(); }); await tick();
  };
  const unmount = async () => { await act(async () => root.unmount()); unmounted = true; };
  const recordReadiness = value => { readiness.push(value); props.onReadiness?.(value); };
  function Parent() {
    const [value, setValue] = React.useState(props.initialReadiness || null);
    const [key, setKey] = React.useState(0);
    return React.createElement(React.Fragment, null,
      React.createElement("button", { disabled: value?.status !== "ready" }, "Build application"),
      React.createElement("button", { onClick: () => setKey(previous => previous + 1) }, "Reopen checks"),
      React.createElement(CheckerSetup, { key, onReadiness: next => { setValue(next); recordReadiness(next); },
        onClose: () => { closes++; } }));
  }
  try {
    await act(async () => root.render(props.parent ? React.createElement(Parent) : React.createElement(CheckerSetup, {
      onReadiness: recordReadiness,
      onClose: () => { closes++; },
    })));
    await tick();
    await check({ document: dom.window.document, calls, readiness, timers, find, click, poll, tick, unmount, closes: () => closes });
  } finally {
    if (!unmounted) await unmount();
    dom.window.close();
    for (const [name, descriptor] of old) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor); else delete globalThis[name];
    }
  }
}

test("preparation needs an explicit bodyless POST, rapid clicks admit once and polls only GET", async () => {
  let prepared = false, finish = false;
  await mount(call => call.method === "POST" ? (prepared = true, status("preparing", false))
    : status(finish ? "ready" : prepared ? "preparing" : "needs_preparation"), async view => {
    assert.deepEqual(view.calls, [{ url: "/api/checker-setup", method: "GET", body: undefined }]);
    assert.match(view.document.body.textContent, /only into this installation/);
    assert.match(view.document.body.textContent, /No administrator access or system Node replacement/);
    const button = view.find("Prepare app checks");
    await act(async () => { button.click(); button.click(); }); await view.tick();
    assert.equal(view.calls.filter(call => call.method === "POST").length, 1);
    assert.equal(view.calls.find(call => call.method === "POST").body, undefined);
    assert.equal(view.find("Prepare app checks"), undefined);
    finish = true; await view.poll();
    assert.equal(view.readiness.at(-1).status, "ready");
    assert.match(view.document.body.textContent, /App checks ready\./);
    assert.doesNotMatch(view.document.body.textContent, /app has been built|tested live|private Node runtime/);
    assert.equal(view.find("Check status"), undefined);
    assert.equal(view.timers.size, 0);
    assert.equal(view.calls.filter(call => call.method === "POST").length, 1);
  });
});

test("slow GET polling stays singleflight and a failed read continues polling without POST", async () => {
  const pending = deferred(); let reads = 0, concurrent = 0, maximum = 0;
  await mount(async () => {
    reads++; concurrent++; maximum = Math.max(maximum, concurrent);
    try {
      if (reads === 2) return await pending.promise;
      if (reads === 3) return new Error("Untrusted private path in error");
      return status(reads >= 4 ? "ready" : "preparing", false);
    } finally { concurrent--; }
  }, async view => {
    await view.poll();
    assert.equal(view.calls.length, 2);
    assert.equal(view.timers.size, 0);
    assert.equal(view.find("Checking status...").disabled, true);
    await act(async () => view.find("Checking status...").click());
    assert.equal(view.calls.length, 2);
    pending.resolve(status("preparing", false)); await view.tick();
    await view.poll();
    assert.match(view.document.body.textContent, /could not be read/);
    assert.equal(view.readiness.at(-1), null);
    assert.doesNotMatch(view.document.body.textContent, /Untrusted private path/);
    assert.equal(view.timers.size, 1);
    await view.poll();
    assert.equal(view.readiness.at(-1).status, "ready");
    assert.equal(maximum, 1);
    assert.ok(view.calls.every(call => call.method === "GET"));
  });
});

test("lost POST response latches until explicit GET confirms the running task and never resends", async () => {
  let admitted = false, readFails = true;
  await mount(call => {
    if (call.method === "POST") { admitted = true; return new Error("Secret-like transport detail"); }
    if (!admitted) return status("needs_preparation");
    return readFails ? new Error("Read unavailable") : status("preparing", false);
  }, async view => {
    await view.click("Prepare app checks");
    assert.match(view.document.body.textContent, /It may already be running/);
    assert.doesNotMatch(view.document.body.textContent, /Secret-like transport detail/);
    assert.equal(view.find("Prepare app checks").disabled, true);
    assert.equal(view.timers.size, 0);
    await view.click("Check status");
    assert.equal(view.find("Prepare app checks").disabled, true);
    readFails = false; await view.click("Check status");
    assert.equal(view.find("Prepare app checks"), undefined);
    assert.equal(view.timers.size, 1);
    assert.equal(view.calls.filter(call => call.method === "POST").length, 1);
    assert.equal(view.readiness.at(-1).status, "preparing");
  });
});

test("a confirmed failed task offers an explicit retry, with no automatic POST", async () => {
  let posts = 0;
  await mount(call => call.method === "POST" ? (posts++, status("failed", true, "Download could not be completed."))
    : status("needs_preparation"), async view => {
    await view.click("Prepare app checks");
    assert.equal(posts, 1); assert.equal(view.timers.size, 0);
    assert.equal(view.find("Prepare app checks").disabled, false);
    assert.match(view.document.body.textContent, /Download could not be completed/);
    await view.click("Prepare app checks"); assert.equal(posts, 2);
  });
});

test("Linux unavailable status makes authoring limits clear and never offers preparation", async () => {
  await mount(() => status("unavailable", false, "App authoring checks are not supported on Linux."), async view => {
    assert.match(view.document.body.textContent, /This computer cannot build applications yet/);
    assert.match(view.document.body.textContent, /not supported on Linux/);
    assert.match(view.document.body.textContent, /continue designing and deploy a package built on a supported Mac/);
    assert.equal(view.find("Prepare app checks"), undefined);
    assert.equal(view.readiness.at(-1).status, "unavailable");
    assert.ok(view.calls.every(call => call.method === "GET"));
  });
});

test("closing during POST makes late completion inert without claiming the task stopped", async () => {
  const pending = deferred();
  await mount(call => call.method === "POST" ? pending.promise : status("needs_preparation"), async view => {
    await view.click("Prepare app checks");
    assert.match(view.document.body.textContent, /download may continue if you close/);
    await view.click("Close");
    assert.equal(view.closes(), 1); assert.equal(view.document.querySelector("section") === null, true);
    pending.resolve(status("ready", false)); await view.tick();
    assert.equal(view.readiness.length, 2);
    assert.equal(view.timers.size, 0);
    assert.equal(view.calls.filter(call => call.method === "POST").length, 1);
  });
});

test("unmount during a poll prevents late readiness callback and further polls", async () => {
  const pending = deferred(); let reads = 0;
  await mount(() => ++reads === 1 ? status("preparing", false) : pending.promise, async view => {
    await view.poll(); await view.unmount();
    pending.resolve(status("ready", false)); await view.tick();
    assert.equal(view.readiness.length, 2); assert.equal(view.timers.size, 0);
    assert.equal(view.calls.length, 2);
  });
});

test("malformed mount status offers only readback, while parent callback failure does not invent lost admission", async () => {
  let reads = 0;
  await mount(call => call.method === "POST" ? status("ready", false)
    : ++reads === 1 ? { status: "ready", can_prepare: "yes", detail: "invalid" } : status("needs_preparation"), async view => {
    assert.deepEqual(view.readiness, [null, null]);
    assert.equal(view.find("Prepare app checks"), undefined);
    await view.click("Check status");
    await view.click("Prepare app checks");
    assert.equal(view.readiness.at(-1).status, "ready");
    assert.doesNotMatch(view.document.body.textContent, /Preparation was not confirmed/);
    assert.equal(view.calls.filter(call => call.method === "POST").length, 1);
  }, { onReadiness() { throw new Error("Parent rendering failure"); } });
});

test("remount clears retained ready during fresh GET and its failure until actual ready readback", async () => {
  const pending = deferred(); let reads = 0;
  await mount(() => ++reads === 1 ? status("ready", false) : reads === 2 ? pending.promise : status("ready", false), async view => {
    assert.equal(view.find("Build application").disabled, false);
    assert.equal(view.find("Check status"), undefined);
    await view.click("Reopen checks");
    assert.equal(view.readiness.at(-1), null);
    assert.equal(view.find("Build application").disabled, true);
    assert.match(view.document.body.textContent, /Reading app check status/);
    pending.resolve(new Error("Unknown host status")); await view.tick();
    assert.equal(view.find("Build application").disabled, true);
    assert.equal(view.readiness.at(-1), null);
    await view.click("Check status");
    assert.equal(view.find("Build application").disabled, false);
    assert.equal(view.readiness.at(-1).status, "ready");
    assert.equal(view.calls.length, 3);
    assert.ok(view.calls.every(call => call.method === "GET"));
  }, { parent: true });
});

test("mount clears inherited parent ready and a late unmounted failure cannot clear later readiness", async () => {
  const pending = deferred(); let latest = status("ready", false);
  await mount(() => pending.promise, async view => {
    assert.equal(view.find("Build application").disabled, true);
    assert.equal(latest, null);
    await view.unmount();
    latest = status("ready", false);
    pending.resolve(new Error("Old checker request failed")); await view.tick();
    assert.equal(view.readiness.length, 1);
    assert.equal(latest.status, "ready");
  }, { parent: true, initialReadiness: status("ready", false), onReadiness: value => { latest = value; } });
});
