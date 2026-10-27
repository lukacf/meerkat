import assert from "node:assert/strict";
import { after, test } from "node:test";
import React, { act, useState } from "react";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const directory = await mkdtemp(join(tmpdir(), "forge-provider-onboarding-"));
const modulePath = join(directory, "provider-setup.mjs");
after(() => rm(directory, { recursive: true, force: true }));
const built = await build({
  entryPoints: [fileURLToPath(new URL("provider-setup.jsx", import.meta.url))],
  bundle: true, write: false, platform: "node", format: "esm", loader: { ".css": "empty" },
  outfile: modulePath, sourcemap: "linked", sourcesContent: true, logLevel: "silent",
  plugins: [{ name: "real-react", setup(bundler) {
    if (process.env.FORGE_PROVIDER_ONBOARDING_SOURCE)
      bundler.onLoad({ filter: /\/provider-setup\.jsx$/ }, async () => ({
        contents: await readFile(process.env.FORGE_PROVIDER_ONBOARDING_SOURCE, "utf8"), loader: "jsx",
        resolveDir: fileURLToPath(new URL(".", import.meta.url)),
      }));
    bundler.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
for (const output of built.outputFiles) await writeFile(output.path, output.contents);
const { ProviderSetup } = await import(pathToFileURL(modulePath).href);

const settings = ({ configured = false, active = false } = {}) => ({
  providers: ["openai", "anthropic", "gemini"].map(provider => ({
    provider, configured: configured && provider === "openai", source: configured && provider === "openai" ? "saved" : null,
    editable: !active,
  })),
  effective_provider: configured ? "openai" : null,
  runtime_active: active, work_active: false, load_error: null,
});

// A collapsed native disclosure must not be part of the novice action path.
const outsideClosedDetails = node => {
  for (let parent = node.parentElement; parent; parent = parent.parentElement)
    if (parent.tagName === "DETAILS" && !parent.open) return false;
  return true;
};

async function mounted({ props = {}, read }, verify) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://offline.test/" });
  const replacements = {
    window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
  };
  const names = [...Object.keys(replacements), "fetch", "localStorage", "sessionStorage"];
  const previous = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  const calls = [], runtimes = [];
  let closes = 0, root, observeReady;
  for (const [name, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, name, { configurable: true, value });
  for (const name of ["localStorage", "sessionStorage"])
    Object.defineProperty(globalThis, name, { configurable: true, get() { throw new Error("Provider setup cannot store browser credentials."); } });
  Object.defineProperty(globalThis, "fetch", { configurable: true, value: async (url, options = {}) => {
    const call = { url, method: options.method || "GET", body: options.body ? JSON.parse(options.body) : undefined };
    calls.push(call);
    const result = await read(call);
    if (result instanceof Error) throw result;
    return { ok: true, json: async () => result };
  } });
  function Host() {
    const [ready, setReady] = useState(props.guideReady === true);
    observeReady = setReady;
    return React.createElement(ProviderSetup, {
      ...props, guideReady: ready,
      onClose: () => { closes += 1; },
      onRuntime: runtime => { runtimes.push(runtime); setReady(runtime.status === "ready"); },
    });
  }
  const document = dom.window.document;
  const button = label => {
    const result = [...document.querySelectorAll("button")].find(item => item.textContent === label);
    assert.ok(result, label);
    return result;
  };
  const settle = async () => act(async () => new Promise(resolve => setTimeout(resolve, 5)));
  const click = async label => { await act(async () => button(label).click()); await settle(); };
  const enter = async value => {
    const input = document.querySelector('input[type="password"]');
    assert.ok(input);
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, "value").set.call(input, value);
      input.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
      input.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
    });
  };
  try {
    const { createRoot } = await import("react-dom/client");
    root = createRoot(document.getElementById("root"));
    await act(async () => root.render(React.createElement(Host)));
    await settle();
    await verify({ document, calls, runtimes, button, click, enter, settle, closes: () => closes,
      observeReady: value => act(async () => observeReady(value)),
      unmount: async () => { await act(async () => root.unmount()); root = null; } });
  } finally {
    if (root) await act(async () => root.unmount());
    dom.window.close();
    for (const [name, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
}

test("first-run ready provider without an idea returns to Forge to describe a need", async () => {
  await mounted({ props: { firstRun: true, idea: "", guideReady: true }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    assert.equal(call.method, "GET");
    return settings({ configured: true, active: true });
  } }, async ({ document, calls, runtimes, button, click, closes }) => {
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.querySelector("h2").textContent, "Ready for your first message");
    assert.equal(document.querySelector(".setup-provider-idea") === null, true);
    assert.equal(document.querySelector(".setup-provider-return p").textContent,
      "Return to Forge to describe what you need. A rough idea is enough.");
    assert.doesNotMatch(document.body.textContent, /Your idea has not been sent|Back to my draft|Next: send your idea/);
    const action = button("Return to Forge");
    assert.equal(action.type, "button"); assert.equal(action.disabled, false);
    assert.equal(action.classList.contains("setup-provider-primary"), true);
    assert.equal(outsideClosedDetails(action), true);
    const modelSettings = [...document.querySelectorAll("summary")].find(item => item.textContent === "Model settings");
    assert.ok(modelSettings);
    assert.ok(action.compareDocumentPosition(modelSettings) & document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING,
      "The ready return action comes before secondary model settings in the mounted action path.");
    assert.equal(document.getElementById(action.getAttribute("aria-describedby")).textContent,
      "Provider access is checked when you send your first message.");
    assert.equal(calls.length, 1);
    assert.deepEqual(runtimes, []);
    await click("Return to Forge");
    assert.equal(closes(), 1, "Returning uses only the existing close callback.");
    assert.equal(calls.length, 1, "Returning does not save credentials, start the guide or send an idea.");
    assert.ok(calls.every(call => call.method === "GET"));
    assert.deepEqual(runtimes, []);
  });
});

test("provider starting context: context-only handoff retains unsent choices and returns to the starting point", async () => {
  const startingContext = Object.freeze({ version: 2, audience_id: "team", kind_id: "organizing" });
  await mounted({ props: { firstRun: true, idea: "", startingContext, guideReady: true }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    assert.equal(call.method, "GET");
    return settings({ configured: true, active: true });
  } }, async ({ document, calls, runtimes, button, click, closes }) => {
    const summary = document.querySelector(".setup-provider-starting-context");
    assert.ok(summary);
    assert.equal(summary.getAttribute("aria-label"), "Starting point, not sent");
    assert.deepEqual([...summary.querySelectorAll(".setup-provider-starting-context__answers span")].map(node => node.textContent),
      ["A team", "Keeping track of things"]);
    assert.match(summary.textContent, /Not sent/);
    assert.equal(document.querySelector(".setup-provider-idea") === null, true, "Choices are not synthesized into user prose.");
    assert.match(document.querySelector(".setup-provider-intro").textContent, /Connecting does not send your starting point\./);
    assert.equal(document.querySelector(".setup-provider-return p").textContent,
      "Next: send your starting point, or add your own words. Your choices are still unsent.");
    const action = button("Back to my starting point");
    assert.equal(action.type, "button"); assert.equal(action.disabled, false);
    assert.equal(outsideClosedDetails(action), true);
    await click("Back to my starting point");
    assert.equal(closes(), 1);
    assert.equal(calls.length, 1, "Returning neither sends choices nor starts or configures the guide.");
    assert.deepEqual(runtimes, []);
    assert.deepEqual(startingContext, { version: 2, audience_id: "team", kind_id: "organizing" });
  });
});

test("provider starting context: raw words stay separate and exact during unavailable settings and ready return", async () => {
  const idea = "  Keep my words\r\n<script>literal()</script>.  \t";
  const startingContext = Object.freeze({ version: 2, audience_id: "household", kind_id: "repetitive" });
  let readable = false;
  await mounted({ props: { firstRun: true, idea, startingContext, guideReady: true }, read: call => {
    assert.equal(call.method, "GET");
    if (call.url === "/api/provider-settings") return readable ? settings({ configured: true, active: true }) : new Error("Private read failure");
    assert.equal(call.url, "/api/architect/status");
    return { available: true, status: "ready" };
  } }, async ({ document, calls, runtimes, click, closes }) => {
    const words = () => document.querySelector(".setup-provider-idea p").textContent;
    const choices = () => [...document.querySelectorAll(".setup-provider-starting-context__answers span")].map(node => node.textContent);
    assert.equal(words(), idea);
    assert.deepEqual(choices(), ["My household", "Repetitive work"]);
    assert.equal(document.querySelector("script") === null, true);
    assert.equal(document.querySelector(".setup-provider-return") === null, true, "A summary is not runtime readiness.");
    assert.match(document.querySelector('[role="alert"]').textContent, /settings could not be read/i);
    assert.match(document.querySelector(".setup-provider-intro").textContent, /Connecting does not send your draft or starting point\./);
    readable = true;
    await click("Check settings");
    assert.equal(words(), idea);
    assert.deepEqual(choices(), ["My household", "Repetitive work"]);
    assert.equal(document.querySelector(".setup-provider-return p").textContent,
      "Next: send your idea and starting point. Both are still unsent.");
    const handoff = document.querySelector(".setup-provider-return");
    assert.equal(document.querySelector(".setup-provider-idea").nextElementSibling === handoff, true,
      "The ready handoff follows the retained idea before configuration details.");
    assert.ok(handoff.compareDocumentPosition(document.querySelector(".setup-provider-model-settings")) &
      document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING);
    await click("Back to my draft");
    assert.equal(closes(), 1);
    assert.equal(calls.length, 3);
    assert.ok(calls.every(call => call.method === "GET"));
    assert.deepEqual(runtimes, [{ available: true, status: "ready" }]);
    assert.equal(idea, "  Keep my words\r\n<script>literal()</script>.  \t");
  });
});

test("provider starting context: malformed choices do not invent a retained starting point", async () => {
  await mounted({ props: { firstRun: true, idea: "", guideReady: true,
    startingContext: { version: 2, audience_id: "unknown", kind_id: "organizing" } }, read: () => settings({ configured: true, active: true }) },
  async ({ document, calls, button }) => {
    assert.equal(document.querySelector(".setup-provider-starting-context") === null, true);
    assert.equal(document.querySelector(".setup-provider-return p").textContent,
      "Return to Forge to describe what you need. A rough idea is enough.");
    assert.ok(button("Return to Forge"));
    assert.equal(calls.length, 1);
  });
});

test("first-run provider handoff retains an unsent idea, treats it as text and focuses its heading", async () => {
  const idea = 'Help our tool library. <script>untrustedDraft()</script>';
  await mounted({ props: { firstRun: true, idea }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    assert.equal(call.method, "GET");
    return settings();
  } }, async ({ document, calls, button }) => {
    const heading = document.querySelector(".setup-provider-heading h2");
    assert.equal(document.activeElement === heading, true);
    assert.equal(heading.textContent, "A model for your guide");
    assert.equal(document.querySelector(".setup-provider-idea p").textContent, idea);
    assert.match(document.querySelector(".setup-provider-idea").textContent, /Not sent/);
    assert.equal(document.querySelector("script") === null, true);
    assert.equal(button("Start guide").disabled, true);
    assert.equal(button("Check saved settings").disabled, false);
    assert.equal(outsideClosedDetails(button("Save locally")), true);
    assert.equal(outsideClosedDetails(button("Start guide")), false);
    assert.equal(outsideClosedDetails(button("Check saved settings")), false);
    assert.equal(calls.length, 1, "Opening the handoff only observes local settings.");
    assert.ok(calls.every(call => call.method === "GET"));
  });
});

test("configured credentials do not imply a started guide, and the observed ready return sends nothing", async () => {
  for (const active of [false, true]) {
    await mounted({ props: { firstRun: true, idea: "Help me organize a reading group.", guideReady: true },
      read: call => {
        assert.equal(call.url, "/api/provider-settings");
        assert.equal(call.method, "GET");
        return settings({ configured: true, active });
      } }, async ({ document, calls, click, closes }) => {
      const heading = document.querySelector("h2");
      if (!active) {
        assert.equal(heading.textContent, "Start your setup guide");
        assert.equal(document.querySelector(".setup-provider-return") === null, true);
        assert.equal(outsideClosedDetails(document.querySelector("input")), false);
        assert.equal(outsideClosedDetails([...document.querySelectorAll("button")].find(item => item.textContent === "Start guide")), true);
        assert.match(document.querySelector(".setup-provider-configured").textContent, /OpenAI.*Configured in Forge/);
      } else {
        assert.equal(heading.textContent, "Ready for your first message");
        assert.match(document.querySelector(".setup-provider-return").textContent, /Next: send your idea\. Your draft is still unsent\./);
        assert.equal(document.querySelector(".setup-provider-return p").getAttribute("role"), "status");
        assert.ok([...document.querySelectorAll("button")].some(item => item.textContent === "Stop setup team"));
        assert.equal(outsideClosedDetails([...document.querySelectorAll("button")].find(item => item.textContent === "Stop setup team")), false);
        assert.equal(outsideClosedDetails([...document.querySelectorAll("button")].find(item => item.textContent === "Check saved settings")), false);
        const before = calls.length;
        await click("Back to my draft");
        assert.equal(closes(), 1);
        assert.equal(calls.length, before, "Returning invokes only the existing close callback.");
      }
      assert.ok(calls.every(call => call.method === "GET"));
    });
  }
});

test("first-run save and startup remain separate explicit actions, with no model send or secret echo", async () => {
  let configured = false, active = false;
  const secret = "synthetic-onboarding-private-key";
  await mounted({ props: { firstRun: true, idea: "Help the neighbourhood share gardening tools." }, read: call => {
    if (call.url === "/api/provider-settings") {
      if (call.method === "PUT") {
        assert.deepEqual(call.body, { provider: "openai", api_key: secret });
        configured = true;
      } else assert.equal(call.method, "GET");
      return settings({ configured, active });
    }
    if (call.url === "/api/architect/status") {
      assert.equal(call.method, "GET");
      return { status: active ? "ready" : "stopped", available: true };
    }
    assert.equal(call.url, "/api/architect/start");
    assert.equal(call.method, "POST");
    assert.equal(configured, true);
    active = true;
    return { status: "ready", available: true };
  } }, async ({ document, calls, click, enter, closes }) => {
    await enter(secret);
    await click("Save locally");
    assert.equal(document.querySelector("h2").textContent, "Start your setup guide");
    assert.equal(document.querySelector("input").value, "");
    assert.doesNotMatch(document.body.innerHTML, /synthetic-onboarding-private-key/);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
    assert.match(document.body.textContent, /saving does not check provider access/);
    assert.match(document.body.textContent, /Saved privately on the computer running Forge/);
    assert.match(document.getElementById("setup-provider-private").textContent,
      /private settings on the computer running Forge/);
    await click("Start guide");
    assert.equal(document.querySelector("h2").textContent, "Ready for your first message");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.match(document.body.textContent, /provider access is checked when it runs/);
    assert.equal(closes(), 0, "Starting the guide does not close the handoff or send the idea.");
    const before = calls.length;
    await click("Back to my draft");
    assert.equal(calls.length, before);
    assert.equal(closes(), 1);
    assert.doesNotMatch(document.body.innerHTML, /synthetic-onboarding-private-key/);
  });
});

test("uncertain first-run saves expose readback, keep the idea and hide private error details", async () => {
  const secret = "synthetic-lost-save-secret";
  await mounted({ props: { firstRun: true, idea: "A shared family reading assistant." }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    if (call.method === "PUT") return new Error(secret);
    assert.equal(call.method, "GET");
    return settings();
  } }, async ({ document, calls, button, click, enter }) => {
    await enter(secret);
    await click("Save locally");
    assert.equal(document.querySelector("h2").textContent, "Let's check your setup");
    assert.equal(document.querySelector("input").value, "");
    assert.doesNotMatch(document.body.innerHTML, /synthetic-lost-save-secret/);
    assert.equal(button("Check saved settings").disabled, false);
    assert.equal(outsideClosedDetails(button("Check saved settings")), true);
    assert.equal(button("Check saved settings").classList.contains("setup-provider-primary"), true);
    assert.equal(button("Save locally").disabled, true);
    assert.equal(document.querySelector(".setup-provider-idea p").textContent, "A shared family reading assistant.");
    assert.equal(document.querySelector(".setup-provider-return") === null, true);
    assert.equal(calls.filter(call => call.method === "PUT").length, 1);
    assert.equal(calls.filter(call => call.method === "POST").length, 0);
  });
});

