import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from "./guide-selection.mjs";
import { ConversationTranscript } from "../../packages/console-components/src/conversation/conversation-transcript";
import { Icon as ConsoleIcon, SpriteSheet } from "../../console/src/icon";
import { createGuideFrameLog, ingestGuideTimeline } from "./native-guide-chat.mjs";
import { request, requestBlob } from "./state.mjs";
import {
  implementationCanDownload, implementationIsCurrent, implementationIsRunning,
  implementationFiles, implementationChecks,
  implementationUnresolved, implementationProgress,
  implementationPackageUrl, implementationConversation,
  implementationCheckSummary, implementationAcceptance, implementationRepairHalt,
  implementationAction, createImplementationAdmissionGate,
  implementationTiming, implementationOverview, implementationReport, implementationActivityStatus,
  implementationSampleSummary, implementationSavedActivity, implementationWaitingActivity,
  getImplementationPageFeedbackStore,
} from "./implementation-panel.mjs";
import "./implementation-panel.css";
import { CheckerSetup } from "./checker-setup.jsx";
import { BuildSelectionRecovery } from "./build-selection-recovery.jsx";
import { packagePreparationReason, retainsCandidateFramework, candidateRuntimeRefreshReady,
  PACKAGE_REVISION_PROMPT } from "./package-availability.mjs";

const implementationUrl = (id) => `/api/guide/${encodeURIComponent(id)}/implementation`;

function useImplementationReceipt(guide, admittedReceipt, selectionScope) {
  const selectionToken = selectionScope?.token();
  const selectionAvailable = guideSelectionAvailable(selectionScope, guide, selectionToken);
  const [snapshot, setSnapshot] = useState({ guideId: null, selectionToken: null, receipt: null, error: "", observation: "loading" });
  const latest = useRef(guide), wakePoll = useRef(null), admission = useRef(0);
  const incoming = useRef(null);
  if (!incoming.current) incoming.current = createImplementationAdmissionGate();
  latest.current = guide;
  useEffect(() => {
    if (!guide?.id) return;
    let stopped = false, timer, inFlight = false, wakeRequested = false;
    setSnapshot(previous => ({ guideId: guide.id, selectionToken,
      receipt: previous.guideId === guide.id ? previous.receipt : null, error: "", observation: "loading" }));
    async function poll() {
      if (stopped || inFlight) return;
      inFlight = true;
      const epoch = admission.current;
      const ownerCurrent = captureGuideSelection(selectionScope, latest.current, selectionToken);
      let delay = 6000;
      try {
        if (!ownerCurrent) return;
        setSnapshot(previous => previous.observation === "known" ? previous
          : { ...previous, observation: "loading", error: "" });
        const receipt = await request(implementationUrl(guide.id), { timeoutMs: 30000 });
        if (stopped || !ownerCurrent() || epoch !== admission.current) return;
        setSnapshot({ guideId: guide.id, selectionToken, receipt, error: "", observation: "known" });
        delay = implementationIsRunning(receipt) ? 1200 : 6000;
      } catch (failure) {
        if (stopped || !ownerCurrent?.() || epoch !== admission.current) return;
        if (failure.status === 404) {
          setSnapshot({ guideId: guide.id, selectionToken, receipt: null, error: "", observation: "known" });
          delay = 15000;
        } else {
          setSnapshot((previous) => ({ ...previous, error: failure.message, errorCode: failure.code, observation: "unknown" }));
          delay = 3000;
        }
      } finally {
        inFlight = false;
        if (!stopped) {
          timer = setTimeout(poll, wakeRequested ? 0 : delay);
          wakeRequested = false;
        }
      }
    }
    const wake = () => {
      clearTimeout(timer);
      if (inFlight) wakeRequested = true;
      else poll();
    };
    wakePoll.current = wake;
    poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
      if (wakePoll.current === wake) wakePoll.current = null;
    };
  }, [guide?.id, selectionToken]);
  useEffect(() => { if (selectionAvailable) wakePoll.current?.(); }, [guide?.id, guide?.version, selectionToken, selectionAvailable]);
  const accept = useCallback((receipt) => {
    if (!guideSelectionAvailable(selectionScope, latest.current)) return;
    if (!implementationIsCurrent(receipt, latest.current)) return;
    admission.current += 1;
    setSnapshot({ guideId: receipt.guide_id, selectionToken: selectionScope?.token(), receipt, error: "", observation: "known" });
    wakePoll.current?.();
  }, []);
  useEffect(() => {
    const receipt = incoming.current.consume(admittedReceipt, guide);
    if (receipt) accept(receipt);
  }, [admittedReceipt, guide?.id, guide?.version, accept]);
  return {
    ...(snapshot.guideId === guide?.id && snapshot.selectionToken === selectionToken && selectionAvailable
      ? snapshot : { guideId: guide?.id, selectionToken,
        receipt: snapshot.guideId === guide?.id ? snapshot.receipt : null, error: "", observation: "loading" }),
    accept, refresh: () => wakePoll.current?.(),
  };
}

