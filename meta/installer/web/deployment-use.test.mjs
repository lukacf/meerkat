import assert from "node:assert/strict";
import test from "node:test";
import { existsSync } from "node:fs";
import { build as bundle } from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import React, { act } from "react";
const webDir = existsSync(new URL("package.json", import.meta.url)) ? new URL("./", import.meta.url) : new URL("../../../web/", import.meta.url);
import { renderToStaticMarkup } from "react-dom/server";
import { installedUseScopeKey, installedUseReadKey, installedApplicationUse, installedUseFeedback,
  installedUseUrl, createInstalledUseGate, copyInstalledUseTask, installedUseStartingIdeas,
  installedUseIdeaPreview, installedUseTaskAllowed, createInstalledUseDraftStore,
  INSTALLED_USE_TASK_LIMIT_MESSAGE, installedUseTaskHandoff, installedNativeTeamUse,
  createInstalledUseImprovementStore, installedUseImprovementHint, installedUseImprovementMatches,
  installedUseFeedbackNotAdmitted } from "./deployment-use.mjs";

const sharedRequire = createRequire(import.meta.url);
const compiled = await bundle({ entryPoints: [fileURLToPath(new URL("deployment-use.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
  nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  plugins: [{ name: "shared-mounted-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
      path: pathToFileURL(sharedRequire.resolve(args.path)).href, external: true,
    }));
  } }] });
