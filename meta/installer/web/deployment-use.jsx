import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from "./guide-selection.mjs";
import React, { useEffect, useRef, useState } from "react";
import { artifactIsCurrent } from "./artifact-current.mjs";
import { request } from "./state.mjs";
import { deploymentSourceKind } from "./deployment-panel.mjs";
import { implementationIsCurrent, implementationIsRunning, implementationCanDownload,
  implementationAcceptance } from "./implementation-panel.mjs";
import { installedUseScopeKey, installedUseReadKey, installedApplicationUse, installedUseFeedback,
  installedUseUrl, createInstalledUseGate, INSTALLED_USE_OUTCOMES, installedNativeTeamUse } from "./deployment-use.mjs";
import { nativeTeamTaskScopeKey, getNativeTeamTaskDraftStore } from "./native-team-task-draft.mjs";
import { copyInstalledUseTask, installedUseIdeaPreview, installedUseTaskAllowed,
  getInstalledUsePageStores, installedUseTaskHandoff,
  installedUseSelectedIdea,
  INSTALLED_USE_TASK_LIMIT_MESSAGE, installedUseSuccessBody,
  installedUseSuccessReport, installedUseSuccessBodyMatches, installedUseRequestId,
  installedUseSuccessNotAdmitted,
  createInstalledUseImprovementStore, installedUseImprovementHint, installedUseImprovementMatches,
  installedUseImprovementIsSameReport, installedUseFeedbackNotAdmitted } from "./deployment-use.mjs";
import "./deployment-use.css";

export function InstalledImprovementHandoff({ admission, build, state }) {
  if (admission?.status !== "submitted" || admission.guideId !== state?.id ||
      !Number.isSafeInteger(admission.version) || typeof admission.buildId !== "string" || !admission.buildId) return null;
  const matched = build?.build_id === admission.buildId && build.guide_id === admission.guideId &&
    build.guide_version === admission.version && implementationIsCurrent(build, state);
  const running = matched && implementationIsRunning(build);
  const terminal = matched && !running && ["completed", "failed", "interrupted"].includes(build.status);
  const ready = terminal && implementationCanDownload(build, state) && build.source_verified === true &&
    implementationAcceptance(build, state)?.passed === true;
  return <div className="installed-improvement-handoff" role="status" data-build-id={admission.buildId}>
    <p>{ready ? "Your improvement is ready to review."
      : terminal ? "Your improvement needs more work."
      : running ? "Your improvement is being built."
      : `Your improvement request for proposal ${admission.version} was admitted. Follow its saved work in Build.`}
      {" "}This installation has not changed.</p>
    {terminal && <a className="setup-action" href="#setup-implementation">Review improvement</a>}
    {ready && <p>Review what changed, then use the installation controls if you want to apply the update.</p>}
  </div>;
}

export function InstalledUseStartingIdeas({ ideas, task = "", ownerKey = "", onTask, onIdea, taskRef, disabled = false }) {
  const [preview, setPreview] = useState(null);
  const current = useRef(null), previewOwner = useRef(null), previewHeading = useRef(null);
  current.current = { ideas, ownerKey, onTask, onIdea, disabled };
  const shown = preview?.ownerKey === ownerKey &&
    JSON.stringify(preview.item) === JSON.stringify(ideas?.tasks?.[preview.index]) ? preview : null;
  previewOwner.current = shown;
  useEffect(() => {
    if (shown) previewHeading.current?.focus({ preventScroll: true });
  }, [shown]);
  function apply(item, index) {
    const selected = current.current;
    if (selected.disabled) return;
    const applied = selected.onIdea ? selected.onIdea(item, index) : selected.onTask?.(item.request);
    if (applied === false) return;
    previewOwner.current = null; setPreview(null); taskRef?.current?.focus();
  }
  function choose(item, index) {
    if (current.current.disabled || current.current.ownerKey !== ownerKey ||
        JSON.stringify(item) !== JSON.stringify(current.current.ideas?.tasks?.[index])) return;
    if (task.trim() && task !== item.request) setPreview({ ownerKey, item, index });
    else apply(item, index);
  }
  function confirm(selected) {
    if (!selected || previewOwner.current !== selected || selected.ownerKey !== current.current.ownerKey ||
        JSON.stringify(selected.item) !== JSON.stringify(current.current.ideas?.tasks?.[selected.index])) return;
    apply(selected.item, selected.index);
  }
  function keep(selected) {
    if (!selected || previewOwner.current !== selected || current.current.disabled) return;
    previewOwner.current = null; setPreview(null); taskRef?.current?.focus();
  }
  if (!ideas?.tasks?.length) return null;
  return <><div className="deployment-use-ideas">
    <div>{ideas.tasks.map((item, index) => <button disabled={disabled} type="button" key={index}
      onClick={() => choose(item, index)}>
      <strong>{item.label}</strong>
      <span>{installedUseIdeaPreview(item.description || item.request)}</span>
    </button>)}</div>
    <span className="deployment-use-help">Suggestions from this installed version. You can preview another idea without changing your writing.</span>
  </div>
    {shown && <section className="deployment-use-idea-preview" aria-label="Preview first-job idea">
      <span className="deployment-use-idea-preview-label">SUGGESTED JOB</span>
      <h5 ref={previewHeading} tabIndex={-1}>{shown.item.label}</h5>
      <p className="deployment-use-idea-preview-task">{shown.item.request}</p>
      <p className="deployment-use-help">Your draft is kept below. Using this idea replaces it. Nothing is sent.</p>
      <div className="deployment-use-idea-preview-actions">
        <button type="button" className="setup-action" disabled={disabled} onClick={() => confirm(shown)}>Use this idea</button>
        <button type="button" className="setup-action" disabled={disabled} onClick={() => keep(shown)}>Keep my draft</button>
      </div>
    </section>}
  </>;
}