function useImplementationTimeline(guideId, receipt, refresh, selectionScope) {
  const [snapshot, setSnapshot] = useState({ buildId: null, frames: [], error: "" });
  const currentReceipt = useRef(receipt), refreshRef = useRef(refresh);
  currentReceipt.current = receipt;
  refreshRef.current = refresh;
  const buildId = receipt?.build_id;
  useEffect(() => {
    if (!guideId || !buildId) return;
    let stopped = false, timer, after;
    const log = createGuideFrameLog();
    setSnapshot({ buildId, frames: [], error: "" });
    async function poll() {
      const ownerCurrent = captureGuideSelection(selectionScope, { id: guideId });
      let delay = implementationIsRunning(currentReceipt.current) ? 1200 : 6000;
      try {
        if (!ownerCurrent) return;
        const query = new URLSearchParams({ build_id: buildId, limit: "200" });
        if (after) query.set("after", after);
        const page = await request(`${implementationUrl(guideId)}/timeline?${query}`);
        if (stopped || !ownerCurrent?.()) return;
        if (page.build_id !== buildId) throw new Error("The selected application build changed.");
        const frames = ingestGuideTimeline(log, page);
        setSnapshot({ buildId, frames: [...frames], error: "" });
        const next = page.next_cursor || after;
        if (page.has_more && next && next !== after) delay = 0;
        after = next;
      } catch (failure) {
        if (stopped || !ownerCurrent?.()) return;
        setSnapshot((previous) => ({ ...previous, error: failure.message }));
        if (failure.status === 409) refreshRef.current?.();
        delay = 3000;
      } finally {
        if (!stopped) timer = setTimeout(poll, delay);
      }
    }
    poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [guideId, buildId]);
  return snapshot.buildId === buildId ? snapshot : { frames: [], error: "" };
}

export function BuilderActivity({ guideId, receipt, refresh, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, { id: guideId }, selectionToken);
  const timeline = useImplementationTimeline(guideId, receipt, refresh, selectionScope);
  const currentPhase = implementationActivityStatus(receipt);
  const scroller = useRef(null), following = useRef(true);
  const { viewState } = useMemo(() => implementationConversation(receipt, timeline.frames),
    [receipt.build_id, receipt.native_identity, timeline.frames]);
  useEffect(() => {
    if (following.current && scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [timeline.frames]);
  return <details open={implementationIsRunning(receipt)} className="implementation-activity">
    <summary>Builder and reviewer activity</summary>
    <div className="implementation-native setup-native-chat" ref={scroller} data-native-implementation-chat="true"
      onScroll={(event) => {
        const node = event.currentTarget;
        following.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
      }}>
      <SpriteSheet />
      <ConversationTranscript viewState={viewState} Icon={ConsoleIcon} compact showTurnDiff={false} />
      {!timeline.frames.length && <p className="implementation-detail">{implementationIsRunning(receipt)
        ? "Waiting for the builder's live activity..." : "No native activity was recorded for this build."}</p>}
    </div>
    {currentPhase && <p className="implementation-detail implementation-current-phase" role="status">
      <strong>{currentPhase.label}</strong>{currentPhase.measurement && <> - {currentPhase.measurement}</>}
    </p>}
    {timeline.error && <p className="implementation-error" role="status">Live activity interrupted. Reconnecting...</p>}
    {receipt.browser_console_url && <a className="implementation-link" href={selectionBlocked ? undefined : receipt.browser_console_url}
      aria-disabled={selectionBlocked} tabIndex={selectionBlocked ? -1 : undefined}
      target="_blank" rel="noreferrer">Open builder console</a>}
  </details>;
}

/** Display only the source-bound actions already selected by setup progress. */
export function ImplementationNextActions({ receipt, state, navigation, onNavigate, Icon, blocked = false }) {
  if (blocked || typeof onNavigate !== "function" || !implementationCanDownload(receipt, state) ||
      navigation?.guide_id !== state.id || navigation.guide_version !== state.version ||
      navigation.build_id !== receipt.build_id || navigation.source_digest !== receipt.source_digest ||
      typeof navigation.source_digest !== "string" || !navigation.source_digest) return null;
  const actions = navigation.actions?.filter((action) => ["trial", "deployment", "console"].includes(action.target)) || [];
  const hasApplicationAction = actions.some((action) => action.target !== "console");
  return <>{actions.map((action) => action.target === "console"
    ? <a key={action.target} className={`setup-action${hasApplicationAction ? "" : " primary"}`} href={action.url} target="_blank" rel="noreferrer">{action.label} <Icon name="arrow" size={15} /></a>
    : <button key={action.target} className="setup-action primary" onClick={() => onNavigate(action)}>{action.label} <Icon name="arrow" size={15} /></button>)}</>;
}

export function ImplementationSavedActivity({ receipt, state }) {
  const activity = implementationSavedActivity(receipt, state);
  if (!activity) return null;
  return <div className="implementation-detail implementation-saved-activity" role="status" aria-live="polite" aria-atomic="true">
    <strong>{activity.label}</strong>
    <ul>{activity.rows.map((row) => <li key={row.kind}>{row.message}{" "}
      <time dateTime={row.at}>{new Date(row.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>
    </li>)}</ul>
  </div>;
}

export function ImplementationWaitingActivity({ receipt, state }) {
  const activity = implementationWaitingActivity(receipt, state);
  if (!activity) return null;
  return <div className="implementation-detail implementation-host-progress" role="status" aria-live="polite" aria-atomic="true">
    <strong>{activity.current ? "Last saved host step" : "Last saved host step for the earlier proposal"}</strong>
    <p>{activity.message}</p>
    <time dateTime={activity.at}>{new Date(activity.at).toLocaleString()}</time>
  </div>;
}

export function ImplementationPanel({ state, plan, designBusy, pendingManual, Icon, onReceipt, onDownloaded, admittedReceipt, navigation, onNavigate, selectionScope, onRevise }) {
  const builds = useImplementationReceipt(state, admittedReceipt, selectionScope);
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const [feedbackStore] = useState(() => getImplementationPageFeedbackStore());
  const [feedback, setFeedback] = useState(() => ({ guideId: state.id, ...feedbackStore.read(state.id) }));
  const [submissions, setSubmissions] = useState({}), [error, setError] = useState("");
  const submitting = !!submissions[state.id];
  const [stopSubmission, setStopSubmission] = useState(null);
  const [downloadRequests, setDownloadRequests] = useState({}), [filesOpen, setFilesOpen] = useState(false);
  const downloading = !!downloadRequests[state.id];
  const downloadPending = useRef(new Map());
  const [fileView, setFileView] = useState({ key: "", path: "", content: "", error: "", loading: false });
  const [checker, setChecker] = useState(null);
  const latest = useRef(state), selection = useRef(0), feedbackEditRevision = useRef(0);
  const navigationLifetime = useRef({ guideId: state.id, version: state.version, revision: 0 });
  if (navigationLifetime.current.guideId !== state.id || navigationLifetime.current.version !== state.version) {
    navigationLifetime.current = { guideId: state.id, version: state.version,
      revision: navigationLifetime.current.revision + 1 };
  }
  useEffect(() => () => { navigationLifetime.current.revision += 1; }, []);
  latest.current = state;
  const receipt = builds.receipt;
  const latestReceipt = useRef(receipt);
  const stoppingBuilds = useRef(new Set());
  const buildAdmissionPending = useRef(new Map());
  latestReceipt.current = receipt;
  const pendingSubmission = submissions[state.id];
  const ownsPendingSubmission = !!pendingSubmission &&
    buildAdmissionPending.current.get(state.id) === pendingSubmission &&
    pendingSubmission.guideId === state.id && pendingSubmission.version === state.version &&
    pendingSubmission.navigationRevision === navigationLifetime.current.revision &&
    guideSelectionAvailable(selectionScope, state, pendingSubmission.selectionToken);
  // This describes only our outstanding POST, not a host lifecycle or a saved
  // build. A current newly selected receipt takes precedence before POST settles.
  const admissionPending = ownsPendingSubmission && !(builds.observation === "known" &&
    implementationIsCurrent(receipt, state) && receipt.build_id !== pendingSubmission.previousBuildId);
  useEffect(() => {
    onReceipt?.(receipt, { guideId: builds.guideId, selectionToken: builds.selectionToken,
      observation: builds.observation, pendingAdmissionVersion: admissionPending ? state.version : null });
  }, [receipt, builds.guideId, builds.selectionToken, builds.observation, admissionPending, state.version, onReceipt]);
  const selectedFeedback = feedback.guideId === state.id ? feedback : feedbackStore.read(state.id);
  const text = selectedFeedback.text;
  useEffect(() => {
    feedbackEditRevision.current += 1;
    setFeedback({ guideId: state.id, ...feedbackStore.read(state.id) });
    setError(""); setFilesOpen(false); setFileView({ key: "", path: "", content: "", error: "", loading: false });
    selection.current += 1;
  }, [state.id]);
  useEffect(() => {
    selection.current += 1;
    setFileView({ key: "", path: "", content: "", error: "", loading: false });
  }, [receipt?.build_id]);
  const running = implementationIsRunning(receipt);
  const cleanupUnavailable = receipt?.cleanup_pending === true && receipt.cleanup_custody_lost === true;
  const cleanupRetry = receipt?.cleanup_pending === true && receipt.cleanup_in_progress !== true &&
    ["completed", "failed", "interrupted"].includes(receipt.status);
  const stopping = running && (receipt?.cleanup_in_progress === true || !cleanupRetry && receipt?.stop_requested === true ||
    stopSubmission?.guideId === state.id && stopSubmission.buildId === receipt?.build_id);
  const current = implementationIsCurrent(receipt, state);
  const statusUnconfirmed = builds.observation !== "known";
  const selectionUnavailable = statusUnconfirmed && builds.errorCode === "build_selection_unavailable";
  const overview = admissionPending ? {
    title: "Build request in progress",
    message: "Your build request is still in progress. Waiting for its saved build status.",
    next: "The confirmed build will appear here. Your builder note is kept while you wait.",
  } : statusUnconfirmed ? {
    title: selectionUnavailable ? "Your saved build needs attention"
      : builds.observation === "loading" ? "Checking your saved build" : "Build status is unavailable",
    message: selectionUnavailable ? "Forge found saved work but could not identify the selected build."
      : builds.observation === "loading"
      ? "Forge is checking for saved application work before starting a build."
      : "Forge could not check the latest build status. Any earlier build shown below is its last saved status.",
    next: selectionUnavailable ? "Check the saved work below. Your builder note is kept."
      : builds.observation === "loading" ? "You can keep editing your builder note while this is checked."
      : "Refresh build status to continue. Your builder note is kept while you wait.",
    attention: builds.observation === "unknown",
  } : implementationOverview(receipt, state);
  const report = implementationReport(receipt), builderSummary = report.find(entry => entry.label === "Builder summary");
  const diagnostics = report.filter(entry => entry.label !== "Builder summary");
  const samples = implementationSampleSummary(receipt, state);
  const timing = implementationTiming(receipt);
  const files = implementationFiles(receipt), checks = implementationChecks(receipt);
  const checkSummary = implementationCheckSummary(receipt);
  const acceptance = implementationAcceptance(receipt, state);
  const repairHalt = implementationRepairHalt(receipt, state);
  const unresolved = implementationUnresolved(receipt), progress = implementationProgress(receipt);
  const canDownload = implementationCanDownload(receipt, state);
  const packageActionClass = !statusUnconfirmed && canDownload && receipt.source_verified === true && acceptance?.passed === true
    ? "setup-action" : "setup-action primary";
  const decisionBlocked = (state.readiness?.decision_blockers?.length || 0) > 0;
  const packageReason = packagePreparationReason(state, plan);
  const retainedContinuation = !statusUnconfirmed && retainsCandidateFramework(receipt, state);
  const candidateRefreshBlocked = retainedContinuation && !candidateRuntimeRefreshReady(receipt, state);
  const canBuild = (!packageReason || retainedContinuation) && !statusUnconfirmed && !decisionBlocked && checker?.status === 'ready' && !!(state.proposal || state.blueprint) && !designBusy && !pendingManual && !submitting && !running;
  const needsFeedback = current && receipt?.status === "completed" && acceptance?.passed;
  const canRefreshFramework = current && ["completed", "failed", "interrupted"].includes(receipt?.status) &&
    typeof receipt?.build_id === "string" && !!receipt.build_id && receipt.worker_stop_confirmed === true &&
    receipt.cleanup_pending !== true && receipt.cleanup_in_progress !== true && receipt.cleanup_custody_lost !== true && !running;
  const canStartFresh = !statusUnconfirmed && receipt?.guide_id === state.id &&
    receipt?.framework?.runtime_candidate?.format === "forge-meta-toolkit-runtime-selection-v1" &&
    ["failed", "interrupted"].includes(receipt.status) &&
    typeof receipt.build_id === "string" && !!receipt.build_id && receipt.worker_stop_confirmed === true &&
    receipt.cleanup_pending !== true && receipt.cleanup_in_progress !== true && receipt.cleanup_custody_lost !== true && !running &&
    /^[a-f0-9]{64}$/.test(receipt.source_digest || "");
  const canShowSavedBuild = !statusUnconfirmed && canRefreshFramework && !!pendingSubmission &&
    buildAdmissionPending.current.get(state.id) === pendingSubmission &&
    pendingSubmission.guideId === state.id && pendingSubmission.version === state.version &&
    pendingSubmission.navigationRevision === navigationLifetime.current.revision &&
    guideSelectionAvailable(selectionScope, state, pendingSubmission.selectionToken) &&
    receipt.build_id !== pendingSubmission.previousBuildId;
  const actionLabel = implementationAction(receipt, state);

  function showSavedBuild() {
    const submitted = pendingSubmission;
    const ownerCurrent = captureGuideSelection(selectionScope, state, submitted?.selectionToken);
    if (!ownerCurrent || !canShowSavedBuild || buildAdmissionPending.current.get(state.id) !== submitted ||
        latest.current?.id !== submitted.guideId || latest.current?.version !== submitted.version ||
        navigationLifetime.current.revision !== submitted.navigationRevision || latestReceipt.current !== receipt) return;
    // Leave the response pending; only its exact local presentation is detached.
    buildAdmissionPending.current.delete(submitted.guideId);
    setSubmissions(previous => {
      if (previous[submitted.guideId] !== submitted) return previous;
      const next = { ...previous }; delete next[submitted.guideId]; return next;
    });
  }

  async function build(refreshFramework = false, startFresh = false) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !canBuild || packagePreparationReason(latest.current) &&
        (refreshFramework || startFresh || !retainsCandidateFramework(latestReceipt.current, latest.current)) ||
        (latest.current?.readiness?.decision_blockers?.length || 0) > 0 ||
        buildAdmissionPending.current.has(state.id) || refreshFramework && !canRefreshFramework ||
        startFresh && !canStartFresh ||
        refreshFramework && retainsCandidateFramework(latestReceipt.current, latest.current) &&
          !candidateRuntimeRefreshReady(latestReceipt.current, latest.current) ||
        !refreshFramework && !startFresh && needsFeedback && !text.trim()) return;
    // A host-selected product moves to the host's current runtime only through
    // the host-owned refresh of its exact accepted build, which carries no
    // request: the unsent draft stays with the page.
    const candidateRefresh = refreshFramework && retainsCandidateFramework(latestReceipt.current, latest.current);
    const submitted = { guideId: state.id, version: state.version, text: candidateRefresh || startFresh ? "" : text,
      previousSourceDigest: receipt?.source_digest,
      editRevision: feedbackEditRevision.current,
      previousBuildId: receipt?.build_id || null, navigationRevision: navigationLifetime.current.revision, selectionToken };
    const ownsAdmission = () => buildAdmissionPending.current.get(submitted.guideId) === submitted;
    const stillHere = () => ownsAdmission() && ownerCurrent() && latest.current?.id === submitted.guideId &&
      latest.current?.version === submitted.version && navigationLifetime.current.revision === submitted.navigationRevision;
    buildAdmissionPending.current.set(submitted.guideId, submitted);
    setSubmissions(previous => ({ ...previous, [submitted.guideId]: submitted })); setError("");
    try {
      const body = { version: submitted.version, request: submitted.text.trim(), previous_build_id: submitted.previousBuildId };
      if (startFresh) {
        body.start_fresh = true;
        body.previous_source_digest = submitted.previousSourceDigest;
      } else if (candidateRefresh) {
        body.refresh_framework = true;
        body.preserve_design = true;
      } else if (refreshFramework) {
        body.refresh_framework = true;
        body.request ||= "Rebuild this application using the current MobKit runtime. Preserve its requested behavior, agent design and authored application code. Verify the refreshed application and run independent review.";
        body.request += `\n\nRefresh the runtime in build ${submitted.previousBuildId}.`;
      }
      const next = await request(implementationUrl(submitted.guideId), {
        method: "POST", body,
      });
      if (!stillHere()) {
        if (ownsAdmission() && ownerCurrent() && latest.current?.id === submitted.guideId) builds.refresh();
        return;
      }
      const selectedBuildId = latestReceipt.current?.build_id || null;
      if (selectedBuildId === submitted.previousBuildId || selectedBuildId === next.build_id) builds.accept(next);
      else builds.refresh();
      setFeedback((previous) => feedbackEditRevision.current === submitted.editRevision &&
        previous.guideId === submitted.guideId && previous.text === submitted.text
        ? { ...previous, ...feedbackStore.write(submitted.guideId, "") } : previous);
    } catch (failure) {
      if (stillHere()) { setError(failure.message); builds.refresh(); }
      else if (ownsAdmission() && ownerCurrent() && latest.current?.id === submitted.guideId) builds.refresh();
    } finally {
      if (buildAdmissionPending.current.get(submitted.guideId) === submitted) buildAdmissionPending.current.delete(submitted.guideId);
      setSubmissions(previous => {
        if (previous[submitted.guideId] !== submitted) return previous;
        const next = { ...previous }; delete next[submitted.guideId]; return next;
      });
    }
  }

  async function stopBuild() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !running || !receipt?.build_id || stopping || cleanupUnavailable) return;
    const submitted = { guideId: state.id, version: state.version, buildId: receipt.build_id,
      navigationRevision: navigationLifetime.current.revision };
    const key = `${submitted.guideId}:${submitted.buildId}`;
    if (stoppingBuilds.current.has(key)) return;
    stoppingBuilds.current.add(key);
    setStopSubmission(submitted); setError("");
    const stillSelected = () => ownerCurrent() && latest.current?.id === submitted.guideId &&
      latest.current?.version === submitted.version && latestReceipt.current?.build_id === submitted.buildId &&
      navigationLifetime.current.revision === submitted.navigationRevision;
    try {
      const next = await request(`${implementationUrl(submitted.guideId)}/stop`, {
        method: "POST", body: { version: submitted.version, build_id: submitted.buildId },
      });
      if (!stillSelected()) {
        if (ownerCurrent() && latest.current?.id === submitted.guideId) builds.refresh();
        return;
      }
      if (next.build_id === submitted.buildId) builds.accept(next);
      builds.refresh();
    } catch (failure) {
      if (stillSelected()) { setError(failure.message); builds.refresh(); }
      else if (ownerCurrent() && latest.current?.id === submitted.guideId) builds.refresh();
    } finally {
      stoppingBuilds.current.delete(key);
      setStopSubmission((previous) => previous?.guideId === submitted.guideId &&
        previous.buildId === submitted.buildId ? null : previous);
    }
  }

  async function inspectFile(path) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !receipt?.build_id) return;
    const token = ++selection.current;
    const key = `${state.id}:${receipt.build_id}:${path}`;
    setFileView({ key, path, content: "", error: "", loading: true });
    try {
      const query = new URLSearchParams({ path, build_id: receipt.build_id });
      const result = await request(`${implementationUrl(state.id)}/files?${query}`);
      if (ownerCurrent() && selection.current === token) setFileView({ key, path: result.path, content: result.content, error: "", loading: false });
    } catch (failure) {
      if (ownerCurrent() && selection.current === token) setFileView({ key, path, content: "", error: failure.message, loading: false });
      if (ownerCurrent() && failure.status === 409) builds.refresh();
    } finally {
      if (selection.current === token) setFileView(previous => previous.key === key ? { ...previous, loading: false } : previous);
    }
  }

  async function download() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent || !canDownload || downloading || downloadPending.current.has(state.id)) return;
    const guideId = state.id, buildId = receipt.build_id, guideVersion = state.version,
      navigationRevision = navigationLifetime.current.revision;
    const stillSelected = () => ownerCurrent() && latest.current?.id === guideId && latest.current?.version === guideVersion &&
      latestReceipt.current?.build_id === buildId && navigationLifetime.current.revision === navigationRevision;
    const submitted = { guideId };
    downloadPending.current.set(guideId, submitted);
    setDownloadRequests(previous => ({ ...previous, [guideId]: submitted })); setError("");
    try {
      const blob = await requestBlob(implementationPackageUrl(receipt, state), { timeoutMs: 60000 });
      if (!stillSelected()) return;
      const url = URL.createObjectURL(blob), link = document.createElement("a");
      link.href = url;
      link.download = `${plan?.project?.slug || "mobkit"}-application.zip`;
      link.click();
      onDownloaded?.();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (failure) {
      if (stillSelected()) { setError(failure.message); builds.refresh(); }
    } finally {
      if (downloadPending.current.get(guideId) === submitted) downloadPending.current.delete(guideId);
      setDownloadRequests(previous => {
        if (previous[guideId] !== submitted) return previous;
        const next = { ...previous }; delete next[guideId]; return next;
      });
    }
  }

  if (!plan?.project || !(state.proposal || state.blueprint)) return null;
  return <section className="implementation-panel" id="setup-implementation" aria-label="Application implementation"
    aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)}>
    <div className={`implementation-kicker ${running || submitting ? "is-active" : ""}`}>
      <i /> Implementation agent
    </div>
    <h2>{overview.title || `Bring ${plan.project.name} to life`}</h2>
    <p role={overview.attention ? "status" : undefined}>{!statusUnconfirmed && packageReason && !running && !retainedContinuation ? "Your design can be accepted and refined while package preparation is pending." : overview.message}</p>
    <p className="implementation-next">{!statusUnconfirmed && packageReason && !running && !retainedContinuation ? "Review the limitation with your guide before starting a new build." : overview.next}</p>
    {packageReason && <div className="implementation-warning" role="status">
      <strong>New package preparation is pending</strong><p>{packageReason}</p>
      {retainedContinuation && <p>You can continue this saved application. Rebuilding it with the current runtime still needs support for this design.</p>}
      {onRevise && <button className="implementation-link" disabled={selectionBlocked || designBusy || pendingManual || submitting}
        onClick={() => {
          if (guideSelectionAvailable(selectionScope, state, selectionToken) && latest.current?.version === state.version && packagePreparationReason(latest.current))
            onRevise(PACKAGE_REVISION_PROMPT, { id: state.id, version: state.version });
        }}>Revise with guide</button>}
    </div>}
    {repairHalt && <div className="implementation-warning implementation-repair-halt" role="status" aria-live="polite">
      <strong>Automatic repair paused</strong><p>{repairHalt.message}</p><p>{repairHalt.next}</p>
    </div>}
    {selectionUnavailable && !admissionPending &&
      <BuildSelectionRecovery state={state} selectionScope={selectionScope}
        blocked={selectionBlocked || designBusy || pendingManual || submitting} onRefresh={builds.refresh} />}
    <ImplementationWaitingActivity receipt={receipt} state={state} />
    <ImplementationSavedActivity receipt={receipt} state={state} />
    {!running && <CheckerSetup onReadiness={setChecker} />}
    {timing && <details className="implementation-timing">
      <summary>{timing.summary}</summary>
      <dl>{timing.phases.map((row) => <div key={row.phase}>
        <dt>{row.label}{row.count > 1 ? ` (${row.count})` : ""}</dt><dd>{row.duration}</dd>
      </div>)}</dl>
      <p>Native runs include tool work and result delivery. These measurements overlap; adding them would count some work twice. Provider inference, queueing, network and cache times are not separately measured.</p>
      {timing.partial && <p>Some build timing is unavailable. These are the retained measurements.</p>}
    </details>}
    {receipt && !current && <div className="implementation-warning" role="status">
      <strong>Your proposal has changed since this build.</strong>
      These files belong to proposal {receipt.guide_version}. Build the current proposal before downloading a package.
    </div>}
    <div className="implementation-actions">
      <ImplementationNextActions receipt={receipt} state={state} navigation={navigation} onNavigate={onNavigate}
        Icon={Icon} blocked={selectionBlocked || statusUnconfirmed || designBusy || pendingManual || submitting} />
      {canShowSavedBuild && <button className="setup-action primary" onClick={showSavedBuild}>Show saved build</button>}
      {statusUnconfirmed && !admissionPending && <button className="setup-action primary" disabled={selectionBlocked || builds.observation === "loading"}
        onClick={() => { if (guideSelectionAvailable(selectionScope, state, selectionToken)) builds.refresh(); }}>
        {builds.observation === "loading" ? "Checking build status..." : "Refresh build status"}
      </button>}
      {!running && (!statusUnconfirmed || receipt || admissionPending) && <button className={packageActionClass} onClick={() => build()} disabled={selectionBlocked || !canBuild || needsFeedback && !text.trim()}>
        {submitting ? "Starting builder..." : actionLabel} <Icon name="arrow" size={15} />
      </button>}
      {running && <button className="setup-action" onClick={stopBuild} disabled={selectionBlocked || stopping || cleanupUnavailable}>
        {cleanupUnavailable ? "Cleanup unavailable" : stopping ? receipt?.stop_requested === true || stopSubmission ? "Stopping builder..." : "Closing build runtime..."
          : cleanupRetry ? "Retry cleanup" : "Stop build"}
      </button>}
      {canDownload && <button className={packageActionClass} onClick={download} disabled={selectionBlocked || downloading}>
        {downloading ? "Preparing download..." : "Download checked application"} <Icon name="download" size={15} />
      </button>}
      {files.length > 0 && <button className="implementation-link" onClick={() => setFilesOpen(!filesOpen)} aria-expanded={filesOpen}>
        {filesOpen ? "Close source files" : `Inspect ${files.length} source files`}
      </button>}
    </div>
    {canShowSavedBuild && <p className="implementation-detail" role="status">Forge found a newer saved build. Show it to continue and keep your notes.</p>}
    {(designBusy || pendingManual) && !running && <p className="implementation-detail">{pendingManual
      ? "Send your manual changes back to the guide before building."
      : "The guide is revising your proposal. Building will be available when this revision is saved."}</p>}
    {decisionBlocked && !running && <p className="implementation-detail" role="status">Answer the guide's open setup decisions before building. Proposed defaults and remaining implementation work can be built once those decisions are clear.</p>}
    {canDownload && <p className="implementation-detail">The package passed the checks below. Installation, credentials, and any remaining integrations still need to be verified in your deployment.</p>}
    {receipt?.status === "completed" && current && !running && !canDownload && <p className="implementation-detail">This build does not have a current verified package yet. Continue implementation to resolve its checks before downloading.</p>}
    {builderSummary && <details className="implementation-builder-summary">
      <summary>Builder summary</summary><p>{builderSummary.text}</p>
    </details>}
    <div className="implementation-feedback">
      <label htmlFor={`implementation-feedback-${state.id}`}>{running ? "Note for the next implementation turn" : receipt ? "What should the builder improve or finish?" : "Anything to add before building? (optional)"}</label>
      {running && <p id={`implementation-feedback-help-${state.id}`} className="implementation-detail">This note is not sent to the running build. When the build ends, use the next implementation action to send it.</p>}
      <textarea disabled={selectionBlocked} id={`implementation-feedback-${state.id}`} value={text}
        aria-describedby={running ? `implementation-feedback-help-${state.id}` : undefined}
        placeholder={receipt ? "Describe a change, a missing behavior, or how to improve the application..." : "The builder already has your conversation and proposed agent system."}
        onChange={(event) => {
          feedbackEditRevision.current += 1;
          setFeedback({ guideId: state.id, ...feedbackStore.write(state.id, event.target.value) });
        }} />
      {!selectedFeedback.persistent && <div className="implementation-detail" role="status">
        <p>Your current builder note is kept in this open page only. Browser storage could not be checked or updated; reloading or closing Forge can lose this edit or restore an older note.</p>
        <button className="implementation-link" type="button" disabled={selectionBlocked} onClick={() => {
          if (guideSelectionAvailable(selectionScope, state, selectionToken))
            setFeedback({ guideId: state.id, ...feedbackStore.retry(state.id) });
        }}>Retry saving builder note</button>
      </div>}
    </div>
    {samples && <p className="implementation-detail">{samples.message}</p>}
    {(canRefreshFramework || canStartFresh) && <details className="implementation-runtime-update">
      <summary>Runtime and host updates</summary>
      {canRefreshFramework && <>
        <p className="implementation-detail">Rebuild with the runtime and generated application host available in this installer, keeping your application work. The new package will be tested and independently reviewed. Your installed application and any packages you have downloaded stay available. You can recover the installed version's source package from its Run area.</p>
        {candidateRefreshBlocked && <p className="implementation-detail">The runtime update starts from this application's independently accepted build.</p>}
        <button className="setup-action" onClick={() => build(true)} disabled={selectionBlocked || !canBuild || !!packageReason || candidateRefreshBlocked}>Rebuild with updated runtime</button>
      </>}
      {canStartFresh && <>
        <p className="implementation-detail">You can build again from your current setup using the current runtime. The previous source and history stay saved; the new build starts without copying the unfinished application or its checks. Your unsent builder note stays here.</p>
        <button className="setup-action" onClick={() => build(false, true)} disabled={selectionBlocked || !canBuild || !!packageReason}>Start a fresh build</button>
      </>}
    </details>}
    {diagnostics.length > 0 && <details className="implementation-report">
      <summary>Build diagnostics - {diagnostics.length} {diagnostics.length === 1 ? "entry" : "entries"}</summary>
      {diagnostics.map((entry) => <div key={entry.label}><strong>{entry.label}</strong><p>{entry.text}</p></div>)}
    </details>}
    {checks.length > 0 && checkSummary.saved && <p className="implementation-detail">Current source is not yet verified. These are saved check results.</p>}
    {checks.length > 0 && <details className={`implementation-check-disclosure ${checkSummary.attention ? "needs-attention" : "is-pass"}`}
      key={`checks:${receipt.build_id}:${receipt.receipt?.id || "latest"}`}>
      <summary>{checkSummary.label}</summary>
      <ul className="implementation-checks" aria-label="Implementation checks">
      {checks.map((check, index) => <li className={checkSummary.saved ? "is-saved" : `is-${check.status}`} key={`${check.name}:${index}`}>
        <span className="implementation-check-icon" aria-hidden="true">{check.status === "pass" ? "+" : check.status === "fail" ? "!" : "?"}</span>
        <div><strong>{check.name}</strong><small>{check.status === "pass" ? (checkSummary.saved ? "Previously passed" : "Passed") : check.status === "fail" ? (checkSummary.saved ? "Previously failed" : "Failed") : "Not verified"}</small>
          {check.detail && <p>{check.detail}</p>}</div>
      </li>)}
      </ul>
    </details>}
    {acceptance && <details className={`implementation-acceptance ${acceptance.passed ? "is-pass" : "needs-attention"}`}
      open={repairHalt ? true : undefined}
      key={`acceptance:${receipt.build_id}:${acceptance.review_round}:${acceptance.passed}:${!!repairHalt}`}>
      <summary>{acceptance.passed ? "Independent offline review passed" : "Independent review found work to do"} - {acceptance.requirements.length} requested {acceptance.requirements.length === 1 ? "outcome" : "outcomes"}, {acceptance.counterexamples.length} {acceptance.counterexamples.length === 1 ? "finding" : "findings"}</summary>
      <p>{acceptance.summary}</p>
      <small>{acceptance.tests_run} protected reviewer tests run - review {acceptance.review_round}. Live connections and deployment need separate verification.</small>
      <ul aria-label="Requested outcome evidence">{acceptance.requirements.map((outcome) => <li key={outcome.id}>
        <strong>{outcome.text}</strong><span className="implementation-evidence-status">{outcome.status === "tested" ? "Offline tested"
          : outcome.status === "source" ? "Implemented in source" : outcome.status === "unresolved" ? "Needs completion" : "Proposed"}</span>
        <p>{outcome.evidence}</p>
        {outcome.remaining_dependencies?.length > 0 && <p className="implementation-detail">Remaining: {outcome.remaining_dependencies.join("; ")}</p>}
      </li>)}</ul>
      {acceptance.counterexamples.length > 0 && <ul aria-label="Independent review findings">{acceptance.counterexamples.map((finding) => <li key={finding}>{finding}</li>)}</ul>}
    </details>}
    {receipt?.status === "completed" && current && !running && !acceptance && <p className="implementation-detail">
      This source has no current independent review. Review and improve it before installing.
    </p>}
    {unresolved.length > 0 && <details className="implementation-warning">
      <summary>{unresolved.length} remaining {unresolved.length === 1 ? "item" : "items"} to connect or complete</summary>
      <ul>{unresolved.map((item) => <li key={item}>{item}</li>)}</ul>
    </details>}
    {receipt?.request && <details><summary>Your build instruction</summary><p className="implementation-detail">{receipt.request}</p></details>}
    {receipt && <BuilderActivity key={receipt.build_id} guideId={state.id} receipt={receipt} refresh={builds.refresh} selectionScope={selectionScope} />}
    {progress.length > 0 && <details><summary>Build progress</summary>
      <ol className="implementation-progress">{progress.map((event, index) => <li key={`${event.at}:${index}`}>{event.message}</li>)}</ol>
    </details>}
    {filesOpen && <div className="implementation-files">
      <div className="implementation-file-list" aria-label="Generated source files">{files.map((file) =>
        <button disabled={selectionBlocked} key={file.path} onClick={() => inspectFile(file.path)} aria-current={fileView.path === file.path ? "true" : undefined}>
          {file.path}{file.modified ? " (authored)" : ""}
        </button>)}</div>
      {fileView.path && <div className="implementation-file-caption">{fileView.path} - read-only source</div>}
      {fileView.loading ? <p className="implementation-detail">Loading source...</p>
        : fileView.error ? <p className="implementation-error" role="alert">{fileView.error}</p>
        : fileView.path && <pre className="implementation-file-content"><code>{fileView.content}</code></pre>}
    </div>}
    {(error || builds.error) && !admissionPending && <div className="implementation-error" role="alert">{error || builds.error}
      {builds.error && !admissionPending && !statusUnconfirmed && <button disabled={selectionBlocked} className="implementation-link" onClick={() => {
        if (guideSelectionAvailable(selectionScope, state, selectionToken)) builds.refresh();
      }}>Refresh build status</button>}
    </div>}
  </section>;
}
