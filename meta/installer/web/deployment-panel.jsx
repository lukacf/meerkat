import { artifactIsCurrent } from "./artifact-current.mjs";
import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from "./guide-selection.mjs";
import React, { useEffect, useRef, useState } from "react";
import { DeploymentActivity } from "./deployment-activity.jsx";
import { request, requestBlob } from "./state.mjs";
import { readDeploymentSetup } from "./deployment-setup-read.mjs";
import { packagePreparationReason } from "./package-availability.mjs";
import { deploymentRequiredFields, deploymentFieldId, deploymentRemovableFields, deploymentFieldIssues } from "./deployment-configuration.mjs";
import { deploymentCanPrepare, deploymentCanFreshInstall, deploymentFreshInstallRestriction, deploymentCanSubmit, deploymentConsole, deploymentIsActive,
  deploymentPhase, deploymentDiagnostic, deploymentUpdateDiagnostics, deploymentFailedAttempts, deploymentSettings, deploymentUrl, deploymentExecutionModes, deploymentHandoff,
  deploymentDestinationLabel, deploymentOptionsCurrent, deploymentFreshInstallNeedsSetup, deploymentRequiresApplicationStartup, deploymentCanStageUpdate, deploymentUpdateAction, deploymentUpdateOwnerStart,
  deploymentUpdatePreparation, deploymentBackgroundUpdate,
  deploymentExperienceKey, deploymentExperiences, createDeploymentPollingGate,
  deploymentCanConfigure, deploymentCanConfirmCleanup, deploymentCanStopFailedBackground, deploymentNeedsCleanup, deploymentResumeAction, deploymentApplicationStartupPending, deploymentConfigurationForm, deploymentConfigurationHints,
  deploymentReconfiguration, deploymentSettingsReadKey, deploymentSettingsReadCurrent,
  deploymentSettingsResponseCurrent, readDeploymentSettings, deploymentConfigurationRevealCurrent, deploymentPortObservation,
  deploymentSourceKind, deploymentInstalledSourceKind, deploymentSourceKey, deploymentInstallSelection,
  deploymentActionIntent, loadDeploymentActionIntent, storeDeploymentActionIntent, clearDeploymentActionIntent,
  deploymentActionObservationUrl, deploymentActionRequestSettled, deploymentProtectedFolderNote } from "./deployment-panel.mjs";
import { InstalledApplicationUseMount } from "./deployment-use.jsx";
import { ApplicationSetup } from "./application-setup.jsx";
import { InstalledPackageDownload } from "./installed-package.jsx";
import { RemoteDeployment } from "./remote-deployment.jsx";
import { GcpGuided } from "./gcp-guided.jsx";
import { loadDeploymentSupportIntent, storeDeploymentSupportIntent, clearDeploymentSupportIntent,
  deploymentSupportIntent, deploymentSupportObservationUrl, deploymentSupportRequestSettled,
  deploymentSupportState, deploymentSupportAction, deploymentSupportBodyFromIntent } from "./deployment-support.mjs";
import { DeploymentSupportPanel } from "./deployment-support.jsx";
import "./deployment-panel.css";

const COMMAND_STORAGE_ISSUE = "The saved installation command could not be checked because browser storage is unavailable or unreadable. Installation details and drafts remain visible; commands stay paused and Forge will not resend them.";