export function InstalledUseWalkthrough({ idea, customized = false }) {
  if (!idea) return <div className="deployment-use-walkthrough" aria-label="Prepare your own first job">
    <h4>Your own first job</h4>
    <p>Include the information the agents should use, what a useful result looks like, and where you want it saved or delivered.</p>
    <p className="deployment-use-help">Use the application's controls to check its required connections and supply your information. This installed package has not declared a walkthrough for this job.</p>
  </div>;
  return <div className="deployment-use-walkthrough" aria-label={`First-job walkthrough: ${idea.label}`}>
    <h4>{idea.label}</h4>
    {customized && <p className="deployment-use-help">You changed the draft. These instructions describe the original suggested job; check that they still fit your edit.</p>}
    <div className="deployment-use-walkthrough-steps">
      <section><h5>1. Get ready</h5>
        {idea.inputs?.length > 0 ? <><strong>Information to provide</strong><ul>{idea.inputs.map((entry, index) => <li key={index}>{entry}</li>)}</ul></>
          : <p>Include the information the agents should use.</p>}
        {idea.prerequisites?.length > 0 ? <><strong>Connections and prerequisites</strong><ul>{idea.prerequisites.map((entry, index) => <li key={index}>{entry}</li>)}</ul></>
          : <p className="deployment-use-help">Check the app's connection setup before trying this job.</p>}
      </section>
      <section><h5>2. Try the job</h5><p>{idea.description || "Review your editable draft, then use the application's controls to try the job."}</p></section>
      <section><h5>3. Check the result</h5>
        <strong>Expected result</strong><p>{idea.expected_result || "Decide what a useful result should include."}</p>
        <strong>Where to find it</strong><p>{idea.result_location || "Look for the reply or saved result in the app."}</p>
      </section>
    </div>
    <p className="deployment-use-help">Instructions from this installed version; check its connections and results in the app.</p>
  </div>;
}

export function InstalledUseSuccessView({ task, pendingTask, ready, sending, report, historical, earlierReport, error, onSuccess,
  persistent, onNextTask, onCheckReports, onClearUnsaved, taskRef, disabled = false }) {
  return <div className="deployment-use-success" aria-label="Report a successful first job">
    <h4>Did the job work?</h4>
    <p>{report ? "Choose another job when you are ready." : "Try the job in your application, then tell us how it went."}</p>
    {pendingTask && pendingTask !== task.trim() && <p className="deployment-use-help">{onClearUnsaved
      ? "An earlier confirmation was not saved. Clear its retry record first; your current draft stays here."
      : "An earlier task is awaiting confirmation. Retry its exact report first; your current draft stays here."}</p>}
    {pendingTask && <blockquote aria-label="Task awaiting success confirmation">{pendingTask}</blockquote>}
    {(!report || pendingTask) && <button className="setup-action" type="button" disabled={disabled || !ready || sending} onClick={onSuccess}>
      {sending ? "Saving your success report..." : pendingTask ? "Retry success confirmation" : "It worked"}
    </button>}
    {!task.trim() && !pendingTask && <p className="deployment-use-help">Describe the job you tried above before recording success.</p>}
    {pendingTask && !persistent && <p className="deployment-use-help" role="status">Browser report storage is unavailable. Keep this page open so a retry uses this same report.</p>}
    {report && <div className="deployment-use-success-confirmed" role="status">
      <strong>{historical ? "An earlier success report is saved for this job." : "Your success report is saved."}</strong>
      <details className="deployment-use-reported-job"><summary>View the reported job</summary><p>{report.task}</p></details>
      {historical && <p>Your current problem-report draft is separate and remains editable.</p>}
    </div>}
    {earlierReport && <div className="deployment-use-help" role="status">
      <p>Your earlier success report was confirmed. Your current task draft remains unchanged.</p>
      <details className="deployment-use-reported-job"><summary>View the earlier reported job</summary><p>{earlierReport.task}</p></details>
    </div>}
    {error && <p className="deployment-error" role="alert">{error}</p>}
    {onClearUnsaved && <button className="setup-action" type="button" disabled={disabled || sending} onClick={onClearUnsaved}>Clear unsaved confirmation</button>}
    {(error || pendingTask) && onCheckReports && <button className="setup-action" type="button" disabled={disabled || sending} onClick={onCheckReports}>Recheck success reports</button>}
    {(report || earlierReport) && <div className="deployment-use-copy">
      <button disabled={disabled} className="setup-action" type="button" onClick={onNextTask}>Choose the next job</button>
      <button disabled={disabled} className="setup-action" type="button" onClick={() => taskRef?.current?.focus()}>Write your own next job</button>

    </div>}
    <p className="deployment-use-help">Forge has not independently verified the result. Reporting success does not change the installation.</p>
  </div>;
}

/** One source-bound human report, with an immutable retry after ambiguous delivery. */
export function useInstalledUseSuccess({ metadata, receipt, state, scope, readKey, reload, task, outcome, revision, store,
  makeBody = installedUseSuccessBody, validate = installedUseSuccessReport, url = installedUseUrl(state.id, "success"),
  isDeploymentBusy = null, selectionScope, commandUnconfirmed = false }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const [sending, setSending] = useState(false), [error, setError] = useState("");
  const [confirmed, setConfirmed] = useState(null), [rejected, setRejected] = useState(null), [, redraw] = useState(0);
  const mounted = useRef(true), operation = useRef(null), selection = useRef(null), latest = useRef(null);
  const deploymentBusy = typeof isDeploymentBusy === "function" ? isDeploymentBusy(receipt, state) !== false
    : ["queued", "connecting", "installing", "checking", "starting", "stopping", "restarting", "recovery-needed"].includes(receipt?.status) ||
    receipt?.pending_operation != null || receipt?.worker_live === true ||
    receipt?.worker_live != null && typeof receipt.worker_live !== "boolean" ||
    receipt?.status === "running" && ["update", "recovery"].includes(receipt.phase) ||
    ["snapshotting", "applying", "verifying", "restoring", "recovering-candidate", "recovering-current", "recovery-needed"].includes(receipt?.update?.status);
  const setupBusy = state?.operation?.status === "running";
  const key = JSON.stringify([scope, readKey, reload, revision, deploymentBusy, setupBusy, selectionToken, commandUnconfirmed]);
  if (!selection.current || selection.current.key !== key) selection.current = { key };
  latest.current = { metadata, receipt, state, task, key, commandUnconfirmed };
  const pending = store.read(scope);
  const expected = pending && makeBody(metadata, receipt, state, pending.task, pending.request_id);
  const pendingCurrent = !!expected && installedUseSuccessBodyMatches(pending, expected, true);
  const rejectedCurrent = rejected?.key === key && installedUseSuccessBodyMatches(pending, rejected.body);
  const saved = validate(metadata?.success_report, metadata, receipt, state);
  const historyIssue = metadata?.success_report_detail != null || metadata?.success_report != null && !saved
    ? "Saved success reports could not be checked for this installation. Your application and drafts remain available; recheck first-use details before recording another success." : "";
  const reportingIssue = commandUnconfirmed
    ? "Success reporting is paused until installation command status can be checked. Your drafts and any pending report remain here."
    : metadata && (metadata.success_reporting_available === false || deploymentBusy || setupBusy)
    ? "Success reporting is unavailable right now. Finish this installation's setup or active action, then recheck. Your drafts remain here." : "";
  const savedCurrent = saved?.task === task.trim() ? saved : null;
  // Draft revisions retire delivery ownership, not a report already validated for this source.
  const currentConfirmed = confirmed?.scope === scope
    ? validate(confirmed.report, metadata, receipt, state) : null;
  const report = currentConfirmed?.task === task.trim() ? currentConfirmed : savedCurrent;
  const earlierReport = currentConfirmed && currentConfirmed.task !== task.trim() ? currentConfirmed
    : saved && saved.task !== task.trim() ? saved : null;
  const ready = !commandUnconfirmed && !selectionBlocked && !!metadata && !historyIssue && !reportingIssue && !rejectedCurrent && (pending ? pendingCurrent : !!makeBody(metadata, receipt, state, task,
    "00000000-0000-4000-8000-000000000000"));
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; selection.current = null; }; }, []);
  useEffect(() => {
    if (commandUnconfirmed || selectionBlocked) return;
    setError("");
    if (pending && saved && validate(saved, metadata, receipt, state, pending)) {
      store.clear(scope, pending); redraw((value) => value + 1);
    }
  }, [metadata, key, selectionBlocked, commandUnconfirmed]);

  function retire() { if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return; selection.current = null; setRejected(null); setError(""); }
  function releaseRejected(body, token) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!mounted.current || selection.current !== token) return;
    if (!store.reject(scope, body)) {
      setRejected({ key, body });
      setError("The host confirmed this success report was not saved, but its browser retry record could not be cleared. Clear the unsaved confirmation before recording another result. Your drafts remain here.");
      return;
    }
    setRejected(null); redraw((value) => value + 1);
    setError("The earlier success report was not saved because the setup changed. Your drafts remain here. Review the job, then choose It worked to confirm it for the current setup.");
  }
  async function submit() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !ready || latest.current.commandUnconfirmed || operation.current) return;
    const body = pending || makeBody(metadata, receipt, state, task, installedUseRequestId());
    if (!body || !store.prepare(scope, body)) {
      setError("This success report could not be prepared. Your task draft remains here."); return;
    }
    const token = selection.current || (selection.current = { key });
    operation.current = token; setSending(true); setError(""); redraw((value) => value + 1);
    try {
      // A lost report response releases the wait, retaining its exact replay body.
      const value = await request(url, { method: "POST", body, timeoutMs: 30000 });
      if (!ownerCurrent() || !mounted.current || selection.current !== token || latest.current.commandUnconfirmed) return;
      const current = latest.current, report = validate(value, current.metadata, current.receipt, current.state, body);
      if (!report) {
        setError("The success report was not confirmed. Retry its exact report; your drafts remain here."); return;
      }
      store.clear(scope, body); setConfirmed({ scope, report }); redraw((value) => value + 1);
    } catch (failure) {
      if (ownerCurrent() && mounted.current && selection.current === token && !latest.current.commandUnconfirmed) {
        if (installedUseSuccessNotAdmitted(failure)) releaseRejected(body, token);
        else setError("The success report could not be confirmed. Retry its exact report; your drafts remain here.");
      }
    } finally {
      if (mounted.current && operation.current === token) {
        operation.current = null; setSending(false); redraw((value) => value + 1);
        if (ownerCurrent() && selection.current !== token)
          setError("The setup or draft changed before success was confirmed. Retry the earlier report; your current draft stays here.");
      }
    }
  }
  return { sending, disabled: selectionBlocked, error: historyIssue || reportingIssue || error, report, historical: !!report && !!outcome && outcome !== "worked",
    earlierReport, ready, onSuccess: submit, retire,
    onClearUnsaved: rejectedCurrent ? () => releaseRejected(rejected.body, selection.current) : null,
    pendingTask: pending?.task || "", persistent: store.persistent(scope) };
}

