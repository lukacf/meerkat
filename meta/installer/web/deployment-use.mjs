import { artifactIsCurrent } from "./artifact-current.mjs";
import { deploymentConsole, deploymentExperiences, deploymentUrl } from "./deployment-panel.mjs";
import { installedTaskHandoffUrl } from "./installed-task-handoff.mjs";

export const INSTALLED_USE_OUTCOMES = Object.freeze([
  { id: "worked", label: "It worked as expected" },
  { id: "partly", label: "It partly worked" },
  { id: "not_worked", label: "It did not work" },
  { id: "blocked", label: "I could not try it" },
]);
const DIGEST = /^[a-f0-9]{64}$/;
const TERMINAL = new Set(["completed", "failed", "interrupted"]);
const text = (value, limit) => typeof value === "string" && value.trim() && value.length <= limit &&
  !/[\u0000-\u001f\u007f]/.test(value) ? value : null;

/** A native team's conversation owner comes from its checked installed source. */
export function installedNativeTeamUse(metadata, receipt, guide) {
  if (receipt?.source_kind !== "native-team" || receipt.guide_id !== guide?.id || receipt.build_id != null ||
      metadata?.source_kind !== "native-team" || metadata.scope !== "verified-installed-source" ||
      metadata.application_outcomes_verified !== false || metadata.build_id != null ||
      ![receipt.source_id, receipt.source_digest, receipt.package_sha256].every((value) => typeof value === "string" && DIGEST.test(value)) ||
      metadata.deployment_id !== receipt.deployment_id || metadata.source_id !== receipt.source_id ||
      metadata.source_digest !== receipt.source_digest || metadata.package_sha256 !== receipt.package_sha256 ||
      metadata.guide_version !== receipt.guide_version || metadata.version !== guide.version || metadata.status !== receipt.status) return null;
  const name = text(metadata.project?.name, 200), identity = text(metadata.entrypoint?.identity, 180), entryName = text(metadata.entrypoint?.name, 200);
  if (!name || !identity || !entryName) return null;
  const url = deploymentConsole(receipt);
  const consoleUrl = url && metadata.native_runtime_available === true && metadata.console_url === url &&
    !new URL(url).search && !new URL(url).hash ? url : null;
  return { name, entrypoint: { identity, name: entryName }, consoleUrl };
}

/** Optional authored ideas belong to this installed source, never trial data. */
export function installedUseStartingIdeas(value) {
  const empty = { tasks: [], detail: null };
  if (value?.scope !== "authored-first-use-suggestions" || !Array.isArray(value.tasks) || value.tasks.length > 8 ||
      (value.metadata_sha256 !== null && (typeof value.metadata_sha256 !== "string" || !DIGEST.test(value.metadata_sha256))) ||
      value.tasks.length > 0 && value.metadata_sha256 === null) return empty;
  const visibleText = (input, limit, multiline = false) => typeof input === "string" && input.trim() && input.length <= limit * 2 &&
    Array.from(input).length <= limit && !(multiline ? /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/ : /[\u0000-\u001f\u007f]/).test(input);
  if (value.detail != null && !visibleText(value.detail, 600)) return empty;
  const tasks = [];
  for (const item of value.tasks) {
    if (!item || !visibleText(item.label, 120) || typeof item.request !== "string" || !item.request.trim() ||
        !installedUseTaskAllowed(item.request) || /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(item.request) ||
        item.description !== undefined && !visibleText(item.description, 400, true) ||
        ["inputs", "prerequisites"].some((field) => item[field] !== undefined && (!Array.isArray(item[field]) ||
          item[field].length > 8 || item[field].some((entry) => !visibleText(entry, 400, true)))) ||
        item.expected_result !== undefined && !visibleText(item.expected_result, 600, true) ||
        item.result_location !== undefined && !visibleText(item.result_location, 400, true)) return empty;
    tasks.push({ label: item.label, request: item.request,
      ...Object.fromEntries(["description", "expected_result", "result_location"].filter((field) => item[field] !== undefined)
        .map((field) => [field, item[field]])),
      ...Object.fromEntries(["inputs", "prerequisites"].filter((field) => item[field] !== undefined)
        .map((field) => [field, [...item[field]]])) });
  }
  const entryId = typeof value.entry_id === "string" && /^[a-z][a-z0-9_-]{0,63}$/.test(value.entry_id) ? value.entry_id : null;
  return { tasks, detail: value.detail ?? null, ...(entryId ? { entryId } : {}) };
}