// Match spelling only for the held-response affordance. The server's
// owned_destination check still owns admission, permissions and symlink checks.
function canonicalInstallDestination(value, homeDirectory) {
  let path = typeof value === "string" ? value.trim() : "";
  if (path === "~" || path.startsWith("~/")) {
    if (typeof homeDirectory !== "string" || !homeDirectory.startsWith("/") || /[\x00-\x1f]/.test(homeDirectory)) return null;
    path = homeDirectory + path.slice(1);
  }
  if (!path.startsWith("/") || path.startsWith("//") || /[\x00-\x1f]/.test(path)) return null;
  const parts = [];
  for (const part of path.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return "/" + parts.join("/");
}

export function DeploymentPanel({ state, plan, build, designBusy, pendingManual, Icon, onReceipt, onRemoteReceipt, remoteTargetRef, onRepairReceipt, onChooseCloudProject,
  initialSourceKind = "application", sourceNavigation = null, selectionScope, agentTeamAvailable = true }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const optionsRequest = useRef(0);
  const [chosenSourceKind, setSourceKind] = useState(initialSourceKind);
  // A Forge that offers no agent team package installs applications only.
  const sourceKind = agentTeamAvailable ? chosenSourceKind : "application";
  const sourceChoice = useRef({ guideId: state.id, explicit: false });
  const sourceKey = deploymentSourceKey(build, state, sourceKind);
  const nativeTeam = sourceKind === "native-team";
  const [snapshot, setSnapshot] = useState({ guideId: null, selectionToken: null, receipt: null, observation: "loading" });
  const [cloudRemoval, setCloudRemoval] = useState(null);
  const [options, setOptions] = useState(null), [destination, setDestination] = useState("");
  const [values, setValues] = useState({}), [useHost, setUseHost] = useState(false);
  const [executionMode, setExecutionMode] = useState("interactive");
  const [formOpen, setFormOpen] = useState(false), [busy, setBusy] = useState(false);
  const busyOperation = useRef(null);
  const [commandReturn, setCommandReturn] = useState(null), commandReturnRef = useRef(null);
  const [formIntent, setFormIntent] = useState("new"), [formOptions, setFormOptions] = useState(null);
  const [removeFields, setRemoveFields] = useState([]), [formReadKey, setFormReadKey] = useState("");
  const configurationRef = useRef(null);
  const [error, setErrorValue] = useState(""), [loadingOptions, setLoadingOptions] = useState(false);
  const commandObservationError = useRef(null);
  function setError(message) {
    // A later form or command error has its own lifetime, even with the same text.
    commandObservationError.current = null;
    setErrorValue(message);
  }
  const [portCheck, setPortCheck] = useState({ key: "", options: null }), [checkingPort, setCheckingPort] = useState(false);
  const portEpoch = useRef(0), portForm = useRef(null);
  const [connection, setConnection] = useState({ guideId: null, confirmed: false, unavailable: false });
  const [supportTracking, setSupportTracking] = useState({ guideId: null, confirmed: false,
    storageAvailable: true, ordinaryClear: false, intent: null, observation: null });
  const [downloadRequests, setDownloadRequests] = useState({});
  const downloading = !!downloadRequests[state.id];
  const downloadPending = useRef(new Map());
  const [experienceSnapshot, setExperienceSnapshot] = useState({ key: "", value: null, issue: null, pending: false });
  const [experienceRetry, setExperienceRetry] = useState(0);
  const latest = useRef({ state, build }), timer = useRef(null), generation = useRef(0);
  const polling = useRef(null);
  if (!polling.current) polling.current = createDeploymentPollingGate();
  const receipt = snapshot.guideId === state.id && snapshot.receipt?.guide_id === state.id ? snapshot.receipt : null;
  const observation = snapshot.guideId === state.id && snapshot.selectionToken === selectionToken && !selectionBlocked
    ? snapshot.observation : "loading";
  const statusUnknown = connection.guideId !== state.id || connection.confirmed !== true;
  const mutationBlocked = selectionBlocked || statusUnknown;
  latest.current = { state, build, receipt, sourceKind, sourceKey, formOpen, formIntent, statusUnknown,
    blocked: !!(designBusy || pendingManual), supportTracking };
  const supportConfirmed = supportTracking.guideId === state.id && supportTracking.confirmed &&
    supportTracking.storageAvailable && supportTracking.ordinaryClear;
  const supportView = deploymentSupportState(receipt, state, { confirmed: supportConfirmed,
    storageAvailable: supportTracking.storageAvailable,
    intent: supportTracking.guideId === state.id ? supportTracking.intent : null,
    observation: supportTracking.guideId === state.id ? supportTracking.observation : null });
  const reconnecting = connection.guideId === state.id && connection.unavailable;
  const active = deploymentIsActive(receipt), canPrepare = deploymentCanPrepare(build, state, sourceKind);
  const packageReason = packagePreparationReason(state);
  const selectedOptions = deploymentSourceKind(options) === sourceKind ? options : null;
  const canInstall = deploymentCanFreshInstall(build, state, selectedOptions), handoff = deploymentHandoff(selectedOptions, build, state);
  const freshInstallRestriction = deploymentFreshInstallRestriction(build, state, selectedOptions);
  const consoleScope = deploymentOptionsCurrent(selectedOptions, build, state) ? selectedOptions.console_scope : null;
  const runtimeScope = receipt?.console_scope || (!receipt || deploymentSourceKind(receipt) === sourceKind &&
    (nativeTeam ? receipt.source_id === options?.source_id : receipt.build_id === options?.build_id) ? consoleScope : null);
  const consoleUrl = deploymentConsole(receipt);
  const experienceKey = deploymentExperienceKey(receipt, state.id);
  const experienceLoading = !!experienceKey && (experienceSnapshot.key !== experienceKey || experienceSnapshot.pending);
  latest.current.experienceKey = experienceKey;
  latest.current.experiencePending = experienceLoading;
  const experiences = deploymentExperiences(experienceSnapshot.key === experienceKey ? experienceSnapshot.value : null, receipt, state.id);
  const experienceIssue = experiences.issue || (experienceSnapshot.key === experienceKey ? experienceSnapshot.issue : null);
  const canStageUpdate = deploymentCanStageUpdate(receipt, build, state, options);
  const canConfigure = deploymentCanConfigure(receipt, build, state);
  const privateFormOptions = deploymentConfigurationHints(formOptions, receipt, destination, formIntent === "settings");
  const formTargetCurrent = !active && !loadingOptions && !latest.current.blocked && (formIntent === "settings"
    ? deploymentCanConfigure(receipt, build, state, formOptions) &&
      formReadKey === deploymentSettingsReadKey(latest.current) && destination === receipt?.destination &&
      executionMode === (receipt?.execution_mode || "interactive")
    : canInstall && deploymentSourceKind(formOptions) === sourceKind && deploymentCanFreshInstall(build, state, formOptions));
  const settingsBody = formIntent === "settings" && formTargetCurrent
    ? deploymentReconfiguration(privateFormOptions, receipt, build, state, values, removeFields, useHost) : null;
  const configurationKey = formOpen ? JSON.stringify([state.id, state.version, sourceKey,
    formIntent, formReadKey, formOptions?.source_kind, formOptions?.source_id, formOptions?.source_digest]) : "";
  latest.current.configurationKey = configurationKey;
  latest.current.formTargetCurrent = formTargetCurrent;
  portForm.current = { key: configurationKey, port: values.MOBKIT_PORT || "", busy, open: formOpen };
  const portObservation = deploymentPortObservation(portCheck.key === configurationKey ? portCheck.options : formOptions,
    build, state, values.MOBKIT_PORT || "", receipt);
  const update = receipt?.update;
  const ownerStart = deploymentUpdateOwnerStart(update);
  const backgroundUpdate = deploymentBackgroundUpdate(receipt, state);
  const recoverCurrent = deploymentUpdateAction(receipt, build, state, "recover-current");
  const stale = receipt && (!artifactIsCurrent(receipt, state) ||
    deploymentSourceKind(receipt) === "application" && build?.guide_id === state.id &&
      !!build.build_id && receipt.build_id !== build.build_id);
  useEffect(() => { onReceipt?.(receipt, { guideId: state.id, selectionToken, observation }); },
    [receipt, state.id, selectionToken, observation, onReceipt]);

  function chooseSource(kind) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    sourceChoice.current = { guideId: state.id, explicit: true };
    setSourceKind(kind);
  }

  function beginBusyOperation(pollToken) {
    const owner = { scope: pollToken?.scope };
    publishCommandReturn(null);
    busyOperation.current = owner;
    setBusy(true);
    return owner;
  }

  function finishBusyOperation(owner) {
    if (busyOperation.current !== owner) return;
    if (commandReturnRef.current?.owner === owner) publishCommandReturn(null);
    busyOperation.current = null;
    setBusy(false);
  }

  function publishCommandReturn(value) {
    commandReturnRef.current = value;
    setCommandReturn(value);
  }

  function commandReturnCurrent(value) {
    const owner = value?.owner, intent = owner?.commandIntent;
    if (owner?.installSelection) return commandReturnRef.current === value && busyOperation.current === owner &&
      !owner.detached && !owner.responseSuperseded && owner.selectionCurrent() &&
      installReceiptMatches(owner, value.receipt) && latest.current.receipt === value.receipt &&
      !latest.current.statusUnknown && polling.current.current(value.pollToken, latest.current.state.id);
    return !!intent && commandReturnRef.current === value && busyOperation.current === owner && !owner.detached &&
      owner.selectionCurrent() && latest.current.state.id === intent.guide_id &&
      latest.current.state.version === owner.version && latest.current.sourceKey === owner.sourceKey &&
      latest.current.receipt === value.receipt && !latest.current.statusUnknown &&
      polling.current.current(value.pollToken, latest.current.state.id);
  }

  function installReceiptMatches(owner, observed) {
    const selected = owner.installSelection;
    return !!selected && !owner.responseSuperseded && latest.current.state.id === selected.guideId &&
      latest.current.state.version === selected.version && latest.current.sourceKey === selected.sourceKey &&
      latest.current.sourceKind === selected.sourceKind && artifactIsCurrent(observed, latest.current.state) &&
      deploymentInstalledSourceKind(observed, selected.guideId) === selected.sourceKind &&
      observed.guide_version === selected.version && selected.destination !== null && observed.destination === selected.destination &&
      (observed.execution_mode || "interactive") === selected.executionMode &&
      /^[a-f0-9]{64}$/.test(observed.package_sha256 || "") && observed.source_digest === selected.sourceDigest &&
      (selected.sourceKind === "native-team" ? observed.source_id === selected.source.source_id
        : observed.build_id === selected.source.build_id);
  }

  function returnToCheckedInstallation() {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    const observed = commandReturnRef.current;
    if (!commandReturnCurrent(observed)) return;
    // Another browser view can replace tracking after this read. Observe it
    // before releasing this exact local presentation owner.
    try {
      if (loadDeploymentActionIntent(globalThis.window.sessionStorage, state.id) ||
          loadDeploymentSupportIntent(globalThis.window.sessionStorage, state.id)) {
        publishCommandReturn(null); refresh(); return;
      }
    } catch { publishCommandReturn(null); refresh(); return; }
    if (!polling.current.invalidate(observed.pollToken, state.id)) return;
    observed.owner.detached = true;
    finishBusyOperation(observed.owner);
  }

  useEffect(() => {
    if (!formOpen || !active) return;
    // A saved admission also wins when the initiating HTTP response was lost.
    setFormOpen(false); setFormOptions(null); setValues({}); setUseHost(false);
    setRemoveFields([]); setFormReadKey("");
    portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false);
  }, [formOpen, active]);

  useEffect(() => {
    if (!formOpen || active || !formTargetCurrent || latest.current.blocked || busy) return;
    if (!deploymentConfigurationRevealCurrent(formIntent, formOptions, formReadKey, latest.current)) return;
    revealDeploymentConfiguration(configurationRef.current, configurationKey);
  }, [formOpen, configurationKey]);

  useEffect(() => {
    if (selectionBlocked) return;
    if (!experienceKey) { setExperienceSnapshot({ key: "", value: null, issue: null, pending: false }); return; }
    let cancelled = false;
    const identity = state.id, ownerCurrent = captureGuideSelection(selectionScope, state);
    if (!ownerCurrent) return;
    latest.current.experiencePending = true;
    setExperienceSnapshot((previous) => ({ key: experienceKey,
      value: previous.key === experienceKey ? previous.value : null,
      issue: previous.key === experienceKey ? previous.issue : null, pending: true }));
    async function load() {
      try {
        const value = await request(`${deploymentUrl(identity)}/experiences`);
        if (!cancelled && ownerCurrent() && latest.current.state.id === identity) setExperienceSnapshot({ key: experienceKey, value, issue: null, pending: false });
      } catch {
        if (!cancelled && ownerCurrent() && latest.current.state.id === identity) setExperienceSnapshot({ key: experienceKey, value: null,
          issue: "Declared application UI links could not be loaded. Installation and native Console status are unchanged.", pending: false });
      }
    }
    load();
    return () => { cancelled = true; };
  }, [experienceKey, experienceRetry, selectionToken, selectionBlocked]);

  async function refresh() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const identity = state.id;
    const operation = busyOperation.current;
    publishCommandReturn(null);
    let intent = null, supportIntent = null, intentReadable = true, actionIssue = "";
    const observed = await polling.current.observe(identity, async () => {
      try {
        intent = loadDeploymentActionIntent(globalThis.window.sessionStorage, identity);
        supportIntent = loadDeploymentSupportIntent(globalThis.window.sessionStorage, identity);
      }
      catch {
        intentReadable = false;
        actionIssue = COMMAND_STORAGE_ISSUE;
      }
      // A settled intent may already have been cleared by a prior observation.
      // The still-pending response keeps only its existing command identity.
      if (intentReadable && intent && operation === busyOperation.current && operation?.commandIntent &&
          Object.keys(operation.commandIntent).some(key => intent[key] !== operation.commandIntent[key]))
        operation.responseSuperseded = true;
      if (intentReadable && !intent && operation === busyOperation.current) intent = operation?.commandIntent || null;
      let settled = false, actionObservation = null;
      if (intent) {
        try {
          actionObservation = await request(deploymentActionObservationUrl(intent));
          settled = deploymentActionRequestSettled(actionObservation, intent);
        } catch { /* Missing or unavailable admission evidence never permits replay. */ }
        if (!settled) actionIssue = "The last installation command has not finished reporting. Forge is checking it without sending it again.";
      }
      let supportObservation = null, supportSettled = false;
      if (supportIntent) {
        try {
          supportObservation = await request(deploymentSupportObservationUrl(supportIntent));
          supportSettled = deploymentSupportRequestSettled(supportObservation, supportIntent);
        } catch { /* Observation never dispatches or resumes a support command. */ }
        if (!supportSettled) actionIssue = "Checking your installation support update. It will continue only when you choose Resume this update.";
      }
      // Receipt visibility is independent of command custody. Only readable
      // tracking and exact settlement can release the mutation controls.
      let receipt;
      try { receipt = await request(deploymentUrl(identity)); }
      catch (failure) { if (failure.status !== 404) throw failure; receipt = null; }
      return { receipt, settled, actionObservation, supportObservation, supportSettled };
    });
    if (!ownerCurrent() || !observed || !polling.current.current(observed.token, latest.current.state.id)) return;
    if (observed.failure) {
      // Passive polling keeps the saved receipt and actual action errors.
      setSnapshot((previous) => ({ guideId: identity, selectionToken, observation: "unknown",
        receipt: previous.guideId === identity ? previous.receipt : null }));
      setConnection({ guideId: identity, confirmed: false, unavailable: true,
        actionIssue: actionIssue || (intent ? "The saved installation command still needs its own completion check." : "") });
      setSupportTracking({ guideId: identity, confirmed: false, storageAvailable: intentReadable,
        ordinaryClear: false, intent: supportIntent, observation: null });
    } else {
      const next = observed.receipt;
      setSnapshot({ guideId: identity, selectionToken, receipt: next.receipt, observation: "known" });
      if (intent && next.settled) {
        try {
          // A concurrent successful observer may already have retired this intent.
          if (loadDeploymentActionIntent(globalThis.window.sessionStorage, identity)) clearDeploymentActionIntent(globalThis.window.sessionStorage, intent);
        } catch {
          setConnection({ guideId: identity, confirmed: false, unavailable: true, actionIssue: COMMAND_STORAGE_ISSUE });
          setSupportTracking({ guideId: identity, confirmed: false, storageAvailable: false,
            ordinaryClear: false, intent: supportIntent, observation: null });
          return;
        }
      }
      if (supportIntent && next.supportSettled) {
        try {
          if (loadDeploymentSupportIntent(globalThis.window.sessionStorage, identity))
            clearDeploymentSupportIntent(globalThis.window.sessionStorage, supportIntent);
        } catch {
          setConnection({ guideId: identity, confirmed: false, unavailable: true, actionIssue: COMMAND_STORAGE_ISSUE });
          setSupportTracking({ guideId: identity, confirmed: false, storageAvailable: false,
            ordinaryClear: false, intent: supportIntent, observation: null });
          return;
        }
      }
      const ordinaryClear = intentReadable && (!intent || next.settled);
      const confirmed = ordinaryClear && (!supportIntent || next.supportSettled);
      setSupportTracking({ guideId: identity, confirmed: intentReadable, storageAvailable: intentReadable,
        ordinaryClear, intent: next.supportSettled ? null : supportIntent,
        observation: next.supportSettled ? null : next.supportObservation });
      setConnection({ guideId: identity, confirmed, unavailable: !confirmed, actionIssue });
      if (confirmed && operation === busyOperation.current && operation?.commandIntent &&
          operation.scope === observed.token.scope && operation.selectionCurrent() &&
          deploymentActionRequestSettled(next.actionObservation, operation.commandIntent) &&
          next.receipt?.guide_id === operation.commandIntent.guide_id &&
          next.receipt.deployment_id === operation.commandIntent.deployment_id &&
          deploymentInstalledSourceKind(next.receipt, identity))
        publishCommandReturn({ owner: operation, receipt: next.receipt, pollToken: observed.token });
      if (confirmed && operation === busyOperation.current && operation?.installSelection && !operation.detached &&
          operation.scope === observed.token.scope && operation.selectionCurrent() && installReceiptMatches(operation, next.receipt))
        publishCommandReturn({ owner: operation, receipt: next.receipt, pollToken: observed.token });
      const pendingError = commandObservationError.current;
      if (confirmed && pendingError && pendingError.selectionToken === selectionToken &&
          pendingError.scope === observed.token.scope &&
          deploymentActionRequestSettled(next.actionObservation, pendingError.intent) &&
          next.actionObservation.action_request.outcome === "returned" &&
          next.receipt?.guide_id === pendingError.intent.guide_id &&
          next.receipt.deployment_id === pendingError.intent.deployment_id) setError("");
    }
  }

  function observeAfterMutationFailure(token, failure, commandIntent = null) {
    const guideId = latest.current.state.id;
    // A read begun before the lost response cannot resolve its admission.
    if (!polling.current.invalidate(token, guideId)) return;
    latest.current.statusUnknown = true;
    setSnapshot((previous) => ({ guideId, selectionToken, observation: "loading",
      receipt: previous.guideId === guideId ? previous.receipt : null }));
    setConnection({ guideId, confirmed: false, unavailable: true });
    setSupportTracking((previous) => ({ ...previous, confirmed: false, ordinaryClear: false }));
    setError(failure.message);
    if (commandIntent && !Number.isInteger(failure.status))
      commandObservationError.current = { intent: commandIntent, selectionToken, scope: token.scope };
    refresh();
  }

  useEffect(() => {
    generation.current += 1;
    sourceChoice.current = { guideId: state.id, explicit: false };
    setSourceKind(initialSourceKind); setOptions(null); setValues({}); setUseHost(false); setDestination(""); setFormOpen(false); setError("");
    setExecutionMode("interactive"); setFormOptions(null); setFormIntent("new"); setLoadingOptions(false); setRemoveFields([]); setFormReadKey("");
    portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false);
  }, [state.id]);

  useEffect(() => {
    if (selectionBlocked) return;
    const scope = polling.current.open(state.id);
    setSnapshot((previous) => ({ guideId: state.id, selectionToken, observation: "loading",
      receipt: previous.guideId === state.id ? previous.receipt : null }));
    setConnection({ guideId: state.id, confirmed: false, unavailable: false });
    setSupportTracking({ guideId: state.id, confirmed: false, storageAvailable: true,
      ordinaryClear: false, intent: null, observation: null });
    let stopped = false;
    async function poll() {
      if (stopped) return;
      await refresh();
      if (!stopped) timer.current = setTimeout(poll, 1500);
    }
    poll();
    return () => {
      stopped = true; portEpoch.current += 1; clearTimeout(timer.current); polling.current.close(scope);
      if (busyOperation.current?.scope === scope) finishBusyOperation(busyOperation.current);
    };
  }, [state.id, selectionToken, selectionBlocked]);

  useEffect(() => {
    if (sourceChoice.current.guideId !== state.id || sourceChoice.current.explicit) return;
    setSourceKind(deploymentInstalledSourceKind(receipt, state.id) || initialSourceKind);
  }, [state.id, initialSourceKind, receipt?.deployment_id, receipt?.source_kind, receipt?.build_id,
    receipt?.source_id, receipt?.source_digest]);

  useEffect(() => {
    if (!sourceNavigation || latest.current.blocked || busy || sourceNavigation.guideId !== state.id ||
        sourceNavigation.version !== state.version || !["native-team", "application"].includes(sourceNavigation.sourceKind) ||
        sourceNavigation.key !== deploymentSourceKey(build, state, sourceNavigation.sourceKind) ||
        !deploymentCanPrepare(build, state, sourceNavigation.sourceKind)) return;
    chooseSource(sourceNavigation.sourceKind);
  }, [sourceNavigation]);

  useEffect(() => {
    // Private drafts never cross the selected application or installation.
    setValues({}); setUseHost(false); setFormOpen(false); setDestination("");
    setExecutionMode("interactive"); setFormOptions(null); setFormIntent("new"); setRemoveFields([]); setFormReadKey("");
  }, [state.id, sourceKey, receipt?.deployment_id, receipt?.build_id, receipt?.source_kind, receipt?.source_id,
    receipt?.package_sha256, receipt?.source_digest, receipt?.destination, receipt?.execution_mode]);

  useEffect(() => {
    // A conversation revision retires reads, while same-source drafts stay in
    // component memory until fresh host options prove their current binding.
    generation.current += 1;
    const operation = busyOperation.current;
    // The first saved admission changes generation too. Only actual selection
    // changes retire this held response's explicit return affordance.
    if (operation?.installSelection && (operation.installSelection.guideId !== state.id ||
        operation.installSelection.version !== state.version || operation.installSelection.sourceKey !== sourceKey ||
        operation.installSelection.sourceKind !== sourceKind)) {
      operation.responseSuperseded = true;
      if (commandReturnRef.current?.owner === operation) publishCommandReturn(null);
    }
    setOptions(null); setLoadingOptions(false);
    portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false);
  }, [state.id, sourceKey, state.version, receipt?.deployment_id, receipt?.build_id, receipt?.source_kind, receipt?.source_id,
    receipt?.package_sha256, receipt?.source_digest, receipt?.destination, receipt?.execution_mode]);

  useEffect(() => {
    if (designBusy || pendingManual) return;
    if (latest.current.formOpen && latest.current.formIntent === "settings") prepareInstalledSettings(true);
    else if (canPrepare) prepareOptions();
  }, [state.id, state.version, sourceKey, canPrepare, canConfigure, designBusy, pendingManual, selectionToken, selectionBlocked,
    receipt?.deployment_id, receipt?.build_id, receipt?.package_sha256, receipt?.source_digest, receipt?.destination, receipt?.execution_mode]);

  async function prepareOptions(intent = null) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (intent === "settings") return prepareInstalledSettings();
    const openForm = intent !== null;
    if (!canPrepare || openForm && (busy || loadingOptions)) return;
    const selected = { guideId: state.id, buildId: build?.build_id, sourceKind, sourceKey, version: state.version, generation: generation.current, optionsRequest: ++optionsRequest.current };
    setLoadingOptions(true); setError("");
    try {
      const query = selected.sourceKind === "native-team" ? { source_kind: "native-team", version: selected.version }
        : { build_id: selected.buildId };
      const next = await readDeploymentSetup(`${deploymentUrl(selected.guideId)}/options?${new URLSearchParams(query)}`);
      if (!ownerCurrent() || selected.generation !== generation.current || latest.current.state.id !== selected.guideId ||
          latest.current.sourceKey !== selected.sourceKey || latest.current.state.version !== selected.version ||
          deploymentSourceKind(next) !== selected.sourceKind ||
          !deploymentOptionsCurrent(next, latest.current.build, latest.current.state)) return;
      setOptions(next);
      if (!openForm) {
        if (latest.current.formOpen && latest.current.formIntent === "new") setFormOptions(next);
        return;
      }
      const form = deploymentConfigurationForm(next, latest.current.receipt, latest.current.build, latest.current.state, intent);
      if (!form) return;
      setFormIntent(form.intent); setFormOptions(form.options); setExecutionMode(form.executionMode);
      setDestination(form.destination); setValues(form.values); setUseHost(false); setRemoveFields([]);
      setFormReadKey(deploymentSettingsReadKey(latest.current)); setFormOpen(true);
      portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false);
    } catch (failure) {
      if (ownerCurrent() && selected.generation === generation.current) setError(failure.message);
    } finally {
      if (selected.optionsRequest === optionsRequest.current && selected.generation === generation.current) setLoadingOptions(false);
    }
  }

  async function prepareInstalledSettings(preserveDraft = false) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (!canConfigure || busy || !preserveDraft && loadingOptions || designBusy || pendingManual) return;
    const selected = { ...latest.current, generation: generation.current, optionsRequest: ++optionsRequest.current,
      pollToken: polling.current.capture(state.id) };
    setLoadingOptions(true); setError("");
    try {
      const next = await readDeploymentSettings(selected, request, () => ({ ...latest.current,
        blocked: !ownerCurrent() || latest.current.blocked || !polling.current.current(selected.pollToken, latest.current.state.id) }));
      if (!ownerCurrent() || !next || selected.generation !== generation.current ||
          !polling.current.current(selected.pollToken, latest.current.state.id) ||
          !deploymentSettingsReadCurrent(selected, latest.current, next)) return;
      const form = deploymentConfigurationForm(next, latest.current.receipt, latest.current.build, latest.current.state, "settings");
      if (!form) return;
      if (preserveDraft) {
        if (!latest.current.formOpen || latest.current.formIntent !== "settings") return;
        setFormOptions(form.options); setFormReadKey(deploymentSettingsReadKey(latest.current));
        return;
      }
      setFormIntent("settings"); setFormOptions(form.options); setExecutionMode(form.executionMode);
      setDestination(form.destination); setValues({}); setUseHost(false); setRemoveFields([]);
      setFormReadKey(deploymentSettingsReadKey(latest.current)); setFormOpen(true);
      portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false);
    } catch (failure) {
      if (ownerCurrent() && selected.generation === generation.current && polling.current.current(selected.pollToken, latest.current.state.id) &&
          deploymentSettingsReadKey(selected) === deploymentSettingsReadKey(latest.current)) setError(failure.message);
    } finally {
      if (selected.optionsRequest === optionsRequest.current && selected.generation === generation.current && polling.current.sameScope(selected.pollToken, latest.current.state.id)) setLoadingOptions(false);
    }
  }

  function changePort(value) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false);
    setValues((previous) => ({ ...previous, MOBKIT_PORT: value }));
  }

  async function checkPort() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const port = values.MOBKIT_PORT || "";
    if (busy || checkingPort || !formTargetCurrent || latest.current.blocked || active ||
        !/^[0-9]+$/.test(port) || Number(port) < 1024 || Number(port) > 65535) return;
    const selected = { key: configurationKey, port, guideId: state.id, version: state.version,
      sourceKey, sourceKind, buildId: build?.build_id, sourceDigest: build?.source_digest, generation: generation.current,
      nativeSourceId: formOptions?.source_id, nativeSourceDigest: formOptions?.source_digest,
      epoch: ++portEpoch.current, readKey: formReadKey, intent: formIntent };
    const current = () => ownerCurrent() && selected.epoch === portEpoch.current && selected.generation === generation.current &&
      portForm.current.open && !portForm.current.busy && portForm.current.key === selected.key && portForm.current.port === selected.port &&
      !latest.current.blocked && !deploymentIsActive(latest.current.receipt) &&
      latest.current.state.id === selected.guideId && latest.current.state.version === selected.version &&
      latest.current.sourceKey === selected.sourceKey &&
      (selected.intent !== "settings" || selected.readKey === deploymentSettingsReadKey(latest.current));
    if (!current()) return;
    setCheckingPort(true); setError("");
    try {
      const query = new URLSearchParams(selected.intent === "settings" ?
        { deployment_id: receipt.deployment_id, console_port: port } : selected.sourceKind === "native-team"
        ? { source_kind: "native-team", version: selected.version, console_port: port } : { build_id: selected.buildId, console_port: port });
      const next = await readDeploymentSetup(`${deploymentUrl(selected.guideId)}/${selected.intent === "settings" ? "settings" : "options"}?${query}`);
      if (!current()) return;
      if (selected.intent !== "settings" && selected.sourceKind === "native-team" &&
          (next.source_id !== selected.nativeSourceId || next.source_digest !== selected.nativeSourceDigest))
        throw new Error("The package changed while checking the port. Reopen private setup before installing.");
      if (!deploymentPortObservation(next, latest.current.build, latest.current.state, port, latest.current.receipt))
        throw new Error("The port check could not be verified. Your setup fields are unchanged. Check again.");
      setPortCheck({ key: selected.key, options: next });
    } catch (failure) {
      if (current()) setError("The port check could not be completed. Your setup fields are unchanged. Check again.");
    } finally {
      if (selected.epoch === portEpoch.current) setCheckingPort(false);
    }
  }

  async function downloadRemotePackage(expected = null) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (!handoff || downloading || downloadPending.current.has(state.id) || designBusy || pendingManual) return;
    const selected = { guideId: state.id, buildId: build?.build_id, version: state.version, generation: generation.current,
      sourceKind, sourceKey, sourceDigest: build?.source_digest, blueprintHash: build?.blueprint_hash };
    const current = () => ownerCurrent() && selected.generation === generation.current && !latest.current.blocked &&
      latest.current.state.id === selected.guideId && latest.current.state.version === selected.version &&
      (selected.sourceKind === "native-team"
        ? latest.current.sourceKind === selected.sourceKind && latest.current.sourceKey === selected.sourceKey
        : latest.current.build?.build_id === selected.buildId && latest.current.build?.source_digest === selected.sourceDigest &&
          latest.current.build?.blueprint_hash === selected.blueprintHash) && (!expected ||
        typeof expected.isCurrent === "function" && expected.isCurrent() &&
        expected.binding?.guide_id === selected.guideId && expected.binding.guide_version === selected.version &&
        expected.binding.build_id === selected.buildId && expected.binding.source_digest === selected.sourceDigest &&
        expected.binding.blueprint_hash === selected.blueprintHash && /^[a-f0-9]{64}$/.test(expected.binding.package_sha256 || "") &&
        (!latest.current.build.package_sha256 || latest.current.build.package_sha256 === expected.binding.package_sha256));
    if (!current()) return;
    downloadPending.current.set(selected.guideId, selected);
    setDownloadRequests((old) => ({ ...old, [selected.guideId]: selected })); setError("");
    try {
      const blob = await requestBlob(handoff.package_url, { timeoutMs: 60000 });
      if (!current()) return;
      if (expected) {
        const bytes = await blob.arrayBuffer();
        if (!current()) return;
        const digest = await crypto.subtle.digest("SHA-256", bytes);
        if (!current()) return;
        const actual = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
        if (actual !== expected.binding.package_sha256)
          throw new Error("The downloaded package differs from the package checked for this cloud plan. Download it again before continuing.");
      }
      const url = URL.createObjectURL(blob), link = document.createElement("a");
      link.href = url; link.download = `${plan.project.slug}-${selected.sourceKind === "native-team" ? "agent-team" : "application"}.zip`; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (failure) {
      if (current()) setError(failure.message);
    } finally {
      if (downloadPending.current.get(selected.guideId) === selected) downloadPending.current.delete(selected.guideId);
      setDownloadRequests((old) => {
        if (old[selected.guideId] !== selected) return old;
        const next = { ...old }; delete next[selected.guideId]; return next;
      });
    }
  }

  async function install(event) {
    event.preventDefault();
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (latest.current.statusUnknown) return;
    if (!latest.current.formTargetCurrent || latest.current.configurationKey !== configurationKey) return;
    if (formIntent === "settings") return saveInstalledSettings();
    if (!canInstall || !formTargetCurrent || !deploymentCanSubmit(privateFormOptions, values, useHost, executionMode) ||
        portObservation && portObservation.status !== "available" || checkingPort || busy || active || designBusy || pendingManual) return;
    const source = deploymentInstallSelection(formOptions, latest.current.build, latest.current.state);
    if (!source || deploymentSourceKind(formOptions) !== latest.current.sourceKind) return;
    const selected = { guideId: state.id, source, sourceKind, sourceKey, sourceDigest: formOptions.source_digest,
      destination: canonicalInstallDestination(destination, formOptions.home_directory), executionMode,
      version: state.version, generation: generation.current,
      pollToken: polling.current.capture(state.id) };
    const operation = beginBusyOperation(selected.pollToken); setError("");
    Object.assign(operation, { installSelection: selected, selectionCurrent: ownerCurrent });
    const responseCurrent = () => busyOperation.current === operation && !operation.detached && !operation.responseSuperseded &&
      ownerCurrent() && selected.generation === generation.current && latest.current.state.id === selected.guideId &&
      latest.current.state.version === selected.version && latest.current.sourceKey === selected.sourceKey;
    try {
      const next = await request(deploymentUrl(selected.guideId), { method: "POST", body: {
        version: selected.version, ...selected.source, destination,
        settings: deploymentSettings(privateFormOptions, values), use_host_provider_credentials: useHost,
        execution_mode: executionMode,
      } });
      if (!responseCurrent() || !polling.current.admit(selected.pollToken, latest.current.state.id)) return;
      setSnapshot({ guideId: selected.guideId, selectionToken, receipt: next, observation: "known" });
      setConnection({ guideId: selected.guideId, confirmed: true, unavailable: false });
      // Credentials are never persisted in browser storage or sent to the guide.
      setValues({}); setUseHost(false); setFormOpen(false);
    } catch (failure) {
      if (responseCurrent()) observeAfterMutationFailure(selected.pollToken, failure);
    } finally { finishBusyOperation(operation); }
  }

  async function saveInstalledSettings() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (latest.current.statusUnknown) return;
    if (!settingsBody || busy || loadingOptions || active || designBusy || pendingManual) return;
    const selected = { ...latest.current, generation: generation.current, body: settingsBody, options: privateFormOptions,
      pollToken: polling.current.capture(state.id) };
    const operation = beginBusyOperation(selected.pollToken); setError("");
    operation.settingsPresentation = { guideId: state.id, selectionToken };
    try {
      const next = await request(`${deploymentUrl(selected.state.id)}/settings`, { method: "POST", body: selected.body });
      if (!ownerCurrent() || selected.generation !== generation.current || !deploymentSettingsResponseCurrent(selected, latest.current, next) ||
          !polling.current.admit(selected.pollToken, latest.current.state.id)) return;
      setSnapshot({ guideId: selected.state.id, selectionToken, receipt: next, observation: "known" });
      setConnection({ guideId: selected.state.id, confirmed: true, unavailable: false });
      setValues({}); setUseHost(false); setRemoveFields([]); setFormOpen(false); setFormOptions(null); setFormReadKey("");
    } catch (failure) {
      if (ownerCurrent() && selected.generation === generation.current && polling.current.current(selected.pollToken, latest.current.state.id) &&
          deploymentSettingsReadKey(selected) === deploymentSettingsReadKey(latest.current)) {
        observeAfterMutationFailure(selected.pollToken, failure);
      }
    } finally { finishBusyOperation(operation); }
  }

  async function stageUpdate() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (latest.current.statusUnknown) return;
    if (!canStageUpdate || busy || designBusy || pendingManual) return;
    const selected = { guideId: state.id, buildId: build.build_id, version: state.version, deploymentId: receipt.deployment_id,
      pollToken: polling.current.capture(state.id) };
    const operation = beginBusyOperation(selected.pollToken); setError("");
    // Presentation only; the original request retains its settlement ownership.
    operation.stagePresentation = { guideId: selected.guideId, selectionToken };
    try {
      const next = await request(`${deploymentUrl(selected.guideId)}/update-stage`, { method: "POST", body: {
        deployment_id: selected.deploymentId, version: selected.version, build_id: selected.buildId,
      } });
      if (ownerCurrent() && polling.current.admit(selected.pollToken, latest.current.state.id)) {
        setSnapshot({ guideId: selected.guideId, selectionToken, receipt: next, observation: "known" });
        setConnection({ guideId: selected.guideId, confirmed: true, unavailable: false });
      }
    } catch (failure) {
      if (ownerCurrent()) observeAfterMutationFailure(selected.pollToken, failure);
    } finally { finishBusyOperation(operation); }
  }

  async function action(name) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (latest.current.statusUnknown) return;
    if (!receipt || busy) return;
    try {
      if (loadDeploymentSupportIntent(globalThis.window.sessionStorage, state.id)) return;
    } catch {
      setConnection({ guideId: state.id, confirmed: false, unavailable: true, actionIssue: COMMAND_STORAGE_ISSUE });
      setSupportTracking((previous) => ({ ...previous, confirmed: false, storageAvailable: false }));
      return;
    }
    const updateAction = ["apply-update", "rollback", "recover-current"].includes(name) ? deploymentUpdateAction(receipt, build, state, name) : null;
    if (["apply-update", "rollback", "recover-current"].includes(name) && (!updateAction || designBusy || pendingManual)) return;
    const selected = { guideId: state.id, deploymentId: receipt.deployment_id, commandIntent: null,
      pollToken: polling.current.capture(state.id) };
    const operation = beginBusyOperation(selected.pollToken); setError("");
    try {
      const body = updateAction || { deployment_id: selected.deploymentId, action: name };
      const intent = deploymentActionIntent(selected.guideId, body, crypto.randomUUID());
      // Save only transport identity before admission, so a reload can observe it.
      try { storeDeploymentActionIntent(globalThis.window.sessionStorage, intent); }
      catch { throw new Error(COMMAND_STORAGE_ISSUE); }
      selected.commandIntent = intent;
      Object.assign(operation, { commandIntent: intent, selectionCurrent: ownerCurrent,
        version: state.version, sourceKey: latest.current.sourceKey });
      if (!ownerCurrent() || !polling.current.invalidate(selected.pollToken, latest.current.state.id)) return;
      selected.pollToken = polling.current.capture(selected.guideId);
      latest.current.statusUnknown = true;
      setSnapshot((previous) => ({ guideId: selected.guideId, selectionToken, observation: "loading",
        receipt: previous.guideId === selected.guideId ? previous.receipt : null }));
      setSupportTracking((previous) => ({ ...previous, confirmed: false, ordinaryClear: false }));
      setConnection({ guideId: selected.guideId, confirmed: false, unavailable: true,
        actionIssue: "Checking the installation command. Forge will not send it again." });
      const next = await request(`${deploymentUrl(selected.guideId)}/action`, { method: "POST",
        body: { ...body, action_request_id: intent.action_request_id } });
      if (busyOperation.current !== operation || operation.detached || operation.responseSuperseded) return;
      if (ownerCurrent() && polling.current.current(selected.pollToken, latest.current.state.id)) {
        if (!deploymentActionRequestSettled(next, intent) || next.guide_id !== intent.guide_id || next.deployment_id !== intent.deployment_id) {
          polling.current.invalidate(selected.pollToken, latest.current.state.id);
          setConnection({ guideId: selected.guideId, confirmed: false, unavailable: true,
            actionIssue: "The command response did not establish its completion. Forge is checking the saved command without sending it again." });
          refresh();
          return;
        }
        try {
          if (loadDeploymentActionIntent(globalThis.window.sessionStorage, selected.guideId)) clearDeploymentActionIntent(globalThis.window.sessionStorage, intent);
        } catch { throw new Error(COMMAND_STORAGE_ISSUE); }
        if (!polling.current.invalidate(selected.pollToken, latest.current.state.id)) return;
        // Command settlement does not carry the checked deployment projection.
        // Keep the last status visible and unconfirmed until a fresh GET returns.
        latest.current.statusUnknown = true;
        setSupportTracking((previous) => ({ ...previous, confirmed: false, ordinaryClear: false }));
        setConnection({ guideId: selected.guideId, confirmed: false, unavailable: true,
          actionIssue: "The installation command finished. Checking current installation status." });
        refresh();
      }
    } catch (failure) {
      if (busyOperation.current === operation && !operation.detached && !operation.responseSuperseded && ownerCurrent())
        observeAfterMutationFailure(selected.pollToken, failure, selected.commandIntent);
    } finally { finishBusyOperation(operation); }
  }

  async function supportAction(name) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !supportConfirmed || !receipt || busy || designBusy || pendingManual) return;
    const selected = { guideId: state.id, deploymentId: receipt.deployment_id,
      pollToken: polling.current.capture(state.id) };
    const operation = beginBusyOperation(selected.pollToken); setError("");
    try {
      const storage = globalThis.window.sessionStorage;
      if (loadDeploymentActionIntent(storage, selected.guideId)) throw new Error("Check the previous installation command before continuing this update.");
      const saved = loadDeploymentSupportIntent(storage, selected.guideId);
      let intent, body;
      if (saved) {
        if (JSON.stringify(saved) !== JSON.stringify(supportTracking.intent))
          throw new Error("The saved support command changed. Check status before continuing.");
        intent = saved;
        body = deploymentSupportAction(receipt, state, name, { confirmed: supportConfirmed,
          intent, observation: supportTracking.observation, resume: true });
      } else {
        if (supportTracking.intent) throw new Error("Check the saved support command before continuing.");
        const command = deploymentSupportAction(receipt, state, name, { confirmed: supportConfirmed });
        if (!command) return;
        intent = deploymentSupportIntent(selected.guideId, command, crypto.randomUUID());
        body = deploymentSupportBodyFromIntent(intent, { initial: true, confirmed: supportConfirmed,
          receipt, guide: state });
        if (!body) return;
        storeDeploymentSupportIntent(storage, intent);
      }
      if (!body || !ownerCurrent() || !polling.current.invalidate(selected.pollToken, latest.current.state.id)) return;
      selected.pollToken = polling.current.capture(selected.guideId);
      latest.current.statusUnknown = true;
      setSnapshot((previous) => ({ guideId: selected.guideId, selectionToken, observation: "loading",
        receipt: previous.guideId === selected.guideId ? previous.receipt : null }));
      setSupportTracking({ guideId: selected.guideId, confirmed: false, storageAvailable: true,
        ordinaryClear: true, intent, observation: null });
      setConnection({ guideId: selected.guideId, confirmed: false, unavailable: true,
        actionIssue: "Checking your installation support update." });
      const next = await request(`${deploymentUrl(selected.guideId)}/action`, { method: "POST", body });
      if (!ownerCurrent() || !polling.current.current(selected.pollToken, latest.current.state.id)) return;
      if (deploymentSupportRequestSettled(next, intent)) clearDeploymentSupportIntent(storage, intent);
      // Both successful and uncertain replies need a fresh receipt observation.
      // Neither browser intent nor the command response becomes runtime truth.
      if (polling.current.invalidate(selected.pollToken, latest.current.state.id)) refresh();
    } catch (failure) {
      if (ownerCurrent()) observeAfterMutationFailure(selected.pollToken, failure);
    } finally { finishBusyOperation(operation); }
  }

  const remoteTarget = plan.target === "gcp" || plan.target === "linux" && options?.host_target !== "linux";
  // Saved destinations keep their observation and exit controls when the draft
  // moves elsewhere. New installation still uses the selected application.
  const cloudControls = <GcpGuided selectionScope={selectionScope} state={state} build={build} allowPreparation={!nativeTeam && plan.target === "gcp"}
    blocked={busy || designBusy || pendingManual} onChooseProject={onChooseCloudProject}
    onCloudRemoval={setCloudRemoval}
    onDownload={handoff && options.target === "gcp" ? downloadRemotePackage : undefined} downloading={downloading} />;
  const remoteControls = <RemoteDeployment selectionScope={selectionScope} state={state} build={build} options={nativeTeam ? null : options}
    plan={plan} allowConnection={!nativeTeam && remoteTarget} blocked={busy || designBusy || pendingManual}
    cloudRemoval={cloudRemoval}
    onReceipt={onRemoteReceipt} targetRef={remoteTargetRef}
    onRepairReceipt={onRepairReceipt} renderConfiguration={(props) => <DeploymentConfigurationFields {...props} />} />;
  const showInstallation = !!(canPrepare || receipt || error || reconnecting || remoteTarget || formOpen ||
    sourceChoice.current.guideId === state.id && sourceChoice.current.explicit);
  const configurationForm = formOpen && <form ref={configurationRef} data-configuration-key={configurationKey} className="deployment-form" onSubmit={install} autoComplete="off">
      <h3 tabIndex={-1} data-configuration-heading>{formIntent === "settings" ? "Change this installation's private settings" : receipt ? "Install in a new folder" : "Set up your installation"}</h3>
      <label>Project folder<input value={destination} onChange={(event) => setDestination(event.target.value)} required spellCheck="false" disabled={selectionBlocked || busy || !formTargetCurrent || formIntent === "settings"} /></label>
      {formIntent === "settings" ? <DeploymentPrivateSettingsInstructions receipt={receipt} /> : <p>Choose a new empty folder. Its private settings are separate from any existing installation.</p>}
      {!formTargetCurrent && <p className="deployment-warning" role="status">{loadingOptions
        ? "Rechecking the current proposal and application settings. Your private setup draft stays here."
        : "Your setup draft is kept in this page and has not been sent. The current proposal and application settings must be checked before continuing."}</p>}
      {formIntent === "settings" && !formTargetCurrent && canConfigure && <button className="setup-action" type="button"
        disabled={mutationBlocked || busy || loadingOptions || designBusy || pendingManual}
        onClick={() => prepareInstalledSettings(true)}>Recheck private settings</button>}
      <label>Console port<input name="MOBKIT_PORT" value={values.MOBKIT_PORT || ""} onChange={(event) => changePort(event.target.value)} inputMode="numeric" required={formIntent !== "settings"} placeholder={formIntent === "settings" ? "Leave blank to keep the installed port" : ""} disabled={selectionBlocked || busy || !formTargetCurrent} /></label>
      {portObservation && <p className={portObservation.status === "available" ? "deployment-field-help" : "deployment-warning"} role="status">{portObservation.detail}</p>}
      <div className="deployment-actions">
        <button className="setup-action" type="button" onClick={checkPort} disabled={selectionBlocked || busy || checkingPort || active || !formTargetCurrent || designBusy || pendingManual ||
          !/^[0-9]+$/.test(values.MOBKIT_PORT || "") || Number(values.MOBKIT_PORT) < 1024 || Number(values.MOBKIT_PORT) > 65535}>{checkingPort ? "Checking port..." : "Check this port"}</button>
        {portObservation?.alternatePort && <button className="setup-action" type="button" disabled={selectionBlocked || busy || active || !formTargetCurrent || designBusy || pendingManual}
          onClick={() => changePort(String(portObservation.alternatePort))}>Use port {portObservation.alternatePort}</button>}
      </div>
      {formIntent !== "settings" && <fieldset className="deployment-run-mode"><legend>How should it run?</legend>
        {deploymentExecutionModes(privateFormOptions).map((item) => <label className="deployment-toggle" key={item.id}>
          <input type="radio" name={`run-mode-${state.id}`} value={item.id} checked={executionMode === item.id}
            disabled={selectionBlocked || busy || !formTargetCurrent || !item.available || formIntent === "settings"} onChange={() => setExecutionMode(item.id)} />
          <span><strong>{item.label}</strong><span className="deployment-field-help">{item.description}{!item.available && item.reason ? ` ${item.reason}` : ""}</span></span>
        </label>)}
      </fieldset>}
      {privateFormOptions?.can_use_host_provider_credentials && <label className="deployment-toggle"><input type="checkbox" checked={useHost} onChange={(event) => setUseHost(event.target.checked)} disabled={selectionBlocked || busy || !formTargetCurrent} /><span>Use existing provider credentials from this Forge host ({privateFormOptions.host_provider_keys.join(", ")}). Copy only these selected keys into this deployment's private configuration.</span></label>}
      <DeploymentConfigurationFields options={privateFormOptions} values={values} useHost={useHost}
        guideId={state.id} busy={selectionBlocked || busy || !formTargetCurrent} removed={removeFields}
        onChange={(key, value) => setValues({ ...values, [key]: value })}
        onRemove={formIntent === "settings" ? (key, checked) => {
          setRemoveFields((previous) => checked ? [...new Set([...previous, key])] : previous.filter((item) => item !== key));
          setValues((previous) => { const next = { ...previous }; delete next[key]; return next; });
        } : undefined} />
      <p className="deployment-private">Configuration values go directly to this computer's private .env. They do not enter guide messages, agent prompts, or exported packages.</p>
      {formIntent !== "settings" && privateFormOptions?.knowledge_mode === "local" && <p className="deployment-warning">{deploymentFreshInstallNeedsSetup(privateFormOptions)
        ? "This application uses a local knowledge service. Docker must already be available."
        : "Install and start also creates the proposed local knowledge service using Docker. Forge generates its private project credentials and verifies authenticated access. Docker must already be available."}</p>}
      {formIntent !== "settings" && <DeploymentInstallDescription nativeTeam={nativeTeam} options={privateFormOptions} />}
      {busy && busyOperation.current?.settingsPresentation?.guideId === state.id &&
        busyOperation.current.settingsPresentation.selectionToken === selectionToken && <p role="status">The save is waiting for a response. If the wait continues, open <strong>Your setups</strong> and select this setup again to check the installation status. The original save may still finish; reopening does not send it again.</p>}
      <div className="deployment-actions"><button className="setup-action primary" disabled={mutationBlocked || active || busy || checkingPort || designBusy || pendingManual || !formTargetCurrent || !destination.trim() || (formIntent === "settings" ? !settingsBody : !deploymentCanSubmit(privateFormOptions, values, useHost, executionMode) || portObservation && portObservation.status !== "available")} type="submit">{busy ? formIntent === "settings" ? "Saving private settings..." : "Admitting installation..." : formIntent === "settings" ? "Save private settings" : deploymentFreshInstallNeedsSetup(privateFormOptions) ? "Install application" : "Install and start"}</button><button className="setup-action" type="button" disabled={selectionBlocked || busy} onClick={() => { portEpoch.current += 1; setPortCheck({ key: "", options: null }); setCheckingPort(false); setFormOpen(false); setFormOptions(null); setValues({}); setUseHost(false); setRemoveFields([]); setFormReadKey(""); }}>Cancel</button></div>
    </form>;

  const readyToUse = deploymentSourceKind(receipt) === "application" && receipt?.status === "running" &&
    observation === "known" && !statusUnknown && !busy && consoleUrl && !stale;
  const statusUnconfirmed = receipt && observation !== "known";
  const installationLocation = receipt && <p className="deployment-location"><code>{receipt.destination}</code><span>{deploymentSourceKind(receipt) === "native-team"
    ? "Installed agent team" : "Installed custom application"} - proposal {receipt.guide_version}. Current proposal {state.version}.</span>
    {!stale && <span>This installation matches the current proposal.</span>}</p>;
  const installationHealth = receipt?.status === "running" && <p className="deployment-health">{observation !== "known"
    ? `Last checked: ${consoleUrl ? "the process and native MobKit Console were available" : "the process had started, but Console health was not verified"}. ${observation === "loading" ? "Checking current installation status." : "Current installation status could not be checked."}`
    : consoleUrl
    ? `${Object.keys(receipt.bootstrap.identities || {}).length} native identities bootstrapped. The process and native MobKit Console are available.`
    : "The process started, but its live console health has not been verified. Check the deployment activity below."}</p>;

  return <section className="deployment-panel" aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)} aria-label="Installation and operation" data-deployment-status={receipt?.status || handoff?.status || "not-installed"}>
    {showInstallation && <>
    <span className="setup-eyebrow">INSTALL & RUN</span>
    {agentTeamAvailable && <fieldset className="deployment-run-mode"><legend>What to install</legend>
      <label><input type="radio" name="deployment-source" value="native-team" checked={nativeTeam}
        disabled={selectionBlocked || busy} onChange={() => chooseSource("native-team")} /> Agent team - Console, agents and configured tools</label>
      <label><input type="radio" name="deployment-source" value="application" checked={!nativeTeam}
        disabled={selectionBlocked || busy} onChange={() => chooseSource("application")} /> Custom application - independently checked build</label>
    </fieldset>}
    {receipt && deploymentSourceKind(receipt) !== sourceKind && <p className="deployment-note" role="status">{nativeTeam
      ? "Agent team is selected for a separate installation. Your installed custom application and its controls are shown below."
      : "Custom application is selected for a separate installation. Your installed agent team and its controls are shown below."}</p>}
    <h2>{statusUnconfirmed ? observation === "loading" ? "Checking installation status" : "Installation status unavailable"
      : receipt ? deploymentPhase(receipt) : handoff ? `Run on ${deploymentDestinationLabel(options)}` : loadingOptions ? "Checking your deployment destination" : deploymentPhase(receipt)}</h2>
    <p>{statusUnconfirmed ? observation === "loading"
      ? "Checking current installation status. Saved details remain visible below."
      : "Current status could not be checked. The saved details below are from the last check."
      : readyToUse ? "Your agents and Console are available. Try a first job below." : receipt?.detail || (!receipt && !canPrepare
      ? packageReason || (nativeTeam ? "Accept your ready plan with Use plan before installing its agent team. You can still download a proposed package above."
        : `An application for ${plan.project.name} must pass independent acceptance before installation. Build or improve it first, then return here to run the checked package.`)
      : handoff
      ? nativeTeam
        ? `This agent team targets ${deploymentDestinationLabel(options)}. This computer runs ${options.host_target === "mac" ? "macOS" : options.host_target === "linux" ? "Linux" : "a different operating system"}. Download the team package below and follow its manual steps on the destination computer; no custom application build is needed.`
        : options.target === "gcp"
        ? `The accepted application for ${plan.project.name} is ready for cloud destination checks. Choose the account and project below.`
        : `The accepted application for ${plan.project.name} targets ${deploymentDestinationLabel(options)}. Use its checked package and the destination-specific steps below.`
      : nativeTeam ? `Install the agent team for ${plan.project.name} into a folder you own, then use its Console. No custom application build is needed.`
        : `Install the accepted application for ${plan.project.name} into a folder you own. After startup, open the application from this page.`)}</p>
    {statusUnconfirmed && receipt.detail && <p className="deployment-note">Last saved update: {receipt.detail}</p>}
    {deploymentDiagnostic(receipt) && <p className="deployment-warning" role="status">{deploymentDiagnostic(receipt)}</p>}
    {!readyToUse && installationLocation}
    {stale && <p className="deployment-warning">{deploymentSourceKind(receipt) === "native-team"
      ? "This installed agent team belongs to an earlier plan. Starting it resumes its installed agents, tools and records. A changed plan needs a separate installation in an empty folder."
      : "This installed custom application belongs to an earlier proposal or build. You can use this installed version; changes to the current proposal have not changed it. Stage the accepted update to check whether it can preserve this folder; changes needing a native migration stay blocked."}</p>}
    {!readyToUse && installationHealth}
    </>}
    {cloudControls}
    {showInstallation && <>
    <InstalledApplicationUseMount selectionScope={selectionScope} state={state} receipt={receipt} build={build}
      blocked={statusUnknown || busy || designBusy || pendingManual} commandUnconfirmed={statusUnknown} onRepairReceipt={onRepairReceipt} Icon={Icon}
      experiences={experienceSnapshot.key === experienceKey ? experienceSnapshot.value : null}
      experienceLoading={experienceLoading} experienceIssue={experienceIssue}
      onReloadExperiences={() => {
        if (!guideSelectionAvailable(selectionScope, state, selectionToken) || !experienceKey ||
            latest.current.experienceKey !== experienceKey || latest.current.experiencePending) return false;
        latest.current.experiencePending = true;
        setExperienceSnapshot((previous) => ({ ...previous, key: experienceKey, pending: true }));
        setExperienceRetry((value) => value + 1);
        return true;
      }} />
    <DeploymentInstallationArea configuration={configurationForm} receipt={receipt}>
      {active && <button className="setup-action" disabled={mutationBlocked || busy || receipt.status === "stopping"} onClick={() => action("stop")}>{deploymentSourceKind(receipt) === "native-team" ? "Stop agent team" : "Stop application"}</button>}
      <DeploymentCleanupRecovery receipt={receipt} state={state} disabled={mutationBlocked || busy} onAction={action} />
      <DeploymentResumeAction receipt={receipt} state={state} disabled={mutationBlocked || busy} onAction={action}
        setupDisabled={selectionBlocked || busy || designBusy || pendingManual || formOpen || !canConfigure}
        confirmed={!mutationBlocked} pending={["prepare-setup", "finish-setup", "start-background"].includes(busyOperation.current?.commandIntent?.action)}
        selectionScope={selectionScope} onSettings={canConfigure && !formOpen ? () => prepareOptions("settings") : null} onCheck={refresh} />
      {(receipt?.execution_mode === "background" && deploymentSourceKind(receipt) === "application" &&
        (receipt.target ?? "local") === "local" && receipt.support_request ||
        supportTracking.guideId === state.id && supportTracking.intent) &&
        <DeploymentSupportPanel view={supportView} receipt={receipt}
          intent={supportTracking.guideId === state.id ? supportTracking.intent : null}
          disabled={selectionBlocked || busy || designBusy || pendingManual || !supportConfirmed}
          busy={busy} onAction={supportAction} onCheck={refresh} />}
      <DeploymentUpdatePreparation receipt={receipt} build={build} state={state} options={options}
        disabled={mutationBlocked || busy || designBusy || pendingManual} onStage={stageUpdate} />
      {canConfigure && !formOpen && !deploymentApplicationStartupPending(receipt, state) && <button className="setup-action" disabled={mutationBlocked || active || busy || designBusy || pendingManual || loadingOptions} onClick={() => prepareOptions("settings")}>Change private settings</button>}
      {canInstall && !active && !formOpen && <button className="setup-action primary" disabled={mutationBlocked || busy || designBusy || pendingManual || loadingOptions} onClick={() => prepareOptions("new")}>{loadingOptions ? "Preparing private setup..." : receipt ? "Install into another folder" : "Install on this computer"}</button>}
      {freshInstallRestriction && <p className="deployment-note" role="status">{freshInstallRestriction}</p>}
      {handoff && options.target === "mac" && <button className="setup-action primary" disabled={selectionBlocked || downloading || designBusy || pendingManual} onClick={() => downloadRemotePackage()}>{downloading ? "Preparing accepted package..." : nativeTeam ? "Download agent team package" : "Download accepted Mac package"} <Icon name="download" size={14} /></button>}
      {canPrepare && !options && !loadingOptions && <button disabled={selectionBlocked} className="setup-action" onClick={() => prepareOptions()}>Check deployment destination</button>}
    </DeploymentInstallationArea>
    <DeploymentSettingsGuidance receipt={receipt} />
    {update && <div className="deployment-update" data-update-status={update.status}>
      <h3>{update.status === "completed" ? ownerStart ? "Accepted application update installed" : "Accepted application update verified" : update.status === "rolled-back" ? ownerStart ? "Previous application restored" : "Previous application restored and verified" : update.status === "blocked" ? receipt.execution_mode === "background" && !backgroundUpdate.supported ? "Update compatibility preview" : "This update needs a migration" : ["recovery-needed", "recovering-current"].includes(update.status) ? "Recover this application" : "Update this installed application"}</h3>
      <p>{update.detail}</p>
      {(receipt.execution_mode !== "background" || backgroundUpdate.supported) && <p className="deployment-note">{ownerStart
        ? "After setup, applying an update installs and checks the accepted source while the application stays stopped. Start restores its completed setup with the installed source. If that Start fails, choose Restore previous application when it is offered. Rollback does not undo external service actions."
        : "Application-only updates keep private settings and local application data and native history. If startup fails, Forge attempts to restore the previous source, preserves recorded activity, and reports whether recovery passed. Rollback does not undo external service actions."}</p>}
      {update.blockers?.length > 0 && <ul className="deployment-warning">{update.blockers.map((item, index) => <li key={index}>{item}</li>)}</ul>}
      <DeploymentUpdateActions receipt={receipt} build={build} state={state}
        disabled={mutationBlocked || busy || designBusy || pendingManual} onAction={action} />
      {["recovery-needed", "recovering-current"].includes(update.status) && <p className="deployment-note">Retry the exact accepted current application using your retained latest records, or recover the previous source. Neither choice undoes recorded native activity or application data. Recovery is complete only after native startup and live Console health pass.</p>}
      {["recovery-needed", "recovering-current"].includes(update.status) && !active && !recoverCurrent &&
        (receipt.execution_mode !== "background" || backgroundUpdate.stopped) && <p className="deployment-warning">Retrying the current application requires this exact candidate to remain independently accepted for the current proposal. Return to its matching proposal before recovery; a newer build cannot silently replace this transaction.</p>}
      {update.changes?.length > 0 && <details className="deployment-evidence"><summary>{update.changes.length} changed package files</summary><ul>{update.changes.map((item) => <li key={item.path}><code>{item.path}</code>: {item.change}</li>)}</ul></details>}
      <DeploymentUpdateDiagnostics update={update} />
    </div>}
    {(receipt || consoleScope) && <p className="deployment-note">Startup checks cover agent and Console availability. Custom UI, external channels and actual application jobs still need their own live verification.</p>}
    <DeploymentRuntimeDetails consoleUrl={consoleUrl} scope={runtimeScope} Icon={Icon} disabled={selectionBlocked || statusUnknown}>
      {readyToUse && <>{installationLocation}{installationHealth}</>}
    </DeploymentRuntimeDetails>
    </>}
    {remoteControls}
    {showInstallation && <>
    {handoff && (["linux", "gcp"].includes(options.target) ? <details className="deployment-evidence remote-manual-deployment">
      <summary>{nativeTeam ? "Install this agent team on your Linux computer" : options.target === "gcp" ? "Manual Google Cloud package and deployment instructions (advanced)" : "Manual Linux package and SSH instructions (optional)"}</summary>
      <button className="setup-action" disabled={selectionBlocked || downloading || designBusy || pendingManual} onClick={() => downloadRemotePackage()}>
        {downloading ? "Preparing accepted package..." : nativeTeam ? "Download agent team package" : `Download accepted ${options.target === "gcp" ? "GCP" : "Linux"} package`} <Icon name="download" size={14} /></button>
      <DeploymentHandoff handoff={handoff} />
    </details> : <DeploymentHandoff handoff={handoff} />)}
    <InstalledPackageDownload receipt={receipt} state={state} selectionScope={selectionScope}
      blocked={statusUnknown || busy || selectionBlocked} />
    {receipt?.checks?.length > 0 && <details className="deployment-evidence" open={receipt.checks.some((check) => check.status !== "pass")}><summary>Installed checks ({receipt.checks.filter((check) => check.status === "pass").length} passed)</summary><ul>{receipt.checks.map((check, index) => <li key={index}><strong>{check.name}: {check.status}</strong><p>{check.detail}</p></li>)}</ul></details>}
    <DeploymentActivity receipt={receipt} active={active} />
    {receipt?.logs?.length > 0 && <details className="deployment-evidence"><summary>Redacted installation and runtime logs</summary><pre>{receipt.logs.map((entry) => `[${entry.phase}] ${entry.text}`).join("\n")}</pre></details>}
    {receipt?.unresolved?.length > 0 && <details className="deployment-evidence"><summary>Remaining integrations and live checks</summary><ul>{receipt.unresolved.map((item, index) => <li key={index}>{item}</li>)}</ul></details>}
    {receipt && <p className="deployment-note">{observation !== "known"
      ? "Current installation status is unconfirmed. Your files and native history stay in the deployment folder. Check installation status before continuing."
      : receipt.status === "stopped"
      ? "The application is stopped. Closing this page leaves it stopped. Your files and native history stay in the deployment folder."
      : receipt.status !== "running"
      ? "Your files and native history stay in the deployment folder. Follow the installation status and any recovery steps above."
      : receipt.execution_mode === "background"
      ? "This application is managed by your operating system and can keep running after Forge closes. Its files and native history stay in the deployment folder."
      : "Closing this page keeps the application running. Stopping Forge interrupts its supervised runtime; your files and native history remain in the deployment folder. Resume explicitly after restarting Forge."}</p>}
    {receipt?.service?.public_summary && <p className="deployment-note">Background service: {receipt.service.public_summary.state}. {receipt.service.public_summary.definition_verified ? "Its owned service definition is verified." : "Its service definition has not been verified."}</p>}
    {deploymentProtectedFolderNote(receipt) && <p className="deployment-note">{deploymentProtectedFolderNote(receipt)}</p>}
    {receipt?.knowledge_managed && <p className="deployment-note">The local knowledge service runs separately from the application. Stop it without deleting its data with <code>.venv/bin/python knowledge/manage.py stop</code> in the project folder. Keep its original <code>knowledge/.env</code>, which contains the database and signing secrets. If those secrets are missing, restore the original private file from your backup before starting again. Changing provider keys in Forge cannot replace them.</p>}
    {busy && !selectionBlocked && busyOperation.current?.stagePresentation?.guideId === state.id &&
      busyOperation.current.stagePresentation.selectionToken === selectionToken && <div className="deployment-warning" role="status" data-stage-response="pending">
      <p>Forge is checking update compatibility. Nothing is being applied. Installation actions will be available when this check returns.</p>
      {!reconnecting && <button className="setup-action" type="button" onClick={refresh}>Check installation status</button>}
    </div>}
    {commandReturnCurrent(commandReturn) && <div className="deployment-warning" role="status">
      {commandReturn.owner.installSelection
        ? <p>Forge has checked that this installation has been admitted and refreshed its status. The original installation response is still pending.
          Returning uses the checked status without cancelling or sending the installation again.</p>
        : <p>Forge has checked that this command has finished and refreshed installation status. Its original command response is still pending.
          Returning uses the checked status while that response remains pending.</p>}
      <button className="setup-action" type="button" disabled={selectionBlocked} onClick={returnToCheckedInstallation}>Return to checked installation</button>
    </div>}
    {reconnecting && <div className="deployment-warning" role="status">
      <p>{connection.actionIssue || "Reconnecting to Forge for deployment status. Any saved receipt remains visible."} Installation commands stay paused until the saved command and current status can be checked; checks retry automatically.</p>
      <button className="setup-action" type="button" disabled={selectionBlocked} onClick={refresh}>Check installation status</button>
    </div>}
    {error && <p className="deployment-error" role="alert">{error}</p>}
    </>}
  </section>;
}

