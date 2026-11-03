// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the launch-mode functions move byte-verbatim as plain JS, and
// their `options = {}` parameter defaults and heterogeneous option-row
// literals raise TS2339 under .ts semantics. Source-contract pins this
// exact text, so suppression must live at file level, not in the moved
// bodies. Resolution/linkage stays guarded behaviorally: the projection
// suite and export-keys test load the bundle and exercise these functions,
// so a missed import or re-export still fails the gate as a ReferenceError.
//
// Launch-mode control plane for the Flow Editor. Moved verbatim from the
// controller.js launch-modes range: launchModeControlState and its patch
// family, dispatch/collection/dependency policy options,
// launchModesFromFlow, edge-condition normalization, and
// collectVisualSteps.
//
// SCC note: flow/launch-modes.ts and contract/options.ts form a
// runtime-only import cycle (no module-init cross-calls), co-moved in S6
// per the extraction design. childLanes comes from the seeded
// flow/step-tree.ts.
import { contractDefaultValue, forkContextOptions } from "../contract/options";
import { profileName } from "../domain/tool-skill-access";
import { findMember } from "../shared/normalize";
import { launchViewForState, viewStringMapFromSchema } from "../views/view-config";
import { childLanes } from "./step-tree";

export function hasAuthoringLaunchMode(source) {
  return !!source && typeof source === "object" && ("launchMode" in source || "launch_mode" in source);
}

export function launchModeFromAuthoringSource(source, fallback) {
  const raw = hasAuthoringLaunchMode(source)
    ? (source.launchMode ?? source.launch_mode)
    : (hasAuthoringLaunchMode(fallback) ? (fallback.launchMode ?? fallback.launch_mode) : null);
  if (!raw || typeof raw !== "object" || !String(raw.kind || "").trim()) return null;
  return normalizeLaunchMode(raw);
}

export function memberDisplayName(members, id) {
  return ((members || []).find((member) => member.id === id) || {}).name || id;
}

export function normalizeLaunchMode(mode) {
  if (!mode || typeof mode !== "object") return null;
  const kind = canonicalLaunchModeKind(mode.kind);
  if (!kind) return null;
  if (kind === "Resume") {
    return {
      kind: "Resume",
      sessionId: String(mode.sessionId || mode.session_id || mode.bridgeSessionId || mode.bridge_session_id || "").trim(),
    };
  }
  if (kind === "Fork") {
    return {
      kind: "Fork",
      from: String(mode.from || mode.sourceMemberId || mode.source_member_id || "").trim(),
      context: normalizeForkContext(mode.context || mode.forkContext || mode.fork_context),
    };
  }
  return { kind };
}

export function launchModeControlState(source, contract, launchView = null) {
  const view = launchViewForState(launchView);
  const authoredLaunchMode = source && typeof source === "object"
    ? (source.launchMode ?? source.launch_mode)
    : null;
  const defaultLaunchMode = contractDefaultValue(contract, "launch_mode");
  const launchMode = normalizeLaunchMode(authoredLaunchMode) || { kind: defaultLaunchMode };
  const launchKind = canonicalLaunchModeKind(launchMode.kind || defaultLaunchMode);
  const launchOptions = launchModeOptions(contract, launchKind, view);
  const defaultForkContext = contractDefaultValue(contract, "fork_context");
  const forkContextValue = normalizeForkContext(launchMode.context || defaultForkContext);
  const forkOptions = forkContextOptions(contract, forkContextValue, view);
  return {
    launchTitle: view.launchTitle,
    graphLaunchTitle: view.graphLaunchTitle,
    resumeSessionLabel: view.resumeSessionLabel,
    resumeSessionPlaceholder: view.resumeSessionPlaceholder,
    forkSourceLabel: view.forkSourceLabel,
    forkContextLabel: view.forkContextLabel,
    graphForkContextLabel: view.graphForkContextLabel,
    launchMode,
    launchKind,
    defaultLaunchMode,
    launchOptions,
    selectedLaunchMode: launchOptions.find((option) => option.value === launchKind),
    defaultForkContext,
    forkContextValue,
    forkContextOptions: forkOptions,
    selectedForkContext: forkOptions.find((option) => option.value === forkContextValue),
  };
}