/** Match the exact installed declaration; never select another UI or Console. */
export function installedUseTaskHandoff(use, task) {
  const entryId = use?.startingIdeas?.entryId;
  if (!use?.consoleUrl || use?.interfaces?.status !== "declared" || use.interfaces.issue || !entryId) return null;
  const entry = use.interfaces.entries.find((item) => item.id === entryId);
  const url = entry && installedTaskHandoffUrl(entry.url, task);
  return url ? { url, entryId, label: entry.label } : null;
}

export function installedUseIdeaPreview(value) {
  const characters = Array.from(value.replace(/\s+/g, " ").trim());
  if (characters.length <= 100) return characters.join("");
  let end = 97;
  if (characters[end] !== " " && characters[end - 1] !== " ") {
    const boundary = characters.slice(0, end).lastIndexOf(" ");
    if (boundary > 0) end = boundary;
  }
  return characters.slice(0, end).join("").trimEnd() + "...";
}

export const INSTALLED_USE_TASK_MAX_CHARACTERS = 3000;
export const INSTALLED_USE_TASK_MAX_UNITS = 6000;
export const INSTALLED_USE_TASK_LIMIT_MESSAGE = "This edit exceeds the 3,000-character limit. Shorten it and try again. Your previous draft remains here.";

export function installedUseTaskAllowed(value) {
  return typeof value === "string" && value.length <= INSTALLED_USE_TASK_MAX_UNITS &&
    Array.from(value).length <= INSTALLED_USE_TASK_MAX_CHARACTERS;
}

// Draft ownership follows installed source, independently of live health and
// harmless transcript revisions. This scope never authorizes a live request.
export function installedUseScopeKey(receipt, guide) {
  if (receipt?.guide_id !== guide?.id || !text(receipt.deployment_id, 200) ||
      !text(receipt.build_id, 200) ||
      !DIGEST.test(receipt.package_sha256 || "") || !Number.isSafeInteger(receipt.guide_version)) return "";
  return JSON.stringify([guide.id, receipt.deployment_id, receipt.build_id,
    receipt.package_sha256, receipt.guide_version]);
}

export function installedUseReadKey(receipt, guide, build) {
  const scope = installedUseScopeKey(receipt, guide);
  if (!scope || !Number.isSafeInteger(guide.version)) return "";
  return JSON.stringify([scope, guide.version, receipt.status, deploymentConsole(receipt),
    build?.build_id, build?.source_digest, build?.status, build?.current,
    build?.current_guide_version ?? build?.guide_version, build?.source_verified]);
}

const DRAFT_PREFIX = "mobkit-installed-use-draft-v1:";
const SELECTION_PREFIX = "mobkit-installed-use-selection-v1:";
const MAX_DRAFT_BYTES = 100000;
const MAX_SELECTION_BYTES = 1000;
const emptyDraft = () => ({ task: "", outcome: "", improvement: "" });

function boundedIdeaSelection(value) {
  if (!value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).length !== 2 || Object.keys(value).some((field) => !["catalogue_sha256", "index"].includes(field)) ||
      !DIGEST.test(value.catalogue_sha256 || "") || !Number.isSafeInteger(value.index) || value.index < 0 || value.index >= 8) return null;
  return { catalogue_sha256: value.catalogue_sha256, index: value.index };
}

/** Restore instructions from the checked catalogue, never from saved content. */
export function installedUseSelectedIdea(firstUse, selection) {
  const selected = boundedIdeaSelection(selection);
  if (!selected || selected.catalogue_sha256 !== firstUse?.metadata_sha256) return null;
  return installedUseStartingIdeas(firstUse).tasks[selected.index] || null;
}