const { InstalledApplicationUseView, InstalledApplicationUseMount, InstalledUseStartingIdeas,
  InstalledImprovementHandoff } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`);
const guide = { id: "guide", version: 16 };
const source = "a".repeat(64), archive = "b".repeat(64), nextSource = "c".repeat(64);
const installed = { deployment_id: "owned-install", guide_id: guide.id, guide_version: 16, current: true,
  build_id: "installed-build", source_digest: source, package_sha256: archive, status: "running",
  process_live: true, health: "ready", console_url: "http://127.0.0.1:8812/console",
  bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
const checked = { guide_id: guide.id, guide_version: 16, current: true, build_id: installed.build_id,
  source_digest: source, source_verified: true, status: "completed" };
const metadata = { deployment_id: installed.deployment_id, build_id: installed.build_id,
  source_digest: source, package_sha256: archive, guide_version: 16, current_guide_version: 16,
  version: 16, current: true, status: "running", scope: "verified-installed-source",
  application_outcomes_verified: false, project: { name: "Observing programme",
    description: "Historical design declaration: not-built. DO_NOT_RENDER_OLD_DECLARATION" },
  entrypoint: { identity: "lead:main", name: "Observing-program owner" },
  native_runtime_available: true, console_url: installed.console_url,
  continuation: { build_id: checked.build_id, source_digest: checked.source_digest,
    available: true, requires_acknowledgement: false } };
const report = { task: "Check the forecast for tonight", outcome: "partly", request: "Explain which forecast data is missing." };
const noUi = { deployment_id: installed.deployment_id, build_id: installed.build_id,
  package_sha256: archive, source_digest: source, metadata_sha256: null,
  scope: "declared-source", verified: false, entries: [] };
const baseView = { use: installedApplicationUse(metadata, installed, guide, checked, noUi), task: report.task,
  outcome: report.outcome, improvement: report.request, acknowledgement: false,
  onTask: () => {}, onOutcome: () => {}, onImprovement: () => {}, onAcknowledgement: () => {},
  onSubmit: () => {}, canSubmit: true, sending: false, blocked: false, loading: false,
  error: "", submitted: false, onReload: () => {}, Icon: () => null };
const render = (props = {}) => renderToStaticMarkup(React.createElement(InstalledApplicationUseView,
  { ...baseView, ...props }));

const improvedBuild = { ...checked, build_id: "admitted-improvement", source_digest: nextSource,
  ready_to_package: true, acceptance_trusted: true, acceptance_current: true,
  acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: nextSource, status: "pass", tests_run: 3 } };

test("improvement completion requires its exact admitted build and current independently accepted source", () => {
  const admission = { status: "submitted", guideId: guide.id, version: guide.version, buildId: improvedBuild.build_id };
  const handoff = (build, state = guide) => renderToStaticMarkup(React.createElement(InstalledImprovementHandoff,
    { admission, build, state }));
  assert.match(handoff(improvedBuild), /Your improvement is ready to review/);
  assert.match(handoff(improvedBuild), /href="#setup-implementation">Review improvement/);
  assert.match(handoff(improvedBuild), /This installation has not changed/);
  for (const change of [{ build_id: "another-improvement" }, { guide_id: "another-guide" },
    { guide_version: 15 }, { current: false }]) {
    assert.doesNotMatch(handoff({ ...improvedBuild, ...change }), /ready to review|Review improvement/);
  }
  for (const change of [{ source_digest: archive }, { acceptance_trusted: false }, { ready_to_package: false },
    { acceptance: { ...improvedBuild.acceptance, issuer: "builder" } },
    { acceptance: { ...improvedBuild.acceptance, status: "needs_repair" } }]) {
    assert.match(handoff({ ...improvedBuild, ...change }), /Your improvement needs more work/);
    assert.doesNotMatch(handoff({ ...improvedBuild, ...change }), /ready to review/);
  }
  assert.match(handoff({ ...improvedBuild, cleanup_pending: true }), /Your improvement is being built/);
  assert.doesNotMatch(handoff({ ...improvedBuild, cleanup_pending: true }), /Review improvement/);
  assert.match(handoff({ ...improvedBuild, current_guide_version: 17 }, { ...guide, version: 17 }), /ready to review/);
});

test("native first-use names only the exact checked installed conversation owner and never borrows application metadata", () => {
  const receipt = { ...installed, source_kind: "native-team", source_id: nextSource, build_id: null };
  const native = { ...metadata, source_kind: "native-team", source_id: nextSource, build_id: null,
    continuation: { available: false }, success_reporting_available: false };
  const use = installedNativeTeamUse(native, receipt, guide);
  assert.deepEqual(use, { name: metadata.project.name, entrypoint: metadata.entrypoint, consoleUrl: installed.console_url });
  for (const change of [{ source_kind: "application" }, { source_id: archive }, { source_digest: archive },
    { package_sha256: source }, { build_id: installed.build_id }, { deployment_id: "another-install" },
    { version: 15 }, { guide_version: 15 }, { status: "stopped" }, { entrypoint: { name: "Invented owner" } }])
    assert.equal(installedNativeTeamUse({ ...native, ...change }, receipt, guide), null);
  assert.equal(installedNativeTeamUse(native, { ...receipt, status: "stopped" }, guide), null);
  assert.equal(installedNativeTeamUse({ ...native, native_runtime_available: false }, receipt, guide).consoleUrl, null);
  assert.equal(installedNativeTeamUse({ ...native, console_url: "https://foreign.example/console" }, receipt, guide).consoleUrl, null);
  assert.equal(installedNativeTeamUse(native, { ...receipt, health: "unknown" }, guide).consoleUrl, null);
});

test("the production first-job view links the installed native Console without sending its local draft", () => {
  const calls = [], html = render({ onSubmit: () => calls.push("feedback"), onTask: () => calls.push("draft") });
  assert.match(html, /Use Observing programme/);
  assert.match(html, /Observing-program owner/);
  assert.match(html, /lead:main/);
  const nativeLink = html.match(/<a\b[^>]*href="http:\/\/127\.0\.0\.1:8812\/console"[^>]*>/)?.[0];
  assert.ok(nativeLink);
  assert.match(nativeLink, /target="_blank"/);
  assert.match(nativeLink, /aria-disabled="false"/);
  assert.match(nativeLink, /rel="noreferrer"/);
  assert.match(html, /Check the forecast for tonight/);
  assert.match(html, /not sent to the application automatically/);
  assert.match(html, /same conversation.*tool activity/);
  assert.doesNotMatch(html, /DO_NOT_RENDER_OLD_DECLARATION|iframe|[?&](identity|draft|token)=/);
  assert.deepEqual(calls, []);
});

test("a selected form-only first job keeps complete authored steps beyond its short idea preview", () => {
  const description = "Open the saved estimate and use its cost fields to record the revised supplier amount and its evidence revision. Keep the original evidence. Then choose Recalculate estimate and read the saved comparison. This is a form action, not a conversational instruction.";
  const catalogue = { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64), detail: null,
    tasks: [{ label: "Recalculate a saved estimate", request: "Revise the supplied quote and compare the resulting estimate.", description,
      inputs: ["The changed amount and its evidence revision"], prerequisites: ["An existing saved estimate"],
      expected_result: "The revised estimate with the original evidence retained.", result_location: "Saved estimate comparison" }] };
  const use = installedApplicationUse({ ...metadata, first_use: catalogue }, installed, guide, checked, noUi);
  const calls = [], html = render({ use, task: catalogue.tasks[0].request,
    selectedIdea: use.startingIdeas.tasks[0], onTask: () => calls.push("task"), onSubmit: () => calls.push("send") });
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM(html);
  try {
    const card = dom.window.document.querySelector(".deployment-use-ideas button");
    assert.ok(card.textContent.includes("..."));
    assert.ok(!card.textContent.includes("This is a form action"), "The idea card is only a short preview.");
    const step = dom.window.document.querySelectorAll(".deployment-use-walkthrough-steps section")[1];
    assert.equal(step.querySelector("p").textContent, description,
      "The selected job must expose its actual controls and saved-result instructions.");
    assert.doesNotMatch(step.textContent, /send it using the application's controls/);
    assert.deepEqual(calls, [], "Viewing these steps does not send the job or approve an action.");
  } finally { dom.window.close(); }
});

test("customizing a selected job keeps its authored directions and the mismatch warning", () => {
  const first_use = { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64), detail: null,
    tasks: [{ label: "Review records", request: "Review the supplied records.",
      description: "Open Records, choose the supplied revision, then use Review. <script>not an executable instruction</script>" }] };
  const use = installedApplicationUse({ ...metadata, first_use }, installed, guide, checked, noUi);
  const html = render({ use, task: "Review the supplied records and compare my extra note.", selectedIdea: use.startingIdeas.tasks[0] });
  assert.match(html, /These instructions describe the original suggested job/);
  assert.match(html, /Open Records, choose the supplied revision, then use Review/);
  assert.match(html, /&lt;script&gt;not an executable instruction&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script>/);
});

test("legacy first-job metadata without directions uses a neutral application-controls fallback", () => {
  const first_use = { scope: "authored-first-use-suggestions", metadata_sha256: "e".repeat(64), detail: null,
    tasks: [{ label: "Review records", request: "Review the supplied records." }] };
  const use = installedApplicationUse({ ...metadata, first_use }, installed, guide, checked, noUi);
  const html = render({ use, task: first_use.tasks[0].request, selectedIdea: use.startingIdeas.tasks[0] });
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM(html);
  try {
    const step = dom.window.document.querySelectorAll(".deployment-use-walkthrough-steps section")[1];
    assert.match(step.textContent, /use the application's controls to try the job/);
    assert.doesNotMatch(step.textContent, /send it using the application's controls/);
  } finally { dom.window.close(); }
});

test("an older installed source keeps its title and entrypoint while the newer candidate requires acknowledgement", () => {
  const older = { ...installed, guide_version: 14, current: false, current_guide_version: null };
  const candidate = { ...checked, build_id: "newer-candidate", source_digest: nextSource };
  const projected = { ...metadata, guide_version: 14, current: false, current_guide_version: null,
    continuation: { build_id: candidate.build_id, source_digest: nextSource, available: true, requires_acknowledgement: true } };
  const use = installedApplicationUse(projected, older, guide, candidate);
  assert.equal(use.name, metadata.project.name);
  assert.equal(use.entrypoint.identity, "lead:main");
  assert.equal(use.continuation.newerCandidate, true);
  assert.equal(installedUseFeedback(projected, older, guide, candidate, report), null);
  const body = installedUseFeedback(projected, older, guide, candidate, { ...report, continueCurrentCandidate: true });
  assert.equal(body.build_id, installed.build_id);
  assert.equal(body.source_digest, source);
  assert.equal(body.continuation_build_id, candidate.build_id);
  assert.equal(body.continuation_source_digest, nextSource);
  const html = render({ use });
  assert.match(html, /installed version/);
  assert.match(html, /Improve the newer candidate using this report/);
  assert.match(html, /installed version stays unchanged/);
});

test("source metadata cannot attach to another selected install, guide, source or read revision", () => {
  for (const change of [{ deployment_id: "other" }, { build_id: "other" }, { source_digest: nextSource },
    { package_sha256: "d".repeat(64) }, { guide_version: 15 }, { guide_version: true }, { version: 15 },
    { scope: "unverified" }, { application_outcomes_verified: true }, { status: "stopped" }]) {
    assert.equal(installedApplicationUse({ ...metadata, ...change }, installed, guide, checked), null);
  }
  assert.equal(installedUseScopeKey({ ...installed, guide_id: "other" }, guide), "");
  assert.equal(InstalledApplicationUseMount({ state: guide, receipt: null, build: checked }), null);
});

test("the ordinary deployment receipt mounts without inventing a source digest and uses verified installed metadata", () => {
  const receipt = { ...installed }; delete receipt.source_digest;
  assert.ok(installedUseScopeKey(receipt, guide));
  assert.ok(InstalledApplicationUseMount({ state: guide, receipt, build: checked, Icon: () => null }));
  assert.ok(installedApplicationUse(metadata, receipt, guide, checked));
  assert.equal(installedUseFeedback(metadata, receipt, guide, checked, report).source_digest, source);
  assert.equal(installedApplicationUse({ ...metadata, source_digest: "missing" }, receipt, guide, checked), null);
  assert.equal(installedApplicationUse({ ...metadata, package_sha256: nextSource }, receipt, guide, checked), null);
  assert.equal(installedApplicationUse(metadata, { ...receipt, source_digest: nextSource }, guide, checked), null);
});

test("live availability is rechecked against owned runtime health and never granted by metadata alone", () => {
  for (const change of [{ status: "stopped" }, { process_live: false }, { health: "failed" },
    { bootstrap: { complete: true, ready: false, counts: { broken: 0 } } },
    { console_url: "http://example.invalid/console" }, { console_url: "http://127.0.0.1:8812/console?token=secret" }]) {
    const receipt = { ...installed, ...change };
    const value = installedApplicationUse({ ...metadata, status: receipt.status }, receipt, guide, checked);
    assert.equal(value.consoleUrl, null);
    const html = render({ use: value });
    assert.doesNotMatch(html, /href=|Open application conversation/);
    assert.match(html, /Your draft stays here/);
    assert.match(html, /Check the forecast for tonight/);
  }
  assert.equal(installedApplicationUse({ ...metadata, native_runtime_available: false }, installed, guide, checked).consoleUrl, null);
  for (const suffix of ["?token=private-value", "#private-value"]) {
    const receipt = { ...installed, console_url: installed.console_url + suffix };
    const declared = { ...metadata, console_url: receipt.console_url };
    assert.equal(installedApplicationUse(declared, receipt, guide, checked).consoleUrl, null);
  }
});

test("human outcomes never create a build without an explicit improvement or promote application success", () => {
  const before = structuredClone({ metadata, installed, guide, checked, report });
  for (const outcome of ["worked", "partly", "not_worked", "blocked"]) {
    assert.equal(installedUseFeedback(metadata, installed, guide, checked, { task: report.task, outcome, request: " " }), null);
    const body = installedUseFeedback(metadata, installed, guide, checked, { ...report, outcome });
    assert.equal(body.outcome, outcome);
    assert.deepEqual(Object.keys(body).sort(), ["version", "deployment_id", "build_id", "source_digest", "package_sha256",
      "continuation_build_id", "continuation_source_digest", "continue_current_candidate", "outcome", "request", "task"].sort());
    assert.match(render({ outcome, submitted: true }), /installation has not changed/);
  }
  assert.deepEqual({ metadata, installed, guide, checked, report }, before);
});

test("repair needs current selected checked terminal source and respects design/deployment/manual blocking", () => {
  for (const change of [{ build_id: "other" }, { source_digest: nextSource }, { current: false },
    { current_guide_version: 15 }, { source_verified: false }, { status: "running" }]) {
    assert.equal(installedUseFeedback(metadata, installed, guide, { ...checked, ...change }, report), null);
  }
  for (const status of ["completed", "failed", "interrupted"]) {
    assert.ok(installedUseFeedback(metadata, installed, guide, { ...checked, status }, report));
  }
  assert.equal(installedUseFeedback(metadata, installed, guide, checked, report, true), null);
  assert.equal(installedUseFeedback({ ...metadata, continuation: { ...metadata.continuation, available: false } }, installed, guide, checked, report), null);
});

test("feedback sends only the manual bounded report and exact installed/current source identifiers", () => {
  const body = installedUseFeedback(metadata, { ...installed, private_settings: { PASSWORD: "MUST_NOT_COPY" },
    logs: ["RAW_LOG_MUST_NOT_COPY"], transcript: "MUST_NOT_HARVEST" }, guide, checked,
    { ...report, task: "  Human job \u00e5  ", request: "  Human improvement \u00e5  " });
  assert.equal(body.task, "Human job \u00e5");
  assert.equal(body.request, "Human improvement \u00e5");
  assert.doesNotMatch(JSON.stringify(body), /MUST_NOT_COPY|MUST_NOT_HARVEST/);
  for (const change of [{ outcome: "unknown" }, { request: "x".repeat(6001) }, { task: "x".repeat(3001) },
    { request: null }, { task: {} }]) {
    assert.equal(installedUseFeedback(metadata, installed, guide, checked, { ...report, ...change }), null);
  }
  assert.equal(installedUseUrl("guide/with space"), "/api/guide/guide%2Fwith%20space/deployment/use");
  assert.equal(installedUseUrl(guide.id, "feedback"), "/api/guide/guide/deployment/feedback");
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("delayed source reads cannot replace newer installation/candidate context or erase local drafts", async () => {
  for (const change of [{ receipt: { ...installed, deployment_id: "another-install" } },
    { receipt: { ...installed, package_sha256: nextSource } }, { guide: { ...guide, version: 17 } },
    { build: { ...checked, build_id: "new-build", source_digest: nextSource } }]) {
    const gate = createInstalledUseGate(), initialKey = installedUseReadKey(installed, guide, checked);
    let currentKey = initialKey, published = null;
    const drafts = { task: report.task, request: report.request }, pending = deferred();
    const token = gate.select(initialKey);
    const read = gate.run(token, () => pending.promise, () => currentKey, (value) => { published = value; });
    currentKey = installedUseReadKey(change.receipt || installed, change.guide || guide, change.build || checked);
    gate.select(currentKey);
    pending.resolve(metadata); await read;
    assert.equal(published, null);
    assert.deepEqual(drafts, { task: report.task, request: report.request });
  }
});

test("guide unmount and same-key remount invalidate old manual feedback results", async () => {
  const gate = createInstalledUseGate(), key = installedUseReadKey(installed, guide, checked);
  const original = gate.select(key), pending = deferred(), received = [];
  const action = gate.run(original, () => pending.promise, () => key, (value) => received.push(value));
  gate.close(original); const replacement = gate.select(key);
  gate.close(original);
  pending.resolve({ build_id: "admitted" }); await action;
  assert.deepEqual(received, []);
  assert.equal(gate.current(replacement, key), true);
  const receipt = { build_id: "actual-admission" };
  await gate.run(replacement, async () => receipt, () => key, (value) => received.push(value));
  assert.deepEqual(received, [receipt]);
});

test("health changes refresh live metadata while keeping the installed task/report scope stable", () => {
  const stopped = { ...installed, status: "stopped", health: "unknown", process_live: false };
  assert.equal(installedUseScopeKey(stopped, guide), installedUseScopeKey(installed, guide));
  assert.notEqual(installedUseReadKey(stopped, guide, checked), installedUseReadKey(installed, guide, checked));
  assert.equal(installedUseScopeKey(installed, { ...guide, version: 17 }), installedUseScopeKey(installed, guide));
  assert.notEqual(installedUseReadKey(installed, { ...guide, version: 17 }, checked), installedUseReadKey(installed, guide, checked));
});

function draftStorage() {
  const values = new Map();
  return { values, getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value), removeItem: (key) => values.delete(key) };
}

test("improvement reload hints retain source and report pins without restoring checked Build state", () => {
  const storage = draftStorage(), scope = installedUseScopeKey(installed, guide);
  const body = installedUseFeedback(metadata, installed, guide, checked, report);
  const hint = installedUseImprovementHint(scope, body), store = createInstalledUseImprovementStore(storage);
  assert.ok(store.save(scope, hint));
  assert.equal(store.persistent(scope), true);
  assert.deepEqual(createInstalledUseImprovementStore(storage).read(scope), hint);
  const hostRequest = "An exact saved host request with this source-bound report.";
  const confirmed = { ...hint, buildId: improvedBuild.build_id, hostRequest };
  assert.ok(store.save(scope, confirmed));
  assert.equal(installedUseImprovementMatches(confirmed, { ...improvedBuild, request: hostRequest }), true);
  for (const change of [{ request: "A different report" }, { build_id: "replacement" },
    { guide_id: "foreign" }, { guide_version: guide.version + 1 }])
    assert.equal(installedUseImprovementMatches(confirmed, { ...improvedBuild, request: hostRequest, ...change }), false);
  for (const change of [{ deployment_id: "foreign" }, { build_id: "foreign" }, { package_sha256: nextSource }, { guide_version: 15 }])
    assert.equal(createInstalledUseImprovementStore(storage).read(installedUseScopeKey({ ...installed, ...change }, guide)), null);
  const saved = JSON.parse([...storage.values.values()][0]);
  assert.deepEqual(Object.keys(saved), ["scope", "hint"]);
  assert.ok(!["status", "ready_to_package", "acceptance", "source_verified"].some(key => key in saved.hint));
});

test("damaged improvement tracking remains uncertain and denied storage retains only open-page tracking", () => {
  const storage = draftStorage(), scope = installedUseScopeKey(installed, guide);
  const hint = installedUseImprovementHint(scope, installedUseFeedback(metadata, installed, guide, checked, report));
  const store = createInstalledUseImprovementStore(storage);
  assert.ok(store.save(scope, hint));
  const key = [...storage.values.keys()][0];
  storage.setItem(key, "invalid saved tracking");
  assert.deepEqual(createInstalledUseImprovementStore(storage).read(scope), { corrupt: true });
  storage.setItem(key, JSON.stringify({ scope, hint: { ...hint, status: "completed", ready_to_package: true } }));
  assert.deepEqual(createInstalledUseImprovementStore(storage).read(scope), { corrupt: true });
  const denied = createInstalledUseImprovementStore({ getItem() { throw new Error("PRIVATE_STORAGE_DETAIL"); },
    setItem() { throw new Error("QUOTA_DETAIL"); }, removeItem() {} });
  assert.ok(denied.save(scope, hint)); assert.deepEqual(denied.read(scope), hint); assert.equal(denied.persistent(scope), false);
});

test("a rejection can retire only its exact pending hint while preserving report drafts and other requests", () => {
  const storage = draftStorage(), scope = installedUseScopeKey(installed, guide);
  const hint = installedUseImprovementHint(scope, installedUseFeedback(metadata, installed, guide, checked, report));
  const store = createInstalledUseImprovementStore(storage), drafts = createInstalledUseDraftStore(storage);
  drafts.update(scope, "task", report.task); drafts.update(scope, "outcome", report.outcome);
  drafts.update(scope, "improvement", report.request);
  assert.ok(store.save(scope, hint));
  assert.equal(store.forgetRejected(scope, { ...hint, request: "A different request." }), false);
  assert.deepEqual(store.read(scope), hint);
  assert.equal(store.forgetRejected(scope, hint), true);
  assert.equal(createInstalledUseImprovementStore(storage).read(scope), null);
  assert.deepEqual(createInstalledUseDraftStore(storage).read(scope),
    { task: report.task, outcome: report.outcome, improvement: report.request });
  const confirmed = { ...hint, buildId: "already-admitted", hostRequest: "Admitted request." };
  store.save(scope, confirmed);
  assert.equal(store.forgetRejected(scope, confirmed), false);
  assert.deepEqual(store.read(scope), confirmed);
  store.save(scope, hint);
  const replacement = { ...hint, request: "A separately retained uncertain request." };
  const other = createInstalledUseImprovementStore(storage); other.save(scope, replacement);
  assert.equal(store.forgetRejected(scope, hint), false);
  assert.deepEqual(createInstalledUseImprovementStore(storage).read(scope), replacement);
});

test("unavailable or ineffective request-tracking removal cannot release a retained pending report", () => {
  for (const removeItem of [() => {}, () => { throw new Error("PRIVATE_STORAGE_DETAIL"); }]) {
    const storage = draftStorage(), scope = installedUseScopeKey(installed, guide);
    const hint = installedUseImprovementHint(scope, installedUseFeedback(metadata, installed, guide, checked, report));
    const store = createInstalledUseImprovementStore({ ...storage, removeItem });
    store.save(scope, hint);
    assert.equal(store.forgetRejected(scope, hint), false);
    assert.deepEqual(store.read(scope), hint);
    assert.deepEqual(createInstalledUseImprovementStore(storage).read(scope), hint);
  }
});

test("only the explicit pre-submission witness distinguishes rejection from an uncertain HTTP error", () => {
  for (const status of [409, 422]) {
    assert.equal(installedUseFeedbackNotAdmitted({ status, code: "installed_feedback_not_admitted" }), true);
    for (const code of [undefined, null, "not_admitted", { code: "installed_feedback_not_admitted" }])
      assert.equal(installedUseFeedbackNotAdmitted({ status, code }), false);
  }
  assert.equal(installedUseFeedbackNotAdmitted({ status: 502, code: "installed_feedback_not_admitted" }), false);
});

test("first-use drafts restore exact human text and outcome after reload for only the installed source", () => {
  const storage = draftStorage(), scope = installedUseScopeKey(installed, guide);
  const drafts = createInstalledUseDraftStore(storage), task = "  Useful job <item>\n\ud83c\udf1f  ",
    improvement = "  Preserve my own wording.\nWhat should change?  ";
  assert.equal(drafts.update(scope, "task", task), true);
  assert.equal(drafts.update(scope, "outcome", "partly"), true);
  assert.equal(drafts.update(scope, "improvement", improvement), true);
  const restored = createInstalledUseDraftStore(storage);
  assert.deepEqual(restored.read(scope), { task, outcome: "partly", improvement });
  assert.equal(restored.persistent(scope), true);
  for (const change of [{ guide_id: "different" }, { deployment_id: "different" }, { build_id: "different" },
    { package_sha256: nextSource }, { guide_version: 15 }]) {
    const receipt = { ...installed, ...change }, owner = installedUseScopeKey(receipt,
      change.guide_id ? { ...guide, id: change.guide_id } : guide);
    assert.deepEqual(restored.read(owner), { task: "", outcome: "", improvement: "" });
  }
  // Runtime health, current proposal and candidate changes do not own this draft.
  const sameOwner = installedUseScopeKey({ ...installed, status: "stopped", process_live: false }, { ...guide, version: 17 });
  assert.equal(sameOwner, scope);
  assert.deepEqual(restored.read(sameOwner), { task, outcome: "partly", improvement });
  const html = render({ task, outcome: "partly", improvement, draftPersistent: true });
  assert.match(html, /Useful job &lt;item&gt;/);
  assert.match(html, /checked="" value="partly"/);
  assert.match(html, /Preserve my own wording/);
  assert.match(html, /across reloads until you close it/);
  assert.doesNotMatch(html, /Your improvement request is with the builder/);
});

test("source rereads and delayed feedback leave newer same-text drafts intact without restoring acknowledgement", async () => {
  const storage = draftStorage(), drafts = createInstalledUseDraftStore(storage), scope = installedUseScopeKey(installed, guide);
  drafts.update(scope, "task", report.task); drafts.update(scope, "improvement", report.request);
  const gate = createInstalledUseGate(), key = installedUseReadKey(installed, guide, checked), pending = deferred();
  let result = null;
  const token = gate.select(key), run = gate.run(token, () => pending.promise, () => key, (value) => { result = value; });
  drafts.update(scope, "task", "A newer edit"); drafts.update(scope, "task", report.task);
  drafts.update(scope, "improvement", "A newer report");
  pending.resolve({ build_id: "confirmed-feedback" }); await run;
  assert.equal(result.build_id, "confirmed-feedback");
  const reload = createInstalledUseDraftStore(storage).read(scope);
  assert.deepEqual(reload, { task: report.task, outcome: "", improvement: "A newer report" });
  assert.equal(drafts.update(scope, "acknowledgement", true), false);
  assert.equal(drafts.update(scope, "password", "PRIVATE_SETTING_MUST_NOT_SAVE"), false);
  assert.deepEqual(Object.keys(JSON.parse([...storage.values.values()][0]).draft), ["task", "outcome", "improvement"]);
  assert.doesNotMatch([...storage.values.values()][0], /PRIVATE_SETTING|confirmed-feedback|acknowledgement|source_digest|console_url/);
});

test("denied or full browser storage preserves editable drafts in the open page and says reload is unavailable", () => {
  const scope = installedUseScopeKey(installed, guide);
  for (const storage of [null, { getItem() { throw new Error("PRIVATE_STORAGE_DETAIL"); }, setItem() {}, removeItem() {} },
    { getItem: () => null, setItem() { throw new Error("QUOTA_DETAIL"); }, removeItem() {} }]) {
    const drafts = createInstalledUseDraftStore(storage);
    assert.equal(drafts.update(scope, "task", report.task), true);
    assert.equal(drafts.update(scope, "improvement", report.request), true);
    assert.deepEqual(drafts.read(scope), { task: report.task, outcome: "", improvement: report.request });
    assert.equal(drafts.persistent(scope), false);
    const value = drafts.read(scope);
    const html = render({ task: value.task, improvement: value.improvement, draftPersistent: drafts.persistent(scope) });
    assert.match(html, /Browser draft storage is unavailable/);
    assert.match(html, /remains in this open page/);
    assert.doesNotMatch(html, /PRIVATE_STORAGE_DETAIL|QUOTA_DETAIL|across reloads until/);
  }
});

test("stored first-use drafts are bounded, copied and cannot restore settings or another owner", () => {
  const storage = draftStorage(), drafts = createInstalledUseDraftStore(storage), scope = installedUseScopeKey(installed, guide);
  drafts.update(scope, "task", report.task);
  const key = [...storage.values.keys()][0], original = storage.values.get(key), before = drafts.read(scope);
  before.task = "Mutation must not change saved draft";
  assert.equal(drafts.read(scope).task, report.task);
  for (const [field, value] of [["task", "\ud83c\udf1f".repeat(3001)], ["improvement", "x".repeat(6001)],
    ["outcome", "invented-success"], ["task", { password: "PRIVATE_SETTING" }]]) {
    assert.equal(drafts.update(scope, field, value), false);
    assert.equal(storage.values.get(key), original);
  }
  for (const raw of ["broken JSON", "x".repeat(100001),
    JSON.stringify({ scope: "different", draft: drafts.read(scope) }),
    JSON.stringify({ scope, draft: { ...drafts.read(scope), password: "PRIVATE_SETTING" } }),
    JSON.stringify({ scope, draft: drafts.read(scope), private_settings: { PASSWORD: "PRIVATE_SETTING" } })]) {
    storage.values.set(key, raw);
    assert.deepEqual(createInstalledUseDraftStore(storage).read(scope), { task: "", outcome: "", improvement: "" });
  }
  assert.equal(drafts.update("invalid-owner", "task", "Must not save"), false);
});

test("clipboard feedback describes the exact copied draft and offers a manual fallback without raw errors", async () => {
  const copies = [], key = installedUseScopeKey(installed, guide);
  let task = report.task;
  assert.equal(await copyInstalledUseTask(task, async (value) => copies.push(value), () => task, () => key, key), "copied");
  assert.deepEqual(copies, [report.task]);
  for (const write of [null, async () => { throw new Error("PRIVATE_BROWSER_DETAIL"); }]) {
    assert.equal(await copyInstalledUseTask(task, write, () => task, () => key, key), "unavailable");
  }
  const html = render({ copyStatus: "unavailable" });
  assert.match(html, /Select task to copy/);
  assert.match(html, /copy manually/);
  assert.doesNotMatch(html, /PRIVATE_BROWSER_DETAIL/);
  assert.match(render({ copyStatus: "copied" }), /Task copied/);
});

test("a delayed clipboard result cannot claim that an edited draft or replaced scope was copied", async () => {
  for (const change of ["draft", "scope", "unmount"]) {
    const pending = deferred(), original = report.task, initial = installedUseScopeKey(installed, guide);
    let task = original, scope = initial;
    const result = copyInstalledUseTask(original, () => pending.promise, () => task, () => scope, initial);
    if (change === "draft") task = "A different useful job";
    if (change === "scope") scope = "different-installation";
    if (change === "unmount") scope = null;
    pending.resolve();
    assert.equal(await result, null);
  }
});

function elements(element) {
  if (!element || typeof element !== "object") return [];
  return [element, ...React.Children.toArray(element.props?.children).flatMap(elements)];
}

test("production outcome and feedback controls only call their explicit human handlers", () => {
  const events = [];
  const view = InstalledApplicationUseView({ ...baseView,
    onCopyTask: () => events.push(["copy"]),
    onOutcome: (value) => events.push(["outcome", value]), onTask: (value) => events.push(["task", value]),
    onImprovement: (value) => events.push(["request", value]), onSubmit: (event) => events.push(["submit", event]) });
  const nodes = elements(view);
  const radio = nodes.find((node) => node.type === "input" && node.props.value === "worked");
  radio.props.onChange();
  assert.deepEqual(events, [["outcome", "worked"]]);
  const fields = nodes.filter((node) => node.type === "textarea");
  fields[0].props.onChange({ target: { value: "My real job" } });
  fields[1].props.onChange({ target: { value: "Please improve it" } });
  assert.deepEqual(events, [["outcome", "worked"], ["task", "My real job"], ["request", "Please improve it"]]);
  nodes.find((node) => node.type === "button" && node.props.children === "Copy task").props.onClick();
  assert.equal(events.at(-1)[0], "copy");
  const event = { preventDefault() {} };
  nodes.find((node) => node.type === "form").props.onSubmit(event);
  assert.equal(events.at(-1)[0], "submit");
  assert.equal(events.at(-1)[1], event);
});

const catalogue = { scope: "authored-first-use-suggestions", metadata_sha256: "d".repeat(64),
  tasks: [{ label: "Review what needs attention", request: "List the items needing attention.\nExplain the next step for each.",
    description: "Start with a useful overview of your own information." }], detail: null };

test("starting ideas require the exact checked installed metadata and never fall back to trial jobs", () => {
  const declared = { ...metadata, first_use: catalogue,
    trial_samples: { tasks: [{ label: "DO_NOT_USE_TRIAL", request: "Synthetic approval DO_NOT_COPY" }] } };
  const before = structuredClone(declared);
  const use = installedApplicationUse(declared, installed, guide, checked);
  assert.deepEqual(use.startingIdeas.tasks, catalogue.tasks);
  assert.equal(use.startingIdeas.detail, null);
  for (const change of [{ deployment_id: "different" }, { build_id: "different" }, { source_digest: nextSource },
    { package_sha256: nextSource }, { version: 15 }, { guide_version: 15 }]) {
    assert.equal(installedApplicationUse({ ...declared, ...change }, installed, guide, checked), null);
  }
  const absent = installedApplicationUse({ ...metadata, trial_samples: declared.trial_samples }, installed, guide, checked);
  assert.deepEqual(absent.startingIdeas.tasks, []);
  assert.equal(absent.consoleUrl, installed.console_url);
  assert.doesNotMatch(render({ use: absent }), /DO_NOT_USE_TRIAL|DO_NOT_COPY|Start from an idea/);
  assert.deepEqual(declared, before);
});

test("invalid optional catalogues do not block genuine Console or feedback and fabricate no task", () => {
  const task = catalogue.tasks[0];
  for (const change of [{ scope: "trial-samples" }, { metadata_sha256: null }, { metadata_sha256: ["d".repeat(64)] },
    { tasks: null }, { tasks: Array(9).fill(task) }, { tasks: [{ ...task, label: "x".repeat(121) }] },
    { tasks: [{ ...task, label: "hidden\u0000label" }] }, { tasks: [{ ...task, request: " " }] },
    { tasks: [{ ...task, request: "x".repeat(3001) }] }, { tasks: [{ ...task, request: "hidden\u0000request" }] },
    { tasks: [{ ...task, description: "x".repeat(401) }] }, { detail: {} }]) {
    const declared = { ...metadata, first_use: { ...catalogue, ...change } };
    const use = installedApplicationUse(declared, installed, guide, checked);
    assert.deepEqual(use.startingIdeas, { tasks: [], detail: null });
    assert.equal(use.consoleUrl, installed.console_url);
    assert.ok(installedUseFeedback(declared, installed, guide, checked, report));
  }
  assert.deepEqual(installedUseStartingIdeas({ ...catalogue, metadata_sha256: null, tasks: [],
    detail: "This installation has no declared starting ideas." }),
  { tasks: [], detail: "This installation has no declared starting ideas." });
});

test("Unicode bounds and request previews are display-only while the full selected job remains exact", async () => {
  const request = "\ud83c\udf1f".repeat(2998) + "\n\u00e5";
  const value = { ...catalogue, tasks: [{ label: "\ud83c\udf1f".repeat(120), request,
    description: "\ud83c\udf1f".repeat(400) }] };
  const before = structuredClone(value), projected = installedUseStartingIdeas(value);
  assert.equal(projected.tasks[0].request, request);
  assert.equal(installedUseTaskAllowed(request), true);
  assert.equal(installedUseTaskAllowed(request + "x"), false);
  assert.deepEqual(installedUseStartingIdeas({ ...value, tasks: [{ ...value.tasks[0], label: value.tasks[0].label + "x" }] }).tasks, []);
  assert.deepEqual(installedUseStartingIdeas({ ...value, tasks: [{ ...value.tasks[0], description: value.tasks[0].description + "x" }] }).tasks, []);
  const preview = installedUseIdeaPreview(request);
  assert.equal(Array.from(preview).length, 100);
  assert.ok(preview.endsWith("..."));
  assert.equal(installedUseIdeaPreview(" First\n second\tthird "), "First second third");
  assert.equal(installedUseIdeaPreview("Read ".repeat(18) + "Wishlist details and explain the next useful step."),
    "Read ".repeat(18).trimEnd() + "...");
  assert.equal(installedUseIdeaPreview("x".repeat(97) + " complete words after the limit"), "x".repeat(97) + "...");
  assert.equal(installedUseIdeaPreview("\u00e5".repeat(105)), "\u00e5".repeat(97) + "...");
  const copies = [], scope = installedUseScopeKey(installed, guide);
  assert.equal(await copyInstalledUseTask(request, async (text) => copies.push(text), () => request, () => scope, scope), "copied");
  assert.deepEqual(copies, [request]);
  assert.equal(installedUseFeedback(metadata, installed, guide, checked, { ...report, task: request }).task, request);
  const changes = [], editor = elements(InstalledApplicationUseView({ ...baseView,
    onTask: (value) => changes.push(value) })).find((node) => node.type === "textarea");
  editor.props.onChange({ target: { value: request } });
  editor.props.onChange({ target: { value: request + "x" } });
  assert.deepEqual(changes, [request]);
  assert.deepEqual(value, before);
});

test("production idea selection only fills the local draft and keeps the exact authored request editable", async () => {
  const calls = [], request = "  Review <item> & explain it.\n" + "context ".repeat(30) + "  ",
    first_use = { ...catalogue, tasks: [{ label: "Review <item>", request,
      description: "Review each item.\r\nExplain the next step.\tUse your own information." }] },
    ideas = installedUseStartingIdeas(first_use);
  assert.equal(ideas.tasks[0].description, "Review each item.\r\nExplain the next step.\tUse your own information.");
  const html = renderToStaticMarkup(React.createElement(InstalledUseStartingIdeas,
    { ideas, onTask: (value) => calls.push(["draft", value]) }));
  assert.deepEqual(calls, [], "Server rendering does not select a suggestion or send a task.");
  assert.match(html, /Review &lt;item&gt;/);
  assert.match(html, /Review each item\. Explain the next step\. Use your own information\./);
  assert.match(html, /preview another idea without changing your writing/);
  assert.doesNotMatch(html, /context context context context context context context context context context context context context/);
  assert.equal(renderToStaticMarkup(React.createElement(InstalledUseStartingIdeas,
    { ideas: { tasks: [], detail: null }, onTask: () => calls.push(["default"]) })), "");
  assert.deepEqual(calls, [], "An absent catalogue fabricates no default task.");
  const requestOnly = renderToStaticMarkup(React.createElement(InstalledUseStartingIdeas,
    { ideas: { tasks: [{ label: "Review", request }] }, onTask: () => calls.push(["unexpected"]) }));
  assert.match(requestOnly, /\.\.\./);
  assert.doesNotMatch(requestOnly, /context context context context context context context context context context context context context/);
  await mountedInstalledReport(noUi, async ({ document, window, calls, admissions }) => {
    const task = document.querySelector(".deployment-use-task-field textarea");
    assert.equal(task.value, "", "Suggestions do not preselect a default task.");
    const button = document.querySelector(".deployment-use-ideas button");
    assert.equal(button.type, "button");
    assert.equal(button.querySelector("strong").textContent, "Review <item>");
    assert.equal(button.querySelector("span").textContent,
      "Review each item. Explain the next step. Use your own information.");
    await act(async () => button.click());
    assert.equal(task.value, request, "A real click fills the complete exact authored request.");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    const edited = request + "\nMy current inventory.";
    await act(async () => {
      setter.call(task, edited); task.dispatchEvent(new window.Event("input", { bubbles: true }));
    });
    assert.equal(task.value, edited, "The complete request remains editable after selection.");
    const help = document.querySelector(".deployment-use-job-help");
    assert.equal(help.open, false, "Optional job help starts folded.");
    help.open = true;
    assert.match(help.textContent, /Get ready|Your own first job/);
    help.open = false;
    assert.equal(document.querySelector(".deployment-use-task-field textarea") === task, true);
    assert.equal(task.value, edited, "Opening job help preserves the exact edited draft.");
    assert.equal(calls.filter((call) => call.method !== "GET").length, 0, "Choosing and editing never sends the task.");
    assert.deepEqual(admissions, []);
  }, { ...metadata, first_use });
  assert.deepEqual(calls, []);
});

test("the improvement form starts closed without hiding sending, failure or submission status", () => {
  const view = InstalledApplicationUseView({ ...baseView, sending: true, error: "A bounded admission error", submitted: true });
  const nodes = elements(view), disclosure = nodes.find((node) => node.props?.className === "deployment-use-feedback-disclosure");
  assert.equal(disclosure.props.open, undefined);
  assert.match(renderToStaticMarkup(disclosure), /Report a problem or request an improvement/);
  const form = elements(disclosure).find((node) => node.type === "form");
  assert.equal(typeof form.props.onSubmit, "function", "The stable report form retains its guarded submission handler");
  assert.equal(elements(disclosure).filter((node) => node.type === "textarea")[0].props.value, report.request);
  for (const role of ["alert", "status"]) {
    const visible = nodes.filter((node) => node.props?.role === role);
    assert.ok(visible.length > 0);
    assert.ok(visible.every((node) => !elements(disclosure).includes(node)));
  }
  assert.match(render({ sending: true, error: "A bounded admission error", submitted: true }),
    /<\/details><p[^>]*role="status">Sending your improvement request.*role="alert">A bounded admission error.*installation has not changed/s);
});

test("the compact handoff keeps the actual identity beside the normal authenticated Console link", () => {
  const view = InstalledApplicationUseView(baseView), nodes = elements(view);
  const entry = nodes.find((node) => node.props?.className === "deployment-use-entry");
  const admin = nodes.find((node) => node.props?.className === "deployment-use-admin");
  const handoff = nodes.find((node) => node.props?.className === "deployment-use-console-handoff");
  assert.ok(elements(entry).some((node) => node.props?.className === "deployment-use-console-handoff"), "The available Console and contact belong to the visible application entrypoint");
  assert.ok(!elements(admin).some((node) => node.props?.className === "deployment-use-console-handoff"));
  assert.equal(elements(admin).filter((node) => node.type === "a").length, 0, "Administration does not duplicate the promoted handoff");
  const link = elements(handoff).find((node) => node.type === "a");
  assert.equal(link.props.href, installed.console_url);
  assert.equal(link.props.target, "_blank");
  assert.equal(new URL(link.props.href).search, ""); assert.equal(new URL(link.props.href).hash, "");
  assert.deepEqual(elements(handoff).filter((node) => node.type === "h5").map((node) => node.props.children), ["Open Console"]);
  const contact = elements(handoff).find((node) => node.props?.className === "deployment-use-contact");
  assert.match(renderToStaticMarkup(contact), /Conversation contact.*Observing-program owner.*lead:main/);
  const send = elements(handoff).find((node) => node.props?.className === "deployment-use-console-send");
  assert.equal(elements(send).filter((node) => node.type === "li").length, 3);
  assert.match(renderToStaticMarkup(send), /If this application uses native chat for everyday work.*Sign in if asked.*Choose.*Observing-program owner.*lead:main.*in the sidebar/s);
  const html = renderToStaticMarkup(handoff);
  assert.match(html, /Observing-program owner/);
  assert.match(html, /lead:main/);
  assert.match(render(), /If this application uses native chat for everyday work/);
  assert.match(render(), /Paste your copied task or type a job, review it, then send it/);
  assert.match(render({ copyStatus: "copied" }), /Task copied\. Use it through the application&#x27;s usual controls\./);
  assert.doesNotMatch(render(), /authorized administration and inspection|application&#x27;s own account and job controls/);
  assert.doesNotMatch(html, /[?&](identity|draft|token)=|iframe/);
  const gated = elements(InstalledApplicationUseView({ ...baseView, selectionBlocked: true }));
  const gatedLink = gated.find((node) => node.type === "a");
  assert.equal(gatedLink.props.href, undefined);
  assert.equal(gatedLink.props["aria-disabled"], true); assert.equal(gatedLink.props.tabIndex, -1);
  assert.equal(gated.find((node) => node.type === "button" && node.props.children === "Copy task").props.disabled, true);
});

const declaredUi = { deployment_id: installed.deployment_id, build_id: installed.build_id,
  package_sha256: archive, source_digest: source, metadata_sha256: "d".repeat(64),
  scope: "declared-source", verified: false,
  entries: [{ id: "workspace", label: "Project workspace", path: "/workspace", port: 8820,
    url: "http://127.0.0.1:8820/workspace", status: "declared", verified: false }] };

async function mountedInstalledReport(experiences, verify, sourceMetadata = metadata, transport = null, initial = {}) {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", webDir))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  if (initial.storage) Object.defineProperty(dom.window, "sessionStorage", { configurable: true, value: initial.storage });
  const calls = [], admissions = [], retries = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: dom.window.sessionStorage,
    IS_REACT_ACT_ENVIRONMENT: true, fetch: async (target, options = {}) => {
      const body = options.body ? JSON.parse(options.body) : null;
      calls.push({ target, method: options.method || "GET", body });
      if (transport) {
        const response = await transport(target, options, body);
        if (response !== undefined) return response;
      }
      if (target === installedUseUrl(guide.id) && !options.method)
        return { ok: true, json: async () => structuredClone(sourceMetadata) };
      assert.equal(target, installedUseUrl(guide.id, "feedback"));
      assert.equal(options.method, "POST");
      return { ok: true, json: async () => ({ guide_id: guide.id, guide_version: guide.version,
        current: true, build_id: "admitted-improvement", status: "queued",
        request: "User's requested improvement:\n" + body.request + "\n\nSynthetic host context: " + JSON.stringify(body) }) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements))
    Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const built = await bundle({ entryPoints: [fileURLToPath(new URL("deployment-use.jsx", import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", loader: { ".css": "empty" },
    nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] });
  const { InstalledApplicationUse, InstalledNativeTeamUse } = await import(
    `data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
  const Component = initial.nativeTeam ? InstalledNativeTeamUse : InstalledApplicationUse;
  const root = createRoot(dom.window.document.getElementById("root"));
  let props = { state: guide, receipt: installed, build: checked, blocked: false,
    onRepairReceipt: (value) => {
      admissions.push(value);
      if (initial.followBuild) {
        props = { ...props, build: value };
        root.render(React.createElement(Component, { ...props, experiences }));
      }
    }, onReloadExperiences: () => retries.push("retry"), Icon: () => null,
    ...initial.props };
  const rerender = (next, flags = {}) => act(async () => root.render(React.createElement(Component,
    { ...props, experiences: next, ...flags })));
  try {
    await rerender(experiences);
    const disclosure = dom.window.document.querySelector(".deployment-use-feedback-disclosure");
    if (initial.expectDisclosure !== false) assert.ok(disclosure);
    if (disclosure) disclosure.open = true;
    await verify({ document: dom.window.document, window: dom.window, calls, admissions, retries, rerender });
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

test("first-use availability recovery uses installation status without recommending Start for a running application", () => {
  for (const [status, instruction] of [["running", /saved installation status is running/],
    ["stopped", /Start the installed application using its installation controls/],
    ["starting", /wait for the installation step to finish/],
    ["failed", /Review installation status and its recovery controls/]]) {
    const receipt = { ...installed, status };
    const use = installedApplicationUse({ ...metadata, status, native_runtime_available: false, console_url: null },
      receipt, guide, checked, noUi);
    const html = render({ use, deploymentStatus: status });
    assert.match(html, instruction);
    assert.match(html, /Check first-use details/);
    assert.doesNotMatch(html, /href=|Retry UI links/);
    if (status !== "stopped") assert.doesNotMatch(html, /Start the installed application/);
  }
});

test("first-use availability recovery rechecks valid unavailable application details through failure and recovery without clearing drafts or command guards", async () => {
  const unavailable = { ...metadata, native_runtime_available: false, console_url: null };
  let response = () => ({ ok: true, json: async () => structuredClone(unavailable) });
  await mountedInstalledReport(noUi, async ui => {
    const { document, calls, admissions } = ui;
    const check = () => namedButton(document, "Check first-use details");
    await fillInstalledReport(ui);
    const taskField = document.querySelector(".deployment-use-field textarea");
    const reportField = document.querySelector(".deployment-use-feedback textarea");
    const disclosure = document.querySelector(".deployment-use-feedback-disclosure");
    const verifyDrafts = () => {
      assertInstalledReportDraft(ui);
      assert.equal(document.querySelector(".deployment-use-field textarea") === taskField, true);
      assert.equal(document.querySelector(".deployment-use-feedback textarea") === reportField, true);
      assert.equal(document.querySelector(".deployment-use-feedback-disclosure") === disclosure, true);
      assert.equal(disclosure.open, true);
      assert.equal(calls.filter(call => call.method !== "GET").length, 0);
      assert.deepEqual(admissions, []);
    };
    assert.match(document.body.textContent, /saved installation status is running/);
    assert.doesNotMatch(document.body.textContent, /Start the installed application|Paste your copied task/);
    assert.equal(document.querySelector('a[href]') === null, true);
    assert.ok(check(), "Verified metadata without Console availability still has a scoped recheck");
    assert.equal(check().disabled, false, "Command custody pauses mutations, not the existing metadata read");
    assert.equal(reportSubmit(document).disabled, true);
    verifyDrafts();

    let finishRead;
    response = () => new Promise(resolve => { finishRead = resolve; });
    await act(async () => check().click());
    assert.equal(namedButton(document, "Checking first-use details...").disabled, true);
    verifyDrafts();
    await act(async () => finishRead({ ok: false, status: 503,
      json: async () => ({ detail: "Synthetic first-use metadata temporarily unavailable." }) }));
    assert.match(document.body.textContent, /first-use details could not be checked/);
    assert.ok(check());
    assert.equal(document.querySelector('a[href]') === null, true);
    verifyDrafts();

    await act(async () => check().click());
    assert.match(document.body.textContent, /Loading this installed application's first-use details/);
    verifyDrafts();
    await act(async () => finishRead({ ok: true, json: async () => structuredClone(metadata) }));
    const consoleLink = document.querySelector('.deployment-use-console-open a');
    assert.ok(consoleLink, "Recovered metadata restores the Console handoff");
    assert.equal(consoleLink.getAttribute("href"), null, "Unconfirmed command custody keeps access disabled");
    assert.equal(consoleLink.getAttribute("aria-disabled"), "true");
    assert.match(document.body.textContent, /For jobs handled through native chat/);
    assert.equal(check() === undefined, true);
    assert.equal(reportSubmit(document).disabled, true);
    assert.equal(namedButton(document, "It worked").disabled, true);
    verifyDrafts();
    assert.equal(calls.length, 3);
  }, unavailable, (target, options) => {
    assert.equal(target, installedUseUrl(guide.id));
    assert.equal(options.method || "GET", "GET");
    return response();
  }, { props: { blocked: true, commandUnconfirmed: true } });
});

test("first-use availability recovery rechecks the verified native owner through failed and successful reads without any installation command", async () => {
  const receipt = { ...installed, source_kind: "native-team", source_id: nextSource, build_id: null };
  const native = { ...metadata, source_kind: "native-team", source_id: nextSource, build_id: null,
    continuation: { available: false }, success_reporting_available: false };
  const unavailable = { ...native, native_runtime_available: false, console_url: null };
  let response = () => ({ ok: true, json: async () => structuredClone(unavailable) });
  await mountedInstalledReport(null, async ({ document, calls, admissions, rerender }) => {
    const check = () => namedButton(document, "Recheck agent team details");
    assert.match(document.body.textContent, /checked installed conversation owner is Observing-program owner/);
    assert.match(document.body.textContent, /saved installation status is running/);
    assert.doesNotMatch(document.body.textContent, /Start this installed team|Open the Console and select/);
    assert.equal(document.querySelector('a[href]') === null, true);
    assert.ok(check(), "A verified owner with unavailable Console must remain recheckable");
    assert.equal(check().disabled, false);

    let finishRead;
    response = () => new Promise(resolve => { finishRead = resolve; });
    await act(async () => check().click());
    assert.match(document.body.textContent, /Checking this installed team's conversation owner/);
    assert.equal(check() === undefined, true);
    await act(async () => finishRead({ ok: false, status: 503,
      json: async () => ({ detail: "Synthetic native first-use metadata temporarily unavailable." }) }));
    assert.match(document.body.textContent, /conversation owner could not be checked/);
    assert.ok(check());
    assert.equal(document.querySelector('a[href]') === null, true);

    await act(async () => check().click());
    assert.match(document.body.textContent, /Checking this installed team's conversation owner/);
    await act(async () => finishRead({ ok: true, json: async () => structuredClone(native) }));
    assert.equal(document.querySelector('a[href]').href, installed.console_url);
    assert.match(document.body.textContent, /Open the Console and select Observing-program owner/);
    assert.equal(check() === undefined, true);
    assert.equal(calls.length, 3);
    for (const [status, instruction] of [["stopped", /Start this installed team using its installation controls/],
      ["starting", /wait for the installation step to finish/],
      ["failed", /Review installation status and its recovery controls/]]) {
      response = () => ({ ok: true, json: async () => ({ ...unavailable, status }) });
      await rerender(null, { receipt: { ...receipt, status } });
      assert.match(document.body.textContent, instruction);
      assert.ok(check());
      assert.equal(document.querySelector('a[href]') === null, true);
      if (status === "stopped") assert.match(document.body.textContent, /Your agent team is stopped/);
      else assert.doesNotMatch(document.body.textContent, /Start this installed team/);
    }
    assert.ok(calls.every(call => call.method === "GET"));
    assert.deepEqual(admissions, []);
  }, unavailable, (target, options) => {
    assert.equal(target, installedUseUrl(guide.id));
    assert.equal(options.method || "GET", "GET");
    return response();
  }, { nativeTeam: true, expectDisclosure: false, props: { receipt, build: null, blocked: true } });
});

for (const [surface, experiences] of [["declared application UI", declaredUi], ["Console-only conversation", noUi]]) {
  test(`the mounted ${surface} report explains the missing outcome without changing drafts or sending automatically`, async () => {
    await mountedInstalledReport(experiences, async ({ document, window, calls, admissions }) => {
      const message = "  Sign-in stopped here.\nPlease keep my \u00e5 \u{1f9ed} context.  ";
      const form = document.querySelector(".deployment-use-feedback");
      const textarea = form.querySelector("textarea");
      const button = form.querySelector('button[type="submit"]');
      const radios = [...form.querySelectorAll('input[type="radio"]')];
      assert.ok(radios.every((radio) => !radio.checked && radio.required));
      const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
      await act(async () => {
        setter.call(textarea, message);
        textarea.dispatchEvent(new window.Event("input", { bubbles: true }));
      });
      assert.equal(textarea.value, message);
      assert.equal(button.disabled, true);
      assert.match(form.textContent, /Choose what happened above before sending/);
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
      await act(async () => radios.find((radio) => radio.value === "blocked").click());
      assert.equal(button.disabled, false);
      assert.doesNotMatch(form.textContent, /Choose what happened above before sending/);
      if (experiences.entries.length) {
        assert.match(form.textContent, /which application page or sign-in step stopped you/);
        assert.match(form.textContent, /native Console is for administration and inspection/);
        assert.doesNotMatch(form.textContent, /from opening or using the application conversation/);
      } else {
        assert.match(form.textContent, /from opening or using the application, what you saw/);
        assert.doesNotMatch(form.textContent, /which application page or sign-in step|native Console is for administration/);
      }
      assert.equal(textarea.value, message);
      assert.equal(document.querySelector(".deployment-use-field textarea").value, "");
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
      await act(async () => button.click());
      const posts = calls.filter((call) => call.method === "POST");
      assert.equal(posts.length, 1);
      assert.deepEqual(posts[0].body, { version: guide.version, deployment_id: installed.deployment_id,
        build_id: installed.build_id, source_digest: source, package_sha256: archive,
        continuation_build_id: checked.build_id, continuation_source_digest: source,
        continue_current_candidate: false, outcome: "blocked", request: message.trim(), task: "" });
      assert.equal(admissions.length, 1);
      assert.equal(admissions[0].build_id, "admitted-improvement");
      assert.equal(textarea.value, message);
      const scope = installedUseScopeKey(installed, guide);
      assert.deepEqual(createInstalledUseDraftStore(window.sessionStorage).read(scope),
        { task: "", outcome: "blocked", improvement: message });
      assert.match(document.body.textContent, /installation has not changed/);
    });
  });
}

const feedbackTask = "  Compare my supplied context \u{1f9ed}\nwithout discarding the original.  ";
const feedbackRequest = "  Keep the saved result and explain the corrected evidence.  ";
const buildStatusUrl = `/api/guide/${guide.id}/implementation`;
const feedbackResponse = (value) => ({ ok: true, json: async () => structuredClone(value) });
const rejectedFeedbackResponse = (status, code) => ({ ok: false, status,
  json: async () => ({ detail: "Synthetic pre-submission rejection.", ...(code ? { code } : {}) }) });
const reportSubmit = (document) => document.querySelector('.deployment-use-feedback button[type="submit"]');
const namedButton = (document, label) => [...document.querySelectorAll("button")].find((button) => button.textContent === label);
async function fillInstalledReport({ document, window }, improvement = feedbackRequest) {
  const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
  await act(async () => {
    const task = document.querySelector(".deployment-use-field textarea"), note = document.querySelector(".deployment-use-feedback textarea");
    setter.call(task, feedbackTask); task.dispatchEvent(new window.Event("input", { bubbles: true }));
    setter.call(note, improvement); note.dispatchEvent(new window.Event("input", { bubbles: true }));
    document.querySelector('input[value="partly"]').click();
  });
}
function assertInstalledReportDraft(ui, improvement = feedbackRequest) {
  assert.equal(ui.document.querySelector(".deployment-use-field textarea").value, feedbackTask);
  assert.equal(ui.document.querySelector(".deployment-use-feedback textarea").value, improvement);
  assert.deepEqual(createInstalledUseDraftStore(ui.window.sessionStorage).read(installedUseScopeKey(installed, guide)),
    { task: feedbackTask, outcome: "partly", improvement });
}

test("an explicitly rejected local report keeps its draft, refreshes setup and remains usable after reload", async () => {
  for (const status of [409, 422]) {
    const storage = draftStorage();
    await mountedInstalledReport(noUi, async ui => {
      await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click());
      assert.match(ui.document.body.textContent, /builder did not receive this report/);
      assert.doesNotMatch(ui.document.body.textContent, /Admission.*unconfirmed/);
      assert.equal(ui.admissions.length, 0); assertInstalledReportDraft(ui);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
      assert.equal(ui.calls.filter(call => call.target === installedUseUrl(guide.id) && call.method === "GET").length, 2);
      assert.equal(reportSubmit(ui.document).disabled, false);
      assert.equal(namedButton(ui.document, "Check Build status") === undefined, true);
    }, metadata, (target, options) => options.method === "POST"
      ? rejectedFeedbackResponse(status, "installed_feedback_not_admitted") : undefined, { storage });
    await mountedInstalledReport(noUi, async ui => {
      assertInstalledReportDraft(ui); assert.equal(reportSubmit(ui.document).disabled, false);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
      await act(async () => reportSubmit(ui.document).click());
      assert.equal(ui.admissions.length, 1);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
    }, metadata, null, { storage });
  }
});

test("an uncoded local409 or422 remains uncertain and cannot resend against its unchanged predecessor", async () => {
  for (const status of [409, 422]) await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click());
    assert.match(ui.document.body.textContent, /Admission.*unconfirmed/); assertInstalledReportDraft(ui);
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assert.equal(reportSubmit(ui.document).disabled, true);
    assert.equal(namedButton(ui.document, "Prepare another report") === undefined, true);
    await act(async () => ui.document.querySelector(".deployment-use-feedback").dispatchEvent(
      new ui.window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
  }, metadata, (target, options) => target === buildStatusUrl ? feedbackResponse(checked)
    : options.method === "POST" ? rejectedFeedbackResponse(status) : undefined);
});

test("the exact local improvement completes visibly without a manual Build check or applying the candidate", async () => {
  await mountedInstalledReport(noUi, async (ui) => {
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    const handoff = () => ui.document.querySelector(".installed-improvement-handoff");
    await ui.rerender(noUi, { build: { ...improvedBuild, status: "queued", ready_to_package: false } });
    assert.match(handoff().textContent, /Your improvement is being built/);
    await ui.rerender(noUi, { build: improvedBuild });
    assert.match(handoff().textContent, /Your improvement is ready to review/);
    assert.equal(handoff().querySelector("a").getAttribute("href"), "#setup-implementation");
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 1);
    assert.equal(ui.calls.filter(call => call.target === buildStatusUrl).length, 0);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assertInstalledReportDraft(ui);
    await ui.rerender(noUi, { build: { ...improvedBuild, build_id: "unrelated-candidate" } });
    assert.doesNotMatch(handoff().textContent, /ready to review/);
    assert.equal(handoff().querySelector("a") === null, true);
  });
});

test("a confirmed installed improvement remains submitted through a harmless revision and cannot send again", async () => {
  let version = guide.version;
  await mountedInstalledReport(noUi, async (ui) => {
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    assert.equal(ui.admissions.length, 1);
    assert.equal(reportSubmit(ui.document).disabled, true);
    version++;
    await ui.rerender(noUi, { state: { ...guide, version }, build: { ...checked, current_guide_version: version } });
    assertInstalledReportDraft(ui);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assert.match(ui.document.body.textContent, /request for proposal 16 was admitted/);
    await act(async () => ui.document.querySelector(".deployment-use-feedback").dispatchEvent(
      new ui.window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
  }, metadata, (target, options) => target === installedUseUrl(guide.id) && !options.method
    ? feedbackResponse({ ...metadata, version }) : undefined);
});

test("a parent display callback error cannot undo a confirmed host admission", async () => {
  await mountedInstalledReport(noUi, async (ui) => {
    await ui.rerender(noUi, { onRepairReceipt: () => { throw new Error("Synthetic display callback failure."); } });
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    assert.match(ui.document.body.textContent, /request for proposal 16 was admitted/);
    assert.doesNotMatch(ui.document.body.textContent, /Admission.*unconfirmed/);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assertInstalledReportDraft(ui);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
  });
});

test("reloading a confirmed local improvement rechecks the exact saved host admission and retains drafts", async () => {
  const storage = draftStorage(); let admitted;
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click());
    admitted = ui.admissions[0]; assert.ok(admitted.request);
  }, metadata, null, { storage });
  const candidate = { ...improvedBuild, request: admitted.request, current_guide_version: guide.version + 1 };
  await mountedInstalledReport(noUi, async ui => {
    assert.equal(ui.calls.filter(call => call.target === buildStatusUrl && call.method === "GET").length, 1);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    assert.match(ui.document.body.textContent, /Your improvement is ready to review/);
    assert.equal(ui.document.querySelector(".installed-improvement-handoff a").getAttribute("href"), "#setup-implementation");
    assert.equal(reportSubmit(ui.document).disabled, true); assertInstalledReportDraft(ui);
    await act(async () => namedButton(ui.document, "Prepare another report").click());
    assert.equal(reportSubmit(ui.document).disabled, true, "Preparing another report cannot resend the retained report.");
    assert.match(ui.document.body.textContent, /Change your report before sending/);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
  }, { ...metadata, version: guide.version + 1, continuation: { build_id: candidate.build_id,
    source_digest: candidate.source_digest, available: true, requires_acknowledgement: true } },
  (target, options) => target === buildStatusUrl && !options.method ? feedbackResponse(candidate) : undefined,
  { storage, props: { state: { ...guide, version: guide.version + 1 }, build: candidate } });
});