export function DeploymentUpdatePreparation({ receipt, build, state, options, disabled, onStage }) {
  const prepared = deploymentUpdatePreparation(receipt, build, state, options);
  if (!prepared.available && !prepared.limitation) return null;
  return <div className={prepared.background ? "deployment-update-preview" : "deployment-update-stage"}>
    {prepared.limitation && <p className="deployment-note">{prepared.limitation}</p>}
    {prepared.available && <button className={`setup-action${prepared.background && !prepared.activationSupported ? "" : " primary"}`}
      type="button" disabled={disabled} onClick={onStage}>{prepared.label}</button>}
  </div>;
}

/** Private setup stays beside the action that opens it, before later panels. */
export function DeploymentResumeAction({ receipt, state, disabled, onAction, setupDisabled = disabled,
  confirmed, pending, selectionScope, onSettings, onCheck }) {
  if (deploymentApplicationStartupPending(receipt, state)) return <DeploymentApplicationSetupHandoff
    receipt={receipt} state={state} disabled={setupDisabled} confirmed={confirmed} pending={pending}
    selectionScope={selectionScope} onAction={onAction} onSettings={onSettings} onCheck={onCheck} />;
  const resume = deploymentResumeAction(receipt, state);
  if (!resume) return null;
  return <div><button className="setup-action" disabled={disabled} onClick={() => onAction(resume.action)}>{resume.label}</button>
    {resume.detail && <p className="deployment-note">{resume.detail}</p>}</div>;
}

