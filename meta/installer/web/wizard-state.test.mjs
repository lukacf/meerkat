import test from "node:test";
import assert from "node:assert/strict";
import {
  createGuideStorage,
  GUIDE_CREATION_KEY,
  guideCreationRequest,
  inputKey,
  inputStartingHelp,
  inputSubmissionText,
  readInput,
  writeInput,
  guideInputForState,
  admitNewGuideInput,
  installSteps,
  workflowLayers,
  unresolvedRequirements,
  setupRequirementGroups,
  implementationHandoff,
  recoverCompletedInput,
  admitSubmittedInput,
  currentGuideActivity,
  captureGuideResumeInput,
  preservedGuideResume,
  recoverGuideResumeInput,
  createGuideLoadGate,
  recoverMissingGuideSelection,
  guideOperationReadMatches,
} from "./wizard-state.mjs";
test("a saved trial never appears as activity in another setup or proposal", () => {
  const activity = { status: "completed", last: { identity: "lead:main" } };
  const snapshot = { guideId: "astronomy", guideVersion: 16, activity };
  assert.equal(currentGuideActivity(snapshot, { id: "astronomy", version: 16 }), activity);
  assert.equal(currentGuideActivity(snapshot, { id: "reading", version: 16 }), null);
  assert.equal(currentGuideActivity(snapshot, { id: "astronomy", version: 17 }), null);
  assert.equal(currentGuideActivity(null, { id: "reading", version: 1 }), null);
});
const storage = () => {
  const values = new Map();
  return {
    getItem: (key) => values.get(key),
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };
};

test("setup creation ownership survives a tab reload without sharing another tab's request", () => {
  const firstTab = storage(), otherTab = storage();
  const request = guideCreationRequest(createGuideStorage(() => firstTab));
  assert.equal(guideCreationRequest(createGuideStorage(() => firstTab)), request);
  assert.notEqual(guideCreationRequest(createGuideStorage(() => otherTab)), request);
  firstTab.removeItem(GUIDE_CREATION_KEY);
  assert.notEqual(guideCreationRequest(createGuideStorage(() => firstTab)), request);
});

test("unavailable creation storage retains its open-page request and refuses a damaged saved owner", () => {
  const page = createGuideStorage(() => { throw new Error("Storage unavailable"); });
  const request = guideCreationRequest(page);
  assert.equal(guideCreationRequest(page), request);
  assert.equal(page.persistent(), false);
  const tab = storage();
  tab.setItem(GUIDE_CREATION_KEY, "damaged");
  assert.throws(() => guideCreationRequest(createGuideStorage(() => tab)), /Keep your idea/);
  assert.equal(tab.getItem(GUIDE_CREATION_KEY), "damaged");
});
test("combined intent and choices are owned by the exact question and do not leak to its successor", () => {
  const store = storage();
  const state = { id: "context", question: { id: "history", stage: "knowledge", title: "Which contexts?", multiple: false,
    options: [{ id: "personal", label: "Personal" }, { id: "shared", label: "Shared" }] } };
  const input = { key: inputKey(state), draft: "Keep this thought", selected: ["personal", "shared"], mode: "answer", combine_options: true };
  writeInput(store, input);
  assert.deepEqual(readInput(store, state), input);
  assert.deepEqual(guideInputForState(store, state, input), input);
  for (const next of [{ ...state, id: "another" }, { ...state, question: { ...state.question, title: "Changed meaning" } },
    { ...state, question: { ...state.question, multiple: true } }]) {
    const restored = readInput(store, next);
    assert.equal(restored.combine_options, undefined);
    if (next.question.multiple !== true) assert.deepEqual(restored.selected, []);
  }
  writeInput(store, { ...input, combine_options: "true" });
  assert.equal(readInput(store, state).combine_options, undefined);
  assert.deepEqual(readInput(store, state).selected, []);
});

test("combined admission clears only its exact sent intent, preserving a newer selection mode", () => {
  const input = { key: "owned", draft: "Sent", selected: ["personal", "shared"], mode: "answer", combine_options: true };
  const request = { action: "answer", text: "Sent", selected: ["personal", "shared"], combine_options: true };
  assert.deepEqual(admitSubmittedInput(input, request), { key: "owned", draft: "", selected: [], mode: "answer" });
  const newer = { ...input, draft: "Unsent", selected: ["shared"] };
  assert.deepEqual(admitSubmittedInput(newer, request), newer);
  const changedMode = { ...input, draft: "Unsent", combine_options: false };
  assert.deepEqual(admitSubmittedInput(changedMode, request), changedMode);
  const backed = admitSubmittedInput(input, { action: "back", text: "", selected: [] });
  assert.deepEqual(backed.selected, []); assert.equal(backed.combine_options, undefined);
});