function boundedUseDraft(value) {
  if (!value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).some((field) => !["task", "outcome", "improvement"].includes(field)) ||
      !installedUseTaskAllowed(value.task) || typeof value.improvement !== "string" ||
      value.improvement.length > 6000 || typeof value.outcome !== "string" ||
      value.outcome !== "" && !INSTALLED_USE_OUTCOMES.some((item) => item.id === value.outcome)) return null;
  return { task: value.task, outcome: value.outcome, improvement: value.improvement };
}

function useDraftKey(scope) {
  try {
    const owner = JSON.parse(scope);
    return Array.isArray(owner) && owner.length === 5 && owner.slice(0, 3).every((value) => text(value, 200)) &&
      typeof owner[3] === "string" && DIGEST.test(owner[3]) && Number.isSafeInteger(owner[4]) &&
      JSON.stringify(owner) === scope ? DRAFT_PREFIX + scope : null;
  } catch { return null; }
}

/** Human drafts and a separate source-scoped walkthrough choice, with a page fallback. */
export function createInstalledUseDraftStore(storage) {
  const records = new Map(), selections = new Map(), selectionBlocked = new Set();
  const selectionKey = (scope) => useDraftKey(scope) ? SELECTION_PREFIX + scope : null;
  const storageAvailable = () => storage?.getItem && storage?.setItem && storage?.removeItem;
  function selection(scope) {
    const key = selectionKey(scope);
    if (!key) return null;
    if (!selections.has(key)) {
      let selected = null;
      try {
        const raw = storage?.getItem?.(key);
        if (typeof raw === "string" && new TextEncoder().encode(raw).byteLength <= MAX_SELECTION_BYTES) {
          const saved = JSON.parse(raw);
          if (saved && !Array.isArray(saved) && Object.keys(saved).length === 2 &&
              Object.keys(saved).every((field) => ["scope", "selection"].includes(field)) && saved.scope === scope)
            selected = boundedIdeaSelection(saved.selection);
        }
      } catch { /* Optional walkthrough storage cannot discard the human draft. */ }
      selections.set(key, selected);
    }
    return selections.get(key);
  }
  function entry(scope) {
    const key = useDraftKey(scope);
    if (!key) return { draft: emptyDraft(), persistent: false };
    if (!records.has(key)) {
      let draft = emptyDraft(), persistent = false;
      try {
        if (storage?.getItem && storage?.setItem && storage?.removeItem) {
          const raw = storage.getItem(key);
          persistent = true;
          if (typeof raw === "string" && new TextEncoder().encode(raw).byteLength <= MAX_DRAFT_BYTES) {
            const saved = JSON.parse(raw);
            if (saved && Object.keys(saved).every((field) => ["scope", "draft"].includes(field)) && saved.scope === scope)
              draft = boundedUseDraft(saved.draft) || draft;
          }
        }
      } catch { persistent = false; }
      records.set(key, { draft, persistent });
    }
    return records.get(key);
  }
  function save(scope, draft, persist = true) {
    const key = useDraftKey(scope);
    let persistent = false;
    try {
      if (persist && !selectionBlocked.has(scope) && storageAvailable()) {
        if (Object.values(draft).some(Boolean)) {
          const raw = JSON.stringify({ scope, draft });
          if (new TextEncoder().encode(raw).byteLength <= MAX_DRAFT_BYTES) {
            storage.setItem(key, raw);
            persistent = storage.getItem(key) === raw;
          }
        } else {
          storage.removeItem(key);
          persistent = storage.getItem(key) === null;
        }
      }
    } catch { /* Keep the current human draft in this open page. */ }
    records.set(key, { draft, persistent });
    return true;
  }
  return {
    read(scope) { return { ...entry(scope).draft }; },
    persistent(scope) { return entry(scope).persistent; },
    selectedIdea(scope) { const selected = selection(scope); return selected ? { ...selected } : null; },
    selectIdea(scope, firstUse, index) {
      const key = selectionKey(scope), selected = boundedIdeaSelection({ catalogue_sha256: firstUse?.metadata_sha256, index });
      const idea = installedUseSelectedIdea(firstUse, selected);
      if (!key || !idea) return false;
      const draft = boundedUseDraft({ ...entry(scope).draft, task: idea.request });
      // Retire the old sidecar before replacing its task. If storage refuses,
      // keep the new pair only in memory so reload cannot join mismatched data.
      let cleared = false;
      try {
        if (storageAvailable()) { storage.removeItem(key); cleared = storage.getItem(key) === null; }
      } catch { /* Preserve the previous stored pair and the new open-page pair. */ }
      if (cleared) selectionBlocked.delete(scope); else selectionBlocked.add(scope);
      save(scope, draft, cleared);
      selections.set(key, selected);
      if (cleared && entry(scope).persistent) {
        try { storage.setItem(key, JSON.stringify({ scope, selection: selected })); } catch { /* The human task is still saved. */ }
      }
      return true;
    },
    update(scope, field, value) {
      if (!useDraftKey(scope) || !["task", "outcome", "improvement"].includes(field)) return false;
      const draft = boundedUseDraft({ ...entry(scope).draft, [field]: value });
      return draft ? save(scope, draft) : false;
    },
  };
}