export function launchModeKindPatch(source, kind, contract, options = {}) {
  const state = launchModeControlState(source, contract);
  const nextKind = canonicalLaunchModeKind(kind);
  if (!launchModeKindAllowed(contract, nextKind)) return {};
  if (nextKind === "Fork") {
    return {
      launchMode: {
        ...state.launchMode,
        kind: "Fork",
        from: options.firstForkSourceId || state.launchMode.from || "",
        context: state.launchMode.context || state.defaultForkContext,
      },
    };
  }
  if (nextKind === "Resume") {
    return {
      launchMode: {
        ...state.launchMode,
        kind: "Resume",
        sessionId: state.launchMode.sessionId || "",
      },
    };
  }
  return { launchMode: { kind: nextKind } };
}

export function launchModeMergePatch(source, patch, contract) {
  const state = launchModeControlState(source, contract);
  const nextPatch = patch && typeof patch === "object" ? { ...patch } : {};
  if (["budgetSplitPolicy", "budget_split_policy", "budget"].some((key) => key in nextPatch)) return {};
  if ("kind" in nextPatch) {
    const kind = canonicalLaunchModeKind(nextPatch.kind);
    if (!launchModeKindAllowed(contract, kind)) return {};
    nextPatch.kind = kind;
  }
  if ("context" in nextPatch) {
    const context = normalizeForkContext(nextPatch.context);
    if (!forkContextAllowed(contract, context)) return {};
    nextPatch.context = context;
  }
  const launchMode = normalizeLaunchMode({ ...state.launchMode, ...nextPatch });
  return launchMode ? { launchMode } : {};
}

export function launchModeSessionPatch(source, sessionId, contract) {
  return launchModeMergePatch(source, { sessionId: String(sessionId || "") }, contract);
}

export function launchSourceAllowed(sourceOptions, from) {
  const value = String(from || "").trim();
  if (!value) return true;
  return (Array.isArray(sourceOptions) ? sourceOptions : [])
    .some((option) => String(option?.value || option?.id || "").trim() === value);
}

export function launchModeForkSourcePatch(source, from, contract, options = {}) {
  const value = String(from || "").trim();
  if (!launchSourceAllowed(options.sourceOptions, value)) return {};
  return launchModeMergePatch(source, { from: value }, contract);
}

export function launchModeForkContextPatch(source, context, contract) {
  return launchModeMergePatch(source, { context }, contract);
}

export function canonicalLaunchModeKind(value) {
  const raw = String(value || "").trim();
  if (!raw) return "";
  const lower = raw.toLowerCase();
  if (lower === "resume") return "Resume";
  if (lower === "fork") return "Fork";
  if (lower === "fresh") return "Fresh";
  return raw;
}

export function launchModeKindAllowed(contract, kind) {
  const canonicalKind = canonicalLaunchModeKind(kind);
  if (!canonicalKind) return false;
  const contractModes = Array.isArray(contract?.mob_definition?.launch_modes)
    ? contract.mob_definition.launch_modes.map(canonicalLaunchModeKind)
    : [];
  return contractModes.includes(canonicalKind);
}

export function normalizeForkContext(value) {
  const raw = String(value || "").trim();
  if (!raw) return "";
  if (raw === "FullHistory") return "full_history";
  if (raw === "LastMessages") return "last_messages";
  return raw;
}

export function forkContextAllowed(contract, context) {
  const normalized = normalizeForkContext(context);
  if (!normalized) return false;
  const contexts = Array.isArray(contract?.mob_definition?.fork_contexts)
    ? contract.mob_definition.fork_contexts.map(normalizeForkContext)
    : [];
  return contexts.includes(normalized);
}

export function launchOptionLabel(labels, value, view, contractLabel) {
  return labels?.[value] || `${value}${view.unsupportedLabelSeparator}${contractLabel}`;
}

export function launchUnsupportedReason(view, contractLabel) {
  return `${view.unsupportedReasonPrefix}${contractLabel}${view.unsupportedReasonSuffix}`;
}

