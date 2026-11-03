import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const source = process.env.FORGE_WIZARD_STATE_SOURCE
  ? pathToFileURL(resolve(process.env.FORGE_WIZARD_STATE_SOURCE)) : new URL("./wizard-state.mjs", import.meta.url);
const {
  createGuideStorage, inputKey, readInput, writeInput, inputSubmissionText,
  captureGuideResumeInput, preservedGuideResume, recoverGuideResumeInput, recoverCompletedInput,
} = await import(source.href);

const guide = {
  id: "starting-context-owner", version: 3,
  question: { id: "purpose", stage: "purpose", title: "What would you like to get done?",
    description: "Describe one useful job.", multiple: false, allow_text: true, options: [] },
};
const context = { version: 2, audience_id: "team", kind_id: "repetitive" };
const discussion = "Discuss how the reviewed saved connection fits this team.";
const originalInput = (draft = "  Keep our handover notes useful.\n  ") => ({
  key: inputKey(guide), draft, selected: [], mode: "answer", starting_help: { ...context },
});
const operation = status => ({ request_id: "a".repeat(64), version: guide.version, status,
  request: { version: guide.version, action: "chat", text: discussion, selected: [], stage: null, blueprint: null } });
const returned = (status = "completed") => ({ ...guide, version: guide.version + 1, operation: operation(status) });
const latestKey = `mobkit-setup-input-v2:${guide.id}:latest`;

function browser() {
  const values = new Map();
  const durable = {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  };
  return { values, durable, page: () => createGuideStorage(() => durable) };
}
function capture(store, input) {
  writeInput(store, input);
  assert.equal(captureGuideResumeInput(store, guide, input, discussion), true);
}

test("saved-tools capture keeps exact unsent audience/type choices through a pending page reload", () => {
  const storage = browser(), input = originalInput(), page = storage.page();
  capture(page, input);
  assert.deepEqual(readInput(storage.page(), guide), input,
    "a new page must recover the separate choices as well as the raw composer text");
  assert.equal(JSON.parse(storage.durable.getItem(latestKey)).starting_help, undefined,
    "latest remains a writing-only migration record");
  const marker = preservedGuideResume(storage.page(), returned("running"));
  assert.deepEqual(marker.input, input);
  assert.equal(marker.text, discussion, "discussion text remains distinct from the unsent idea");
  assert.equal(marker.request_id, operation("running").request_id);
});

test("completed discussion remount recovers a choices-only starting point without consuming or inserting prose", () => {
  const storage = browser(), input = originalInput("");
  capture(storage.page(), input);
  const recovered = recoverCompletedInput(storage.page(), returned());
  assert.deepEqual(recovered, input, "completion must retain the unsent audience/type metadata");
  assert.equal(recovered.draft, "", "recovery cannot rewrite the raw composer with generated context");
  assert.equal(inputSubmissionText(recovered), "For a team, I would like help with repetitive work.");
  assert.deepEqual(readInput(storage.page(), returned()), input);
  assert.equal(JSON.parse(storage.durable.getItem(latestKey)).starting_help, undefined);
});

test("remount recovery uses newer same-input context edits and removals instead of the capture", () => {
  for (const retainedContext of [{ version: 2, audience_id: "household", kind_id: "organizing" }, null]) {
    const storage = browser(), input = originalInput();
    capture(storage.page(), input);
    const edited = { ...input };
    if (retainedContext) edited.starting_help = retainedContext;
    else delete edited.starting_help;
    writeInput(storage.page(), edited);
    assert.deepEqual(recoverCompletedInput(storage.page(), returned()), edited);
    assert.deepEqual(readInput(storage.page(), returned()), edited);
    assert.equal(JSON.parse(storage.durable.getItem(latestKey)).starting_help, undefined);
  }
});

test("live removal owns the exact input even when an earlier full record still has starting context", () => {
  const storage = browser(), input = originalInput();
  capture(storage.page(), input);
  const editing = { ...input }; delete editing.starting_help;
  assert.deepEqual(recoverGuideResumeInput(storage.page(), returned(), editing), editing);
  assert.deepEqual(readInput(storage.page(), returned()), editing,
    "the persisted capture cannot resurrect deliberately removed live choices");
});

test("a changed question retains writing but cannot acquire starting choices from the original question", () => {
  const storage = browser(), input = originalInput();
  capture(storage.page(), input);
  const next = { ...returned(), question: { ...guide.question, title: "Who needs their own private history?" } };
  const recovered = recoverCompletedInput(storage.page(), next);
  assert.equal(recovered.key, inputKey(next));
  assert.equal(recovered.draft, input.draft);
  assert.equal(recovered.starting_help, undefined);
  assert.deepEqual(readInput(storage.page(), guide), input,
    "the unsent choices stay saved under the original question only");
});

test("another guide cannot consume the discussion marker or borrow its unsent starting point", () => {
  const storage = browser(), input = originalInput();
  capture(storage.page(), input);
  const before = [...storage.values];
  const other = { ...returned(), id: "other-starting-context-owner" };
  assert.equal(recoverGuideResumeInput(storage.page(), other, input), null);
  assert.equal(readInput(storage.page(), other).starting_help, undefined);
  assert.equal(readInput(storage.page(), other).draft, "");
  assert.deepEqual([...storage.values], before, "foreign readback cannot rewrite the original input");
});

test("mismatched full records cannot attach stale starting choices to the latest unsent input", () => {
  for (const changed of [
    { key: inputKey({ ...guide, question: { ...guide.question, title: "Different meaning" } }) },
    { draft: "A newer unsent task" }, { mode: "chat" }, { selected: ["newer-selection"] }, { combine_options: true },
  ]) {
    const storage = browser(), input = originalInput();
    capture(storage.page(), input);
    const latest = JSON.parse(storage.durable.getItem(latestKey));
    storage.durable.setItem(input.key, JSON.stringify({ ...input, ...changed }));
    const recovered = recoverCompletedInput(storage.page(), returned());
    assert.equal(recovered.starting_help, undefined, "every ordinary input field must still match");
    assert.equal(recovered.draft, latest.draft);
    assert.equal(recovered.mode, latest.mode);
  }
});
