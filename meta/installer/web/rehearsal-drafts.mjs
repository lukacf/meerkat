import { artifactIsCurrent } from "./artifact-current.mjs";

const PREFIX = "mobkit-trial-draft-v1:";
const MAX_RECORD_BYTES = 1_500_000;
const HASH = /^[a-f0-9]{64}$/;
const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
const id = (value) => typeof value === "string" && /^[A-Za-z0-9:_-]{1,180}$/.test(value);
const version = (value) => Number.isSafeInteger(value) && value >= 0;
const LIMITS = {
  setup: { sample: 6000, context: 180000, applicationData: 100000, flow: 80 },
  turn: { followUp: 6000, improvement: 6000 },
};

function applicationSource(build, guide) {
  return artifactIsCurrent(build, guide) && build.source_verified === true && id(build.build_id) &&
    typeof build.source_digest === "string" && HASH.test(build.source_digest) &&
    ["completed", "failed", "interrupted"].includes(build.status)
    ? { build_id: build.build_id, source_digest: build.source_digest } : null;
}

/** Browser draft ownership only; these keys never authorize a native request. */
export function rehearsalSetupDraftOwner(guide, build, mode) {
  if (!id(guide?.id) || !version(guide.version) || !["design", "application"].includes(mode)) return null;
  const source = mode === "application" ? applicationSource(build, guide) : {};
  if (!source) return null;
  return { kind: "setup", guide_id: guide.id, guide_version: guide.version, mode, ...source };
}

export function rehearsalSetupDraftCanContinue(previousGuide, previousBuild, guide, build) {
  const previous = rehearsalSetupDraftOwner(previousGuide, previousBuild, "application");
  const next = rehearsalSetupDraftOwner(guide, build, "application");
  return !!previous && !!next && previous.guide_id === next.guide_id &&
    previous.build_id === next.build_id && previous.source_digest === next.source_digest &&
    JSON.stringify([previousGuide.proposal || previousGuide.blueprint, previousGuide.brief]) ===
      JSON.stringify([guide.proposal || guide.blueprint, guide.brief]);
}

export function rehearsalTurnDraftOwner(receipt, build, guide) {
  if (!artifactIsCurrent(receipt, guide) || !id(receipt.rehearsal_id) || !version(guide?.version) ||
      !id(guide.id) || !["design", "application"].includes(receipt.mode)) return null;
  const setup = rehearsalSetupDraftOwner(guide, build, receipt.mode);
  if (!setup || receipt.mode === "application" &&
      (receipt.application_source?.build_id !== setup.build_id ||
       receipt.application_source?.source_digest !== setup.source_digest)) return null;
  return { ...setup, kind: "turn", rehearsal_id: receipt.rehearsal_id,
    original_guide_version: receipt.guide_version };
}

export function rehearsalDraftAdmissionMatches(owner, receipt) {
  return owner?.kind === "setup" && id(receipt?.rehearsal_id) &&
    receipt.guide_id === owner.guide_id && receipt.guide_version === owner.guide_version &&
    receipt.mode === owner.mode && (owner.mode !== "application" ||
      receipt.application_source?.build_id === owner.build_id &&
      receipt.application_source?.source_digest === owner.source_digest);
}

function empty(owner) {
  return Object.fromEntries(Object.keys(LIMITS[owner?.kind] || {}).map((key) => [key, ""]));
}

function boundedDraft(owner, value) {
  const limits = LIMITS[owner?.kind];
  if (!limits || !value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).some((key) => !Object.hasOwn(limits, key))) return null;
  const result = empty(owner);
  for (const [key, limit] of Object.entries(limits)) {
    const item = value[key] ?? "";
    if (typeof item !== "string" || item.length > limit) return null;
    result[key] = item;
  }
  return result;
}

function boundedPending(owner, pending) {
  if (!pending || typeof pending.text !== "string" || !pending.text.trim() || pending.text.length > 6000 || !version(pending.revision)) return null;
  if (owner.kind === "turn" && pending.kind === "continue" && Number.isSafeInteger(pending.turn) &&
      pending.turn > 1 && pending.turn <= 10) return { kind: "continue", text: pending.text, turn: pending.turn, revision: pending.revision };
  return null;
}

function revisions(owner, saved = {}) {
  return Object.fromEntries(Object.keys(empty(owner)).map((field) => [field, version(saved?.[field]) ? saved[field] : 0]));
}

