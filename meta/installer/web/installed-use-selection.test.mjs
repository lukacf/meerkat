import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { installedUseUrl, installedUseScopeKey, createInstalledUseDraftStore, installedUseSelectedIdea } from "./deployment-use.mjs";
import { remoteInstalledUseBinding, remoteInstalledUseUrl } from "./remote-installed-use.mjs";

// The isolated candidate uses the already installed, pinned test dependencies.
const dependencyWeb = process.env.MOBKIT_TEST_WEB || fileURLToPath(new URL("./", import.meta.url));
const require = createRequire(resolve(dependencyWeb, "package.json"));
const React = require("react"), { act } = React, { build: bundle } = require("esbuild");
const { JSDOM } = createRequire(resolve(dependencyWeb, "../../console/package.json"))("jsdom");
const state = { id: "11111111-1111-4111-8111-111111111111", version: 4 };
const digest = "a".repeat(64), archive = "b".repeat(64);
const build = { guide_id: state.id, guide_version: state.version, current: true, source_verified: true,
  build_id: "accepted-source", source_digest: digest, status: "completed" };
const firstUse = { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64),
  tasks: [{ label: "Review the seed inventory", request: "Review the supplied seed inventory for the community garden.",
    inputs: ["The latest inventory spreadsheet"], prerequisites: ["Use the garden's own workspace"],
    expected_result: "A list of missing seed varieties and the evidence used.",
    result_location: "The saved inventory review in the garden workspace." }], detail: null };
const localReceipt = { deployment_id: "local-install", guide_id: state.id, guide_version: state.version,
  build_id: build.build_id, source_digest: digest, package_sha256: archive, current: true, status: "running",
  process_live: true, health: "ready", console_url: "http://127.0.0.1:8812/console",
  bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
const remoteReceipt = { protocol: 1, guide_id: state.id, guide_version: state.version,
  deployment_id: "22222222-2222-4222-8222-222222222222", build_id: build.build_id,
  accepted_source_digest: digest, package_sha256: archive, transferred_source_digest: "c".repeat(64),
  controller_sha256: "d".repeat(64), endpoint: { host: "fixture-linux.local", user: "fixture-user", ssh_port: 22 },
  host: { uid: 1001, home: "/home/fixture-user", destination: "/home/fixture-user/src/garden",
    platform: "linux", python: "3.11.9" }, destination: "/home/fixture-user/src/garden",
  host_key_fingerprint: "SHA256:" + "A".repeat(43), status: "running", admitted: true,
  worker_live: false, observation: { available: true }, source_current: true, ready: true,
  application_experiences: { protocol: 1, supported: true, status: "none", source_digest: digest,
    installed_source_digest: "c".repeat(64), metadata_sha256: null, entries: [] } };
const localExperiences = { deployment_id: localReceipt.deployment_id, build_id: build.build_id,
  package_sha256: archive, source_digest: digest, metadata_sha256: "f".repeat(64),
  scope: "declared-source", verified: false, entries: [{ id: "garden", label: "Garden workspace",
    path: "/workspace", port: 8820, url: "http://127.0.0.1:8820/workspace", status: "declared", verified: false }] };

function sourceMetadata(surface, receipt, catalogue) {
  const common = { deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: digest, package_sha256: receipt.package_sha256, guide_version: receipt.guide_version,
    version: state.version, current: true, status: receipt.status, application_outcomes_verified: false,
    project: { name: "Community garden", description: "A source-declared inventory assistant." },
    entrypoint: { identity: "garden:main", name: "Garden coordinator" }, first_use: structuredClone(catalogue),
    native_runtime_available: receipt.status === "running", continuation: { build_id: build.build_id,
      source_digest: digest, available: true, requires_acknowledgement: false } };
  if (surface === "local") return { ...common, scope: "verified-installed-source",
    console_url: receipt.status === "running" ? receipt.console_url : null };
  return { ...common, protocol: 1, guide_id: state.id, scope: "frozen-remote-package-source",
    accepted_source_digest: digest, transferred_source_digest: receipt.transferred_source_digest,
    controller_sha256: receipt.controller_sha256, remote_binding: JSON.parse(remoteInstalledUseBinding(receipt, state)),
    installed_source_verified: true, observation: { scope: "saved-destination-observation", status: receipt.status,
      available: true, freshness: "fresh", admitted: true, source_current: true } };
}

const components = new Map();
async function component(surface) {
  if (!components.has(surface)) components.set(surface, (async () => {
    const filename = surface === "local" ? "deployment-use.jsx" : "remote-installed-use.jsx";
    const built = await bundle({ entryPoints: [fileURLToPath(new URL(filename, import.meta.url))],
      bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
      nodePaths: [resolve(dependencyWeb, "node_modules")], plugins: [{ name: "shared-mounted-react", setup(builder) {
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
          path: pathToFileURL(require.resolve(args.path)).href, external: true,
        }));
      } }] });
    const module = await import("data:text/javascript;base64," + Buffer.from(built.outputFiles[0].text).toString("base64"));
    return surface === "local" ? module.InstalledApplicationUseMount : module.RemoteInstalledUse;
  })());
  return components.get(surface);
}

