import { artifactIsCurrent } from "./artifact-current.mjs";

const UUID = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const HASH = /^[0-9a-f]{64}$/;
const PREFIX = "mobkit-selected-job-replay-v1:";
const BODY_FIELDS = ["version", "original_build_id", "original_source_digest", "build_id", "source_digest", "start_request_id"];
const object = (value) => !!value && typeof value === "object" && !Array.isArray(value);
const uuid = (value) => typeof value === "string" && UUID.test(value);
const hash = (value) => typeof value === "string" && HASH.test(value);
const version = (value) => Number.isSafeInteger(value) && value >= 1;
const exact = (value, fields) => object(value) && Object.keys(value).length === fields.length &&
  Object.keys(value).every((key) => fields.includes(key));

/** The host's capsule and source checks own availability; stale trials do not. */
export function applicationTrialReplayOffer(build, state) {
  const offered = build?.trial_replay;
  if (!artifactIsCurrent(build, state) || build.source_verified !== true ||
      !["completed", "failed", "interrupted"].includes(build.status) || build.cleanup_pending === true ||
      !object(offered) || offered.available !== true || !uuid(state?.id) || !version(state.version) ||
      offered.guide_id !== state.id || offered.guide_version !== state.version ||
      !uuid(offered.rehearsal_id) || !uuid(offered.original_build_id) || !hash(offered.original_source_digest) ||
      !uuid(offered.build_id) || offered.build_id !== build.build_id ||
      !hash(offered.source_digest) || offered.source_digest !== build.source_digest || !hash(offered.payload_sha256)) return null;
  return { guide_id: state.id, rehearsal_id: offered.rehearsal_id, payload_sha256: offered.payload_sha256,
    version: state.version, original_build_id: offered.original_build_id,
    original_source_digest: offered.original_source_digest, build_id: offered.build_id, source_digest: offered.source_digest };
}

/** A finished successor is a recorded run, not acceptance of its domain result. */
export function applicationTrialReplayResult(offer, receipt) {
  if (!offer || !receipt || receipt.rehearsal_id === offer.rehearsal_id ||
      receipt.current !== true || receipt.status !== "completed" ||
      receipt.worker_stop_confirmed !== true || receipt.application_stop_confirmed !== true ||
      receipt.trace_complete !== true || receipt.trial_replay?.status !== "completed" ||
      !version(receipt.trial_replay.expected_turns) ||
      receipt.trial_replay.completed_turns !== receipt.trial_replay.expected_turns) return null;
  const action = { guide_id: offer.guide_id, rehearsal_id: offer.rehearsal_id,
    payload_sha256: offer.payload_sha256, body: Object.fromEntries(BODY_FIELDS.map((field) =>
      [field, field === "start_request_id" ? receipt.rehearsal_id : offer[field]])) };
  return trialReplayAcknowledged(action, receipt) ? receipt : null;
}

function checkedAction(value) {
  if (!object(value) || Object.keys(value).some((key) =>
      !["guide_id", "rehearsal_id", "payload_sha256", "body", "start_rejection"].includes(key)) ||
      !uuid(value.guide_id) || !uuid(value.rehearsal_id) || !hash(value.payload_sha256) ||
      !exact(value.body, BODY_FIELDS) || !version(value.body.version) ||
      !uuid(value.body.original_build_id) || !hash(value.body.original_source_digest) ||
      !uuid(value.body.build_id) || !hash(value.body.source_digest) || !uuid(value.body.start_request_id)) return null;
  if (Object.hasOwn(value, "start_rejection") && !trialReplayRejected(value, value.start_rejection)) return null;
  return structuredClone(value);
}

export function trialReplayAcknowledged(action, receipt) {
  if (!checkedActionWithoutRejection(action) || !object(receipt) || receipt.start_request_complete !== true ||
      receipt.start_request_id !== action.body.start_request_id || receipt.rehearsal_id !== action.body.start_request_id ||
      !hash(receipt.start_request_fingerprint) || receipt.guide_id !== action.guide_id ||
      receipt.guide_version !== action.body.version || receipt.mode !== "application" ||
      receipt.application_source?.build_id !== action.body.build_id ||
      receipt.application_source?.source_digest !== action.body.source_digest) return false;
  const replay = receipt.trial_replay;
  return object(replay) && replay.replay_version === 1 && replay.original_rehearsal_id === action.rehearsal_id &&
    replay.original_build_id === action.body.original_build_id && replay.original_source_digest === action.body.original_source_digest &&
    replay.build_id === action.body.build_id && replay.source_digest === action.body.source_digest &&
    replay.payload_sha256 === action.payload_sha256;
}

function checkedActionWithoutRejection(action) {
  if (!object(action)) return null;
  const { start_rejection, ...value } = action;
  return checkedAction(value);
}