test("anonymous admission and resume recovery retain combined intent only for the same question", () => {
  const store = storage();
  const state = { id: "owned", version: 3, question: { id: "history", stage: "knowledge", title: "Contexts", multiple: false,
    options: [{ id: "personal" }, { id: "shared" }] } };
  const anonymous = { key: inputKey({ ...state, id: null }), draft: "Unsent", selected: ["personal", "shared"], mode: "answer", combine_options: true };
  const admitted = admitNewGuideInput(store, state, anonymous);
  assert.deepEqual(admitted, { ...anonymous, key: inputKey(state) });
  const changed = admitNewGuideInput(store, { ...state, question: { ...state.question, title: "Different context" } }, anonymous);
  assert.deepEqual(changed.selected, []); assert.equal(changed.combine_options, undefined);
  assert.equal(captureGuideResumeInput(store, state, admitted, "Review saved progress"), true);
  const running = { ...state, operation: { request_id: "c".repeat(64), version: 3, status: "running",
    request: { version: 3, action: "chat", text: "Review saved progress", selected: [], stage: null, blueprint: null } } };
  assert.deepEqual(recoverGuideResumeInput(store, running, admitted), admitted);
  const completed = { ...running, operation: { ...running.operation, status: "completed" } };
  assert.deepEqual(recoverCompletedInput(store, completed), admitted);
});
test("denied browser storage keeps raw drafts and exact resume intent only in this open page", () => {
  const store = createGuideStorage(() => { throw new Error("Storage denied"); });
  const state = { id: "page-only", version: 3, question: { stage: "purpose", title: "Choose", options: [{ id: "private" }] } };
  const input = { key: inputKey(state), draft: "  Future thought\n\u00e5\u{1f9ed}  ", selected: ["private"], mode: "chat" };
  writeInput(store, input);
  assert.equal(store.persistent(), false);
  assert.deepEqual(readInput(store, state), input);
  assert.equal(captureGuideResumeInput(store, state, input, "Review saved progress"), true);
  assert.equal(preservedGuideResume(store, state), null);
  const returned = { ...state, operation: { request_id: "a".repeat(64), version: 3, status: "running",
    request: { version: 3, action: "chat", text: "Review saved progress", selected: [], stage: null, blueprint: null } } };
  assert.equal(preservedGuideResume(store, returned).request_id, returned.operation.request_id);
  assert.equal(preservedGuideResume(store, { ...returned, id: "other-guide" }), null);
  assert.equal(readInput(createGuideStorage(() => null), state).draft, "");
});

test("healthy browser reads remain current and a later read failure falls back to the last known page value", () => {
  const durable = storage(), store = createGuideStorage(() => durable);
  durable.setItem("draft", "First");
  assert.equal(store.getItem("draft"), "First");
  durable.setItem("draft", "Newer edit from another tab");
  assert.equal(store.getItem("draft"), "Newer edit from another tab");
  let notices = 0;
  const unsubscribe = store.subscribe(() => notices++);
  durable.getItem = () => { throw new Error("Read denied"); };
  assert.equal(store.getItem("draft"), "Newer edit from another tab");
  store.setItem("draft", "Latest open-page edit");
  assert.equal(store.getItem("draft"), "Latest open-page edit");
  assert.equal(store.persistent(), false);
  assert.equal(notices, 1);
  unsubscribe();
});

test("full or silently refusing storage cannot turn a browser completion marker into server completion", () => {
  for (const setItem of [() => { throw new Error("Quota exceeded"); }, () => {}]) {
    const durable = storage();
    durable.setItem = setItem;
    const store = createGuideStorage(() => durable);
    const server = { id: "saved-guide", version: 5, operation: { request_id: "b".repeat(64), status: "running" } };
    const before = structuredClone(server);
    assert.doesNotThrow(() => store.setItem(`mobkit-setup-completed-v1:${server.id}`, server.operation.request_id));
    assert.equal(store.persistent(), false);
    assert.deepEqual(server, before);
    // Recovery still uses the actual returned operation status and request.
    assert.equal(recoverCompletedInput(store, server).draft, "");
    assert.equal(server.operation.status, "running");
  }
});

