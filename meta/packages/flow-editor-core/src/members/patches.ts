// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the member-patch functions move byte-verbatim as plain JS, and
// memberSchemaCascadePatch's destructured `= {}` parameter default raises
// TS2339 under .ts semantics. Source-contract pins this exact text, so
// suppression must live at file level, not in the moved bodies.
// Resolution/linkage stays guarded behaviorally: the projection suite and
// export-keys test load the bundle and exercise these functions, so a
// missed import or re-export still fails the gate as a ReferenceError.
//
// Member patch semantics for the Flow Editor controller plane. Moved
// verbatim from the controller.js member-patches range: memberPromptSkeleton,
// the member field patches (name/realm-profile/system-prompt/profile-binding/
// runtime-mode/model/schema/backend/inline-peer-notification-limit),
// memberSchemaCascadePatch, and the provider-params editor state/patch pair.
// The generic normalizers that closed the original cluster
// (normalizeProfileBackend, normalizeMaxInlinePeerNotifications,
// normalizePositiveInteger, normalizeStringList, normalizeOutputFormat,
// normalizeProviderParams) moved to shared/normalize.ts in S1.
//
// SCC note: flow/reconcile.ts is the co-moved S8 partner — the imports
// below close the reconcile<->member-patches cycle, which is runtime-only
// (no module-init cross-calls) and safe under ESM.
import {
  profileBackendOptions,
  profileBindingOptions,
  runtimeModeOptions,
} from "../contract/options";
import {
  catalogValueAllowed,
  optionValueAllowed,
  reconcileConditionFieldAvailability,
} from "../flow/reconcile";
import {
  normalizeMaxInlinePeerNotifications,
  normalizeProviderParams,
} from "../shared/normalize";
import { agentDetailViewForState } from "../views/view-config";

export function memberPromptSkeleton(member) {
  const notes = String(member?.systemPrompt || "").trim();
  const name = member?.name || "this agent";
  const intent = notes || `Act as the ${member?.role || "member"} of the mob.`;
  const lines = [
    `You are ${name}, a member of a Meerkat mob.`,
    "",
    "## Mandate",
    intent.replace(/\s+/g, " "),
    "",
    "## Operating rules",
    "- Read the shared mob workpad and prior members' output before acting.",
    "- Do exactly what this step requires — no more, no less.",
    member?.schema ? `- Emit a ${member.schema} as your structured output.` : "- Return a concise, well-structured result.",
    "- Hand off cleanly: state what you did and what the next member needs.",
  ];
  return lines.join("\n");
}

export function memberNamePatch(rawName) {
  return { name: String(rawName || "") };
}

export function memberRealmProfilePatch(rawProfile) {
  return { realmProfile: String(rawProfile || "").trim() };
}

export function memberSystemPromptPatch(rawPrompt) {
  return { systemPrompt: String(rawPrompt || "") };
}

export function memberProfileBindingPatch(member, rawBinding, contract) {
  const binding = String(rawBinding || "").trim();
  if (!optionValueAllowed(profileBindingOptions(contract, binding), binding)) return {};
  return {
    profileBinding: binding,
    realmProfile: binding === "realm_profile"
      ? String(member?.realmProfile || member?.role || member?.name || "")
      : "",
  };
}

export function memberRuntimeModePatch(rawMode, contract, deploySettings) {
  const runtimeMode = String(rawMode || "").trim();
  if (!optionValueAllowed(runtimeModeOptions(contract, deploySettings, runtimeMode), runtimeMode)) return {};
  return { runtimeMode };
}

export function memberModelPatch(rawModel, modelCatalog) {
  const model = String(rawModel || "").trim();
  const ids = (modelCatalog || []).map((entry) => String(entry?.id || "").trim()).filter(Boolean);
  if (!catalogValueAllowed(ids, model, { allowBlank: false })) return {};
  return { model };
}

export function memberSchemaPatch(rawSchema, schemas) {
  const schema = String(rawSchema || "").trim();
  if (Array.isArray(schemas)) {
    const ids = schemas.map((entry) => String(entry?.id || "").trim()).filter(Boolean);
    if (schema && !ids.includes(schema)) return {};
  }
  return { schema };
}

export function memberSchemaCascadePatch({ memberId, members, flow, edges, instances, schemas } = {}, rawSchema) {
  const id = String(memberId || "").trim();
  const list = Array.isArray(members) ? members : [];
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const current = list.find((member) => String(member?.id || "").trim() === id) || null;
  if (!current) {
    return { ok: false, error: "member not found", members: list, flow, edges, instances: sourceInstances, patch: null };
  }
  const patch = memberSchemaPatch(rawSchema, schemas);
  if (!Object.prototype.hasOwnProperty.call(patch, "schema")) {
    return { ok: false, error: "unknown schema", members: list, flow, edges, instances: sourceInstances, patch: null };
  }
  const nextMember = { ...current, ...patch };
  const nextMembers = list.map((member) => String(member?.id || "").trim() === id ? nextMember : member);
  const reconciled = reconcileConditionFieldAvailability({
    flow,
    edges,
    members: nextMembers,
    instances: sourceInstances,
    schemas,
  });
  return {
    ok: true,
    error: "",
    patch,
    member: nextMember,
    members: nextMembers,
    flow: reconciled.flow,
    edges: reconciled.edges,
    instances: sourceInstances,
  };
}

export function memberBackendPatch(rawBackend, contract) {
  const backend = String(rawBackend || "").trim();
  if (!optionValueAllowed(profileBackendOptions(contract, backend, true), backend, { allowBlank: true })) return {};
  return { backend };
}

export function memberMaxInlinePeerNotificationsPatch(rawValue) {
  return { maxInlinePeerNotifications: normalizeMaxInlinePeerNotifications(rawValue) };
}

export function memberProviderParamsEditorState(member, agentDetailView = null) {
  const view = agentDetailViewForState(agentDetailView);
  return {
    label: view.providerParamsLabel,
    text: member?.providerParams ? JSON.stringify(member.providerParams, null, 2) : "",
    placeholder: view.providerParamsPlaceholder,
    rows: view.providerParamsRows,
    invalidJsonLabel: view.providerParamsInvalidJsonLabel,
  };
}

export function memberProviderParamsPatch(rawText, agentDetailView = null) {
  const view = agentDetailViewForState(agentDetailView);
  const text = String(rawText || "").trim();
  if (!text) return { ok: true, patch: { providerParams: null }, error: "" };
  try {
    const parsed = JSON.parse(text);
    const normalized = normalizeProviderParams(parsed);
    if (!normalized) {
      return { ok: false, patch: null, error: view.providerParamsObjectRequiredError };
    }
    return { ok: true, patch: { providerParams: normalized }, error: "" };
  } catch (err) {
    return { ok: false, patch: null, error: err?.message || view.providerParamsInvalidJsonLabel };
  }
}
