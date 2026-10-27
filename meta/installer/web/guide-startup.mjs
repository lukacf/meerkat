// Only the host's typed latch authorizes a deliberate setup-team retry.
const STARTUP_PHASES = new Set(['gateway_preparation', 'storage_preparation', 'native_bootstrap', 'identity_warmup', 'readiness_check']);
const STARTUP_ERRORS = new Set(['startup_timeout', 'native_refusal', 'storage_preparation', 'startup_failure']);

export function guideStartupObservation(value) {
  if (!value || typeof value.status !== 'string' || typeof value.startup_retry_required !== 'boolean') return null;
  return { status: value.status, retryRequired: value.status === 'failed' && value.startup_retry_required === true,
    available: typeof value.available === 'boolean' ? value.available : null,
    ...(STARTUP_PHASES.has(value.startup_phase) ? { phase: value.startup_phase } : {}),
    ...(STARTUP_ERRORS.has(value.startup_error_kind) ? { errorKind: value.startup_error_kind } : {}),
    ...(value.startup_error_kind === 'native_refusal' && Number.isSafeInteger(value.startup_error_code)
      ? { errorCode: value.startup_error_code } : {}) };
}

// Explain only the host's closed categories. Exception text and provider keys
// never become presentation, and a phase is not a completion percentage.
export function guideStartupDetail(observation) {
  if (!observation || !['starting', 'failed'].includes(observation.status)) return null;
  const phase = STARTUP_PHASES.has(observation.phase) ? observation.phase : null;
  const errorKind = STARTUP_ERRORS.has(observation.errorKind) ? observation.errorKind : null;
  const copy = observation.status === 'failed' ? {
    startup_timeout: ['Your guide took too long to start', 'The local agent runtime did not finish starting in time. Check guide status before trying again.'],
    native_refusal: ['The agent runtime could not start', 'The local runtime declined startup. Check guide status before changing settings or trying again.'],
    storage_preparation: ['Your guide workspace needs attention', 'Forge could not prepare its saved workspace on the computer running Forge. Check that computer\'s storage and access permissions, then check guide status.'],
    startup_failure: ['Your guide could not start', 'Check guide status to confirm what happened before changing settings or trying again.'],
  }[errorKind] || ['Your guide could not start', 'Check guide status to confirm what happened before changing settings or trying again.'] : {
    gateway_preparation: ['Preparing the agent runtime', 'Forge is preparing the local software that runs your guide. The first start may need a download.'],
    storage_preparation: ['Preparing your guide workspace', 'Forge is preparing the saved workspace on the computer running Forge.'],
    native_bootstrap: ['Starting your setup team', 'The local agent runtime is starting.'],
    identity_warmup: ['Preparing your agents', 'Forge is preparing the agents that help design and build your team.'],
    readiness_check: ['Checking that your guide is ready', 'Forge is checking that the setup team can accept your next message.'],
  }[phase] || ['Starting your setup guide', 'Waiting for the local agent runtime to report its status.'];
  return { title: copy[0], detail: copy[1], phase, errorKind };
}

export function guideStartupPresentation(state, snapshot) {
  if (!state?.id || snapshot?.guideId !== state.id) return null;
  if (snapshot.retrying) return 'retrying';
  if (snapshot.startup?.retryRequired === true) return 'failed';
  if (snapshot.startup?.status === 'starting') return 'starting';
  if (snapshot.startup?.available === false) return 'unavailable';
  return snapshot.error ? 'reconnecting' : null;
}

// Each controller belongs to one effect lifetime, even when a guide is revisited.
export function createGuideStartupRecovery({ guideId, currentGuideId, request, onUpdate, onRecovered }) {
  let disposed = false, epoch = 0, retrying = false, startup = null, retryOwner = null, checkingOwner = null;
  const current = (token = epoch) => !disposed && token === epoch && currentGuideId() === guideId;
  const publish = (patch, token) => { if (current(token)) onUpdate(patch); };
  async function observeFailure(token = epoch) {
    if (!current(token)) return null;
    let observed = null;
    try { observed = guideStartupObservation(await request('/api/architect/status', { timeoutMs: 30000 })); } catch {}
    if (!current(token)) return null;
    startup = observed;
    publish({ startup }, token);
    return observed;
  }
  async function retry() {
    if (!current() || retryOwner || checkingOwner || !startup?.retryRequired) return false;
    retrying = true;
    const token = ++epoch;
    const owner = retryOwner = { token };
    startup = null; // A lost POST response cannot reuse the preceding failed observation.
    publish({ startup: null, retrying: true, retryPending: true, retryError: '' }, token);
    let observed = null, failed = false;
    try {
      observed = guideStartupObservation(await request('/api/architect/start', { method: 'POST' }));
      if (!current(token)) return false;
      startup = observed;
      publish({ startup }, token);
    } catch {
      if (!current(token)) return false;
      failed = true;
      observed = await observeFailure(token);
    } finally {
      if (retryOwner === owner) retryOwner = null;
      if (current(token)) {
        retrying = false;
        publish({ retrying: false, retryPending: false, retryError: failed ? 'The retry response was interrupted. Current setup-team status is shown below.' : '' }, token);
        onRecovered?.();
      } else if (current()) {
        // The original call is still joined, but a newer read owns its presentation.
        if (!checkingOwner) retrying = false;
        publish({ retryPending: false, ...(!checkingOwner ? { retrying: false } : {}) }, epoch);
      }
    }
    return current(token) && observed?.retryRequired === false;
  }
  async function checkStatus() {
    if (!current() || checkingOwner) return false;
    const owner = checkingOwner = { token: ++epoch };
    publish({ checkingStatus: true, retryError: '' }, owner.token);
    try {
      const observed = guideStartupObservation(await request('/api/architect/status', { timeoutMs: 30000 }));
      if (!current(owner.token) || checkingOwner !== owner) return false;
      if (!observed) throw new Error('Setup-team status was not confirmed.');
      startup = observed;
      retrying = false;
      publish({ startup, retrying: false, retryPending: !!retryOwner, checkingStatus: false, retryError: '', error: '' }, owner.token);
      checkingOwner = null;
      onRecovered?.();
      return observed.status === 'ready';
    } catch {
      if (current(owner.token) && checkingOwner === owner) {
        retrying = !!retryOwner;
        publish({ retrying, checkingStatus: false,
          retryError: 'Current setup-team status could not be confirmed. Check again; the original Start request will not be repeated.' }, owner.token);
        checkingOwner = null;
        if (!retryOwner) onRecovered?.();
      }
      return false;
    } finally {
      if (checkingOwner === owner) checkingOwner = null;
    }
  }
  return { observeFailure, retry, checkStatus, current, generation: () => epoch, isRetrying: () => retrying || !!checkingOwner,
    timelineRecovered() { startup = null; },
    dispose() { disposed = true; epoch += 1; } };
}
