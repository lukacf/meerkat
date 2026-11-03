import { artifactIsCurrent } from "./artifact-current.mjs";
import { applicationTrialAvailable, applicationTrialSamples } from "./rehearsal-panel.mjs";
import {
  buildConversationViewState, mapFramesToTimelineEntries,
  sortConversationTimelineEntries,
} from "../../packages/console-core/src/index.ts";

const ACTIVE = new Set(["queued", "running", "checking"]);
const BUILD_STATUSES = new Set([...ACTIVE, "completed", "failed", "interrupted"]);

const FEEDBACK_PREFIX = "mobkit-implementation-feedback-v1:";
const pageFeedbackStores = new WeakMap();

/** Unsent human notes stay guide-owned in this page when browser persistence fails. */
export function createImplementationFeedbackStore(storageFactory = () => globalThis.localStorage) {
  const notes = new Map();
  const visible = (note) => ({ text: note.text, persistent: note.persistent });
  function read(guideId) {
    if (!notes.has(guideId)) {
      let text = "", persistent = false;
      try {
        const saved = storageFactory().getItem(FEEDBACK_PREFIX + guideId);
        if (saved === null || typeof saved === "string") { text = saved || ""; persistent = true; }
      } catch { /* Browser recovery is optional; current page notes remain authoritative. */ }
      notes.set(guideId, { text, persistent, edited: false });
    }
    return visible(notes.get(guideId));
  }
  function write(guideId, text) {
    const note = { text, persistent: false, edited: true };
    notes.set(guideId, note);
    try {
      const storage = storageFactory(), key = FEEDBACK_PREFIX + guideId;
      if (text) storage.setItem(key, text);
      else storage.removeItem(key);
      note.persistent = storage.getItem(key) === (text || null);
    } catch { /* Never replace an unsent edit with the older persisted value. */ }
    return visible(note);
  }
  function retry(guideId) {
    read(guideId);
    if (notes.get(guideId).edited) return write(guideId, notes.get(guideId).text);
    // An unread empty view must not erase a previously stored note.
    notes.delete(guideId);
    return read(guideId);
  }
  return { read, write, retry };
}

export function getImplementationPageFeedbackStore(page = globalThis.window) {
  if (!page || (typeof page !== "object" && typeof page !== "function")) return createImplementationFeedbackStore();
  if (!pageFeedbackStores.has(page)) pageFeedbackStores.set(page, createImplementationFeedbackStore());
  // Panel navigation can unmount the builder; a page reload still needs browser storage.
  return pageFeedbackStores.get(page);
}

export function createImplementationAdmissionGate() {
  const seen = new Set();
  return {
    consume(receipt, guide) {
      if (!receipt || typeof receipt.build_id !== "string" || !receipt.build_id ||
          typeof receipt.guide_id !== "string" || !receipt.guide_id ||
          !Number.isSafeInteger(receipt.guide_version) || !BUILD_STATUSES.has(receipt.status)) return null;
      const key = `${receipt.guide_id}:${receipt.build_id}`;
      if (seen.has(key)) return null;
      // An admission is a one-time notification. It must never replay an old
      // queued snapshot after polling advanced or after switching conversations.
      seen.add(key);
      return implementationIsCurrent(receipt, guide) ? receipt : null;
    },
  };
}

export function implementationIsRunning(receipt) {
  return ACTIVE.has(receipt?.status) || receipt?.cleanup_pending === true;
}

export function implementationIsCurrent(receipt, guide) {
  return artifactIsCurrent(receipt, guide);
}

export function implementationCanDownload(receipt, guide) {
  return receipt?.status === "completed" && receipt.ready_to_package === true &&
    receipt.cleanup_pending !== true &&
    implementationIsCurrent(receipt, guide);
}

export function implementationPackageUrl(receipt, guide) {
  if (!implementationCanDownload(receipt, guide) || !receipt.build_id) return null;
  const query = new URLSearchParams({ build_id: receipt.build_id });
  return `/api/guide/${encodeURIComponent(guide.id)}/implementation/package?${query}`;
}

