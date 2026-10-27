import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const web = resolve(process.env.FORGE_PROVIDER_READ_WEB || dirname(fileURLToPath(import.meta.url)));
const require = createRequire(resolve(web, "package.json"));
const { build } = require("esbuild");
const React = require("react");
const { act, useState } = React;
const { JSDOM } = createRequire(resolve(web, "../../console/package.json"))("jsdom");
const source = process.env.FORGE_PROVIDER_READ_SOURCE;
const requestSource = process.env.FORGE_PROVIDER_READ_REQUEST_SOURCE;
const bundlePath = process.env.FORGE_PROVIDER_READ_BUNDLE;
const compiled = await build({
  absWorkingDir: web, entryPoints: [resolve(web, "provider-setup.jsx")], bundle: true,
  write: false, platform: "node", format: "esm", logLevel: "silent", loader: { ".css": "empty" },
  outfile: "provider-read-recovery.mjs", sourcemap: "external", sourcesContent: true,
  nodePaths: [resolve(web, "node_modules")],
  plugins: [{ name: "actual-provider-source", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
    builder.onLoad({ filter: /(?:provider-setup\.jsx|state\.mjs)$/ }, async args => {
      const selected = args.path === resolve(web, "provider-setup.jsx") ? source
        : args.path === resolve(web, "state.mjs") ? requestSource : null;
      if (!selected) return;
      return { contents: await readFile(selected, "utf8"), resolveDir: web,
        loader: args.path.endsWith(".jsx") ? "jsx" : "js" };
    });
  } }],
});
const javascript = compiled.outputFiles.find(file => file.path.endsWith(".mjs"));
const sourceMap = compiled.outputFiles.find(file => file.path.endsWith(".map"));
if (bundlePath) {
  await writeFile(bundlePath, javascript.contents);
  await writeFile(`${bundlePath}.map`, sourceMap.contents);
}
const { ProviderSetup } = await import(`data:text/javascript;base64,${Buffer.from(javascript.contents).toString("base64")}`);

const idea = "  Compare forecasts for our club\nKeep each person's observing history private.  ";
const secret = "synthetic-unsent-provider-key";
const settings = ({ provider = null, active = false, runtimeStatus = active ? "ready" : "stopped" } = {}) => ({
  providers: ["openai", "anthropic", "gemini"].map(name => ({ provider: name,
    configured: name === provider, source: name === provider ? "saved" : null, editable: !active })),
  effective_provider: provider, runtime_active: active, runtime_status: runtimeStatus, work_active: false,
  load_error: null, provider_binding_error: null,
});
const ok = value => ({ ok: true, json: async () => value });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};

