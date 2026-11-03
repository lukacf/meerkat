import { blueprintDiff, clone } from "./state.mjs";

export const GUIDE_KEY = "mobkit-launch-guide-v1";
export const GUIDE_MANUAL_KEY = "mobkit-launch-guide-manual-v1";
export function pendingManual(storage, guideId) {
  try {
    const value = JSON.parse(storage.getItem(GUIDE_MANUAL_KEY));
    return value?.guideId === guideId && value.feedback?.blueprint &&
      Array.isArray(value.feedback.changes) && typeof value.feedback.text === "string"
      ? value.feedback : null;
  } catch {
    return null;
  }
}
export const GUIDE_STAGES = [
  { id: "purpose", name: "Your idea", detail: "What should it help with?" },
  { id: "connections", name: "Your tools", detail: "Connect what you use" },
  { id: "knowledge", name: "Your knowledge", detail: "Give it useful context" },
  { id: "autonomy", name: "Working together", detail: "Choose how it helps" },
  { id: "destination", name: "Where it lives", detail: "Choose your computer" },
  { id: "review", name: "Your plan", detail: "Review and get started" },
];
export function stageIndex(state) {
  return Math.max(
    0,
    GUIDE_STAGES.findIndex(
      (s) => s.id === (state?.question?.stage || state?.stage),
    ),
  );
}
export function toggleAnswer(selected, id, multiple) {
  return multiple
    ? selected.includes(id)
      ? selected.filter((value) => value !== id)
      : [...selected, id]
    : selected.includes(id)
      ? []
      : [id];
}
export function canCombineOptions(question) {
  return question?.multiple !== true && question?.stage !== "review" && question?.options?.length >= 2;
}
export function canSubmitAnswer(question, selected, text, combineOptions = false) {
  const allowed = new Set((question?.options || []).map((o) => o.id));
  return (
    (combineOptions === true
      ? canCombineOptions(question) && selected.length >= 2
      : question?.multiple === true || selected.length <= 1) &&
    new Set(selected).size === selected.length &&
    selected.every((id) => allowed.has(id)) &&
    (selected.length > 0 || Boolean(question?.allow_text !== false && text.trim()))
  );
}
export function manualFeedback(before, after) {
  const changes = blueprintDiff(before, after);
  return changes.length
    ? {
        blueprint: clone(after),
        text: `I adjusted these details manually: ${changes.join("; ")}. Keep my choices and check that they fit the setup we are discussing. Explain any changes you recommend before applying them.`,
        changes,
      }
    : null;
}
export function guideMessageEntries(messages, makeEntry) {
  return (messages || [])
    .filter(
      (message) => typeof message.text === "string" && message.text.trim(),
    )
    .map((message, i) => ({
      ...makeEntry(
        message.role === "agent" ? "assistant" : message.role,
        message.text,
      ),
      id: message.id || `guide-message-${i}`,
    }));
}
