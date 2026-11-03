import React, { useEffect, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionEvents } from "./guide-selection.mjs";
import { request } from "./state.mjs";
import { artifactIsCurrent } from "./artifact-current.mjs";
import { remoteProductSelected } from "./remote-deployment.mjs";
import { createInstalledUseGate, installedUseSessionStorage, copyInstalledUseTask, installedUseTaskAllowed,
  installedUseSelectedIdea, INSTALLED_USE_TASK_LIMIT_MESSAGE, INSTALLED_USE_OUTCOMES } from "./deployment-use.mjs";
import { installedUseImprovementMatches, installedUseImprovementIsSameReport, installedUseFeedbackNotAdmitted } from "./deployment-use.mjs";
import { remoteInstalledUseBinding, remoteInstalledUseDraftScope, createRemoteInstalledUseDraftStore,
  remoteInstalledUseReadKey, remoteInstalledUseBusy, remoteInstalledUseUrl, remoteInstalledApplicationUse,
  remoteInstalledUseFeedback, remoteInstalledUseTaskHandoff, createRemoteInstalledUseSuccessStore,
  remoteInstalledUseSuccessBody, remoteInstalledUseSuccessReport, createRemoteInstalledUseImprovementStore,
  remoteInstalledUseImprovementHint } from "./remote-installed-use.mjs";
import { InstalledUseStartingIdeas, InstalledUseWalkthrough, InstalledUseSuccessView, useInstalledUseSuccess,
  InstalledImprovementHandoff } from "./deployment-use.jsx";
import "./deployment-use.css";

const pageRemoteInstalledUseStores = new WeakMap();

// The exact destination binding owns its existing store namespace in this window.
function getRemoteInstalledUsePageStores(binding, page = globalThis.window) {
  const ownsPage = !!page && (typeof page === "object" || typeof page === "function");
  const create = () => {
    const improvementStorage = ownsPage ? installedUseSessionStorage(page) : null;
    return { drafts: createRemoteInstalledUseDraftStore(improvementStorage, binding),
      successStore: createRemoteInstalledUseSuccessStore(improvementStorage, binding), improvementStorage,
      improvementStore: createRemoteInstalledUseImprovementStore(improvementStorage, binding) };
  };
  if (!ownsPage) return create();
  if (!pageRemoteInstalledUseStores.has(page)) pageRemoteInstalledUseStores.set(page, new Map());
  const stores = pageRemoteInstalledUseStores.get(page);
  if (!stores.has(binding)) stores.set(binding, create());
  return stores.get(binding);
}

export function RemoteInstalledUse(props) {
  // Destination preflight has not admitted an application to try. Preserve
  // guidance and reports for admitted attempts even if they later stop or fail.
  const binding = props.receipt?.admitted === true ? remoteInstalledUseBinding(props.receipt, props.state) : "";
  return binding ? <RemoteInstalledUsePanel key={binding} {...props} binding={binding} /> : null;
}