async function mounted(read, verify, { ready = false } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/" });
  const realSetTimeout = globalThis.setTimeout, realClearTimeout = globalThis.clearTimeout;
  const realSetInterval = globalThis.setInterval, realClearInterval = globalThis.clearInterval;
  const deadlines = new Map(), intervals = new Map(), calls = [], runtimes = [];
  let closes = 0, root;
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    // Advance only explicit response deadlines and the component's start-status observation.
    setTimeout(callback, delay, ...args) {
      if (delay !== 30000) return realSetTimeout(callback, delay, ...args);
      const token = {};
      deadlines.set(token, () => callback(...args));
      return token;
    },
    clearTimeout(token) {
      if (deadlines.has(token)) deadlines.delete(token);
      else realClearTimeout(token);
    },
    setInterval(callback, delay, ...args) {
      if (delay !== 4000) return realSetInterval(callback, delay, ...args);
      const token = {};
      intervals.set(token, () => callback(...args));
      return token;
    },
    clearInterval(token) {
      if (intervals.has(token)) intervals.delete(token);
      else realClearInterval(token);
    },
    fetch: async (url, options = {}) => {
      const call = { url, method: options.method || "GET", body: options.body && JSON.parse(options.body), signal: options.signal };
      calls.push(call);
      return read(call);
    },
  };
  const names = [...Object.keys(replacements), "localStorage", "sessionStorage"];
  const previous = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  for (const [name, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, name, { configurable: true, value });
  for (const name of ["localStorage", "sessionStorage"])
    Object.defineProperty(globalThis, name, { configurable: true, get() { throw new Error("Provider keys must stay outside browser storage."); } });
  const settle = async () => act(async () => new Promise(done => realSetTimeout(done, 5)));
  const document = dom.window.document;
  const button = label => {
    const value = [...document.querySelectorAll("button")].find(item => item.textContent === label);
    assert.ok(value, label);
    return value;
  };
  const click = async label => { await act(async () => button(label).click()); await settle(); };
  const chooseProvider = async provider => {
    const select = document.querySelector("select");
    await act(async () => {
      select.value = provider;
      select.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    });
  };
  const enter = async value => {
    const input = document.querySelector('input[type="password"]');
    assert.ok(input);
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(input, value);
      input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
  };
  const elapseReadDeadline = async () => {
    await act(async () => {
      for (const callback of [...deadlines.values()]) callback();
    });
    await settle();
  };
  const pollStart = async () => {
    await act(async () => {
      for (const callback of [...intervals.values()]) callback();
    });
    await settle();
  };
  const unmount = async () => {
    await act(async () => root.unmount());
    root = null;
    await settle();
  };
  function Host() {
    const [guideReady, setGuideReady] = useState(ready);
    return React.createElement(ProviderSetup, { firstRun: true, idea, guideReady,
      onRuntime: runtime => { runtimes.push(runtime); setGuideReady(runtime.status === "ready"); },
      onClose: () => { closes += 1; } });
  }
  try {
    const { createRoot } = await import(pathToFileURL(require.resolve("react-dom/client")).href);
    root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(Host)));
    await settle();
    await verify({ document, calls, runtimes, button, click, enter, chooseProvider, settle,
      elapseReadDeadline, pollStart, unmount, closes: () => closes, deadlines });
    assert.equal(deadlines.size, 0, "Settled response deadlines are cleared");
    assert.equal(dom.window.localStorage.length, 0, "Provider keys are not stored locally in the browser");
    assert.equal(dom.window.sessionStorage.length, 0, "Provider keys are not stored in browser session storage");
  } finally {
    if (root) await act(async () => root.unmount());
    for (const token of [...deadlines.keys()]) replacements.clearTimeout(token);
    dom.window.close();
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
}

test("a stalled initial provider-settings body exposes deliberate retry and retains the unsent idea", async () => {
  const oldBody = deferred();
  let reads = 0, signal;
  await mounted(call => {
    assert.equal(call.method, "GET");
    if (call.url === "/api/provider-settings") {
      if (++reads === 1) { signal = call.signal; return { ok: true, json: () => oldBody.promise }; }
      return ok(settings({ provider: "openai", active: true }));
    }
    assert.equal(call.url, "/api/architect/status");
    return ok({ status: "ready", available: true });
  }, async ({ document, button, click, calls, elapseReadDeadline, settle, closes }) => {
    assert.match(document.body.textContent, /Reading local settings/);
    await elapseReadDeadline();
    assert.match(document.body.textContent, /Provider settings could not be read/, "A stalled body must reach the existing recoverable error");
    assert.equal(signal.aborted, true);
    assert.equal(button("Check settings").disabled, false);
    assert.equal(button("Back to my idea").disabled, false);
    assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
    await click("Check settings");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    await act(async () => oldBody.resolve(settings()));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready", "Late original settings cannot replace the checked owner");
    assert.equal(closes(), 0);
    await click("Back to my draft");
    assert.equal(closes(), 1);
    assert.ok(calls.every(call => call.method === "GET"));
  }, { ready: true });
});

test("a stalled explicit status check releases retry without transplanting or clearing the unsent provider key", async () => {
  const oldBody = deferred();
  let statusReads = 0, settingsReads = 0, signal;
  await mounted(call => {
    assert.equal(call.method, "GET");
    if (call.url === "/api/provider-settings") return ok(++settingsReads < 3 ? settings() : settings({ provider: "gemini", active: true }));
    assert.equal(call.url, "/api/architect/status");
    if (++statusReads === 1) { signal = call.signal; return { ok: true, json: () => oldBody.promise }; }
    return ok({ status: "ready", available: true });
  }, async ({ document, button, click, chooseProvider, enter, elapseReadDeadline, settle, calls }) => {
    await chooseProvider("gemini");
    await enter(secret);
    await click("Check saved settings");
    assert.equal(button("Back to my idea").disabled, true);
    await elapseReadDeadline();
    assert.match(document.body.textContent, /Current settings or team status could not be read/, "Status read must settle into explicit recovery");
    assert.equal(signal.aborted, true);
    assert.equal(button("Check saved settings").disabled, false);
    assert.equal(button("Back to my idea").disabled, false);
    assert.equal(document.querySelector("select").value, "gemini");
    assert.equal(document.querySelector("input").value, secret);
    assert.doesNotMatch(document.body.textContent, new RegExp(secret));
    await click("Check saved settings");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    await act(async () => oldBody.resolve({ status: "stopped", available: false }));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready", "Late old status cannot overwrite successful retry");
    assert.ok(calls.every(call => call.method === "GET"));
  });
});

