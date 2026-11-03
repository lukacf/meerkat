import { canonicalJson } from "./state.mjs";

// The host's existing GuideResponse serializer supplies these ordinary defaults.
// Normalize representation only; never infer admission from similar wording.
function savedRequest(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const result = { action: "answer", selected: [], text: "", stage: null, blueprint: null, ...value };
  if (result.combine_options === false) delete result.combine_options;
  if (Array.isArray(result.attachments) && result.attachments.length === 0) delete result.attachments;
  if (result.manual_intent === null) delete result.manual_intent;
  return result;
}

/** Join a lost Send response to the existing host receipt for these exact bytes. */
export function guideAdmissionMatches(owner, next) {
  const operation = next?.operation;
  return !!owner?.request && next?.id === owner.id &&
    Number.isSafeInteger(next.version) && next.version >= owner.request.version &&
    Number.isSafeInteger(owner.request.version) && operation?.version === owner.request.version &&
    typeof operation.request_id === "string" && /^[a-f0-9]{64}$/.test(operation.request_id) &&
    ["running", "completed", "failed", "interrupted"].includes(operation.status) &&
    canonicalJson(savedRequest(operation.request)) === canonicalJson(savedRequest(owner.request));
}