test("configured first-run model settings stay optional, and editing a key cannot silently start the guide", async () => {
  await mounted({ props: { firstRun: true, idea: "Help me coordinate a community garden." }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    assert.equal(call.method, "GET");
    return settings({ configured: true });
  } }, async ({ document, calls, button, enter }) => {
    const detail = document.querySelector(".setup-provider-model-settings");
    assert.equal(detail.open, false);
    assert.equal(button("Start guide").classList.contains("setup-provider-primary"), true);
    await act(async () => detail.querySelector("summary").click());
    assert.equal(detail.open, true);
    await enter("synthetic-unsaved-provider-key");
    assert.equal(button("Save locally").disabled, false);
    assert.equal(button("Save locally").classList.contains("setup-provider-primary"), true);
    assert.equal(button("Start guide").disabled, true);
    assert.equal(outsideClosedDetails(button("Start guide")), false);
    assert.equal(document.querySelector(".setup-provider-idea p").textContent, "Help me coordinate a community garden.");
    assert.equal(calls.length, 1, "Opening settings and typing remain local actions.");
    assert.ok(calls.every(call => call.method === "GET"));
  });
});

test("unconfigured first-run key help stays optional and follows the chosen provider without any write", async () => {
  await mounted({ props: { firstRun: true, idea: "Help our volunteer group." }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    assert.equal(call.method, "GET");
    return settings();
  } }, async ({ document, calls, button }) => {
    const help = document.querySelector(".setup-provider-key-help");
    assert.equal(help.open, false);
    assert.equal(help.querySelector("summary").textContent, "I do not have a key yet");
    assert.equal(document.querySelector(".setup-provider-model").textContent, "Default modelgpt-6.1-sol");
    await act(async () => help.querySelector("summary").click());
    assert.equal(help.open, true);
    assert.match(help.textContent, /OpenAI API account/);
    assert.match(help.textContent, /API usage is billed by your model provider/);
    assert.match(help.textContent, /paste it above and choose Save locally/);
    const picker = document.getElementById("setup-guide-provider");
    await act(async () => {
      picker.value = "anthropic";
      picker.dispatchEvent(new window.Event("change", { bubbles: true }));
    });
    assert.match(help.textContent, /Anthropic API account/);
    assert.equal(document.querySelector(".setup-provider-model").textContent, "Default modelclaude-sonnet-4-6");
    assert.equal(document.querySelector(".setup-provider-key-heading a").getAttribute("href"),
      "https://platform.claude.com/docs/en/get-api-key");
    assert.equal(document.querySelectorAll("a").length, 1, "Help reuses the existing provider key link.");
    assert.equal(button("Save locally").disabled, true);
    assert.equal(calls.length, 1, "Opening help and choosing a provider do not save, start, or send.");
  });
});