export function DeploymentApplicationSetupHandoff(props) {
  return <ApplicationSetup {...props} />;
}

export function DeploymentPrivateSettingsInstructions({ receipt }) {
  return <p>Leave configured fields blank to keep their installed values, or explicitly remove an optional saved value. Saving keeps this installed application stopped. {deploymentRequiresApplicationStartup(receipt)
    ? "Complete application setup before starting."
    : "Use Start installed application when you are ready."}</p>;
}

export function DeploymentInstallDescription({ nativeTeam, options }) {
  return <p>{deploymentFreshInstallNeedsSetup(options)
    ? "Forge installs the runtime and checks your application. Application setup follows; your agents do not start yet."
    : nativeTeam
    ? "This action installs the pinned runtime and starts the agents and tools in your accepted plan. It does not build custom application screens."
    : "This action downloads the pinned runtime and dependencies, runs the accepted application code, and starts the proposed agents. Remaining live integrations still require their own verification."}</p>;
}

export function DeploymentCleanupRecovery({ receipt, state, disabled, onAction }) {
  if (deploymentCanStopFailedBackground(receipt, state)) return <div>
    <button className="setup-action" disabled={disabled} onClick={() => onAction("stop")}>Stop and verify cleanup</button>
    <p className="deployment-note">Stop the owned background service and verify cleanup before correcting private settings. Your installed source and saved records stay in this folder; starting again is a separate action.</p>
  </div>;
  if (receipt?.guide_id !== state?.id || !deploymentNeedsCleanup(receipt)) return null;
  if (!deploymentCanConfirmCleanup(receipt, state)) return <p className="deployment-warning" role="status">
    {receipt.cleanup_recovery?.reason || "The failed runtime's cleanup is unconfirmed. Preserve this folder; settings and a fresh start remain blocked until cleanup can be verified."}
  </p>;
  return <div>
    <button className="setup-action" disabled={disabled} onClick={() => onAction("stop")}>Stop and verify cleanup</button>
    <p className="deployment-note">Verify cleanup of this failed start to unlock its private settings. Your installed source and saved records stay in this folder; starting again is a separate action.</p>
  </div>;
}

