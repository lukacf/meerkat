import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const web = resolve(process.env.FORGE_PENDING_ATTEMPT_WEB || dirname(fileURLToPath(import.meta.url)));
const require = createRequire(resolve(web, "package.json"));
const { build } = require("esbuild");
const replacement = process.env.FORGE_PENDING_ATTEMPT_SOURCE;
const built = await build({
  absWorkingDir: web, entryPoints: [resolve(web, "native-guide-chat.mjs")],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
  outfile: "guide-pending-attempt.mjs", sourcemap: "external", sourcesContent: true,
  nodePaths: [resolve(web, "node_modules")],
  plugins: [{ name: "exact-guide-source", setup(builder) {
    builder.onLoad({ filter: /native-guide-chat\.mjs$/ }, async args => {
      if (!replacement || args.path !== resolve(web, "native-guide-chat.mjs")) return;
      return { contents: await readFile(replacement, "utf8"), loader: "js", resolveDir: web };
    });
  } }],
});
const javascript = built.outputFiles.find(file => file.path.endsWith(".mjs"));
const sourceMap = built.outputFiles.find(file => file.path.endsWith(".map"));
if (process.env.FORGE_PENDING_ATTEMPT_BUNDLE) {
  await writeFile(process.env.FORGE_PENDING_ATTEMPT_BUNDLE, javascript.contents);
  await writeFile(`${process.env.FORGE_PENDING_ATTEMPT_BUNDLE}.map`, sourceMap.contents);
}
export const { createGuideFrameLog, ingestGuideTimeline, guideConversation } =
  await import(`data:text/javascript;base64,${Buffer.from(javascript.contents).toString("base64")}`);

// These native-frame and operation fixtures match the maintained adapter harness.
export const frame = (id, kind, payload, offset = 0) => ({
  id, kind, payload, interaction_id: "turn-one", identity: "architect:main",
  timestamp_ms: 1000 + offset, cursor: `console:${offset + 1}`, frame_version: 1,
  source: { kind: "runtime" },
});
export const binding = { interaction_id: "turn-one", user_message_id: "saved-user", user_text: "Help my team",
  operation_request_id: "a".repeat(64), operation_started_at: "2026-10-04T09:00:00Z" };
export const state = {
  id: "setup-one", operation: { status: "running", request_id: binding.operation_request_id,
    started_at: binding.operation_started_at }, messages: [],
};

test("an identical retry stays pending until its own running attempt has an unambiguous native admission", () => {
  const log = createGuideFrameLog();
  const ref = { id: "evidence-note", filename: "note.txt", media_type: "text/plain",
    url: "/api/guide/setup-one/attachments/evidence-note" };
  const oldBinding = { ...binding, user_text: "Yes", user_message_id: "failed-user", attachments: [ref] };
  let frames = ingestGuideTimeline(log, { frames: [
    frame("failed-input", "user_input", { content: "Yes\n\n[note.txt](/api/guide/setup-one/attachments/evidence-note)" }),
    frame("failed-tool", "tool_call_requested", { id: "old-call", name: "publish_guide_step", args: {} }, 1),
    frame("failed-result", "tool_result_received", { id: "old-call", name: "publish_guide_step", is_error: true }, 2),
    frame("failed-terminal", "interaction_failed", { error: "Publication was not saved" }, 3),
  ] });
  const failed = { ...state, operation: { ...state.operation, status: "failed" }, messages: [
    { id: "prior-user", role: "user", text: "Help our observing club", created_at: new Date(100).toISOString() },
    { id: "prior-guide", role: "assistant", text: "Which histories should be private?", created_at: new Date(200).toISOString() },
  ] };
  const pendingMessage = { role: "user", text: "Yes", attachments: [ref], created_at: new Date(2000).toISOString() };
  const pending = result => result.entries.some(entry => entry.id === "setup-message:pending");
  const retainsHistory = result => {
    assert.ok(result.entries.some(entry => entry.id === "setup-message:prior-user"));
    assert.ok(result.entries.some(entry => entry.id === "setup-message:prior-guide"));
    assert.ok(result.entries.some(entry => entry.interactionId === oldBinding.interaction_id && entry.identity.role === "user"));
    assert.ok(result.entries.some(entry => entry.interactionId === oldBinding.interaction_id &&
      entry.blocks?.some(block => block.type === "tool-call")), "The failed native activity stays in history");
  };
  let result = guideConversation({ state: failed, frames, bindings: [oldBinding], pendingMessage });
  assert.equal(pending(result), true, "A failed old turn must not consume the new pre-admission preview");
  retainsHistory(result);

  const retry = { ...failed, operation: { ...state.operation, started_at: "2026-10-04T09:01:00Z" } };
  const newBinding = { ...oldBinding, interaction_id: "turn-two", user_message_id: "retry-user",
    operation_started_at: retry.operation.started_at };
  result = guideConversation({ state: retry, frames, bindings: [oldBinding, newBinding], pendingMessage });
  assert.equal(pending(result), true, "A binding without its native user frame cannot consume the preview");
  frames = ingestGuideTimeline(log, { frames: [{
    ...frame("retry-input", "user_input", { content: "Yes\n\n[note.txt](/api/guide/setup-one/attachments/evidence-note)" }, 1000),
    interaction_id: "turn-two",
  }, { ...frame("other-input", "user_input", { content: "Yes" }, 1001), interaction_id: "ambiguous-turn" }] });
  for (const candidate of [
    { ...newBinding, operation_request_id: "b".repeat(64) },
    { ...newBinding, operation_started_at: oldBinding.operation_started_at },
    { interaction_id: newBinding.interaction_id, user_message_id: newBinding.user_message_id, user_text: "Yes", attachments: [ref] },
  ]) {
    result = guideConversation({ state: retry, frames, bindings: [oldBinding, candidate], pendingMessage });
    assert.equal(pending(result), true, "Wrong or absent native attempt metadata must not reconcile a repeated answer");
    retainsHistory(result);
  }
  result = guideConversation({ state: retry, frames, bindings: [oldBinding, newBinding,
    { ...newBinding, interaction_id: "ambiguous-turn" }], pendingMessage });
  assert.equal(pending(result), true, "Ambiguous matching native attempts cannot consume the preview");
  for (const candidate of [
    { ...newBinding, user_text: "Different text" },
    { ...newBinding, attachments: [{ ...ref, id: "another-note" }] },
  ]) {
    result = guideConversation({ state: retry, frames, bindings: [oldBinding, candidate], pendingMessage });
    assert.equal(pending(result), true, "Current attempt still requires matching submitted text and attachments");
  }
  result = guideConversation({ state: retry, frames, bindings: [oldBinding, newBinding], pendingMessage });
  assert.equal(pending(result), false, "Only the exact newly admitted running attempt replaces its pending preview");
  retainsHistory(result);
  assert.equal(result.entries.filter(entry => entry.interactionId === "turn-two" && entry.identity.role === "user").length, 1);
  assert.equal(result.entries.filter(entry => entry.interactionId === oldBinding.interaction_id && entry.identity.role === "user").length, 1);
});
