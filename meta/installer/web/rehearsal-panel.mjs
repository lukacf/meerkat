import { artifactIsCurrent } from "./artifact-current.mjs";
import { buildConversationViewState, mapFramesToTimelineEntries, sortConversationTimelineEntries } from "../../packages/console-core/src/index.ts";
import { createGuideFrameLog, ingestGuideTimeline } from "./native-guide-chat.mjs";

export const rehearsalActive = (receipt) => ["queued", "starting", "running", "waiting_input", "ending", "canceling"].includes(receipt?.status);
export const rehearsalCurrent = artifactIsCurrent;
const checkedApplicationSource = (build, state) => artifactIsCurrent(build, state) && build.source_verified === true &&
  typeof build.build_id === "string" && !!build.build_id && typeof build.source_digest === "string" && !!build.source_digest &&
  ["completed", "failed", "interrupted"].includes(build.status);
export const applicationTrialAvailable = (build, state) => checkedApplicationSource(build, state) && build.cleanup_pending !== true;
const finiteJson = (item) => typeof item === "number" ? Number.isFinite(item) : !item || typeof item !== "object" || Object.values(item).every(finiteJson);

/** Local request ordering only. Host receipts still own status and currentness. */
export function createRehearsalReceiptEpoch(guideId) {
  let epoch = 0, stopped = false;
  const admissions = new Set();
  const capture = (guide) => !stopped && guide?.id === guideId
    ? { epoch, guide_id: guideId, guide_version: guide.version } : null;
  const current = (ticket, guide) => !!ticket && !stopped && ticket.epoch === epoch &&
    ticket.guide_id === guide?.id && ticket.guide_version === guide.version;
  return {
    capture, current,
    invalidate() { epoch += 1; },
    stop() { stopped = true; epoch += 1; },
    holdReads() {
      const admission = {};
      epoch += 1; admissions.add(admission);
      return () => admissions.delete(admission);
    },
    async read(load, guide, { onReceipt, onUnavailable }) {
      if (admissions.size) return false;
      const ticket = capture(guide());
      if (!ticket) return false;
      try {
        const value = await load();
        if (!current(ticket, guide())) return false;
        onReceipt(value);
        onUnavailable(false);
      } catch (failure) {
        if (!current(ticket, guide())) return false;
        onUnavailable(failure.status !== 404);
      }
      return true;
    },
  };
}

export function applicationTrialMatches(receipt, build, state) {
  return receipt?.mode === "application" && rehearsalCurrent(receipt, state) && checkedApplicationSource(build, state) &&
    receipt.application_source?.build_id === build.build_id && receipt.application_source?.source_digest === build.source_digest;
}

const PREVIEW_SCOPE = "offline-application-preview", PREVIEW_SANITIZATION = "inert-html-v1";
const MAX_PREVIEW_DOCUMENT_BYTES = 100_000;
const previewText = (value, limit) => typeof value === "string" && [...value].length <= limit &&
  !/[\u0000-\u001f\u007f]/.test(value);

