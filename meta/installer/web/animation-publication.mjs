export function hasPublishedAnimation(receipt) {
  return (receipt?.status === 'published' || receipt?.status === 'completed')
    && typeof receipt.artifact?.code === 'string' && receipt.artifact.code.length > 0;
}

export function shouldObserveAnimationTail(receipt) {
  return hasPublishedAnimation(receipt) && receipt.status === 'published'
    && receipt.native_run_status === 'unknown'
    && !['timed_out', 'interrupted', 'unavailable'].includes(receipt.observation_status);
}

export function pendingAnimationNotice(receipt) {
  return receipt?.status === 'observing' && receipt.observation_status === 'reconnecting'
    ? 'Still composing. Reconnecting to the same animation request.' : '';
}

export function animationObservationUnavailable(receipt) {
  return ['queued', 'running', 'observing'].includes(receipt?.status)
    && receipt.observation_status === 'unavailable' && !hasPublishedAnimation(receipt);
}

export function shouldReattachAnimationPoll(status, attempts) {
  // A missing legacy receipt can reattach through the saved source/admission
  // identity. One exact-body reattachment handles that case without turning
  // unrelated failures into repeated generation requests.
  return status === 404 && attempts === 0;
}

export function animationRequestMatches(request, current) {
  return !!request && !!current && typeof request.streamId === 'string'
    && typeof request.proposalKey === 'string' && Number.isSafeInteger(request.attempt)
    && request.attempt >= 0 && request.streamId === current.streamId
    && request.proposalKey === current.proposalKey && request.attempt === current.attempt;
}

const ANIMATION_FAILURES = Object.freeze({
  failed: 'The visualization agent could not finish this scene.',
  interrupted: 'The animation update was interrupted.',
  unavailable: 'The animation update could not be checked. Its native status is unknown.',
  execution: 'The custom animation could not run.',
  intent: 'This view\'s animation request could not be confirmed.',
});

export function animationFailure(request, kind) {
  return {request: {...request}, kind: typeof kind === 'string' && Object.hasOwn(ANIMATION_FAILURES, kind) ? kind : 'unavailable'};
}

/** Presentation only. An observation problem does not establish native failure. */
export function animationGenerationStatus({request, generation, failure, notice = '', authored = false,
  updating = false, currentScene = false, refining = false, recovered = false}) {
  if (generation === 'fallback' && animationRequestMatches(failure?.request, request)
      && typeof failure.kind === 'string' && Object.hasOwn(ANIMATION_FAILURES, failure.kind)) {
    return {message: ANIMATION_FAILURES[failure.kind], detail: authored
      ? updating ? 'Your previous animation is still shown. It does not include the latest proposal changes.'
        : currentScene ? 'Your current animation is still shown.'
          : 'Your last working animation is still shown. No replacement was applied.'
      : 'The automatic network preview is shown.', failure: true, canRetry: true,
      retryLabel: ['unavailable', 'intent'].includes(failure.kind) ? 'Check again' : 'Retry agent'};
  }
  return {message: generation === 'working'
    ? notice ? `${notice}${authored ? ' Your previous scene remains visible.' : ' Automatic preview is provisional.'}`
      : authored ? refining ? 'Refining your animation...' : 'Updating the animation as your spec evolves...'
        : 'Composing your first animation. Automatic preview is provisional.'
    : updating ? 'Previous animation - latest changes still need a scene'
      : authored ? 'Scene composed by your visualization agent' : 'Automatic network preview',
    detail: recovered && authored ? 'Previous proposal restored from this browser session. Waiting for the current scene.' : '', failure: false, canRetry: false};
}

export function checkedAnimationFeedback(feedback, {required = false} = {}) {
  if (typeof feedback !== 'string') throw new Error('Describe what you want to change in the animation.');
  if (feedback.length > 1000) throw new Error('Keep animation feedback to 1000 characters or fewer.');
  const value = feedback.replace(/[\u0000-\u001f\u007f-\u009f]/g, ' ').replace(/\s+/g, ' ').trim();
  if (required && !value) throw new Error('Describe what you want to change in the animation.');
  return value;
}

export function animationRequestBody(blueprint, model, feedback = '', streamId, setupId) {
  return {blueprint, model, feedback: checkedAnimationFeedback(feedback), stream_id: streamId,
    ...(setupId ? {setup_id: setupId} : {})};
}

const digest = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);