export function installedUseSessionStorage(page = globalThis) {
  try { return page?.sessionStorage ?? null; } catch { return null; }
}

const pageInstalledUseStores = new WeakMap();

/** Keep source-scoped drafts and exact report tracking in this window until it closes. */
export function getInstalledUsePageStores(page = globalThis.window) {
  const ownsPage = !!page && (typeof page === "object" || typeof page === "function");
  const create = () => {
    const improvementStorage = ownsPage ? installedUseSessionStorage(page) : null;
    return { drafts: createInstalledUseDraftStore(improvementStorage),
      successStore: createInstalledUseSuccessStore(improvementStorage), improvementStorage,
      improvementStore: createInstalledUseImprovementStore(improvementStorage) };
  };
  if (!ownsPage) return create();
  if (!pageInstalledUseStores.has(page)) pageInstalledUseStores.set(page, create());
  return pageInstalledUseStores.get(page);
}

const IMPROVEMENT_PREFIX = "mobkit-installed-use-improvement-v1:";
const IMPROVEMENT_FIELDS = ["guideId", "version", "sourceDigest", "continuationBuildId", "continuationSourceDigest",
  "task", "outcome", "request", "buildId", "hostRequest"];

function boundedImprovementHint(scope, value) {
  if (!useDraftKey(scope) || !value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).length !== IMPROVEMENT_FIELDS.length ||
      !Object.keys(value).every((key) => IMPROVEMENT_FIELDS.includes(key))) return null;
  const owner = JSON.parse(scope);
  if (value.guideId !== owner[0] || !Number.isSafeInteger(value.version) || value.version < owner[4] ||
      typeof value.sourceDigest !== "string" || !DIGEST.test(value.sourceDigest) || !text(value.continuationBuildId, 200) ||
      typeof value.continuationSourceDigest !== "string" || !DIGEST.test(value.continuationSourceDigest) || !installedUseTaskAllowed(value.task) ||
      value.task !== value.task.trim() || !INSTALLED_USE_OUTCOMES.some((item) => item.id === value.outcome) ||
      typeof value.request !== "string" || !value.request.trim() || value.request.length > 6000 ||
      value.request !== value.request.trim() || value.buildId !== null && !text(value.buildId, 200) ||
      value.hostRequest !== null && (typeof value.hostRequest !== "string" || !value.hostRequest.trim() ||
        value.hostRequest.length > 100000 || value.buildId === null)) return null;
  return Object.fromEntries(IMPROVEMENT_FIELDS.map((key) => [key, value[key]]));
}

/** Retain navigation pins and report text separately from checked Build state. */
export function installedUseImprovementHint(scope, body) {
  if (!useDraftKey(scope)) return null;
  const owner = JSON.parse(scope);
  if (body?.deployment_id !== owner[1].replace(/^remote:/, "") || body.build_id !== owner[2] ||
      body.package_sha256 !== owner[3]) return null;
  return boundedImprovementHint(scope, { guideId: owner[0], version: body.version,
    sourceDigest: body.source_digest, continuationBuildId: body.continuation_build_id,
    continuationSourceDigest: body.continuation_source_digest, task: body.task, outcome: body.outcome,
    request: body.request, buildId: null, hostRequest: null });
}

export function installedUseImprovementIsSameReport(hint, body) {
  return !!hint && !!body && hint.task === body.task && hint.outcome === body.outcome && hint.request === body.request;
}