test("configured model readback takes precedence and absent readback stays labelled as a default", async () => {
  for (const currentModel of ["custom-configured-model", null]) {
    await mounted({ props: { firstRun: true, idea: "Coordinate our repair workshop." }, read: call => {
      assert.equal(call.method, "GET");
      return { ...settings({ configured: true }), effective_model: currentModel };
    } }, async ({ document, calls }) => {
      assert.equal(document.querySelector(".setup-provider-key-help") === null, true);
      const model = document.querySelector(".setup-provider-configured-label");
      assert.equal(model.querySelector("span").textContent, currentModel ? "Current model" : "Default model");
      assert.equal(model.querySelector("strong").textContent, currentModel || "gpt-6.1-sol");
      const detail = document.querySelector(".setup-provider-model-settings");
      await act(async () => detail.querySelector("summary").click());
      assert.equal(document.querySelector(".setup-provider-model strong").textContent, currentModel || "gpt-6.1-sol");
      assert.equal(calls.length, 1, "Displaying readback does not fetch or change model settings.");
    });
  }
});

test("first-run focused Save hands keyboard focus to the incoming Start heading", async () => {
  let configured = false;
  await mounted({ props: { firstRun: true }, read: call => {
    if (call.url === "/api/provider-settings") {
      if (call.method === "PUT") configured = true;
      return settings({ configured });
    }
    assert.equal(call.url, "/api/architect/status");
    return { status: "stopped", available: true };
  } }, async ({ document, button, click, enter }) => {
    await enter("synthetic-focus-key");
    const initiating = button("Save locally");
    initiating.focus();
    assert.equal(document.activeElement === initiating, true);
    await click("Save locally");
    assert.equal(initiating.isConnected, false, "The visible Save control is replaced by the next step.");
    const heading = document.querySelector("h2");
    assert.equal(heading.textContent, "Start your setup guide");
    assert.equal(document.activeElement === heading, true, "Keyboard focus follows the completed local Save action.");
  });
});