test("a confirmed save with a stalled status body remains saved and recovers without another PUT", async () => {
  let saved = false, statusReads = 0;
  await mounted(call => {
    if (call.url === "/api/provider-settings") {
      if (call.method === "PUT") {
        assert.deepEqual(call.body, { provider: "gemini", api_key: secret });
        assert.ok(call.signal instanceof AbortSignal, "Only the private-settings response has an explicit deadline");
        saved = true;
      }
      return ok(settings({ provider: saved ? "gemini" : null }));
    }
    assert.equal(call.method, "GET");
    assert.equal(call.url, "/api/architect/status");
    return ++statusReads === 1 ? { ok: true, json: () => new Promise(() => {}) }
      : ok({ status: "stopped", available: true });
  }, async ({ document, button, click, chooseProvider, enter, elapseReadDeadline, calls }) => {
    await chooseProvider("gemini");
    await enter(secret);
    await click("Save locally");
    assert.equal(button("Back to my idea").disabled, true);
    await elapseReadDeadline();
    assert.match(document.body.textContent, /The key was saved\. Team status could not be refreshed/, "Confirmed save must reach a bounded read error");
    assert.doesNotMatch(document.body.textContent, /Saving was not confirmed/);
    assert.equal(document.querySelector("input").value, "");
    assert.equal(document.querySelector("select").value, "gemini");
    assert.equal(button("Back to my idea").disabled, false);
    await click("Check saved settings");
    assert.equal(button("Start guide").disabled, false);
    assert.equal(calls.filter(call => call.method === "PUT").length, 1);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
  });
});

for (const action of ["start", "stop"]) {
  test(`a confirmed ${action} with a stalled settings body releases check without retrying the native action`, async () => {
    let active = action === "stop", settingsReads = 0;
    await mounted(call => {
      if (call.url === "/api/provider-settings") {
        assert.equal(call.method, "GET");
        if (++settingsReads === 2) return { ok: true, json: () => new Promise(() => {}) };
        return ok(settings({ provider: "openai", active }));
      }
      if (call.method === "POST") {
        assert.equal(call.url, `/api/architect/${action}`);
        assert.equal(call.signal, undefined, "Native action retains caller-owned lifetime");
        active = action === "start";
      } else assert.equal(call.url, "/api/architect/status");
      return ok({ status: active ? "ready" : "stopped", available: true });
    }, async ({ document, button, click, elapseReadDeadline, calls }) => {
      await click(action === "start" ? "Start guide" : "Stop setup team");
      assert.equal(button("Back to my idea").disabled, true);
      await elapseReadDeadline();
      assert.match(document.body.textContent, /Team status was received, but local settings could not be refreshed/, "Confirmed native response must reach bounded settings recovery");
      assert.doesNotMatch(document.body.textContent, /The team could not become ready|The team could not stop/);
      assert.equal(button("Back to my idea").disabled, false);
      await click("Check saved settings");
      if (action === "start") assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
      else assert.equal(button("Start guide").disabled, false);
      assert.equal(calls.filter(call => call.method === "POST").length, 1);
      assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
    }, { ready: action === "stop" });
  });
}