export function installedUseFeedbackNotAdmitted(error) {
  return error?.code === "installed_feedback_not_admitted" && [409, 422].includes(error.status);
}

export function installedUseImprovementMatches(hint, receipt) {
  return !!hint?.buildId && typeof hint.hostRequest === "string" && hint.hostRequest.trim() &&
    receipt?.guide_id === hint.guideId && receipt.guide_version === hint.version && receipt.build_id === hint.buildId &&
    receipt.request === hint.hostRequest && ["queued", "running", "checking", "completed", "failed", "interrupted"].includes(receipt.status);
}

/** A hint blocks accidental resends; only a fresh host receipt restores work. */
export function createInstalledUseImprovementStore(storage) {
  const records = new Map();
  let writesUnavailable = false;
  function keyFor(scope) { return useDraftKey(scope) ? IMPROVEMENT_PREFIX + scope : null; }
  function entry(scope) {
    const key = keyFor(scope);
    if (!key) return { hint: null, persistent: false };
    if (!records.has(key)) {
      let hint = null, persistent = false;
      try {
        if (storage?.getItem && storage?.setItem && storage?.removeItem) {
          const raw = storage.getItem(key); persistent = true;
          if (raw !== null) {
            // Damaged retained tracking must not release a possibly sent report.
            hint = { corrupt: true };
            if (typeof raw === "string" && new TextEncoder().encode(raw).byteLength <= 200000) {
              const saved = JSON.parse(raw);
              if (saved && Object.keys(saved).length === 2 && saved.scope === scope)
                hint = boundedImprovementHint(scope, saved.hint) || hint;
            }
          }
        }
      } catch { persistent = false; }
      records.set(key, { hint, persistent });
    }
    return records.get(key);
  }
  return {
    read(scope) { const hint = entry(scope).hint; return hint ? { ...hint } : null; },
    persistent(scope) { return entry(scope).persistent; },
    save(scope, value) {
      const key = keyFor(scope), hint = boundedImprovementHint(scope, value);
      if (!key || !hint) return false;
      let persistent = false;
      try {
        if (!writesUnavailable && storage?.getItem && storage?.setItem && storage?.removeItem) {
          const raw = JSON.stringify({ scope, hint });
          if (new TextEncoder().encode(raw).byteLength <= 200000) {
            storage.setItem(key, raw); persistent = storage.getItem(key) === raw;
            if (!persistent) writesUnavailable = true;
          }
        }
      } catch { writesUnavailable = true; /* Keep exact tracking in the open page without claiming reload recovery. */ }
      records.set(key, { hint, persistent }); return true;
    },
    forgetRejected(scope, expected) {
      const key = keyFor(scope), hint = boundedImprovementHint(scope, expected), retained = entry(scope);
      // A rejection of this POST cannot retire another request or a confirmed
      // admission. Preserve uncertain tracking if browser storage changed.
      if (!key || !hint || hint.buildId !== null || hint.hostRequest !== null ||
          JSON.stringify(retained.hint) !== JSON.stringify(hint)) return false;
      // The caller's rejection witness retires only this page's cached request.
      // Leave unverified storage bytes untouched without claiming durable removal.
      if (retained.persistent === false) {
        records.set(key, { hint: null, persistent: false }); return true;
      }
      let persistent = false;
      if (storage) {
        try {
          if (!storage.getItem || !storage.removeItem) return false;
          const raw = storage.getItem(key);
          if (raw !== null) {
            const saved = JSON.parse(raw);
            if (saved?.scope !== scope || Object.keys(saved).length !== 2 ||
                JSON.stringify(boundedImprovementHint(scope, saved.hint)) !== JSON.stringify(hint)) return false;
            storage.removeItem(key);
          }
          if (storage.getItem(key) !== null) return false;
          persistent = true;
        } catch { return false; }
      }
      records.set(key, { hint: null, persistent }); return true;
    },
  };
}