test("first-run focused Start hands keyboard focus to the incoming Ready heading", async () => {
  let active = false;
  await mounted({ props: { firstRun: true, idea: "Help our local library." }, read: call => {
    if (call.url === "/api/provider-settings") return settings({ configured: true, active });
    assert.equal(call.url, "/api/architect/start");
    assert.equal(call.method, "POST");
    active = true;
    return { status: "ready", available: true };
  } }, async ({ document, calls, button, click, closes }) => {
    const initiating = button("Start guide");
    const container = initiating.closest(".setup-provider"), remove = container.removeChild;
    let browserBlur = false;
    container.removeChild = function(child) {
      if (child.contains(initiating)) {
        browserBlur = true;
        initiating.dispatchEvent(new document.defaultView.FocusEvent("focusout", { bubbles: true, relatedTarget: null }));
      }
      return remove.call(this, child);
    };
    initiating.focus();
    await click("Start guide");
    assert.equal(browserBlur, true, "Exercise browser blur before the old action is disconnected.");
    assert.equal(initiating.isConnected, false);
    const heading = document.querySelector("h2");
    assert.equal(heading.textContent, "Ready for your first message");
    assert.equal(document.activeElement === heading, true);
    assert.equal(closes(), 0);
    assert.equal(calls.filter(call => call.method === "POST").length, 1, "Focus repair does not submit the idea.");
  });
});