export function implementationConversation(receipt, frames = []) {
  const identity = receipt.native_identity || "implementation:main";
  const agent = { identity, agent_id: identity, member_id: identity,
    label: "Application builder", kind: "host", role: "Implementation agent" };
  const identities = new Map();
  for (const frame of frames) {
    const owner = frame.worker_native_identity || frame.identity || identity;
    if (!identities.has(owner)) identities.set(owner, []);
    identities.get(owner).push(frame);
  }
  const entries = sortConversationTimelineEntries([...identities].flatMap(([owner, ownFrames]) => {
    const reviewer = owner === "acceptance:main";
    const ownAgent = { ...agent, identity: owner, agent_id: owner, member_id: owner,
      label: reviewer ? "Independent reviewer" : "Application builder",
      role: reviewer ? "Acceptance reviewer" : "Implementation agent" };
    return mapFramesToTimelineEntries(ownAgent, ownFrames, {
    // Adapt launch inputs as typed user entries before omitting them. Turning
    // this off instead exposes user_input as an expandable raw runtime event.
    renderInteractionStartsAsUser: true,
    renderTextDeltas: true,
    suppressEmbeddedRunStartedPrompt: true,
    });
  }).filter((entry) => entry.identity?.role !== "user").map((entry) => ({
    ...entry, identity: entry.identity?.role === "assistant" ? { ...entry.identity, showLabel: true } : entry.identity,
    ...(entry.blocks ? { blocks: entry.blocks.map((block) => block.type === "thinking"
      ? { ...block, label: block.label || "Reasoning summary" } : block) } : {}),
  })));
  return { entries, viewState: {
    ...buildConversationViewState({ memberId: identity,
      agentLabel: "Application builder", agent, entries }),
    conversationId: `implementation:${receipt.build_id}`,
  } };
}

export function implementationPhase(receipt) {
  const labels = {
    queued: "Waiting to build", running: "Building your application",
    checking: "Checking the application", completed: receipt?.acceptance_current ? "Application independently checked" : "Application source checked",
    failed: "This build needs attention", interrupted: "Build interrupted",
  };
  return {
    label: receipt?.cleanup_pending === true && !ACTIVE.has(receipt?.status) && receipt.cleanup_in_progress !== true ? "Build cleanup needs attention"
      : receipt?.stop_requested === true && implementationIsRunning(receipt) ? "Stopping your build"
      : receipt?.cleanup_in_progress === true && receipt.cleanup_pending === true ? "Closing the build runtime"
      : receipt?.phase === "independent_review" && implementationIsRunning(receipt) ? "Independent review in progress"
      : receipt?.phase === "automatic_repair" && implementationIsRunning(receipt) ? "Repairing review findings"
      : labels[receipt?.status] || "Build your application",
    running: implementationIsRunning(receipt),
    attention: ["failed", "interrupted"].includes(receipt?.status),
  };
}

export function implementationActivityStatus(receipt) {
  const phase = implementationPhase(receipt);
  return phase.running ? { label: phase.label, measurement: implementationTiming(receipt)?.summary || null } : null;
}

export function implementationAction(receipt, guide) {
  if (!receipt) return "Build application";
  if (!implementationIsCurrent(receipt, guide)) return "Build updated proposal";
  const acceptance = implementationAcceptance(receipt, guide);
  if (acceptance && !acceptance.passed) return "Continue repairing application";
  if (["failed", "interrupted"].includes(receipt.status)) return "Resume saved build";
  return acceptance?.passed ? "Continue implementation" : "Review and improve application";
}

export function implementationAcceptance(receipt, guide) {
  const report = receipt?.acceptance;
  const trusted = receipt?.acceptance_trusted === true && receipt?.acceptance_current === true &&
    implementationIsCurrent(receipt, guide) && report?.issuer === "mobkit-launch-acceptance-host" &&
    report.source_digest === receipt.source_digest;
  if (!trusted) return null;
  return { ...report, passed: report.status === "pass" && Number.isSafeInteger(report.tests_run) && report.tests_run > 0,
    requirements: Array.isArray(report.requirements) ? report.requirements : [],
    counterexamples: Array.isArray(report.counterexamples) ? report.counterexamples : [],
    remaining_dependencies: Array.isArray(report.remaining_dependencies) ? report.remaining_dependencies : [],
  };
}