test("a stalled start-progress status body releases only observation while the admitted start remains pending", async () => {
  const startResponse = deferred();
  let statusReads = 0, active = false;
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings({ provider: "openai", active }));
    if (call.method === "POST") {
      assert.equal(call.url, "/api/architect/start");
      assert.equal(call.signal, undefined, "No read deadline may abort the start POST");
      return startResponse.promise;
    }
    assert.equal(call.url, "/api/architect/status");
    return ++statusReads === 1 ? { ok: true, json: () => new Promise(() => {}) }
      : ok({ status: "ready", available: true });
  }, async ({ document, button, click, pollStart, elapseReadDeadline, calls, settle, runtimes }) => {
    await click("Start guide");
    await pollStart();
    assert.equal(statusReads, 1);
    await elapseReadDeadline();
    assert.equal(button("Back to my idea").disabled, true, "Pending start retains its owner after observation timeout");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "starting");
    await pollStart();
    assert.equal(statusReads, 2, "Only the stalled read releases its observation fence");
    assert.equal(runtimes.at(-1).status, "ready");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    active = true;
    await act(async () => startResponse.resolve(ok({ status: "ready", available: true })));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(button("Back to my idea").disabled, false);
  });
});

test("pending start recovery checks fresh settings and status without replacing Start or sending the idea", async () => {
  const startBody = deferred(), oldPoll = deferred();
  let active = false, statuses = 0;
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings({ provider: "openai", active }));
    if (call.method === "POST") {
      assert.equal(call.url, "/api/architect/start");
      assert.equal(call.signal, undefined, "The original mutation is never aborted or given a read deadline");
      return { ok: true, json: () => startBody.promise };
    }
    assert.equal(call.url, "/api/architect/status");
    if (++statuses === 1) return ok({ status: "ready" });
    if (statuses === 2) return { ok: true, json: () => oldPoll.promise };
    return ok({ status: "ready" });
  }, async ({ document, button, click, pollStart, calls, runtimes, settle, closes, deadlines }) => {
    await click("Start guide");
    assert.equal(deadlines.size, 0, "A pending mutation body has no read timeout");
    await pollStart();
    assert.equal(runtimes.at(-1).status, "ready");
    assert.equal(document.querySelector(".setup-provider-return") === null, true, "A poll alone does not refresh the settings or release Start");
    assert.equal(button("Back to my idea").disabled, true);
    await pollStart();
    active = true;
    await click("Check guide status");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(button("Back to my idea").disabled, false);
    assert.equal(button("Stop setup team").disabled, true, "The admitted Start still owns mutations");
    await click("Stop setup team");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    const observed = runtimes.length, reads = calls.length;
    await act(async () => oldPoll.resolve({ status: "stopped" }));
    await settle();
    await pollStart();
    assert.equal(runtimes.length, observed, "An older or later background poll cannot override the explicit check");
    assert.equal(calls.length, reads, "Explicit readback also stops new background polling");
    await act(async () => startBody.reject(new Error(secret)));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.doesNotMatch(document.body.textContent, /could not become ready|synthetic-unsent-provider-key/);
    assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
    assert.equal(calls.filter(call => call.method !== "GET").length, 1);
    assert.equal(closes(), 0, "Readback does not navigate or submit the idea");
    await click("Back to my draft");
    assert.equal(closes(), 1);
    assert.equal(calls.filter(call => call.method !== "GET").length, 1);
  });
});

for (const mismatch of ["active-starting", "inactive-ready", "status-starting"]) {
  test(`pending start recovery refuses ${mismatch} until both fresh observations are ready`, async () => {
    const startBody = deferred();
    let checked = false;
    await mounted(call => {
      if (call.url === "/api/provider-settings") return ok(settings({ provider: "openai",
        active: checked && mismatch !== "inactive-ready",
        runtimeStatus: !checked ? "stopped" : mismatch === "active-starting" ? "starting" : "ready" }));
      if (call.method === "POST") return { ok: true, json: () => startBody.promise };
      assert.equal(call.url, "/api/architect/status");
      return ok({ status: mismatch === "status-starting" ? "starting" : "ready" });
    }, async ({ document, button, click, calls, settle }) => {
      await click("Start guide");
      checked = true;
      await click("Check guide status");
      assert.equal(document.querySelector(".setup-provider-return") === null, true);
      assert.equal(button("Back to my idea").disabled, true);
      assert.equal(button("Check guide status").disabled, false, "A non-ready observation remains deliberately recheckable");
      assert.equal(calls.filter(call => call.method !== "GET").length, 1);
      await act(async () => startBody.resolve({ status: "ready" }));
      await settle();
      assert.equal(document.querySelector(".setup-provider-return") === null, true, "A stale original Start cannot upgrade the explicit observation");
      assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
    }, { ready: true });
  });
}