function boundedInitialAction(guideId, value) {
  const owner = value?.owner, body = value?.body, submitted = value?.submitted;
  if (!value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).some((key) => !["owner", "body", "submitted", "preparation_failure", "start_rejection"].includes(key)) ||
      !id(guideId) || owner?.kind !== "setup" || owner.guide_id !== guideId || !version(owner.guide_version) ||
      !["design", "application"].includes(owner.mode) ||
      Object.keys(owner).some((key) => !["kind", "guide_id", "guide_version", "mode", "build_id", "source_digest"].includes(key)) ||
      owner.mode === "application" && (!id(owner.build_id) || !HASH.test(owner.source_digest || "")) ||
      owner.mode === "design" && (owner.build_id != null || owner.source_digest != null) ||
      !body || typeof body !== "object" || Array.isArray(body) ||
      Object.keys(body).some((key) => !["version", "text", "mode", "keep_open", "build_id", "application_fixtures", "fixtures", "flow_id", "params", "start_request_id"].includes(key)) ||
      body.version !== owner.guide_version || body.mode !== owner.mode || !UUID.test(body.start_request_id || "") ||
      typeof body.keep_open !== "boolean" || typeof body.text !== "string" || !body.text.trim() || body.text.length > 6000 ||
      body.text !== body.text.trim() || owner.mode === "application" && body.build_id !== owner.build_id ||
      owner.mode === "design" && (body.build_id != null || body.application_fixtures != null)) return null;
  const draft = boundedDraft(owner, submitted?.draft), savedRevisions = submitted?.revisions;
  if (!draft || draft.sample.trim() !== body.text || !savedRevisions ||
      Object.keys(LIMITS.setup).some((field) => !version(savedRevisions[field])) ||
      Object.keys(savedRevisions).some((field) => !Object.hasOwn(LIMITS.setup, field))) return null;
  try {
    let values = 0;
    const record = { owner, body, submitted: { draft, revisions: savedRevisions } };
    if (Object.hasOwn(value, "preparation_failure")) {
      const failure = preparationFailure(record, value.preparation_failure);
      if (!failure || Object.keys(value.preparation_failure).some((key) => !Object.hasOwn(failure, key))) return null;
      record.preparation_failure = failure;
    }
    if (Object.hasOwn(value, "start_rejection")) {
      const rejection = rehearsalInitialRejected(record, value.start_rejection);
      if (!rejection || record.preparation_failure) return null;
      record.start_rejection = rejection;
    }
    const raw = JSON.stringify(record, (_key, item) => {
      if (++values > 100_000 || typeof item === "number" && !Number.isFinite(item) ||
          ["undefined", "function", "symbol", "bigint"].includes(typeof item)) throw new Error("Invalid saved trial request.");
      return item;
    });
    return new TextEncoder().encode(raw).byteLength <= MAX_RECORD_BYTES ? JSON.parse(raw) : null;
  } catch { return null; }
}

/** Check a direct POST/replay response, after the host compares the full frozen request. */
export function rehearsalInitialResponseMatches(action, receipt) {
  const body = action?.body;
  return !!body && typeof receipt?.start_request_complete === "boolean" && HASH.test(receipt.start_request_fingerprint || "") &&
    receipt.start_request_id === body.start_request_id && receipt.rehearsal_id === body.start_request_id &&
    rehearsalDraftAdmissionMatches(action.owner, receipt) && receipt.text === body.text;
}

export function rehearsalInitialAcknowledged(action, receipt) {
  return rehearsalInitialResponseMatches(action, receipt) && receipt.start_request_complete === true;
}

/** Only a direct full-request POST response may establish this host rejection. */
export function rehearsalInitialRejected(action, witness) {
  const fields = ["guide_id", "guide_version", "mode", "start_request_id", "start_request_fingerprint",
    "witness_version", "status", "start_request_complete"];
  return action?.owner && action?.body && witness && typeof witness === "object" && !Array.isArray(witness) &&
    Object.keys(witness).length === fields.length && Object.keys(witness).every((key) => fields.includes(key)) &&
    witness.witness_version === 1 && witness.status === "rejected" && witness.start_request_complete === false &&
    witness.guide_id === action.owner.guide_id && witness.guide_version === action.owner.guide_version &&
    witness.mode === action.owner.mode && witness.start_request_id === action.body.start_request_id &&
    HASH.test(witness.start_request_fingerprint || "") ? structuredClone(witness) : null;
}