function installedUseUnavailableInstruction(status, nativeTeam = false) {
  if (status === "stopped") return nativeTeam
    ? "Start this installed team using its installation controls, then wait for its Console health check."
    : "Start the installed application using its installation controls.";
  if (status === "running") return "The saved installation status is running, but these first-use details do not confirm Console availability. Check installation status, then recheck first-use details.";
  if (["queued", "installing", "checking", "starting", "stopping"].includes(status))
    return "Check installation status and wait for the installation step to finish, then recheck first-use details.";
  return "Review installation status and its recovery controls, then recheck first-use details.";
}

export function InstalledApplicationUseView({ use, task, outcome, improvement, acknowledgement,
  onTask, onOutcome, onImprovement, onAcknowledgement, onSubmit, canSubmit, sending, blocked,
  loading, error, submitted, onReload, onCopyTask, onSelectTask, taskRef, copying, copyStatus,
  experienceLoading, experienceIssue, onReloadExperiences, draftPersistent, taskIssue, onTaskRejected,
  selectedIdea, onIdea, ideasOwnerKey, success, onNextTask, onCheckReports, deploymentStatus, Icon, selectionBlocked = false,
  commandUnconfirmed = false }) {
  const accessBlocked = selectionBlocked || commandUnconfirmed;
  const stopped = deploymentStatus === "stopped";
  const interfaces = use?.interfaces?.entries || [];
  const interfaceIssue = use?.interfaces?.issue || experienceIssue;
  const interfaceUnresolved = !!experienceLoading || !!interfaceIssue ||
    !["none", "declared"].includes(use?.interfaces?.status);
  const customUi = !interfaceUnresolved && interfaces.length > 0;
  const noDeclaredUi = !interfaceUnresolved && use?.interfaces?.status === "none";
  const showConsoleHandoff = noDeclaredUi && !!use?.consoleUrl;
  const missingInterfaceUrls = deploymentStatus === "running" && use?.interfaces?.status === "declared" &&
    interfaces.some((entry) => !entry.url);
  const linkReadsPending = !!loading || !!experienceLoading;
  function retryUiLinks() {
    if (selectionBlocked || linkReadsPending || onReloadExperiences?.() === false) return;
    onReload?.();
  }
  const taskHandoff = customUi && !accessBlocked && !blocked && !loading ? installedUseTaskHandoff(use, task) : null;
  const primaryEntryAvailable = !accessBlocked && (showConsoleHandoff ||
    customUi && interfaces.length === 1 && !!interfaces[0].url);
  const outcomeMissing = !INSTALLED_USE_OUTCOMES.some((item) => item.id === outcome);
  const blockedReportHint = outcome === "blocked" ? interfaceUnresolved
    ? "Describe which step stopped you from opening or using the application, what you saw, and what you expected. Its everyday entrypoint still needs to be checked."
    : customUi
    ? "Describe which application page or sign-in step stopped you, what you saw, and what you expected. Use this report for everyday application work; the native Console is for administration and inspection."
    : "Describe which step stopped you from opening or using the application, what you saw, and what you expected."
    : null;
  const draftPersistenceHelp = draftPersistent === true && <p className="deployment-use-help">Your task and problem-report drafts stay in this browser tab across reloads until you close it. Leave out passwords and private settings.</p>;
  const taskDraft = <>
    {draftPersistent === false && <p className="deployment-use-help" role="status">Browser draft storage is unavailable. Your draft remains in this open page; keep it here before reloading.</p>}
    <label className="deployment-use-field deployment-use-task-field">
      <span className="deployment-use-draft-heading"><span>What would you like it to do first?</span>
        <span className="deployment-use-draft-count">{Array.from(task).length} / 3,000</span></span>
      <textarea disabled={selectionBlocked} ref={taskRef} value={task} rows={3} aria-invalid={!!taskIssue}
        onChange={(event) => { if (installedUseTaskAllowed(event.target.value)) onTask(event.target.value);
          else onTaskRejected?.(); }}
        placeholder="Describe a real job in your own words." />
      {taskIssue && <span className="deployment-error" role="alert">{taskIssue}</span>}
      <span>{interfaceUnresolved ? "This task is not sent automatically. Keep it here or copy it for use after the application entrypoint has been checked."
        : customUi ? "This task is not sent to the application automatically. Copy it or use it as a starting point in the application UI."
        : "This task is not sent to the application automatically. Copy it for use through the application's usual controls."}</span>
    </label>
    {draftPersistenceHelp}
  </>;
  const taskCopyControls = <div className="deployment-use-copy">
    <button className={`setup-action${taskHandoff || primaryEntryAvailable ? "" : " primary"}`} type="button" disabled={selectionBlocked || !task.trim() || copying} onClick={onCopyTask}>
      {copying ? "Copying task..." : "Copy task"}
    </button>
    {(copying || copyStatus === "unavailable") && <button disabled={selectionBlocked} className="setup-action" type="button" onClick={onSelectTask}>Select task to copy</button>}
    {(copying || copyStatus) && <span role="status">{copying
      ? "Clipboard copy is still pending. You can select the task and copy it manually."
      : copyStatus === "copied"
      ? interfaceUnresolved ? "Task copied. Use it after the application entrypoint has been checked."
        : customUi ? "Task copied. Use it in the application UI." : "Task copied. Use it through the application's usual controls."
      : "The task could not be copied. Select it and copy manually."}</span>}
  </div>;
  const reportDraft = <>
    <fieldset disabled={selectionBlocked || sending}><legend>Did it do what you expected? (required)</legend>
      <div className="deployment-use-outcomes">{INSTALLED_USE_OUTCOMES.map((item) => <label key={item.id}>
        <input type="radio" name="installed-application-outcome" value={item.id} checked={outcome === item.id}
          required onChange={() => onOutcome(item.id)} />{item.label}
      </label>)}</div>
    </fieldset>
    <label className="deployment-use-field">What happened, and what should change? (required)
      <textarea value={improvement} required maxLength={6000} rows={3} disabled={selectionBlocked || sending}
        onChange={(event) => onImprovement(event.target.value)}
        placeholder="Describe what you saw and the improvement you want." />
      {blockedReportHint && <span>{blockedReportHint}</span>}
      <span>Include only information you want the builder to see. Leave out passwords and other private settings.</span>
    </label>
  </>;
  const consoleAccess = use?.consoleUrl ? <>
      <div className="deployment-use-conversation">
        <a className="setup-action" href={accessBlocked ? undefined : use.consoleUrl} aria-disabled={accessBlocked} tabIndex={accessBlocked ? -1 : undefined} target="_blank" rel="noreferrer">Open native Console <Icon name="arrow" size={14} /></a>
        <span><strong>{use.entrypoint.name}</strong> <code>{use.entrypoint.identity}</code></span>
      </div>
      {interfaceUnresolved ? <p className="deployment-use-help">Use the native Console for authorized administration and inspection. Sign in if asked. Check the application's everyday entrypoint before sending this job.</p>
        : customUi ? <p className="deployment-use-help">Use the native Console for authorized administration and inspection. Sign in if asked. Use the application's own account and job controls for everyday work.</p>
        : <div className="deployment-use-help">
          {noDeclaredUi && <p>The native Console opens your installed team and its conversations. Follow the application's own instructions for other ways to use it.</p>}
          <p>If this application uses native chat for everyday work, sign in if asked and choose <code>{use.entrypoint.identity}</code> in the sidebar.
            Paste your copied task or type a job, then send it. Continue in that same conversation for replies, tool activity and follow-up questions.</p>
          <p>If it uses its own UI, use that UI's sign-in, job and saved-result controls. Native Console remains available for administration and inspection.</p>
        </div>}
    </> : <>
      <p className="deployment-use-help">{installedUseUnavailableInstruction(deploymentStatus)} {interfaceUnresolved ? "Then check its everyday application entrypoint."
        : customUi ? "Then use its declared application UI." : "Then use its usual entrypoint. Native Console opens once available."} Your draft stays here.</p>
      <button disabled={selectionBlocked || loading} className="setup-action" type="button" onClick={onReload}>
        {loading ? "Checking first-use details..." : "Check first-use details"}
      </button>
    </>;
  return <section className="deployment-use" aria-label="Use your application">
    <header className="deployment-use-header">
      {use ? <>
        <span className="setup-eyebrow">YOUR INSTALLED APPLICATION</span>
        <h3>{stopped ? "Your application is stopped" : `Use ${use.name}`}</h3>
        {stopped && <p>Your files and saved work are kept. Start the installed application to open its interfaces.</p>}
        {!stopped && <p>Open the application to get started. You can prepare a request here first if helpful.</p>}
      </> : <>
        <h3>Use your application</h3>
        <p>{loading ? "Loading this installed application's first-use details..."
          : "This installed application's first-use details could not be checked. Installation controls and status remain available."}</p>
        {!loading && <button disabled={selectionBlocked} className="setup-action" type="button" onClick={onReload}>Check first-use details</button>}
      </>}
    </header>
    {commandUnconfirmed && <p className="deployment-use-help" role="status">Application and Console links are paused until installation status can be checked. Your task and problem-report drafts remain here; you can keep writing or copy your task.</p>}
    <section className="deployment-use-entry" aria-label="Where to go">
          {use && <>
            {showConsoleHandoff && <div className="deployment-use-console-handoff">
              <div className="deployment-use-console-open">
                <h5>Open Console</h5>
                <p className="deployment-use-help">The native Console opens your installed team and its conversations.</p>
                <a className="setup-action primary" href={accessBlocked ? undefined : use.consoleUrl} aria-disabled={accessBlocked}
                  tabIndex={accessBlocked ? -1 : undefined} target="_blank" rel="noreferrer">Open native Console <Icon name="arrow" size={14} /></a>
              </div>
              <div className="deployment-use-contact">
                <span>Conversation contact</span><strong>{use.entrypoint.name}</strong><code>{use.entrypoint.identity}</code>
              </div>
              <p className="deployment-use-help">For jobs handled through native chat, choose <strong>{use.entrypoint.name}</strong> in the Console, paste or type your request, review it, then send it. Sign in if asked.</p>
              <details className="deployment-use-console-send">
                <summary>How to send through native chat</summary>
                <p className="deployment-use-help">If this application uses native chat for everyday work:</p>
                <ol>
                  <li>Sign in if asked.</li>
                  <li>Choose <strong>{use.entrypoint.name}</strong> (<code>{use.entrypoint.identity}</code>) in the sidebar.</li>
                  <li>Paste your copied task or type a job, review it, then send it.</li>
                </ol>
                <p className="deployment-use-help">Continue in that same conversation for replies, tool activity and follow-up questions.</p>
                <p className="deployment-use-help">If it uses its own UI, use that UI's sign-in, job and saved-result controls. Native Console remains available for administration and inspection.</p>
              </details>
            </div>}
            {customUi && <div className="deployment-use-interfaces" aria-label="Declared application UI">
              <h4>{stopped ? "Open after starting the application" : interfaces.length > 1 ? "Choose an application UI" : "Open the application UI"}</h4>
              {!stopped && <p>Sign in if asked, then try your job using the application's own controls.</p>}
              {taskHandoff && <div className="deployment-use-entry-handoff">
                <a className="setup-action primary" href={taskHandoff.url} target="_blank" rel="noopener noreferrer"
                  referrerPolicy="no-referrer" aria-label={`Continue with this task in ${taskHandoff.label}`}>Continue with this task</a>
                <p className="deployment-use-help">Your task opens as an editable draft in {taskHandoff.label}. Sign in if asked; use the app's send and confirmation controls. Your draft stays here.</p>
              </div>}
              <ul>{interfaces.map((entry) => <li key={entry.id}>
                {!entry.url && <strong>{entry.label}</strong>}
                {entry.url ? <a className={`setup-action${interfaces.length === 1 && !taskHandoff ? " primary" : ""}`} href={accessBlocked ? undefined : entry.url} aria-disabled={accessBlocked} tabIndex={accessBlocked ? -1 : undefined} target="_blank" rel="noreferrer">{/^open\s/i.test(entry.label) ? entry.label : `Open ${entry.label}`} <Icon name="arrow" size={14} /></a>
                  : <span className="deployment-use-help">{entry.reason || "The installed application's native Console must be available before opening its declared UI."}</span>}
              </li>)}</ul>
              {missingInterfaceUrls && <button disabled={selectionBlocked || linkReadsPending} className="setup-action" type="button" onClick={retryUiLinks}>Retry UI links</button>}
            </div>}
            {interfaceUnresolved && <div className="deployment-use-help" role="status">
              <p>{experienceLoading ? "Checking this installed package's declared application UI links..."
                : interfaceIssue || "This installed application's everyday entrypoint has not been checked yet."}</p>
              <button disabled={selectionBlocked || linkReadsPending} className="setup-action" type="button" onClick={retryUiLinks}>Retry UI links</button>
              <p>Your task and problem-report drafts remain here. Check the application entrypoint before using the task. Use native Console only for authorized administration and inspection.</p>
            </div>}
            {!use.consoleUrl && <div className="deployment-use-availability">{consoleAccess}</div>}
          </>}
          {!use && <>
            {interfaceIssue && <div className="deployment-use-help" role="status"><p>{interfaceIssue}</p>
              <button disabled={selectionBlocked || linkReadsPending} className="setup-action" type="button" onClick={retryUiLinks}>Retry UI links</button></div>}
            <p className="deployment-use-help">Your task and problem-report drafts remain here. Keep editing or copy your task while these details are unavailable. Check first-use details before opening the application or sending a report.</p>
          </>}
    </section>
    <div className="deployment-use-workspace">
      <div className="deployment-use-compose">
        <section className="deployment-use-phase deployment-use-prepare">
          <div className="deployment-use-phase-heading"><h4>Prepare a request (optional)</h4></div>
          {use && <InstalledUseStartingIdeas disabled={selectionBlocked} ideas={use.startingIdeas} task={task}
            ownerKey={ideasOwnerKey} taskRef={taskRef} onTask={onTask} onIdea={onIdea} />}
          {taskDraft}
        </section>
        <section className="deployment-use-phase deployment-use-handoff">
          <div className="deployment-use-phase-heading"><h4>Use this request</h4></div>
          {taskCopyControls}

        </section>
        <div className="deployment-use-help-column">
          <details className="deployment-use-job-help">
            <summary>Help with this job</summary>
            {use ? <InstalledUseWalkthrough idea={selectedIdea || use.startingIdeas.tasks.find((item) => item.request === task)}
              customized={!!selectedIdea && selectedIdea.request !== task} />
              : <p className="deployment-use-help">First-job directions will appear after this installed application's details have been checked. Your editable draft remains on this page.</p>}
          </details>
        </div>
      </div>

    </div>
    {use && !use.installedCurrent && <p className="deployment-note">These use instructions describe the installed version.</p>}
    <div className="deployment-use-phase deployment-use-result">
      <div className="deployment-use-phase-heading"><h4>How did it go?</h4></div>
      {use && success && <InstalledUseSuccessView disabled={selectionBlocked} task={task} {...success} onNextTask={onNextTask} onCheckReports={onCheckReports} taskRef={taskRef} />}
      <details className="deployment-use-feedback-disclosure">
        <summary>Report a problem or request an improvement</summary>
        <form className="deployment-use-feedback" onSubmit={(event) => {
          event.preventDefault();
          if (!use || selectionBlocked || !canSubmit || sending || blocked || loading) return;
          onSubmit(event);
        }}>
          {reportDraft}
          {use ? <>
            {use.continuation.requiresAcknowledgement && <label className="deployment-use-acknowledgement">
              <input type="checkbox" checked={acknowledgement} disabled={selectionBlocked || sending}
                onChange={(event) => onAcknowledgement(event.target.checked)} />
              <span>{use.continuation.newerCandidate
                ? "Improve the newer candidate using this report. This installed version stays unchanged."
                : "Improve the selected candidate for the current design using this report. This installed version stays unchanged."}</span>
            </label>}
            {outcomeMissing && <p className="deployment-use-help">Choose what happened above before sending your improvement request.</p>}
            {!outcomeMissing && !improvement.trim() && <p className="deployment-use-help">Describe what happened and what should change before sending your improvement request.</p>}
            <button className="setup-action" type="submit" disabled={selectionBlocked || !canSubmit || sending || blocked}>
              {sending ? "Sending your improvement request..." : "Improve this behavior"}
            </button>
            <p className="deployment-use-help">Your answer is a report of your experience. When you request an improvement, your task and report go to the builder for independent checks; applying an update is a separate step.</p>
            {!use.continuation.available && <p className="deployment-note">The selected application must finish its source checks before the builder can use this report. You can keep writing it here.</p>}
            {blocked && <p className="deployment-note">Finish the current design changes or deployment action before sending this improvement request.</p>}
          </> : <p className="deployment-use-help">Your report stays as a draft until this installed application's first-use details can be checked.</p>}
        </form>
      </details>
      {use && sending && <p className="deployment-use-help" role="status">Sending your improvement request...</p>}
      {use && error && <p className="deployment-error" role="alert">{error}</p>}
      {use && submitted && <p className="deployment-use-submitted" role="status">Your improvement request is with the builder. Follow its progress in Build; this installation has not changed.</p>}
    </div>
    {use && <details className="deployment-use-admin">
      <summary>Administration and connection details</summary>
      {customUi && <p className="deployment-use-help">These links are declared by this installed package. Their reachability, authentication and application workflows have not been verified.</p>}
      <p className="deployment-use-help">Choosing a starting idea or copying this draft does not send a request or approve an action.</p>
      {customUi && <ul className="deployment-use-paths">{interfaces.map((entry) => <li key={entry.id}>{entry.label}: <code>{entry.path}</code></li>)}</ul>}
      {use.consoleUrl && !showConsoleHandoff && consoleAccess}
    </details>}
  </section>;
}

export function InstalledApplicationUse({ state, receipt, build, blocked, commandUnconfirmed = false, onRepairReceipt,
  experiences, experienceLoading, experienceIssue, onReloadExperiences, Icon, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const [source, setSource] = useState({ key: "", metadata: null });
  const [stores] = useState(() => getInstalledUsePageStores());
  const { drafts, successStore, improvementStorage, improvementStore } = stores;
  const ideasField = useRef(null), draftRevision = useRef(0);
  const [, refreshDrafts] = useState(0), [acknowledgement, setAcknowledgement] = useState(false);
  const scope = installedUseScopeKey(receipt, state);
  const { task, outcome, improvement } = drafts.read(scope);
  function updateDraft(field, value) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (drafts.update(scope, field, value)) {
      if (field === "task" || field === "outcome") { draftRevision.current++; success.retire(); }
      if (field === "task" && latest.current) latest.current.task = value;
      refreshDrafts((revision) => revision + 1);
    }
  }
  const [loading, setLoading] = useState(true), [sending, setSending] = useState(false);
  const [error, setError] = useState(""), [reload, setReload] = useState(0);
  const [rejectedReport, setRejectedReport] = useState(false);
  const [admission, setAdmission] = useState(() => {
    const hint = improvementStore.read(scope);
    return hint ? { ...hint, status: hint.buildId ? "recovering" : "unknown", restored: true } : null;
  }), [buildObservation, setBuildObservation] = useState(null);
  const [checkingBuild, setCheckingBuild] = useState(false);
  const admissionRef = useRef(admission), lastHint = useRef(improvementStore.read(scope));
  const posting = useRef(null), statusSequence = useRef(0);
  const [copyStatus, setCopyStatus] = useState(""), [copying, setCopying] = useState(false);
  const [taskIssue, setTaskIssue] = useState("");
  const gate = useRef(null), latest = useRef(null), taskField = useRef(null), mounted = useRef(true);
  if (!gate.current) gate.current = createInstalledUseGate();
  const readKey = installedUseReadKey(receipt, state, build);
  latest.current = { state, receipt, build, blocked, readKey, task, loading,
    scope };
  const metadata = source.key === readKey ? source.metadata : null;
  const use = installedApplicationUse(metadata, receipt, state, build, experiences);
  const ideasOwnerKey = JSON.stringify([scope, readKey, metadata?.first_use?.metadata_sha256]);
  latest.current.ideasOwnerKey = ideasOwnerKey;
  const success = useInstalledUseSuccess({ metadata, receipt, state, scope, readKey, reload, task, outcome,
    revision: draftRevision.current, store: successStore, selectionScope, commandUnconfirmed });
  const selected = use ? installedUseSelectedIdea(metadata.first_use, drafts.selectedIdea(scope)) : null;
  const body = installedUseFeedback(metadata, receipt, state, build,
    { task, outcome, request: improvement, continueCurrentCandidate: acknowledgement }, blocked);
  useEffect(() => {
    const ownerCurrent = captureGuideSelection(selectionScope, state);
    if (!ownerCurrent) return;
    const token = gate.current.select(readKey);
    latest.current.loading = true;
    setLoading(true); if (!admissionRef.current) setError("");
    gate.current.run(token, () => request(installedUseUrl(state.id)), () => latest.current.readKey,
      (value) => { if (!ownerCurrent()) return; setSource({ key: readKey, metadata: value }); setLoading(false); },
      () => { if (!ownerCurrent()) return; setSource({ key: readKey, metadata: null }); setLoading(false); });
    return () => gate.current.close(token);
  }, [readKey, reload, selectionToken, selectionBlocked]);
  useEffect(() => { setAcknowledgement(false); }, [build?.build_id, build?.source_digest, state.version]);
  useEffect(() => { setCopyStatus(""); }, [task]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    const observedCurrent = buildObservation?.key === readKey && buildObservation.ownerCurrent?.();
    if (!selectionBlocked && admissionRef.current?.restored && !checkingBuild && !observedCurrent) checkBuildStatus();
  }, [readKey, checkingBuild, buildObservation, selectionToken, selectionBlocked]);
  useEffect(() => {
    const selected = admissionRef.current;
    if (selectionBlocked || !selected?.restored || buildObservation?.key !== readKey || buildObservation.admission !== selected ||
        !buildObservation.ownerCurrent?.() || !buildObservation.receipt || !metadata) return;
    // Ordinary receipts need not carry a digest. Their checked installed-source
    // metadata owns that pin, independently of the selected newer candidate.
    const matched = !!installedApplicationUse(metadata, receipt, state, null) &&
      selected.sourceDigest === metadata.source_digest && installedUseImprovementMatches(selected, buildObservation.receipt);
    const recovered = { ...selected, status: matched ? "submitted" : "unknown", restored: !matched };
    admissionRef.current = recovered; setAdmission(recovered);
    // Publish after reconciliation so a parent candidate refresh cannot retire
    // the exact observation before its source-bound admission is restored.
    if (artifactIsCurrent(buildObservation.receipt, state)) {
      try { onRepairReceipt?.(buildObservation.receipt); }
      catch { /* Display failures cannot erase the host observation. */ }
    }
  }, [readKey, metadata, buildObservation, selectionToken, selectionBlocked]);

  async function copyTask() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !task.trim() || copying) return;
    const scope = latest.current.scope;
    setCopying(true); setCopyStatus("");
    const status = await copyInstalledUseTask(task,
      typeof navigator !== "undefined" && navigator.clipboard?.writeText
        ? (value) => navigator.clipboard.writeText(value) : null,
      () => latest.current.task, () => mounted.current ? latest.current.scope : null, scope);
    if (!mounted.current) return;
    setCopying(false);
    if (ownerCurrent() && status) setCopyStatus(status);
  }

  function ownsAdmissionReport(selected) {
    // Observation and confirmation may replace state, but share this exact POST body.
    return admissionRef.current === selected || !!selected?.body && admissionRef.current?.body === selected.body;
  }

  async function submit(event) {
    event.preventDefault();
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (!body || loading || latest.current.readKey !== readKey || latest.current.blocked ||
        posting.current || admissionRef.current || installedUseImprovementIsSameReport(lastHint.current, body) || !onRepairReceipt) return;
    const hint = installedUseImprovementHint(scope, body);
    if (!hint || !improvementStore.save(scope, hint)) return;
    const persistentHint = improvementStore.persistent(scope);
    lastHint.current = hint;
    const selected = { ...hint, readKey, body, status: "pending" };
    posting.current = selected; admissionRef.current = selected; setAdmission(selected);
    statusSequence.current++; setBuildObservation(null); setSending(true); setError(""); setRejectedReport(false);
    try {
      const value = await request(installedUseUrl(selected.guideId, "feedback"), { method: "POST", body: selected.body });
      if (posting.current !== selected) return;
      if (value?.guide_id !== selected.guideId || value.guide_version !== selected.version ||
          typeof value.build_id !== "string" || !value.build_id ||
          !["queued", "running", "checking", "completed", "failed", "interrupted"].includes(value.status))
        throw new Error("Unconfirmed improvement admission.");
      const confirmedHint = { ...hint, buildId: value.build_id,
        hostRequest: typeof value.request === "string" && value.request.length <= 100000 ? value.request : null };
      // Keep this exact admission recoverable even after its view unmounts.
      // A fresh read protects newer tracking from another mounted owner view.
      const retained = persistentHint && improvementStorage ? createInstalledUseImprovementStore(improvementStorage).read(scope)
        : mounted.current && posting.current === selected && ownsAdmissionReport(selected)
          ? improvementStore.read(scope) : null;
      if (JSON.stringify(improvementStore.read(scope)) !== JSON.stringify(hint) ||
          JSON.stringify(retained) !== JSON.stringify(hint) ||
          !improvementStore.save(scope, confirmedHint)) return;
      if (!mounted.current || posting.current !== selected || !ownsAdmissionReport(selected)) return;
      lastHint.current = confirmedHint;
      const confirmed = { ...selected, ...confirmedHint, status: "submitted" };
      if (!ownerCurrent()) {
        admissionRef.current = { ...confirmed, status: "recovering", restored: true };
        return;
      }
      admissionRef.current = confirmed; setAdmission(confirmed);
      // A delayed reply can confirm its own admission without replacing the
      // newer selected candidate or pretending it belongs to a newer revision.
      if (!selected.observed && latest.current.readKey === selected.readKey && artifactIsCurrent(value, latest.current.state)) {
        try { onRepairReceipt(value); }
        catch { /* A display callback cannot undo a confirmed admission. */ }
      }
    } catch (failure) {
      if (mounted.current && posting.current === selected && ownsAdmissionReport(selected)) {
        if (!ownerCurrent()) {
          admissionRef.current = { ...selected, status: "unknown", restored: true };
          return;
        }
        if (installedUseFeedbackNotAdmitted(failure)) {
          const retained = persistentHint && improvementStorage ? createInstalledUseImprovementStore(improvementStorage).read(scope)
            : improvementStore.read(scope);
          if (JSON.stringify(improvementStore.read(scope)) !== JSON.stringify(hint) ||
              JSON.stringify(retained) !== JSON.stringify(hint)) return;
          const rejected = { ...selected, status: "rejected" };
          admissionRef.current = rejected; setAdmission(rejected);
          releaseRejectedReport(rejected);
          return;
        }
        if (selected.observed) return;
        const unknown = { ...selected, status: "unknown" };
        admissionRef.current = unknown; setAdmission(unknown);
        setError("Admission of your improvement request is unconfirmed. Check Build status before preparing another report. Your drafts remain here.");
      }
    } finally {
      if (mounted.current && posting.current === selected) { posting.current = null; setSending(false); }
    }
  }

  function releaseRejectedReport(selected) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    const hint = installedUseImprovementHint(scope, selected?.body);
    if (!mounted.current || admissionRef.current !== selected || selected?.status !== "rejected" ||
        !hint || !improvementStore.forgetRejected(scope, hint)) return;
    statusSequence.current++; lastHint.current = null; admissionRef.current = null;
    setAdmission(null); setBuildObservation(null); setCheckingBuild(false); setError(""); setRejectedReport(true);
    setLoading(true); setSource({ key: "", metadata: null }); success.retire(); setReload(value => value + 1);
  }

  async function checkBuildStatus() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !admissionRef.current || checkingBuild) return;
    if (posting.current === admissionRef.current) {
      const pending = posting.current;
      pending.observed = true;
      // Observation does not establish admission of this exact report. Keep
      // its entered POST intact while the newer read owns presentation.
      const unknown = { ...pending, status: "unknown" };
      admissionRef.current = unknown; setAdmission(unknown);
    }
    if (admissionRef.current.restored && improvementStorage) {
      const expected = improvementStore.read(scope), original = admissionRef.current;
      const retained = createInstalledUseImprovementStore(improvementStorage).read(scope);
      // An earlier view can confirm this report after this view has remounted.
      // Refresh only its confirmation, without adopting another owner or report.
      if (expected && !expected.corrupt && retained && !retained.corrupt &&
          Object.keys(expected).every(field => ["buildId", "hostRequest"].includes(field) || original[field] === expected[field]) &&
          typeof retained.buildId === "string" && retained.buildId &&
          typeof retained.hostRequest === "string" && retained.hostRequest.trim() &&
          Object.keys(expected).every(field => ["buildId", "hostRequest"].includes(field) || retained[field] === expected[field]) &&
          (expected.buildId === null && expected.hostRequest === null ||
            retained.buildId === expected.buildId && retained.hostRequest === expected.hostRequest) &&
          (original.buildId === null && original.hostRequest === null ||
            retained.buildId === original.buildId && retained.hostRequest === original.hostRequest)) {
        const recovered = { ...original, ...retained, status: "recovering", restored: true };
        lastHint.current = retained; admissionRef.current = recovered; setAdmission(recovered);
      }
    }
    const selected = admissionRef.current, key = latest.current.readKey, sequence = ++statusSequence.current;
    setCheckingBuild(true); setBuildObservation(null);
    try {
      const value = await request(`/api/guide/${encodeURIComponent(state.id)}/implementation`);
      if (!ownerCurrent() || !mounted.current || sequence !== statusSequence.current || !ownsAdmissionReport(selected) || latest.current.readKey !== key) return;
      if (value?.guide_id !== state.id || !Number.isSafeInteger(value.guide_version) ||
          typeof value.build_id !== "string" || !value.build_id ||
          !["queued", "running", "checking", "completed", "failed", "interrupted"].includes(value.status))
        throw new Error("Unverified Build status.");
      const current = artifactIsCurrent(value, latest.current.state);
      const ready = current && value.source_verified === true && /^[a-f0-9]{64}$/.test(value.source_digest || "") && value.cleanup_pending !== true &&
        ["completed", "failed", "interrupted"].includes(value.status);
      setBuildObservation({ key, receipt: value, ready, admission: admissionRef.current, ownerCurrent });
      if (current && !selected.restored) { try { onRepairReceipt?.(value); } catch { /* Display failures cannot erase the host observation. */ } }
    } catch {
      if (ownerCurrent() && mounted.current && sequence === statusSequence.current && ownsAdmissionReport(selected))
        setBuildObservation({ key, ownerCurrent, issue: "Build status could not be checked. The admission state is unchanged; no request was resent." });
    } finally {
      if (mounted.current && sequence === statusSequence.current) setCheckingBuild(false);
    }
  }

  const observation = buildObservation?.key === readKey ? buildObservation : null;
  const pendingObserved = posting.current?.observed === true && observation?.admission === admission &&
    observation?.ownerCurrent?.();
  const canPrepareReport = !admission?.corrupt && !!observation?.ready && observation.receipt.build_id === build?.build_id &&
    observation.receipt.source_digest === build?.source_digest &&
    (admission?.status === "submitted" && (!admission.observed || installedUseImprovementMatches(admission, observation.receipt)) ||
      observation.receipt.build_id !== admission?.continuationBuildId) &&
    !blocked && !loading && (!sending || pendingObserved);
  function prepareAnotherReport() {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!canPrepareReport || !mounted.current || admissionRef.current !== admission || latest.current.readKey !== readKey ||
        latest.current.blocked || posting.current && !pendingObserved) return;
    // This explicit new report leaves the original response pending; it does
    // not cancel or resend that request. Its late outcome cannot take over.
    if (posting.current) { posting.current = null; setSending(false); }
    statusSequence.current++; admissionRef.current = null; setAdmission(null); setBuildObservation(null);
    setError(""); setAcknowledgement(false); success.retire(); setReload((value) => value + 1);
  }
  const duplicateReport = !admission && installedUseImprovementIsSameReport(lastHint.current,
    { task: task.trim(), outcome, request: improvement.trim() });
  const observedImprovement = observation?.receipt && installedUseImprovementMatches(admission, observation.receipt)
    ? observation.receipt : build;
  return <div ref={ideasField} aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)}><InstalledApplicationUseView use={use} deploymentStatus={receipt?.status} task={task} outcome={outcome} improvement={improvement}
    acknowledgement={acknowledgement} onTask={(value) => { if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return; setTaskIssue(""); updateDraft("task", value); }}
    taskIssue={taskIssue} onTaskRejected={() => { if (guideSelectionAvailable(selectionScope, state, selectionToken)) setTaskIssue(INSTALLED_USE_TASK_LIMIT_MESSAGE); }}
    onOutcome={(value) => updateDraft("outcome", value)} onImprovement={(value) => updateDraft("improvement", value)}
    onAcknowledgement={value => { if (guideSelectionAvailable(selectionScope, state, selectionToken)) setAcknowledgement(value); }} onSubmit={submit} canSubmit={!!body && !!onRepairReceipt && !admission && !duplicateReport && !loading}
    sending={sending} blocked={blocked} selectionBlocked={selectionBlocked} commandUnconfirmed={commandUnconfirmed} loading={loading} error={error} submitted={false}
    onReload={() => {
      if (!guideSelectionAvailable(selectionScope, state, selectionToken) || latest.current.loading) return;
      latest.current.loading = true; setLoading(true); success.retire(); setReload((value) => value + 1);
    }} onCopyTask={copyTask}
    onSelectTask={() => { taskField.current?.focus(); taskField.current?.select(); }} taskRef={taskField}
    copying={copying} copyStatus={copyStatus} experienceLoading={experienceLoading}
    experienceIssue={experienceIssue} onReloadExperiences={() => {
      if (!guideSelectionAvailable(selectionScope, state, selectionToken) || latest.current.loading || experienceLoading) return false;
      return onReloadExperiences?.();
    }}
    draftPersistent={drafts.persistent(scope)} selectedIdea={selected} ideasOwnerKey={ideasOwnerKey}
    onIdea={(item, index) => {
      if (!mounted.current || latest.current.ideasOwnerKey !== ideasOwnerKey || latest.current.loading ||
          !guideSelectionAvailable(selectionScope, state, selectionToken) || !use ||
          JSON.stringify(item) !== JSON.stringify(use.startingIdeas.tasks[index]) ||
          !drafts.selectIdea(scope, metadata.first_use, index)) return false;
      setTaskIssue(""); draftRevision.current++; success.retire(); latest.current.task = item.request;
      refreshDrafts((revision) => revision + 1);
      return true;
    }}
    success={success} onNextTask={() => { ideasField.current?.querySelector(".deployment-use-ideas button")?.focus();
      if (!use?.startingIdeas.tasks.length) taskField.current?.focus(); }}
    onCheckReports={() => { if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return; success.retire(); setReload((value) => value + 1); }} Icon={Icon} />
    {admission && <div className="deployment-use-submitted" role="status">
      {admission.status === "submitted" ? <InstalledImprovementHandoff admission={admission} build={observedImprovement} state={state} />
        : <p>{admission.status === "pending" ? "Waiting for admission of your improvement request. Your drafts remain here."
          : admission.status === "rejected" ? "The builder rejected this report before starting it. Saved request tracking could not be cleared. Restore browser storage, then try clearing it again. Your drafts remain here."
          : admission.status === "recovering" && (checkingBuild || loading) ? "Checking the saved improvement link against Build. Your drafts remain here."
          : admission.buildId ? "The saved improvement link has not been matched to the current Build. It may refer to an earlier candidate. Your drafts remain here."
          : "Admission of your improvement request is unconfirmed. It may have reached the builder. Your drafts remain here."}</p>}
      {observation?.receipt && <p>Build reports a {artifactIsCurrent(observation.receipt, state) ? "current" : "previous"} candidate with status <strong>{observation.receipt.status}</strong>.
        {admission.status === "unknown" && " This observation does not confirm whether your earlier report was admitted."}</p>}
      {observation?.issue && <p>{observation.issue}</p>}
      {admission.status === "rejected"
        ? <button className="setup-action" type="button" disabled={selectionBlocked || sending} onClick={() => releaseRejectedReport(admissionRef.current)}>Retry clearing report tracking</button>
        : <button className="setup-action" type="button" disabled={selectionBlocked || checkingBuild} onClick={checkBuildStatus}>
          {checkingBuild ? "Checking Build status..." : "Check Build status"}</button>}
      {canPrepareReport && <><button className="setup-action" type="button" onClick={prepareAnotherReport}>Prepare another report</button>
        <p>{pendingObserved && "The original response remains pending; preparing another report does not cancel it. "}
          Preparing another report does not resend the earlier request or change the installation. Review your retained draft before sending.</p></>}
    </div>}
    {rejectedReport && <p className="deployment-note" role="status">The builder did not receive this report.
      Your drafts remain here. Review the current setup before sending again.</p>}
    {duplicateReport && <p className="deployment-note">Change your report before sending another improvement request. The earlier report will not be resent.</p>}
    {lastHint.current && !improvementStore.persistent(scope) && <p className="deployment-note">Improvement tracking is available in this open page only. Browser storage is unavailable, so reload cannot recover this request. Your draft remains here.</p>}
  </div>;
}

