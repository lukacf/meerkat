import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from "./guide-selection.mjs";
import React, { useEffect, useMemo, useRef, useState } from "react";
import { ConversationTranscript } from "../../packages/console-components/src/conversation/conversation-transcript";
import { Icon as ConsoleIcon, SpriteSheet } from "../../console/src/icon";
import { request } from "./state.mjs";
import { createGuideFrameLog, ingestGuideTimeline } from "./native-guide-chat.mjs";
import { rehearsalActive, rehearsalCurrent, rehearsalConversation, rehearsalActivityFromTimeline, rehearsalReplay,
  applicationTrialAvailable, applicationTrialSamples, applicationTrialRepair, rehearsalSamplePreview,
  applicationTrialFeedback, applicationTrialImprovement, applicationTrialSummary, rehearsalNavigationMode, rehearsalRunRequest,
  createRehearsalReceiptEpoch, rehearsalCanContinue, rehearsalSessionSummary, admitRehearsalFollowUp,
  applicationTrialPreview, applicationTrialEndToRepair, requestRehearsalStart } from "./rehearsal-panel.mjs";
import "./rehearsal-panel.css";
import { createRehearsalDraftStore, rehearsalSessionStorage, rehearsalSetupDraftOwner,
  rehearsalSetupDraftCanContinue,
  rehearsalTurnDraftOwner, rehearsalDraftAdmissionMatches, rehearsalInitialAcknowledged, rehearsalInitialResponseMatches,
  rehearsalInitialRejected } from "./rehearsal-drafts.mjs";
import { applicationTrialReplayOffer, applicationTrialReplayResult, createTrialReplayStore, trialReplayAcknowledged,
  trialReplayRejected, requestTrialReplay } from "./trial-replay.mjs";

export function RehearsalSameJobReplay({ offer, receipt, action, blocked, disabled, sending, onTry, onRecover, onSeparate }) {
  if (action) return <div className="rehearsal-repair" role="status">
    <h3>Try the same job again</h3>
    <p>{action.start_rejection
      ? "The host confirmed that this exact retry was not admitted. Its saved request is preserved."
      : "This retry is awaiting confirmation. Recover it before starting another trial; recovery keeps the exact saved job and starts no duplicate trial."}</p>
    <button className="setup-action" type="button" disabled={disabled || sending} onClick={onRecover}>
      {sending ? "Recovering retry..." : "Recover saved retry"}</button>
    {action.start_rejection && <button className="implementation-link" type="button" disabled={disabled || sending} onClick={onSeparate}>
      Keep rejected retry and prepare another</button>}
  </div>;
  if (blocked) return <p className="implementation-warning" role="status">This tab's saved retry could not be read.
    Inspect the saved trial and reload when browser storage is available before trying the job again.</p>;
  if (applicationTrialReplayResult(offer, receipt)) return <div className="rehearsal-continuation" role="status">
    <h3>Saved job replay finished</h3>
    <p>Your repaired application ran the original job with its saved sample data and follow-up requests.
      Review the recorded results below. This was an offline trial; the outcome still needs review.</p>
    <button className="implementation-link" type="button" disabled={disabled || sending} onClick={onTry}>
      {sending ? "Starting saved job..." : "Run the saved job again"}</button>
  </div>;
  return offer ? <div className="rehearsal-repair">
    <h3>Try the same job again</h3>
    <p>Run the repaired application with the original saved job, sample data and follow-up requests.
      Your earlier trial stays in its history. This is an offline trial; the new outcome still needs review.</p>
    <button className="setup-action" type="button" disabled={disabled || sending} onClick={onTry}>
      {sending ? "Starting saved job..." : "Try the same job again"}</button>
  </div> : null;
}