export function installedApplicationUse(metadata, receipt, guide, build, experiences = null) {
  if (!installedUseScopeKey(receipt, guide) || metadata?.scope !== "verified-installed-source" ||
      metadata.application_outcomes_verified !== false || metadata.deployment_id !== receipt.deployment_id ||
      metadata.build_id !== receipt.build_id || !DIGEST.test(metadata.source_digest || "") ||
      receipt.source_digest !== undefined && metadata.source_digest !== receipt.source_digest ||
      metadata.package_sha256 !== receipt.package_sha256 || metadata.guide_version !== receipt.guide_version ||
      !Number.isSafeInteger(metadata.guide_version) || metadata.version !== guide.version ||
      metadata.status !== receipt.status) return null;
  const name = text(metadata.project?.name, 200), identity = text(metadata.entrypoint?.identity, 180),
    entryName = text(metadata.entrypoint?.name, 200);
  if (!name || !identity || !entryName) return null;
  const liveUrl = deploymentConsole(receipt);
  const consoleUrl = liveUrl && metadata.native_runtime_available === true && metadata.console_url === liveUrl &&
    !new URL(liveUrl).search && !new URL(liveUrl).hash ? liveUrl : null;
  // The installed-source read supplies the digest when the ordinary receipt
  // does not. A selected candidate cannot provide this interface authority.
  const declared = deploymentExperiences(experiences, { ...receipt, source_digest: metadata.source_digest }, guide.id);
  // A checked empty declaration is distinct from a read that did not finish.
  // The shared projection's empty-entry branch does not validate these digests.
  const discoveryChecked = !!experiences && !declared.issue &&
    typeof experiences.source_digest === "string" && DIGEST.test(experiences.source_digest) &&
    experiences.source_digest === metadata.source_digest &&
    (experiences.metadata_sha256 === null || typeof experiences.metadata_sha256 === "string" && DIGEST.test(experiences.metadata_sha256));
  const interfaces = { ...declared,
    status: discoveryChecked ? declared.entries.length ? "declared" : "none" : "unresolved",
    entries: declared.entries.map((entry) => ({ ...entry,
    url: consoleUrl ? entry.url : null })) };
  const selected = metadata.continuation;
  const canContinue = selected?.available === true && artifactIsCurrent(build, guide) &&
    build.source_verified === true && TERMINAL.has(build.status) &&
    selected.build_id === build.build_id && selected.source_digest === build.source_digest &&
    DIGEST.test(selected.source_digest || "");
  const newerCandidate = !!text(selected?.build_id, 200) && DIGEST.test(selected?.source_digest || "") &&
    (selected.build_id !== metadata.build_id || selected.source_digest !== metadata.source_digest);
  return { name, entrypoint: { identity, name: entryName }, consoleUrl, interfaces,
    installedCurrent: metadata.current === true,
    startingIdeas: installedUseStartingIdeas(metadata.first_use),
    continuation: { available: canContinue, newerCandidate,
      requiresAcknowledgement: metadata.current !== true || newerCandidate || selected?.requires_acknowledgement === true },
  };
}

export function installedUseFeedback(metadata, receipt, guide, build, report, blocked = false) {
  const use = installedApplicationUse(metadata, receipt, guide, build);
  if (blocked || !use?.continuation.available || !INSTALLED_USE_OUTCOMES.some((item) => item.id === report?.outcome) ||
      typeof report.request !== "string" || !report.request.trim() || report.request.length > 6000 ||
      !installedUseTaskAllowed(report.task ?? "") ||
      use.continuation.requiresAcknowledgement && report.continueCurrentCandidate !== true) return null;
  return { version: guide.version, deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: metadata.source_digest, package_sha256: receipt.package_sha256,
    continuation_build_id: build.build_id, continuation_source_digest: build.source_digest,
    continue_current_candidate: report.continueCurrentCandidate === true,
    outcome: report.outcome, request: report.request.trim(), task: (report.task ?? "").trim() };
}

export function installedUseUrl(guideId, operation = "use") {
  return `${deploymentUrl(guideId)}/${operation === "feedback" ? "feedback" : operation === "success" ? "use/success" : "use"}`;
}

const SUCCESS_FIELDS = ["version", "deployment_id", "build_id", "source_digest", "package_sha256", "request_id", "task"];
const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const SUCCESS_PREFIX = "mobkit-installed-use-success-v1:";
const plainTask = (value) => installedUseTaskAllowed(value) && value.trim() &&
  !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(value) &&
  !Array.from(value).some((character) => character.length === 1 && character.charCodeAt(0) >= 0xd800 && character.charCodeAt(0) <= 0xdfff);