export function InstalledApplicationUseMount(props) {
  if (deploymentSourceKind(props.receipt) === "native-team") return <InstalledNativeTeamUse {...props} />;
  const key = installedUseScopeKey(props.receipt, props.state);
  return key ? <InstalledApplicationUse key={key} {...props} /> : null;
}

export function InstalledNativeTeamUse({ receipt, state, Icon, selectionScope, commandUnconfirmed = false }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const accessBlocked = selectionBlocked || commandUnconfirmed;
  const [snapshot, setSnapshot] = useState(null), [retry, setRetry] = useState(0);
  const [drafts] = useState(() => getNativeTeamTaskDraftStore());
  const [, refreshDraft] = useState(0), [taskIssue, setTaskIssue] = useState("");
  const [copyStatus, setCopyStatus] = useState(""), [copying, setCopying] = useState(false);
  const taskField = useRef(null), copyAttempt = useRef(null), mounted = useRef(true), latestTask = useRef(null);
  const scope = nativeTeamTaskScopeKey(receipt, state), task = drafts.read(scope);
  latestTask.current = { scope, task };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    copyAttempt.current = null; setCopying(false); setCopyStatus(""); setTaskIssue("");
  }, [scope, task]);
  function updateTask(value) {
    if (!scope || latestTask.current.scope !== scope || !guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!installedUseTaskAllowed(value)) { setTaskIssue(INSTALLED_USE_TASK_LIMIT_MESSAGE); return; }
    if (drafts.update(scope, value)) {
      latestTask.current.task = value; setTaskIssue(""); refreshDraft(revision => revision + 1);
    }
  }
  async function copyTask() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !scope || latestTask.current.scope !== scope || !task.trim() || copying) return;
    const attempt = { scope, task };
    copyAttempt.current = attempt; setCopying(true); setCopyStatus("");
    try {
      const status = await copyInstalledUseTask(task,
        globalThis.navigator?.clipboard?.writeText?.bind(globalThis.navigator.clipboard),
        () => latestTask.current.task, () => latestTask.current.scope, scope);
      if (status && mounted.current && ownerCurrent() && copyAttempt.current === attempt)
        setCopyStatus(status);
    } finally {
      if (mounted.current && copyAttempt.current === attempt) { copyAttempt.current = null; setCopying(false); }
    }
  }
  const key = JSON.stringify([state?.id, state?.version, receipt?.deployment_id, receipt?.source_id,
    receipt?.source_digest, receipt?.package_sha256, receipt?.status, receipt?.console_url,
    receipt?.health, receipt?.process_live, receipt?.bootstrap?.complete, receipt?.bootstrap?.ready, receipt?.bootstrap?.counts?.broken]);
  const selection = useRef(null);
  if (selection.current?.key !== key) selection.current = { key };
  const valid = receipt?.guide_id === state?.id && receipt?.build_id == null &&
    [receipt?.source_id, receipt?.source_digest, receipt?.package_sha256].every((value) => typeof value === "string" && /^[a-f0-9]{64}$/.test(value));
  useEffect(() => {
    const ownerCurrent = captureGuideSelection(selectionScope, state);
    if (!valid || !ownerCurrent) return;
    let stopped = false;
    const token = selection.current;
    request(installedUseUrl(state.id)).then((value) => {
      if (!stopped && ownerCurrent() && selection.current === token) setSnapshot({ key, value });
    }, () => {
      if (!stopped && ownerCurrent() && selection.current === token) setSnapshot({ key, value: null });
    });
    return () => { stopped = true; };
  }, [key, retry, valid, selectionToken, selectionBlocked]);
  const observed = snapshot?.key === key;
  const use = observed && installedNativeTeamUse(snapshot.value, receipt, state);
  if (!valid) return null;
  return <section className="deployment-use" aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)} aria-label="Use your agent team">
    <span className="setup-eyebrow">YOUR INSTALLED AGENT TEAM</span>
    <h3>{use?.consoleUrl ? `Talk to ${use.entrypoint.name}` : use
      ? receipt.status === "stopped" ? "Your agent team is stopped" : "Check your agent team's Console"
      : "Try your first job in the Console"}</h3>
    {use ? <p>{use.consoleUrl ? "Open the Console and select " : "The checked installed conversation owner is "}<strong>{use.entrypoint.name}</strong> (<code>{use.entrypoint.identity}</code>).
      {use.consoleUrl && <> Tell it the job, the information to use, and what a useful result should include.</>}</p>
      : <p>{observed ? "The installed conversation owner could not be checked. Recheck before choosing an agent for your first job."
        : "Checking this installed team's conversation owner..."}</p>}
    {scope && <>
      <label className="deployment-use-field deployment-use-task-field">
        <span className="deployment-use-draft-heading"><span>What would you like it to do first?</span>
          <span className="deployment-use-draft-count">{Array.from(task).length} / 3,000</span></span>
        <textarea disabled={selectionBlocked} ref={taskField} value={task} rows={4} aria-invalid={!!taskIssue}
          onChange={event => updateTask(event.target.value)} placeholder="Describe a real job in your own words." />
        {taskIssue && <span className="deployment-error" role="alert">{taskIssue}</span>}
        <span>This task is not sent automatically. Copy it, then review and send it in the agent team's Console when available. Leave out passwords and private settings.</span>
      </label>
      <div className="deployment-use-copy">
        <button className="setup-action" type="button" disabled={selectionBlocked || !task.trim() || copying} onClick={copyTask}>
          {copying ? "Copying task..." : "Copy task"}</button>
        {(copying || copyStatus === "unavailable") && <button disabled={selectionBlocked} className="setup-action" type="button" onClick={() => {
          if (!guideSelectionAvailable(selectionScope, state, selectionToken) || latestTask.current.scope !== scope) return;
          taskField.current?.focus(); taskField.current?.select();
        }}>Select task to copy</button>}
        {(copying || copyStatus) && <span role="status">{copying
          ? "Clipboard copy is still pending. You can select the task and copy it manually."
          : copyStatus === "copied" ? "Task copied. Review it in the Console before sending."
          : "The task could not be copied. Select it and copy manually."}</span>}
      </div>
      <p className="deployment-use-help" role={drafts.persistent(scope) ? undefined : "status"}>{drafts.persistent(scope)
        ? "Your task draft stays in this browser tab across reloads until you close it. Starting or stopping this installed team keeps the draft."
        : "Browser draft storage is unavailable. Your task stays in this open page. Copy it before closing or reloading."}</p>
    </>}
    {commandUnconfirmed && <p className="deployment-use-help" role="status">Console links are paused until installation status can be checked. Your task draft remains here; you can keep writing or copy it.</p>}
    {!selectionBlocked && use?.consoleUrl ? <a className="setup-action primary" href={accessBlocked ? undefined : use.consoleUrl}
      aria-disabled={accessBlocked} tabIndex={accessBlocked ? -1 : undefined} target="_blank" rel="noopener noreferrer">
      Open agent team Console {Icon && <Icon name="external-link" size={14} />}</a>
      : !selectionBlocked && use && <p>{installedUseUnavailableInstruction(receipt.status, true)}</p>}
    {observed && !use?.consoleUrl && <button disabled={selectionBlocked} className="setup-action" type="button" onClick={() => {
      if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
      setSnapshot(null); setRetry((value) => value + 1);
    }}>Recheck agent team details</button>}
    <p className="deployment-use-help">Check the actual reply and any saved results in the Console. This package runs your configured agents and tools; it does not add custom application screens or prove that a job has worked.</p>
  </section>;
}