export function implementationRepairHalt(receipt, guide) {
  const report = implementationAcceptance(receipt, guide), halt = receipt?.repair_halt;
  if (receipt?.status !== "failed" || receipt.phase !== "automatic_repair" || implementationIsRunning(receipt) ||
      receipt.source_verified !== true || receipt.ready_to_package !== false || report?.status !== "needs_repair" ||
      report.ready_to_package !== false || halt?.reason !== "unchanged_source_after_automatic_repair" ||
      !Number.isSafeInteger(halt.repair_round) || halt.repair_round < 1 || halt.repair_round !== receipt.repair_round ||
      !Number.isSafeInteger(halt.review_round) || halt.review_round < 1 || halt.review_round !== report.review_round ||
      !Number.isSafeInteger(halt.acceptance_policy_version) || halt.acceptance_policy_version < 1 ||
      halt.acceptance_policy_version !== report.acceptance_policy_version ||
      halt.acceptance_policy_version !== receipt.acceptance_policy_version) return null;
  const matches = ["source_digest", "blueprint_digest", "brief_digest", "binding_digest"].every((key) =>
    typeof halt[key] === "string" && halt[key].length === 64 && /^[a-f0-9]{64}$/.test(halt[key]) && halt[key] === report[key]);
  if (!matches) return null;
  return { message: "The last automatic repair saved no source change. The independent findings below still need attention.",
    next: "Review the unmet outcomes. Add an implementation change below, or revise the proposal with your guide before continuing." };
}

export function implementationOverview(receipt, guide) {
  if (!receipt) return { title: null,
    message: "The builder will turn your conversation and agent design into an application, test it, and prepare its package.",
    next: "Add anything else you want it to do, or start building below.", attention: false };
  if (receipt.cleanup_pending === true && receipt.cleanup_custody_lost === true) return { title: "Build cleanup needs recovery",
    message: "This installer no longer has the worker handle needed to confirm cleanup. Your saved source and reports are preserved.",
    next: "Recovery needs to confirm cleanup before another build can start. You can inspect the saved source and reports below.", attention: true };
  if (implementationIsRunning(receipt)) {
    const phase = implementationPhase(receipt);
    if (receipt.cleanup_pending === true && !ACTIVE.has(receipt.status) && receipt.cleanup_in_progress !== true) return { title: phase.label,
      message: "The worker has not finished closing its runtime. Its saved source is preserved.",
      next: "Retry cleanup below. A new build will be available after cleanup is confirmed.", attention: true };
    if (receipt.stop_requested === true || receipt.cleanup_in_progress === true) return { title: phase.label,
      message: "The builder is stopping and closing its runtime. Its saved source is preserved.",
      next: "Resume the saved build once cleanup finishes.", attention: false };
    if (!implementationIsCurrent(receipt, guide)) return { title: "An earlier proposal is still building",
      message: "Your setup changed while this build was working. Its source and recorded activity are preserved.",
      next: "Stop this build below before building your updated design.", attention: true };
    return { title: phase.label,
      message: receipt.phase === "independent_review" ? "A separate reviewer is checking the application's behavior against your requested outcomes."
        : receipt.phase === "automatic_repair" ? "The builder is working on findings from the independent review."
        : receipt.status === "checking" ? "The host is checking the generated source and its tests."
        : receipt.status === "queued" ? "Your build request is saved and waiting to start."
        : "The builder is working from the saved application requirements for your proposed agent system.",
      next: "You can follow the builder and reviewer activity below. Installation and live connections still need separate verification.", attention: false };
  }
  if (!implementationIsCurrent(receipt, guide)) return { title: "Build your updated design",
    message: "This saved build belongs to an earlier proposal or application brief.",
    next: "Build the current design before downloading or installing. The earlier source and reports remain available below.", attention: true };
  const acceptance = implementationAcceptance(receipt, guide);
  if (acceptance && !acceptance.passed) return { title: "Your application needs repair",
    message: "The independent review found work that still needs to be completed.",
    next: "Continue below to send the saved findings and regression tests back to the builder. You can add a preference; technical repair instructions are optional.", attention: true };
  if (receipt.status === "failed") return { title: "This build needs attention",
    message: "The build did not finish with a verified package. Its source and diagnostics are saved.",
    next: "Resume the saved build below. The builder can use its previous work; you do not need to start from scratch.", attention: true };
  if (receipt.status === "interrupted") return { title: "Your build was interrupted",
    message: "The saved source and recorded activity are still available.",
    next: "Resume the saved build below to finish its checks and independent review.", attention: true };
  if (acceptance?.passed && implementationCanDownload(receipt, guide)) return { title: "Your application passed offline review",
    message: "The current source passed independent offline tests and has a checked package.",
    next: "Try a sample job, or configure an installation. Live connections, custom interfaces and deployment still need their own checks.", attention: false };
  return { title: "The saved source needs further verification",
    message: "Source checks alone do not establish current independent acceptance or a verified package.",
    next: "Review and continue implementation below before installing.", attention: true };
}

