import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from "./guide-selection.mjs";
import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { readDeploymentSetup } from "./deployment-setup-read.mjs";
import { createDeploymentPollingGate } from "./deployment-panel.mjs";
import { remoteDeploymentUrl, remoteSelectionKey, remotePrivateDraftScope, remoteCanConnect, remoteConnectionInput, remoteReceiptOwned,
  remoteReceiptBinding, remoteReceiptCurrent, remoteObservationAvailable, remotePrivateOptions, remoteCanConfigure,
  remotePendingBuildHandoff,
  remoteCanRetryInstall, remotePrivateBinding, remotePrivateDraftBound, remotePrivateDraftRetained, remoteFailureGuidance, remoteCanInstall, remoteActionAvailable,
  remoteSetupConsolePortValue, remoteSetupPortObservation, remoteSetupInstallSettings,
  remoteIsBusy, remoteInstallAdmitted, remoteDeploymentChecks, remoteDeploymentActivity, remoteDeploymentStep,
  remoteNativeReady, remotePhase, remoteCloudRemoval, remoteProductSelected, remoteActionBody, remoteControlBindingKey, remoteProductActionSettled, remoteProductControl } from "./remote-deployment.mjs";
import "./remote-deployment.css";
import { RemoteBrowserAccess } from "./remote-browser-access.jsx";
import { RemoteApplicationAccess } from "./remote-application-access.jsx";
import { RemoteInstalledUse } from "./remote-installed-use.jsx";
import { CloudResourceGuidance } from "./gcp-resource-guidance.jsx";
import { RemotePrivateSettings } from "./remote-private-settings.jsx";
import { RemoteApplicationSetup } from "./remote-application-setup.jsx";