test("failed removal is locally effective and never resurrects a consumed marker or another setup", () => {
  const durable = storage();
  durable.setItem("old-marker", "consumed");
  const store = createGuideStorage(() => durable);
  assert.equal(store.getItem("old-marker"), "consumed");
  durable.removeItem = () => { throw new Error("Remove denied"); };
  assert.doesNotThrow(() => store.removeItem("old-marker"));
  assert.equal(store.getItem("old-marker"), null);
  assert.equal(store.persistent(), false);
  store.setItem("current-draft", "Unsent");
  assert.deepEqual(store.keys(), ["current-draft"]);
  assert.equal(store.getItem("other-setup"), null);
});
test("durable admission clears submitted text and choices before the guide finishes", () => {
  const input = {key: "one", draft: "Build my team", selected: ["private"], mode: "chat"};
  assert.deepEqual(admitSubmittedInput(input, {action: "answer", text: "Build my team", selected: ["private"]}),
    {key: "one", draft: "", selected: [], mode: "answer"});
  assert.deepEqual(admitSubmittedInput({...input, draft: "A new thought", selected: ["shared"]},
    {action: "answer", text: "Build my team", selected: ["private"]}),
    {...input, draft: "A new thought", selected: ["shared"]});
  assert.equal(admitSubmittedInput(input, {action: "chat", text: "Help me choose", selected: []}).draft, "Build my team");
});
test("implementation download preserves the current agent's interfaces and acceptance checks", () => {
  const detail = "Expected app-to-agent interface:\nuser_id, school_id, question\n\nAcceptance checks:\nA Pine user cannot read Oak case history.";
  const state = {status: "handoff", proposal: {project: {name: "School support"}}, brief: {facts: {}, requirements: []}, messages: [
    {role: "assistant", text: "Obsolete draft"},
    {role: "user", text: "Prepare the implementation handoff."},
    {role: "assistant", text: detail},
  ]};
  const output = implementationHandoff(state);
  assert.ok(output.includes(detail));
  assert.ok(!output.includes("Obsolete draft"));
  assert.ok(output.includes("remains unimplemented"));
});
test("drafts and valid choices survive reload without crossing questions or setups", () => {
  const store = storage();
  const state = {
    id: "one",
    question: {
      stage: "connections",
      title: "Your sources?",
      options: [{ id: "files" }],
    },
  };
  writeInput(store, {
    key: inputKey(state),
    draft: "Keep this context",
    selected: ["files", "invented"],
    mode: "chat",
  });
  assert.deepEqual(readInput(store, state).selected, ["files"]);
  assert.equal(readInput(store, state).draft, "Keep this context");
  assert.equal(readInput(store, state).mode, "chat");
  assert.equal(readInput(store, { ...state, id: "two" }).draft, "");
  assert.equal(
    readInput(store, {
      ...state,
      question: { ...state.question, title: "Something else?" },
    }).draft,
    "Keep this context",
  );
});
test("adaptive question meaning scopes choices while unsent text survives a changed decision", () => {
  const before = { id: "adaptive", question: { id: "shared_access", stage: "purpose",
    title: "How should this work?", description: "Choose who can share context.", multiple: true,
    allow_text: true, options: [
      { id: "staff", label: "Staff share context", description: "Shared staff workspace" },
      { id: "external", label: "Include outside helpers", description: "External helpers can participate" },
    ] } };
  const draft = "  My own next detail\n\u00e5\u{1f9ed}  ";
  const edits = [
    question => { question.id = "primary_entrypoint"; },
    question => { question.stage = "connections"; },
    question => { question.title = "How should people use this?"; },
    question => { question.description = "Choose one primary interface."; },
    question => { question.multiple = false; },
    question => { question.allow_text = false; },
    question => { question.options[0].label = "Staff web app"; },
    question => { question.options[0].description = "Staff use their own browser login"; },
    question => { question.options.reverse(); },
  ];
  for (const edit of edits) {
    const store = storage(), next = structuredClone(before);
    writeInput(store, { key: inputKey(before), draft, selected: ["staff", "external"], mode: "chat" });
    edit(next.question);
    assert.notEqual(inputKey(next), inputKey(before));
    assert.deepEqual(readInput(store, next), { key: inputKey(next), draft, selected: [], mode: "chat" });
    const completed = { ...next, operation: { status: "completed", request: { text: "Earlier submitted answer", selected: [] } } };
    assert.deepEqual(recoverCompletedInput(store, completed), readInput(store, next));
    assert.equal(readInput(store, { ...next, id: "another" }).draft, "");
  }
  assert.equal(inputKey({ ...before, version: 12 }), inputKey(before), "an unrelated guide revision keeps this decision scoped");
  assert.equal(inputKey({ ...before, question: { ...before.question } }), inputKey(before));
});
test("older ambiguous keys recover only text and malformed single-choice drafts restore no implicit choice", () => {
  const state = { id: "legacy", question: { id: "audience", stage: "purpose", title: "Who is it for?",
    description: "Choose one primary audience.", multiple: false, allow_text: true,
    options: [{ id: "staff", label: "Staff", description: "Internal staff" },
      { id: "guests", label: "Guests", description: "External guests" }] } };
  const store = storage(), oldKey = 'mobkit-setup-input-v2:legacy:["purpose","Who is it for?",["staff","guests"]]';
  writeInput(store, { key: oldKey, draft: " Keep my description. ", selected: ["staff", "guests"], mode: "answer" });
  assert.deepEqual(readInput(store, state), { key: inputKey(state), draft: " Keep my description. ", selected: [], mode: "answer" });
  writeInput(store, { key: inputKey(state), draft: "", selected: ["staff", "guests"], mode: "answer" });
  assert.deepEqual(readInput(store, state).selected, []);
  writeInput(store, { key: inputKey(state), draft: "", selected: ["staff", "staff"], mode: "answer" });
  assert.deepEqual(readInput(store, state).selected, ["staff"]);
});
test("loading another saved setup restores its own draft, while anonymous text transfers only through new admission", () => {
  const store = storage(), question = { id: "purpose", stage: "purpose", title: "What should it do?",
    description: "Describe the first job.", multiple: false, options: [] };
  const anonymous = { key: inputKey({ question }), draft: " My workshop idea\n\u00e5 ", selected: [], mode: "chat" };
  const telescope = { id: "telescope", question }, clinic = { id: "clinic", question };
  const savedTelescope = { key: inputKey(telescope), draft: " My telescope follow-up ", selected: [], mode: "answer" };
  const savedClinic = { key: inputKey(clinic), draft: " My clinic follow-up ", selected: [], mode: "chat" };
  for (const input of [anonymous, savedTelescope, savedClinic]) writeInput(store, input);
  assert.deepEqual(guideInputForState(store, telescope, anonymous), savedTelescope);
  assert.deepEqual(guideInputForState(store, clinic, savedTelescope), savedClinic);
  assert.deepEqual(guideInputForState(store, { question }, savedClinic), anonymous);
  const admitted = { id: "newly-admitted", question };
  assert.equal(guideInputForState(store, admitted, anonymous).draft, "");
  assert.deepEqual(admitNewGuideInput(store, admitted, anonymous), { ...anonymous, key: inputKey(admitted) });
  assert.equal(admitNewGuideInput(store, admitted, savedTelescope).draft, "");
  assert.deepEqual(readInput(store, telescope), savedTelescope);
  assert.deepEqual(readInput(store, clinic), savedClinic);
  assert.deepEqual(readInput(store, { question }), anonymous);
});
test("same-setup live edits survive question refresh, and anonymous admission never carries old choice meaning", () => {
  const store = storage(), before = { id: "one", question: { id: "sources", stage: "connections",
    title: "Sources?", description: "Pick useful sources.", multiple: true, options: [{ id: "files" }] } };
  const current = { key: inputKey(before), draft: "  My newer edit\n ", selected: ["files"], mode: "chat" };
  const next = { ...before, question: { ...before.question, description: "Pick a primary source.", multiple: false } };
  assert.deepEqual(guideInputForState(store, next, current), { key: inputKey(next), draft: current.draft, selected: [], mode: "chat" });
  assert.deepEqual(guideInputForState(store, before, { ...current, draft: "" }), { ...current, draft: "" });
  const anonymous = { ...current, key: inputKey({ ...before, id: null }) };
  assert.deepEqual(admitNewGuideInput(store, { ...next, id: "admitted" }, anonymous), {
    key: inputKey({ ...next, id: "admitted" }), draft: current.draft, selected: [], mode: "chat" });
});
test("a completed response carries an unsent future draft across a closed browser and a changed question", () => {
  const store = storage();
  const before = {id: "one", question: {stage: "purpose", title: "Before", options: [{id: "old"}]}};
  const after = {id: "one", question: {stage: "connections", title: "After", options: [{id: "new"}]}, operation: {status: "completed", request: {text: "Submitted answer", selected: ["old"]}}};
  writeInput(store, {key: inputKey(before), draft: "Important future draft", selected: ["old"], mode: "chat"});
  assert.deepEqual(recoverCompletedInput(store, after), {key: inputKey(after), draft: "Important future draft", selected: [], mode: "chat"});
  assert.equal(recoverCompletedInput(store, {...after, id: "two"}).draft, "");
  writeInput(store, {key: inputKey(before), draft: "Submitted answer", selected: ["old"], mode: "answer"});
  assert.equal(recoverCompletedInput(store, after).draft, "");
  writeInput(store, {key: inputKey(after), draft: "Current question draft", selected: ["new"], mode: "answer"});
  assert.equal(recoverCompletedInput(store, after).draft, "Current question draft");
});