export function DeploymentInstallationArea({ children, configuration, receipt }) {
  return <><div className="deployment-actions">{children}</div>{!deploymentIsActive(receipt) && configuration}</>;
}

export function DeploymentSettingsGuidance({ receipt }) {
  if (receipt?.status !== "running") return null;
  return <p className="deployment-note">To change private settings, stop the application first. Forge checks its saved source and cleanup before opening setup.</p>;
}

/** A delayed options result must not focus another form or an unmounted page. */
export function revealDeploymentConfiguration(form, key) {
  if (!key || !form?.isConnected || form.dataset?.configurationKey !== key) return false;
  const heading = form.querySelector("[data-configuration-heading]");
  if (!heading || typeof heading.focus !== "function") return false;
  heading.focus({ preventScroll: true });
  form.scrollIntoView({ block: "start", behavior: "auto" });
  return true;
}

export function DeploymentUpdateActions({ receipt, build, state, disabled, onAction }) {
  const update = receipt?.update;
  if (!update) return null;
  const background = receipt.execution_mode === "background";
  const capability = deploymentBackgroundUpdate(receipt, state);
  const active = deploymentIsActive(receipt);
  const apply = deploymentUpdateAction(receipt, build, state, "apply-update");
  const recover = deploymentUpdateAction(receipt, build, state, "recover-current");
  const rollback = deploymentUpdateAction(receipt, build, state, "rollback");
  const needsStop = background ? capability.supported && !capability.stopped
    : active && (update.status === "staged" || update.rollback_available);
  return <>
    {needsStop && <p className="deployment-note">{background
      ? "Stop this background service and wait for confirmed native cleanup before changing its source. A missing service PID alone does not confirm cleanup."
      : "Stop the application before applying or rolling back its source."}</p>}
    <div className="deployment-actions">
      {needsStop && background && !active && <button className="setup-action" type="button" disabled={disabled}
        onClick={() => onAction("stop")}>Stop and confirm cleanup</button>}
      {apply && <button className="setup-action primary" type="button" disabled={disabled}
        onClick={() => onAction("apply-update")}>{update.status === "snapshotting" ? "Resume staged update" : "Apply and verify update"}</button>}
      {recover && <button className="setup-action primary" type="button" disabled={disabled}
        onClick={() => onAction("recover-current")}>Retry current application</button>}
      {rollback && <button className="setup-action" type="button" disabled={disabled}
        onClick={() => onAction("rollback")}>{update.status === "completed" ? "Restore previous application" : "Recover previous application"}</button>}
    </div>
  </>;
}

