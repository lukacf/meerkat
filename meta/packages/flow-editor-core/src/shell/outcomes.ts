// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the shell-outcomes functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (topRailState,
// criticalErrorOutcome) plus `options = {}` defaults raise TS2339 under .ts
// semantics. Source-contract pins this exact text, so suppression must live
// at file level, not in the moved bodies. Resolution/linkage stays guarded
// behaviorally: the projection suite and export-keys test load the bundle
// and exercise these functions, so a missed import or re-export still fails
// the gate as a ReferenceError.
//
// Shell outcome/transition projections for the Flow Editor controller
// plane. Moved verbatim from the controller.js shell-outcomes range: API
// display rows, validation-sheet and deploy-plan-trace state, top-rail
// state and navigation/editor-mode/theme transitions, validate/export/
// deploy outcomes, overlay open/close/clear transitions, and the
// error-outcome family (criticalErrorOutcome and its five wrappers).
// diagnosticsToRows/deployResultToRows re-homed here from the drafts
// range per the extraction design, killing the drafts<->shell cycle.
import { errorViewForState } from "../schema/field-edit";
import { deployViewForState } from "../views/view-config";

export function diagnosticsToRows(validation) {
  if (Array.isArray(validation?.display_rows)) {
    return apiDisplayRows(validation.display_rows);
  }
  return [];
}

export function deployResultToRows(result) {
  if (Array.isArray(result?.display_rows)) {
    return apiDisplayRows(result.display_rows);
  }
  return [];
}

export function apiDisplayRows(rows) {
  return (Array.isArray(rows) ? rows : [])
    .filter((row) => row && typeof row === "object")
    .map((row) => ({
      kind: String(row.kind || ""),
      glyph: String(row.glyph || ""),
      head: String(row.head || ""),
      sub: String(row.sub || ""),
      meta: String(row.meta || ""),
    }));
}

export function validationSheetState(results, options = {}) {
  const view = deployViewForState(options.deployView);
  const rows = Array.isArray(results) ? results : [];
  const counts = rows.reduce((acc, row) => {
    const kind = row?.kind || "warn";
    if (kind === "ok") acc.ok += 1;
    else if (kind === "crit") acc.crit += 1;
    else acc.warn += 1;
    return acc;
  }, { ok: 0, warn: 0, crit: 0 });
  const stage = String(options.stage || "").trim();
  const stageBlocksActions = !!stage && stage !== "valid";
  const actionsDisabled = counts.crit > 0 || stageBlocksActions;
  const deployExecuteAllowed = options.capabilities?.authoring_capabilities?.deploy_execute_allowed !== false;
  return {
    rows,
    counts,
    eyebrow: view.validationEyebrow,
    title: `${counts.ok} ${view.validationPassedLabel} · ${counts.warn} ${view.validationWarningsLabel} · ${counts.crit} ${view.validationBlockingLabel}`,
    publishLabel: view.publishLabel,
    deployPlanLabel: view.deployPlanLabel,
    deployLabel: view.deployLabel,
    closeLabel: view.closeLabel,
    actionsDisabled,
    publishDisabled: actionsDisabled,
    deployPlanDisabled: actionsDisabled,
    deployRunDisabled: actionsDisabled || !deployExecuteAllowed,
  };
}

export function deployPlanTraceState(document, plan, options = {}) {
  const view = deployViewForState(options.deployView);
  const steps = Array.isArray(plan?.plan_trace) && plan.plan_trace.length
    ? plan.plan_trace
    : [{
      node: null,
      head: view.planUnavailableHead,
      body: view.planUnavailableBody,
    }];
  const title = document?.mob_id || document?.name || "meta_flow";
  const subtitle = plan?.command || "";
  const packLabel = plan?.pack_path || "";
  return {
    steps,
    eyebrow: view.planEyebrow,
    title,
    subtitle,
    packLabel,
    firstLabel: view.planFirstLabel,
    closeLabel: view.closeLabel,
    stepLabel: view.planStepLabel,
    previousLabel: view.planPreviousLabel,
    nextLabel: view.planNextLabel,
  };
}