test("first-run action completion preserves newer focus even after navigation blurs back to body", async () => {
  for (const action of ["save", "start"]) for (const destination of ["outside", "guide controls"]) {
    let configured = action === "start", active = false, release;
    const response = new Promise(resolve => { release = resolve; });
    await mounted({ props: { firstRun: true }, read: async call => {
      if (call.url === "/api/provider-settings") {
        if (call.method === "PUT") return response;
        return settings({ configured, active });
      }
      if (call.url === "/api/architect/start") return response;
      assert.equal(call.url, "/api/architect/status");
      return { status: "stopped", available: true };
    } }, async ({ document, button, click, enter, settle }) => {
      if (action === "save") await enter("synthetic-departure-key");
      const label = action === "save" ? "Save locally" : "Start guide";
      button(label).focus();
      await click(label);
      const newer = destination === "outside" ? document.createElement("input")
        : document.querySelector(".setup-provider-routine-controls summary");
      if (destination === "outside") document.body.append(newer);
      newer.focus();
      assert.equal(document.activeElement === newer, true, `${destination} receives the user's newer focus.`);
      newer.blur();
      assert.equal(document.activeElement === document.body, true);
      configured = true; active = action === "start";
      await act(async () => release(action === "save" ? settings({ configured, active }) : { status: "ready", available: true }));
      await settle();
      assert.equal(document.querySelector(".setup-provider").dataset.providerStep, action === "save" ? "start" : "ready");
      assert.equal(document.activeElement === document.body, true, `${action} cannot reclaim focus after ${destination} navigation.`);
    });
  }
});