export function installedUseSuccessRequest(binding, task, requestId) {
  if (!Number.isSafeInteger(binding?.version) || binding.version < 1 || !text(binding.deployment_id, 200) ||
      !text(binding.build_id, 200) || !DIGEST.test(binding.source_digest || "") ||
      !DIGEST.test(binding.package_sha256 || "") || typeof requestId !== "string" || !UUID.test(requestId) || !plainTask(task)) return null;
  return { version: binding.version, deployment_id: binding.deployment_id, build_id: binding.build_id,
    source_digest: binding.source_digest, package_sha256: binding.package_sha256, request_id: requestId, task: task.trim() };
}

export function installedUseSuccessBody(metadata, receipt, guide, task, requestId) {
  if (metadata?.success_reporting_available === false || !installedApplicationUse(metadata, receipt, guide, null)) return null;
  return installedUseSuccessRequest({ version: guide.version, deployment_id: receipt.deployment_id,
    build_id: receipt.build_id, source_digest: metadata.source_digest, package_sha256: receipt.package_sha256 }, task, requestId);
}

export function installedUseSuccessBodyMatches(value, expected, historical = false) {
  if (!value || !expected || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).length !== SUCCESS_FIELDS.length || !Object.keys(value).every((key) => SUCCESS_FIELDS.includes(key)) ||
      !installedUseSuccessRequest(value, value.task, value.request_id) || value.task !== value.task.trim()) return false;
  return SUCCESS_FIELDS.every((key) => key === "version" && historical
    ? value.version <= expected.version : value[key] === expected[key]);
}

export function installedUseSuccessNotAdmitted(error) {
  return error?.code === "installed_success_not_admitted" && error.status === 409;
}

/** A saved human report is separate from observed application execution. */
export function installedUseSuccessRecord(value, binding, guideId, body = null) {
  const expected = installedUseSuccessRequest(binding, value?.task, value?.request_id);
  const supplied = value && Object.fromEntries(SUCCESS_FIELDS.map((key) => [key, value[key]]));
  if (!expected || value.scope !== "human-reported-installed-success" || value.outcome !== "worked" ||
      value.application_outcomes_verified !== false || value.guide_id !== guideId ||
      !installedUseSuccessBodyMatches(supplied, expected, true) ||
      body && !installedUseSuccessBodyMatches(supplied, body) || !text(value.recorded_at, 80) ||
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:\d{2})$/.test(value.recorded_at) ||
      !Number.isFinite(Date.parse(value.recorded_at))) return null;
  return { ...supplied, scope: value.scope, guide_id: guideId, outcome: "worked",
    application_outcomes_verified: false, recorded_at: value.recorded_at };
}

export function installedUseSuccessReport(value, metadata, receipt, guide, body = null) {
  if (!installedApplicationUse(metadata, receipt, guide, null)) return null;
  return installedUseSuccessRecord(value, { version: guide.version, deployment_id: receipt.deployment_id,
    build_id: receipt.build_id, source_digest: metadata.source_digest, package_sha256: receipt.package_sha256 }, guide.id, body);
}

export function installedUseRequestId(crypto = globalThis.crypto) {
  try { const id = crypto?.randomUUID?.().toLowerCase(); return typeof id === "string" && UUID.test(id) ? id : null; }
  catch { return null; }
}