const resumeText = "Review the actual lifecycle evidence and recommend the next useful decision.";
const resumeBefore = { id: "resume-setup", version: 7,
  question: { stage: "review", title: "Your saved choices?", options: [{ id: "shared" }, { id: "private" }] } };
const resumeInput = { key: inputKey(resumeBefore), draft: resumeText, selected: ["shared"], mode: "answer" };
const resumeOperation = { request_id: "a".repeat(64), version: 7, status: "completed",
  request: { version: 7, action: "chat", text: resumeText, selected: [], stage: null, blueprint: null } };
const resumeAfter = { ...resumeBefore, version: 8, operation: resumeOperation };

test("workplace context adoption preserves unsent input through completion and reload", () => {
  const store = storage(), intent = { action: "organization", organization_fingerprint: "b".repeat(64) };
  assert.equal(captureGuideResumeInput(store, resumeBefore, resumeInput, "", intent), true);
  const after = { ...resumeAfter, operation: { ...resumeOperation,
    request: { ...resumeOperation.request, ...intent, text: "" } } };
  assert.equal(preservedGuideResume(store, after).request_id, resumeOperation.request_id);
  assert.deepEqual(recoverGuideResumeInput(store, after, resumeInput), resumeInput);
  assert.deepEqual(recoverCompletedInput(store, after), resumeInput);
  assert.deepEqual(readInput(store, after), resumeInput);
  for (const changed of [{ organization_fingerprint: "c".repeat(64) }, { action: "chat" }, { text: "Another request" }]) {
    assert.equal(preservedGuideResume(store, { ...after,
      operation: { ...after.operation, request: { ...after.operation.request, ...changed } } }), null);
  }
});

test("an explicit guide resume preserves a same-text draft and answer choices at admission, completion and reload", () => {
  const store = storage(), before = structuredClone(resumeInput);
  assert.equal(captureGuideResumeInput(store, resumeBefore, resumeInput, resumeText), true);
  assert.equal(preservedGuideResume(store, { ...resumeBefore, operation: { ...resumeOperation, status: "running" } }).request_id,
    resumeOperation.request_id);
  assert.deepEqual(recoverGuideResumeInput(store, resumeAfter, resumeInput), resumeInput);
  assert.deepEqual(recoverCompletedInput(store, resumeAfter), resumeInput);
  assert.deepEqual(readInput(store, resumeAfter), resumeInput);
  assert.deepEqual(resumeInput, before);
  assert.equal(admitSubmittedInput(resumeInput, { action: "chat", text: resumeText }).draft, "");
  assert.deepEqual(guideInputForState(store, resumeAfter, resumeInput, { newlyCompleted: true }), resumeInput);
});

test("only a newly acknowledged completion consumes matching live input during same-setup reconciliation", () => {
  const store = storage();
  const before = { id: "completion-reload", question: { id: "before", stage: "purpose", title: "Before", options: [] } };
  const input = { key: inputKey(before), draft: "Submitted answer", selected: [], mode: "answer" };
  const after = { ...before, question: { ...before.question, id: "after", title: "After" },
    operation: { status: "completed", request_id: "c".repeat(64),
      request: { action: "answer", text: input.draft, selected: [] } } };
  writeInput(store, input);
  writeInput(store, recoverCompletedInput(store, after));
  assert.equal(guideInputForState(store, after, input, { newlyCompleted: true }).draft, "");
  assert.equal(guideInputForState(store, after, { ...input, draft: "  Newer unsent detail\n\u00e5  " },
    { newlyCompleted: true }).draft, "  Newer unsent detail\n\u00e5  ");
  assert.equal(guideInputForState(store, after, input).draft, input.draft,
    "an already acknowledged completion cannot consume a new same-text draft");
  assert.equal(guideInputForState(store, { ...after, id: "another-setup" }, input,
    { newlyCompleted: true }).draft, "");
});

test("resume keeps old choices under their original question and carries exact unsent edits into a new question", () => {
  const store = storage();
  captureGuideResumeInput(store, resumeBefore, resumeInput, resumeText);
  const editing = { ...resumeInput, draft: "My unsent next thought\n\u00e5", selected: ["private"], mode: "chat" };
  const next = { ...resumeAfter,
    question: { stage: "review", title: "A different decision?", options: [{ id: "shared" }, { id: "new" }] } };
  const result = recoverGuideResumeInput(store, next, editing);
  assert.equal(result.key, inputKey(next));
  assert.equal(result.draft, editing.draft);
  assert.equal(result.mode, "chat");
  assert.deepEqual(result.selected, []);
  assert.deepEqual(readInput(store, resumeBefore), editing);
  assert.deepEqual(recoverCompletedInput(store, next), result);
  // Even an ID shared by two questions is not implicitly answered in the new one.
  assert.deepEqual(readInput(store, next).selected, []);
  const cleared = { ...result, draft: "", selected: ["new"], mode: "answer" };
  assert.deepEqual(recoverGuideResumeInput(store, next, cleared), cleared);
  assert.deepEqual(readInput(store, resumeBefore).selected, ["private"]);
});