function RemoteInstalledUsePanel({ state, receipt, build, blocked = false, commandUnconfirmed = false, onRepairReceipt, onTaskHandoff, binding, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !captureGuideSelection(selectionScope, state, selectionToken);
  const [stores] = useState(() => getRemoteInstalledUsePageStores(binding));
  const { drafts, successStore, improvementStorage, improvementStore } = stores;
  const scope = remoteInstalledUseDraftScope(receipt, state), draft = drafts.read(scope);
  const draftRevision = useRef(0), panel = useRef(null);
  const [, redraw] = useState(0), [source, setSource] = useState({ key: "", metadata: null });
  const [loading, setLoading] = useState(true), [reload, setReload] = useState(0);
  const [sending, setSending] = useState(false), [error, setError] = useState("");
  const [rejectedReport, setRejectedReport] = useState(false);
  const [improvementAdmission, setImprovementAdmission] = useState(() => {
    const hint = improvementStore.read(scope);
    return hint ? { ...hint, status: hint.buildId ? "recovering" : "unknown", restored: true } : null;
  });
  const admissionRef = useRef(improvementAdmission), lastHint = useRef(improvementStore.read(scope));
  const [buildObservation, setBuildObservation] = useState(null), [checkingBuild, setCheckingBuild] = useState(false);
  const statusSequence = useRef(0);
  const [acknowledgement, setAcknowledgement] = useState(false), [taskIssue, setTaskIssue] = useState("");
  const [copying, setCopying] = useState(false), [copyStatus, setCopyStatus] = useState("");
  const gate = useRef(null), latest = useRef(null), taskField = useRef(null), mounted = useRef(true), posting = useRef(null);
  const sequence = useRef(0);
  if (!gate.current) gate.current = createInstalledUseGate();
  const readKey = remoteInstalledUseReadKey(receipt, state, build);
  latest.current = { state, receipt, build, blocked, readKey, scope, binding, task: draft.task };
  const metadata = source.key === readKey ? source.metadata : null;
  const use = remoteInstalledApplicationUse(metadata, receipt, state, build);
  const ideasOwnerKey = JSON.stringify([scope, readKey, metadata?.first_use?.metadata_sha256]);
  latest.current.ideasOwnerKey = ideasOwnerKey;
  const success = useInstalledUseSuccess({ metadata, receipt, state, scope, readKey, reload, task: draft.task, outcome: draft.outcome,
    revision: draftRevision.current, store: successStore, makeBody: remoteInstalledUseSuccessBody,
    validate: remoteInstalledUseSuccessReport, url: remoteInstalledUseUrl(state.id, receipt.deployment_id, "success"),
    isDeploymentBusy: remoteProductSelected(receipt) ? remoteInstalledUseBusy : null, selectionScope, commandUnconfirmed });
  const selected = use && (installedUseSelectedIdea(metadata.first_use, drafts.selectedIdea(scope)) ||
    use.startingIdeas.tasks.find((item) => item.request === draft.task));
  const noDeclaredUi = use?.noDeclaredUi === true;
  const customUi = use?.interfaces.status === "resolved" && use.interfaces.entries.length > 0;
  const entrypointUnresolved = !!use && !noDeclaredUi && !customUi;
  const body = remoteInstalledUseFeedback(metadata, receipt, state, build,
    { task: draft.task, outcome: draft.outcome, request: draft.improvement, continueCurrentCandidate: acknowledgement }, blocked);

  useEffect(() => {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    onTaskHandoff?.(!blocked && !loading ? remoteInstalledUseTaskHandoff(metadata, receipt, state, build, draft.task) : null);
    return () => { if (ownerCurrent()) onTaskHandoff?.(null); };
  }, [readKey, metadata, draft.task, blocked, loading, onTaskHandoff, selectionToken, selectionBlocked]);

  function updateDraft(field, value) {
    if (!captureGuideSelection(selectionScope, state, selectionToken)) return;
    if (drafts.update(scope, field, value)) {
      if (field === "task" || field === "outcome") { draftRevision.current++; success.retire(); }
      if (field === "task") latest.current.task = value;
      redraw((revision) => revision + 1);
    }
  }

  useEffect(() => {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const token = gate.current.select(readKey), selectedSequence = ++sequence.current;
    setLoading(true); if (!admissionRef.current) setError("");
    gate.current.run(token, () => request(remoteInstalledUseUrl(state.id, receipt.deployment_id)),
      () => latest.current.readKey,
      (value) => { if (ownerCurrent() && selectedSequence === sequence.current) {
        setSource({ key: readKey, metadata: value }); setLoading(false);
      } },
      () => { if (ownerCurrent() && selectedSequence === sequence.current) {
        setSource({ key: readKey, metadata: null }); setLoading(false);
      } });
    return () => gate.current.close(token);
  }, [readKey, reload, selectionToken, selectionBlocked]);
  useEffect(() => { setAcknowledgement(false); }, [state.version, build?.build_id, build?.source_digest]);
  useEffect(() => { setCopyStatus(""); }, [draft.task]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    const observed = buildObservation?.key === readKey && buildObservation.ownerCurrent?.();
    if (!selectionBlocked && admissionRef.current?.restored && !checkingBuild && !observed) checkBuildStatus();
  }, [selectionToken, selectionBlocked, readKey, checkingBuild, buildObservation]);
  useEffect(() => {
    if (!captureGuideSelection(selectionScope, state, selectionToken)) return;
    const selected = admissionRef.current;
    if (!selected?.restored || buildObservation?.key !== readKey || buildObservation.admission !== selected ||
        !buildObservation.ownerCurrent?.() || !buildObservation.receipt || !metadata) return;
    const matched = !!remoteInstalledApplicationUse(metadata, receipt, state, null) &&
      selected.sourceDigest === metadata.source_digest && installedUseImprovementMatches(selected, buildObservation.receipt);
    const recovered = { ...selected, status: matched ? "submitted" : "unknown", restored: !matched };
    admissionRef.current = recovered; setImprovementAdmission(recovered);
    if (artifactIsCurrent(buildObservation.receipt, state)) {
      try { onRepairReceipt?.(buildObservation.receipt); }
      catch { /* Publish only after the source-bound admission has been reconciled. */ }
    }
  }, [readKey, metadata, buildObservation, selectionToken, selectionBlocked]);

  async function copyTask() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !draft.task.trim() || copying) return;
    setCopying(true); setCopyStatus("");
    const status = await copyInstalledUseTask(draft.task,
      typeof navigator !== "undefined" && navigator.clipboard?.writeText
        ? (value) => navigator.clipboard.writeText(value) : null,
      () => latest.current.task, () => mounted.current ? latest.current.binding : null, binding);
    if (!mounted.current) return;
    setCopying(false); if (ownerCurrent() && status) setCopyStatus(status);
  }

  function ownsAdmissionReport(selected) {
    // Observation and confirmation may replace state, but share this exact POST body.
    return admissionRef.current === selected || !!selected?.body && admissionRef.current?.body === selected.body;
  }

  async function submit(event) {
    event.preventDefault();
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !body || loading || latest.current.readKey !== readKey || posting.current || admissionRef.current ||
        installedUseImprovementIsSameReport(lastHint.current, body) || !onRepairReceipt) return;
    const hint = remoteInstalledUseImprovementHint(receipt, state, body);
    if (!hint || !improvementStore.save(scope, hint)) return;
    const persistentHint = improvementStore.persistent(scope);
    lastHint.current = hint;
    const selected = { ...hint, readKey, body, status: "pending" };
    posting.current = selected; admissionRef.current = selected; setImprovementAdmission(selected);
    statusSequence.current++; setBuildObservation(null); setSending(true); setError(""); setRejectedReport(false);
    try {
      const value = await request(remoteInstalledUseUrl(selected.guideId, receipt.deployment_id, "feedback"),
        { method: "POST", body: selected.body });
      if (posting.current !== selected) return;
      if (value?.guide_id !== selected.guideId || value.guide_version !== selected.version ||
          typeof value.build_id !== "string" || !value.build_id ||
          !["queued", "running", "checking", "completed", "failed", "interrupted"].includes(value.status))
        throw new Error("Unconfirmed improvement admission.");
      const confirmedHint = { ...hint, buildId: value.build_id,
        hostRequest: typeof value.request === "string" && value.request.length <= 100000 ? value.request : null };
      // A remounted owner can replace tracking while this response is waiting.
      // Read its captured storage namespace afresh before retaining confirmation.
      if (JSON.stringify(improvementStore.read(scope)) !== JSON.stringify(hint)) return;
      if (persistentHint) {
        const retained = createRemoteInstalledUseImprovementStore(improvementStorage, binding).read(scope);
        if (JSON.stringify(retained) !== JSON.stringify(hint)) return;
      } else if (!mounted.current || posting.current !== selected || !ownsAdmissionReport(selected)) return;
      if (!improvementStore.save(scope, confirmedHint)) return;
      if (!mounted.current || posting.current !== selected || !ownsAdmissionReport(selected)) return;
      lastHint.current = confirmedHint;
      const confirmed = { ...selected, ...confirmedHint, status: "submitted" };
      if (!ownerCurrent()) {
        // Keep the owned admission recoverable without publishing its retired response.
        admissionRef.current = { ...confirmed, status: "recovering", restored: true };
        return;
      }
      admissionRef.current = confirmed; setImprovementAdmission(confirmed);
      // Retain the exact admission while this installed owner remains mounted.
      // A newer candidate must not be replaced by the delayed response.
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
          const retained = persistentHint ? createRemoteInstalledUseImprovementStore(improvementStorage, binding).read(scope)
            : improvementStore.read(scope);
          if (JSON.stringify(improvementStore.read(scope)) !== JSON.stringify(hint) ||
              JSON.stringify(retained) !== JSON.stringify(hint)) return;
          const rejected = { ...selected, status: "rejected" };
          admissionRef.current = rejected; setImprovementAdmission(rejected);
          releaseRejectedReport(rejected);
          return;
        }
        if (selected.observed) return;
        const unknown = { ...selected, status: "unknown" };
        admissionRef.current = unknown; setImprovementAdmission(unknown);
        setError("The improvement request could not be confirmed. Check Build before retrying. Your drafts remain here.");
      }
    } finally {
      if (mounted.current && posting.current === selected) { posting.current = null; setSending(false); }
    }
  }

  function releaseRejectedReport(selected) {
    if (!captureGuideSelection(selectionScope, state, selectionToken)) return;
    const hint = remoteInstalledUseImprovementHint(receipt, state, selected?.body);
    if (!mounted.current || admissionRef.current !== selected || selected?.status !== "rejected" ||
        !hint || !improvementStore.forgetRejected(scope, hint)) return;
    statusSequence.current++; lastHint.current = null; admissionRef.current = null;
    setImprovementAdmission(null); setBuildObservation(null); setCheckingBuild(false); setError(""); setRejectedReport(true);
    setLoading(true); setSource({ key: "", metadata: null }); success.retire(); setReload(value => value + 1);
  }

  async function checkBuildStatus() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !admissionRef.current || checkingBuild) return;
    if (posting.current === admissionRef.current) {
      const pending = posting.current;
      pending.observed = true;
      // Preserve the entered POST without treating a newer Build as admission
      // of this exact report. The independent read now owns presentation.
      const unknown = { ...pending, status: "unknown" };
      admissionRef.current = unknown; setImprovementAdmission(unknown);
    }
    let selected = admissionRef.current;
    if (selected.restored && improvementStorage) {
      const expected = improvementStore.read(scope);
      const retained = createRemoteInstalledUseImprovementStore(improvementStorage, binding).read(scope);
      // Returning before the original POST settles can leave this mount with
      // its pending snapshot. Adopt only that exact report's confirmation.
      if (expected && !expected.corrupt && retained && !retained.corrupt &&
          Object.keys(expected).every(key => selected[key] === expected[key]) &&
          Object.keys(expected).every(key => key === "buildId" || key === "hostRequest" || retained[key] === expected[key]) &&
          retained.buildId && typeof retained.hostRequest === "string" &&
          (expected.buildId === null && expected.hostRequest === null ||
            expected.buildId === retained.buildId && expected.hostRequest === retained.hostRequest)) {
        selected = { ...selected, ...retained, status: "recovering", restored: true };
        admissionRef.current = selected; setImprovementAdmission(selected);
      }
    }
    const key = latest.current.readKey, serial = ++statusSequence.current;
    setCheckingBuild(true); setBuildObservation(null);
    try {
      const value = await request(`/api/guide/${encodeURIComponent(state.id)}/implementation`);
      if (!mounted.current || !ownerCurrent() || serial !== statusSequence.current || !ownsAdmissionReport(selected) || latest.current.readKey !== key) return;
      if (value?.guide_id !== state.id || !Number.isSafeInteger(value.guide_version) ||
          typeof value.build_id !== "string" || !value.build_id ||
          !["queued", "running", "checking", "completed", "failed", "interrupted"].includes(value.status))
        throw new Error("Unverified Build status.");
      const current = artifactIsCurrent(value, latest.current.state);
      const ready = current && value.source_verified === true && /^[a-f0-9]{64}$/.test(value.source_digest || "") &&
        value.cleanup_pending !== true && ["completed", "failed", "interrupted"].includes(value.status);
      setBuildObservation({ key, receipt: value, ready, admission: admissionRef.current, ownerCurrent });
      if (current && !selected.restored) { try { onRepairReceipt?.(value); } catch { /* Display failures cannot erase the host observation. */ } }
    } catch {
      if (mounted.current && ownerCurrent() && serial === statusSequence.current && ownsAdmissionReport(selected))
        setBuildObservation({ key, ownerCurrent,
          issue: "Build status could not be checked. The admission state is unchanged; no request was resent." });
    } finally {
      if (mounted.current && serial === statusSequence.current) setCheckingBuild(false);
    }
  }

  const observation = buildObservation?.key === readKey ? buildObservation : null;
  const pendingObserved = posting.current?.observed === true && observation?.admission === improvementAdmission &&
    observation?.ownerCurrent?.();
  const canPrepareReport = !improvementAdmission?.corrupt && !!observation?.ready && observation.receipt.build_id === build?.build_id &&
    observation.receipt.source_digest === build?.source_digest &&
    (improvementAdmission?.status === "submitted" && (!improvementAdmission.observed || installedUseImprovementMatches(improvementAdmission, observation.receipt)) ||
      observation.receipt.build_id !== improvementAdmission?.continuationBuildId) &&
    !blocked && !loading && (!sending || pendingObserved);
  function prepareAnotherReport() {
    if (!captureGuideSelection(selectionScope, state, selectionToken) || !canPrepareReport || !mounted.current || admissionRef.current !== improvementAdmission || latest.current.readKey !== readKey ||
        latest.current.blocked || posting.current && !pendingObserved) return;
    // Preparing a distinct report does not cancel or replay the original POST.
    if (posting.current) { posting.current = null; setSending(false); }
    statusSequence.current++; admissionRef.current = null; setImprovementAdmission(null); setBuildObservation(null);
    setError(""); setAcknowledgement(false); success.retire(); setReload(value => value + 1);
  }
  const duplicateReport = !improvementAdmission && installedUseImprovementIsSameReport(lastHint.current,
    { task: draft.task.trim(), outcome: draft.outcome, request: draft.improvement.trim() });
  const observedImprovement = observation?.receipt && installedUseImprovementMatches(improvementAdmission, observation.receipt)
    ? observation.receipt : build;

  return <section ref={panel} className="deployment-use remote-installed-use" aria-label="Try a useful first job"
    aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)}>
    <h3>{use ? "Try " + use.name : "Try a useful first job"}</h3>
    <p>Use the connection controls above to check or open your application. Preparing a request here is optional.
      {noDeclaredUi && " No application links are declared in this package."}</p>
    {!use && <p role="status">{loading ? "Loading this saved package's first-job suggestions..."
      : "Forge could not check first-job guidance for this saved package. Your drafts remain here."}</p>}
    {!loading && (!use || !use.observationAvailable || !use.installedVerified || !use.nativeAvailable) && <button className="setup-action" type="button" disabled={selectionBlocked} onClick={() => {
      if (captureGuideSelection(selectionScope, state, selectionToken)) { success.retire(); setReload((value) => value + 1); }
    }}>Recheck first-job guidance</button>}
    {noDeclaredUi && <div className="deployment-use-help">
      <p>Native contact: <strong>{use.entrypoint.name}</strong> <code>{use.entrypoint.identity}</code></p>
      {!use.nativeAvailable && <p>When the installation is running, use the controls above to connect and open native Console. Your draft stays here.</p>}
      <details className="deployment-use-console-send">
        <summary>Using native chat for this app</summary>
        <p>If this app uses native chat for everyday work, open native Console and sign in if asked.
          If this conversation is not already selected, choose <code>{use.entrypoint.identity}</code> in the sidebar.
          Paste your copied task or type a job, then send it. Stay in that same conversation for replies, tool activity and follow-up questions.
          Look for the response under the request you sent.</p>
        <p>If this app uses its own UI, use that UI's login, job and result controls. Native Console remains for administration and inspection.</p>
      </details>
    </div>}
    {customUi && <p className="deployment-use-help">Use the application's own controls to send the job and find its result.
      Native Console remains for administration and inspection; copying a draft does not send a request or approve an action.</p>}
    {entrypointUnresolved && <p className="deployment-use-help">This application's everyday entrypoint has not been checked.
      Check its application links above before sending the job. Use native Console for administration and inspection.</p>}
    <div className="deployment-use-phase-heading"><h4>Prepare a request (optional)</h4></div>
    {use && <InstalledUseStartingIdeas ideas={use.startingIdeas} task={draft.task} taskRef={taskField}
      ownerKey={ideasOwnerKey} disabled={selectionBlocked} onIdea={(item, index) => {
        if (!mounted.current || latest.current.ideasOwnerKey !== ideasOwnerKey ||
            !captureGuideSelection(selectionScope, state, selectionToken) ||
            JSON.stringify(item) !== JSON.stringify(use.startingIdeas.tasks[index]) ||
            !drafts.selectIdea(scope, metadata.first_use, index)) return false;
          setTaskIssue(""); draftRevision.current++; success.retire(); latest.current.task = item.request;
          redraw((revision) => revision + 1);
          return true;
      }} />}
    <label className="deployment-use-field">What would you like it to do first?
      <textarea ref={taskField} value={draft.task} rows={3} aria-invalid={!!taskIssue} disabled={selectionBlocked}
        onChange={(event) => { if (!captureGuideSelection(selectionScope, state, selectionToken)) return;
          if (installedUseTaskAllowed(event.target.value)) {
          setTaskIssue(""); updateDraft("task", event.target.value);
        } else setTaskIssue(INSTALLED_USE_TASK_LIMIT_MESSAGE); }}
        placeholder="Describe a real job in your own words." />
      <span>{noDeclaredUi ? "This draft is not sent automatically. Copy it for use in the application's own UI or native chat, if that is its everyday interface."
        : customUi ? "This draft is not sent automatically. Copy it or use it as a starting point in your application's own controls."
        : "This draft is not sent automatically. Keep it here or copy it for use after the application's everyday entrypoint has been checked."}</span>
      {taskIssue && <span className="deployment-error" role="alert">{taskIssue}</span>}
    </label>
    <div className="deployment-use-copy">
      <button className="setup-action" type="button" disabled={selectionBlocked || !draft.task.trim() || copying} onClick={copyTask}>
        {copying ? "Copying task..." : "Copy task"}
      </button>
      {(copying || copyStatus === "unavailable") && <button className="setup-action" type="button"
        disabled={selectionBlocked} onClick={() => {
          if (captureGuideSelection(selectionScope, state, selectionToken)) { taskField.current?.focus(); taskField.current?.select(); }
        }}>Select task to copy</button>}
      {(copying || copyStatus) && <span role="status">{copying
        ? "Clipboard copy is still pending. You can select the task and copy it manually."
        : copyStatus === "copied" ? noDeclaredUi ? "Task copied. Use the application's everyday UI or native chat, if that is its everyday interface."
        : customUi ? "Task copied. Use it in your application." : "Task copied. Use it after the application's everyday entrypoint has been checked."
        : "The task could not be copied. Select it and copy manually."}</span>}
    </div>
    {customUi && use.interfaces.entries.some((entry) => entry.id === use.startingIdeas.entryId) && <p className="deployment-use-help">When its checked browser connection is ready, use Continue with this task beside the matching application link above. Your editable draft stays here.</p>}
    {use && <details className="deployment-use-job-help">
      <summary>Help with this job</summary>
      <InstalledUseWalkthrough idea={selected} customized={!!selected && selected.request !== draft.task} />
    </details>}
    <p className="deployment-use-help">{drafts.persistent(scope)
      ? "Your task and report drafts stay in this browser tab across reloads. Leave out passwords and private settings."
      : "Browser draft storage is unavailable. Your drafts remain in this open page; keep them here before reloading."}</p>
    {use && <InstalledUseSuccessView disabled={selectionBlocked} task={draft.task} {...success} taskRef={taskField}
      onNextTask={() => { if (!captureGuideSelection(selectionScope, state, selectionToken)) return;
        panel.current?.querySelector(".deployment-use-ideas button")?.focus();
        if (!use.startingIdeas.tasks.length) taskField.current?.focus(); }}
      onCheckReports={() => { if (captureGuideSelection(selectionScope, state, selectionToken)) {
        success.retire(); setReload((value) => value + 1);
      } }} />}
    <details className="deployment-use-feedback-disclosure">
      <summary>Report what happened or request an improvement</summary>
      <form className="deployment-use-feedback" onSubmit={submit}>
        <fieldset disabled={selectionBlocked || sending}><legend>What happened when you tried it? (required)</legend>
          <div className="deployment-use-outcomes">{INSTALLED_USE_OUTCOMES.map((item) => <label key={item.id}>
            <input type="radio" name={"remote-installed-outcome-" + receipt.deployment_id} value={item.id}
              checked={draft.outcome === item.id} required onChange={() => updateDraft("outcome", item.id)} />{item.label}
          </label>)}</div>
        </fieldset>
        <label className="deployment-use-field">What happened, and what should change?
          <textarea value={draft.improvement} rows={3} maxLength={6000} disabled={selectionBlocked || sending}
            onChange={(event) => updateDraft("improvement", event.target.value)}
            placeholder="Describe what you saw, what you expected and the improvement you want." />
          {draft.outcome === "blocked" && <span>Describe the setup, page or sign-in step that stopped you. You can report a blocked setup even if you could not try the job.</span>}
        </label>
        {use?.continuation.requiresAcknowledgement && <label className="deployment-use-acknowledgement">
          <input type="checkbox" checked={acknowledgement} disabled={selectionBlocked || sending}
            onChange={(event) => { if (captureGuideSelection(selectionScope, state, selectionToken)) setAcknowledgement(event.target.checked); }} />
          <span>Improve the selected current candidate using this report. This installation stays unchanged.</span>
        </label>}
        <button className="setup-action" type="submit" disabled={selectionBlocked || !body || loading || sending || improvementAdmission || duplicateReport || blocked || !onRepairReceipt}>
          {sending ? "Sending your improvement request..." : "Ask builder to improve it"}
        </button>
        <p className="deployment-use-help">Your answer is a manual report. Forge has not observed this application job or verified its result.
          Sending it asks the builder to check a candidate; installing an update is a separate step.</p>
        {!!draft.improvement.trim() && !draft.outcome && <p className="deployment-note">Choose what happened above before sending.</p>}
        {(!use?.continuation.available || blocked) &&
          <p className="deployment-note">{use?.continuation.reason || "Finish current source checks or deployment actions before sending feedback."} Your drafts remain here.</p>}
      </form>
    </details>
    <InstalledImprovementHandoff admission={improvementAdmission} build={observedImprovement} state={state} />
    {improvementAdmission && <div className="deployment-use-submitted" role="status">
      {improvementAdmission.status !== "submitted" && <p>{improvementAdmission.status === "pending"
        ? "Waiting for admission of your improvement request. Your drafts remain here."
        : improvementAdmission.status === "rejected" ? "The builder rejected this report before starting it. Saved request tracking could not be cleared. Restore browser storage, then try clearing it again. Your drafts remain here."
        : improvementAdmission.status === "recovering" && (checkingBuild || loading) ? "Checking the saved improvement link against Build. Your drafts remain here."
        : improvementAdmission.buildId ? "The saved improvement link has not been matched to the current Build. It may refer to an earlier candidate. Your drafts remain here."
        : "Admission of your improvement request is unconfirmed. It may have reached the builder. Your drafts remain here."}</p>}
      {observation?.receipt && <p>Build reports a {artifactIsCurrent(observation.receipt, state) ? "current" : "previous"} candidate with status <strong>{observation.receipt.status}</strong>.
        {improvementAdmission.status === "unknown" && " This observation does not confirm whether your earlier report was admitted."}</p>}
      {observation?.issue && <p>{observation.issue}</p>}
      {improvementAdmission.status === "rejected"
        ? <button className="setup-action" type="button" disabled={selectionBlocked || sending} onClick={() => releaseRejectedReport(admissionRef.current)}>Retry clearing report tracking</button>
        : <button className="setup-action" type="button" disabled={selectionBlocked || checkingBuild} onClick={checkBuildStatus}>
          {checkingBuild ? "Checking Build status..." : "Check Build status"}</button>}
      {canPrepareReport && <><button className="setup-action" type="button" disabled={selectionBlocked} onClick={prepareAnotherReport}>Prepare another report</button>
        <p>{pendingObserved && "The original response remains pending; preparing another report does not cancel it. "}
          Preparing another report does not resend the earlier request or change the installation. Review your retained draft before sending.</p></>}
    </div>}
    {error && <p className="deployment-error" role="alert">{error}</p>}
    {rejectedReport && <p className="deployment-note" role="status">The builder did not receive this report.
      Your drafts remain here. Review the current setup before sending again.</p>}
    {duplicateReport && <p className="deployment-note">Change your report before sending another improvement request. The earlier report will not be resent.</p>}
    {lastHint.current && !improvementStore.persistent(scope) && <p className="deployment-note">Improvement tracking is available in this open page only. Browser storage is unavailable, so reload cannot recover this request. Your draft remains here.</p>}
    <details className="deployment-evidence"><summary>About these first-job suggestions</summary>
      <p>Suggestions come from the frozen saved package. Reading them does not verify installed source, a running service, sign-in or a completed task.
        The application and Console controls above report their own connection checks.</p>
    </details>
  </section>;
}