export function RehearsalInitialRecovery({ action, blocked, supported, loading, disabled, onRecover, onSeparate, onRetry }) {
  const connectionNotice = !loading && supported === null ? <div className="implementation-warning" role="status">
    <p>Could not check trial recovery. Your draft and any saved trial request are preserved.
      Check the connection again before starting or recovering a trial.</p>
    <button className="implementation-link" type="button" onClick={onRetry}>Retry connection check</button>
  </div> : loading ? <p className="implementation-detail" role="status">Checking trial request recovery...</p> : null;
  if (action) return <>{connectionNotice}<div className="implementation-warning" role="status">
    <p>{action.start_rejection ? "The host confirmed that this exact request was not admitted. Keep its history and correct your draft before preparing a new trial."
      : "This trial request is awaiting confirmation. Recover its saved admission before starting another trial."}
      {" "}Recovery uses the original proposal {action.owner.guide_version} and exact request, not your newer edits.</p>
    <button className="setup-action" type="button" disabled={disabled || loading || !supported} onClick={onRecover}>Recover trial</button>
    {action.preparation_failure && <>
      <p>The earlier preparation did not finish. Keep its saved request history and prepare a separate trial.
        Your drafts stay unchanged; the next Try is a new request, subject to the host's current source and cleanup checks.</p>
      <button className="implementation-link" type="button" disabled={disabled || loading || !supported}
        onClick={onSeparate}>Keep failed start and prepare a new trial</button>
    </>}
    {action.start_rejection && <button className="implementation-link" type="button" disabled={disabled || loading || !supported}
      onClick={onSeparate}>Keep rejected start and correct trial</button>}
    {!loading && supported === false && <p>This Forge host does not support this saved request's recovery. Keep its draft and return to the supporting host.</p>}
  </div></>;
  if (connectionNotice) return connectionNotice;
  if (blocked && supported) return <p className="implementation-warning" role="status">This tab's saved trial recovery could not be read. Your draft remains editable here. Inspect the saved trial and reload when browser storage is available before starting another.</p>;
  return supported === false ? <p className="implementation-detail">This Forge host uses the older trial-start protocol. If a start response is lost, inspect its saved trial before trying again; replay protection is unavailable.</p> : null;
}

export function RehearsalPreparationHistory({ entries }) {
  return entries.length > 0 ? <details className="implementation-detail"><summary>Earlier trial preparations</summary>
    {entries.map((item) => <div key={item.start_request_id}>
      <p>Proposal {item.guide_version}, {item.mode === "application" ? "application" : "design"} trial: {item.status === "rejected"
        ? "the host rejected this exact request before admission. This does not confirm cleanup of any other trial."
        : "preparation did not finish. No trial admission or cleanup is confirmed by this record."}</p>
      <p>Saved request reference: <code>{item.start_request_id}</code></p>
    </div>)}
  </details> : null;
}

export function RehearsalSampleChoices({ samples, selected, disabled, onSelect }) {
  return <fieldset><legend>Choose a sample job, or write your own below</legend>
    {samples.map((item, index) => <button key={index} type="button"
      aria-pressed={selected === index} disabled={disabled} onClick={() => onSelect(item, index)}>
      <strong>{item.label}</strong><span>{rehearsalSamplePreview(item.request)}</span>
    </button>)}
  </fieldset>;
}

export function RehearsalTrialSummary({ receipt, canRepair }) {
  if (!receipt) return null;
  const summary = applicationTrialSummary(receipt);
  return <>
    <p className={summary?.problem ? "implementation-warning" : "implementation-detail"} role="status">
      <strong>{receipt.mode === "application" ? "Saved application trial" : "Saved design trial"}: </strong>
      {summary?.text || receipt.detail || receipt.status}
      {summary?.problem && canRepair && " Choose Fix this trial to ask the builder to investigate."}
    </p>
    {summary && receipt.detail && <details><summary>Saved native trial result</summary><p>{receipt.detail}</p></details>}
    {receipt.current && receipt.browser_console_url && <a className="implementation-link"
      href={receipt.browser_console_url} target="_blank" rel="noreferrer">Open trial console</a>}
  </>;
}

export function RehearsalApplicationPreview({ preview, replaying }) {
  if (!preview) return null;
  return <section className="rehearsal-application-preview" aria-label="Offline application preview">
    <h3>Offline application preview</h3>
    <p className="implementation-detail">Sample state. You can expand details; application controls are inactive.</p>
    {preview.status === "available" ? <>
      {preview.title && <p className="rehearsal-preview-title">{preview.title}</p>}
      <iframe key={preview.key} title={preview.title ? `Offline application preview: ${preview.title}` : "Offline application preview"}
        sandbox="" referrerPolicy="no-referrer" srcDoc={preview.html} />
      <p className="implementation-detail rehearsal-preview-state">Latest saved sample state, revision {preview.revision}.
        {replaying && " The agent replay may show an earlier event; this preview remains the latest sample state."}
        {" "}This does not verify live UI interaction, authentication or deployment.
      </p>
    </> : <p className="implementation-detail" role="status">{preview.detail}</p>}
  </section>;
}

export function RehearsalContinuation({ receipt, eligible, text, onText, onContinue, onEnd,
  continuing, ending, blocked, selectionBlocked = false, endToRepair = false }) {
  const summary = rehearsalSessionSummary(receipt);
  if (!summary) return null;
  const live = rehearsalActive(receipt), waiting = receipt.status === "waiting_input";
  const savedJourney = !!receipt.trial_replay;
  const expected = receipt.trial_replay?.expected_turns;
  const completed = receipt.turn_count ?? receipt.trial_replay?.completed_turns;
  const progress = savedJourney && Number.isSafeInteger(expected) && expected > 0 &&
    Number.isSafeInteger(completed) && completed >= 0 && completed <= expected;
  return <div className="rehearsal-continuation">
    <p role="status">{savedJourney && live && !["ending", "canceling"].includes(receipt.status)
      ? "Replaying the saved journey with its original requests and sample state." : summary.text}</p>
    {savedJourney && <>
      {progress && <p className="implementation-detail" role="status">Saved journey progress: {completed} of {expected} requests have settled.</p>}
      {live && !["ending", "canceling"].includes(receipt.status) && <p className="implementation-detail">
        {progress && completed === expected
          ? "All saved requests have settled. The host is ending this trial and checking worker cleanup."
          : "The next saved request is sent automatically after the current request settles. You can end the trial at any time."}
      </p>}
    </>}
    {endToRepair && <p className="implementation-warning" role="status">An application callback failed or was unavailable.
      Choose End trial before asking the builder to fix these findings. Once the host confirms cleanup, choose Fix this trial below.</p>}
    {(!savedJourney && summary.idleExpiresAt || summary.expiresAt) && <p className="implementation-detail rehearsal-lifetime">
      {!savedJourney && summary.idleExpiresAt && <>Follow-up window ends <time dateTime={summary.idleExpiresAt}>{new Date(summary.idleExpiresAt).toLocaleTimeString()}</time>. </>}
      {summary.expiresAt && <>Trial time limit <time dateTime={summary.expiresAt}>{new Date(summary.expiresAt).toLocaleTimeString()}</time>.</>}
    </p>}
    {waiting && !savedJourney && <>
      <label htmlFor={`trial-follow-up-${receipt.rehearsal_id}`}>What would you ask next?</label>
      <textarea id={`trial-follow-up-${receipt.rehearsal_id}`} value={text} maxLength={6000}
        disabled={selectionBlocked || ending} onChange={(event) => onText(event.target.value)}
        placeholder="Ask a follow-up using the same conversation and sample data." />
      {!eligible && <p className="implementation-warning">This saved trial cannot accept a follow-up. End it, then start a new trial for the current design and checked source.</p>}
    </>}
    {live && <div className="implementation-actions">
      {waiting && !savedJourney && <button className="setup-action" onClick={onContinue}
        disabled={selectionBlocked || !eligible || !text.trim() || blocked || continuing || ending}>
        {continuing ? "Sending follow-up..." : "Continue this trial"}
      </button>}
      <button className="implementation-link" onClick={onEnd} disabled={selectionBlocked || continuing || ending || receipt.status === "ending" || receipt.status === "canceling"}>
        {ending || receipt.status === "ending" || receipt.status === "canceling" ? "Ending trial..." : "End trial"}
      </button>
    </div>}
  </div>;
}

export function RehearsalPanel({ state, plan, build, busy, pendingManual, navigation, onActivity, onRepairReceipt, onReceipt, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const [receipt, setReceipt] = useState(null), [frames, setFrames] = useState([]);
  const [drafts] = useState(() => createRehearsalDraftStore(rehearsalSessionStorage()));
  const [jobReplays] = useState(() => createTrialReplayStore(rehearsalSessionStorage()));
  const [, refreshDrafts] = useState(0);
  const [startCapability, setStartCapability] = useState({ loading: true, replay: null });
  const [capabilityCheck, setCapabilityCheck] = useState(0);
  const initialAction = drafts.initial(state.id), initialBlocked = drafts.initialBlocked(state.id);
  const initialHistory = drafts.initialHistory(state.id), initialPosting = useRef(new Map());
  const replayAction = jobReplays.pending(state.id), replayBlocked = jobReplays.blocked(state.id);
  const replayPosting = useRef(new Map()), [replaySending, setReplaySending] = useState(false);
  const [mode, setMode] = useState(() => drafts.preferredMode(state, build) || "design");
  const applicationSetup = useRef(null);
  const setupOwner = rehearsalSetupDraftOwner(state, build, mode);
  const turnOwner = rehearsalTurnDraftOwner(receipt, build, state);
  const { sample = "", flow = "", context = "", applicationData = "" } = drafts.read(setupOwner);
  const { followUp = "", improvement = "" } = drafts.read(turnOwner);
  const [error, setError] = useState("");
  const [statusUnavailable, setStatusUnavailable] = useState(false), [timelineUnavailable, setTimelineUnavailable] = useState(false);
  const [sending, setSending] = useState(false);
  const [canceling, setCanceling] = useState(false);
  const [continuing, setContinuing] = useState(false);
  const [repairing, setRepairing] = useState(false), [sampleChoice, setSampleChoice] = useState(null);
  const [events, setEvents] = useState([]);
  const [traceIndex, setTraceIndex] = useState(null), [replaying, setReplaying] = useState(false);
  const selected = useRef(0), current = useRef(state), latestReceipt = useRef(receipt);
  const receiptEpoch = useRef(null), wakePoll = useRef(null);
  const currentBuild = useRef(build), currentFollowUp = useRef(followUp);
  current.current = state;
  currentBuild.current = build; currentFollowUp.current = followUp;
  latestReceipt.current = receipt;
  function acceptReceipt(value) {
    if (!guideSelectionAvailable(selectionScope, current.current)) return;
    const guide = current.current, latestBuild = currentBuild.current;
    const followOwner = rehearsalTurnDraftOwner(value, latestBuild, guide);
    if (drafts.acknowledge(followOwner, value)) {
      currentFollowUp.current = drafts.read(followOwner).followUp;
      refreshDrafts((revision) => revision + 1);
    }
    latestReceipt.current = value; setReceipt(value);
  }
  function chooseMode(value) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (value !== "application") applicationSetup.current = null;
    drafts.selectMode(state, build, value); setMode(value);
  }
  function updateSetup(field, value) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!drafts.update(setupOwner, field, value)) return;
    if (mode !== "application") applicationSetup.current = null;
    drafts.selectMode(state, build, mode);
    refreshDrafts((revision) => revision + 1);
  }
  const setSample = (value) => updateSetup("sample", value);
  const setFlow = (value) => updateSetup("flow", value);
  const setContext = (value) => updateSetup("context", value);
  const setApplicationData = (value) => updateSetup("applicationData", value);
  function updateTurn(field, value) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!drafts.update(turnOwner, field, value)) return false;
    refreshDrafts((revision) => revision + 1);
    return true;
  }
  const setImprovement = (value) => updateTurn("improvement", value);
  function updateFollowUp(value) {
    if (updateTurn("followUp", value)) currentFollowUp.current = value;
  }
  const url = `/api/guide/${encodeURIComponent(state.id)}/rehearsal`;
  const active = rehearsalActive(receipt), valid = rehearsalCurrent(receipt, state);
  const canTryApplication = applicationTrialAvailable(build, state);
  const samples = applicationTrialSamples(build, state);
  const repairBody = applicationTrialRepair(receipt, build, state);
  const feedbackBody = applicationTrialFeedback(receipt, build, state);
  const improvementBody = applicationTrialImprovement(receipt, build, state, improvement);
  const correctionBody = improvement.trim() ? improvementBody : repairBody;
  const canContinue = !receipt?.trial_replay && rehearsalCanContinue(receipt, build, state);
  const openTrial = receipt?.keep_open === true && active;
  const preview = applicationTrialPreview(receipt, build, state);
  const replayOffer = applicationTrialReplayOffer(build, state);
  useEffect(() => {
    let stopped = false;
    setStartCapability({ loading: true, replay: null });
    request("/api/capabilities").then((value) => {
      const replay = value?.rehearsal?.start_request_replay;
      if (!stopped) setStartCapability({ loading: false, replay: typeof replay === "boolean" ? replay : null });
    }, () => { if (!stopped) setStartCapability({ loading: false, replay: null }); });
    return () => { stopped = true; };
  }, [capabilityCheck]);
  useEffect(() => { onReceipt?.(receipt); }, [receipt, onReceipt]);
  useEffect(() => { setSampleChoice(null); }, [build?.source_digest]);
  useEffect(() => {
    let previous = applicationSetup.current;
    const sameInputs = previous && previous.guide.id === state.id &&
      JSON.stringify([previous.guide.proposal || previous.guide.blueprint, previous.guide.brief]) ===
        JSON.stringify([state.proposal || state.blueprint, state.brief]);
    if (previous && (!sameInputs || build && (build.build_id !== previous.build.build_id ||
        build.source_digest !== previous.build.source_digest || build.current === false) ||
        active || initialAction || initialBlocked || initialPosting.current.has(state.id) || replayAction || replayBlocked || replayPosting.current.has(state.id))) {
      applicationSetup.current = null; previous = null;
    }
    const owner = rehearsalSetupDraftOwner(state, build, "application");
    if (!owner || active || initialAction || initialBlocked || initialPosting.current.has(state.id) || replayAction || replayBlocked || replayPosting.current.has(state.id)) return;
    const continuingSetup = previous && rehearsalSetupDraftCanContinue(previous.guide, previous.build, state, build);
    if (continuingSetup) {
      if (drafts.retainSetup(previous.owner, owner)) refreshDrafts((revision) => revision + 1);
      if (!drafts.preferredMode(state, build)) setMode("application");
    }
    if (mode === "application" || continuingSetup) applicationSetup.current = { guide: state, build, owner };
  }, [state.id, state.version, state.proposal, state.blueprint, state.brief, build, mode,
    active, initialAction, initialBlocked, replayAction, replayBlocked]);
  useEffect(() => {
    if (!active) {
      const preferred = drafts.preferredMode(state, build);
      if (preferred) setMode(preferred);
    }
  }, [state.id, state.version, build?.build_id, build?.source_digest]);
  useEffect(() => {
    if (active) setMode(receipt.mode === "application" ? "application" : "design");
    else if (!canTryApplication) setMode("design");
  }, [canTryApplication, active, receipt?.mode]);
  useEffect(() => {
    let stopped = false, timer, inFlight = false, wakeRequested = false;
    const epoch = createRehearsalReceiptEpoch(state.id);
    receiptEpoch.current = epoch;
    latestReceipt.current = null; setReceipt(null);
    setFrames([]); setEvents([]); setSending(false); setCanceling(false); setContinuing(false); setRepairing(false); setReplaySending(false); setSampleChoice(null); setError("");
    setStatusUnavailable(false); setTimelineUnavailable(false);
    setTraceIndex(null); setReplaying(false);
    setMode(drafts.preferredMode(state, build) || "design");
    selected.current += 1;
    const poll = async () => {
      if (stopped || inFlight) return;
      inFlight = true;
      const ownerCurrent = captureGuideSelection(selectionScope, current.current);
      try {
        if (!ownerCurrent) return;
        await epoch.read(() => request(url), () => ownerCurrent() ? current.current : null,
          { onReceipt: (value) => { if (ownerCurrent()) acceptReceipt(value); },
            onUnavailable: (value) => { if (ownerCurrent()) setStatusUnavailable(value); } });
      }
      finally {
        inFlight = false;
        if (!stopped) { timer = setTimeout(poll, wakeRequested ? 0 : 1800); wakeRequested = false; }
      }
    };
    const wake = () => { clearTimeout(timer); if (inFlight) wakeRequested = true; else poll(); };
    wakePoll.current = wake;
    poll();
    return () => {
      stopped = true; epoch.stop(); selected.current += 1; clearTimeout(timer);
      if (wakePoll.current === wake) wakePoll.current = null;
    };
  }, [url]);
  useEffect(() => {
    receiptEpoch.current?.invalidate(); selected.current += 1;
    setSending(false); setCanceling(false); setContinuing(false); setRepairing(false); setReplaySending(false);
    wakePoll.current?.();
  }, [state.id, state.version]);
  useEffect(() => {
    if (busy || pendingManual) return;
    const nextMode = rehearsalNavigationMode(navigation, state, build, receipt);
    if (nextMode) chooseMode(nextMode);
    // This explicit navigation selects only the mode. Receipt polling and
    // later source changes must not replay it or replace the user's drafts.
  }, [navigation]);
  useEffect(() => {
    if (!receipt?.rehearsal_id) return;
    let stopped = false, timer, after;
    const guideId = state.id, trialId = receipt.rehearsal_id;
    const stillSelected = () => !stopped && current.current.id === guideId &&
      latestReceipt.current?.rehearsal_id === trialId;
    const log = createGuideFrameLog();
    const nativeEvents = [];
    setFrames([]); setEvents([]);
    setTimelineUnavailable(false);
    setTraceIndex(null); setReplaying(false);
    const poll = async () => {
      const ownerCurrent = captureGuideSelection(selectionScope, current.current);
      let delay = 1200;
      try {
        if (!ownerCurrent) return;
        const query = new URLSearchParams({ rehearsal_id: trialId, limit: "200" });
        if (after) query.set("after", after);
        const page = await request(`${url}/timeline?${query}`);
        if (!ownerCurrent() || !stillSelected() || page.rehearsal_id !== trialId) return;
        setTimelineUnavailable(false);
        nativeEvents.push(...(page.frames || []));
        setEvents([...nativeEvents]);
        setFrames([...ingestGuideTimeline(log, page)]);
        after = page.next_cursor || after;
        delay = page.next_cursor !== page.latest_cursor ? 0 : 1200;
      } catch { if (ownerCurrent?.() && stillSelected()) setTimelineUnavailable(true); delay = 3000; }
      finally { if (!stopped) timer = setTimeout(poll, delay); }
    };
    poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [url, receipt?.rehearsal_id]);
  const replayFrames = useMemo(() => traceIndex === null ? frames : rehearsalReplay(events, traceIndex), [frames, events, traceIndex]);
  const activity = useMemo(() => rehearsalActivityFromTimeline(receipt,
    traceIndex === null ? events : events.slice(0, traceIndex + 1), state, { replay: traceIndex !== null }),
    [receipt, events, state, traceIndex]);
  useEffect(() => { onActivity?.(activity); }, [activity, onActivity]);
  useEffect(() => {
    if (!replaying || !events.length) return;
    const timer = setTimeout(() => setTraceIndex((index) => {
      const next = (index ?? -1) + 1;
      if (next >= events.length) { setReplaying(false); return events.length - 1; }
      return next;
    }), 500);
    return () => clearTimeout(timer);
  }, [replaying, traceIndex, events.length]);
  const view = useMemo(() => rehearsalConversation(receipt, replayFrames), [receipt, replayFrames]);
  async function run(recover = false) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (current.current.id !== state.id || current.current.version !== state.version ||
        recover !== true && mode === "application" && (!applicationTrialAvailable(currentBuild.current, current.current) ||
          currentBuild.current.build_id !== build?.build_id || currentBuild.current.source_digest !== build?.source_digest)) return;
    const savedAction = drafts.initial(state.id);
    if (initialPosting.current.has(state.id) || replayPosting.current.has(state.id) || replaySending || replayAction || replayBlocked || sending || canceling || continuing || repairing || busy || pendingManual || startCapability.loading || startCapability.replay === null ||
        (recover === true ? !savedAction || !startCapability.replay : !!savedAction || active || !sample.trim() ||
          startCapability.replay && drafts.initialBlocked(state.id))) return;
    const epoch = receiptEpoch.current;
    if (!epoch) return;
    const token = ++selected.current;
    const release = epoch.holdReads();
    const ticket = epoch.capture(state);
    if (!ticket) { release(); return; }
    const posting = { guideId: state.id };
    initialPosting.current.set(posting.guideId, posting);
    setSending(true); setError("");
    const submittedSample = sample, submittedOwner = setupOwner;
    const submittedDraft = drafts.capture(submittedOwner, "sample");
    try {
      let action = savedAction;
      let body = action?.body || rehearsalRunRequest(state, build, { mode, sample, context, applicationData, flow });
      if (!action && startCapability.replay) {
        if (typeof globalThis.crypto?.randomUUID !== "function")
          throw new Error("This browser cannot create a trial recovery key. No trial request was sent; your draft remains editable.");
        body = { ...body, start_request_id: globalThis.crypto.randomUUID() };
        action = drafts.beginInitial(submittedOwner, body);
        if (!action) throw new Error("The trial recovery key could not be saved in this tab. No trial request was sent; your draft remains editable. Free browser storage or allow it before starting a trial.");
        refreshDrafts((revision) => revision + 1);
      }
      const value = await requestRehearsalStart(url, body);
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) {
        if (action && rehearsalInitialRejected(action, value?.start_rejection)) {
          if (!drafts.recordRejectedStart(state.id, value.start_rejection))
            throw new Error("The host rejected this request before admission, but its history could not be saved. Your pending request and drafts are preserved. Allow browser storage, then recover it again.");
          refreshDrafts((revision) => revision + 1);
          throw new Error(`${value.detail} Your draft and original request are preserved. Choose Keep rejected start and correct trial to prepare a corrected request.`);
        }
        if (action && value?.start_request_complete === false &&
            rehearsalInitialResponseMatches(action, value) && value.status === "unavailable") {
          if (!drafts.recordPreparationFailure(state.id, value))
            throw new Error("The earlier preparation did not finish, but its recovery history could not be saved in this tab. Your pending request and drafts are preserved. Allow browser storage, then recover it again.");
          refreshDrafts((revision) => revision + 1);
          throw new Error("This saved trial's preparation did not finish. Recovery did not start another worker. Your exact request remains saved; its admission is not confirmed.");
        }
        if (action ? !rehearsalInitialAcknowledged(action, value)
          : !rehearsalDraftAdmissionMatches(submittedOwner, value) || value.text !== submittedSample.trim())
          throw new Error("The trial admission could not be matched to this draft. Your text is preserved; check the saved trial before retrying.");
        if (action) {
          drafts.acknowledgeInitial(state.id, value); refreshDrafts((revision) => revision + 1);
          if (!drafts.read(action.owner).sample) setSampleChoice(null);
        } else if (drafts.clearSubmitted(submittedOwner, "sample", submittedDraft)) {
          setSampleChoice(null); refreshDrafts((revision) => revision + 1);
        }
        epoch.invalidate(); acceptReceipt(value); setStatusUnavailable(false); setFrames([]);
      }
    } catch (failure) {
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) { epoch.invalidate(); setError(failure.message); }
    }
    finally {
      if (initialPosting.current.get(posting.guideId) === posting) initialPosting.current.delete(posting.guideId); refreshDrafts((revision) => revision + 1); release();
      if (token === selected.current) { setSending(false); wakePoll.current?.(); }
    }
  }
  async function trySameJob(recover = false) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    const saved = jobReplays.pending(state.id);
    if (replayPosting.current.has(state.id) || initialPosting.current.has(state.id) || sending || canceling || continuing || repairing || busy || pendingManual ||
        initialAction || initialBlocked || (recover ? !saved : saved || replayBlocked || active || !replayOffer)) return;
    const epoch = receiptEpoch.current;
    if (!epoch) return;
    const token = ++selected.current, guideId = state.id;
    const release = epoch.holdReads(), ticket = epoch.capture(state);
    if (!ticket) { release(); return; }
    const posting = { guideId: state.id };
    replayPosting.current.set(posting.guideId, posting); setReplaySending(true); setError("");
    try {
      let action = saved;
      if (!action) {
        if (typeof globalThis.crypto?.randomUUID !== "function")
          throw new Error("This browser cannot create a saved retry key. No retry was sent.");
        action = jobReplays.begin(replayOffer, globalThis.crypto.randomUUID());
        if (!action) throw new Error("The retry could not be saved in this tab. No retry was sent. Allow browser storage before trying again.");
        refreshDrafts((revision) => revision + 1);
      }
      const replayUrl = `/api/guide/${encodeURIComponent(action.guide_id)}/rehearsal/${encodeURIComponent(action.rehearsal_id)}/replay`;
      const value = await requestTrialReplay(replayUrl, action.body);
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) {
        if (trialReplayRejected(action, value?.start_rejection)) {
          if (!jobReplays.recordRejected(guideId, value.start_rejection))
            throw new Error("The rejected retry's history could not be saved. Its exact pending action remains here; allow browser storage and recover it again.");
          refreshDrafts((revision) => revision + 1);
          throw new Error(`${value.detail} Keep the rejected retry before preparing another.`);
        }
        if (!trialReplayAcknowledged(action, value))
          throw new Error("The retry response did not match its saved job and application. Recover the saved retry before starting another trial.");
        if (!jobReplays.acknowledge(guideId, value))
          throw new Error("The retry was admitted, but this tab could not save its confirmation. Recover the same saved retry; this will not start a duplicate trial.");
        refreshDrafts((revision) => revision + 1);
        epoch.invalidate(); acceptReceipt(value); setStatusUnavailable(false); setFrames([]);
        setTraceIndex(null); setReplaying(false);
      }
    } catch (failure) {
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) { epoch.invalidate(); setError(failure.message); }
    } finally {
      if (replayPosting.current.get(posting.guideId) === posting) replayPosting.current.delete(posting.guideId); refreshDrafts((revision) => revision + 1); release();
      if (token === selected.current) { setReplaySending(false); wakePoll.current?.(); }
    }
  }
  function keepRejectedReplay(actionId) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (replayPosting.current.has(state.id) || initialPosting.current.has(state.id) || sending || canceling || continuing || repairing || busy || pendingManual ||
        !jobReplays.keepRejected(state.id, actionId)) {
      setError("The rejected retry's history could not be preserved. Recover its saved action before preparing another.");
      return;
    }
    setError(""); refreshDrafts((revision) => revision + 1);
  }
  function separateInitial(scope) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!scope || current.current.id !== scope.guideId || current.current.version !== scope.version ||
        selected.current !== scope.token || initialPosting.current.has(state.id) || sending || canceling || continuing || repairing ||
        busy || pendingManual || startCapability.loading || !startCapability.replay) return;
    if (!drafts.archivePreparationFailure(scope.guideId, scope.actionId)) {
      setError("The earlier start's history could not be saved in this tab. Its pending request and your drafts remain here. Allow browser storage before preparing a separate trial.");
      return;
    }
    setError(""); refreshDrafts((revision) => revision + 1);
  }
  async function cancel() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (replayPosting.current.has(state.id) || replaySending || sending || canceling || continuing || repairing || !active || !receipt?.rehearsal_id) return;
    const epoch = receiptEpoch.current;
    if (!epoch) return;
    const trialId = receipt.rehearsal_id, token = ++selected.current;
    const release = epoch.holdReads();
    const ticket = epoch.capture(state);
    if (!ticket) { release(); return; }
    setCanceling(true); setError("");
    try {
      const value = await request(`${url}/${receipt.keep_open === true ? "end" : "cancel"}`, { method: "POST", body: { rehearsal_id: trialId } });
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) {
        epoch.invalidate();
        if (latestReceipt.current?.rehearsal_id === trialId && value?.rehearsal_id === trialId && value.guide_id === state.id) {
          acceptReceipt(value); setStatusUnavailable(false);
        }
      }
    } catch (failure) {
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) { epoch.invalidate(); setError(failure.message); }
    } finally { release(); if (token === selected.current) { setCanceling(false); wakePoll.current?.(); } }
  }
  async function continueTrial() {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (replayPosting.current.has(state.id) || replaySending || replayAction || replayBlocked || sending || canceling || continuing || repairing || busy || pendingManual || !canContinue) return;
    const token = ++selected.current;
    setContinuing(true); setError("");
    const submittedDraft = drafts.capture(turnOwner, "followUp");
    drafts.markSubmitted(turnOwner, { kind: "continue", text: followUp, turn: receipt.turn_count + 1 });
    await admitRehearsalFollowUp({ receipt, build, state, text: followUp, epoch: receiptEpoch.current,
      load: (body) => request(`${url}/continue`, { method: "POST", body }),
      current: () => ({ state: ownerCurrent() ? current.current : null, receipt: latestReceipt.current,
        build: currentBuild.current, text: currentFollowUp.current }),
      onReceipt: (value) => { if (!ownerCurrent()) return; acceptReceipt(value); setStatusUnavailable(false); setTraceIndex(null); setReplaying(false); },
      onClear: () => {
        if (!ownerCurrent()) return;
        if (drafts.clearSubmitted(turnOwner, "followUp", submittedDraft)) {
          currentFollowUp.current = ""; refreshDrafts((revision) => revision + 1);
        }
      }, onError: (failure) => { if (ownerCurrent()) setError(failure.message); } });
    if (token === selected.current) { setContinuing(false); wakePoll.current?.(); }
  }
  async function repair(body) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return;
    if (!body || !onRepairReceipt || replayPosting.current.has(state.id) || replaySending || replayAction || replayBlocked || sending || canceling || continuing || repairing || active || busy || pendingManual) return;
    const epoch = receiptEpoch.current;
    if (!epoch) return;
    const token = ++selected.current;
    const release = epoch.holdReads();
    const ticket = epoch.capture(state);
    if (!ticket) { release(); return; }
    setRepairing(true); setError("");
    const submittedOwner = turnOwner, submittedDraft = drafts.capture(turnOwner, "improvement");
    try {
      const value = await request(`${url}/repair`, { method: "POST", body });
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) {
        epoch.invalidate(); onRepairReceipt(value);
        if (body.request && value?.guide_id === state.id && typeof value.build_id === "string" &&
            drafts.clearSubmitted(submittedOwner, "improvement", submittedDraft))
          refreshDrafts((revision) => revision + 1);
      }
    } catch (failure) {
      if (ownerCurrent() && token === selected.current && epoch.current(ticket, current.current)) { epoch.invalidate(); setError(failure.message); }
    }
    finally { release(); if (token === selected.current) { setRepairing(false); wakePoll.current?.(); } }
  }
  const recoveryScope = { guideId: state.id, version: state.version, token: selected.current,
    actionId: initialAction?.body.start_request_id };
  return <section className="rehearsal-panel" aria-label="Try your mob" aria-disabled={selectionBlocked}
    {...guideSelectionEvents(selectionScope, state, selectionToken)}>
    <div className="implementation-kicker"><i /> Try your mob</div>
    <h2>Give your mob a first job</h2>
    <RehearsalInitialRecovery action={initialAction} blocked={initialBlocked} supported={startCapability.replay}
      loading={startCapability.loading} disabled={selectionBlocked || initialPosting.current.has(state.id) || replayPosting.current.has(state.id) || replaySending || !!replayAction || replayBlocked || sending || canceling || continuing || repairing || busy || pendingManual}
      onRecover={() => run(true)} onSeparate={() => separateInitial(recoveryScope)}
      onRetry={() => setCapabilityCheck((revision) => revision + 1)} />
    <RehearsalPreparationHistory entries={initialHistory} />
    <RehearsalSameJobReplay offer={replayOffer} receipt={receipt} action={replayAction} blocked={replayBlocked}
      sending={replaySending} disabled={selectionBlocked || initialPosting.current.has(state.id) || sending || canceling || continuing || repairing || busy || pendingManual ||
        !!initialAction || initialBlocked || !replayAction && active}
      onTry={() => trySameJob()} onRecover={() => trySameJob(true)}
      onSeparate={() => keepRejectedReplay(replayAction?.body.start_request_id)} />
    {!openTrial && <>
    <fieldset className="rehearsal-choice"><legend>Try with</legend>
      <label><input type="radio" name={`trial-mode-${state.id}`} checked={mode === "design"} onChange={() => chooseMode("design")} disabled={selectionBlocked || active || sending || canceling || repairing} /> Agent design</label>
      <label><input type="radio" name={`trial-mode-${state.id}`} checked={mode === "application"} onChange={() => chooseMode("application")} disabled={selectionBlocked || active || sending || canceling || repairing || !canTryApplication} /> Built application</label>
    </fieldset>
    {!canTryApplication && <p className="implementation-detail">You can try the application after the builder produces checked source.</p>}
    {mode === "application" && <div className="rehearsal-samples">
      {samples.length > 0 ? <RehearsalSampleChoices samples={samples} selected={sampleChoice}
        disabled={selectionBlocked || active || sending || canceling || repairing || busy || pendingManual}
        onSelect={(item, index) => { setSampleChoice(index); setSample(item.request); setApplicationData(JSON.stringify(item.fixtures, null, 2)); setFlow(""); }} />
        : <p>{build?.trial_samples?.detail || "This application has no prepared sample jobs. Enter your own request, or ask the builder to prepare useful samples."}</p>}
    </div>}
    <label htmlFor={`trial-${state.id}`}>Your request</label>
    <textarea disabled={selectionBlocked} id={`trial-${state.id}`} value={sample} maxLength={6000} onChange={(event) => { setSampleChoice(null); setSample(event.target.value); }}
      placeholder="Describe a realistic request, with enough context for your agents to work on it." />
    <p className="implementation-detail rehearsal-scope">{mode === "application"
      ? "Runs your application's code offline with isolated sample data. Live integrations remain unavailable. This does not deploy or test your installed application."
      : "Explore how the proposed agents collaborate in a separate trial. External services and application callbacks are unavailable."}</p>
    <div className="implementation-actions">
      <button className="setup-action" onClick={() => run()} disabled={selectionBlocked || initialPosting.current.has(state.id) || replayPosting.current.has(state.id) || replaySending || !!replayAction || replayBlocked || sending || canceling || repairing || active || busy || pendingManual ||
        startCapability.loading || startCapability.replay === null || !!initialAction || startCapability.replay && initialBlocked || mode === "application" && !canTryApplication || !sample.trim()}>
        {sending ? "Starting rehearsal..." : active ? "Rehearsal in progress" : mode === "application" ? "Try application request" : "Try agent design"}
      </button>
      {active && <button className="implementation-link" onClick={cancel} disabled={selectionBlocked || sending || canceling || continuing || repairing}>
        {canceling ? "Stopping rehearsal..." : "Stop rehearsal"}</button>}
    </div>
    <p className="implementation-detail rehearsal-follow-up-note">{flow ? `Workflow: ${plan?.flows?.find(item => item.id === flow)?.name || flow}. Runs once; its trace stays saved.`
      : "Stays open for follow-up questions with the same sample state."}</p>
    <details className="rehearsal-options"><summary>Sample data and options</summary>
      <label htmlFor={`trial-context-${state.id}`}>Sample context</label>
      <p>Paste a document or example input. Agents with a workspace grant can read this in-trial sample file. It does not connect a live service.</p>
      <textarea disabled={selectionBlocked} id={`trial-context-${state.id}`} aria-label="Sample context for rehearsal" value={context} maxLength={180000}
        onChange={(event) => setContext(event.target.value)} placeholder="Paste a sample document, event or project context..." />
      {mode === "application" && <>
        <label htmlFor={`trial-data-${state.id}`}>Application sample data</label>
        <p>A sample job fills in its prepared data. You can adjust this JSON object for another example. Empty data is useful only when the application's offline hook supports it.</p>
        <textarea disabled={selectionBlocked} id={`trial-data-${state.id}`} aria-label="Application sample data" value={applicationData} maxLength={100000}
          onChange={(event) => { setSampleChoice(null); setApplicationData(event.target.value); }} placeholder="{}" spellCheck="false" />
      </>}
      {plan?.flows?.length > 0 && <label className="rehearsal-mode">How should this request start?
        <select disabled={selectionBlocked} value={flow} onChange={(event) => setFlow(event.target.value)}>
          <option value="">Conversation with your main agent</option>
          {plan.flows.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
        </select>
      </label>}
    </details>
    </>}
    <RehearsalContinuation receipt={receipt} eligible={canContinue} text={followUp} onText={updateFollowUp}
      onContinue={continueTrial} onEnd={cancel} continuing={continuing} ending={canceling} selectionBlocked={selectionBlocked}
      blocked={selectionBlocked || replaySending || !!replayAction || replayBlocked || sending || repairing || busy || pendingManual}
      endToRepair={!!onRepairReceipt && applicationTrialEndToRepair(receipt, build, state)} />
    {receipt && !valid && <p className="implementation-warning">This trial belongs to an earlier proposal or application source. Its history remains available.
      {replayOffer ? " Choose Try the same job again above to test the repaired application with its saved inputs."
        : " Run a new trial for the current design and build."}</p>}
    <RehearsalTrialSummary receipt={receipt} canRepair={!!repairBody && !!onRepairReceipt} />
    {receipt?.application_trial && <p className="implementation-detail">{receipt.application_trial.detail || receipt.application_trial.scope}
      {receipt.application_trial.available === false && " Application callbacks were unavailable in this trial."}</p>}
    {feedbackBody && onRepairReceipt && <div className="rehearsal-repair">
      <label htmlFor={`trial-improvement-${state.id}`}>What should work better after this trial?</label>
      <p>{repairBody
        ? "The builder can investigate this saved trial problem, repair the application and run fresh independent checks. Add what you expected to happen, or leave this blank to investigate the recorded problem."
        : "A completed agent turn may still leave your job unfinished. Describe what you wanted to happen; the builder will review the actual saved callback results with your feedback."}</p>
      <textarea id={`trial-improvement-${state.id}`} aria-label="What should work better after this trial?" value={improvement}
        maxLength={6000} disabled={selectionBlocked || repairing} onChange={(event) => setImprovement(event.target.value)}
        placeholder="Describe the missing behavior or a change you want to try." />
      <button className="setup-action" onClick={() => repair(correctionBody)}
        disabled={selectionBlocked || !correctionBody || replaySending || !!replayAction || replayBlocked || repairing || sending || canceling || continuing || active || busy || pendingManual}>
        {repairing ? "Sending trial to builder..." : repairBody ? "Fix this trial" : "Improve from this trial"}
      </button>
    </div>}
    {receipt?.application_observations?.length > 0 && <details><summary>Application callback observations</summary>
      <pre>{JSON.stringify(receipt.application_observations, null, 2)}</pre>
    </details>}
    {(preview || frames.length > 0) && <div className="rehearsal-experience" data-has-preview={!!preview && frames.length > 0}>
    <RehearsalApplicationPreview preview={preview} replaying={traceIndex !== null} />
    {frames.length > 0 && <details className="implementation-activity" open><summary>Actual agent activity</summary>
      <div className="rehearsal-replay">
        <button className="implementation-link" onClick={() => { setTraceIndex(0); setReplaying(true); }}>Replay trace</button>
        {replaying && <button className="implementation-link" onClick={() => setReplaying(false)}>Pause replay</button>}
        {traceIndex !== null && <button className="implementation-link" onClick={() => { setTraceIndex(null); setReplaying(false); }}>Latest activity</button>}
        <label>Trace event {traceIndex === null ? events.length : traceIndex + 1} of {events.length}
          <input type="range" aria-label="Native trace position" min="0" max={Math.max(0, events.length - 1)}
            value={traceIndex ?? events.length - 1} onChange={(event) => { setTraceIndex(Number(event.target.value)); setReplaying(false); }} />
        </label>
      </div>
      <div className="setup-native-chat rehearsal-transcript"><SpriteSheet />
        <ConversationTranscript viewState={view} Icon={ConsoleIcon} compact showTurnDiff={false} />
      </div>
    </details>}
    </div>}
    {receipt?.masked_capabilities && <details><summary>Trial limitations</summary><pre>{JSON.stringify(receipt.masked_capabilities, null, 2)}</pre></details>}
    {(statusUnavailable || timelineUnavailable) && <p className="implementation-warning" role="status">Reconnecting to Forge. Saved trial results remain here; activity will resume when the connection returns.</p>}
    {error && <p className="implementation-error" role="alert">{error}</p>}
  </section>;
}