/** A host-sanitized view of the exact current trial state, never a UI transport. */
export function applicationTrialPreview(receipt, build, state) {
  if (!applicationTrialMatches(receipt, build, state)) return null;
  const preview = receipt.application_preview, revision = receipt.application_observation_revision;
  if (!preview || typeof preview !== "object" || Array.isArray(preview) ||
    !["available", "unsupported", "failed"].includes(preview.status) ||
    preview.scope !== PREVIEW_SCOPE || preview.sanitization !== PREVIEW_SANITIZATION || preview.live_ready !== false ||
    !previewText(receipt.rehearsal_id, 180) || !receipt.rehearsal_id || preview.trial_id !== receipt.rehearsal_id ||
    preview.build_id !== build.build_id || preview.source_digest !== build.source_digest ||
    !/^[a-f0-9]{64}$/.test(preview.source_digest) ||
    !Number.isSafeInteger(preview.guide_version) || preview.guide_version !== receipt.guide_version ||
    !Number.isSafeInteger(revision) || revision < 0 || preview.observation_revision !== revision ||
    preview.title != null && !previewText(preview.title, 120)) return null;
  const key = JSON.stringify([receipt.guide_id, receipt.guide_version, preview.trial_id,
    preview.build_id, preview.source_digest, revision, preview.status]);
  if (preview.status !== "available") {
    if (preview.html != null || preview.document_sha256 != null) return null;
    return { key, status: preview.status, title: preview.title || null, html: null,
      detail: previewText(preview.detail, 600) && preview.detail.trim() ? preview.detail : preview.status === "unsupported"
        ? "This application does not provide an offline preview. The trial's outcome is shown separately."
        : "The offline preview could not be rendered. The trial's outcome is shown separately." };
  }
  if (!previewText(preview.title, 120) || !preview.title.trim() ||
    typeof preview.html !== "string" || !preview.html.trim() || preview.html.includes("\u0000") ||
    new TextEncoder().encode(preview.html).byteLength > MAX_PREVIEW_DOCUMENT_BYTES ||
    typeof preview.document_sha256 !== "string" || !/^[a-f0-9]{64}$/.test(preview.document_sha256)) return null;
  return { key: `${key}:${preview.document_sha256}`, status: "available", title: preview.title || null,
    html: preview.html, revision, documentSha256: preview.document_sha256 };
}

/** A navigation request selects a trial kind, never submits or replaces drafts. */
export function rehearsalNavigationIntent(mode, state, build) {
  if (!state?.id || !Number.isSafeInteger(state.version) || !["design", "application"].includes(mode) ||
    mode === "application" && !applicationTrialAvailable(build, state)) return null;
  return { guide_id: state.id, guide_version: state.version, mode,
    ...(mode === "application" ? { build_id: build.build_id, source_digest: build.source_digest } : {}) };
}

export function rehearsalNavigationMode(intent, state, build, receipt) {
  if (!intent || intent.guide_id !== state?.id || !Number.isSafeInteger(intent.guide_version) ||
    intent.guide_version !== state.version || rehearsalActive(receipt)) return null;
  if (intent.mode === "design") return "design";
  return intent.mode === "application" && applicationTrialAvailable(build, state) &&
    intent.build_id === build.build_id && intent.source_digest === build.source_digest ? "application" : null;
}

export function rehearsalRunRequest(state, build, { mode, sample, context = "", applicationData = "", flow = "" }) {
  if (!state?.id || !Number.isSafeInteger(state.version) || !["design", "application"].includes(mode))
    throw new Error("Choose a trial for the current saved proposal.");
  if (typeof sample !== "string" || !sample.trim()) throw new Error("Describe a sample job to try.");
  if ([...sample].length > 6000) throw new Error("Use a sample message of at most 6000 characters.");
  if (typeof context !== "string" || [...context].some((char) => char.length === 1 && char.charCodeAt(0) >= 0xd800 && char.charCodeAt(0) <= 0xdfff))
    throw new Error("Use valid UTF8 text for the sample context. Your draft is preserved.");
  if (context.trim() && new TextEncoder().encode(context).byteLength > 200_000)
    throw new Error("Sample context exceeds the 200 KB UTF8 limit. Shorten it before trying; your draft is preserved.");
  const text = sample.trim();
  // validate_trial uses Python json.dumps(params, ensure_ascii=True), including
  // its space after the colon and two Unicode escapes for astral characters.
  const flowJson = '{"request": ' + JSON.stringify(text).replace(/[\u007f-\uffff]/g,
    (char) => "\\u" + char.charCodeAt(0).toString(16).padStart(4, "0")) + '}';
  if (flow && flowJson.length > 30_000)
    throw new Error("This flow's sample exceeds the 30 KB parameter limit. Shorten it or choose a conversation trial; your draft is preserved.");
  if (mode === "application" && build?.cleanup_pending === true)
    throw new Error("Wait for the build and its runtime cleanup to finish before starting a new application trial. Your draft is preserved.");
  if (mode === "application" && !applicationTrialAvailable(build, state))
    throw new Error("The application build changed. Choose its current checked source before trying it.");
  return { version: state.version, text, mode,
    keep_open: !flow,
    ...(mode === "application" ? { build_id: build.build_id, application_fixtures: applicationTrialFixtures(applicationData) } : {}),
    ...(context.trim() ? { fixtures: { files: { "sample-context.txt": context }, notes: [] } } : {}),
    ...(flow ? { flow_id: flow, params: { request: text } } : {}) };
}