test("local reload hints cannot borrow a replacement build, different host report or unverifiable admission", async () => {
  const storage = draftStorage(); let admitted;
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click()); admitted = ui.admissions[0];
  }, metadata, null, { storage });
  for (const change of [{ build_id: "replacement-build" }, { request: "A different source-bound host report" },
    { guide_version: guide.version + 1 }, { guide_id: "foreign" }, null]) {
    const candidate = { ...improvedBuild, request: admitted.request, ...change };
    await mountedInstalledReport(noUi, async ui => {
      assert.equal(ui.document.querySelector(".installed-improvement-handoff") === null, true);
      assert.doesNotMatch(ui.document.body.textContent, /ready to review|request for proposal \d+ was admitted/);
      assert.equal(reportSubmit(ui.document).disabled, true); assertInstalledReportDraft(ui);
      await act(async () => ui.document.querySelector("form").dispatchEvent(new ui.window.Event("submit", { bubbles: true, cancelable: true })));
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    }, metadata, (target, options) => {
      if (target === buildStatusUrl && !options.method) {
        if (change === null) throw new TypeError("Synthetic unavailable read.");
        return feedbackResponse(candidate);
      }
    }, { storage, props: { build: candidate } });
  }
});

test("local reload waits for checked installed metadata when its receipt omits the source digest", async () => {
  const storage = draftStorage(); let admitted, finishSource;
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click()); admitted = ui.admissions[0];
  }, metadata, null, { storage });
  const candidate = { ...improvedBuild, request: admitted.request };
  const { source_digest, ...ordinaryReceipt } = installed;
  assert.equal(source_digest, source);
  await mountedInstalledReport(noUi, async ui => {
    assert.equal(ui.document.querySelector(".installed-improvement-handoff") === null, true);
    assert.doesNotMatch(ui.document.body.textContent, /ready to review/);
    await act(async () => finishSource(feedbackResponse({ ...metadata, continuation: {
      build_id: candidate.build_id, source_digest: candidate.source_digest, available: true, requires_acknowledgement: true } })));
    assert.match(ui.document.body.textContent, /Your improvement is ready to review/);
    assertInstalledReportDraft(ui);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
  }, metadata, (target, options) => {
    if (target === installedUseUrl(guide.id) && !options.method) return new Promise(resolve => { finishSource = resolve; });
    if (target === buildStatusUrl && !options.method) return feedbackResponse(candidate);
  }, { storage, props: { receipt: ordinaryReceipt, build: candidate }, expectDisclosure: false });
});