export function implementationReport(receipt) {
  const rows = [];
  for (const [label, value] of [["Builder summary", receipt?.summary], ["Build diagnostic", receipt?.detail],
    ["Cleanup diagnostic", receipt?.cleanup_detail]]) {
    if (typeof value === "string" && value.trim() && !rows.some((row) => row.text === value))
      rows.push({ label, text: value });
  }
  return rows;
}

export function implementationSampleSummary(receipt, guide) {
  if (!applicationTrialAvailable(receipt, guide) || !receipt.trial_samples ||
      receipt.trial_samples.source_digest !== receipt.source_digest) return null;
  const count = applicationTrialSamples(receipt, guide).length;
  return count ? { count, message: `${count} prepared sample ${count === 1 ? "job is" : "jobs are"} available in Try your design. Samples use offline data and do not verify live connections.` }
    : { count: 0, message: "No prepared sample jobs are available for this checked source. You can enter your own request in Try your design, or ask the builder below to add useful sample choices." };
}

export function implementationFiles(receipt) {
  return (Array.isArray(receipt?.files) ? receipt.files : [])
    .filter((file) => file && typeof file.path === "string" && file.path.length)
    .map((file) => ({ ...file }));
}

const timingLabels = {
  preparation: "Preparing source", gateway: "Preparing the native runtime",
  storage: "Preparing native storage", bootstrap: "Starting native agents",
  builder_native: "Builder native run", reviewer_native: "Reviewer native run",
  native_admission: "Native request admission", review_snapshot: "Freezing review source",
  review_seal: "Verifying completed review", final_checks: "Verifying the final package",
  shutdown: "Stopping the build runtime", tool: "Host tool callbacks",
};
const validDuration = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 86400000;
const timingDuration = (milliseconds) => milliseconds < 60000
  ? `${(milliseconds / 1000).toFixed(1)}s`
  : `${Math.floor(milliseconds / 60000)}m ${Math.floor(milliseconds % 60000 / 1000)}s`;

export function implementationTiming(receipt) {
  const timing = receipt?.timing;
  if (!timing || timing.version !== 1 || timing.build_id !== receipt.build_id ||
      timing.scope !== "host_build" || timing.overlap !== true || !validDuration(timing.elapsed_ms)) return null;
  const phases = (Array.isArray(timing.phases) ? timing.phases : []).filter((row) =>
    row && Object.hasOwn(timingLabels, row.phase) && validDuration(row.duration_ms) &&
    Number.isSafeInteger(row.count) && row.count > 0 && row.count <= 1000000)
    .map((row) => ({ phase: row.phase, label: timingLabels[row.phase], count: row.count,
      duration: timingDuration(row.duration_ms) }));
  return { summary: `${timing.partial === true ? "Partial measurement" : timing.finished === true ? "Recorded build time" : "Recorded so far"}: ${timingDuration(timing.elapsed_ms)}`,
    phases, partial: timing.partial === true };
}

export function implementationChecks(receipt) {
  return (Array.isArray(receipt?.checks) ? receipt.checks : [])
    .filter((check) => check && typeof check.name === "string")
    .map((check) => ({ ...check }));
}

export function implementationCheckSummary(receipt) {
  const checks = implementationChecks(receipt);
  const passed = checks.filter((check) => check.status === "pass").length;
  const failed = checks.filter((check) => check.status === "fail").length;
  const unverified = checks.length - passed - failed;
  const allPassed = checks.length > 0 && passed === checks.length;
  const sourceDigest = typeof receipt?.source_digest === "string" && receipt.source_digest;
  const checkedDigests = [receipt?.checked_digest, receipt?.receipt?.digest]
    .filter((digest) => typeof digest === "string" && digest);
  const saved = receipt?.source_verified === false || !!(sourceDigest && checkedDigests.some((digest) => digest !== sourceDigest));
  const count = receipt?.receipt?.tests_run;
  const authoredTests = Number.isSafeInteger(count) && count > 0 ? count : null;
  const outcomes = allPassed ? `${passed} ${passed === 1 ? "check" : "checks"} passed`
    : [passed && `${passed} passed`, failed && `${failed} failed`, unverified && `${unverified} not verified`]
      .filter(Boolean).join(", ");
  return { allPassed, saved, attention: checks.length > 0 && (!allPassed || saved),
    passed, failed, unverified, authoredTests,
    label: (saved ? "Saved check results: " : "") + outcomes + (authoredTests ? ` - ${authoredTests} authored ${authoredTests === 1 ? "test" : "tests"} run` : ""),
  };
}