export function DeploymentRuntimeDetails({ consoleUrl, scope, Icon, disabled = false, children }) {
  if (!consoleUrl && !scope && !children) return null;
  return <details className="deployment-evidence deployment-interface-scope">
    <summary>Runtime and native Console details (optional)</summary>
    {children}
    {consoleUrl && <a className="setup-action" href={disabled ? undefined : consoleUrl} aria-disabled={disabled}
      tabIndex={disabled ? -1 : undefined} target="_blank" rel="noreferrer">Open native MobKit Console directly <Icon name="arrow" size={14} /></a>}
    {scope?.requested_interfaces && <p><strong>Requested experience:</strong> {scope.requested_interfaces}</p>}
    {scope && <p>{scope.authentication === "local-password"
      ? "Sign in with the Console password you chose for this installation. Your configured user identity is applied automatically. Your provider API key is separate."
      : "This accepted package uses a loopback-only native Console without a separate Console login. Keep remote access inside an authenticated SSH or IAP tunnel."}</p>}
  </details>;
}

export function DeploymentHandoff({ handoff }) {
  return <div className="deployment-handoff">
    <p className="deployment-warning"><strong>Remote deployment is not connected.</strong> Follow the location shown with each command; these steps do not require a remote connection from Forge. Forge has not provisioned or verified this destination.</p>
    <ol>{handoff.steps.map((step, index) => <li key={index}>
      <strong>{step.title}</strong><span className="deployment-field-help">{step.where}</span>
      <p>{step.description}</p>
      {step.commands.length > 0 && <pre>{step.commands.join("\n")}</pre>}
    </li>)}</ol>
    {handoff.operations?.length > 0 && <details className="deployment-evidence">
      <summary>Manage the installed application on the destination</summary>
      <p>Run only the command for the action you need, from this same extracted folder on the destination. These are reference actions, not additional installation steps. Service removal preserves private configuration and application/native data.</p>
      {handoff.operations.map((operation, index) => <div key={index}>
        <strong>{operation.title}</strong>
        {operation.description && <p>{operation.description}</p>}
        <pre>{operation.commands.join("\n")}</pre>
      </div>)}
    </details>}
    <details className="deployment-evidence" open={handoff.steps.length === 0}><summary>What still needs verification and access</summary><ul>{handoff.limitations.map((item, index) => <li key={index}>{item}</li>)}</ul></details>
  </div>;
}