test("local reload keeps the joined improvement when the parent immediately publishes the recovered candidate", async () => {
  const storage = draftStorage(); let admitted;
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click()); admitted = ui.admissions[0];
  }, metadata, null, { storage });
  const candidate = { ...improvedBuild, request: admitted.request };
  await mountedInstalledReport(noUi, async ui => {
    assert.match(ui.document.body.textContent, /Your improvement is ready to review/);
    assert.equal(ui.admissions.length, 1);
    assert.equal(ui.admissions[0].build_id, candidate.build_id);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    assertInstalledReportDraft(ui);
  }, { ...metadata, continuation: { build_id: candidate.build_id, source_digest: candidate.source_digest,
    available: true, requires_acknowledgement: true } },
  (target, options) => target === buildStatusUrl && !options.method ? feedbackResponse(candidate) : undefined,
  { storage, followBuild: true });
});

test("a stale automatic local recovery read cannot overwrite a new proposal revision", async () => {
  const storage = draftStorage(); let admitted, finish;
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click()); admitted = ui.admissions[0];
  }, metadata, null, { storage });
  const candidate = { ...improvedBuild, request: admitted.request };
  let version = guide.version;
  await mountedInstalledReport(noUi, async ui => {
    version++;
    await ui.rerender(noUi, { state: { ...guide, version }, build: { ...candidate, current_guide_version: version } });
    await act(async () => finish(feedbackResponse(candidate)));
    assert.equal(ui.document.querySelector(".installed-improvement-handoff") === null, true);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assert.equal(ui.admissions.length, 0);
    assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    assertInstalledReportDraft(ui);
  }, metadata, (target, options) => {
    if (target === installedUseUrl(guide.id) && !options.method) return feedbackResponse({ ...metadata, version });
    if (target === buildStatusUrl && !options.method) return new Promise(resolve => { finish = resolve; });
  }, { storage, props: { build: candidate } });
});