test("first-run model-key replacement with the same final step keeps focus on its retained action", async () => {
  await mounted({ props: { firstRun: true }, read: call => {
    if (call.url === "/api/provider-settings") return settings({ configured: true });
    assert.equal(call.url, "/api/architect/status");
    return { status: "stopped", available: true };
  } }, async ({ document, button, click, enter }) => {
    await act(async () => document.querySelector(".setup-provider-model-settings summary").click());
    await enter("synthetic-replacement-key");
    const initiating = button("Save locally");
    initiating.focus();
    await click("Save locally");
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "start");
    assert.equal(initiating.isConnected, true);
    assert.equal(document.activeElement === initiating, true, "Saving an edited key does not create a new first-run step.");
  });
});

test("first-run observed readiness without a local action preserves the user's outside focus", async () => {
  await mounted({ props: { firstRun: true }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    return settings({ configured: true, active: true });
  } }, async ({ document, observeReady }) => {
    const outside = document.createElement("input");
    document.body.append(outside); outside.focus();
    await observeReady(true);
    assert.equal(document.querySelector("h2").textContent, "Ready for your first message");
    assert.equal(document.activeElement === outside, true, "A prop observation cannot arm a local-action focus transition.");
  });
});

test("first-run unmounted Save response cannot reclaim focus or refresh the discarded component", async () => {
  let release;
  const response = new Promise(resolve => { release = resolve; });
  await mounted({ props: { firstRun: true }, read: call => {
    assert.equal(call.url, "/api/provider-settings");
    return call.method === "PUT" ? response : settings();
  } }, async ({ document, calls, button, click, enter, unmount, settle }) => {
    await enter("synthetic-unmounted-key");
    button("Save locally").focus();
    await click("Save locally");
    await unmount();
    const outside = document.createElement("input");
    document.body.append(outside); outside.focus();
    await act(async () => release(settings({ configured: true })));
    await settle();
    assert.equal(document.activeElement === outside, true);
    assert.equal(document.querySelector(".setup-provider") === null, true);
    assert.equal(calls.length, 2, "The stale response does not read runtime state or start work.");
  });
});