/** Retain one immutable admission across ambiguous delivery and tab reloads. */
export function createInstalledUseSuccessStore(storage) {
  const records = new Map();
  function keyFor(scope) { return useDraftKey(scope) ? SUCCESS_PREFIX + scope : null; }
  function valid(scope, body) {
    const owner = JSON.parse(scope);
    return installedUseSuccessBodyMatches(body, installedUseSuccessRequest(body, body?.task, body?.request_id)) &&
      body.deployment_id === owner[1].replace(/^remote:/, "") && body.build_id === owner[2] && body.package_sha256 === owner[3];
  }
  function entry(scope) {
    const key = keyFor(scope);
    if (!key) return { body: null, persistent: false };
    if (!records.has(key)) {
      let body = null, persistent = false;
      try {
        if (storage?.getItem && storage?.setItem && storage?.removeItem) {
          const raw = storage.getItem(key); persistent = true;
          if (typeof raw === "string" && new TextEncoder().encode(raw).byteLength <= MAX_DRAFT_BYTES) {
            const saved = JSON.parse(raw);
            if (saved && Object.keys(saved).length === 2 && saved.scope === scope && valid(scope, saved.body)) body = saved.body;
          }
        }
      } catch { persistent = false; }
      records.set(key, { body, persistent });
    }
    return records.get(key);
  }
  return {
    read(scope) { const body = entry(scope).body; return body ? { ...body } : null; },
    persistent(scope) { return entry(scope).persistent; },
    prepare(scope, body) {
      const key = keyFor(scope);
      if (!key || !valid(scope, body)) return false;
      const prior = entry(scope);
      if (prior.body) return installedUseSuccessBodyMatches(prior.body, body);
      let persistent = false;
      try {
        if (storage?.getItem && storage?.setItem && storage?.removeItem) {
          const raw = JSON.stringify({ scope, body }); storage.setItem(key, raw); persistent = storage.getItem(key) === raw;
        }
      } catch { /* The exact request remains in this open page. */ }
      records.set(key, { body: { ...body }, persistent }); return true;
    },
    clear(scope, body) {
      const key = keyFor(scope), prior = entry(scope);
      if (!key || !installedUseSuccessBodyMatches(prior.body, body)) return false;
      let persistent = false;
      try { if (storage?.removeItem && storage?.getItem) { storage.removeItem(key); persistent = storage.getItem(key) === null; } }
      catch { /* A retained exact request can safely be replayed after reload. */ }
      records.set(key, { body: null, persistent }); return true;
    },
    reject(scope, body) {
      const key = keyFor(scope), prior = entry(scope);
      // Only a host-confirmed refusal can release this exact unsaved request.
      // Persisted requests require verified removal, including for later reload.
      if (!key || !installedUseSuccessBodyMatches(prior.body, body)) return false;
      // Retire only the page-only cache, leaving any storage bytes untouched.
      // This does not establish durable removal or availability after reload.
      if (prior.persistent === false) {
        records.set(key, { body: null, persistent: false }); return true;
      }
      let persistent = false;
      if (storage) {
        try {
          if (!storage.getItem || !storage.removeItem) return false;
          const raw = storage.getItem(key);
          if (raw !== null) {
            if (typeof raw !== "string" || new TextEncoder().encode(raw).byteLength > MAX_DRAFT_BYTES) return false;
            const saved = JSON.parse(raw);
            if (!saved || Object.keys(saved).length !== 2 || saved.scope !== scope ||
                !installedUseSuccessBodyMatches(saved.body, body)) return false;
            storage.removeItem(key);
          }
          if (storage.getItem(key) !== null) return false;
          persistent = true;
        } catch { return false; }
      }
      records.set(key, { body: null, persistent }); return true;
    },
  };
}

export async function copyInstalledUseTask(task, write, currentTask, currentScope, scope) {
  if (!installedUseTaskAllowed(task) || !task.trim()) return null;
  let status = "unavailable";
  try {
    if (typeof write === "function") { await write(task); status = "copied"; }
  } catch { /* The UI offers selection and manual copy without raw browser errors. */ }
  return currentTask() === task && currentScope() === scope ? status : null;
}

// The same production gate surrounds both asynchronous source reads and manual
// feedback admissions. Old mounts and changed sources cannot publish a result.
export function createInstalledUseGate() {
  let serial = 0, selection = null;
  return {
    select(key) {
      if (!selection || selection.key !== key) selection = { key, serial: ++serial };
      return selection;
    },
    current(token, key) { return !!token && token === selection && token.key === key; },
    close(token) { if (selection === token) selection = null; },
    async run(token, load, currentKey, publish, failure) {
      try {
        const value = await load();
        if (this.current(token, currentKey())) publish(value);
      } catch (error) {
        if (this.current(token, currentKey())) failure?.(error);
      }
    },
  };
}