test("ambiguous transport and missing immediate operation metadata preserve input until the real receipt arrives", () => {
  const store = storage();
  captureGuideResumeInput(store, resumeBefore, resumeInput, resumeText);
  assert.equal(preservedGuideResume(store, { ...resumeAfter, operation: undefined }), null);
  assert.deepEqual(readInput(store, resumeBefore), resumeInput);
  for (const status of ["failed", "interrupted"]) {
    const next = { ...resumeBefore, operation: { ...resumeOperation, status } };
    assert.equal(preservedGuideResume(store, next).request_id, resumeOperation.request_id);
    assert.deepEqual(readInput(store, next), resumeInput);
  }
  assert.deepEqual(recoverCompletedInput(store, resumeAfter), resumeInput);
});

test("resume preservation is bound to the exact returned request ID, original version, body and input scope", () => {
  for (const change of [{ id: "another-setup" }, { operation: { ...resumeOperation, version: 6 } },
    { operation: { ...resumeOperation, request_id: ["a".repeat(64)] } },
    { operation: { ...resumeOperation, request: { ...resumeOperation.request, version: 6 } } },
    { operation: { ...resumeOperation, request: { ...resumeOperation.request, text: "A different request" } } },
    { operation: { ...resumeOperation, request: { ...resumeOperation.request, selected: ["shared"] } } },
    { operation: { ...resumeOperation, request: { ...resumeOperation.request, action: "answer" } } },
    { operation: { ...resumeOperation, request: { ...resumeOperation.request, stage: "destination" } } },
    { operation: { ...resumeOperation, request: { ...resumeOperation.request, blueprint: {} } } }]) {
    const store = storage();
    captureGuideResumeInput(store, resumeBefore, resumeInput, resumeText);
    assert.equal(preservedGuideResume(store, { ...resumeAfter, ...change }), null);
    assert.deepEqual(readInput(store, resumeBefore), resumeInput);
  }
  const store = storage();
  captureGuideResumeInput(store, resumeBefore, resumeInput, resumeText);
  preservedGuideResume(store, resumeAfter);
  assert.equal(preservedGuideResume(store, { ...resumeAfter, operation: { ...resumeOperation, request_id: "b".repeat(64) } }), null);
  assert.equal(captureGuideResumeInput(store, resumeBefore, { ...resumeInput, key: inputKey({ ...resumeBefore, id: "other" }) }, resumeText), false);
  assert.equal(captureGuideResumeInput({ getItem: () => null, setItem() { throw new Error("storage unavailable"); } },
    resumeBefore, resumeInput, resumeText), false);
});

test("late guide loads and polled operations cannot replace a different setup, version or completed request", () => {
  const gate = createGuideLoadGate(), first = gate.begin(), second = gate.begin();
  assert.equal(gate.current(first), false);
  assert.equal(gate.finish(first), false);
  assert.equal(gate.pending(), true);
  assert.equal(gate.finish(second), true);
  assert.equal(gate.current(second), true);
  assert.equal(gate.pending(), false);
  const current = { ...resumeBefore, operation: { ...resumeOperation, status: "running" } };
  const watched = { guide_id: current.id, guide_version: current.version,
    request_id: current.operation.request_id, operation_version: current.operation.version };
  assert.equal(guideOperationReadMatches(watched, current, resumeAfter), true);
  for (const change of [{ id: "different" }, { version: 8 }, { operation: { ...current.operation, status: "completed" } },
    { operation: { ...current.operation, request_id: "b".repeat(64) } }, { operation: { ...current.operation, version: 6 } }])
    assert.equal(guideOperationReadMatches(watched, { ...current, ...change }, resumeAfter), false);
  for (const change of [{ id: "different" }, { version: 6 },
    { operation: { ...resumeOperation, request_id: "b".repeat(64) } }, { operation: { ...resumeOperation, version: 6 } }])
    assert.equal(guideOperationReadMatches(watched, current, { ...resumeAfter, ...change }), false);
});

test("reloading the same running operation enrolls a new watch rather than keeping a dead navigation token", () => {
  const gate = createGuideLoadGate(), initial = gate.begin();
  gate.finish(initial);
  const running = { ...resumeBefore, operation: { ...resumeOperation, status: "running" } };
  const watched = { guide_id: running.id, guide_version: running.version,
    request_id: running.operation.request_id, operation_version: running.operation.version };
  const reload = gate.begin();
  assert.equal(gate.pending(), true);
  assert.equal(gate.current(initial), false);
  gate.finish(reload);
  const reenrolled = gate.token();
  assert.equal(gate.current(reenrolled), true);
  assert.equal(gate.pending(), false);
  assert.equal(gate.current(initial) && guideOperationReadMatches(watched, running, resumeAfter), false);
  assert.equal(gate.current(reenrolled) && guideOperationReadMatches(watched, running, resumeAfter), true);
});

test("a late missing-guide response cannot erase a newer selection or enter its fallback path", () => {
  const store = storage(), gate = createGuideLoadGate(), key = "selected-guide";
  store.setItem(key, "setup-A");
  const old = gate.begin(), next = gate.begin();
  store.setItem(key, "setup-B");
  gate.finish(next);
  assert.equal(recoverMissingGuideSelection(store, key, "setup-A", gate, old), false);
  assert.equal(store.getItem(key), "setup-B");
  // Current missing A still cannot delete a different saved B selection.
  const current = gate.begin();
  assert.equal(recoverMissingGuideSelection(store, key, "setup-A", gate, current), true);
  assert.equal(store.getItem(key), "setup-B");
  assert.equal(recoverMissingGuideSelection(store, key, "setup-B", gate, current), true);
  assert.equal(store.getItem(key), undefined);
});