/** Preserve ordinary error ambiguity; only a typed direct start response may be inspected. */
export async function requestRehearsalStart(url, body) {
  const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  let value;
  try { value = await response.json(); } catch {
    throw new Error(`Trial response could not be read (${response.status}). Recover the saved request before starting another.`);
  }
  if (response.ok) return value;
  if ([404, 409, 422, 503].includes(response.status) && value?.start_rejection?.witness_version === 1)
    return { start_rejection: value.start_rejection, detail: typeof value.detail === "string" ? value.detail : "The host rejected this trial before admission." };
  throw new Error(typeof value?.detail === "string" ? value.detail : `Trial request failed (${response.status}).`);
}

export function applicationTrialSamples(build, state) {
  if (!applicationTrialAvailable(build, state) || build.trial_samples?.source_digest !== build.source_digest) return [];
  const cases = build.trial_samples?.cases;
  if (!Array.isArray(cases) || cases.length > 12) return [];
  try {
    return cases.map((item) => {
      if (!item || typeof item.label !== "string" || !item.label.trim() || item.label.length > 160 ||
        /[\u0000-\u001f]/.test(item.label) || typeof item.request !== "string" || !item.request.trim() || item.request.length > 6000)
        throw new Error("Invalid application sample choice.");
      const fixtures = item.fixtures === undefined ? {} : item.fixtures;
      if (!finiteJson(fixtures)) throw new Error("Invalid application sample values.");
      return { label: item.label, request: item.request, fixtures: applicationTrialFixtures(JSON.stringify(fixtures)) };
    });
  } catch { return []; }
}

/** A card preview only. The selected job and its fixtures remain unchanged. */
export function rehearsalSamplePreview(request) {
  if (typeof request !== "string") return "";
  const characters = [...request.replace(/\s+/g, " ").trim()];
  return characters.length > 180 ? `${characters.slice(0, 177).join("")}...` : characters.join("");
}

export function applicationTrialFeedback(receipt, build, state) {
  if (!applicationTrialMatches(receipt, build, state) ||
    receipt.application_stop_confirmed === false ||
    receipt.keep_open === true && receipt.application_stop_confirmed !== true ||
    ((Object.hasOwn(receipt, "worker_stop_confirmed") || receipt.keep_open === true) && receipt.worker_stop_confirmed !== true) ||
    !["completed", "failed", "interrupted", "unavailable"].includes(receipt.status)) return null;
  return { version: state.version, rehearsal_id: receipt.rehearsal_id,
    build_id: build.build_id, source_digest: build.source_digest };
}

export function applicationTrialRepair(receipt, build, state) {
  const problem = ["failed", "interrupted", "unavailable"].includes(receipt?.status) || receipt?.application_trial?.available === false ||
    receipt?.application_observations?.some((item) => item?.kind === "callback_failed") ||
    applicationTrialUnavailableCallbacks(receipt).length > 0;
  return problem ? applicationTrialFeedback(receipt, build, state) : null;
}

/** Guidance only: ending and confirmed cleanup still precede repair admission. */
export function applicationTrialEndToRepair(receipt, build, state) {
  return applicationTrialMatches(receipt, build, state) && receipt.keep_open === true &&
    receipt.status === "waiting_input" && (receipt.application_trial?.available === false ||
      receipt.application_observations?.some((item) => item?.kind === "callback_failed") ||
      applicationTrialUnavailableCallbacks(receipt).length > 0);
}