export function DeploymentUpdateDiagnostics({ update }) {
  const diagnostics = deploymentUpdateDiagnostics(update);
  const archive = deploymentFailedAttempts(update);
  const history = diagnostics.historical ? diagnostics.entries : [];
  if (!diagnostics.entries.length && !archive.attempts.length && !archive.truncated && !archive.unavailable) return null;
  return <>
    {!diagnostics.historical && diagnostics.entries.map((entry) => <p key={entry.label} className="deployment-error">{entry.label}: {entry.detail}</p>)}
    {(history.length > 0 || archive.attempts.length > 0 || archive.truncated > 0 || archive.unavailable) && <details className="deployment-evidence">
      <summary>Earlier startup and recovery attempts</summary>
      <p>The current result is shown above. These messages and logs are earlier diagnostic output; they do not establish current health, cleanup or an application result.</p>
      {history.length > 0 && <ul>{history.map((entry) => <li key={entry.label}><strong>{entry.label}: </strong>{entry.detail}</li>)}</ul>}
      {archive.attempts.map((attempt) => <div key={attempt.id}>
        <p><strong>{attempt.source === "candidate" ? "Earlier candidate startup" : "Earlier previous-source recovery"}</strong></p>
        <p>Attempt <code>{attempt.id}</code>, build <code>{attempt.build_id}</code>. Captured {attempt.captured_at}.</p>
        {attempt.error && <p>{attempt.error}</p>}
        {attempt.logs.length > 0 && <details className="deployment-evidence">
          <summary>Redacted output ({attempt.logs.length} {attempt.logs.length === 1 ? "entry" : "entries"})</summary>
          <pre>{attempt.logs.map((entry) => `[${entry.phase}] ${entry.text}`).join("\n")}</pre>
        </details>}
        {attempt.logs_truncated && <p>Some earlier output was omitted to keep this record bounded.</p>}
      </div>)}
      {archive.truncated > 0 && <p>{archive.truncated} older attempt records were omitted.</p>}
      {archive.unavailable && <p>Some saved diagnostics could not be matched to this update and are not shown.</p>}
    </details>}
  </>;
}