test("a delayed failed poll cannot put a reconnect error over an already completed operation", async () => {
  let reject;
  const response = new Promise((resolve, no) => { reject = no; });
  let current = { ...resumeBefore, operation: { ...resumeOperation, status: "running" } }, error = "";
  const watched = { guide_id: current.id, guide_version: current.version,
    request_id: current.operation.request_id, operation_version: current.operation.version };
  const pending = response.catch(() => {
    if (guideOperationReadMatches(watched, current, current)) error = "Connection interrupted";
  });
  current = resumeAfter;
  reject(new Error("a delayed read failed"));
  await pending;
  assert.equal(error, "");
});
test("workflow summary respects parallel dependencies instead of source order", () => {
  const flow = {steps: [{id:"finish",depends_on:["check","a","b"]},{id:"a",depends_on:[]},{id:"check",depends_on:["a","b"]},{id:"b",depends_on:[]}]};
  assert.deepEqual(workflowLayers(flow).map(layer => layer.map(step => step.id)), [["a","b"],["check"],["finish"]]);
  assert.deepEqual(unresolvedRequirements({brief:{requirements:[{disposition:"cancelled",outcome:"Old job"},{disposition:"open",outcome:"Needed app"}]}}, null), ["Needed app"]);
});
test("host readiness groups are authoritative and warnings/prerequisites remain other checks", () => {
  const state = { readiness: { ready: false, context_ready: true,
    decision_blockers: ["Choose who operates it", "Choose who operates it"],
    implementation_blockers: ["Implement the requested staff page"] },
    brief: { requirements: [
      { disposition: "open", outcome: "Earlier decision" },
      { disposition: "open", outcome: "Earlier implementation", extension: "Build it" },
      { disposition: "prerequisite", outcome: "Provide local credentials" },
    ] }, warnings: ["Verify the actual connection", "Verify the actual connection"] };
  const before = structuredClone(state);
  assert.deepEqual(setupRequirementGroups(state), {
    decisions: ["Choose who operates it"], implementation: ["Implement the requested staff page"],
    checks: ["Provide local credentials", "Verify the actual connection"],
  });
  assert.deepEqual(state, before);
  assert.equal(state.readiness.ready, false);
});
test("authoritative empty groups suppress obsolete fallback work without promoting readiness", () => {
  const state = { readiness: { ready: false, decision_blockers: [], implementation_blockers: [] },
    brief: { requirements: [{ disposition: "open", outcome: "Old choice" },
      { disposition: "handoff", outcome: "Old missing code", extension: "Build it" }] } };
  assert.deepEqual(setupRequirementGroups(state), { decisions: [], implementation: [], checks: [] });
  assert.equal(state.readiness.ready, false);
});
test("saved feature groups use exact disposition and explicit extension, never wording", () => {
  const state = { brief: { requirements: [
    { disposition: "open", outcome: "Implementation still required: words in a pending choice", extension: "" },
    { disposition: "open", outcome: "Agree how to handle: words in missing code", extension: "Build the adapter" },
    { disposition: "handoff", outcome: "An implementation handoff" },
    { disposition: "prerequisite", outcome: "Install the required dependency" },
    ...["supported", "deferred", "cancelled"].map(disposition => ({ disposition, outcome: `Omit ${disposition}`, extension: "Old work" })),
  ] }, warnings: ["A separate warning"] };
  assert.deepEqual(setupRequirementGroups(state), {
    decisions: ["Implementation still required: words in a pending choice"],
    implementation: ["Agree how to handle: words in missing code: Build the adapter", "An implementation handoff"],
    checks: ["Install the required dependency", "A separate warning"],
  });
});
test("each typed host category can override its fallback independently", () => {
  const state = { readiness: { decision_blockers: [] }, brief: { requirements: [
    { disposition: "open", outcome: "Pending choice" },
    { disposition: "open", outcome: "Requested application", extension: "Build the custom UI" },
  ] } };
  assert.deepEqual(setupRequirementGroups(state), { decisions: [],
    implementation: ["Requested application: Build the custom UI"], checks: [] });
});
test("malformed categories and legacy checks do not become decisions or readiness grants", () => {
  const state = { readiness: { ready: false, decision_blockers: "not an array", implementation_blockers: [null, false, {}, "", "Saved host work"] },
    feature_gaps: ["A legacy check", { status: "prerequisite", description: "Install a dependency" },
      { disposition: "open", outcome: "Choose an operating owner", extension: {} }, null,
      { status: "accepted", outcome: "Already accepted" }], warnings: [null, true, "A warning", " "] };
  const before = structuredClone(state);
  assert.deepEqual(setupRequirementGroups(state), { decisions: ["Choose an operating owner"],
    implementation: ["Saved host work"], checks: ["A legacy check", "Install a dependency", "A warning"] });
  assert.deepEqual(state, before);
  assert.deepEqual(setupRequirementGroups(null), { decisions: [], implementation: [], checks: [] });
  assert.deepEqual(setupRequirementGroups({ brief: { requirements: [] }, feature_gaps: ["Obsolete legacy data"] }),
    { decisions: [], implementation: [], checks: [] });
});
test("local and cloud checklists include conditional knowledge startup and complete cloud handoff", () => {
  const plan = {
    project: { slug: "test" },
    target: "mac",
    knowledge: { mode: "none" },
    agent_classes: [],
    connections: [],
    advanced: { port: 4100 },
    gcp: { project: "demo", zone: "europe-west1-b" },
  };
  assert.ok(!JSON.stringify(installSteps(plan)).includes("manage.py"));
  const local = JSON.stringify(
    installSteps({ ...plan, knowledge: { mode: "local" } }),
  );
  assert.ok(local.includes("manage.py up"));
  assert.ok(local.includes("manage.py verify"));
  const remote = JSON.stringify(
    installSteps({ ...plan, target: "gcp", knowledge: { mode: "remote" } }),
  );
  assert.ok(!remote.includes("manage.py up"));
  assert.ok(remote.includes("setup.py"));
  assert.ok(remote.includes("install-service.sh"));
  assert.ok(remote.includes("tunnel-gcp.sh"));
});