/** Only a direct replay response can establish exact durable non-admission. */
export function trialReplayRejected(action, witness) {
  const fields = ["guide_id", "guide_version", "mode", "start_request_id", "start_request_fingerprint",
    "witness_version", "status", "start_request_complete", "replay_version", "original_rehearsal_id",
    "original_build_id", "original_source_digest", "build_id", "source_digest", "payload_sha256"];
  return checkedActionWithoutRejection(action) && exact(witness, fields) &&
    witness.witness_version === 1 && witness.replay_version === 1 && witness.status === "rejected" &&
    witness.start_request_complete === false && witness.mode === "application" &&
    witness.guide_id === action.guide_id && witness.guide_version === action.body.version &&
    witness.start_request_id === action.body.start_request_id && hash(witness.start_request_fingerprint) &&
    witness.original_rehearsal_id === action.rehearsal_id && witness.original_build_id === action.body.original_build_id &&
    witness.original_source_digest === action.body.original_source_digest && witness.build_id === action.body.build_id &&
    witness.source_digest === action.body.source_digest && witness.payload_sha256 === action.payload_sha256
    ? structuredClone(witness) : null;
}

/** Persist the exact action before sending. Browser records contain bindings only. */
export function createTrialReplayStore(storage) {
  const invalid = new Set();
  const key = (guideId) => PREFIX + guideId;
  const pending = (guideId) => {
    if (!uuid(guideId)) return null;
    try {
      const raw = storage?.getItem(key(guideId));
      if (raw == null) return null;
      const value = typeof raw === "string" && raw.length <= 8000 ? checkedAction(JSON.parse(raw)) : null;
      if (!value || value.guide_id !== guideId) { invalid.add(guideId); return null; }
      return value;
    } catch { invalid.add(guideId); return null; }
  };
  const write = (guideId, value) => {
    if (!storage?.getItem || !storage?.setItem) return false;
    try {
      const raw = JSON.stringify(value);
      storage.setItem(key(guideId), raw);
      return storage.getItem(key(guideId)) === raw;
    } catch { return false; }
  };
  const remove = (guideId) => {
    if (!storage?.removeItem || !storage?.getItem) return false;
    try { storage.removeItem(key(guideId)); return storage.getItem(key(guideId)) == null; } catch { return false; }
  };
  return {
    pending,
    blocked(guideId) { pending(guideId); return invalid.has(guideId); },
    begin(offer, actionId) {
      if (!offer || pending(offer.guide_id) || invalid.has(offer.guide_id)) return null;
      const value = checkedAction({ guide_id: offer.guide_id, rehearsal_id: offer.rehearsal_id,
        payload_sha256: offer.payload_sha256, body: Object.fromEntries(BODY_FIELDS.map((field) =>
          [field, field === "start_request_id" ? actionId : offer[field]])) });
      return value && write(value.guide_id, value) ? structuredClone(value) : null;
    },
    acknowledge(guideId, receipt) {
      const value = pending(guideId);
      return !!value && trialReplayAcknowledged(value, receipt) && remove(guideId);
    },
    recordRejected(guideId, witness) {
      const value = pending(guideId), rejection = value && trialReplayRejected(value, witness);
      return !!rejection && write(guideId, { ...value, start_rejection: rejection });
    },
    keepRejected(guideId, actionId) {
      const value = pending(guideId);
      if (!value || value.body.start_request_id !== actionId || !trialReplayRejected(value, value.start_rejection)) return false;
      const historyKey = key(guideId) + ":rejected";
      try {
        const raw = storage.getItem(historyKey);
        if (raw != null && (typeof raw !== "string" || raw.length > 256000)) return false;
        const history = raw == null ? [] : JSON.parse(raw);
        if (!Array.isArray(history) || history.length > 32 || history.some((item) =>
            !checkedAction(item) || item.guide_id !== guideId || !trialReplayRejected(item, item.start_rejection))) return false;
        const next = JSON.stringify([...history.filter((item) => item.body.start_request_id !== actionId), value].slice(-32));
        storage.setItem(historyKey, next);
        return storage.getItem(historyKey) === next && remove(guideId);
      } catch { return false; }
    },
  };
}

/** Ordinary HTTP failures remain ambiguous; only an exact typed witness is usable. */
export async function requestTrialReplay(url, body, fetcher = globalThis.fetch) {
  const response = await fetcher(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  let value;
  try { value = await response.json(); } catch {
    throw new Error("The retry response could not be read. Recover this saved retry before starting another trial.");
  }
  if (response.ok) return value;
  if ([404, 409, 422, 503].includes(response.status) && value?.start_rejection?.replay_version === 1)
    return { start_rejection: value.start_rejection, detail: typeof value.detail === "string" ? value.detail : "The host rejected this retry before admission." };
  throw new Error(typeof value?.detail === "string" ? value.detail : `The retry request failed (${response.status}). Recover its saved action before trying again.`);
}