/** A matching HTTP receipt proves host request acceptance, not native admission. */
export function animationAdmissionMatches(receipt, binding) {
  return !!binding && digest(receipt?.id) && digest(receipt.request_fingerprint) &&
    digest(receipt.blueprint_hash) && digest(receipt.model_hash) && digest(receipt.native_request_key) &&
    Number.isSafeInteger(receipt.source_generation) && receipt.source_generation > 0 &&
    (receipt.setup_id ?? null) === (binding.setup_id ?? null) && receipt.stream_id === binding.stream_id &&
    receipt.request_fingerprint === binding.request_fingerprint && receipt.blueprint_hash === binding.blueprint_hash &&
    receipt.model_hash === binding.model_hash && ['queued', 'running', 'observing', 'published', 'completed'].includes(receipt.status);
}

export function acknowledgeAnimationRefinement(drafts, selection, submitted, receipt, binding, request, current) {
  if (!animationRequestMatches(request, current) || !animationAdmissionMatches(receipt, binding)) return null;
  return {accepted: true, cleared: drafts.clearSubmitted(selection, submitted)};
}

const FEEDBACK_PREFIX = 'mobkit-animation-feedback-v1:';
const feedbackScope = (streamId, proposalHash) => typeof streamId === 'string' && streamId.length > 0 &&
  streamId.length <= 256 && typeof proposalHash === 'string' && /^[a-f0-9]{64}$/.test(proposalHash);

function checkedPreferenceCorrection(value, streamId) {
  const fields = ['feedback', 'expected_parent', 'review_id'];
  const parentFields = ['operation_id', 'code_hash', 'blueprint_hash', 'model_hash', 'setup_id', 'stream_id'];
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).length !== fields.length
      || !fields.every(name => Object.hasOwn(value, name)) || !digest(value.review_id)) return null;
  const parent = value.expected_parent;
  if (!parent || typeof parent !== 'object' || Array.isArray(parent) || Object.keys(parent).length !== parentFields.length
      || !parentFields.every(name => Object.hasOwn(parent, name)) || parent.stream_id !== streamId
      || !['operation_id', 'code_hash', 'blueprint_hash', 'model_hash'].every(name => digest(parent[name]))
      || parent.setup_id !== null && (typeof parent.setup_id !== 'string' || !parent.setup_id || parent.setup_id.length > 256)) return null;
  try {
    if (checkedAnimationFeedback(value.feedback, {required: true}) !== value.feedback) return null;
    return structuredClone(value);
  } catch { return null; }
}

/** Saved request data is a replay hint only; the host still admits its exact source and ownership. */
export function readAnimationPreferenceRequest(storage, streamId, proposalHash) {
  if (!feedbackScope(streamId, proposalHash)) return null;
  try {
    const raw = storage.getItem(FEEDBACK_PREFIX + streamId);
    if (typeof raw !== 'string' || raw.length > 5000) return null;
    const saved = JSON.parse(raw);
    if (saved?.proposal_hash !== proposalHash) return null;
    const feedback = checkedAnimationFeedback(saved.feedback);
    const correction = Object.hasOwn(saved, 'correction') ? checkedPreferenceCorrection(saved.correction, streamId) : null;
    if (Object.hasOwn(saved, 'correction') && !correction) return null;
    return {feedback, correction};
  } catch { return null; }
}

export function readAnimationPreference(storage, streamId, proposalHash) {
  return readAnimationPreferenceRequest(storage, streamId, proposalHash)?.feedback || '';
}

export function saveAnimationPreference(storage, streamId, proposalHash, feedback) {
  if (!feedbackScope(streamId, proposalHash)) return false;
  try {
    storage.setItem(FEEDBACK_PREFIX + streamId, JSON.stringify({proposal_hash: proposalHash,
      feedback: checkedAnimationFeedback(feedback, {required: true})}));
    return true;
  } catch { return false; }
}

export function saveAnimationCorrectionPreference(storage, streamId, proposalHash, parentFeedback, correction) {
  if (!feedbackScope(streamId, proposalHash)) return false;
  const checked = checkedPreferenceCorrection(correction, streamId);
  if (!checked) return false;
  try {
    storage.setItem(FEEDBACK_PREFIX + streamId, JSON.stringify({proposal_hash: proposalHash,
      feedback: checkedAnimationFeedback(parentFeedback), correction: checked}));
    return true;
  } catch { return false; }
}

export function animationCorrectionParentMatches(receipt, binding, expected) {
  return animationAdmissionMatches(receipt, binding) && hasPublishedAnimation(receipt)
    && receipt.id === expected?.operation_id && receipt.artifact.code_hash === expected.code_hash
    && ['blueprint_hash', 'model_hash', 'stream_id', 'setup_id'].every(name => (receipt[name] ?? null) === (expected[name] ?? null));
}