export function DeploymentConfigurationFields({ options, values, useHost, guideId, busy, onChange, removed = [], onRemove }) {
  const missing = deploymentRequiredFields(options, values, useHost);
  const issues = deploymentFieldIssues(options, values, removed);
  const issueByKey = new Map(issues.map((issue) => [issue.key, issue]));
  const missingKeys = new Set(missing.map((field) => field.key));
  const removable = new Set(onRemove ? deploymentRemovableFields(options).map((field) => field.key) : []);
  const removal = new Set(removed);
  const fields = (options?.fields || []).filter((field) => !field.automatic);
  const renderField = (field) => {
    const id = deploymentFieldId(guideId, field.key), needsInput = missingKeys.has(field.key);
    const issue = issueByKey.get(field.key);
    const describedBy = [field.description && `${id}-description`, needsInput && `${id}-required`, issue && `${id}-constraint`].filter(Boolean).join(" ") || undefined;
    const removing = removable.has(field.key) && removal.has(field.key);
    return <React.Fragment key={field.key}><label htmlFor={id}>{field.label} <small>{field.required ? "Required" : "Optional"}</small>
      <input id={id} name={field.key} type={field.secret ? "password" : "text"} value={values[field.key] || ""}
        placeholder={removing ? "Saved value marked for removal" : field.configured ? "Already configured; leave blank to keep" : field.secret ? "Stored only in this deployment" : field.default || ""}
        required={needsInput} aria-required={needsInput} aria-describedby={describedBy}
        aria-invalid={issue ? true : undefined} inputMode={field.type === "integer" && !field.secret ? "numeric" : undefined}
        onChange={(event) => onChange(field.key, event.target.value)}
        autoComplete={field.secret ? "new-password" : "off"} spellCheck="false" disabled={busy || removing} />
      {field.description && <span id={`${id}-description`} className="deployment-field-help">{field.description}</span>}
      {needsInput && <span id={`${id}-required`} className="deployment-field-help">Enter {field.label} to continue.</span>}
      {issue && <span id={`${id}-constraint`} className="deployment-field-help">{issue.detail}</span>}
    </label>{removable.has(field.key) && <label className="deployment-remove" htmlFor={`${id}-remove`}>
      <input id={`${id}-remove`} type="checkbox" checked={removing} disabled={busy}
        onChange={(event) => onRemove(field.key, event.target.checked)} />
      <span>Remove the saved {field.label} value when saving. Application defaults may still apply.</span>
    </label>}</React.Fragment>;
  };
  const optional = fields.filter((field) => !field.required);
  return <>
    {missing.length > 0 && <p className="deployment-warning" role="status" aria-live="polite">
      Complete the required settings to continue: {missing.map((field, index) => <React.Fragment key={field.key}>
        {index > 0 && ", "}<a href={`#${deploymentFieldId(guideId, field.key)}`}>{field.label}</a>
      </React.Fragment>)}.
    </p>}
    {issues.length > 0 && <p className="deployment-warning" role="status" aria-live="polite">
      Check these settings before continuing: {issues.map((issue, index) => <React.Fragment key={issue.key}>
        {index > 0 && ", "}<a href={`#${deploymentFieldId(guideId, issue.key)}`}>{issue.label}</a>
      </React.Fragment>)}.
    </p>}
    {fields.filter((field) => field.required).map(renderField)}
    {optional.length > 0 && <details className="deployment-optional-settings">
      <summary>Optional settings ({optional.length})</summary>
      <p className="deployment-field-help">You can leave these unchanged. Open them to customize this installation.</p>
      {optional.map(renderField)}
    </details>}
  </>;
}