export function implementationUnresolved(receipt) {
  return [...new Set((Array.isArray(receipt?.unresolved) ? receipt.unresolved : [])
    .filter((item) => typeof item === "string" && item.trim()))];
}

export function implementationProgress(receipt) {
  return (Array.isArray(receipt?.progress) ? receipt.progress : [])
    .filter((event) => event && typeof event.message === "string" && event.message.trim())
    .map((event) => ({ ...event }));
}

const savedActivityTime = (value) => typeof value === "string" && value.length <= 40 &&
  /^\d{4}-\d{2}-\d{2}T/.test(value) && Number.isFinite(Date.parse(value)) ? value : null;

/** Show a saved host step, without inferring what the native agent is doing now. */
export function implementationWaitingActivity(receipt, guide) {
  if (!receipt || receipt.guide_id !== guide?.id || typeof receipt.build_id !== "string" ||
      !receipt.build_id || !Number.isSafeInteger(receipt.guide_version) ||
      !BUILD_STATUSES.has(receipt.status) || !implementationIsRunning(receipt)) return null;
  const events = Array.isArray(receipt.progress) ? receipt.progress.slice(-100) : [];
  for (let index = events.length - 1; index >= 0; index--) {
    const event = events[index], at = savedActivityTime(event?.at), message = event?.message;
    if (at && typeof message === "string" && message.length <= 2000 && message.trim())
      return { current: implementationIsCurrent(receipt, guide), at, message };
  }
  return null;
}

/** Summarize host-recorded events without projecting paths, arguments or prose. */
export function implementationSavedActivity(receipt, guide) {
  if (!receipt || receipt.guide_id !== guide?.id || typeof receipt.build_id !== "string" ||
      !receipt.build_id || !Number.isSafeInteger(receipt.guide_version) || !BUILD_STATUSES.has(receipt.status)) return null;
  const current = implementationIsCurrent(receipt, guide), rows = [];
  let source, check;
  for (const [index, event] of (Array.isArray(receipt.progress) ? receipt.progress : []).entries()) {
    const at = savedActivityTime(event?.at), message = event?.message;
    if (!at || typeof message !== "string") continue;
    const batch = /^Updated (?:[1-9]|[12][0-9]|3[0-2]) source files in one checked batch\.$/.exec(message);
    if (/^(Writing |Editing )\S/.test(message) || batch?.[0] === message) source = { index, at };
    const outcome = message === "Running isolated syntax and application tests." ? "running"
      : message === "Application checks passed." ? "pass"
      : message === "Application checks need attention." ? "attention" : null;
    if (outcome) check = { index, at, outcome };
  }
  if (source) rows.push({ kind: "source", at: source.at, message: "Application source change saved." });
  if (check) {
    const superseded = source && source.index > check.index;
    const running = check.outcome === "running" && receipt.status === "checking" &&
      !superseded && receipt.stop_requested !== true && receipt.cleanup_in_progress !== true;
    rows.push({ kind: "check", at: check.at, message: superseded
      ? "Earlier source checks are out of date after the saved change."
      : running ? "Checking the saved source and its offline tests."
      : check.outcome === "pass" ? "Last source check passed. Independent review and installation are separate."
      : check.outcome === "attention" ? "Last source check found work to do."
      : "A source check was started; its result was not recorded." });
  }
  const failures = receipt.builder_publication_failures;
  const recovery = Array.isArray(failures) ? failures.find((row) =>
    row?.reason === "completed_without_current_publication" && row.retry_requested === true &&
    ["initial_build", "automatic_repair"].includes(row.builder_phase) &&
    typeof row.source_digest === "string" && /^[a-f0-9]{64}$/.test(row.source_digest) && savedActivityTime(row.at)) : null;
  if (receipt.builder_publication_retries === 1 && recovery) rows.push({ kind: "recovery", at: recovery.at,
    message: "One additional builder turn was requested to finish checked source publication." });
  return rows.length ? { current, label: current ? "Latest saved build activity" : "Saved activity from an earlier design", rows } : null;
}