export function topRailState({ contract, deploySettings, stage, view, mobOpen, theme, deployView, capabilities } = {}) {
  const shell = deployViewForState(deployView);
  const contractState = contract?.error ? shell.apiErrorLabel : contract ? shell.apiReadyLabel : shell.apiLoadingLabel;
  const deployCommand = contract?.deploy_settings?.command || "";
  const deploySurface = deploySettings?.surface || contract?.deploy_settings?.surfaces?.[0] || "";
  const deployActionsDisabled = stage !== "valid";
  const deployExecuteAllowed = capabilities?.authoring_capabilities?.deploy_execute_allowed !== false;
  const nextTheme = theme === "dark" ? "light" : "dark";
  const open = !!mobOpen;
  return {
    mobOpen: open,
    sectionTabs: open
      ? [
        { target: "flow-tab", view: "editor", label: shell.flowTabLabel, current: view === "editor" },
        { target: "agents-tab", view: "agents", label: shell.agentsTabLabel, current: view === "agents" },
        { target: "settings-tab", view: "settings", label: shell.settingsTabLabel, current: view === "settings" },
      ]
      : [],
    brandLabel: shell.brandLabel,
    mobStatusTitle: shell.mobStatusTitle,
    mobFileLabel: shell.mobFileLabel,
    contractState,
    deployPrefixLabel: shell.deployPrefixLabel,
    deployCommand,
    deploySurface,
    flowsCrumbLabel: shell.flowsCrumbLabel,
    crumbSeparator: shell.crumbSeparator,
    planTraceLabel: shell.planTraceLabel,
    importLabel: shell.importLabel,
    validateLabel: shell.validateLabel,
    publishLabel: shell.publishLabel,
    deployPlanLabel: shell.deployPlanLabel,
    deployLabel: shell.deployLabel,
    overflowLabel: shell.overflowLabel,
    deployActionsDisabled,
    deployRunDisabled: deployActionsDisabled || !deployExecuteAllowed,
    themeToggleTitle: `${shell.themeSwitchPrefix} ${nextTheme} ${shell.themeSwitchSuffix}`,
    themeToggleLabel: nextTheme === "light" ? shell.darkThemeLabel : shell.lightThemeLabel,
    basicModeTitle: shell.basicModeTitle,
    basicModeLabel: shell.basicModeLabel,
    graphModeTitle: shell.graphModeTitle,
    graphModeLabel: shell.graphModeLabel,
  };
}

// Real navigation state machine over {library, editor (FLOW), agents,
// settings}. The library is home; the three section views are only
// reachable while a mob is open (context.mobOpen). No toggling: each
// target names its destination.
export function topRailNavigationTransition(currentView, target, context = {}) {
  const view = String(currentView || "library");
  const mobOpen = !!context.mobOpen;
  switch (String(target || "")) {
    case "flow-tab":
      return mobOpen ? { view: "editor" } : null;
    case "agents-tab":
      return mobOpen ? { view: "agents" } : null;
    case "settings-tab":
      return mobOpen ? { view: "settings" } : null;
    case "library":
      return view === "library" ? null : { view: "library" };
    default:
      return null;
  }
}

// Deep-link boot intent over location.search, parsed exactly once by the
// shell at module scope. ?open=<registry-row-id> hydrates that row into the
// FLOW section, ?intent=new opens the library's NEW MOB modal (with an
// optional ?template=<id> preselection), anything else is the library
// itself. ?embedded=1 marks host-owned chrome on any intent.
export function bootIntentFromQuery(search) {
  const params = new URLSearchParams(typeof search === "string" ? search : "");
  const embedded = params.get("embedded") === "1";
  const openId = String(params.get("open") || "").trim();
  if (openId) return { kind: "open", id: openId, embedded };
  if (String(params.get("intent") || "").trim() === "new") {
    return { kind: "new", template: String(params.get("template") || "").trim(), embedded };
  }
  return { kind: "library", embedded };
}

// Host integration event payloads: the shell dispatches one window
// CustomEvent plus one parent postMessage per successful create / registry
// save / execute deploy, both carrying exactly this payload. The type
// strings are wire protocol for embedding hosts (documented in
// docs/guides/flow-editor.mdx), not user-facing copy.
export function hostEventPayload(kind, { id, stage, ok } = {}) {
  const eventKind = String(kind || "");
  if (eventKind !== "created" && eventKind !== "saved" && eventKind !== "deployed") return null;
  return {
    type: `meta-flow-editor:${eventKind}`,
    id: String(id || ""),
    stage: String(stage || ""),
    ok: ok === true,
  };
}

export function editorModeTransition(target) {
  const editorMode = String(target || "");
  if (editorMode !== "basic" && editorMode !== "advanced") return null;
  return { editorMode };
}