for (const browserBlur of [false, true]) test(`first-run check-ready focus reaches the incoming heading (${browserBlur ? "browser blur" : "ordinary"})`, async () => {
  let active = false, releaseStart;
  const startResponse = new Promise(resolve => { releaseStart = resolve; });
  await mounted({ props: { firstRun: true, idea: "Keep our workshop organized." }, read: call => {
    if (call.url === "/api/provider-settings") return {
      ...settings({ configured: true, active }), runtime_status: active ? "ready" : "stopped",
    };
    if (call.url === "/api/architect/start") return startResponse;
    assert.equal(call.url, "/api/architect/status");
    assert.equal(call.method, "GET");
    return { status: "ready", available: true };
  } }, async ({ document, calls, button, click, settle, closes }) => {
    await click("Start guide");
    const initiating = button("Check guide status");
    if (browserBlur) {
      const container = initiating.closest(".setup-provider"), remove = container.removeChild;
      container.removeChild = function(child) {
        if (child.contains(initiating)) initiating.dispatchEvent(new document.defaultView.FocusEvent("focusout", {
          bubbles: true, relatedTarget: null,
        }));
        return remove.call(this, child);
      };
    }
    initiating.focus();
    active = true;
    await click("Check guide status");
    const heading = document.querySelector("h2");
    assert.equal(heading.textContent, "Ready for your first message");
    assert.equal(initiating.isConnected, false, "The settled status read replaces its visible action.");
    assert.equal(document.activeElement === heading, true, "Keyboard focus follows the user's successful check.");
    assert.equal(closes(), 0, "Status observation does not send or dismiss the user's idea.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1, "Status observation never repeats the pending Start.");
    assert.ok(calls.every(call => call.method === "GET" || call.url === "/api/architect/start"));
    await act(async () => releaseStart({ status: "ready", available: true }));
    await settle();
    assert.equal(document.activeElement === heading, true, "Late settlement of the original Start keeps the completed handoff focus.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});

for (const destination of ["outside", "guide controls"]) test(`first-run check-ready focus preserves newer navigation (${destination})`, async () => {
  let active = false, checking = false, releaseStart, releaseRead;
  const startResponse = new Promise(resolve => { releaseStart = resolve; });
  const statusResponse = new Promise(resolve => { releaseRead = resolve; });
  await mounted({ props: { firstRun: true }, read: call => {
    if (call.url === "/api/provider-settings") return checking ? statusResponse : settings({ configured: true });
    if (call.url === "/api/architect/start") return startResponse;
    assert.equal(call.url, "/api/architect/status");
    return { status: "ready", available: true };
  } }, async ({ document, calls, button, click, settle }) => {
    await click("Start guide");
    const initiating = button("Check guide status");
    initiating.focus();
    checking = true;
    await click("Check guide status");
    const newer = destination === "outside" ? document.createElement("input")
      : document.querySelector(".setup-provider-routine-controls summary");
    if (destination === "outside") document.body.append(newer);
    newer.focus(); newer.blur();
    assert.equal(document.activeElement === document.body, true);
    active = true;
    await act(async () => releaseRead({ ...settings({ configured: true, active }), runtime_status: "ready" }));
    await settle();
    assert.equal(document.querySelector(".setup-provider").dataset.providerStep, "ready");
    assert.equal(document.activeElement === document.body, true, "Completed check cannot reclaim focus after newer navigation.");
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    await act(async () => releaseStart({ status: "ready", available: true }));
    await settle();
    assert.equal(document.activeElement === document.body, true);
  });
});

test("first-run check-ready focus is discarded with the unmounted provider", async () => {
  let checking = false, releaseStart, releaseRead;
  const startResponse = new Promise(resolve => { releaseStart = resolve; });
  const statusResponse = new Promise(resolve => { releaseRead = resolve; });
  await mounted({ props: { firstRun: true }, read: call => {
    if (call.url === "/api/provider-settings") return checking ? statusResponse : settings({ configured: true });
    if (call.url === "/api/architect/start") return startResponse;
    assert.fail(`Discarded status read cannot request ${call.url}`);
  } }, async ({ document, calls, button, click, unmount, settle }) => {
    await click("Start guide");
    button("Check guide status").focus();
    checking = true;
    await click("Check guide status");
    await unmount();
    const newer = document.createElement("input"); document.body.append(newer); newer.focus();
    await act(async () => releaseRead({ ...settings({ configured: true, active: true }), runtime_status: "ready" }));
    await settle();
    assert.equal(document.activeElement === newer, true);
    assert.equal(document.querySelector(".setup-provider") === null, true);
    assert.equal(calls.filter(call => call.url === "/api/architect/status").length, 0, "Unmount retires the status reader before the second read.");
    await act(async () => releaseStart({ status: "ready", available: true }));
    await settle();
    assert.equal(document.activeElement === newer, true);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
  });
});