for (const result of ["success", "failure"]) {
  test(`pending start recovery retains an entered settings read when the original POST settles with ${result}`, async () => {
    const startBody = deferred(), settingsBody = deferred();
    let reads = 0;
    await mounted(call => {
      if (call.url === "/api/provider-settings") return ++reads === 1 ? ok(settings({ provider: "openai" }))
        : { ok: true, json: () => settingsBody.promise };
      if (call.method === "POST") return { ok: true, json: () => startBody.promise };
      assert.equal(call.url, "/api/architect/status");
      return ok({ status: "ready" });
    }, async ({ document, button, click, calls, settle, runtimes }) => {
      await click("Start guide");
      await click("Check guide status");
      assert.equal(button("Checking guide status...").disabled, true);
      await act(async () => result === "failure" ? startBody.reject(new Error(secret))
        : startBody.resolve({ status: "stopped" }));
      await settle();
      assert.equal(button("Checking guide status...").disabled, true, "The original finally cannot release this independently entered read");
      assert.equal(button("Back to my idea").disabled, true);
      assert.equal(button("Start guide").disabled, true);
      assert.equal(runtimes.length, 0, "Neither stale success nor failure publishes over the explicit reader");
      await click("Checking guide status...");
      assert.equal(reads, 2, "The explicit read remains singleflight");
      await act(async () => settingsBody.resolve(settings({ provider: "openai", active: true })));
      await settle();
      assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
      assert.equal(button("Back to my idea").disabled, false);
      assert.equal(runtimes.at(-1).status, "ready");
      assert.doesNotMatch(document.body.textContent, /could not become ready|synthetic-unsent-provider-key/);
      assert.equal(calls.filter(call => call.method !== "GET").length, 1);
    });
  });
}

test("pending start recovery retries a bounded failed status read and ignores its late body", async () => {
  const startBody = deferred(), oldStatus = deferred();
  let active = false, statuses = 0, oldSignal;
  await mounted(call => {
    if (call.url === "/api/provider-settings") return ok(settings({ provider: "openai", active }));
    if (call.method === "POST") {
      assert.equal(call.signal, undefined);
      return { ok: true, json: () => startBody.promise };
    }
    assert.equal(call.url, "/api/architect/status");
    if (++statuses === 1) { oldSignal = call.signal; return { ok: true, json: () => oldStatus.promise }; }
    return ok({ status: "ready" });
  }, async ({ document, button, click, calls, settle, elapseReadDeadline }) => {
    await click("Start guide");
    active = true;
    await click("Check guide status");
    await elapseReadDeadline();
    assert.equal(oldSignal.aborted, true, "Only the read's signal is aborted");
    assert.equal(button("Back to my idea").disabled, true);
    assert.equal(button("Check guide status").disabled, false);
    assert.match(document.body.textContent, /Guide status could not be confirmed/);
    await click("Check guide status");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    await act(async () => oldStatus.resolve({ status: "stopped" }));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
    assert.equal(calls.filter(call => call.method !== "GET").length, 1);
    await act(async () => startBody.resolve({ status: "stopped" }));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
  });
});