export function launchModeOptions(contract, currentKind, launchView = null) {
  const view = launchViewForState(launchView);
  const contractModes = Array.isArray(contract?.mob_definition?.launch_modes) && contract.mob_definition.launch_modes.length
    ? contract.mob_definition.launch_modes.map(canonicalLaunchModeKind)
    : [];
  const modes = [...contractModes];
  const currentSource = currentKind || contractDefaultValue(contract, "launch_mode");
  const current = currentSource ? canonicalLaunchModeKind(currentSource) : "";
  if (current && !modes.includes(current)) modes.push(current);
  return modes.map((mode) => {
    const supported = contractModes.includes(mode);
    return {
      value: mode,
      label: launchOptionLabel(view.launchModeLabels, mode, view, view.launchModesContractLabel),
      disabled: !supported,
      reason: supported ? "" : launchUnsupportedReason(view, view.launchModesContractLabel),
    };
  });
}

export function normalizeDispatchMode(mode) {
  return String(mode || "").trim();
}

export function dispatchModeOptions(contract, currentMode) {
  const contractLabel = "dispatch_modes";
  const contractModes = Array.isArray(contract?.mob_definition?.dispatch_modes) && contract.mob_definition.dispatch_modes.length
    ? contract.mob_definition.dispatch_modes.map(String)
    : [];
  const modes = [...contractModes];
  const current = String(currentMode || contractDefaultValue(contract, "dispatch_mode") || "").trim();
  if (!modes.includes(current)) modes.push(current);
  const labels = viewStringMapFromSchema(contract?.mob_definition?.dispatch_mode_labels);
  return modes.map((mode) => {
    const supported = contractModes.includes(mode);
    return {
      value: mode,
      label: labels[mode] || mobDefinitionUnsupportedOptionLabel(contract, mode, contractLabel),
      disabled: !supported,
      reason: supported ? "" : mobDefinitionUnsupportedOptionReason(contract, contractLabel),
    };
  });
}

export function dispatchModeAllowed(contract, mode) {
  const value = String(mode || "").trim();
  if (!value) return true;
  const contractModes = Array.isArray(contract?.mob_definition?.dispatch_modes)
    ? contract.mob_definition.dispatch_modes.map(String)
    : [];
  return contractModes.includes(value);
}

export function normalizeCollectionMode(policy) {
  const raw = typeof policy === "object" && policy
    ? String(policy.type || "").trim()
    : String(policy || "").trim();
  return raw;
}

export function dependencyModeOptions(contract, currentMode) {
  const contractLabel = "dependency_modes";
  const contractModes = Array.isArray(contract?.mob_definition?.dependency_modes) && contract.mob_definition.dependency_modes.length
    ? contract.mob_definition.dependency_modes.map(String)
    : [];
  const modes = [...contractModes];
  const current = String(currentMode || contractDefaultValue(contract, "dependency_mode") || "").trim();
  if (!modes.includes(current)) modes.push(current);
  const labels = viewStringMapFromSchema(contract?.mob_definition?.dependency_mode_labels);
  return modes.map((mode) => {
    const supported = contractModes.includes(mode);
    return {
      value: mode,
      label: labels[mode] || mobDefinitionUnsupportedOptionLabel(contract, mode, contractLabel),
      disabled: !supported,
      reason: supported ? "" : mobDefinitionUnsupportedOptionReason(contract, contractLabel),
    };
  });
}

export function dependencyModeAllowed(contract, mode) {
  const value = String(mode || "").trim();
  if (!value) return true;
  const contractModes = Array.isArray(contract?.mob_definition?.dependency_modes)
    ? contract.mob_definition.dependency_modes.map(String)
    : [];
  return contractModes.includes(value);
}

export function collectionPolicyOptions(contract, currentPolicy) {
  const contractLabel = "collection_policies";
  const contractPolicies = Array.isArray(contract?.mob_definition?.collection_policies) && contract.mob_definition.collection_policies.length
    ? contract.mob_definition.collection_policies.map(String)
    : [];
  const policies = [...contractPolicies];
  const current = String(currentPolicy || contractDefaultValue(contract, "collection_policy") || "").trim();
  if (!policies.includes(current)) policies.push(current);
  const labels = viewStringMapFromSchema(contract?.mob_definition?.collection_policy_labels);
  return policies.map((policy) => {
    const supported = contractPolicies.includes(policy);
    return {
      value: policy,
      label: labels[policy] || mobDefinitionUnsupportedOptionLabel(contract, policy, contractLabel),
      disabled: !supported,
      reason: supported ? "" : mobDefinitionUnsupportedOptionReason(contract, contractLabel),
    };
  });
}