test("local reload readiness comes from fresh acceptance and cleanup, never the saved navigation hint", async () => {
  const storage = draftStorage(); let admitted;
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click()); admitted = ui.admissions[0];
  }, metadata, null, { storage });
  for (const change of [{ cleanup_pending: true }, { source_verified: false }, { ready_to_package: false },
    { acceptance_trusted: false }, { acceptance: { ...improvedBuild.acceptance, status: "needs_repair" } }]) {
    const candidate = { ...improvedBuild, request: admitted.request, ...change };
    await mountedInstalledReport(noUi, async ui => {
      assert.ok(ui.document.querySelector(".installed-improvement-handoff"));
      assert.doesNotMatch(ui.document.body.textContent, /ready to review/);
      assert.equal(reportSubmit(ui.document).disabled, true);
      assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    }, metadata, (target, options) => target === buildStatusUrl && !options.method ? feedbackResponse(candidate) : undefined,
    { storage, props: { build: candidate } });
  }
});

test("a lost local feedback response stays uncertain through reload and cannot release its unchanged report", async () => {
  const storage = draftStorage();
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click());
    assert.match(ui.document.body.textContent, /Admission.*unconfirmed/);
  }, metadata, (target, options) => {
    if (target === installedUseUrl(guide.id, "feedback") && options.method === "POST") throw new TypeError("Synthetic lost reply.");
  }, { storage });
  let candidate = checked;
  await mountedInstalledReport(noUi, async ui => {
    assert.match(ui.document.body.textContent, /Admission.*unconfirmed/);
    assert.equal(reportSubmit(ui.document).disabled, true); assertInstalledReportDraft(ui);
    assert.equal(namedButton(ui.document, "Prepare another report") === undefined, true);
    candidate = { ...improvedBuild, build_id: "later-observed-build" };
    await ui.rerender(noUi, { build: candidate });
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assert.match(ui.document.body.textContent, /does not confirm whether your earlier report was admitted/);
    await act(async () => namedButton(ui.document, "Prepare another report").click());
    await act(async () => ui.document.querySelector(".deployment-use-acknowledgement input").click());
    await act(async () => ui.document.querySelector("form").dispatchEvent(new ui.window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(reportSubmit(ui.document).disabled, true); assert.equal(ui.calls.filter(call => call.method === "POST").length, 0);
    assert.match(ui.document.body.textContent, /Change your report before sending/);
  }, metadata, (target, options) => {
    if (target === buildStatusUrl && !options.method) return feedbackResponse(candidate);
    if (target === installedUseUrl(guide.id) && !options.method) return feedbackResponse({ ...metadata,
      continuation: { build_id: candidate.build_id, source_digest: candidate.source_digest, available: true,
        requires_acknowledgement: candidate.build_id !== checked.build_id } });
  }, { storage });
});