test('starting context ownership: exact input reload retains metadata without latest-key migration across question or guide', () => {
  const store = storage(), state = { id: 'owned', question: { id: 'purpose', title: 'Describe your idea', options: [] } };
  const prefix = '  My project\r\n\t  \n\n', context = 'For a team, I would like help organizing information and commitments.';
  const starting_help = { audience_id: 'team', kind_id: 'organizing', prefix, inserted: prefix + context };
  const input = { key: inputKey(state), draft: prefix + context + '\n\nMy task.  \r\n', selected: [], mode: 'answer', starting_help };
  writeInput(store, input);
  assert.equal(JSON.parse(store.getItem('mobkit-setup-input-v2:owned:latest')).starting_help, undefined, 'Only the exact input key stores helper ownership.');
  assert.deepEqual(readInput(store, state), input);
  assert.deepEqual(guideInputForState(store, state, input), input);
  const next = { ...state, question: { ...state.question, title: 'A different question' } };
  const restored = readInput(store, next);
  assert.equal(restored.draft, input.draft);
  assert.equal(restored.starting_help, undefined, 'Latest draft migration carries writing only, never applied helper authority.');
  assert.equal(guideInputForState(store, next, input).starting_help, undefined);
  assert.equal(readInput(store, { ...state, id: 'other' }).starting_help, undefined);
  assert.equal(readInput(store, { ...state, id: null }).starting_help, undefined);
  assert.equal(admitNewGuideInput(store, state, { ...input, key: inputKey({ ...state, id: null }) }).starting_help, undefined);
  assert.equal(admitSubmittedInput(input, { action: 'answer', text: input.draft, selected: [] }).starting_help, undefined);
  const textOnly = { ...input }; delete textOnly.starting_help;
  assert.equal(guideInputForState(store, state, textOnly).starting_help, undefined, 'A current hint-free draft cannot borrow a stale saved hint.');
});

test('starting context ownership: invalidated, malformed and foreign-key hints restore only writing', () => {
  const state = { id: 'owned', question: null }, store = storage();
  const starting_help = { audience_id: 'team', kind_id: 'organizing', prefix: '', inserted: 'For a team, I would like help organizing information and commitments.' };
  const input = { key: inputKey(state), draft: starting_help.inserted + '\nMy task', selected: [], mode: 'answer', starting_help };
  for (const changes of [{ draft: '' }, { draft: input.draft.replace('organizing', 'sorting') },
    { starting_help: { ...starting_help, inserted: '' } }, { starting_help: { ...starting_help, prefix: 'unowned' } },
    { starting_help: { ...starting_help, kind_id: 2 } }, { starting_help: null }]) {
    writeInput(store, { ...input, ...changes });
    const restored = readInput(store, state);
    assert.equal(restored.draft, changes.draft ?? input.draft);
    assert.equal(restored.starting_help, undefined);
  }
  store.setItem(inputKey(state), JSON.stringify({ ...input, key: inputKey({ ...state, id: 'foreign' }) }));
  assert.equal(readInput(store, state).draft, input.draft);
  assert.equal(readInput(store, state).starting_help, undefined, 'Metadata requires the saved envelope to name this exact key.');
});

test('starting context ownership: denied storage keeps applied context in its exact open-page draft only', () => {
  const store = createGuideStorage(() => { throw new Error('Storage denied'); });
  const state = { id: null, question: null }, prefix = '  My own wording\r\n\n\n';
  const context = 'For a team, I would like help organizing information and commitments.';
  const input = { key: inputKey(state), draft: prefix + context + '\nLater task.  ', selected: [], mode: 'answer',
    starting_help: { audience_id: 'team', kind_id: 'organizing', prefix, inserted: prefix + context } };
  writeInput(store, input);
  assert.equal(store.persistent(), false);
  assert.deepEqual(readInput(store, state), input);
  assert.equal(readInput(createGuideStorage(() => null), state).draft, '');
  assert.equal(readInput(createGuideStorage(() => null), state).starting_help, undefined);
});

const startingContextHint = { version: 2, audience_id: 'team', kind_id: 'organizing' };
const startingContext = 'For a team, I would like help organizing information and commitments.';

test('separate starting context: submission joins context while preserving every raw draft byte', () => {
  const input = { key: inputKey({ id: 'owned' }), draft: '  My task\r\n\t\u00e5  \r\n', selected: [], mode: 'answer', starting_help: startingContextHint };
  assert.equal(inputStartingHelp(input), startingContextHint);
  assert.equal(inputSubmissionText(input), startingContext + '\n\n' + input.draft);
  assert.equal(inputSubmissionText({ ...input, draft: '' }), startingContext);
  assert.equal(inputSubmissionText({ ...input, draft: ' \r\n' }), startingContext + '\n\n \r\n');
  assert.equal(inputSubmissionText({ ...input, starting_help: undefined }), input.draft);
  assert.equal(inputSubmissionText({ ...input, starting_help: { ...startingContextHint, kind_id: 'unknown' } }), input.draft);
  const legacy = { audience_id: 'team', kind_id: 'organizing', prefix: '', inserted: startingContext };
  assert.equal(inputSubmissionText({ ...input, draft: startingContext + '\r\nTask', starting_help: legacy }), startingContext + '\r\nTask');
});

test('separate starting context: exact-key reload retains context-only input and wrong keys migrate raw writing only', () => {
  const store = storage(), state = { id: 'owned', question: { id: 'purpose', title: 'Your idea', options: [] } };
  const input = { key: inputKey(state), draft: '', selected: [], mode: 'answer', starting_help: startingContextHint };
  writeInput(store, input);
  assert.deepEqual(readInput(store, state), input);
  assert.equal(JSON.parse(store.getItem('mobkit-setup-input-v2:owned:latest')).starting_help, undefined);
  const written = { ...input, draft: '  My own words\r\n' };
  writeInput(store, written);
  const next = { ...state, question: { ...state.question, title: 'Changed question meaning' } };
  assert.equal(readInput(store, next).draft, written.draft);
  assert.equal(readInput(store, next).starting_help, undefined);
  assert.equal(guideInputForState(store, next, written).starting_help, undefined);
  for (const foreign of [{ ...state, id: 'another' }, { ...state, id: null }]) {
    assert.equal(readInput(store, foreign).starting_help, undefined);
    assert.equal(guideInputForState(store, foreign, written).starting_help, undefined);
  }
  // A forged latest envelope never carries choices into a different question.
  store.setItem('mobkit-setup-input-v2:owned:latest', JSON.stringify(written));
  assert.equal(readInput(store, next).starting_help, undefined);
  store.setItem(input.key, JSON.stringify({ ...written, key: inputKey(next) }));
  assert.equal(readInput(store, state).draft, written.draft);
  assert.equal(readInput(store, state).starting_help, undefined);
});