export function RemoteDeployment({ state, build, options, plan, blocked = false, allowConnection = true, cloudRemoval = null, renderConfiguration, onRepairReceipt, onReceipt, targetRef, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const [saved, setSaved] = useState({ guideId: null, selectionToken: null, receipt: null, observation: "loading" });
  const [draft, setDraft] = useState({ host: "", user: "", ssh_port: "22", destination: `~/src/${plan.project.slug}` });
  const [values, setValues] = useState({}), [formBinding, setFormBinding] = useState("");
  const [busy, setBusy] = useState(""), [error, setError] = useState(null);
  const [unavailable, setUnavailable] = useState(false), [loaded, setLoaded] = useState(false);
  const [firstTaskHandoff, setFirstTaskHandoff] = useState(null);
  const [settingsEditor, setSettingsEditor] = useState(null), [settingsOpenRequest, setSettingsOpenRequest] = useState(null);
  const [portCheck, setPortCheck] = useState(null), [portCheckRevision, setPortCheckRevision] = useState(0);
  const gate = useRef(null), timer = useRef(null), latest = useRef(null), formRef = useRef(null), observationSequence = useRef(0);
  const connectionRequest = useRef(new Map()), handoffLifetime = useRef({ key: "", revision: 0 });
  const privateSettingsOperation = useRef(null);
  if (!gate.current) gate.current = createDeploymentPollingGate();
  const receipt = saved.guideId === state.id && remoteReceiptOwned(saved.receipt, state) ? saved.receipt : null;
  const removalSignal = receipt?.target === "gcp" && cloudRemoval?.guideId === state.id &&
    cloudRemoval.operationId === receipt.cloud_operation_id && cloudRemoval.operationId === receipt.endpoint.operation_id ? cloudRemoval : null;
  const removalStatus = remoteCloudRemoval(receipt) || removalSignal?.status;
  const cloudRemoving = !!remoteCloudRemoval(receipt) || removalSignal?.pending === true;
  const observation = saved.guideId !== state.id || saved.selectionToken !== selectionToken || selectionBlocked || busy ||
    removalSignal?.pending === true && !remoteCloudRemoval(receipt)
    ? "loading" : unavailable ? "unknown" : saved.observation;
  useEffect(() => { onReceipt?.(receipt, { guideId: state.id, selectionToken, observation }); },
    [receipt, state.id, selectionToken, observation, onReceipt]);
  blocked = blocked || !allowConnection || cloudRemoving;
  const cloudMode = receipt ? receipt.target === "gcp" : plan.target === "gcp";
  const selection = remoteSelectionKey(state, build), binding = receipt ? remoteReceiptBinding(receipt) : "";
  const settingsEditing = settingsEditor?.open === true && settingsEditor.guideId === state.id && settingsEditor.binding === binding;
  const privateBinding = receipt ? remotePrivateBinding(receipt) : "";
  const canConnect = remoteCanConnect(options, build, state, blocked);
  const configure = remoteCanConfigure(receipt, options, build, state, blocked);
  const retry = remoteCanRetryInstall(receipt, options, build, state, blocked);
  const privateOptions = remotePrivateOptions(receipt);
  const draftBound = remotePrivateDraftBound(receipt, options, build, state, formBinding);
  const draftRetained = remotePrivateDraftRetained(receipt, options, build, state, formBinding);
  const privateDraftScope = remotePrivateDraftScope(state, build);
  const formOpen = allowConnection && !cloudRemoving && draftBound && remoteObservationAvailable(receipt) && !!privateOptions;
  const formReady = configure && !unavailable;
  const portValue = remoteSetupConsolePortValue(receipt, options, build, state, values);
  const portValid = typeof portValue === "string" && /^[1-9][0-9]{3,4}$/.test(portValue) &&
    Number(portValue) >= 1024 && Number(portValue) <= 65535;
  const portCheckKey = formOpen && portValid ? JSON.stringify([selection, privateBinding, formBinding, options.console_port, portValue, portCheckRevision]) : "";
  const checkedPort = formReady && portCheck?.key === portCheckKey ? portCheck : null;
  const portReady = portValue === null || portValid && checkedPort?.observation?.status === "available";
  const current = remoteReceiptCurrent(receipt, options, build, state);
  const productExecutionMode = !unavailable && remoteProductControl(receipt, state)?.factory_status?.execution_mode;
  const failureGuidance = remoteFailureGuidance(receipt);
  const destinationCheckFailure = receipt?.status === "failed" && !unavailable && remoteObservationAvailable(receipt) &&
    receipt.remote_admission_requested === false && receipt.preflight?.ready === false &&
    Array.isArray(receipt.preflight.checks) && receipt.preflight.checks.some((check) => check?.status === "failed") &&
    ["admitted", "worker_live", "service_owned", "service_observed"].every((key) => receipt[key] == null || receipt[key] === false) &&
    ["worker_pid", "service", "operation_id", "install_operation_id", "pending_operation", "retry_of_operation_id", "previous_operation_id"]
      .every((key) => receipt[key] == null);
  const deploymentStep = remoteDeploymentStep(receipt, unavailable || cloudRemoving);
  const activity = remoteDeploymentActivity(receipt?.progress);
  const pendingHandoff = unavailable ? null : remotePendingBuildHandoff(receipt, options, build, state, blocked);
  const handoffKey = JSON.stringify([state.id, pendingHandoff?.key || null]);
  if (handoffLifetime.current.key !== handoffKey) {
    handoffLifetime.current = { key: handoffKey, revision: handoffLifetime.current.revision + 1 };
  }
  const handoffRevision = handoffLifetime.current.revision;
  latest.current = { state, build, options, blocked, receipt, selection, busy, unavailable, pendingHandoff, handoffRevision,
    formOpen, formReady, portCheckKey, portValue };
  useEffect(() => () => { handoffLifetime.current.revision += 1; }, []);

  function publish(next, identity) {
    if (!remoteReceiptOwned(next, latest.current.state) || identity !== latest.current.state.id) return false;
    setSaved({ guideId: identity, selectionToken: selectionScope?.token(), receipt: next, observation: "known" });
    setUnavailable(false); setLoaded(true);
    setDraft((previous) => !previous.host && !previous.user ? { ...next.endpoint,
      ssh_port: String(next.endpoint.ssh_port), destination: next.destination } : previous);
    setError((previous) => {
      const command = previous?.command;
      if (command?.ownerCurrent() && gate.current.sameScope(command.token, identity) &&
          command.guideId === identity && command.action === previous.kind &&
          command.selection === latest.current.selection &&
          command.binding === remoteReceiptBinding(latest.current.receipt) && command.binding === remoteReceiptBinding(next) &&
          command.controlBinding && command.controlBinding === remoteControlBindingKey(latest.current.receipt?.product_control?.binding) &&
          command.controlBinding === remoteControlBindingKey(next?.product_control?.binding) &&
          remoteProductActionSettled(next, latest.current.state, command.requestId, command.action)?.outcome === "returned") return null;
      return remoteInstallAdmitted(next) && ["install", "retry"].includes(previous?.kind) &&
        previous.deploymentId === next.deployment_id ? null : previous;
    });
    return true;
  }

  function settingsBusy(kind, job) {
    if (kind) {
      if (job.guideId !== latest.current.state.id || latest.current.busy && privateSettingsOperation.current !== job) return false;
      privateSettingsOperation.current = job;
      setBusy(`settings-${kind}`);
    } else if (privateSettingsOperation.current === job) {
      privateSettingsOperation.current = null;
      setBusy((previous) => previous.startsWith("settings-") ? "" : previous);
    }
    return true;
  }

  async function refresh(expectedSelectionToken) {
    const ownerCurrent = captureGuideSelection(selectionScope, latest.current.state, expectedSelectionToken);
    if (!ownerCurrent) return;
    const identity = latest.current.state.id;
    const sequence = ++observationSequence.current;
    const observed = await gate.current.observe(identity, () => request(remoteDeploymentUrl(identity)));
    if (!ownerCurrent() || sequence !== observationSequence.current || !observed || !gate.current.current(observed.token, latest.current.state.id)) return;
    if (observed.failure) {
      setLoaded(true);
      if (observed.failure.status === 404) {
        // A previously admitted deployment is preserved during uncertain reads.
        if (!latest.current.receipt) { setSaved({ guideId: identity, selectionToken: selectionScope?.token(), receipt: null, observation: "known" }); setUnavailable(false); }
        else setUnavailable(true);
      } else setUnavailable(true);
    } else if (!publish(observed.receipt, identity)) setUnavailable(true);
    if (observed.failure && (observed.failure.status !== 404 || latest.current.receipt) ||
        !observed.failure && !remoteReceiptOwned(observed.receipt, latest.current.state))
      setSaved((previous) => ({ guideId: identity, selectionToken: selectionScope?.token(),
        receipt: previous.guideId === identity ? previous.receipt : null, observation: "unknown" }));
  }

  useEffect(() => {
    const scope = gate.current.open(state.id);
    setSaved({ guideId: state.id, selectionToken, receipt: null, observation: "loading" }); setValues({}); setFormBinding("");
    setDraft({ host: "", user: "", ssh_port: "22", destination: `~/src/${plan.project.slug}` });
    setBusy(""); setError(null); setUnavailable(false); setLoaded(false);
    let stopped = false;
    async function poll() {
      if (stopped) return;
      await refresh();
      if (!stopped) timer.current = setTimeout(poll, 2000);
    }
    poll();
    return () => { stopped = true; clearTimeout(timer.current); gate.current.close(scope); };
  }, [state.id]);

  useEffect(() => { setValues({}); setFormBinding(""); }, [privateDraftScope]);
  useEffect(() => {
    if (receipt?.status === "not-installed") { setValues({}); setFormBinding(""); }
  }, [receipt?.deployment_id, receipt?.status]);
  useEffect(() => {
    if (!formBinding || draftRetained) return;
    setValues({}); setFormBinding("");
  }, [formBinding, draftRetained]);
  useEffect(() => {
    if (!formOpen) return;
    formRef.current?.querySelector("h4")?.focus({ preventScroll: true });
  }, [formOpen]);

  useEffect(() => {
    if (formOpen && portValue !== null && !Object.hasOwn(values, "MOBKIT_PORT"))
      setValues((previous) => Object.hasOwn(previous, "MOBKIT_PORT") ? previous : { ...previous, MOBKIT_PORT: portValue });
  }, [formOpen, portValue, values]);

  useEffect(() => {
    if (selectionBlocked) return;
    if (!portCheckKey || !formReady) { setPortCheck(null); return; }
    let cancelled = false;
    const ownerCurrent = captureGuideSelection(selectionScope, state);
    if (!ownerCurrent) return;
    setPortCheck({ key: portCheckKey, checking: true, observation: null });
    const query = new URLSearchParams({ build_id: build.build_id, console_port: portValue });
    readDeploymentSetup(`/api/guide/${encodeURIComponent(state.id)}/deployment/options?${query}`).then((checkedOptions) => {
      const current = latest.current;
      if (cancelled || !ownerCurrent() || current.portCheckKey !== portCheckKey || !current.formOpen || !current.formReady) return;
      const observation = remoteSetupPortObservation(checkedOptions, current.receipt, current.options,
        current.build, current.state, current.portValue, current.blocked);
      setPortCheck({ key: portCheckKey, checking: false, observation });
    }).catch(() => {
      if (!cancelled && ownerCurrent() && latest.current.portCheckKey === portCheckKey)
        setPortCheck({ key: portCheckKey, checking: false, observation: null });
    });
    return () => { cancelled = true; };
  }, [portCheckKey, formReady, selectionToken, selectionBlocked]);

  async function connect(event) {
    event.preventDefault();
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const input = remoteConnectionInput(draft);
    if (!input || !canConnect || busy || latest.current.unavailable || connectionRequest.current.has(state.id) || remoteIsBusy(receipt) || ["running", "recovery-needed"].includes(receipt?.status)) return;
    const selected = { guideId: state.id, selection, token: gate.current.capture(state.id), input };
    connectionRequest.current.set(selected.guideId, selected);
    setBusy("connect"); setError(null); setValues({}); setFormBinding("");
    try {
      const next = await request(remoteDeploymentUrl(selected.guideId, "connect"), { method: "POST", body: {
        version: state.version, build_id: build.build_id, ...input,
      } });
      if (!ownerCurrent() || selected.selection !== latest.current.selection || !remoteReceiptCurrent(next, latest.current.options,
          latest.current.build, latest.current.state) || next.endpoint.host !== input.host || next.endpoint.user !== input.user ||
          next.endpoint.ssh_port !== input.ssh_port || !gate.current.admit(selected.token, latest.current.state.id)) return;
      publish(next, selected.guideId);
      if (remoteCanConfigure(next, latest.current.options, latest.current.build, latest.current.state, latest.current.blocked))
        setFormBinding(remotePrivateBinding(next));
    } catch (failure) {
      if (ownerCurrent() && selected.selection === latest.current.selection && gate.current.current(selected.token, latest.current.state.id)) {
        setError({ kind: "connect", text: failure.message || "SSH access could not be checked. Your destination details remain here." });
        setUnavailable(true);
        refresh();
      }
    } finally {
      if (connectionRequest.current.get(selected.guideId) === selected) connectionRequest.current.delete(selected.guideId);
      if (gate.current.sameScope(selected.token, latest.current.state.id)) setBusy("");
    }
  }

  async function checkUpdatedApplication(handoff, revision) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const stillSelected = () => ownerCurrent() && latest.current.handoffRevision === revision &&
      latest.current.pendingHandoff?.key === handoff?.key;
    if (!handoff || !stillSelected() || latest.current.busy || connectionRequest.current.has(state.id)) return;
    const selected = { ...handoff, token: gate.current.capture(handoff.guideId) };
    if (!selected.token) return;
    connectionRequest.current.set(selected.guideId, selected);
    setBusy("connect-updated"); setError(null); setValues({}); setFormBinding("");
    setDraft({ ...selected.input, ssh_port: String(selected.input.ssh_port) });
    try {
      const next = await request(remoteDeploymentUrl(selected.guideId, "connect"), { method: "POST", body: {
        version: selected.version, build_id: selected.buildId, ...selected.input,
      } });
      if (!stillSelected() || !remoteReceiptCurrent(next, latest.current.options, latest.current.build, latest.current.state) ||
          next.deployment_id === selected.previousDeploymentId || next.destination !== selected.input.destination ||
          next.endpoint.host !== selected.input.host || next.endpoint.user !== selected.input.user ||
          next.endpoint.ssh_port !== selected.input.ssh_port || !gate.current.admit(selected.token, latest.current.state.id)) return;
      publish(next, selected.guideId);
      if (remoteCanConfigure(next, latest.current.options, latest.current.build, latest.current.state, latest.current.blocked))
        setFormBinding(remotePrivateBinding(next));
    } catch (failure) {
      if (stillSelected() && gate.current.current(selected.token, latest.current.state.id)) {
        setError({ kind: "connect-updated", text: failure.message || "The updated application could not be checked. Your destination details remain here." });
        setUnavailable(true);
        refresh();
      }
    } finally {
      if (connectionRequest.current.get(selected.guideId) === selected) connectionRequest.current.delete(selected.guideId);
      if (gate.current.sameScope(selected.token, latest.current.state.id)) setBusy("");
    }
  }

  async function install(event) {
    event.preventDefault();
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (!formOpen || !formReady || !portReady || busy || !remoteCanInstall(receipt, options, build, state, values, blocked)) return;
    const selected = { guideId: state.id, selection, binding, deploymentId: receipt.deployment_id,
      previousOperationId: retry ? receipt.retry_of_operation_id : null, token: gate.current.capture(state.id) };
    const settings = remoteSetupInstallSettings(receipt, options, build, state, values, blocked);
    setBusy(retry ? "retry" : "install"); setError(null);
    setValues({}); setFormBinding("");
    try {
      const next = await request(remoteDeploymentUrl(selected.guideId, selected.previousOperationId ? "retry" : "install"), { method: "POST", body: {
        version: state.version, build_id: build.build_id, deployment_id: selected.deploymentId,
        ...(selected.previousOperationId ? { previous_operation_id: selected.previousOperationId } : {}), settings,
      } });
      if (!ownerCurrent() || selected.selection !== latest.current.selection || selected.binding !== remoteReceiptBinding(latest.current.receipt) ||
          remoteReceiptBinding(next) !== selected.binding) return;
      if (selected.previousOperationId &&
          (next.operation_id !== selected.previousOperationId && next.previous_operation_id !== selected.previousOperationId ||
           latest.current.receipt?.operation_id !== selected.previousOperationId && latest.current.receipt?.operation_id !== next.operation_id)) return;
      if (!gate.current.admit(selected.token, latest.current.state.id)) return;
      if (publish(next, selected.guideId)) { setValues({}); setFormBinding(""); }
    } catch {
      if (ownerCurrent() && selected.selection === latest.current.selection && gate.current.current(selected.token, latest.current.state.id)) {
        setError({ kind: selected.previousOperationId ? "retry" : "install", deploymentId: selected.deploymentId,
          text: `Forge could not confirm the ${selected.previousOperationId ? "retry" : "installation"} response. Checking its saved status before another action.` });
        setUnavailable(true);
        refresh();
      }
    } finally { if (gate.current.sameScope(selected.token, latest.current.state.id)) setBusy(""); }
  }

  async function action(name) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (busy || cloudRemoving || name !== "stop" && latest.current.unavailable || !remoteActionAvailable(receipt, name, options, build, state, blocked)) return;
    const selected = { guideId: state.id, selection, binding, deploymentId: receipt.deployment_id,
      token: gate.current.capture(state.id), controlBinding: remoteControlBindingKey(receipt?.product_control?.binding) };
    const packet = remoteActionBody(receipt, name, options, build, state,
      remoteProductSelected(receipt) ? crypto.randomUUID() : undefined, blocked);
    if (!packet) return;
    setBusy(name); setError(null); setValues({}); setFormBinding("");
    try {
      const next = await request(remoteDeploymentUrl(selected.guideId, "action"), { method: "POST",
        body: packet });
      if (!ownerCurrent() || name !== "stop" && selected.selection !== latest.current.selection ||
          selected.binding !== remoteReceiptBinding(latest.current.receipt) || remoteReceiptBinding(next) !== selected.binding ||
          selected.controlBinding !== remoteControlBindingKey(latest.current.receipt?.product_control?.binding) ||
          selected.controlBinding !== remoteControlBindingKey(next?.product_control?.binding) || !gate.current.admit(selected.token, latest.current.state.id)) return;
      publish(next, selected.guideId);
    } catch {
      if (ownerCurrent() && gate.current.current(selected.token, latest.current.state.id)) {
        setError({ kind: name, text: `Forge could not confirm ${name === "stop" ? "the stop" : "this action"}. Recheck the owned application's status before retrying.`,
          ...(packet.action_request_id ? { command: { ...selected, action: name, requestId: packet.action_request_id, ownerCurrent } } : {}) });
        setUnavailable(true);
        refresh();
      }
    } finally { if (gate.current.sameScope(selected.token, latest.current.state.id)) setBusy(""); }
  }

  const endpointLocked = !!busy || remoteIsBusy(receipt) || ["running", "recovery-needed"].includes(receipt?.status);
  if (!allowConnection && !receipt && (!loaded || !unavailable)) return null;
  const destinationForm = <form className="remote-destination-form" onSubmit={connect} autoComplete="off">
      <label htmlFor={`remote-host-${state.id}`}>Computer address<input id={`remote-host-${state.id}`} name="host" value={draft.host}
        required spellCheck="false" placeholder="For example, workshop.local" disabled={selectionBlocked || endpointLocked}
        onChange={(event) => setDraft({ ...draft, host: event.target.value })} /></label>
      <label htmlFor={`remote-user-${state.id}`}>SSH login<input id={`remote-user-${state.id}`} name="user" value={draft.user}
        required spellCheck="false" placeholder="Your Linux user name" disabled={selectionBlocked || endpointLocked}
        onChange={(event) => setDraft({ ...draft, user: event.target.value })} /></label>
      <label htmlFor={`remote-folder-${state.id}`}>Project folder on that computer<input id={`remote-folder-${state.id}`} name="destination"
        value={draft.destination} required spellCheck="false" disabled={selectionBlocked || endpointLocked}
        onChange={(event) => setDraft({ ...draft, destination: event.target.value })} /></label>
      <details className="remote-connection-overrides"><summary>Connection options</summary>
        <label htmlFor={`remote-port-${state.id}`}>SSH port<input id={`remote-port-${state.id}`} name="ssh_port" value={draft.ssh_port}
          inputMode="numeric" required disabled={selectionBlocked || endpointLocked}
          onChange={(event) => setDraft({ ...draft, ssh_port: event.target.value })} /></label>
      </details>
      <div className="deployment-actions"><button className="setup-action primary" type="submit"
        disabled={selectionBlocked || unavailable || !canConnect || endpointLocked || !remoteConnectionInput(draft)}>{busy === "connect" ? "Checking SSH access..." : "Connect and check destination"}</button></div>
      <p className="deployment-note">This checks access and prerequisites. It does not install the application, request an SSH password or enable services with sudo.</p>
    </form>;
  return <section ref={targetRef} aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)} id={cloudMode ? `remote-cloud-${state.id}` : undefined} tabIndex={cloudMode ? -1 : undefined} className="remote-deployment" aria-label={cloudMode ? "Guided cloud application installation" : "Guided Linux deployment"} data-remote-status={receipt?.status || "not-connected"}>
    <span className="setup-eyebrow">{cloudMode ? "YOUR CLOUD APPLICATION" : "YOUR LINUX COMPUTER"}</span>
    <h3>{cloudRemoving ? removalStatus === "removed" ? "This cloud installation's resources were removed" : "Cloud removal is requested" : pendingHandoff ? "Check your updated application on this computer" : destinationCheckFailure ? "Destination checks need attention" : remotePhase(receipt)}</h3>
    <p>{cloudRemoving ? "Application access, installation and restart are withdrawn for the saved cloud removal request. Its original installation receipt remains below."
      : pendingHandoff ? "Keep your chosen computer and project folder. You can choose another destination below."
      : cloudMode ? "Use the verified cloud computer above, then enter private application settings here. Forge checks its SSH identity and Linux prerequisites before installation." : "Use an existing SSH key or agent and a host already trusted by this computer. Forge checks the destination before asking for private application settings."}</p>
    {!canConnect && <p className="deployment-note">A current independently accepted application for this destination is needed before connecting or installing. Any saved owned deployment remains inspectable below.</p>}
    {canConnect && !cloudMode && !pendingHandoff && destinationForm}
    {receipt && <div className="remote-owned-installation">
      <p className="deployment-location"><strong>{receipt.endpoint.user}@{receipt.endpoint.host}:{receipt.endpoint.ssh_port}</strong><code>{receipt.destination}</code></p>
      {receipt.detail && <p>{cloudRemoving && <strong>Last application observation: </strong>}{receipt.detail}</p>}
      {deploymentStep && <p className="deployment-note remote-deployment-step" role="status" data-deployment-phase={deploymentStep.phase}>
        <strong>{deploymentStep.caption}:</strong> {deploymentStep.label}
      </p>}
      {!current && !pendingHandoff && !cloudRemoving && <p className="deployment-warning">This owned deployment belongs to an earlier proposal or build. Its status and stop action remain available. Private installation and restart require the matching accepted source.</p>}
      {pendingHandoff && <div className="remote-recovery remote-build-handoff">
        <h4>Your updated application is ready to check</h4>
        <p>Use the current checked application on this computer, in the same project folder. Forge will check the destination again, then ask for fresh private settings. This does not install or start it.</p>
        <div className="deployment-actions"><button className="setup-action primary" type="button" disabled={selectionBlocked || !!busy}
          onClick={() => checkUpdatedApplication(pendingHandoff, handoffRevision)}>{busy === "connect-updated" ? "Checking updated application..." : "Check the updated application here"}</button></div>
      </div>}
      {(!remoteObservationAvailable(receipt) || unavailable) && <p className="deployment-warning" role="status">Remote observation is unavailable. {receipt.last_known_status ? `Last reported state: ${receipt.last_known_status}. ` : ""}This does not prove the application failed or stopped.</p>}
      {receipt.status === "recovery-needed" && !cloudRemoving && <p className="deployment-warning" role="status">Cleanup of the owned remote operation is uncertain. Preserve this destination. Installation and restart remain blocked until the backend verifies cleanup; a missing SSH connection does not establish a stopped application.</p>}
      {receipt.status === "failed" && !pendingHandoff && !cloudRemoving && <div className="remote-recovery" role="status"><h4>{destinationCheckFailure ? "Destination checks need attention" : "Recover this installation"}</h4>
        {destinationCheckFailure ? <p>Fix the failed items under Destination checks on this same {cloudMode ? "cloud VM" : "Linux computer"}, then choose Recheck remote status. Keep this destination and project folder.</p> : <>
        {failureGuidance && <p><strong>{failureGuidance.label}</strong> {failureGuidance.detail}</p>}
        {receipt.retry_preflight_ready === false && <p>The latest retry check found an unmet prerequisite. Address the items under Retry prerequisites, then prepare another attempt.</p>}
        {retry && !unavailable
          ? <p>Forge verified that the previous attempt ended and its worker was cleaned up. Review the checks and activity below, fix any unmet prerequisite on your Linux computer, then enter private settings again to retry the same accepted application in this folder.</p>
          : <p>{!remoteObservationAvailable(receipt) || unavailable
            ? "The previous attempt reported a failure, but Forge cannot verify its current state. Recheck remote status before starting another attempt."
            : receipt.worker_live !== false || receipt.cleanup_confirmed !== true
              ? "Cleanup of the previous attempt has not been verified. Preserve this folder and recheck remote status before another installation."
              : !current || receipt.source_current === false
                ? "Retry needs this installation's exact accepted source. Preserve this folder and return to its matching accepted build before another attempt."
                : "This attempt has not been cleared for another installation. Recheck remote status and the checks below. Use the owned stop action if it is available, then recheck."}</p>}
        {busy === "retry" && <p>Sending your deliberate retry. Saved remote status will confirm whether the destination admitted it.</p>}
        </>}
      </div>}
      {receipt.status === "running" && !cloudRemoving && (!remoteNativeReady(receipt) || unavailable) && <p className="deployment-warning" role="status">Native readiness is unverified. {receipt.source_current === false ? "The installed source differs from the accepted package. " : ""}A running service alone does not verify its source or health. Recheck this owned deployment before using it.</p>}
      <RemoteChecks checks={receipt.preflight?.checks} title="Destination checks" />
      <RemoteChecks checks={receipt.retry_checks} title="Retry prerequisites" />
      {draftRetained && !formReady && <p className="deployment-note" role="status">Your private setup draft is kept on this page. Setup resumes when the same application and destination are confirmed.</p>}
      {busy === "install" && <p role="status">Sending your installation request. Saved remote status will confirm whether the destination admitted it.</p>}
      {configure && !formOpen && !unavailable && <button className="setup-action primary" type="button" disabled={selectionBlocked || !!busy}
        onClick={() => { setValues({}); setFormBinding(privateBinding); }}>{retry ? "Prepare another install attempt" : receipt.status === "not-installed" ? "Retry private setup" : "Continue to private setup"}</button>}
      {formOpen && <form className="deployment-form remote-private-form" ref={formRef} onSubmit={install} autoComplete="off">
        <h4 tabIndex={-1}>{retry ? "Private settings for another installation attempt" : cloudMode ? "Private settings for this cloud installation" : "Private settings for this Linux installation"}</h4>
        {retry && <p>Enter private settings again for this attempt. Forge will retry the same accepted application in this folder.</p>}
        <p>These fields come from this exact accepted package. Values are sent directly to the destination's private configuration over SSH, separate from guide messages and exported source.</p>
        {renderConfiguration({ options: privateOptions, values, useHost: false, guideId: `remote-${state.id}`,
          busy: selectionBlocked || !!busy || !formReady, onChange: (key, value) => setValues((previous) => ({ ...previous, [key]: value })) })}
        {portValue === null && Object.hasOwn(values, "MOBKIT_PORT") && <p className="deployment-warning" role="status">Your port choice is kept in this draft. Setup needs the matching accepted package's port information before continuing.</p>}
        {portValue === null && !Object.hasOwn(values, "MOBKIT_PORT") && receipt.remote_admission_requested === false && receipt.admitted !== true &&
          <p className="deployment-warning" role="status">Setup needs the matching accepted package's Console port before continuing. Reload its setup information and recheck remote status.</p>}
        {portValue !== null && <div className="remote-console-port">
          <label htmlFor={`remote-console-port-${state.id}`}>Native Console port
            <input id={`remote-console-port-${state.id}`} name="MOBKIT_PORT" type="text" inputMode="numeric"
              value={portValue} disabled={selectionBlocked || !!busy || !formReady} aria-describedby={`remote-console-port-help-${state.id}`}
              onChange={(event) => setValues((previous) => ({ ...previous, MOBKIT_PORT: event.target.value }))} /></label>
          <p id={`remote-console-port-help-${state.id}`} className="deployment-note">Console uses this port on your Linux computer and for private browser access here.</p>
          <p className={checkedPort?.observation?.status === "occupied" ? "deployment-warning" : "deployment-note"} role="status">
            {!portValid ? "Choose a port from 1024 to 65535."
              : !checkedPort || checkedPort.checking ? "Checking this computer's port..."
              : checkedPort.observation?.status === "available" ? "Available on this computer when checked. Linux startup is checked after installation."
              : checkedPort.observation?.status === "occupied" ? "An app on this computer already uses this port. Choose another to keep it running."
              : "This computer's port could not be checked. Try checking again."}</p>
          <div className="deployment-actions">
            {checkedPort?.observation?.alternatePort && <button className="setup-action" type="button" disabled={selectionBlocked || !!busy || !formReady}
              onClick={() => setValues((previous) => ({ ...previous, MOBKIT_PORT: String(checkedPort.observation.alternatePort) }))}>Use port {checkedPort.observation.alternatePort}</button>}
            <button className="setup-action" type="button" disabled={selectionBlocked || !!busy || !formReady || !portValid || checkedPort?.checking}
              onClick={() => setPortCheckRevision((previous) => previous + 1)}>Check this port again</button>
          </div>
          <p className="deployment-note">This check does not reserve the port. Availability can change before you open Console.</p>
        </div>}
        <p className="deployment-private">{receipt.product_runtime === true
          ? "Forge installs the accepted application's source and private settings in your chosen folder. It stays stopped while you prepare and finish its application setup below."
          : retry
          ? "Retry keeps the accepted source, application data and saved installation evidence in this folder. It replaces the validated private configuration and reruns guarded dependency and startup steps. External channels and application jobs still need live verification."
          : "Install and start transfers the accepted source, downloads its pinned runtime and dependencies, runs its checks and starts its owned user service. External channels and application jobs still need live verification."}</p>
        {receipt.product_runtime !== true && <p className="deployment-note">{cloudMode ? "The cloud bootstrap prepares a user service for unattended operation. The destination checks report whether lingering is enabled; application restart after reboot remains unverified." : "This runs as a systemd service for your Linux user and may stop on logout unless this computer keeps user services running. Forge checks that the user service manager is reachable; it does not enable persistence after logout or verify restart after reboot."}</p>}
        <div className="deployment-actions"><button className="setup-action primary" type="submit"
          disabled={selectionBlocked || !!busy || unavailable || !portReady || !remoteCanInstall(receipt, options, build, state, values, blocked)}>{busy === "install" ? "Admitting remote installation..." : receipt.product_runtime === true ? cloudMode ? "Install in Google Cloud" : "Install application" : retry ? "Retry installation on this Linux computer" : cloudMode ? "Install and start in Google Cloud" : "Install and start on Linux"}</button>
          <button className="setup-action" type="button" disabled={selectionBlocked || !!busy} onClick={() => { setValues({}); setFormBinding(""); }}>Cancel private setup</button></div>
      </form>}
      {!cloudRemoving && !settingsEditing && <RemoteApplicationSetup state={state} options={options} build={build} receipt={receipt}
        selectionScope={selectionScope} confirmed={!unavailable && remoteObservationAvailable(receipt)} pending={!!busy || !!receipt.pending_operation}
        disabled={blocked || selectionBlocked || unavailable} onAction={action} onCheck={refresh}
        onSettings={() => setSettingsOpenRequest({ guideId: state.id, binding })} />}
      {!cloudRemoving && <RemotePrivateSettings state={state} build={build} options={options} receipt={receipt}
        blocked={blocked || !!busy && !busy.startsWith("settings-")} observationUnavailable={unavailable}
        selectionScope={selectionScope} renderConfiguration={renderConfiguration} onBusy={settingsBusy}
        openRequest={settingsOpenRequest?.guideId === state.id && settingsOpenRequest.binding === binding ? settingsOpenRequest : null}
        onEditing={(open, owner) => setSettingsEditor((previous) => open
          ? { ...owner, open }
          : previous?.editor === owner.editor ? null : previous)}
        onReceipt={publish} onRecheck={refresh} />}
      {!settingsEditing && <div className="deployment-actions">
        <button className="setup-action" type="button" disabled={selectionBlocked || !!busy} onClick={() => refresh(selectionToken)}>Recheck remote status</button>
        {!cloudRemoving && remoteActionAvailable(receipt, "stop", options, build, state, blocked) && <button className="setup-action" type="button"
          disabled={selectionBlocked || !!busy} onClick={() => action("stop")}>{busy === "stop" ? "Checking owned stop..." : "Stop owned application"}</button>}
        {remoteActionAvailable(receipt, "restart", options, build, state, blocked) && <button className="setup-action" type="button"
          disabled={selectionBlocked || !!busy || unavailable} onClick={() => action("restart")}>{busy === "restart" ? "Restarting accepted application..." : "Restart accepted application"}</button>}
      </div>}
      {receipt.target === "gcp" && removalStatus !== "removed" && <CloudResourceGuidance destination={receipt.endpoint} removalAvailable={removalSignal?.available === true} removalInProgress={cloudRemoving} />}
      <RemoteChecks checks={remoteDeploymentChecks(receipt)} title="Deployment checks" />
      {activity.length > 0 && <details className="deployment-evidence" open={remoteIsBusy(receipt)}>
        <summary>Remote deployment activity</summary><ol>{activity.map((entry, index) => <li key={index}>{entry.message}</li>)}</ol>
      </details>}
      {(receipt.status === "running" && !cloudRemoving || remoteProductSelected(receipt)) && <div className="remote-first-use">
        {receipt.status === "running" && !cloudRemoving && <h4>{remoteNativeReady(receipt) && !unavailable ? "Use your installed application" : "Check your installed runtime"}</h4>}
        <RemoteApplicationAccess selectionScope={selectionScope} state={state} build={build} options={options} receipt={receipt}
          blocked={blocked || !!busy} observationUnavailable={unavailable} onRecheck={refresh} taskHandoff={firstTaskHandoff} />
        {receipt.status === "running" && !cloudRemoving && <details className="deployment-evidence"><summary>What has been checked</summary>
          <p>Native runtime status does not establish a useful application result. The application UI, sign-in, phone access and external workflows have not been verified.</p>
          <p>{receipt.product_runtime === true
            ? productExecutionMode === "interactive"
              ? "Your application is running in interactive mode. Background operation is configured separately in application setup; logout and reboot continuity have not been verified."
              : productExecutionMode === "background"
                ? "Your application is running in background mode. Check the observed continuity result before relying on restart after logout or reboot."
                : "Forge has not confirmed this application's operation mode. Recheck remote status before relying on unattended operation."
            : cloudMode ? "Running now does not prove restart after reboot. Check the observed user-service continuity result below before relying on unattended operation." : "Running now does not prove it will survive logout or reboot. It uses your Linux user's systemd service; keeping it running after logout is a separate host setting."}</p>
        </details>}</div>}
      <RemoteBrowserAccess selectionScope={selectionScope} state={state} build={build} options={options} receipt={receipt}
        blocked={blocked || !!busy} observationUnavailable={unavailable} removalPending={cloudRemoving} />
      {!cloudRemoving && <RemoteInstalledUse selectionScope={selectionScope} state={state} build={build} receipt={receipt}
        blocked={blocked || !!busy} commandUnconfirmed={unavailable} onRepairReceipt={onRepairReceipt} onTaskHandoff={setFirstTaskHandoff} />}
    </div>}
    {pendingHandoff && <details className="remote-destination-change"><summary>Choose a different computer or folder</summary>{destinationForm}</details>}
    {unavailable && !receipt && <p className="deployment-warning" role="status">Forge could not check saved remote status. Your destination details remain here. Recheck before starting another installation.</p>}
    {!receipt && <button className="setup-action" type="button" disabled={selectionBlocked || !!busy} onClick={() => refresh(selectionToken)}>{loaded ? "Recheck saved remote status" : "Checking saved remote status..."}</button>}
    {error && <p className="deployment-error" role="alert">{error.text}</p>}
  </section>;
}

function RemoteChecks({ checks, title }) {
  if (!Array.isArray(checks) || !checks.length) return null;
  return <details className="deployment-evidence" open={checks.some((check) => !["pass", "passed"].includes(check.status))}>
    <summary>{title}</summary><ul>{checks.map((check, index) => <li key={index}><strong>{check.name}: {check.status}</strong>
      {check.detail && <p>{check.detail}</p>}</li>)}</ul></details>;
}
