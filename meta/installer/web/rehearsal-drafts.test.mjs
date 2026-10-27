import { test } from "node:test";
import assert from "node:assert/strict";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createRehearsalDraftStore, rehearsalSetupDraftOwner, rehearsalTurnDraftOwner,
  rehearsalDraftAdmissionMatches, rehearsalInitialAcknowledged, rehearsalSessionStorage, rehearsalInitialRejected } from "./rehearsal-drafts.mjs";

const guide = { id: "reading", version: 7 };
const build = { guide_id: guide.id, guide_version: 7, current: true, status: "completed",
  source_verified: true, build_id: "build-a", source_digest: "a".repeat(64) };
const receipt = { guide_id: guide.id, guide_version: 7, current: true, mode: "application",
  rehearsal_id: "trial-a", status: "waiting_input", keep_open: true, can_continue: true,
  worker_stop_confirmed: false, turn_count: 1, current_turn: 1,
  application_source: { build_id: build.build_id, source_digest: build.source_digest } };
const setup = () => rehearsalSetupDraftOwner(guide, build, "application");
const turn = () => rehearsalTurnDraftOwner(receipt, build, guide);
const draftKey = (owner) => "mobkit-trial-draft-v1:" + JSON.stringify(owner);
function storage() {
  const values = new Map(), changes = [];
  return { values, changes, getItem: (key) => values.get(key) ?? null,
    setItem(key, value) { changes.push(["set", key]); values.set(key, value); },
    removeItem(key) { changes.push(["remove", key]); values.delete(key); } };
}
const fresh = (saved) => createRehearsalDraftStore(saved);
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const bundled = await bundle({ entryPoints: [fileURLToPath(new URL("rehearsal-panel.mjs", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent" });
const { admitRehearsalFollowUp, createRehearsalReceiptEpoch, rehearsalRunRequest } = await import(
  `data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

const actionId = "11443f55-c84a-420c-a5f2-f3f3d478fe9c";
function initialRequest(drafts, owner = setup(), values = {}) {
  const draft = { sample: "  Compare two choices.\n日本語 📚  ", context: "\nExact sample context\n",
    applicationData: '{"numbers":[0.000001],"words":["Å"]}', flow: "compare", ...values };
  for (const [field, value] of Object.entries(draft)) drafts.update(owner, field, value);
  drafts.selectMode(guide, build, owner.mode);
  const body = { ...rehearsalRunRequest(guide, build, { ...draft, mode: owner.mode }), start_request_id: actionId };
  return { action: drafts.beginInitial(owner, body), body, draft };
}
function initialReceipt(action, extra = {}) {
  return { ...receipt, mode: action.owner.mode, rehearsal_id: action.body.start_request_id,
    start_request_id: action.body.start_request_id, start_request_complete: true,
    start_request_fingerprint: "c".repeat(64), text: action.body.text, status: "completed",
    keep_open: action.body.keep_open, flow_id: action.body.flow_id ?? null,
    current: true, worker_stop_confirmed: true, application_stop_confirmed: true, ...extra };
}
function rejectedReceipt(action, extra = {}) {
  return { guide_id: action.owner.guide_id, guide_version: action.owner.guide_version, mode: action.owner.mode,
    start_request_id: action.body.start_request_id, start_request_fingerprint: "d".repeat(64),
    witness_version: 1, status: "rejected", start_request_complete: false, ...extra };
}

test("rejected-action history accepts only exact typed non-admission and retains original request plus revised draft", () => {
  const saved = storage(), drafts = fresh(saved), { action, body } = initialRequest(drafts);
  const witness = rejectedReceipt(action);
  for (const changed of [{ status: "unavailable" }, { witness_version: 2 }, { guide_id: "another" },
    { guide_version: 8 }, { mode: "design" }, { start_request_id: "wrong" }, { start_request_complete: true },
    { start_request_fingerprint: "missing" }, { worker_stop_confirmed: true }])
    assert.equal(drafts.recordRejectedStart(guide.id, { ...witness, ...changed }), false);
  assert.equal(drafts.archivePreparationFailure(guide.id, actionId), false);
  assert.deepEqual(rehearsalInitialRejected(action, witness), witness);
  assert.equal(drafts.recordRejectedStart(guide.id, witness), true);
  drafts.update(setup(), "sample", "Corrected exact unsent text\n");
  assert.equal(drafts.archivePreparationFailure(guide.id, actionId), true);
  assert.equal(drafts.initial(guide.id), null);
  assert.equal(fresh(saved).read(setup()).sample, "Corrected exact unsent text\n");
  assert.deepEqual(fresh(saved).initialHistory(guide.id), [witness]);
  const archived = JSON.parse(saved.getItem(`mobkit-trial-draft-v1:initial-history:${guide.id}:${actionId}`));
  assert.deepEqual(archived.body, body);
  assert.deepEqual(archived.start_rejection, witness);
});

test("a lost initial POST retains a separately frozen full action across reload and source changes", () => {
  const saved = storage(), drafts = fresh(saved), { action, body, draft } = initialRequest(drafts);
  assert.ok(action);
  assert.deepEqual(action.body, body);
  assert.deepEqual(action.submitted.draft, draft);
  // Returned copies and subsequent edits cannot mutate the replay request.
  action.body.fixtures.files["sample-context.txt"] = "Changed copy";
  drafts.update(setup(), "sample", "A newer unsent question");
  drafts.update(setup(), "applicationData", '{"different":true}');
  const restored = fresh(saved);
  assert.deepEqual(restored.initial(guide.id).body, body);
  assert.equal(restored.read(setup()).sample, "A newer unsent question");
  assert.equal(restored.beginInitial(setup(), { ...body, start_request_id: "22443f55-c84a-420c-a5f2-f3f3d478fe9c" }), null);
  assert.equal(restored.initial("another-guide"), null);
  assert.deepEqual(restored.initial(guide.id).owner, setup());
  assert.equal(restored.read(rehearsalSetupDraftOwner({ ...guide, version: 8 },
    { ...build, current_guide_version: 8, build_id: "build-b", source_digest: "b".repeat(64) }, "application")).sample, "");
});

test("only a committed exact keyed POST response resolves the initial action", () => {
  const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
  const acknowledged = initialReceipt(action);
  for (const change of [{ start_request_id: "another" }, { rehearsal_id: "another" },
    { start_request_complete: false }, { start_request_complete: null },
    { start_request_fingerprint: "missing" }, { guide_id: "another-guide" }, { guide_version: 8 },
    { mode: "design" }, { text: "Different job" },
    { application_source: { ...receipt.application_source, source_digest: "b".repeat(64) } }]) {
    assert.equal(rehearsalInitialAcknowledged(action, { ...acknowledged, ...change }), false);
    assert.equal(drafts.acknowledgeInitial(guide.id, { ...acknowledged, ...change }), false);
    assert.equal(drafts.read(setup()).sample, draft.sample);
    assert.deepEqual(drafts.initial(guide.id).body, action.body);
  }
  assert.equal(drafts.acknowledgeInitial(guide.id, acknowledged), true);
  assert.equal(drafts.initial(guide.id), null);
  assert.deepEqual(fresh(saved).read(setup()), { ...draft, sample: "" });
  assert.equal(drafts.acknowledgeInitial(guide.id, acknowledged), false);
});

test("initial host acknowledgement preserves A -> B -> A and any changed input revision through reload", () => {
  for (const field of ["sample", "context", "applicationData", "flow"]) {
    const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
    drafts.update(setup(), field, "An intervening edit");
    drafts.update(setup(), field, draft[field]);
    const restored = fresh(saved);
    assert.equal(restored.acknowledgeInitial(guide.id, initialReceipt(action)), true);
    assert.deepEqual(fresh(saved).read(setup()), draft);
    assert.equal(restored.initial(guide.id), null);
  }
});

test("new keyed starts require a verified durable write and keep denied or full drafts editable", () => {
  const failed = [null,
    { getItem() { throw new Error("denied read"); }, setItem() {}, removeItem() {} },
    { getItem() { return null; }, setItem() { throw new Error("quota"); }, removeItem() {} },
    { getItem() { return null; }, setItem() {}, removeItem() {} }];
  for (const saved of failed) {
    const drafts = fresh(saved), { action, draft } = initialRequest(drafts);
    assert.equal(action, null);
    assert.deepEqual(drafts.read(setup()), draft);
    assert.equal(drafts.update(setup(), "sample", "Still editable"), true);
    assert.equal(drafts.read(setup()).sample, "Still editable");
  }
  const saved = storage(), drafts = fresh(saved), { action } = initialRequest(drafts);
  saved.removeItem = () => { throw new Error("denied remove"); };
  assert.equal(drafts.acknowledgeInitial(guide.id, initialReceipt(action)), true);
  assert.equal(drafts.initial(guide.id), null);
  // A failed removal can reoffer the same harmless replay, not fresh work.
  assert.equal(fresh(saved).initial(guide.id).body.start_request_id, actionId);
  assert.equal(fresh(saved).read(setup()).sample, "");
});

test("malformed recovery records fail closed without filling another guide's draft", () => {
  const saved = storage(), drafts = fresh(saved), { action } = initialRequest(drafts);
  const key = `mobkit-trial-draft-v1:initial:${guide.id}`;
  for (const changed of ["not-json", JSON.stringify({ ...action, unknown: true }),
    JSON.stringify({ ...action, owner: { ...action.owner, guide_id: "another" } }),
    JSON.stringify({ ...action, body: { ...action.body, start_request_id: actionId.toUpperCase() } }),
    JSON.stringify({ ...action, submitted: { ...action.submitted, revisions: { sample: 0 } } }),
    "x".repeat(1_500_001)]) {
    saved.values.set(key, changed);
    const restored = fresh(saved);
    assert.equal(restored.initial(guide.id), null);
    assert.equal(restored.initialBlocked(guide.id), true);
    assert.equal(restored.beginInitial(setup(), action.body), null);
    assert.equal(restored.initial("another"), null);
  }
});

test("only a direct unavailable partial witness can be explicitly archived without changing any draft revision", () => {
  const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
  const partial = initialReceipt(action, { start_request_complete: false, status: "unavailable" });
  assert.equal(drafts.archivePreparationFailure(guide.id, actionId), false);
  assert.equal(drafts.archivePreparationFailure("another-guide", undefined), false);
  for (const change of [{ status: "failed" }, { start_request_complete: true }, { guide_id: "another" },
    { start_request_id: "different" }, { text: "Another request" }])
    assert.equal(drafts.recordPreparationFailure(guide.id, { ...partial, ...change }), false);
  assert.equal(drafts.recordPreparationFailure(guide.id, partial), true);
  drafts.update(setup(), "sample", "A later edit"); drafts.update(setup(), "sample", draft.sample);
  const capture = drafts.capture(setup(), "sample"), current = drafts.read(setup());
  assert.equal(drafts.archivePreparationFailure(guide.id, "22443f55-c84a-420c-a5f2-f3f3d478fe9c"), false);
  assert.equal(drafts.archivePreparationFailure("another-guide", actionId), false);
  assert.equal(drafts.archivePreparationFailure(guide.id, actionId), true);
  assert.deepEqual(drafts.read(setup()), current);
  assert.deepEqual(drafts.capture(setup(), "sample"), capture);
  assert.equal(drafts.initial(guide.id), null);
  const restored = fresh(saved), history = restored.initialHistory(guide.id);
  assert.equal(history.length, 1);
  assert.equal(history[0].start_request_id, actionId);
  assert.equal(history[0].start_request_complete, false);
  assert.equal(Object.hasOwn(history[0], "text"), false);
  const retained = JSON.parse(saved.values.get(`mobkit-trial-draft-v1:initial-history:${guide.id}:${actionId}`));
  assert.deepEqual(retained.body, action.body);
  assert.equal(retained.preparation_failure.start_request_complete, false);
  assert.equal(Object.hasOwn(retained.preparation_failure, "worker_stop_confirmed"), false);
  assert.equal(Object.hasOwn(retained.preparation_failure, "native_admission"), false);
  assert.equal(restored.beginInitial(setup(), { ...action.body, start_request_id: "22443f55-c84a-420c-a5f2-f3f3d478fe9c" })?.body.start_request_id,
    "22443f55-c84a-420c-a5f2-f3f3d478fe9c");
  assert.deepEqual(restored.read(setup()), current);
});

test("failed or silent history writes and removals keep the partial action and every draft", () => {
  for (const failure of ["write", "silent-write", "remove", "silent-remove"]) {
    const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
    assert.equal(drafts.recordPreparationFailure(guide.id, initialReceipt(action,
      { start_request_complete: false, status: "unavailable" })), true);
    if (failure.includes("write")) {
      const set = saved.setItem;
      saved.setItem = (key, value) => {
        if (key.includes("initial-history:")) { if (failure === "write") throw new Error("quota"); return; }
        set(key, value);
      };
    } else saved.removeItem = () => { if (failure === "remove") throw new Error("denied"); };
    assert.equal(drafts.archivePreparationFailure(guide.id, actionId), false);
    assert.deepEqual(drafts.initial(guide.id).body, action.body);
    assert.deepEqual(drafts.read(setup()), draft);
    assert.deepEqual(fresh(saved).initial(guide.id).body, action.body);
    assert.equal(drafts.beginInitial(setup(), { ...action.body, start_request_id: "22443f55-c84a-420c-a5f2-f3f3d478fe9c" }), null);
  }
});

test("a fresh draft store restores exact sample, context, fixtures and mode within the same source", () => {
  const saved = storage(), drafts = fresh(saved), owner = setup();
  const values = { sample: "  Jämför två böcker.\n日本語 📚  ", context: "\nA supplied example\n",
    applicationData: '{\n  "books": ["Invented book"]\n}', flow: "compare-books" };
  for (const [field, value] of Object.entries(values)) assert.equal(drafts.update(owner, field, value), true);
  drafts.selectMode(guide, build, "application");
  const restored = fresh(saved);
  assert.deepEqual(restored.read(setup()), values);
  assert.equal(restored.preferredMode(guide, build), "application");
  assert.deepEqual(restored.read(rehearsalSetupDraftOwner(guide, build, "design")),
    { sample: "", context: "", applicationData: "", flow: "" });
});

test("new guide versions and checked sources never inherit another source's application draft", () => {
  const saved = storage(), drafts = fresh(saved);
  drafts.update(setup(), "sample", "Keep only with source A");
  drafts.selectMode(guide, build, "application");
  for (const [nextGuide, nextBuild] of [
    [{ ...guide, id: "other" }, { ...build, guide_id: "other" }],
    [{ ...guide, version: 8 }, { ...build, current_guide_version: 8 }],
    [guide, { ...build, build_id: "build-b" }], [guide, { ...build, source_digest: "b".repeat(64) }],
  ]) {
    const restored = fresh(saved), owner = rehearsalSetupDraftOwner(nextGuide, nextBuild, "application");
    assert.ok(owner);
    assert.equal(restored.read(owner).sample, "");
    assert.equal(restored.preferredMode(nextGuide, nextBuild), null);
  }
  for (const change of [{ current: false }, { source_verified: false }, { status: "running" },
    { guide_version: 6 }, { source_digest: "unverified-text" }])
    assert.equal(rehearsalSetupDraftOwner(guide, { ...build, ...change }, "application"), null);
  assert.equal(fresh(saved).read(setup()).sample, "Keep only with source A");
});

test("follow-up and improvement drafts survive progress but remain tied to the same actual trial", () => {
  const saved = storage(), drafts = fresh(saved);
  drafts.update(turn(), "followUp", "  And what about the second choice?  ");
  drafts.update(turn(), "improvement", "Keep my comparison visible.");
  const restored = fresh(saved);
  for (const status of ["waiting_input", "running", "ending", "completed"]) {
    const owner = rehearsalTurnDraftOwner({ ...receipt, status, turn_count: 2, detail: "New progress" }, build, guide);
    assert.deepEqual(restored.read(owner), { followUp: "  And what about the second choice?  ", improvement: "Keep my comparison visible." });
  }
  assert.equal(restored.read(rehearsalTurnDraftOwner({ ...receipt, rehearsal_id: "trial-b" }, build, guide)).followUp, "");
  assert.equal(rehearsalTurnDraftOwner({ ...receipt, current: false }, build, guide), null);
  assert.equal(rehearsalTurnDraftOwner(receipt, { ...build, source_digest: "b".repeat(64) }, guide), null);
  assert.equal(rehearsalTurnDraftOwner({ ...receipt, mode: "design", application_source: undefined }, null, guide)?.mode, "design");
});

test("receipt refresh cannot reread storage over an already edited draft", () => {
  const saved = storage(), drafts = fresh(saved), owner = turn();
  drafts.update(owner, "followUp", "The text I am editing now");
  saved.values.set(draftKey(owner), JSON.stringify({ owner, draft: { followUp: "Older storage value", improvement: "" } }));
  const polledOwner = rehearsalTurnDraftOwner({ ...receipt, turn_count: 2, detail: "A receipt refresh" }, build, guide);
  assert.equal(drafts.read(polledOwner).followUp, "The text I am editing now");
});

test("clearing an acknowledged draft keeps context, fixtures and newer edits", () => {
  const saved = storage(), drafts = fresh(saved), owner = setup();
  drafts.update(owner, "sample", "Submitted sample");
  drafts.update(owner, "context", "Retain this context");
  drafts.update(owner, "applicationData", '{"books":[]}');
  assert.equal(drafts.clearSubmitted(owner, "sample", "Submitted sample"), true);
  assert.deepEqual(fresh(saved).read(owner), { sample: "", context: "Retain this context", applicationData: '{"books":[]}', flow: "" });
  drafts.update(owner, "sample", "Newer text while admission was pending");
  assert.equal(drafts.clearSubmitted(owner, "sample", "Submitted sample"), false);
  assert.equal(fresh(saved).read(owner).sample, "Newer text while admission was pending");
  drafts.update(turn(), "improvement", "A new edit during builder admission");
  assert.equal(drafts.clearSubmitted(turn(), "improvement", "The submitted feedback"), false);
  assert.equal(drafts.read(turn()).improvement, "A new edit during builder admission");
  assert.equal(drafts.clearSubmitted(turn(), "improvement", "A new edit during builder admission"), true);
});

test("an initial request with unknown transport outcome is retained instead of guessed from a new receipt", () => {
  const saved = storage(), drafts = fresh(saved), owner = setup();
  drafts.update(owner, "sample", "Initial job text");
  assert.equal(rehearsalDraftAdmissionMatches(owner, { ...receipt, text: "Initial job text" }), true);
  // Initial receipts do not expose the whole submitted context/fixture input.
  // A later text match cannot prove that this tab's POST was acknowledged.
  assert.equal(drafts.acknowledge(owner, { ...receipt, text: "Initial job text" }), false);
  assert.equal(fresh(saved).read(owner).sample, "Initial job text");
  for (const changed of [{ ...receipt, guide_id: "other" }, { ...receipt, guide_version: 6 },
    { ...receipt, application_source: { ...receipt.application_source, build_id: "build-b" } }])
    assert.equal(rehearsalDraftAdmissionMatches(owner, changed), false);
});

test("edits after submission are preserved even when the user changes back to identical text", () => {
  const saved = storage(), drafts = fresh(saved), owner = setup();
  drafts.update(owner, "sample", "Same text");
  const captured = drafts.capture(owner, "sample");
  drafts.update(owner, "sample", "An intervening edit");
  drafts.update(owner, "sample", "Same text");
  assert.equal(drafts.clearSubmitted(owner, "sample", captured), false);
  assert.equal(fresh(saved).read(owner).sample, "Same text");
  const turnOwner = turn();
  drafts.update(turnOwner, "followUp", "Same follow-up");
  drafts.markSubmitted(turnOwner, { kind: "continue", text: "Same follow-up", turn: 2 });
  drafts.update(turnOwner, "followUp", "Edited during admission");
  drafts.update(turnOwner, "followUp", "Same follow-up");
  const reloaded = fresh(saved);
  assert.equal(reloaded.acknowledge(turnOwner, { ...receipt, turns: [
    { turn: 2, command_id: "command-2", text: "Same follow-up", status: "admitting" }] }), true);
  assert.equal(fresh(saved).read(turnOwner).followUp, "Same follow-up");
});

test("reload reconciles only the exact saved follow-up turn and preserves text written afterward", () => {
  const saved = storage(), drafts = fresh(saved), owner = turn(), text = "  Follow up on book A.\n  ";
  drafts.update(owner, "followUp", text);
  assert.equal(drafts.markSubmitted(owner, { kind: "continue", text, turn: 2 }), true);
  const acknowledged = { ...receipt, turn_count: 2, turns: [{ turn: 2, command_id: "command-2", text: text.trim(), status: "admitting" }] };
  for (const changed of [{ ...acknowledged, rehearsal_id: "trial-b" },
    { ...acknowledged, application_source: { ...receipt.application_source, source_digest: "b".repeat(64) } },
    { ...acknowledged, turns: [{ ...acknowledged.turns[0], turn: 3 }] },
    { ...acknowledged, turns: [{ ...acknowledged.turns[0], text: "Another request" }] },
    { ...acknowledged, turns: [{ ...acknowledged.turns[0], command_id: "initial" }] },
    { ...acknowledged, turns: [] }]) {
    const reloaded = fresh(saved);
    assert.equal(reloaded.acknowledge(owner, changed), false);
    assert.equal(reloaded.read(owner).followUp, text);
  }
  const reloaded = fresh(saved);
  assert.equal(reloaded.acknowledge(owner, acknowledged), true);
  assert.equal(fresh(saved).read(owner).followUp, "");
  assert.equal(reloaded.acknowledge(owner, acknowledged), false);
  drafts.update(owner, "followUp", text);
  drafts.markSubmitted(owner, { kind: "continue", text, turn: 2 });
  drafts.update(owner, "followUp", "A newer question");
  const edited = fresh(saved);
  assert.equal(edited.acknowledge(owner, acknowledged), true);
  assert.equal(fresh(saved).read(owner).followUp, "A newer question");
});

test("the production follow-up admission helper keeps an edit made during a delayed POST across reload", async () => {
  const saved = storage(), drafts = fresh(saved), owner = turn(), response = deferred();
  const text = "Original follow-up", editedText = "New unsent follow-up";
  drafts.update(owner, "followUp", text);
  drafts.markSubmitted(owner, { kind: "continue", text, turn: 2 });
  let selectedReceipt = receipt, requests = 0;
  const pending = admitRehearsalFollowUp({ receipt, build, state: guide, text, epoch: createRehearsalReceiptEpoch(guide.id),
    load: (body) => { requests += 1; assert.equal(body.expected_turn, 1); return response.promise; },
    current: () => ({ state: guide, build, receipt: selectedReceipt, text: drafts.read(owner).followUp }),
    onReceipt: (value) => { selectedReceipt = value; },
    onClear: () => drafts.clearSubmitted(owner, "followUp", text),
    onError: () => assert.fail("Unexpected admission error") });
  drafts.update(owner, "followUp", editedText);
  response.resolve({ ...receipt, status: "running", turn_command: { id: "command-2", turn: 2, status: "sent" } });
  assert.equal(await pending, true);
  assert.equal(fresh(saved).read(owner).followUp, editedText);
  assert.equal(requests, 1);
  const reloaded = fresh(saved);
  assert.equal(reloaded.acknowledge(owner, { ...receipt, turns: [{ turn: 2, command_id: "command-2", text, status: "completed" }] }), true);
  assert.equal(fresh(saved).read(owner).followUp, editedText);
  assert.equal(requests, 1);
});

test("an ambiguous follow-up failure preserves a pending draft until the actual worker row appears", async () => {
  const saved = storage(), drafts = fresh(saved), owner = turn(), response = deferred(), text = "Keep this request";
  drafts.update(owner, "followUp", text);
  drafts.markSubmitted(owner, { kind: "continue", text, turn: 2 });
  let errors = 0, calls = 0;
  const pending = admitRehearsalFollowUp({ receipt, build, state: guide, text, epoch: createRehearsalReceiptEpoch(guide.id),
    load: () => { calls += 1; return response.promise; },
    current: () => ({ state: guide, build, receipt, text: drafts.read(owner).followUp }),
    onReceipt: () => assert.fail("No response was received"), onClear: () => assert.fail("An unknown outcome cannot clear text"),
    onError: () => { errors += 1; } });
  response.reject(new Error("Lost transport acknowledgment"));
  assert.equal(await pending, false);
  assert.equal(errors, 1);
  assert.equal(fresh(saved).read(owner).followUp, text);
  const restored = fresh(saved);
  assert.equal(restored.acknowledge(owner, { ...receipt, turns: [{ turn: 2, command_id: "command-2", text, status: "admitting" }] }), true);
  assert.equal(fresh(saved).read(owner).followUp, "");
  assert.equal(calls, 1);
});

test("storage failures preserve usable in-memory drafts and never throw during clearing", () => {
  const denied = { getItem() { throw new Error("denied"); }, setItem() { throw new Error("full"); }, removeItem() { throw new Error("denied"); } };
  const drafts = fresh(denied), owner = turn();
  assert.equal(drafts.update(owner, "followUp", "Usable without browser storage"), true);
  assert.equal(drafts.read(owner).followUp, "Usable without browser storage");
  assert.equal(drafts.clearSubmitted(owner, "followUp", "Usable without browser storage"), true);
  assert.equal(drafts.read(owner).followUp, "");
  assert.equal(drafts.preferredMode(guide, build), null);
  assert.doesNotThrow(() => drafts.selectMode(guide, build, "application"));
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "sessionStorage");
  try {
    Object.defineProperty(globalThis, "sessionStorage", { configurable: true, get() { throw new Error("denied"); } });
    assert.equal(rehearsalSessionStorage(), null);
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "sessionStorage", descriptor);
    else delete globalThis.sessionStorage;
  }
});

test("invalid, oversized or mismatched stored input cannot populate the production draft", () => {
  const owner = setup();
  for (const raw of ["not-json", "x".repeat(1_500_001), JSON.stringify({ owner, draft: { sample: 42 } }),
    JSON.stringify({ owner, draft: { sample: "x".repeat(6001) } }),
    JSON.stringify({ owner, draft: { sample: "text", unknown: "text" } }),
    JSON.stringify({ owner: { ...owner, source_digest: "b".repeat(64) }, draft: { sample: "wrong source" } }),
    JSON.stringify({ owner, draft: { sample: "text" }, unknown: "text" })]) {
    const saved = storage(); saved.values.set(draftKey(owner), raw);
    assert.equal(fresh(saved).read(owner).sample, "");
  }
  const drafts = fresh(storage());
  assert.equal(drafts.update(owner, "sample", "界".repeat(6000)), true);
  assert.equal(drafts.update(owner, "sample", "界".repeat(6001)), false);
  assert.equal(drafts.read(owner).sample.length, 6000);
  assert.equal(drafts.update(owner, "credential", "Never harvested"), false);
  assert.equal(drafts.markSubmitted(turn(), { kind: "continue", text: "Bounded", turn: 11 }), false);
});

const require = createRequire(import.meta.url);
const componentBundle = await bundle({ entryPoints: [fileURLToPath(new URL("rehearsal-panel.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", loader: { ".css": "empty" }, logLevel: "silent",
  nodePaths: [fileURLToPath(new URL("node_modules/", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { RehearsalPanel } = await import(`data:text/javascript;base64,${Buffer.from(componentBundle.outputFiles[0].text).toString("base64")}`);

async function mountedInitialTrial({ saved = storage(), capability = true, initialState = guide, initialBuild = build,
  initialLatest = null, onRepairReceipt, onPost }, verify) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const requests = [], ids = [actionId, "22443f55-c84a-420c-a5f2-f3f3d478fe9c"];
  let latest = initialLatest, issued = fresh(saved).initial(initialState.id) ? 1 : 0,
    selectedState = initialState, selectedBuild = initialBuild;
  const response = (value) => ({ ok: true, json: async () => value });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: saved, IS_REACT_ACT_ENVIRONMENT: true,
    crypto: { randomUUID: () => ids[issued++] },
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (target, options = {}) => {
      const call = { target, method: options.method || "GET", ...(options.body ? { body: JSON.parse(options.body) } : {}) };
      requests.push(call);
      if (target === "/api/capabilities") {
        const value = typeof capability === "function" ? await capability() : capability;
        if (value === "failed") throw new Error("Capability connection unavailable");
        return response({ rehearsal: { start_request_replay: value } });
      }
      if (options.method === "POST") {
        const value = await onPost(call.body, call);
        return value?.httpStatus ? { ok: false, status: value.httpStatus, json: async () => value.body } : response(value);
      }
      if (target.includes("/timeline?")) return response({ rehearsal_id: latest?.rehearsal_id, frames: [], exhausted: true });
      return latest ? response(latest) : { ok: false, status: 404, json: async () => ({ detail: "No saved trial." }) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  const mount = async (state = selectedState, candidate = selectedBuild) => {
    selectedState = state; selectedBuild = candidate;
    await act(async () => root.render(React.createElement(RehearsalPanel,
      { state, build: candidate, onRepairReceipt, plan: { flows: [{ id: "compare", name: "Compare choices" }] } })));
  };
  const button = (label) => [...dom.window.document.querySelectorAll("button")].find((item) => item.textContent === label);
  const text = () => dom.window.document.querySelector(`#trial-${selectedState.id}`);
  const editField = async (element, value) => {
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set.call(element, value);
      element.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
    });
  };
  const edit = (value) => editField(text(), value);
  try {
    await mount();
    // Observe the settled initial host read before interacting with the panel.
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
    await verify({ dom, requests, saved, mount, button, text, edit, editField,
      setLatest(value) { latest = value; },
      click: async (label) => { assert.ok(button(label), label); await act(async () => button(label).click()); },
      settle: async (callback) => { await act(async () => { callback(); await new Promise((resolve) => setTimeout(resolve, 10)); }); } });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

const contextualGuide = { ...guide, proposal: { project: { name: "Reading circle" }, agents: [{ id: "reader" }] },
  brief: { facts: { purpose: "Compare the supplied book choices", audience: "Local volunteers" } } };
const trialDraft = { sample: "  Compare these supplied choices for our volunteers.\nKeep the budget explicit. 📚  ",
  context: "Original evidence, not a live connection.\nÅ and 日本語 remain exact.\n",
  applicationData: '{"books":[{"title":"North","cost":45},{"title":"South","cost":60}],"budget":70}', flow: "compare" };
async function typeApplicationDraft(ui) {
  const application = [...ui.dom.window.document.querySelectorAll('input[type="radio"]')][1];
  if (!application.checked) await act(async () => application.click());
  await ui.edit(trialDraft.sample);
  await ui.editField(ui.dom.window.document.querySelector('[aria-label="Sample context for rehearsal"]'), trialDraft.context);
  await ui.editField(ui.dom.window.document.querySelector('[aria-label="Application sample data"]'), trialDraft.applicationData);
  await act(async () => {
    const select = ui.dom.window.document.querySelector("select"); select.value = trialDraft.flow;
    select.dispatchEvent(new ui.dom.window.Event("change", { bubbles: true }));
  });
}
function assertTrialDraft(ui, values = trialDraft) {
  assert.equal(ui.text().value, values.sample);
  assert.equal(ui.dom.window.document.querySelector('[aria-label="Sample context for rehearsal"]').value, values.context);
  assert.equal(ui.dom.window.document.querySelector('[aria-label="Application sample data"]').value, values.applicationData);
  assert.equal(ui.dom.window.document.querySelector("select").value, values.flow);
  assert.equal([...ui.dom.window.document.querySelectorAll('input[type="radio"]')][1].checked, true);
}

test("a clarification retains the mounted unsent application job only after the same canonical source is checked", async () => {
  const saved = storage(), nextGuide = { ...contextualGuide, version: 8 };
  const nextBuild = { ...build, current_guide_version: 8 };
  let posted;
  await mountedInitialTrial({ saved, initialState: contextualGuide, onPost(body) {
    posted = body;
    const action = fresh(saved).initial(guide.id);
    assert.equal(action.owner.guide_version, 8);
    assert.deepEqual(action.submitted.draft, trialDraft);
    return initialReceipt(action, { guide_version: 8 });
  } }, async (ui) => {
    await typeApplicationDraft(ui);
    assertTrialDraft(ui);
    const oldTry = ui.button("Try application request");
    const oldTryHandler = oldTry[Object.keys(oldTry).find((key) => key.startsWith("__reactProps$"))].onClick;
    const oldBytes = saved.getItem(draftKey(setup())), before = [...saved.values];
    await ui.mount(nextGuide, build); // Earlier currentness is not proof for version 8.
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    assert.equal(saved.getItem(draftKey(setup())), oldBytes);
    assert.equal(ui.button("Try agent design").disabled, true);
    await ui.mount(nextGuide, nextBuild);
    assertTrialDraft(ui);
    assert.deepEqual([...saved.values], before); // No storage migration or request reservation.
    assert.equal(fresh(saved).read(rehearsalSetupDraftOwner(nextGuide, nextBuild, "application")).sample, "");
    assert.equal(ui.button("Try application request").disabled, false);
    await act(async () => oldTryHandler());
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    await ui.click("Try application request");
    assert.equal(posted.version, 8); assert.equal(posted.build_id, build.build_id);
    assert.equal(posted.text, trialDraft.sample.trim()); assert.equal(posted.flow_id, "compare");
    assert.deepEqual(posted.params, { request: trialDraft.sample.trim() });
    assert.deepEqual(posted.application_fixtures, JSON.parse(trialDraft.applicationData));
    assert.equal(posted.fixtures.files["sample-context.txt"], trialDraft.context);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.text().value, "");
    assert.equal(saved.getItem(draftKey(setup())), oldBytes);
  });
});

test("different source, guide, material inputs or canonical-stale build never adopt an older unsent job", async () => {
  const nextGuide = { ...contextualGuide, version: 8 }, currentBuild = { ...build, current_guide_version: 8 };
  for (const [state, candidate] of [
    [nextGuide, { ...currentBuild, build_id: "build-b", source_digest: "b".repeat(64) }],
    [{ ...nextGuide, id: "different-guide" }, { ...currentBuild, guide_id: "different-guide" }],
    [{ ...nextGuide, brief: { facts: { purpose: "A different job" } } }, currentBuild],
    [{ ...nextGuide, proposal: { ...nextGuide.proposal, agents: [{ id: "another-reader" }] } }, currentBuild],
    [nextGuide, { ...currentBuild, current: false }],
  ]) {
    const saved = storage();
    await mountedInitialTrial({ saved, initialState: contextualGuide, onPost() { assert.fail("Changed setup cannot submit the old draft"); } }, async (ui) => {
      await typeApplicationDraft(ui); const before = [...saved.values];
      await ui.mount(state, candidate);
      assert.equal(ui.text().value, "");
      assert.equal(ui.dom.window.document.querySelector('[aria-label="Sample context for rehearsal"]').value, "");
      assert.equal(ui.dom.window.document.querySelector("select").value, "");
      assert.deepEqual([...saved.values], before);
      assert.deepEqual(fresh(saved).read(setup()), trialDraft);
      assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    });
  }
});

test("a pending original start is not carried or rewritten by a same-source clarification", async () => {
  const saved = storage(), lost = deferred(), nextGuide = { ...contextualGuide, version: 8 };
  const nextBuild = { ...build, current_guide_version: 8 }; let original, posts = 0;
  await mountedInitialTrial({ saved, initialState: contextualGuide, onPost(body) {
    if (++posts === 1) { original = structuredClone(body); return lost.promise; }
    assert.deepEqual(body, original);
    return initialReceipt(fresh(saved).initial(guide.id), { current_guide_version: 8 });
  } }, async (ui) => {
    await typeApplicationDraft(ui);
    await ui.click("Try application request");
    const actionBytes = saved.getItem(`mobkit-trial-draft-v1:initial:${guide.id}`);
    await ui.mount(nextGuide, nextBuild);
    await ui.settle(() => lost.reject(new Error("Lost original admission response")));
    assert.equal(ui.text().value, "");
    assert.equal(saved.getItem(`mobkit-trial-draft-v1:initial:${guide.id}`), actionBytes);
    assert.equal(fresh(saved).initial(guide.id).owner.guide_version, 7);
    assert.deepEqual(fresh(saved).initial(guide.id).submitted.draft, trialDraft);
    assert.equal(fresh(saved).read(rehearsalSetupDraftOwner(nextGuide, nextBuild, "application")).sample, "");
    assert.match(ui.dom.window.document.body.textContent, /original proposal 7/);
    assert.equal(posts, 1);
    await ui.click("Recover trial");
    assert.equal(posts, 2); assert.equal(original.version, 7);
    assert.equal(fresh(saved).initial(guide.id), null);
    assert.equal(ui.text().value, "");
  });
});

test("new design-mode edits while application currentness is pending are not replaced by the earlier application job", async () => {
  const saved = storage(), nextGuide = { ...contextualGuide, version: 8 }, nextBuild = { ...build, current_guide_version: 8 };
  await mountedInitialTrial({ saved, initialState: contextualGuide, onPost() { assert.fail("Typing cannot start a trial"); } }, async (ui) => {
    await typeApplicationDraft(ui);
    await ui.mount(nextGuide, build);
    await ui.edit("A separate design question while the application source is being checked.");
    await ui.mount(nextGuide, nextBuild);
    assert.equal([...ui.dom.window.document.querySelectorAll('input[type="radio"]')][0].checked, true);
    assert.equal(ui.text().value, "A separate design question while the application source is being checked.");
    assert.deepEqual(fresh(saved).read(setup()), trialDraft);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
  });
});

test("the mounted composer rejects excessive UTF8 context before saving a start and correction remains a normal Try", async () => {
  const saved = storage(), owner = rehearsalSetupDraftOwner(guide, null, "design"), drafts = fresh(saved);
  const sample = "  Use my exact sample\n  ", context = "界".repeat(80_000);
  drafts.update(owner, "sample", sample); drafts.update(owner, "context", context);
  await mountedInitialTrial({ saved, initialBuild: null, onPost(body) {
    assert.equal(body.fixtures.files["sample-context.txt"], "Corrected context\n");
    return initialReceipt(fresh(saved).initial(guide.id));
  } }, async (ui) => {
    await ui.click("Try agent design");
    assert.match(ui.dom.window.document.body.textContent, /200 KB UTF8/);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    assert.equal(fresh(saved).initial(guide.id), null);
    assert.deepEqual(fresh(saved).read(owner), { sample, context, applicationData: "", flow: "" });
    assert.equal(ui.text().value, sample);
    const contextField = ui.dom.window.document.querySelector('[aria-label="Sample context for rehearsal"]');
    assert.equal(contextField.value, context);
    await ui.editField(contextField, "Corrected context\n");
    await ui.click("Try agent design");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
    assert.equal(ui.text().value, "");
    assert.equal(contextField.value, "Corrected context\n");
  });
});

test("the mounted flow parameter bound rejects before beginInitial without changing text, context or selection", async () => {
  const saved = storage(), owner = rehearsalSetupDraftOwner(guide, null, "design"), drafts = fresh(saved);
  const sample = "界".repeat(5000);
  for (const [field, value] of Object.entries({ sample, context: "Keep context", flow: "compare" })) drafts.update(owner, field, value);
  await mountedInitialTrial({ saved, initialBuild: null, onPost() { assert.fail("Invalid flow must not POST"); } }, async (ui) => {
    await ui.click("Try agent design");
    assert.match(ui.dom.window.document.body.textContent, /30 KB parameter/);
    assert.equal(fresh(saved).initial(guide.id), null);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    assert.equal(ui.text().value, sample);
    assert.deepEqual(fresh(saved).read(owner), { sample, context: "Keep context", applicationData: "", flow: "compare" });
  });
});

test("a direct host rejection supports explicit correction with a new key, retaining original history and edits", async () => {
  const saved = storage(), { action, body, draft } = initialRequest(fresh(saved));
  let calls = 0, corrected;
  await mountedInitialTrial({ saved, onPost(request) {
    if (++calls === 1) return { httpStatus: 422, body: { detail: "The host rejected this isolated fixture.", start_rejection: rejectedReceipt(action) } };
    corrected = request;
    return initialReceipt({ owner: setup(), body: request });
  } }, async (ui) => {
    await ui.click("Recover trial");
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(ui.text().value, draft.sample);
    await ui.edit("  Corrected job\n");
    await ui.click("Keep rejected start and correct trial");
    assert.equal(calls, 1);
    assert.equal(ui.text().value, "  Corrected job\n");
    assert.equal(ui.button("Try application request").disabled, false);
    assert.equal(fresh(saved).initialHistory(guide.id)[0].status, "rejected");
    await ui.click("Try application request");
    assert.notEqual(corrected.start_request_id, actionId);
    assert.equal(corrected.text, "Corrected job");
    assert.equal(calls, 2);
    const archived = JSON.parse(saved.getItem(`mobkit-trial-draft-v1:initial-history:${guide.id}:${actionId}`));
    assert.deepEqual(archived.body, body);
    assert.deepEqual(archived.submitted.draft, draft);
  });
});

test("ordinary HTTP errors and mismatched negative responses preserve the pending exact request without an escape", async () => {
  for (const response of [
    { httpStatus: 422, body: { detail: "Validation failed without a host witness." } },
    { httpStatus: 504, body: { detail: "Timeout", start_rejection: rejectedReceipt({ owner: setup(), body: { start_request_id: actionId } }) } },
    { httpStatus: 422, body: { detail: "Wrong binding", start_rejection: rejectedReceipt({ owner: setup(), body: { start_request_id: actionId } }, { guide_version: 8 }) } },
  ]) {
    const saved = storage(), { body, draft } = initialRequest(fresh(saved));
    await mountedInitialTrial({ saved, onPost: () => response }, async (ui) => {
      await ui.click("Recover trial");
      assert.equal(ui.button("Keep rejected start and correct trial") === undefined, true);
      assert.equal(ui.button("Try application request").disabled, true);
      assert.equal(ui.text().value, draft.sample);
      assert.deepEqual(fresh(saved).initial(guide.id).body, body);
      assert.deepEqual(fresh(saved).initialHistory(guide.id), []);
    });
  }
});

test("a mounted waiting callback failure ends first and exposes repair only after exact host cleanup", async () => {
  const failed = { ...receipt, application_observations: [{ kind: "callback_failed", callback: "collect" }] };
  let posts = 0;
  await mountedInitialTrial({ initialLatest: failed, onRepairReceipt() {}, onPost(body, call) {
    posts += 1;
    assert.ok(call.target.endsWith("/end"));
    assert.deepEqual(body, { rehearsal_id: receipt.rehearsal_id });
    return { ...failed, status: "completed", worker_stop_confirmed: false, application_stop_confirmed: false };
  } }, async (ui) => {
    assert.match(ui.dom.window.document.body.textContent, /Once the host confirms cleanup/);
    assert.equal(ui.button("Fix this trial") === undefined, true);
    assert.equal(posts, 0);
    ui.setLatest({ ...failed, status: "completed", worker_stop_confirmed: false, application_stop_confirmed: false });
    await ui.click("End trial");
    assert.equal(posts, 1);
    assert.equal(ui.button("Fix this trial") === undefined, true);
    const stopped = { ...failed, status: "completed", worker_stop_confirmed: true, application_stop_confirmed: true };
    ui.setLatest(stopped);
    // A fresh panel reads the same actual host receipt rather than inventing cleanup locally.
  });
  await mountedInitialTrial({ initialLatest: { ...failed, status: "completed", worker_stop_confirmed: true,
    application_stop_confirmed: true }, onRepairReceipt() {}, onPost() { assert.fail("Rendering cannot repair"); } }, async (ui) => {
    assert.ok(ui.button("Fix this trial"), ui.dom.window.document.body.textContent);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
  });
});

test("a failed trial accepts a correction with its exact source and retains the draft after builder rejection", async () => {
  const saved = storage(), correction = "Show the missing cotton-seam checks separately from hazards.\nKeep the experienced human decision pending.";
  const failed = { ...receipt, status: "completed", worker_stop_confirmed: true, application_stop_confirmed: true,
    application_observations: [{ kind: "callback_failed", identity: "triage:main", name: "triage_publish" }] };
  const admissions = [];
  let attempts = 0;
  await mountedInitialTrial({ saved, initialLatest: failed, onRepairReceipt(value) { admissions.push(value); },
    onPost(body, call) {
      assert.ok(call.target.endsWith("/repair"));
      assert.deepEqual(body, { version: guide.version, rehearsal_id: failed.rehearsal_id,
        build_id: build.build_id, source_digest: build.source_digest, request: correction });
      if (++attempts === 1) return { httpStatus: 409, body: { detail: "The builder is already working. Your correction was not admitted." } };
      return { guide_id: guide.id, guide_version: guide.version, build_id: "corrected-build", status: "queued" };
    } }, async (ui) => {
    const feedback = () => ui.dom.window.document.querySelector('[aria-label="What should work better after this trial?"]');
    assert.equal(ui.dom.window.document.querySelectorAll('[aria-label="What should work better after this trial?"]').length, 1);
    await ui.editField(feedback(), correction);
    await ui.click("Fix this trial");
    assert.equal(attempts, 1);
    assert.equal(admissions.length, 0);
    assert.equal(feedback().value, correction);
    assert.equal(fresh(saved).read(rehearsalTurnDraftOwner(failed, build, guide)).improvement, correction);
    assert.match(ui.dom.window.document.body.textContent, /Your correction was not admitted/);
    await ui.click("Fix this trial");
    assert.equal(attempts, 2);
    assert.equal(admissions.length, 1);
    assert.equal(feedback().value, "");
    assert.equal(fresh(saved).read(rehearsalTurnDraftOwner(failed, build, guide)).improvement, "");
  });
});

test("failed trial investigation remains available without feedback while a successful trial needs a correction", async () => {
  const stopped = { ...receipt, status: "completed", worker_stop_confirmed: true, application_stop_confirmed: true };
  await mountedInitialTrial({ initialLatest: { ...stopped, application_observations: [{ kind: "callback_failed" }] },
    onRepairReceipt() {}, onPost(body, call) {
      assert.ok(call.target.endsWith("/repair"));
      assert.deepEqual(body, { version: guide.version, rehearsal_id: receipt.rehearsal_id,
        build_id: build.build_id, source_digest: build.source_digest });
      return { guide_id: guide.id, guide_version: guide.version, build_id: "investigation-build", status: "queued" };
    } }, async (ui) => {
    assert.equal(ui.dom.window.document.querySelector('[aria-label="What should work better after this trial?"]').value, "");
    assert.equal(ui.button("Fix this trial").disabled, false);
    await ui.click("Fix this trial");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
  });
  const correction = "Make the saved assessment's next steps easier to find.";
  await mountedInitialTrial({ initialLatest: stopped, onRepairReceipt() {}, onPost(body) {
    assert.deepEqual(body, { version: guide.version, rehearsal_id: receipt.rehearsal_id,
      build_id: build.build_id, source_digest: build.source_digest, request: correction });
    return { guide_id: guide.id, guide_version: guide.version, build_id: "improved-build", status: "queued" };
  } }, async (ui) => {
    assert.ok(ui.button("Improve from this trial"), ui.dom.window.document.body.textContent);
    assert.equal(ui.button("Improve from this trial").disabled, true);
    assert.equal(ui.button("Fix this trial") === undefined, true);
    await ui.editField(ui.dom.window.document.querySelector('[aria-label="What should work better after this trial?"]'), correction);
    assert.equal(ui.button("Improve from this trial").disabled, false);
    await ui.click("Improve from this trial");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
  });
});

test("failed trial correction stays unavailable until terminal current source and both cleanup receipts", async () => {
  const failed = { ...receipt, status: "completed", worker_stop_confirmed: true, application_stop_confirmed: true,
    application_observations: [{ kind: "callback_failed" }] };
  for (const changes of [{ status: "waiting_input" }, { worker_stop_confirmed: false },
    { application_stop_confirmed: false }, { current: false },
    { application_source: { build_id: build.build_id, source_digest: "b".repeat(64) } }]) {
    await mountedInitialTrial({ initialLatest: { ...failed, ...changes }, onRepairReceipt() {},
      onPost() { assert.fail("An ineligible trial cannot admit a correction"); } }, async (ui) => {
      assert.equal(ui.dom.window.document.querySelector('[aria-label="What should work better after this trial?"]') === null, true);
      assert.equal(ui.button("Fix this trial") === undefined, true);
      assert.equal(ui.button("Improve from this trial") === undefined, true);
      assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    });
  }
});

test("the production panel retains a lost start, refuses latest-GET clearing, and explicitly replays its exact body after reload", async () => {
  const saved = storage(), drafts = fresh(saved), owner = setup();
  const initial = "  Compare choices\n日本語 📚  ";
  drafts.update(owner, "sample", initial); drafts.update(owner, "context", "Original context\n");
  drafts.update(owner, "applicationData", '{"numbers":[0.000001],"words":["Å"]}');
  drafts.update(owner, "flow", "compare"); drafts.selectMode(guide, build, "application");
  const lost = deferred(); let frozenBody, acknowledged;
  await mountedInitialTrial({ saved, onPost(body) { frozenBody = structuredClone(body); return lost.promise; } }, async (ui) => {
    await ui.click("Try application request");
    assert.equal(frozenBody.start_request_id, actionId);
    assert.equal(frozenBody.text, initial.trim());
    assert.equal(ui.text().value, initial);
    for (const value of ["A newer edit", initial]) await ui.edit(value);
    const action = fresh(saved).initial(guide.id);
    acknowledged = initialReceipt(action);
    ui.setLatest(acknowledged);
    await ui.settle(() => lost.reject(new Error("Lost POST response")));
    assert.equal(ui.text().value, initial);
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(ui.button("Recover trial").disabled, false);
    assert.deepEqual(fresh(saved).initial(guide.id).body, frozenBody);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
  });
  let replayed;
  await mountedInitialTrial({ saved, onPost(body) { replayed = body; return acknowledged; } }, async (ui) => {
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    assert.equal(ui.text().value, initial);
    await ui.click("Recover trial");
    assert.deepEqual(replayed, frozenBody);
    assert.equal(ui.text().value, initial);
    assert.equal(fresh(saved).initial(guide.id), null);
    assert.equal(ui.button("Recover trial") === undefined, true);
    assert.equal(ui.button("Try application request").disabled, false);
  });
});

test("a recovery on a newer proposal uses the saved original source and a new deliberate Try gets a fresh action", async () => {
  const saved = storage(), drafts = fresh(saved), { action, body } = initialRequest(drafts);
  const nextGuide = { ...guide, version: 8 }, nextBuild = { ...build, current_guide_version: 8,
    build_id: "build-b", source_digest: "b".repeat(64) };
  const nextOwner = rehearsalSetupDraftOwner(nextGuide, nextBuild, "application");
  drafts.update(nextOwner, "sample", "A job for the newer checked source");
  drafts.selectMode(nextGuide, nextBuild, "application");
  let calls = 0;
  await mountedInitialTrial({ saved, initialState: nextGuide, initialBuild: nextBuild, onPost(request) {
    calls += 1;
    if (calls === 1) { assert.deepEqual(request, body); return initialReceipt(action, { current: false, current_guide_version: 8 }); }
    assert.equal(request.version, 8); assert.equal(request.build_id, nextBuild.build_id);
    assert.notEqual(request.start_request_id, actionId);
    return initialReceipt({ owner: nextOwner, body: request }, { guide_version: 8,
      application_source: { build_id: nextBuild.build_id, source_digest: nextBuild.source_digest } });
  } }, async (ui) => {
    assert.equal(calls, 0);
    assert.match(ui.dom.window.document.body.textContent, /original proposal 7/);
    await ui.click("Recover trial");
    assert.equal(ui.text().value, "A job for the newer checked source");
    await ui.click("Try application request");
    assert.equal(calls, 2);
    assert.equal(ui.text().value, "");
  });
});

test("a partial host reservation cannot clear or unlock a fresh Try, and another setup cannot recover it", async () => {
  const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
  await mountedInitialTrial({ saved, onPost: () => initialReceipt(action, { start_request_complete: false, status: "unavailable" }) }, async (ui) => {
    await ui.click("Recover trial");
    assert.equal(ui.text().value, draft.sample);
    assert.match(ui.dom.window.document.body.textContent, /preparation did not finish/);
    assert.match(ui.dom.window.document.body.textContent, /did not start another worker/);
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(fresh(saved).initial(guide.id).body.start_request_id, actionId);
    await ui.mount({ id: "other", version: 1 }, null);
    assert.equal(ui.button("Recover trial") === undefined, true);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
    assert.deepEqual(fresh(saved).initial(guide.id).body, action.body);
  });
});

test("the production partial-start escape archives only on explicit click, keeps drafts, and the next Try uses a new ID", async () => {
  const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
  let calls = 0, nextBody;
  await mountedInitialTrial({ saved, onPost(body) {
    calls += 1;
    if (calls === 1) return initialReceipt(action, { start_request_complete: false, status: "unavailable" });
    nextBody = body;
    assert.notEqual(body.start_request_id, actionId);
    return initialReceipt({ owner: setup(), body });
  } }, async (ui) => {
    assert.equal(ui.button("Keep failed start and prepare a new trial") === undefined, true);
    await ui.click("Recover trial");
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(calls, 1);
    for (const value of ["A newer edit", draft.sample]) await ui.edit(value);
    await ui.click("Keep failed start and prepare a new trial");
    assert.equal(calls, 1);
    assert.equal(ui.text().value, draft.sample);
    assert.equal(ui.button("Try application request").disabled, false);
    assert.equal(fresh(saved).initialHistory(guide.id)[0].start_request_id, actionId);
    assert.match(ui.dom.window.document.body.textContent, /Earlier trial preparations/);
    assert.match(ui.dom.window.document.body.textContent, /No trial admission or cleanup is confirmed/);
    await ui.click("Try application request");
    assert.equal(calls, 2);
    assert.equal(nextBody.start_request_id, "22443f55-c84a-420c-a5f2-f3f3d478fe9c");
    assert.equal(ui.text().value, "");
    assert.equal(fresh(saved).initialHistory(guide.id).length, 1);
    assert.deepEqual(fresh(saved).read(setup()), { ...draft, sample: "" });
  });
  await mountedInitialTrial({ saved, onPost() { assert.fail("History/reload cannot send"); } }, async (ui) => {
    assert.match(ui.dom.window.document.body.textContent, /Earlier trial preparations/);
    assert.match(ui.dom.window.document.body.textContent, new RegExp(actionId));
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
  });
});

test("partial escape stays scoped and is unavailable during another pending POST", async () => {
  const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
  drafts.recordPreparationFailure(guide.id, initialReceipt(action, { start_request_complete: false, status: "unavailable" }));
  const delayed = deferred(); let calls = 0;
  await mountedInitialTrial({ saved, onPost() { calls += 1; return delayed.promise; } }, async (ui) => {
    const oldButton = ui.button("Keep failed start and prepare a new trial");
    assert.equal(oldButton.disabled, false);
    await ui.click("Recover trial");
    assert.equal(ui.button("Keep failed start and prepare a new trial").disabled, true);
    await ui.click("Keep failed start and prepare a new trial");
    assert.ok(fresh(saved).initial(guide.id));
    assert.equal(calls, 1);
    await ui.mount({ id: "another-guide", version: 7 }, null);
    assert.equal(ui.button("Keep failed start and prepare a new trial") === undefined, true);
    await act(async () => oldButton.click());
    assert.ok(fresh(saved).initial(guide.id));
    await ui.settle(() => delayed.resolve(initialReceipt(action, { start_request_complete: false, status: "unavailable" })));
    await ui.mount(guide, build);
    assert.equal(ui.text().value, draft.sample);
    assert.equal(ui.button("Keep failed start and prepare a new trial").disabled, false);
    assert.equal(calls, 1);
  });
});

test("the actual escape refuses a denied history write and leaves the pending action and editable draft", async () => {
  const saved = storage(), drafts = fresh(saved), { action, draft } = initialRequest(drafts);
  drafts.recordPreparationFailure(guide.id, initialReceipt(action, { start_request_complete: false, status: "unavailable" }));
  const set = saved.setItem;
  saved.setItem = (key, value) => { if (key.includes("initial-history:")) throw new Error("quota"); set(key, value); };
  await mountedInitialTrial({ saved, onPost() { assert.fail("Escape must not POST"); } }, async (ui) => {
    await ui.click("Keep failed start and prepare a new trial");
    assert.match(ui.dom.window.document.body.textContent, /history could not be saved/);
    assert.equal(ui.text().value, draft.sample);
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    assert.equal(fresh(saved).initial(guide.id).body.start_request_id, actionId);
    await ui.edit("Editable after history storage failure");
    assert.equal(ui.text().value, "Editable after history storage failure");
  });
});

test("confirmed legacy capability preserves the older admission protocol without claiming keyed recovery", async () => {
  const capability = false;
  const saved = storage(), drafts = fresh(saved), owner = rehearsalSetupDraftOwner(guide, null, "design");
  drafts.update(owner, "sample", "A legacy host request");
  await mountedInitialTrial({ saved, capability, initialBuild: null, onPost(body) {
    assert.equal(Object.hasOwn(body, "start_request_id"), false);
    return { ...receipt, mode: "design", rehearsal_id: "legacy-trial", text: body.text, status: "completed" };
  } }, async (ui) => {
    assert.match(ui.dom.window.document.body.textContent, /older trial-start protocol/);
    await ui.click("Try agent design");
    assert.equal(ui.text().value, "");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
  });
  const pendingStorage = storage(); initialRequest(fresh(pendingStorage));
  await mountedInitialTrial({ saved: pendingStorage, capability, onPost() { assert.fail("No unsupported replay"); } }, async (ui) => {
    assert.equal(ui.button("Recover trial").disabled, true);
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
  });
});

test("an unavailable or malformed capability blocks admission and preserves trial drafts and pending recovery", async () => {
  for (const capability of ["failed", "true", null, undefined]) {
    const saved = storage(), owner = rehearsalSetupDraftOwner(guide, null, "design");
    const draft = { sample: "Keep my job", context: "Keep my context", applicationData: "", flow: "" };
    for (const [field, value] of Object.entries(draft)) fresh(saved).update(owner, field, value);
    await mountedInitialTrial({ saved, capability: () => capability, initialBuild: null,
      onPost() { assert.fail("Unverified capability must not admit any trial"); } }, async (ui) => {
      assert.match(ui.dom.window.document.body.textContent, /Could not check trial recovery/);
      assert.doesNotMatch(ui.dom.window.document.body.textContent, /older trial-start protocol/);
      assert.equal(ui.button("Try agent design").disabled, true);
      assert.equal(ui.button("Retry connection check").disabled, false);
      await ui.click("Try agent design");
      assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
      assert.deepEqual(fresh(saved).read(owner), draft);
      assert.equal(fresh(saved).initial(guide.id), null);
    });
    const pendingStorage = storage(), { action } = initialRequest(fresh(pendingStorage));
    const before = new Map(pendingStorage.values);
    await mountedInitialTrial({ saved: pendingStorage, capability: () => capability,
      onPost() { assert.fail("Unverified capability must not replay a pending trial"); } }, async (ui) => {
      assert.equal(ui.button("Recover trial").disabled, true);
      assert.equal(ui.button("Try application request").disabled, true);
      assert.doesNotMatch(ui.dom.window.document.body.textContent, /does not support this saved request/);
      await ui.click("Retry connection check");
      assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
      assert.deepEqual(fresh(pendingStorage).initial(guide.id), action);
      assert.deepEqual(pendingStorage.values, before);
    });
  }
});

test("retrying a failed capability check enables a durably keyed fresh trial without changing its draft", async () => {
  const saved = storage(), owner = rehearsalSetupDraftOwner(guide, null, "design");
  fresh(saved).update(owner, "sample", "  Keep this job\n");
  let checks = 0;
  await mountedInitialTrial({ saved, initialBuild: null, capability: () => ++checks === 1 ? "failed" : true,
    onPost(body) {
      const action = fresh(saved).initial(guide.id);
      assert.equal(body.start_request_id, actionId);
      assert.deepEqual(body, action.body);
      return initialReceipt(action);
    } }, async (ui) => {
    assert.equal(ui.button("Try agent design").disabled, true);
    await ui.click("Retry connection check");
    assert.equal(checks, 2);
    assert.equal(ui.text().value, "  Keep this job\n");
    assert.equal(ui.button("Try agent design").disabled, false);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    await ui.click("Try agent design");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
    assert.equal(fresh(saved).initial(guide.id), null);
    assert.equal(ui.text().value, "");
  });
});

test("reload after a failed capability check recovers only the saved keyed request after explicit retry", async () => {
  const saved = storage(), { action, body } = initialRequest(fresh(saved));
  await mountedInitialTrial({ saved, capability: "failed",
    onPost() { assert.fail("A failed capability check must not start a request"); } }, async (ui) => {
    assert.equal(ui.button("Recover trial").disabled, true);
    assert.deepEqual(fresh(saved).initial(guide.id), action);
  });
  let checks = 0;
  await mountedInitialTrial({ saved, capability: () => ++checks === 1 ? "failed" : true,
    onPost(request) {
      assert.deepEqual(request, body);
      assert.equal(request.start_request_id, actionId);
      return initialReceipt(action);
    } }, async (ui) => {
    await ui.edit("A newer job for later");
    await ui.click("Retry connection check");
    assert.equal(ui.button("Recover trial").disabled, false);
    assert.equal(ui.button("Try application request").disabled, true);
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    await ui.click("Recover trial");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
    assert.equal(fresh(saved).initial(guide.id), null);
    assert.equal(ui.text().value, "A newer job for later");
  });
});

test("a delayed connection retry admits only the current guide draft after navigation", async () => {
  const saved = storage(), nextGuide = { id: "another-guide", version: 8 };
  const owner = rehearsalSetupDraftOwner(guide, null, "design"), nextOwner = rehearsalSetupDraftOwner(nextGuide, null, "design");
  fresh(saved).update(owner, "sample", "Keep the earlier guide draft");
  fresh(saved).update(nextOwner, "sample", "Try the current guide job");
  const delayed = deferred(); let checks = 0;
  await mountedInitialTrial({ saved, initialBuild: null, capability: () => ++checks === 1 ? "failed" : delayed.promise,
    onPost(body, call) {
      assert.equal(call.target, `/api/guide/${nextGuide.id}/rehearsal`);
      assert.equal(body.version, nextGuide.version);
      assert.equal(body.text, "Try the current guide job");
      const action = fresh(saved).initial(nextGuide.id);
      assert.deepEqual(body, action.body);
      return initialReceipt(action, { guide_id: nextGuide.id, guide_version: nextGuide.version });
    } }, async (ui) => {
    await ui.click("Retry connection check");
    assert.equal(ui.button("Try agent design").disabled, true);
    await ui.mount(nextGuide, null);
    await ui.settle(() => delayed.resolve(true));
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
    assert.equal(ui.text().value, "Try the current guide job");
    await ui.click("Try agent design");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 1);
    assert.equal(fresh(saved).read(owner).sample, "Keep the earlier guide draft");
    assert.equal(fresh(saved).read(nextOwner).sample, "");
  });
});

test("a delayed original start cannot clear or publish after a guide switch and remains explicitly recoverable", async () => {
  for (const nextGuide of [{ ...guide, version: 8 }, { id: "another-guide", version: 7 }]) {
    const saved = storage(), drafts = fresh(saved), owner = rehearsalSetupDraftOwner(guide, null, "design");
    const original = "Original unsent request";
    drafts.update(owner, "sample", original);
    const delayed = deferred(); let action, calls = 0;
    await mountedInitialTrial({ saved, initialBuild: null, onPost(body) {
      calls += 1;
      action = fresh(saved).initial(guide.id);
      assert.deepEqual(body, action.body);
      return calls === 1 ? delayed.promise : initialReceipt(action);
    } }, async (ui) => {
      await ui.click("Try agent design");
      await ui.mount(nextGuide, null);
      await ui.settle(() => delayed.resolve(initialReceipt(action)));
      assert.equal(ui.text().value, "");
      assert.equal(ui.dom.window.document.querySelector('[role="alert"]') === null, true);
      assert.equal(fresh(saved).read(owner).sample, original);
      assert.equal(fresh(saved).initial(guide.id).body.start_request_id, actionId);
      await ui.mount(guide, null);
      assert.equal(ui.text().value, original);
      assert.equal(calls, 1);
      await ui.click("Recover trial");
      assert.equal(calls, 2);
      assert.equal(ui.text().value, "");
      assert.equal(fresh(saved).initial(guide.id), null);
    });
  }
});

test("the actual supported panel sends nothing when the recovery record cannot be durably written", async () => {
  const saved = storage(), owner = rehearsalSetupDraftOwner(guide, null, "design"), drafts = fresh(saved);
  drafts.update(owner, "sample", "Keep me editable");
  saved.setItem = (key) => { if (key.includes("initial:")) throw new Error("quota"); };
  await mountedInitialTrial({ saved, initialBuild: null, onPost() { assert.fail("No request without recovery storage"); } }, async (ui) => {
    await ui.click("Try agent design");
    assert.equal(ui.text().value, "Keep me editable");
    assert.match(ui.dom.window.document.body.textContent, /No trial request was sent/);
    await ui.edit("Still editable after the storage failure");
    assert.equal(ui.text().value, "Still editable after the storage failure");
    assert.equal(ui.requests.filter((call) => call.method === "POST").length, 0);
  });
});

test("a fresh mount of the actual Try panel restores same-source fields without submitting or changing them", () => {
  const saved = storage(), drafts = fresh(saved), owner = setup();
  const values = { sample: "My unsent realistic job", context: "A retained example document", applicationData: '{"books":[]}', flow: "" };
  for (const [field, value] of Object.entries(values)) drafts.update(owner, field, value);
  drafts.selectMode(guide, build, "application");
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "sessionStorage");
  Object.defineProperty(globalThis, "sessionStorage", { configurable: true, value: saved });
  try {
    const before = [...saved.values]; let actions = 0;
    const render = (candidate = build) => renderToStaticMarkup(React.createElement(RehearsalPanel,
      { state: guide, plan: { flows: [] }, build: candidate, onRepairReceipt: () => { actions += 1; } }));
    for (let mount = 0; mount < 2; mount += 1) {
      const html = render();
      assert.match(html, /My unsent realistic job/);
      assert.match(html, /A retained example document/);
      assert.match(html, /&quot;books&quot;:\[\]/);
      assert.match(html, /Try application request/);
    }
    const changed = render({ ...build, build_id: "build-b", source_digest: "b".repeat(64) });
    assert.doesNotMatch(changed, /My unsent realistic job|A retained example document/);
    assert.deepEqual([...saved.values], before);
    assert.equal(actions, 0);
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "sessionStorage", descriptor);
    else delete globalThis.sessionStorage;
  }
});