test("unavailable improvement storage keeps the ordinary local report usable and states its reload limitation", async () => {
  const storage = { getItem() { throw new Error("PRIVATE_STORAGE_DETAIL"); }, setItem() { throw new Error("QUOTA_DETAIL"); }, removeItem() {} };
  await mountedInstalledReport(noUi, async ui => {
    await fillInstalledReport(ui); await act(async () => reportSubmit(ui.document).click());
    assert.equal(ui.admissions.length, 1);
    assert.match(ui.document.body.textContent, /Improvement tracking is available in this open page only/);
    assert.doesNotMatch(ui.document.body.textContent, /PRIVATE_STORAGE_DETAIL|QUOTA_DETAIL/);
  }, metadata, null, { storage });
});

test("lost installed feedback requires actual Build reads and a separate deliberate report after a checked new candidate", async () => {
  let candidate = structuredClone(checked), postCount = 0;
  await mountedInstalledReport(noUi, async (ui) => {
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    assert.match(ui.document.body.textContent, /Admission of your improvement request is unconfirmed.*may have reached/s);
    assert.equal(reportSubmit(ui.document).disabled, true);
    await act(async () => ui.document.querySelector(".deployment-use-feedback").dispatchEvent(
      new ui.window.Event("submit", { bubbles: true, cancelable: true })));
    assert.equal(postCount, 1);
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assert.equal(ui.calls.filter((call) => call.target === buildStatusUrl && call.method === "GET").length, 1);
    assert.match(ui.document.body.textContent, /current candidate with status completed.*does not confirm whether your earlier report was admitted/s);
    assert.equal(namedButton(ui.document, "Prepare another report") === undefined, true, "The old terminal candidate cannot resolve a lost admission.");
    candidate = { ...checked, build_id: "observed-current-repair", source_digest: nextSource, status: "running" };
    await ui.rerender(noUi, { build: candidate });
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assert.equal(ui.admissions.at(-1).build_id, candidate.build_id, "Only the actual status receipt reaches Build.");
    assert.match(ui.document.body.textContent, /status running.*does not confirm whether your earlier report was admitted/s);
    assert.equal(namedButton(ui.document, "Prepare another report") === undefined, true);
    candidate = { ...candidate, status: "completed", cleanup_pending: false };
    await ui.rerender(noUi, { build: candidate });
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assertInstalledReportDraft(ui);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assert.equal(postCount, 1);
    await act(async () => namedButton(ui.document, "Prepare another report").click());
    assertInstalledReportDraft(ui);
    assert.equal(postCount, 1, "Preparing a report is not a resend.");
    const nextRequest = "A separate follow-up: keep the previous result and show the evidence revision.";
    await fillInstalledReport(ui, nextRequest);
    await act(async () => ui.document.querySelector(".deployment-use-acknowledgement input").click());
    await act(async () => reportSubmit(ui.document).click());
    assert.equal(postCount, 2);
    const posts = ui.calls.filter((call) => call.method === "POST");
    assert.equal(posts[1].body.request, nextRequest);
    assert.equal(posts[1].body.continuation_build_id, candidate.build_id);
    assert.equal(posts[1].body.continuation_source_digest, nextSource);
    assert.equal(posts[1].body.source_digest, source, "Installed source ownership remains distinct.");
    assertInstalledReportDraft(ui, nextRequest);
  }, metadata, (target, options, body) => {
    if (target === installedUseUrl(guide.id) && !options.method) return feedbackResponse({ ...metadata,
      continuation: { build_id: candidate.build_id, source_digest: candidate.source_digest,
        available: candidate.status === "completed", requires_acknowledgement: candidate.build_id !== checked.build_id } });
    if (target === buildStatusUrl && !options.method) return feedbackResponse(candidate);
    if (target === installedUseUrl(guide.id, "feedback") && options.method === "POST") {
      if (++postCount === 1) throw new TypeError("Lost reply after possible admission.");
      return feedbackResponse({ guide_id: guide.id, guide_version: body.version, build_id: "new-explicit-report", current: true, status: "queued" });
    }
  });
});

for (const failure of [false, true]) test(`a delayed ${failure ? "lost" : "confirmed"} feedback reply retains its original revision after setup changes`, async () => {
  let version = guide.version, finish;
  await mountedInstalledReport(noUi, async (ui) => {
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    assert.equal(reportSubmit(ui.document).disabled, true);
    version++;
    await ui.rerender(noUi, { state: { ...guide, version }, build: { ...checked, current_guide_version: version } });
    assertInstalledReportDraft(ui);
    assert.equal(reportSubmit(ui.document).disabled, true, "A metadata reload cannot release an in-flight admission.");
    await act(async () => finish());
    assert.equal(ui.admissions.length, 0, "An old reply cannot replace the newer selected context.");
    assert.match(ui.document.body.textContent, failure ? /Admission.*unconfirmed/ : /request for proposal 16 was admitted/);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assertInstalledReportDraft(ui);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
  }, metadata, (target, options) => {
    if (target === installedUseUrl(guide.id) && !options.method) return feedbackResponse({ ...metadata, version });
    if (target === installedUseUrl(guide.id, "feedback") && options.method === "POST") return new Promise((resolve, reject) => {
      finish = () => failure ? reject(new TypeError("Lost admission reply."))
        : resolve(feedbackResponse({ guide_id: guide.id, guide_version: guide.version, build_id: "earlier-admitted", current: true, status: "queued" }));
    });
  });
});

test("a stale or unavailable Build read never clears an unconfirmed installed report", async () => {
  let version = guide.version, finish, reads = 0;
  await mountedInstalledReport(noUi, async (ui) => {
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    await act(async () => namedButton(ui.document, "Check Build status").click());
    version++;
    await ui.rerender(noUi, { state: { ...guide, version }, build: { ...checked, current_guide_version: version } });
    await act(async () => finish(feedbackResponse({ ...checked, build_id: "stale-status", source_digest: nextSource })));
    assert.doesNotMatch(ui.document.body.textContent, /Build reports a/);
    assert.equal(namedButton(ui.document, "Prepare another report") === undefined, true);
    assert.equal(reportSubmit(ui.document).disabled, true);
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assert.match(ui.document.body.textContent, /Build status could not be checked/);
    assert.match(ui.document.body.textContent, /Admission.*unconfirmed/);
    assertInstalledReportDraft(ui);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.admissions.length, 0);
  }, metadata, (target, options) => {
    if (target === installedUseUrl(guide.id) && !options.method) return feedbackResponse({ ...metadata, version });
    if (target === installedUseUrl(guide.id, "feedback") && options.method === "POST") throw new TypeError("Lost reply.");
    if (target === buildStatusUrl && !options.method) {
      if (++reads === 1) return new Promise((resolve) => { finish = resolve; });
      throw new TypeError("Status unavailable.");
    }
  });
});

test("foreign admission and Build replies cannot confirm installed feedback or unlock another send", async () => {
  await mountedInstalledReport(noUi, async (ui) => {
    await fillInstalledReport(ui);
    await act(async () => reportSubmit(ui.document).click());
    assert.match(ui.document.body.textContent, /Admission.*unconfirmed/);
    await act(async () => namedButton(ui.document, "Check Build status").click());
    assert.match(ui.document.body.textContent, /Build status could not be checked/);
    assert.doesNotMatch(ui.document.body.textContent, /Build reports a|is with the builder/);
    assert.equal(namedButton(ui.document, "Prepare another report") === undefined, true);
    assert.equal(reportSubmit(ui.document).disabled, true);
    assertInstalledReportDraft(ui);
    assert.equal(ui.admissions.length, 0);
    assert.equal(ui.calls.filter((call) => call.method === "POST").length, 1);
  }, metadata, (target, options) => {
    if (target === installedUseUrl(guide.id, "feedback") && options.method === "POST") return feedbackResponse({
      guide_id: "another-guide", guide_version: guide.version, build_id: "foreign-admission", current: true, status: "queued" });
    if (target === buildStatusUrl && !options.method) return feedbackResponse({ ...checked, guide_id: "another-guide" });
  });
});