function preparationFailure(action, receipt) {
  if (!rehearsalInitialResponseMatches(action, receipt) || receipt.start_request_complete !== false ||
      receipt.status !== "unavailable") return null;
  return { rehearsal_id: receipt.rehearsal_id, start_request_id: receipt.start_request_id,
    start_request_fingerprint: receipt.start_request_fingerprint, start_request_complete: false,
    status: "unavailable", guide_id: receipt.guide_id, guide_version: receipt.guide_version,
    mode: receipt.mode, text: receipt.text, ...(receipt.mode === "application" ? {
      application_source: { build_id: receipt.application_source.build_id, source_digest: receipt.application_source.source_digest },
    } : {}) };
}

function preparationHistory(guideId, value) {
  if (value?.status === "rejected") {
    const action = { owner: { guide_id: guideId, guide_version: value.guide_version, mode: value.mode },
      body: { start_request_id: value.start_request_id } };
    return version(value.guide_version) && UUID.test(value.start_request_id || "") &&
      ["design", "application"].includes(value.mode) ? rehearsalInitialRejected(action, value) : null;
  }
  if (!value || typeof value !== "object" || Array.isArray(value) || value.guide_id !== guideId ||
      !version(value.guide_version) || !UUID.test(value.start_request_id || "") ||
      !HASH.test(value.start_request_fingerprint || "") || value.status !== "unavailable" ||
      value.start_request_complete !== false || !["design", "application"].includes(value.mode)) return null;
  const fields = ["guide_id", "guide_version", "start_request_id", "start_request_fingerprint", "status", "start_request_complete", "mode"];
  if (value.mode === "application") {
    fields.push("application_source");
    if (!id(value.application_source?.build_id) || !HASH.test(value.application_source?.source_digest || "") ||
        Object.keys(value.application_source).some((key) => !["build_id", "source_digest"].includes(key))) return null;
  }
  if (Object.keys(value).some((key) => !fields.includes(key))) return null;
  return Object.fromEntries(fields.map((key) => [key, structuredClone(value[key])]));
}