/** Missing production callbacks declared by the host, never returned prose. */
function applicationTrialUnavailableCallbacks(receipt) {
  const dependencies = receipt?.application_trial?.dependencies, unavailable = new Set();
  if (!Array.isArray(dependencies)) return [];
  for (const item of dependencies) {
    if (typeof item !== "string") continue;
    const marker = ": unavailable callbacks ", index = item.indexOf(marker);
    if (index < 1 || index > 180 || /[\u0000-\u001f]/.test(item.slice(0, index))) continue;
    for (const name of item.slice(index + marker.length).split(", ")) {
      if (/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(name)) unavailable.add(`${item.slice(0, index)}:${name}`);
      if (unavailable.size > 1000) return [...unavailable];
    }
  }
  return [...unavailable];
}

/** Project host-recorded callback problems separately from native completion. */
export function applicationTrialSummary(receipt) {
  if (receipt?.mode !== "application") return null;
  const observations = Array.isArray(receipt.application_observations) ? receipt.application_observations : [];
  let failedCount = 0, returned = false;
  for (const item of observations) {
    if (item?.kind === "callback_failed") failedCount = Math.min(1001, failedCount + 1);
    if (item?.kind === "callback_returned") returned = true;
  }
  const unavailableCount = applicationTrialUnavailableCallbacks(receipt).length;
  const count = (value, singular, plural) => `${value > 1000 ? "More than 1000" : value} ${value === 1 ? singular : plural}`;
  const problems = [];
  if (failedCount) problems.push(`${count(failedCount, "application callback failed", "application callbacks failed")}`);
  if (unavailableCount) problems.push(`${count(unavailableCount, "application callback was unavailable", "application callbacks were unavailable")}`);
  else if (receipt.application_trial?.available === false) problems.push("Application callbacks were unavailable");
  const native = { completed: "The native trial finished.", unavailable: "The application trial could not start.",
    failed: "The trial stopped with an error.", interrupted: "The trial was interrupted.",
    queued: "The trial is queued.", starting: "The native trial is starting.", running: "The native trial is running.",
    waiting_input: "This turn has settled. The isolated trial is waiting for your follow-up.",
    ending: "The trial is ending and its workers are stopping.", canceling: "The trial is stopping." }[receipt.status] || "Saved application trial.";
  const detail = problems.length ? problems.join("; ") + "."
    : receipt.status === "completed" ? returned
      ? "Recorded callback results are offline observations, not proof that your job is complete."
      : "No application callback results were recorded." : "";
  return { text: [native, detail].filter(Boolean).join(" "), failedCount, unavailableCount,
    problem: !!problems.length || ["failed", "interrupted", "unavailable"].includes(receipt.status) };
}

/** Exact current source and a host-owned waiting session, not terminal success. */
export function rehearsalCanContinue(receipt, build, state) {
  return receipt?.keep_open === true && !receipt.flow_id && receipt.status === "waiting_input" &&
    receipt.can_continue === true && receipt.worker_stop_confirmed !== true &&
    typeof receipt.rehearsal_id === "string" && !!receipt.rehearsal_id &&
    Number.isSafeInteger(receipt.turn_count) && receipt.turn_count > 0 &&
    rehearsalCurrent(receipt, state) && (receipt.mode === "design" || applicationTrialMatches(receipt, build, state));
}

export function rehearsalContinueRequest(receipt, build, state, text) {
  if (!rehearsalCanContinue(receipt, build, state) || typeof text !== "string" || !text.trim() || text.length > 6000) return null;
  return { version: state.version, rehearsal_id: receipt.rehearsal_id, expected_turn: receipt.turn_count, text: text.trim() };
}

function rehearsalTurnScope(receipt, build, state) {
  if (!receipt?.rehearsal_id || !rehearsalCurrent(receipt, state) ||
    receipt.mode !== "design" && !applicationTrialMatches(receipt, build, state)) return null;
  return JSON.stringify([state.id, state.version, receipt.rehearsal_id, receipt.guide_version, receipt.mode,
    receipt.application_source?.build_id, receipt.application_source?.source_digest]);
}