test("the production handoff puts the exact installed declared UI before secondary native Console without sending a task", () => {
  const before = structuredClone({ declaredUi, metadata, installed, guide, checked }), calls = [];
  const use = installedApplicationUse(metadata, installed, guide, checked, declaredUi);
  const view = InstalledApplicationUseView({ ...baseView, use,
    onTask: () => calls.push("draft"), onSubmit: () => calls.push("feedback"), onCopyTask: () => calls.push("copy") });
  const nodes = elements(view), links = nodes.filter((node) => node.type === "a");
  assert.deepEqual(links.map((link) => link.props.href), [declaredUi.entries[0].url, installed.console_url]);
  assert.equal(links[0].props.className, "setup-action primary");
  assert.equal(links[1].props.className, "setup-action");
  assert.equal(nodes.find((node) => node.type === "button" && node.props.children === "Copy task").props.className, "setup-action");
  const admin = nodes.find((node) => node.props?.className === "deployment-use-admin");
  const entry = nodes.find((node) => node.props?.className === "deployment-use-entry");
  assert.deepEqual(elements(entry).filter((node) => node.type === "a").map((node) => node.props.href), [declaredUi.entries[0].url]);
  assert.ok(!elements(entry).some((node) => node.props?.className === "deployment-use-console-handoff"));
  assert.equal(admin.type, "details"); assert.equal(admin.props.open, undefined);
  assert.ok(elements(admin).some((node) => node.type === "a" && node.props.href === installed.console_url));
  assert.ok(!elements(admin).some((node) => node.type === "a" && node.props.href === declaredUi.entries[0].url));
  const help = nodes.find((node) => node.props?.className === "deployment-use-job-help");
  assert.equal(help.type, "details"); assert.equal(help.props.open, undefined);
  assert.equal(links[0].props.target, "_blank");
  const html = renderToStaticMarkup(view);
  assert.match(html, /Open Project workspace/);
  assert.ok(html.indexOf("Open Project workspace") < html.indexOf("What would you like it to do first?"));
  assert.match(html, /declared by this installed package/);
  assert.match(html, /reachability, authentication and application workflows have not been verified/);
  assert.match(html, /Open native Console/);
  assert.match(html, /authorized administration and inspection/);
  assert.match(html, /application&#x27;s own account and job controls for everyday work/);
  assert.match(html, /does not send a request or approve an action/);
  assert.doesNotMatch(html, /Open application conversation|conversation fallback|Paste your copied task or type a job|send it yourself in the conversation|iframe|[?&](identity|draft|token|task)=/);
  const copied = render({ use, copyStatus: "copied" });
  assert.match(copied, /Task copied\. Use it in the application UI\./);
  assert.doesNotMatch(copied, /native conversation you open|Paste it into the application conversation/);
  assert.equal(nodes.find((node) => node.type === "textarea").props.value, report.task);
  assert.deepEqual(calls, []);
  assert.deepEqual({ declaredUi, metadata, installed, guide, checked }, before);
  const body = installedUseFeedback(metadata, installed, guide, checked, report);
  assert.equal(body.task, report.task);
  assert.equal(body.source_digest, source);
});

test("multiple source-declared UIs remain labelled choices without a fabricated default or verified workflow", () => {
  const second = { ...declaredUi.entries[0], id: "overview", label: "Overview <panel>", path: "/overview",
    url: "http://127.0.0.1:8820/overview" };
  const use = installedApplicationUse(metadata, installed, guide, checked,
    { ...declaredUi, entries: [declaredUi.entries[0], second] });
  const html = render({ use }), links = elements(InstalledApplicationUseView({ ...baseView, use })).filter((node) => node.type === "a");
  assert.match(html, /Choose an application UI/);
  assert.match(html, /Overview &lt;panel&gt;/);
  assert.deepEqual(links.map((node) => node.props.href), [declaredUi.entries[0].url, second.url, installed.console_url]);
  assert.equal(links.filter((node) => node.props.className.includes("primary")).length, 0);
  assert.equal(elements(InstalledApplicationUseView({ ...baseView, use }))
    .find((node) => node.type === "button" && node.props.children === "Copy task").props.className, "setup-action primary");
  const choices = elements(InstalledApplicationUseView({ ...baseView, use }))
    .find((node) => node.props?.className === "deployment-use-interfaces");
  assert.doesNotMatch(renderToStaticMarkup(choices), /selected|recommended|automatically opened|workflow verified/i);
});

test("installed UI authority comes from checked A even without a receipt digest and never from selected newer B", () => {
  const receipt = { ...installed, current: false }; delete receipt.source_digest;
  const candidate = { ...checked, build_id: "newer-B", source_digest: nextSource, status: "failed", source_verified: false };
  const useMetadata = { ...metadata, current: false, continuation: { build_id: candidate.build_id,
    source_digest: candidate.source_digest, available: false, requires_acknowledgement: true } };
  const use = installedApplicationUse(useMetadata, receipt, guide, candidate, declaredUi);
  assert.equal(use.interfaces.entries[0].url, declaredUi.entries[0].url);
  assert.equal(use.continuation.available, false);
  assert.match(render({ use }), /These use instructions describe the installed version/);
  for (const change of [{ deployment_id: "other-install" }, { build_id: candidate.build_id },
    { package_sha256: nextSource }, { source_digest: nextSource }]) {
    const stale = installedApplicationUse(useMetadata, receipt, guide, candidate, { ...declaredUi, ...change });
    assert.deepEqual(stale.interfaces.entries, []);
    assert.ok(stale.interfaces.issue);
    assert.doesNotMatch(render({ use: stale }), /8820\/workspace/);
    assert.equal(stale.consoleUrl, installed.console_url);
  }
});

test("stopped and unavailable declared interfaces preserve the editable task without clickable UI or fabricated health", () => {
  for (const change of [{ status: "stopped" }, { health: "failed" }, { process_live: false },
    { bootstrap: { complete: false, ready: false } }]) {
    const receipt = { ...installed, ...change };
    const use = installedApplicationUse({ ...metadata, status: receipt.status }, receipt, guide, checked, declaredUi);
    assert.equal(use.interfaces.entries[0].url, null);
    assert.equal(use.interfaces.entries[0].verified, false);
    const html = render({ use, deploymentStatus: receipt.status });
    assert.match(html, /Project workspace/);
    assert.match(html, /Your draft stays here/);
    assert.match(html, /Then use its declared application UI/);
    if (receipt.status === "stopped") {
      assert.match(html, /Your application is stopped/);
      assert.match(html, /Your files and saved work are kept\. Start the installed application to open its interfaces\./);
      assert.match(html, /Open after starting the application/);
      assert.doesNotMatch(html, /Use Observing programme|Open the application UI|Sign in if asked/);
      assert.ok(html.indexOf("Your application is stopped") < html.indexOf("Open after starting the application"));
    } else {
      assert.match(html, /Use Observing programme|Open the application UI/);
      assert.doesNotMatch(html, /Your application is stopped/);
    }
    assert.doesNotMatch(html, /Its conversation opens after/);
    assert.doesNotMatch(html, /href=|Open native Console/);
    assert.doesNotMatch(html, /deployment-use-console-handoff/);
    assert.match(html, /Check the forecast for tonight/);
  }
  const unavailable = installedApplicationUse({ ...metadata, native_runtime_available: false }, installed, guide, checked, declaredUi);
  assert.equal(unavailable.interfaces.entries[0].url, null);
  const missing = installedApplicationUse(metadata, installed, guide, checked,
    { ...declaredUi, entries: [{ ...declaredUi.entries[0], url: null, reason: "Configure the declared nonsecret port in private setup." }] });
  assert.match(render({ use: missing }), /Configure the declared nonsecret port in private setup/);
  assert.doesNotMatch(render({ use: missing }), /8820\/workspace/);
  assert.equal(missing.consoleUrl, installed.console_url);
  assert.match(render({ use: missing }), /Open native Console.*authorized administration and inspection/s);
  assert.doesNotMatch(render({ use: missing }), /conversation fallback|Paste your copied task or type a job|Open application conversation/);
});

test("unresolved UI discovery keeps retry ahead of the optional draft and native Console neutral", () => {
  for (const value of [null,
    { ...declaredUi, error: "DO_NOT_RENDER_RAW_ERROR" },
    { ...declaredUi, entries: [{ ...declaredUi.entries[0], url: "https://outside.invalid/workspace" }] },
    { ...declaredUi, entries: [{ ...declaredUi.entries[0], url: "http://127.0.0.1:8820/workspace?token=private" }] }]) {
    const use = installedApplicationUse(metadata, installed, guide, checked, value), html = render({ use });
    const nodes = elements(InstalledApplicationUseView({ ...baseView, use }));
    const phase = nodes.find((node) => node.props?.className === "deployment-use-phase deployment-use-handoff");
    const admin = nodes.find((node) => node.props?.className === "deployment-use-admin");
    assert.equal(elements(phase).filter((node) => node.type === "a").length, 0);
    assert.deepEqual(elements(admin).filter((node) => node.type === "a").map((node) => node.props.href), [installed.console_url]);
    assert.deepEqual(use.interfaces.entries, []);
    assert.equal(use.interfaces.status, "unresolved");
    assert.match(html, /Retry UI links/);
    assert.ok(html.indexOf("Retry UI links") < html.indexOf("What would you like it to do first?"));
    assert.match(html, /Open native Console/);
    assert.match(html, /authorized administration and inspection/);
    assert.match(html, /Check the forecast for tonight/);
    assert.doesNotMatch(html, /Open application conversation|Paste your copied task or type a job/);
    assert.doesNotMatch(html, /outside\.invalid|token=private|DO_NOT_RENDER_RAW_ERROR|8820\/workspace/);
    assert.doesNotMatch(html, /deployment-use-console-handoff/);
  }
  const issue = "Declared application UI links could not be loaded. Installation and native Console status are unchanged.";
  const html = render({ experienceIssue: issue, experienceLoading: false });
  assert.match(html, /Retry UI links/);
  assert.match(html, /Installation and native Console status are unchanged/);
  assert.doesNotMatch(html, /Open application conversation|Paste your copied task or type a job/);
  const loading = elements(InstalledApplicationUseView({ ...baseView, experienceLoading: true }));
  assert.equal(loading.find((node) => node.type === "a").props.className, "setup-action");
  assert.match(render({ experienceLoading: true }), /Checking this installed package&#x27;s declared application UI links/);
  assert.doesNotMatch(render({ experienceLoading: true, copyStatus: "copied", outcome: "blocked" }),
    /Open application conversation|Paste your copied task|from opening or using the application conversation/);
  const noUse = render({ use: null, experienceIssue: issue, loading: false });
  assert.match(noUse, /Retry UI links/);
  assert.doesNotMatch(noUse, /href=/);
});

test("source-bound absent UI links show conditional native-chat guidance without granting an application channel", () => {
  for (const value of [noUi, { ...noUi, metadata_sha256: "d".repeat(64) }]) {
    const use = installedApplicationUse(metadata, installed, guide, checked, value);
    assert.equal(use.interfaces.status, "none");
    const html = render({ use, copyStatus: "copied", outcome: "blocked" });
    assert.match(html, /Open native Console/);
    assert.match(html, /If this application uses native chat for everyday work/);
    assert.match(html, /For jobs handled through native chat/);
    assert.ok(html.indexOf("For jobs handled through native chat") < html.indexOf("How to send through native chat"),
      "Conditional sending instructions are visible before the optional details.");
    const handoff = elements(InstalledApplicationUseView({ ...baseView, use }))
      .find((node) => node.props?.className === "deployment-use-console-send");
    assert.equal(handoff.type, "details"); assert.equal(handoff.props.open, undefined);
    assert.match(renderToStaticMarkup(handoff), /How to send through native chat.*review it, then send it/s);
    assert.doesNotMatch(html, /<h5>Open application<\/h5>/);
    assert.match(html, /If it uses its own UI, use that UI&#x27;s sign-in, job and saved-result controls/);
    assert.match(html, /Task copied\. Use it through the application&#x27;s usual controls/);
    assert.match(html, /from opening or using the application, what you saw/);
    assert.doesNotMatch(html, /Open application conversation|Task copied\. Paste it|Console-only/);
    assert.doesNotMatch(html, /Retry UI links|authorized administration and inspection/);
  }
  for (const change of [{ status: "stopped" }, { health: "failed" }, { process_live: false }]) {
    const receipt = { ...installed, ...change };
    const use = installedApplicationUse({ ...metadata, status: receipt.status }, receipt, guide, checked, noUi);
    assert.equal(use.interfaces.status, "none"); assert.equal(use.consoleUrl, null);
    const html = render({ use, deploymentStatus: receipt.status });
    assert.match(html, /Check first-use details|Checking first-use details/);
    assert.match(html, /Your draft stays here/);
    assert.match(html, /Check the forecast for tonight/);
    assert.doesNotMatch(html, /deployment-use-console-handoff|href=|Open native Console|Paste your copied task/);
    if (receipt.status === "stopped") assert.match(html, /Start the installed application/);
  }
  for (const change of [{ deployment_id: "other-install" }, { build_id: "newer-candidate" },
    { package_sha256: nextSource }, { source_digest: nextSource }, { source_digest: undefined },
    { source_digest: "not-a-digest" }, { metadata_sha256: undefined }, { metadata_sha256: "not-a-digest" },
    { metadata_sha256: ["d".repeat(64)] },
    { scope: "unverified" }, { verified: true }, { error: "DO_NOT_RENDER_RAW_ERROR" }]) {
    const value = { ...noUi, ...change }, use = installedApplicationUse(metadata, installed, guide, checked, value);
    assert.equal(use.interfaces.status, "unresolved");
    const html = render({ use, copyStatus: "copied", outcome: "blocked" });
    assert.match(html, /Retry UI links/);
    assert.match(html, /Task copied\. Use it after the application entrypoint has been checked/);
    assert.match(html, /everyday entrypoint still needs to be checked/);
    assert.doesNotMatch(html, /Open application conversation|Paste your copied task|DO_NOT_RENDER_RAW_ERROR/);
    assert.ok(installedUseFeedback(metadata, installed, guide, checked, { ...report, outcome: "blocked" }));
  }
});

test("mounted UI discovery retry preserves task and report drafts across unresolved and resolved surfaces", async () => {
  await mountedInstalledReport(null, async ({ document, window, calls, admissions, retries, rerender }) => {
    const setText = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    const task = "  Keep my actual task \u{1f9ed}\nwith its context.  ", improvement = "The application entrypoint could not be checked.";
    await act(async () => {
      const field = document.querySelector(".deployment-use-field textarea");
      setText.call(field, task); field.dispatchEvent(new window.Event("input", { bubbles: true }));
      const reportField = document.querySelector(".deployment-use-feedback textarea");
      setText.call(reportField, improvement); reportField.dispatchEvent(new window.Event("input", { bubbles: true }));
      document.querySelector('input[value="blocked"]').click();
    });
    const saved = { task, outcome: "blocked", improvement };
    const verifyDrafts = () => {
      assert.equal(document.querySelector(".deployment-use-field textarea").value, task);
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, improvement);
      assert.equal(document.querySelector('input[value="blocked"]').checked, true);
      assert.deepEqual(createInstalledUseDraftStore(window.sessionStorage).read(installedUseScopeKey(installed, guide)), saved);
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
      assert.deepEqual(admissions, []);
    };
    verifyDrafts();
    assert.doesNotMatch(document.body.textContent, /Open application conversation|Paste your copied task/);
    assert.equal(document.querySelector('.deployment-use-feedback button[type="submit"]').disabled, false);
    await act(async () => [...document.querySelectorAll("button")].find((button) => button.textContent === "Retry UI links").click());
    assert.deepEqual(retries, ["retry"]);
    await rerender(noUi, { experienceLoading: true });
    verifyDrafts();
    assert.match(document.body.textContent, /Checking this installed package's declared application UI links/);
    assert.doesNotMatch(document.body.textContent, /Open application conversation|Paste your copied task/);
    await rerender(noUi);
    verifyDrafts();
    const handoff = document.querySelector(".deployment-use-entry");
    assert.equal(handoff.querySelector("a").href, installed.console_url);
    assert.equal(handoff.querySelector(".deployment-use-contact strong").textContent, metadata.entrypoint.name);
    assert.equal(handoff.querySelector(".deployment-use-contact code").textContent, metadata.entrypoint.identity);
    assert.equal(handoff.querySelectorAll(".deployment-use-console-send li").length, 3);
    assert.equal(document.querySelector(".deployment-use-admin a") === null, true);
    assert.match(document.body.textContent, /Open application conversation|Paste your copied task/);
    assert.doesNotMatch(document.body.textContent, /Retry UI links/);
    await rerender(declaredUi);
    verifyDrafts();
    assert.match(document.body.textContent, /Open Project workspace/);
    assert.equal(document.querySelector(".deployment-use-entry a").href, declaredUi.entries[0].url);
    assert.equal(document.querySelector(".deployment-use-console-handoff") === null, true);
    assert.equal(document.querySelector(".deployment-use-admin a").href, installed.console_url);
    assert.doesNotMatch(document.body.textContent, /Open application conversation|Paste your copied task/);
  });
});

test("mounted oversized first-job edits show the limit and retain the exact previous draft and copy value", async () => {
  await mountedInstalledReport(noUi, async ({ document, window, calls, admissions }) => {
    const field = document.querySelector(".deployment-use-field textarea");
    // Browser maxlength truncation must not replace a whole rejected paste
    // before the production code-point validator can see it.
    assert.equal(field.hasAttribute("maxlength"), false);
    const reportField = document.querySelector(".deployment-use-feedback textarea");
    const setText = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    const edit = async (element, value) => act(async () => {
      setText.call(element, value); element.dispatchEvent(new window.Event("input", { bubbles: true }));
    });
    const previous = "  Existing draft \u{1f9ed}\nwith exact whitespace.  ", improvement = "Keep this independent report.";
    await edit(field, previous);
    await edit(reportField, improvement);
    await act(async () => document.querySelector('input[value="blocked"]').click());
    await edit(field, "x".repeat(3001));
    assert.equal(field.value, previous);
    assert.equal(field.getAttribute("aria-invalid"), "true");
    assert.match(document.querySelector('[role="alert"]').textContent, /3,000-character limit.*previous draft remains here/);
    assert.deepEqual(createInstalledUseDraftStore(window.sessionStorage).read(installedUseScopeKey(installed, guide)),
      { task: previous, outcome: "blocked", improvement });
    const copied = [];
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText: async (value) => copied.push(value) } });
    await act(async () => [...document.querySelectorAll("button")].find((button) => button.textContent === "Copy task").click());
    assert.deepEqual(copied, [previous]);
    await edit(field, "\u{1f9ed}".repeat(3000));
    assert.equal(field.value, "\u{1f9ed}".repeat(3000));
    assert.equal(field.value.length, 6000);
    assert.equal(field.getAttribute("aria-invalid"), "false");
    assert.equal(document.querySelector('[role="alert"]') === null, true);
    await edit(field, "\u{1f9ed}".repeat(3001));
    assert.equal(field.value, "\u{1f9ed}".repeat(3000));
    assert.equal(document.querySelector('[role="alert"]').textContent, INSTALLED_USE_TASK_LIMIT_MESSAGE);
    await act(async () => document.querySelector(".deployment-use-ideas button").click());
    assert.equal(field.value, "\u{1f9ed}".repeat(3000), "Previewing a suggestion preserves the previous bounded task.");
    assert.equal(document.querySelector('[role="alert"]').textContent, INSTALLED_USE_TASK_LIMIT_MESSAGE);
    await act(async () => [...document.querySelectorAll("button")].find((button) => button.textContent === "Use this idea").click());
    assert.equal(field.value, catalogue.tasks[0].request);
    assert.equal(document.querySelector('[role="alert"]') === null, true);
    assert.equal(reportField.value, improvement);
    assert.equal(document.querySelector('input[value="blocked"]').checked, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.deepEqual(admissions, []);
  }, { ...metadata, first_use: catalogue });
});


const optedInSource = { ...metadata, first_use: { ...catalogue, entry_id: "workspace" } };
const taskFromLink = (link) => JSON.parse(decodeURIComponent(new URL(link.href).hash.slice("#mobkit-task=".length))).task;

test("local handoff requires the exact authored entry and checked installed source with current native health", () => {
  const projected = installedApplicationUse(optedInSource, installed, guide, checked, declaredUi);
  assert.equal(installedUseTaskHandoff(projected, "Human edited task").entryId, "workspace");
  assert.equal(new URL(installedUseTaskHandoff(projected, "Human edited task").url).search, "");
  for (const entry_id of [null, "missing", "Workspace", " workspace", "workspace ", "x".repeat(65)]) {
    const use = installedApplicationUse({ ...optedInSource, first_use: { ...catalogue, entry_id } }, installed, guide, checked, declaredUi);
    assert.equal(installedUseTaskHandoff(use, "Human edited task"), null);
    assert.deepEqual(use.startingIdeas.tasks, catalogue.tasks);
  }
  for (const change of [{ process_live: false }, { health: "failed" }, { bootstrap: { complete: false, ready: false } }]) {
    const use = installedApplicationUse(optedInSource, { ...installed, ...change }, guide, checked, declaredUi);
    assert.equal(installedUseTaskHandoff(use, "Human edited task"), null);
  }
  for (const change of [{ source_digest: nextSource }, { package_sha256: nextSource }, { deployment_id: "other" }])
    assert.equal(installedUseTaskHandoff(installedApplicationUse({ ...optedInSource, ...change }, installed, guide, checked, declaredUi), "Task"), null);
  assert.equal(installedUseTaskHandoff(installedApplicationUse(optedInSource, installed, guide, checked, noUi), "Task"), null);
  assert.equal(installedUseTaskHandoff(installedApplicationUse(optedInSource, installed, guide, checked, null), "Task"), null);
});

test("mounted local task handoff uses the current edit, retains manual copy and sends no task or authorization", async () => {
  await mountedInstalledReport(declaredUi, async ({ document, window, calls, admissions, rerender }) => {
    const handoff = () => [...document.querySelectorAll("a")].find((link) => link.textContent === "Continue with this task");
    assert.equal(handoff() === undefined, true);
    await act(async () => document.querySelector(".deployment-use-ideas button").click());
    assert.equal(taskFromLink(handoff()), catalogue.tasks[0].request);
    const copy = () => [...document.querySelectorAll("button")].find((button) => button.textContent === "Copy task");
    const assertEditableHandoffPrimary = () => {
      assert.equal(handoff().className, "setup-action primary");
      assert.equal(copy().className, "setup-action");
      assert.equal(handoff().parentElement.firstElementChild === handoff(), true, "The declared editable handoff leads its action row");
      assert.equal(document.querySelector(".deployment-use-entry a").href, handoff().href,
        "The ready editable draft leads the application entry actions");
      assert.ok([...document.querySelectorAll(".deployment-use-entry a")]
        .some(link => link.href === declaredUi.entries[0].url), "The ordinary application entrypoint remains available");
      assert.ok([...document.querySelectorAll(".deployment-use-admin a")]
        .every(link => !link.classList.contains("primary")), "Administration remains secondary");
    };
    assertEditableHandoffPrimary();
    const task = "  My edited \u{1f9ed} job\nwith specific limits.  ";
    const field = document.querySelector(".deployment-use-field textarea");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    await act(async () => { setter.call(field, task); field.dispatchEvent(new window.Event("input", { bubbles: true })); });
    assert.equal(taskFromLink(handoff()), task);
    assert.equal(handoff().getAttribute("referrerpolicy"), "no-referrer");
    assert.equal(handoff().rel, "noopener noreferrer");
    assert.equal(handoff().target, "_blank");
    assert.match(document.body.textContent, /reachability, authentication and application workflows have not been verified/);
    assert.match(document.body.textContent, /send and confirmation controls/);
    const copied = [];
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText: async (value) => copied.push(value) } });
    await act(async () => [...document.querySelectorAll("button")].find((button) => button.textContent === "Copy task").click());
    assert.deepEqual(copied, [task]);
    assertEditableHandoffPrimary();
    await rerender(declaredUi, { blocked: true });
    assert.equal(handoff() === undefined, true);
    assert.equal(copy().className, "setup-action", "The ordinary application entry remains primary when the optional draft handoff is gated");
    assert.equal(field.value, task);
    await rerender(declaredUi);
    assert.equal(taskFromLink(handoff()), task);
    await rerender(null, { experienceLoading: true });
    assert.equal(handoff() === undefined, true);
    assert.equal(field.value, task);
    await rerender(declaredUi, { receipt: { ...installed, process_live: false } });
    assert.equal(handoff() === undefined, true);
    assert.equal(field.value, task);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0);
    assert.deepEqual(admissions, []);
  }, optedInSource);
});