export function mobDefinitionUnsupportedOptionLabel(contract, value, contractLabel) {
  const separator = String(contract?.mob_definition?.option_unsupported_label_separator || " ");
  return `${value}${separator}${contractLabel}`;
}

export function mobDefinitionUnsupportedOptionReason(contract, contractLabel) {
  const prefix = String(contract?.mob_definition?.option_unsupported_reason_prefix || "");
  const suffix = String(contract?.mob_definition?.option_unsupported_reason_suffix || "");
  return `${prefix}${contractLabel}${suffix}`;
}

export function collectionPolicyAllowed(contract, policy) {
  const value = String(policy || "").trim();
  if (!value) return true;
  const contractPolicies = Array.isArray(contract?.mob_definition?.collection_policies)
    ? contract.mob_definition.collection_policies.map(String)
    : [];
  return contractPolicies.includes(value);
}

export function launchModesFromFlow(flow, members) {
  const out = [];
  collectVisualSteps(flow?.steps || [], (step) => {
    if (step.type !== "member") return;
    const member = findMember(members, step.role);
    const launchMode = launchModeFromAuthoringSource(step);
    const row = {
      step_id: step.id,
      member_id: step.role || "",
      profile: profileName(member || { id: step.role }),
      launch_mode: launchMode,
    };
    out.push(row);
  });
  return out;
}

export function conditionTextFromEdge(edge, fallback) {
  if (!edge) return fallback;
  const condition = normalizedEdgeCondition(edge);
  if (condition?.path) {
    if (condition.val === undefined || condition.val === null || String(condition.val).trim() === "") return fallback;
    const value = condition.val;
    if (!condition.op) return fallback;
    return `${condition.path} ${condition.op} ${JSON.stringify(value)}`;
  }
  return edge.label || fallback;
}

export function edgeConditionToEditorCond(edge) {
  const condition = normalizedEdgeCondition(edge);
  const rawVar = String(condition?.path || "").trim();
  const op = String(condition?.op || "").trim();
  const val = condition?.val === undefined || condition?.val === null ? "" : String(condition.val);
  if (!rawVar || !op || !val) return null;
  const parts = rawVar.split(".").filter(Boolean);
  if (parts.length === 2 && parts[0] === "params") {
    return {
      namespace: "params",
      stepId: "params",
      field: parts[1],
      op,
      val,
    };
  }
  if (parts.length === 3 && parts[0] === "steps") {
    return {
      namespace: "steps",
      stepId: parts[1],
      field: parts[2],
      op,
      val,
    };
  }
  return null;
}

export function repeatConditionFromEdge(edge, stepId) {
  const condition = normalizedEdgeCondition(edge);
  if (condition?.path) {
    const parts = String(condition.path).split(".").filter(Boolean);
    const field = parts.pop() || "";
    const op = String(condition.op || "").trim();
    const val = condition.val === undefined || condition.val === null ? "" : String(condition.val).trim();
    if (!field || !op || !val) return null;
    return { stepId, field, op, val };
  }
  const label = String(edge?.label || "");
  const match = /([A-Za-z0-9_.-]+)\s*(==|>|<)\s*['"]?([^'"]+)['"]?/.exec(label);
  if (match) {
    return { stepId, field: match[1].split(".").pop(), op: match[2], val: match[3] };
  }
  return null;
}

export function normalizedEdgeCondition(edge) {
  const cond = edge?.cond || {};
  const op = String(cond.op || cond.operator || "").trim();
  const val = cond.val ?? cond.value;
  if (cond.var || cond.path || cond.source) {
    return {
      path: String(cond.var || cond.path || cond.source || "").trim(),
      op,
      val,
    };
  }
  const namespace = String(cond.namespace || "").trim();
  const stepId = String(cond.stepId || cond.step_id || "").trim();
  const field = String(cond.field || "").trim();
  if (field && (namespace === "params" || stepId === "params")) {
    return { path: `params.${field}`, op, val };
  }
  if (field && stepId) {
    return { path: `steps.${stepId}.${field}`, op, val };
  }
  return null;
}

export function collectVisualSteps(steps, visit) {
  for (const step of steps || []) {
    visit(step);
    for (const lane of childLanes(step)) collectVisualSteps(lane.steps, visit);
  }
}