/** A response must acknowledge this exact host command before clearing its draft. */
export async function admitRehearsalFollowUp({ receipt, build, state, text, epoch, load, current,
  onReceipt, onClear, onError }) {
  const body = rehearsalContinueRequest(receipt, build, state, text);
  if (!body || !epoch) return false;
  const release = epoch.holdReads();
  const ticket = epoch.capture(state), scope = rehearsalTurnScope(receipt, build, state);
  const stillCurrent = () => {
    const now = current();
    return epoch.current(ticket, now.state) && rehearsalTurnScope(now.receipt, now.build, now.state) === scope;
  };
  if (!ticket || !scope || !stillCurrent()) { release(); return false; }
  try {
    const value = await load(body);
    if (!stillCurrent()) return false;
    const now = current(), command = value?.turn_command;
    if (rehearsalTurnScope(value, now.build, now.state) !== scope || value.keep_open !== true ||
      typeof command?.id !== "string" || !command.id || command.turn !== body.expected_turn + 1 ||
      !["sent", "acknowledged"].includes(command.status))
      throw new Error("The follow-up was not acknowledged for this trial. Reload its saved status before trying again.");
    epoch.invalidate(); onReceipt(value);
    if (current().text === text) onClear();
    return true;
  } catch (failure) {
    if (!stillCurrent()) return false;
    epoch.invalidate(); onError(failure);
    return false;
  } finally { release(); }
}

export function rehearsalSessionSummary(receipt) {
  if (receipt?.keep_open !== true) return null;
  const terminal = ["completed", "failed", "interrupted", "unavailable"].includes(receipt.status);
  const ended = { user_ended: "You ended this trial.", idle_timeout: "This trial ended after waiting for a follow-up.",
    lifetime_limit: "This trial reached its time limit.", turn_limit: "This trial reached its turn limit.",
    owner_eof: "This trial ended when its owner disconnected." }[receipt.ended_reason];
  const cleanup = receipt.worker_stop_confirmed !== true ? "Worker cleanup has not been confirmed yet."
    : receipt.mode === "application" && receipt.application_stop_confirmed !== true
      ? "The native worker has stopped. Application cleanup has not been confirmed."
      : "Its workers have stopped. The actual trace remains available.";
  const text = terminal ? `${ended || "This trial has ended."} ${cleanup}`
    : receipt.status === "waiting_input" ? "Continue in this same isolated conversation and sample state, or end the trial."
    : ["ending", "canceling"].includes(receipt.status) ? "Ending this trial and stopping its workers."
    : "Your agents are working in this same isolated trial.";
  const timestamp = (value) => typeof value === "string" && Number.isFinite(Date.parse(value)) ? value : null;
  return { text, idleExpiresAt: receipt.status === "waiting_input" ? timestamp(receipt.idle_expires_at) : null,
    expiresAt: !terminal ? timestamp(receipt.expires_at) : null, terminal };
}

export function applicationTrialImprovement(receipt, build, state, request) {
  const body = applicationTrialFeedback(receipt, build, state);
  return body && typeof request === "string" && request.trim() && request.length <= 6000
    ? { ...body, request: request.trim() } : null;
}

export function applicationTrialFixtures(text) {
  if (!text.trim()) return {};
  const value = JSON.parse(text);
  if (!value || typeof value !== "object" || Array.isArray(value) || new TextEncoder().encode(JSON.stringify(value)).length > 100000)
    throw new Error("Use an ordinary JSON object of at most 100 KB for application sample data.");
  if (!finiteJson(value)) throw new Error("Application sample data must contain finite JSON values.");
  return value;
}

export function rehearsalReplay(events, index) {
  return ingestGuideTimeline(createGuideFrameLog(), { frames: events.slice(0, index + 1) });
}