/** Same-tab storage plus an in-memory fallback when browser storage is denied. */
export function createRehearsalDraftStore(storage) {
  const drafts = new Map();
  const initialActions = new Map(), invalidInitial = new Set();
  const initialKey = (guideId) => `${PREFIX}initial:${guideId}`;
  const historyKey = (guideId) => `${PREFIX}initial-history:${guideId}`;
  const history = (guideId) => {
    if (!id(guideId)) return null;
    try {
      const raw = storage?.getItem(historyKey(guideId));
      if (raw == null) return [];
      if (typeof raw !== "string" || new TextEncoder().encode(raw).byteLength > 32_000) return null;
      const saved = JSON.parse(raw);
      if (!Array.isArray(saved) || saved.length > 32) return null;
      const checked = saved.map((item) => preparationHistory(guideId, item));
      return checked.every(Boolean) && new Set(checked.map((item) => item.start_request_id)).size === checked.length ? checked : null;
    } catch { return null; }
  };
  const saveVerified = (key, value) => {
    if (!storage?.getItem || !storage?.setItem) return false;
    storage.setItem(key, value);
    return storage.getItem(key) === value;
  };
  const initial = (guideId) => {
    if (!id(guideId)) return null;
    if (!initialActions.has(guideId)) {
      let value = null;
      try {
        const raw = storage?.getItem(initialKey(guideId));
        if (raw != null) {
          if (typeof raw === "string" && new TextEncoder().encode(raw).byteLength <= MAX_RECORD_BYTES)
            value = boundedInitialAction(guideId, JSON.parse(raw));
          if (!value) invalidInitial.add(guideId);
        }
      } catch { invalidInitial.add(guideId); }
      initialActions.set(guideId, value);
    }
    const value = initialActions.get(guideId);
    return value ? structuredClone(value) : null;
  };
  const keyFor = (owner) => owner ? PREFIX + JSON.stringify(owner) : null;
  const entry = (owner) => {
    const key = keyFor(owner);
    if (!key) return { draft: empty(owner), pending: null, revisions: revisions(owner) };
    if (!drafts.has(key)) {
      let value = null;
      try {
        const raw = storage?.getItem(key);
        if (typeof raw === "string" && new TextEncoder().encode(raw).byteLength <= MAX_RECORD_BYTES) {
          const saved = JSON.parse(raw);
          if (saved && !Object.keys(saved).some((field) => !["owner", "draft", "pending", "revisions"].includes(field)) &&
              JSON.stringify(saved.owner) === JSON.stringify(owner)) {
            const draft = boundedDraft(owner, saved.draft);
            if (draft) value = { draft, pending: boundedPending(owner, saved.pending), revisions: revisions(owner, saved.revisions) };
          }
        }
      } catch {}
      drafts.set(key, value || { draft: empty(owner), pending: null, revisions: revisions(owner) });
    }
    return drafts.get(key);
  };
  const read = (owner) => ({ ...entry(owner).draft });
  const write = (owner, value, pending = entry(owner).pending) => {
    const key = keyFor(owner), draft = boundedDraft(owner, value);
    if (!key || !draft) return false;
    const previous = entry(owner), nextRevisions = revisions(owner, previous.revisions);
    for (const field of Object.keys(draft)) if (draft[field] !== previous.draft[field])
      nextRevisions[field] = Math.min(Number.MAX_SAFE_INTEGER, nextRevisions[field] + 1);
    drafts.set(key, { draft, pending, revisions: nextRevisions });
    try {
      if (Object.values(draft).some(Boolean) || pending) {
        const raw = JSON.stringify({ owner, draft, revisions: nextRevisions, ...(pending ? { pending } : {}) });
        if (new TextEncoder().encode(raw).byteLength <= MAX_RECORD_BYTES) storage?.setItem(key, raw);
      } else storage?.removeItem(key);
    } catch {}
    return true;
  };
  return {
    read,
    initial,
    // Carry only unsent setup fields in this mounted store. Persisted owners and
    // saved request identities stay unchanged until an ordinary edit or Try.
    retainSetup(previousOwner, owner) {
      if (previousOwner?.kind !== "setup" || owner?.kind !== "setup" ||
          previousOwner.mode !== "application" || owner.mode !== "application" ||
          previousOwner.guide_id !== owner.guide_id || previousOwner.guide_version === owner.guide_version ||
          previousOwner.build_id !== owner.build_id || previousOwner.source_digest !== owner.source_digest ||
          initial(owner.guide_id) || invalidInitial.has(owner.guide_id)) return false;
      const previous = entry(previousOwner), next = entry(owner);
      if (previous.pending || next.pending || !Object.values(previous.draft).some(Boolean) ||
          Object.values(next.draft).some(Boolean)) return false;
      drafts.set(keyFor(owner), { draft: { ...previous.draft }, pending: null,
        revisions: { ...previous.revisions } });
      return true;
    },
    initialHistory(guideId) { return history(guideId) || []; },
    initialBlocked(guideId) { initial(guideId); return invalidInitial.has(guideId); },
    beginInitial(owner, body) {
      if (!owner || initial(owner.guide_id) || invalidInitial.has(owner.guide_id)) return null;
      const current = entry(owner);
      const action = boundedInitialAction(owner.guide_id, { owner, body,
        submitted: { draft: current.draft, revisions: current.revisions } });
      if (!action) return null;
      try {
        const raw = JSON.stringify(action);
        if (!saveVerified(initialKey(owner.guide_id), raw)) return null;
        initialActions.set(owner.guide_id, action);
        return structuredClone(action);
      } catch { return null; }
    },
    // Only an exact direct POST response can establish this preparation witness.
    recordPreparationFailure(guideId, receipt) {
      const action = initial(guideId), failure = preparationFailure(action, receipt);
      if (!failure) return false;
      const saved = boundedInitialAction(guideId, { ...action, preparation_failure: failure });
      if (!saved) return false;
      try {
        if (!saveVerified(initialKey(guideId), JSON.stringify(saved))) return false;
        initialActions.set(guideId, saved);
        return true;
      } catch { return false; }
    },
    recordRejectedStart(guideId, witness) {
      const action = initial(guideId), rejection = rehearsalInitialRejected(action, witness);
      if (!rejection) return false;
      const saved = boundedInitialAction(guideId, { ...action, start_rejection: rejection });
      if (!saved) return false;
      try {
        if (!saveVerified(initialKey(guideId), JSON.stringify(saved))) return false;
        initialActions.set(guideId, saved);
        return true;
      } catch { return false; }
    },
    // Explicitly archive a failed preparation; this never submits or certifies cleanup.
    archivePreparationFailure(guideId, actionId) {
      const action = initial(guideId), previous = history(guideId);
      if (!action || action.body.start_request_id !== actionId || !(action.preparation_failure || action.start_rejection) || !previous) return false;
      const { text: _text, rehearsal_id: _id, ...metadata } = action.preparation_failure || action.start_rejection;
      const entry = preparationHistory(guideId, metadata);
      if (!entry) return false;
      const existing = previous.find((item) => item.start_request_id === actionId);
      if (existing && JSON.stringify(existing) !== JSON.stringify(entry) || !existing && previous.length >= 32) return false;
      const next = existing ? previous : [...previous, entry];
      try {
        const archiveKey = `${historyKey(guideId)}:${actionId}`, raw = JSON.stringify(action);
        const archived = storage?.getItem(archiveKey);
        if (archived != null && archived !== raw) return false;
        if (!saveVerified(archiveKey, raw) || !saveVerified(historyKey(guideId), JSON.stringify(next))) return false;
        storage.removeItem(initialKey(guideId));
        if (storage.getItem(initialKey(guideId)) != null) return false;
        initialActions.set(guideId, null); invalidInitial.delete(guideId);
        return true;
      } catch { return false; }
    },
    // Latest-status GETs are not full-request acknowledgements. Only the
    // explicitly submitted/replayed POST response may call this method.
    acknowledgeInitial(guideId, receipt) {
      const action = initial(guideId);
      if (!rehearsalInitialAcknowledged(action, receipt)) return false;
      const value = entry(action.owner), submitted = action.submitted;
      const unchanged = Object.keys(LIMITS.setup).every((field) => value.draft[field] === submitted.draft[field] &&
        value.revisions[field] === submitted.revisions[field]);
      write(action.owner, unchanged ? { ...value.draft, sample: "" } : value.draft);
      initialActions.set(guideId, null); invalidInitial.delete(guideId);
      try { storage?.removeItem(initialKey(guideId)); } catch {}
      return true;
    },
    capture(owner, field) {
      const value = entry(owner);
      return Object.hasOwn(value.draft, field) ? { text: value.draft[field], revision: value.revisions[field] } : null;
    },
    update(owner, field, value) {
      if (!Object.hasOwn(LIMITS[owner?.kind] || {}, field)) return false;
      return write(owner, { ...read(owner), [field]: value });
    },
    clearSubmitted(owner, field, submitted) {
      const value = entry(owner), current = { ...value.draft };
      const text = typeof submitted === "string" ? submitted : submitted?.text;
      if (typeof text !== "string" || !Object.hasOwn(current, field)) return false;
      const same = current[field] === text && (typeof submitted === "string" || value.revisions[field] === submitted.revision);
      write(owner, same ? { ...current, [field]: "" } : current, null);
      return same;
    },
    markSubmitted(owner, pending) {
      const bounded = boundedPending(owner || {}, { ...pending, revision: entry(owner).revisions.followUp });
      return bounded ? write(owner, read(owner), bounded) : false;
    },
    acknowledge(owner, receipt) {
      const pending = entry(owner).pending;
      if (!pending) return false;
      let field;
      if (pending.kind === "continue" && owner.rehearsal_id === receipt?.rehearsal_id &&
          owner.guide_id === receipt.guide_id && owner.original_guide_version === receipt.guide_version &&
          owner.mode === receipt.mode && (owner.mode !== "application" ||
            owner.build_id === receipt.application_source?.build_id && owner.source_digest === receipt.application_source?.source_digest) &&
          Array.isArray(receipt.turns) && receipt.turns.some((turn) => turn?.turn === pending.turn &&
            id(turn.command_id) && turn.command_id !== "initial" && turn.text === pending.text.trim())) field = "followUp";
      if (!field) return false;
      const value = entry(owner), current = { ...value.draft };
      write(owner, current[field] === pending.text && value.revisions[field] === pending.revision
        ? { ...current, [field]: "" } : current, null);
      return true;
    },
    preferredMode(guide, build) {
      if (!id(guide?.id) || !version(guide.version)) return null;
      try {
        const raw = storage?.getItem(`${PREFIX}mode:${guide.id}:${guide.version}`);
        if (typeof raw !== "string" || raw.length > 1000) return null;
        const saved = JSON.parse(raw), owner = rehearsalSetupDraftOwner(guide, build, saved?.mode);
        return owner && JSON.stringify(saved) === JSON.stringify(owner) ? saved.mode : null;
      } catch { return null; }
    },
    selectMode(guide, build, mode) {
      const owner = rehearsalSetupDraftOwner(guide, build, mode);
      if (!owner) return;
      try { storage?.setItem(`${PREFIX}mode:${guide.id}:${guide.version}`, JSON.stringify(owner)); } catch {}
    },
  };
}

export function rehearsalSessionStorage() {
  try { return globalThis.sessionStorage; } catch { return null; }
}