test('separate starting context: only explicit same-question anonymous creation transfers the hint', () => {
  const store = storage(), state = { id: 'created', question: { id: 'purpose', title: 'Your idea', options: [] } };
  for (const draft of ['', '  My task\r\n']) {
    const anonymous = { key: inputKey({ ...state, id: null }), draft, selected: [], mode: 'answer', starting_help: startingContextHint };
    assert.deepEqual(admitNewGuideInput(store, state, anonymous), { ...anonymous, key: inputKey(state) });
    assert.equal(guideInputForState(store, state, anonymous).starting_help, undefined);
    const changed = { ...state, question: { ...state.question, title: 'New meaning' } };
    assert.equal(admitNewGuideInput(store, changed, anonymous).starting_help, undefined);
    const another = { ...anonymous, key: inputKey({ ...state, id: 'existing' }) };
    assert.equal(admitNewGuideInput(store, state, another).starting_help, undefined);
  }
});

test('separate starting context: matching sends consume context and draft while later writing and choices remain', () => {
  const input = { key: inputKey({ id: 'owned' }), draft: 'Raw task\r\n', selected: ['private'], mode: 'answer', starting_help: startingContextHint };
  const submitted = { action: 'answer', text: startingContext + '\n\n' + input.draft, selected: ['private'] };
  const cleared = { key: input.key, draft: '', selected: [], mode: 'answer' };
  assert.deepEqual(admitSubmittedInput(input, submitted), cleared);
  assert.deepEqual(admitSubmittedInput({ ...input, draft: '' }, { ...submitted, text: startingContext }), cleared);
  const later = { ...input, draft: 'Later task\r\n' };
  assert.deepEqual(admitSubmittedInput(later, submitted), later);
  const changedChoices = { ...input, selected: ['shared'] };
  assert.deepEqual(admitSubmittedInput(changedChoices, submitted), changedChoices);
  const changedCombine = { ...input, combine_options: true };
  assert.deepEqual(admitSubmittedInput(changedCombine, submitted), changedCombine);
  const chat = admitSubmittedInput({ ...input, mode: 'chat' }, { action: 'chat', text: submitted.text });
  assert.deepEqual(chat, { ...cleared, selected: ['private'] });
  assert.deepEqual(admitSubmittedInput(input, { ...submitted, text: input.draft }), input, 'Raw text alone does not acknowledge a context-bearing send.');
});

test('separate starting context: edited or removed context is not consumed by an earlier send', () => {
  const store = storage(), state = { id: 'owned' };
  const input = { key: inputKey(state), draft: '  Original words\r\n', selected: [], mode: 'answer', starting_help: startingContextHint };
  const submitted = { action: 'answer', text: startingContext + '\n\n' + input.draft, selected: [] };
  const changed = { ...input, starting_help: { ...startingContextHint, kind_id: 'repetitive' } };
  writeInput(store, changed);
  assert.deepEqual(readInput(store, state), changed);
  assert.deepEqual(admitSubmittedInput(changed, submitted), changed);
  const removed = { ...input }; delete removed.starting_help;
  writeInput(store, removed);
  assert.deepEqual(readInput(store, state), removed);
  assert.deepEqual(admitSubmittedInput(removed, submitted), removed);
  const backed = admitSubmittedInput(changed, { action: 'back', text: '', selected: [] });
  assert.equal(backed.draft, input.draft);
  assert.equal(backed.starting_help, undefined);
});

test('separate starting context: live input never borrows a saved hint across changed questions', () => {
  const store = storage(), before = { id: 'owned', question: { id: 'purpose', title: 'Your idea', options: [] } };
  const next = { ...before, question: { ...before.question, title: 'A different question' } };
  const saved = { key: inputKey(next), draft: 'Same raw words\r\n', selected: [], mode: 'answer', starting_help: startingContextHint };
  writeInput(store, saved);
  const current = { ...saved, key: inputKey(before) }; delete current.starting_help;
  assert.equal(guideInputForState(store, next, current).starting_help, undefined);
  const removed = { ...saved }; delete removed.starting_help;
  assert.equal(guideInputForState(store, next, removed).starting_help, undefined);
});

test('separate starting context: acknowledged completion clears combined text without erasing a later task', () => {
  const store = storage(), state = { id: 'owned', version: 2 };
  const input = { key: inputKey(state), draft: 'My useful task\r\n', selected: [], mode: 'answer', starting_help: startingContextHint };
  const after = { ...state, operation: { status: 'completed', request: { action: 'answer', text: startingContext + '\n\n' + input.draft, selected: [] } } };
  writeInput(store, input);
  assert.equal(recoverCompletedInput(store, after).starting_help, undefined);
  assert.equal(recoverCompletedInput(store, after).draft, '');
  const later = { ...input, draft: 'A later useful task\r\n' };
  assert.equal(guideInputForState(store, after, later, { newlyCompleted: true }).draft, later.draft);
  assert.deepEqual(guideInputForState(store, after, later, { newlyCompleted: true }).starting_help, startingContextHint);
  assert.deepEqual(guideInputForState(store, after, input).starting_help, startingContextHint, 'Historical completion does not consume a deliberately repeated idea.');
});