export function rehearsalConversation(receipt, frames) {
  const agents = new Map((receipt?.identities || []).map((item) => [typeof item === "string" ? item : item.identity,
    typeof item === "string" ? { identity: item, name: item } : item]));
  const groups = new Map();
  for (const frame of frames) {
    const identity = frame.identity || frame.data?.identity || frame.agentId || "runtime";
    if (!groups.has(identity)) groups.set(identity, []);
    groups.get(identity).push(frame);
  }
  const entries = sortConversationTimelineEntries([...groups].flatMap(([identity, ownFrames]) => {
    const known = agents.get(identity);
    const agent = { identity, agent_id: identity, member_id: identity, label: known?.name || known?.display_name || identity,
      kind: "host", role: known?.role || "Rehearsal agent" };
    return mapFramesToTimelineEntries(agent, ownFrames, { renderInteractionStartsAsUser: true,
      renderTextDeltas: true, suppressEmbeddedRunStartedPrompt: true }).map((entry) => ({ ...entry,
        identity: entry.identity?.role === "assistant" ? { ...entry.identity, showLabel: true } : entry.identity }));
  }));
  return buildConversationViewState({ memberId: `rehearsal:${receipt?.rehearsal_id}`, agentLabel: "Your mob in rehearsal", entries });
}

export function rehearsalActivity(receipt, frames, state, { replay = false } = {}) {
  return rehearsalActivityFromTimeline(receipt, frames, state, { replay });
}

export function rehearsalActivityFromTimeline(receipt, frames, state, { replay = false } = {}) {
  if (!rehearsalCurrent(receipt, state) || !frames.length) return null;
  const identities = new Set((receipt.identities || []).map((item) => typeof item === "string" ? item : item.identity));
  const active = new Map(), completed = new Set(), starts = new Set(), terminals = new Set();
  let last = null;
  for (const envelope of frames) {
    const update = (envelope.event || envelope.kind) === "frame_updated"
      ? (envelope.payload || envelope.data)?.frame : null;
    const frame = update || envelope;
    const identity = frame.identity || frame.data?.identity || frame.agentId;
    if (!identities.has(identity)) continue;
    const event = frame.event || frame.kind;
    const interaction = frame.interactionId || frame.interaction_id;
    const run = frame.runId || frame.run_id;
    // Native peer continuations can omit run/interaction IDs. Their actual
    // source event (or stable frame) identifies a distinct run generation.
    const eventId = frame.source_event_id || frame.id || envelope.id;
    const generation = JSON.stringify([identity, eventId]);
    const key = JSON.stringify([identity, run || interaction || "activity"]);
    if (event === "run_started" && !starts.has(generation)) {
      starts.add(generation);
      active.set(`run:${generation}`, { identity, interaction, run });
    }
    if (["interaction_started", "text_delta", "reasoning_delta", "tool_call_requested"].includes(event)
      && !completed.has(key)) active.set(key, { identity, interaction, run });
    const interactionTerminal = ["interaction_complete", "interaction_failed", "interaction_canceled", "interaction_cancelled"].includes(event);
    if ((interactionTerminal || ["run_completed", "run_failed", "run_canceled", "run_cancelled"].includes(event))
      && !terminals.has(generation)) {
      // Replayed versions of an old completion cannot close a newer no-ID run.
      terminals.add(generation);
      completed.add(key);
      for (const [activeRun, value] of active) if (value.identity === identity &&
        (run ? value.run === run || !value.run && interactionTerminal && interaction && value.interaction === interaction
          : interaction ? value.interaction === interaction : true)) {
        active.delete(activeRun); completed.add(activeRun);
      }
      if (interactionTerminal && interaction) completed.add(JSON.stringify([identity, interaction]));
    }
    last = { identity, event, timestamp: frame.timestampMs || frame.timestamp_ms, cursor: envelope.cursor || frame.cursor };
  }
  if (!rehearsalActive(receipt) && !replay) active.clear();
  return { rehearsalId: receipt.rehearsal_id, status: replay ? "replaying" : receipt.status,
    scope: receipt.mode === "application" ? "native-application-trial" : "native-design-rehearsal",
    activeNodeIds: [...new Set([...active.values()].map((item) => item.identity))], last, frameCount: frames.length, flowId: receipt.flow_id || null };
}
