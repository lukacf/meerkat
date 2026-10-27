const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const LEASE_MS = 90000, RENEW_MS = 30000, RETRY_MS = 2000;

export function animationIntentError() {
  return Object.assign(new Error('This view animation request could not be confirmed.'), {animationFailureKind: 'intent'});
}

export function isStaleAnimationIntent(response, body) {
  return response?.status === 409 && body?.code === 'stale_visualization_intent';
}

export function checkedAnimationIntentAck(scope, ack) {
  return !!ack && ack.consumer_id === scope.consumer_id && ack.intent_revision === scope.intent_revision &&
    ack.setup_id === scope.setup_id && ack.stream_id === scope.stream_id &&
    typeof ack.intent_token === 'string' && UUID.test(ack.intent_token) && ack.status === 'live';
}

/** Scheduling never changes factual source, correction provenance or native identity. */
export function withAnimationIntent(body, intent) {
  const {consumer_intent: ignored, ...factual} = body;
  const {consumer_id, intent_revision, setup_id, stream_id, intent_token} = intent;
  return {...structuredClone(factual), consumer_intent: {consumer_id, intent_revision, setup_id, stream_id, intent_token}};
}

/** One fresh viewer identity; the host remains the scheduling/admission authority. */
export function createAnimationConsumer({fetcher = (...args) => fetch(...args), uuid = () => crypto.randomUUID(),
  setTimer = (...args) => setTimeout(...args), clearTimer = timer => clearTimeout(timer), now = () => Date.now(), initialRevision = 0} = {}) {
  const consumerId = uuid();
  if (!UUID.test(consumerId) || !Number.isSafeInteger(initialRevision) || initialRevision < 0) throw animationIntentError();
  let revision = initialRevision, current = null;
  const post = async (action, body, keepalive = false) => {
    const response = await fetcher(`/api/visualization-intents/${action}`, {method: 'POST',
      headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body), ...(keepalive ? {keepalive: true} : {})});
    const data = await response.json();
    if (!response.ok) throw Object.assign(animationIntentError(), {stale: isStaleAnimationIntent(response, data)});
    return data;
  };
  return {consumerId, declare({setup_id = null, stream_id}, {onUnavailable = () => {}} = {}) {
    if (revision === Number.MAX_SAFE_INTEGER) throw animationIntentError();
    current?.release();
    const scope = {consumer_id: consumerId, intent_revision: ++revision, setup_id, stream_id};
    let token = null, status = 'declaring', renewal, expiry, retry, resolveReady, rejectReady, releaseSent = false;
    const ready = new Promise((resolve, reject) => {resolveReady = resolve; rejectReady = reject;});
    // Cleanup may reject before the body debounce begins awaiting the ack.
    ready.catch(() => {});
    const owns = () => current === lease && !['released', 'stale', 'unavailable'].includes(status);
    const stopTimers = () => {clearTimer(renewal); clearTimer(expiry); clearTimer(retry);};
    const withdraw = () => {
      if (releaseSent) return;
      releaseSent = true;
      // Withdrawal is independent of aborted publication observation. A
      // pre-ack release deliberately omits the token and still tombstones.
      void post('release', {...scope, ...(token ? {intent_token: token} : {})}, true).catch(() => {});
    };
    const unavailable = stale => {
      if (!owns()) return;
      status = stale ? 'stale' : 'unavailable'; stopTimers(); withdraw(); rejectReady(animationIntentError()); onUnavailable();
    };
    const confirm = ack => {
      if (!owns()) return false;
      if (!checkedAnimationIntentAck(scope, ack) || token && token !== ack.intent_token) throw animationIntentError();
      const hostExpiry = typeof ack.expires_at === 'string' ? Date.parse(ack.expires_at) : NaN;
      const remaining = Number.isFinite(hostExpiry) ? Math.min(LEASE_MS, hostExpiry - now()) : LEASE_MS;
      if (remaining <= 0) {unavailable(true); return false;}
      token = ack.intent_token; status = 'live';
      clearTimer(expiry); expiry = setTimer(() => unavailable(true), remaining);
      clearTimer(renewal); renewal = setTimer(() => exchange('renew'), RENEW_MS);
      resolveReady({...scope, intent_token: token});
      return true;
    };
    const exchange = async (action, attempt = 0) => {
      if (!owns()) return;
      const body = {...scope, ...(token ? {intent_token: token} : {})};
      try {confirm(await post(action, body));}
      catch (error) {
        if (!owns()) return;
        if (error?.stale || attempt) unavailable(!!error?.stale);
        else retry = setTimer(() => exchange(action, 1), RETRY_MS);
      }
    };
    const lease = {scope: {...scope}, ready,
      get active() {return owns();},
      get status() {return status;},
      invalidate() {unavailable(true);},
      async binding() {
        await ready;
        if (!owns() || status !== 'live') throw animationIntentError();
        return {...scope, intent_token: token};
      },
      release() {
        if (status === 'released') return;
        status = 'released'; stopTimers(); rejectReady(animationIntentError());
        withdraw();
      }};
    current = lease;
    // A response that never arrives cannot leave local desire active forever.
    expiry = setTimer(() => unavailable(false), LEASE_MS);
    void exchange('declare');
    return lease;
  }};
}