test("mounted local pending clipboard copy offers selection of the current task without stale copied feedback", async () => {
  const pending = deferred(), copied = [];
  let completed = false;
  await mountedInstalledReport(noUi, async ({ document, window, calls, admissions }) => {
    const field = document.querySelector(".deployment-use-field textarea");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    const edit = (value) => act(async () => { setter.call(field, value); field.dispatchEvent(new window.Event("input", { bubbles: true })); });
    const original = "  Original job.\nKeep these limits.  ", current = "  Edited \u{1f9ed} job.\nKeep every character.  ";
    await edit(original);
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText: async (value) => {
      copied.push(value); await pending.promise; completed = true;
    } } });
    await act(async () => namedButton(document, "Copy task").click());
    assert.equal(namedButton(document, "Copying task...").disabled, true);
    assert.equal(namedButton(document, "Select task to copy").disabled, false);
    assert.match(document.querySelector(".deployment-use-copy [role='status']").textContent, /Clipboard copy is still pending/);
    assert.doesNotMatch(document.body.textContent, /Task copied|could not be copied/);
    assert.equal(field.disabled, false);
    await edit(current);
    await act(async () => namedButton(document, "Select task to copy").click());
    assert.equal(document.activeElement === field, true);
    assert.equal(field.value, current);
    assert.equal(field.selectionStart, 0); assert.equal(field.selectionEnd, current.length);
    assert.equal(completed, false, "Manual selection leaves the original clipboard write pending");
    assert.deepEqual(copied, [original]);
    assert.equal(namedButton(document, "Copying task...").disabled, true);
    assert.equal(calls.filter((call) => call.method === "POST").length, 0); assert.deepEqual(admissions, []);
    await act(async () => { pending.resolve(); await pending.promise; });
    assert.equal(completed, true);
    assert.equal(field.value, current);
    assert.equal(namedButton(document, "Copy task").disabled, false);
    assert.equal(namedButton(document, "Copying task...") === undefined, true);
    assert.equal(namedButton(document, "Select task to copy") === undefined, true);
    assert.doesNotMatch(document.body.textContent, /Task copied|Clipboard copy is still pending|could not be copied/);
    assert.deepEqual(copied, [original]);
  });
});


test("accepted first-job success survives draft qualifications and remains bound to its checked installation source", async () => {
  let selectedMetadata = metadata;
  await mountedInstalledReport(noUi, async ({ document, window, calls, admissions, rerender }) => {
    const field = document.querySelector(".deployment-use-field textarea");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
    const edit = (value, target = field) => act(async () => {
      setter.call(target, value); target.dispatchEvent(new window.Event("input", { bubbles: true }));
    });
    const success = () => document.querySelector(".deployment-use-success");
    const taskA = "  Task A: check a real observing window.\nKeep the supplied site.  ";
    const taskB = "Task B: prepare tomorrow's equipment list.";
    const qualification = "Keep this unsent qualification.\nThe forecast answer was incomplete.";
    await edit(taskA);
    await act(async () => namedButton(document, "It worked").click());
    const originalPost = calls.find(call => call.method === "POST");
    assert.equal(originalPost.target, installedUseUrl(guide.id, "success"));
    assert.equal(originalPost.body.task, taskA.trim());
    assert.match(success().textContent, /Your success report is saved/);
    const count = calls.length;
    const partly = document.querySelector('.deployment-use-feedback input[value="partly"]');
    await act(async () => partly.click());
    await edit(qualification, document.querySelector(".deployment-use-feedback textarea"));
    const assertHistorical = () => {
      assert.match(success().textContent, /An earlier success report is saved for this job/);
      assert.doesNotMatch(success().textContent, /Your success report is saved/);
      assert.equal(success().querySelector(".deployment-use-reported-job p").textContent, taskA.trim());
      assert.equal(document.querySelector(".deployment-use-feedback textarea").value, qualification);
      assert.equal(partly.checked, true);
      assert.equal(namedButton(document, "It worked") === undefined, true);
    };
    assertHistorical();
    assert.equal(field.value, taskA);
    await edit(taskB);
    assert.match(success().textContent, /earlier success report was confirmed/);
    assert.doesNotMatch(success().textContent, /An earlier success report is saved for this job|Your success report is saved/);
    assert.equal(field.value, taskB);
    await edit(taskA); assertHistorical();
    assert.equal(field.value, taskA);
    assert.equal(calls.length, count, "Draft edits retain the checked report without a metadata recheck or another report");
    assert.deepEqual(admissions, []);

    // The draft scope omits source_digest; the returned record must still be revalidated.
    const changedSource = { ...installed, source_digest: nextSource };
    assert.equal(installedUseScopeKey(changedSource, guide), installedUseScopeKey(installed, guide));
    selectedMetadata = { ...metadata, source_digest: nextSource };
    await rerender(noUi, { receipt: changedSource, build: { ...checked, source_digest: nextSource } });
    assert.doesNotMatch(success().textContent, /An earlier success report is saved for this job|Your success report is saved|earlier success report was confirmed/);
    assert.equal(field.value, taskA);
    selectedMetadata = metadata; await rerender(noUi); assertHistorical();
    const changedInstall = { ...installed, deployment_id: "another-install" };
    selectedMetadata = { ...metadata, deployment_id: changedInstall.deployment_id };
    await rerender(noUi, { receipt: changedInstall });
    await edit(taskA); await act(async () => document.querySelector('.deployment-use-feedback input[value="partly"]').click());
    assert.doesNotMatch(success().textContent, /An earlier success report is saved for this job|Your success report is saved|earlier success report was confirmed/);
    selectedMetadata = metadata; await rerender(noUi); assertHistorical();
    assert.equal(field.value, taskA);
    assert.equal(calls.filter(call => call.method === "POST").length, 1);
    assert.equal(calls.filter(call => call.method === "GET").length, 5);
    assert.deepEqual(admissions, []);
  }, metadata, (target, options, body) => {
    if (target === installedUseUrl(guide.id) && !options.method)
      return { ok: true, json: async () => structuredClone(selectedMetadata) };
    assert.equal(target, installedUseUrl(guide.id, "success")); assert.equal(options.method, "POST");
    return { ok: true, json: async () => ({ ...body, scope: "human-reported-installed-success", guide_id: guide.id,
      outcome: "worked", recorded_at: "2026-10-11T12:00:00Z", application_outcomes_verified: false }) };
  });
});


function firstJobPrimaryActions(props) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM(render(props));
  try {
    return [...dom.window.document.querySelectorAll(".deployment-use-entry .setup-action.primary, .deployment-use-handoff .setup-action.primary")]
      .filter(node => !node.disabled && node.getAttribute("aria-disabled") !== "true" && (node.tagName !== "A" || node.hasAttribute("href")))
      .map(node => node.textContent.trim());
  } finally { dom.window.close(); }
}

test("first-job Console entry leads while copying remains a secondary option", () => {
  assert.deepEqual(firstJobPrimaryActions({}), ["Open native Console"]);
});

test("first-job application entry leads until an authored editable task handoff is available", () => {
  const ordinary = installedApplicationUse(metadata, installed, guide, checked, declaredUi);
  assert.deepEqual(firstJobPrimaryActions({ use: ordinary }), ["Open Project workspace"]);
  const editable = installedApplicationUse(optedInSource, installed, guide, checked, declaredUi);
  assert.deepEqual(firstJobPrimaryActions({ use: editable }), ["Continue with this task"]);
  const html = render({ use: editable });
  assert.ok(html.indexOf("Continue with this task") < html.indexOf("Prepare a request"),
    "The ready draft has a clear opening action before optional request preparation");
});

test("uncertain runtime status retains manual copy without a clickable primary app link", () => {
  const use = installedApplicationUse(metadata, installed, guide, checked, declaredUi);
  assert.deepEqual(firstJobPrimaryActions({ use, commandUnconfirmed: true }), ["Copy task"]);
  assert.deepEqual(firstJobPrimaryActions({ use, selectionBlocked: true }), []);
});