for (const held of ["headers", "successful body", "refused body"]) {
  test(`provider save deadline recovers held ${held} without reusing old credentials or replacing checked state`, async () => {
    const oldResponse = deferred();
    let settingsReads = 0, saveSignal;
    await mounted(call => {
      if (call.url === "/api/provider-settings") {
        if (call.method === "PUT") {
          assert.deepEqual(call.body, { provider: "gemini", api_key: secret });
          saveSignal = call.signal;
          if (held === "headers") return oldResponse.promise;
          return { ok: held === "successful body", status: held === "refused body" ? 409 : 200,
            json: () => oldResponse.promise };
        }
        assert.equal(call.method, "GET");
        return ok(++settingsReads === 1 ? settings({ provider: "openai" })
          : settings({ provider: "gemini", active: true }));
      }
      assert.equal(call.method, "GET", "A settings deadline must not admit a native action");
      assert.equal(call.url, "/api/architect/status");
      return ok({ status: "ready", available: true });
    }, async ({ document, button, click, chooseProvider, enter, elapseReadDeadline, settle, calls, closes, runtimes }) => {
      await chooseProvider("gemini");
      await enter(secret);
      await click("Save locally");
      assert.equal(button("Saving...").disabled, true);
      assert.equal(button("Back to my idea").disabled, true);
      assert.equal(document.querySelector("input").value, secret, "The entered key belongs only to the outstanding save");
      await elapseReadDeadline();
      assert.equal(document.querySelector("input").value, "", "The response deadline clears the submitted key");
      assert.equal(saveSignal?.aborted, true, "The explicit settings deadline releases its response wait");
      assert.match(document.body.textContent, /Saving was not confirmed\. Check saved settings before continuing or entering a replacement key\./);
      assert.doesNotMatch(document.body.textContent, new RegExp(secret));
      assert.equal(button("Back to my idea").disabled, false);
      assert.equal(button("Check saved settings").disabled, false);
      const start = button("Start guide");
      assert.ok(start.closest(".setup-provider-routine-controls"), "The secondary Guide controls must also honor uncertain saving");
      assert.equal(start.disabled, true, "Previously configured credentials cannot enable Start before explicit readback");
      await click("Start guide");
      await enter("synthetic-replacement-key-not-sent");
      assert.equal(button("Save locally").disabled, true, "A replacement cannot be saved before checking the original save");
      await click("Save locally");
      assert.equal(calls.filter(call => call.method === "PUT").length, 1);
      assert.equal(calls.filter(call => call.method === "POST").length, 0);
      assert.equal(runtimes.length, 0, "An uncertain save does not report the guide as ready");
      assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
      assert.equal(closes(), 0, "A deadline does not automatically leave the idea or send it");

      await click("Check saved settings");
      assert.equal(settingsReads, 2, "Recovery explicitly rereads private settings");
      assert.equal(calls.filter(call => call.url === "/api/architect/status").length, 1);
      assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
      assert.equal(document.querySelector("select").value, "gemini");
      assert.equal(document.querySelector("input").value, "", "Explicit readback discards any replacement draft after uncertain saving");
      assert.equal(button("Stop setup team").disabled, false, "Confirmed readback releases normal native controls");
      const observations = runtimes.length, reads = calls.length;
      await act(async () => oldResponse.resolve(held === "headers" ? ok(settings({ provider: "openai" }))
        : held === "refused body" ? { detail: secret } : settings({ provider: "openai" })));
      await settle();
      assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready", "Late original response cannot replace checked readiness");
      assert.equal(document.querySelector("select").value, "gemini", "Late original response cannot replace the checked provider");
      assert.equal(runtimes.length, observations);
      assert.equal(calls.length, reads, "A late save response cannot schedule another status read");
      assert.doesNotMatch(document.body.textContent, /Saving was not confirmed|synthetic-unsent-provider-key|synthetic-replacement-key-not-sent/);
      assert.equal(calls.filter(call => call.method !== "GET").length, 1);
      await click("Back to my draft");
      assert.equal(closes(), 1);
      assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
      assert.equal(calls.filter(call => call.method !== "GET").length, 1, "Continuing does not save again or submit the idea");
    });
  });
}

test("provider save deadline ownership ignores a late successful body after unmount", async () => {
  const oldBody = deferred();
  await mounted(call => {
    assert.equal(call.url, "/api/provider-settings");
    if (call.method === "GET") return ok(settings());
    assert.equal(call.method, "PUT");
    assert.deepEqual(call.body, { provider: "gemini", api_key: secret });
    return { ok: true, json: () => oldBody.promise };
  }, async ({ document, click, chooseProvider, enter, unmount, settle, calls, runtimes, closes, deadlines }) => {
    await chooseProvider("gemini");
    await enter(secret);
    await click("Save locally");
    assert.equal(deadlines.size, 1, "The admitted private-settings save owns one response deadline");
    await unmount();
    assert.equal(document.getElementById("root").childElementCount, 0);
    await act(async () => oldBody.resolve(settings({ provider: "gemini", active: true })));
    await settle();
    assert.equal(deadlines.size, 0, "Settling the old response clears its deadline after unmount");
    assert.equal(runtimes.length, 0, "An unmounted save cannot report runtime readiness");
    assert.equal(closes(), 0);
    assert.equal(calls.length, 2, "An unmounted save cannot enter its follow-up status request");
    assert.equal(document.getElementById("root").childElementCount, 0);
    assert.doesNotMatch(document.body.textContent, new RegExp(secret));
  });
});