export function themeToggleTransition(currentTheme) {
  return {
    field: "theme",
    value: currentTheme === "dark" ? "light" : "dark",
  };
}

export function validationOutcome(document, result) {
  const validation = result || null;
  return {
    document,
    validation,
    validationRows: diagnosticsToRows(validation),
    stage: validation?.ok ? "valid" : "draft",
  };
}

export function exportOutcome(document, result, options = {}) {
  const validation = result?.validation || null;
  if (validation?.ok) {
    requireExportArchiveMetadata(result);
  }
  const publishedStage = options.publishedStage || "published";
  return {
    document,
    exportResult: result || null,
    validation,
    validationRows: diagnosticsToRows(validation),
    stage: validation?.ok ? publishedStage : "draft",
  };
}

export function requireExportArchiveMetadata(result) {
  if (!String(result?.content_base64 || "").trim()) {
    throw new Error("meta/mobpacks/export did not return content_base64");
  }
  if (!String(result?.media_type || "").trim()) {
    throw new Error("meta/mobpacks/export did not return media_type");
  }
  if (!String(result?.filename || "").trim()) {
    throw new Error("meta/mobpacks/export did not return filename");
  }
}

export function deployOutcome(document, result, options = {}) {
  const validation = result?.validation || null;
  const executing = options.execute === true;
  const deployOk =
    executing &&
    result?.executed === true &&
    result?.success === true &&
    result?.status_code === 0;
  return {
    document,
    deployResult: result || null,
    validation,
    validationRows: deployResultToRows(result),
    stage: validation?.ok && deployOk ? "deployed" : "draft",
  };
}

export function validationSheetOpenTransition() {
  return { validate: true };
}

export function validationSheetCloseTransition() {
  return { validate: false };
}

export function deployPlanTraceReadyTransition(document, plan) {
  return {
    deployPlanOpen: true,
    deployPlanDocument: document || null,
    deployPlanResult: plan || null,
    incrementDeployPlanKey: true,
  };
}

export function deployPlanTraceCloseTransition() {
  return { deployPlanOpen: false };
}

export function apiOverlayClearTransition() {
  return {
    deployPlanOpen: false,
    validate: false,
  };
}

export function errorMessage(error) {
  return error?.message || String(error || "");
}

export function criticalErrorOutcome({ head, error, meta, errorView } = {}) {
  const view = errorViewForState(errorView);
  return {
    validationRows: [{
      kind: "crit",
      glyph: view.criticalGlyph,
      head: String(head || view.genericErrorHead),
      sub: errorMessage(error),
      meta: String(meta || ""),
    }],
    stage: "draft",
  };
}

export function deployErrorOutcome(error, options = {}) {
  const view = errorViewForState(options.errorView);
  return criticalErrorOutcome({
    head: options.execute ? view.deployFailedHead : view.deployPlanFailedHead,
    error,
    meta: view.deployErrorMeta,
    errorView: view,
  });
}

export function sourceErrorOutcome(error, options = {}) {
  const view = errorViewForState(options.errorView);
  return criticalErrorOutcome({
    head: view.sourceFailedHead,
    error,
    meta: view.sourceErrorMeta,
    errorView: view,
  });
}

export function validationErrorOutcome(error, options = {}) {
  const view = errorViewForState(options.errorView);
  return criticalErrorOutcome({
    head: view.validationApiFailedHead,
    error,
    meta: view.rpcErrorMeta,
    errorView: view,
  });
}

export function exportErrorOutcome(error, options = {}) {
  const view = errorViewForState(options.errorView);
  return criticalErrorOutcome({
    head: view.exportFailedHead,
    error,
    meta: view.rpcErrorMeta,
    errorView: view,
  });
}

export function importErrorOutcome(error, options = {}) {
  const view = errorViewForState(options.errorView);
  return criticalErrorOutcome({
    head: view.importFailedHead,
    error,
    meta: options.filename || "",
    errorView: view,
  });
}

// The standard failure surface for a ?open=<id> boot intent whose row is
// not in the registry: the library stays up and the validation sheet
// carries this row.
export function bootIntentOpenFailureOutcome(intent, options = {}) {
  const view = errorViewForState(options.errorView);
  return criticalErrorOutcome({
    head: view.bootOpenMissingHead,
    error: String(intent?.id || ""),
    meta: view.bootOpenMissingMeta,
    errorView: view,
  });
}
