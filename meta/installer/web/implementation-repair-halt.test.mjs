import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";

async function load(path, mounted = false) {
  const require = createRequire(import.meta.url);
  const built = await build({ entryPoints: [fileURLToPath(new URL(path, import.meta.url))], bundle: true,
    write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
    plugins: mounted ? [{ name: "same-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] : [] });
  return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
}
const { implementationRepairHalt, implementationAction, implementationCanDownload } = await load("implementation-panel.mjs");
const guide = { id: "synthetic-private-planning", version: 4, proposal: { target: "mac" } };
const bindings = { source_digest: "a".repeat(64), blueprint_digest: "b".repeat(64),
  brief_digest: "c".repeat(64), binding_digest: "d".repeat(64) };
function failedBuild() {
  return { guide_id: guide.id, guide_version: guide.version, build_id: "synthetic-failed-build", current: true,
    status: "failed", phase: "automatic_repair", ready_to_package: false, source_verified: true,
    source_digest: bindings.source_digest, acceptance_trusted: true, acceptance_current: true,
    repair_round: 1, acceptance_policy_version: 1,
    acceptance: { issuer: "mobkit-launch-acceptance-host", ...bindings, status: "needs_repair",
      ready_to_package: false, review_round: 2, acceptance_policy_version: 1, tests_run: 13,
      summary: "Private planning still needs a useful answer.",
      requirements: [{ id: "private-answer", text: "Answer the organiser's question", status: "unresolved",
        evidence: "The private question was retained but no answer was produced." }],
      counterexamples: ["A changed quote must update the saved cost answer."], remaining_dependencies: [] },
    repair_halt: { ...bindings, reason: "unchanged_source_after_automatic_repair", repair_round: 1,
      review_round: 2, acceptance_policy_version: 1, at: "2026-10-03T17:40:00Z",
      native_completion: { cursor: "private-cursor-canary" }, extra: "private-halt-canary" } };
}

async function mountedPanel(receipt, inspect) {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], url = `/api/guide/${guide.id}/implementation`;
  let current = structuredClone(receipt);
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (target, options = {}) => {
      calls.push({ target, method: options.method || "GET", body: options.body });
      if (target === '/api/checker-setup') return {ok:true,json:async()=>({status:'ready',can_prepare:false,detail:'Synthetic ready checker'})};
      if (target === url && options.method === "POST") {
        assert.deepEqual(JSON.parse(options.body), { version: guide.version, request: "", previous_build_id: receipt.build_id });
        current = { ...current, build_id: "synthetic-next-build", status: "queued", phase: "queued",
          acceptance_trusted: false, acceptance_current: false, acceptance: null, repair_halt: null };
        return { ok: true, json: async () => structuredClone(current) };
      }
      if (target === url) return { ok: true, json: async () => structuredClone(current) };
      assert.ok(target.startsWith(`${url}/timeline?`), `Unexpected synthetic request: ${target}`);
      return { ok: true, json: async () => ({ build_id: current.build_id, frames: [], has_more: false }) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const { ImplementationPanel } = await load("implementation-panel.jsx", true);
  const root = createRoot(dom.window.document.getElementById("root"));
  try {
    await act(async () => root.render(React.createElement(ImplementationPanel,
      { state: guide, plan: { project: { name: "Synthetic private planner" } }, Icon: () => null })));
    await inspect(dom.window.document, calls, act);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

test("a trusted current unchanged repair opens actual findings without another admission or disabling manual repair", async () => {
  await mountedPanel(failedBuild(), async (document, calls, act) => {
    const review = document.querySelector(".implementation-acceptance");
    assert.equal(review?.open, true, "The remaining independent findings should be expanded after the exact no-change halt.");
    const notice = document.querySelector(".implementation-repair-halt");
    assert.match(notice?.textContent || "", /automatic repair saved no source change/i);
    assert.match(notice.textContent, /Add an implementation change below, or revise the proposal with your guide/);
    assert.equal(notice.getAttribute("role"), "status");
    assert.match(review.textContent, /changed quote must update the saved cost answer/);
    assert.match(review.textContent, /no answer was produced/);
    assert.doesNotMatch(notice.textContent, /private-halt-canary|private-cursor-canary|[a-d]{64}/);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    const repair = [...document.querySelectorAll("button")].find((button) => button.textContent.includes("Continue repairing application"));
    assert.equal(repair.disabled, false);
    assert.equal(document.querySelector("textarea").disabled, false);
    await act(async () => repair.click());
    assert.equal(calls.filter((call) => call.method === "POST").length, 1,
      "Only the user's explicit repair click may send another build request.");
    assert.equal(document.querySelector(".implementation-repair-halt") === null, true);
  });
});

test("the halt projection is fixed wording only and retains existing repair and download decisions", () => {
  const receipt = failedBuild(), before = structuredClone(receipt);
  const view = implementationRepairHalt(receipt, guide);
  assert.match(view.message, /automatic repair saved no source change/i);
  assert.doesNotMatch(JSON.stringify(view), /private-|[a-d]{64}|cursor|completion/);
  assert.equal(implementationAction(receipt, guide), "Continue repairing application");
  assert.equal(implementationCanDownload(receipt, guide), false);
  assert.deepEqual(receipt, before);
});

const invalid = [
  ["an earlier guide revision", (r) => { r.current = false; }],
  ["a different guide", (r) => { r.guide_id = "other-guide"; }],
  ["a newer saved source", (r) => { r.source_digest = "e".repeat(64); }],
  ["an untrusted report", (r) => { r.acceptance_trusted = false; }],
  ["a historical report", (r) => { r.acceptance_current = false; }],
  ["a passed report", (r) => { r.acceptance.status = "pass"; }],
  ["an incomplete report", (r) => { r.acceptance.status = "incomplete"; }],
  ["a running repair", (r) => { r.status = "running"; }],
  ["a completed build", (r) => { r.status = "completed"; }],
  ["an interrupted build", (r) => { r.status = "interrupted"; }],
  ["pending cleanup", (r) => { r.cleanup_pending = true; }],
  ["another halt reason", (r) => { r.repair_halt.reason = "private-halt-canary"; }],
  ["another phase", (r) => { r.phase = "independent_review"; }],
  ["a mismatched review round", (r) => { r.repair_halt.review_round += 1; }],
  ["a boolean review round", (r) => { r.repair_halt.review_round = true; r.acceptance.review_round = true; }],
  ["an absent repair round", (r) => { delete r.repair_round; }],
  ["a mismatched repair round", (r) => { r.repair_halt.repair_round += 1; }],
  ["a mismatched policy", (r) => { r.repair_halt.acceptance_policy_version += 1; }],
  ["a positive package claim", (r) => { r.ready_to_package = true; }],
  ["a missing source check", (r) => { r.source_verified = false; }],
  ["a foreign issuer", (r) => { r.acceptance.issuer = "private-issuer-canary"; }],
  ["a bare diagnostic instead of a halt", (r) => { delete r.repair_halt; r.detail = "Automatic repair made no source change."; }],
  ...Object.keys(bindings).map((key) => [`a changed ${key}`, (r) => { r.repair_halt[key] = "e".repeat(64); }]),
  ...Object.keys(bindings).map((key) => [`an invalid ${key}`, (r) => { r.repair_halt[key] = "private-digest-canary"; r.acceptance[key] = "private-digest-canary"; }]),
];
for (const [label, change] of invalid) test(`${label} cannot present an unchanged current repair halt`, () => {
  const receipt = failedBuild(); change(receipt);
  assert.equal(implementationRepairHalt(receipt, guide), null);
});

test("a newline-suffixed digest cannot fabricate a canonical source/context binding", () => {
  for (const key of Object.keys(bindings)) {
    const receipt = failedBuild();
    receipt.repair_halt[key] += "\n";
    receipt.acceptance[key] += "\n";
    if (key === "source_digest") receipt.source_digest += "\n";
    assert.equal(implementationRepairHalt(receipt, guide), null);
  }
});

test("historical findings remain collapsed and running repairs keep their normal live status", async () => {
  for (const change of [(r) => { r.repair_halt.source_digest = "e".repeat(64); }, (r) => { r.status = "running"; }]) {
    const receipt = failedBuild(); change(receipt);
    await mountedPanel(receipt, async (document, calls) => {
      assert.equal(document.querySelector(".implementation-repair-halt") === null, true);
      assert.equal(document.querySelector(".implementation-acceptance")?.open, false);
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
      if (receipt.status === "running") {
        assert.match(document.querySelector("h2").textContent, /Repairing review findings/);
        assert.ok([...document.querySelectorAll("button")].some((button) => button.textContent.includes("Stop build")));
      }
    });
  }
});