async function mounted(surface, verify, initialCatalogue = firstUse) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], admissions = [];
  let receipt = structuredClone(surface === "local" ? localReceipt : remoteReceipt);
  let catalogue = structuredClone(initialCatalogue);
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (target, options = {}) => {
      calls.push({ target, method: options.method || "GET" });
      assert.equal(options.method, undefined, "Choosing or restoring a walkthrough must never send a request.");
      assert.equal(target, surface === "local" ? installedUseUrl(state.id) : remoteInstalledUseUrl(state.id, receipt.deployment_id));
      return { ok: true, json: async () => sourceMetadata(surface, receipt, catalogue) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = require("react-dom/client"), Component = await component(surface);
  let root = createRoot(dom.window.document.getElementById("root"));
  const render = () => act(async () => root.render(React.createElement(Component, {
    state, receipt, build, blocked: false, experiences: localExperiences, Icon: () => null,
    onRepairReceipt: (value) => admissions.push(value),
  })));
  const task = () => dom.window.document.querySelector(".deployment-use-field textarea");
  const walkthrough = () => dom.window.document.querySelector(".deployment-use-walkthrough")?.textContent || "";
  async function edit(value, field = task()) {
    const setter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(field, value); field.dispatchEvent(new dom.window.Event("input", { bubbles: true })); });
  }
  try {
    await render();
    await verify({ calls, admissions, task, walkthrough, edit, document: dom.window.document, storage: dom.window.sessionStorage,
      async choose(index = 0) { await act(async () => dom.window.document.querySelectorAll(".deployment-use-ideas button")[index].click()); },
      async press(label) {
        const button = [...dom.window.document.querySelectorAll("button")].find((entry) => entry.textContent.trim() === label);
        assert.ok(button, `The ${label} control must be available.`);
        await act(async () => button.click());
      },
      async invoke(handler) { await act(async () => handler()); },
      async refresh(patch = {}, nextCatalogue = catalogue) { receipt = { ...receipt, ...patch }; catalogue = structuredClone(nextCatalogue); await render(); },
      async remount() { await act(async () => root.unmount()); root = createRoot(dom.window.document.getElementById("root")); await render(); },
    });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

for (const surface of ["local", "remote"]) {
  for (const transition of ["equal-catalog stop/restart refresh", "same-source page remount"]) {
    test(`${surface}: a customized chosen first job keeps its checked walkthrough across ${transition}`, async () => {
      await mounted(surface, async ({ choose, edit, task, walkthrough, refresh, remount, calls, admissions }) => {
        await choose();
        const custom = "  Review our current inventory, including donated seeds.\nKeep the original evidence.  ";
        await edit(custom);
        assert.equal(task().value, custom);
        assert.match(walkthrough(), /These instructions describe the original suggested job/);
        if (transition === "same-source page remount") await remount();
        else {
          await refresh({ status: "stopped", process_live: false, ready: false });
          await refresh({ status: "running", process_live: true, ready: true });
        }
        assert.equal(task().value, custom, "Exact customized human text must remain unchanged.");
        assert.match(walkthrough(), /A list of missing seed varieties and the evidence used/);
        assert.match(walkthrough(), /The saved inventory review in the garden workspace/);
        assert.match(walkthrough(), /These instructions describe the original suggested job/);
        assert.equal(calls.filter((call) => call.method !== "GET").length, 0);
        assert.deepEqual(admissions, []);
      });
    });
  }
  test(`${surface}: a changed checked catalogue retires the chosen walkthrough but preserves customized human text`, async () => {
    await mounted(surface, async ({ choose, edit, task, walkthrough, refresh, remount, calls }) => {
      await choose(); await edit("Keep my edited first job.");
      const changed = { ...firstUse, metadata_sha256: "9".repeat(64), tasks: [{ ...firstUse.tasks[0],
        expected_result: "CHANGED CATALOGUE WALKTHROUGH" }] };
      await refresh({ status: "stopped", process_live: false, ready: false }, changed);
      await remount();
      assert.equal(task().value, "Keep my edited first job.");
      assert.match(walkthrough(), /Your own first job/);
      assert.doesNotMatch(walkthrough(), /missing seed varieties|CHANGED CATALOGUE WALKTHROUGH/);
      assert.equal(calls.filter((call) => call.method !== "GET").length, 0);
    });
  });
  test(`${surface}: a different installed package cannot restore the old draft or walkthrough choice`, async () => {
    await mounted(surface, async ({ choose, edit, task, walkthrough, refresh, calls }) => {
      await choose(); await edit("My old package's edited job.");
      await refresh({ package_sha256: "8".repeat(64) });
      assert.equal(task().value, "");
      assert.match(walkthrough(), /Your own first job/);
      assert.doesNotMatch(walkthrough(), /missing seed varieties/);
      assert.equal(calls.filter((call) => call.method !== "GET").length, 0);
    });
  });
}

const suggestionCatalogue = { ...firstUse, tasks: [...firstUse.tasks, {
  label: "Plan the next seed order", request: "Plan a seed order for the garden's next planting season.",
  inputs: ["The next planting plan"], prerequisites: ["Check the garden's ordering rules"],
  expected_result: "A proposed seed order with quantities and reasons.",
  result_location: "The saved seed order proposal in the garden workspace.",
}] };

function storedEntries(storage) {
  return Object.keys(storage).sort().map((key) => [key, storage.getItem(key)]);
}

function clickHandler(button) {
  assert.ok(button, "An explicit confirmation control must be rendered.");
  const key = Object.keys(button).find((name) => name.startsWith("__reactProps$"));
  assert.ok(key, "Capture the actual mounted confirmation callback.");
  return button[key].onClick;
}

for (const surface of ["local", "remote"]) {
  test(`first-job suggestion draft: ${surface} previews and keeps the exact customized task and checked walkthrough`, async () => {
    await mounted(surface, async ({ choose, press, edit, task, walkthrough, document, storage, remount, calls, admissions }) => {
      await choose();
      const original = "  Review our actual inventory for October.\nInclude the donated seeds and keep their provenance.  ";
      await edit(original);
      const before = storedEntries(storage), instructions = walkthrough();
      await choose(1);
      assert.equal(task().value, original, "Inspecting a different suggestion must not replace current writing.");
      assert.equal(walkthrough(), instructions, "The checked walkthrough remains the selected job until explicit use.");
      assert.deepEqual(storedEntries(storage), before, "Preview must not change the persisted draft or choice.");
      assert.ok([...document.querySelectorAll("button")].some((button) => button.textContent.trim() === "Use this idea"));
      assert.match(document.querySelector(".deployment-use").textContent, /replace/i, "The explicit choice must explain replacement.");
      const edited = original + "\nUse the updated count from yesterday.";
      await edit(edited);
      const latest = storedEntries(storage);
      await press("Keep my draft");
      assert.equal(task().value, edited, "Keeping the draft preserves edits made while the preview was open.");
      assert.equal(walkthrough(), instructions);
      assert.deepEqual(storedEntries(storage), latest);
      assert.equal([...document.querySelectorAll("button")].some((button) => button.textContent.trim() === "Use this idea"), false);
      await remount();
      assert.equal(task().value, edited);
      assert.equal(walkthrough(), instructions, "The original selected walkthrough survives a real remount.");
      assert.equal(calls.filter((call) => call.method !== "GET").length, 0);
      assert.deepEqual(admissions, []);
    }, suggestionCatalogue);
  });

  test(`first-job suggestion draft: ${surface} explicitly replaces the latest edited task and only its selected walkthrough`, async () => {
    await mounted(surface, async ({ choose, press, edit, task, walkthrough, document, storage, remount, calls, admissions }) => {
      await choose();
      await edit("My current job includes real donated seeds.");
      const improvement = document.querySelectorAll(".deployment-use-field textarea")[1];
      await edit("Keep this separate improvement report exactly.\nDo not send it.", improvement);
      await act(async () => document.querySelector('input[type="radio"][value="partly"]').click());
      const before = storedEntries(storage), instructions = walkthrough();
      await choose(1);
      assert.equal(task().value, "My current job includes real donated seeds.");
      assert.equal(walkthrough(), instructions);
      assert.deepEqual(storedEntries(storage), before);
      await edit("  Latest text entered while previewing.\nThis is the draft being explicitly replaced.  ");
      const latest = storedEntries(storage);
      await press("Use this idea");
      assert.equal(task().value, suggestionCatalogue.tasks[1].request);
      assert.match(walkthrough(), /A proposed seed order with quantities and reasons/);
      assert.doesNotMatch(walkthrough(), /These instructions describe the original suggested job/);
      assert.equal(document.querySelectorAll(".deployment-use-field textarea")[1].value,
        "Keep this separate improvement report exactly.\nDo not send it.");
      assert.equal(document.querySelector('input[type="radio"][value="partly"]').checked, true);
      const beforeDraft = latest.find(([key]) => key.includes("mobkit-installed-use-draft-v1:"));
      const afterDraft = storedEntries(storage).find(([key]) => key === beforeDraft[0]);
      assert.deepEqual(JSON.parse(afterDraft[1]), { ...JSON.parse(beforeDraft[1]), draft: {
        ...JSON.parse(beforeDraft[1]).draft, task: suggestionCatalogue.tasks[1].request,
      } }, "Explicit replacement changes only task text in the persisted human draft.");
      await remount();
      assert.equal(task().value, suggestionCatalogue.tasks[1].request);
      assert.match(walkthrough(), /A proposed seed order with quantities and reasons/);
      assert.equal(calls.filter((call) => call.method !== "GET").length, 0);
      assert.deepEqual(admissions, []);
    }, suggestionCatalogue);
  });

  test(`first-job suggestion draft: ${surface} fills an empty task directly and retires an old package's explicit choice`, async () => {
    await mounted(surface, async ({ choose, invoke, edit, task, walkthrough, document, storage, refresh, remount, calls, admissions }) => {
      assert.equal(task().value, "");
      await choose(1);
      assert.equal(task().value, suggestionCatalogue.tasks[1].request, "An empty draft retains direct-fill behavior.");
      assert.match(walkthrough(), /A proposed seed order with quantities and reasons/);
      assert.equal([...document.querySelectorAll("button")].some((button) => button.textContent.trim() === "Use this idea"), false);
      await edit("An edited job belonging only to package A.");
      await choose(0);
      assert.equal(task().value, "An edited job belonging only to package A.");
      const staleUse = clickHandler([...document.querySelectorAll("button")].find((button) => button.textContent.trim() === "Use this idea"));
      await refresh({ package_sha256: "8".repeat(64) });
      assert.equal(task().value, "", "Package B has a separate human draft.");
      await choose(1);
      await edit("  Preserve package B's real job.\nIt has different seed counts.  ");
      const current = task().value, instructions = walkthrough(), before = storedEntries(storage);
      await invoke(staleUse);
      assert.equal(task().value, current, "A retired package A confirmation cannot alter package B.");
      assert.equal(walkthrough(), instructions);
      assert.deepEqual(storedEntries(storage), before, "The stale callback cannot modify either package's stored draft or choice.");
      await remount();
      assert.equal(task().value, current);
      assert.equal(walkthrough(), instructions);
      assert.equal(calls.filter((call) => call.method !== "GET").length, 0);
      assert.deepEqual(admissions, []);
    }, suggestionCatalogue);
  });
}

function memoryStorage() {
  const values = new Map();
  return { values, getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: (key) => values.delete(key) };
}

test("walkthrough storage keeps the previous human-draft schema and saves no authored instructions", () => {
  const storage = memoryStorage(), scope = installedUseScopeKey(localReceipt, state), drafts = createInstalledUseDraftStore(storage);
  drafts.update(scope, "outcome", "partly"); drafts.update(scope, "improvement", "Keep the original report wording.");
  assert.equal(drafts.selectIdea(scope, firstUse, 0), true);
  drafts.update(scope, "task", "Edited human text.");
  const entries = [...storage.values.entries()], draft = entries.find(([key]) => key.startsWith("mobkit-installed-use-draft-v1:"));
  const selected = entries.find(([key]) => key.startsWith("mobkit-installed-use-selection-v1:"));
  assert.deepEqual(JSON.parse(draft[1]), { scope, draft: { task: "Edited human text.", outcome: "partly",
    improvement: "Keep the original report wording." } });
  assert.deepEqual(JSON.parse(selected[1]), { scope, selection: { catalogue_sha256: firstUse.metadata_sha256, index: 0 } });
  assert.doesNotMatch(selected[1], /seed|workspace|request|expected_result|result_location|inputs|prerequisites/);
  const restored = createInstalledUseDraftStore(storage);
  assert.deepEqual(restored.read(scope), drafts.read(scope));
  assert.deepEqual(installedUseSelectedIdea(structuredClone(firstUse), restored.selectedIdea(scope)), firstUse.tasks[0]);
  const copy = restored.selectedIdea(scope); copy.index = 7;
  assert.equal(restored.selectedIdea(scope).index, 0);
  assert.equal(installedUseSelectedIdea({ ...firstUse, metadata_sha256: "8".repeat(64) }, copy), null);
});

test("invalid or oversized stored walkthrough choices leave the human draft available", () => {
  const storage = memoryStorage(), scope = installedUseScopeKey(localReceipt, state), drafts = createInstalledUseDraftStore(storage);
  drafts.selectIdea(scope, firstUse, 0); drafts.update(scope, "task", "Edited human text.");
  const key = [...storage.values.keys()].find((entry) => entry.startsWith("mobkit-installed-use-selection-v1:"));
  const selected = { catalogue_sha256: firstUse.metadata_sha256, index: 0 };
  for (const raw of ["broken JSON", "x".repeat(1001),
    JSON.stringify({ scope: "other", selection: selected }),
    ...[{ index: -1 }, { index: 8 }, { index: true }, { index: "0" }, { catalogue_sha256: "unverified" },
      { expected_result: "STORED INSTRUCTIONS MUST NOT APPEAR" }].map((patch) => JSON.stringify({ scope, selection: { ...selected, ...patch } }))]) {
    storage.values.set(key, raw);
    const restored = createInstalledUseDraftStore(storage);
    assert.equal(restored.read(scope).task, "Edited human text.");
    assert.equal(restored.selectedIdea(scope), null);
  }
  assert.equal(drafts.selectIdea(scope, { ...firstUse, scope: "trial-data" }, 0), false);
  assert.equal(drafts.selectIdea(scope, firstUse, 1), false);
  assert.equal(drafts.selectIdea("unbound", firstUse, 0), false);
});

test("walkthrough choices belong to the same exact installed source scope as the human draft", () => {
  const storage = memoryStorage(), scope = installedUseScopeKey(localReceipt, state), drafts = createInstalledUseDraftStore(storage);
  drafts.selectIdea(scope, firstUse, 0);
  for (const patch of [{ deployment_id: "another-install" }, { build_id: "another-build" },
    { package_sha256: "7".repeat(64) }, { guide_version: 3 }, { guide_id: "another-guide" }]) {
    const receipt = { ...localReceipt, ...patch }, owner = installedUseScopeKey(receipt,
      patch.guide_id ? { ...state, id: patch.guide_id } : state);
    assert.equal(createInstalledUseDraftStore(storage).selectedIdea(owner), null);
    assert.deepEqual(createInstalledUseDraftStore(storage).read(owner), { task: "", outcome: "", improvement: "" });
  }
  const unavailable = createInstalledUseDraftStore(null);
  assert.equal(unavailable.selectIdea(scope, firstUse, 0), true);
  assert.deepEqual(installedUseSelectedIdea(firstUse, unavailable.selectedIdea(scope)), firstUse.tasks[0]);
  assert.equal(unavailable.persistent(scope), false);
});

test("refused sidecar retirement cannot leave a newly stored human task joined to the old choice", () => {
  const storage = memoryStorage(), scope = installedUseScopeKey(localReceipt, state), drafts = createInstalledUseDraftStore(storage);
  const two = { ...firstUse, tasks: [...firstUse.tasks, { label: "Another first job", request: "A different authored job." }] };
  drafts.selectIdea(scope, two, 0);
  const before = [...storage.values.entries()];
  storage.removeItem = () => { throw new Error("STORAGE_FAILURE"); };
  assert.equal(drafts.selectIdea(scope, two, 1), true);
  drafts.update(scope, "task", "Edited second job."); drafts.update(scope, "outcome", "partly");
  assert.equal(drafts.read(scope).task, "Edited second job.");
  assert.equal(drafts.selectedIdea(scope).index, 1);
  assert.equal(drafts.persistent(scope), false);
  assert.deepEqual([...storage.values.entries()], before);
  const restored = createInstalledUseDraftStore(storage);
  assert.equal(restored.read(scope).task, firstUse.tasks[0].request);
  assert.equal(restored.selectedIdea(scope).index, 0);
});
