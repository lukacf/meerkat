// A presentation-only sibling view for the existing protected native review.
// It owns no ConsoleConnect port, SDK, native session, grant or stored policy.
const PREFIX = "/console/forge/controller-review";
const STATUS = new Set(["starting", "awaiting_prompt", "prompt", "prompt_expired",
  "answer_submitted", "answer_forwarded", "outcome_unknown", "review_completed",
  "denied", "unavailable", "pending_reconciliation"]);
const POLLING = new Set(["starting", "awaiting_prompt", "prompt",
  "answer_submitted", "answer_forwarded"]);
const mounts = new WeakMap();
const text = value => String(value).replace(
  /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u200b-\u200f\u2028-\u202e\u2060-\u206f\ufeff]/gu,
  char => "[U+" + char.codePointAt(0).toString(16).toUpperCase().padStart(4, "0") + "]");

function exact(value, fields) {
  return value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).length === fields.length
    && fields.every(field => Object.hasOwn(value, field));
}
function expiry(value) {
  if (value === null) return null;
  const parts = typeof value === "string" && /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[Zz]|[+-](\d{2}):(\d{2}))$/.exec(value);
  if (!parts) return NaN;
  const [year, month, day, hour, minute, second, zoneHour = 0, zoneMinute = 0] = parts.slice(1).map(part => part === undefined ? 0 : Number(part));
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (year < 1 || month < 1 || month > 12 || day < 1 || day > days[month - 1]
      || hour > 23 || minute > 59 || second > 59 || zoneHour > 23 || zoneMinute > 59) return NaN;
  return Date.parse(value.replace("t", "T").replace("z", "Z"));
}
function jsonValue(value, ancestors = new Set()) {
  if (value === null || ["string", "boolean"].includes(typeof value)) return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (typeof value !== "object" || ancestors.has(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  if (!Array.isArray(value) && prototype !== Object.prototype && prototype !== null) return false;
  if (Object.getOwnPropertySymbols(value).length) return false;
  ancestors.add(value);
  const entries = Array.isArray(value)
    ? Array.from({ length: value.length }, (_, index) => Object.getOwnPropertyDescriptor(value, String(index)))
    : Object.keys(value).map(key => Object.getOwnPropertyDescriptor(value, key));
  const valid = entries.every(entry => entry && Object.hasOwn(entry, "value") && jsonValue(entry.value, ancestors));
  ancestors.delete(value);
  return valid;
}
function promptValue(value) {
  if (!exact(value, ["kind", "approval_id", "summary", "body", "allowed_decisions", "expires_at"])
      || value.kind !== "approval" || typeof value.approval_id !== "string" || !value.approval_id
      || typeof value.summary !== "string" || !value.summary
      || !Array.isArray(value.allowed_decisions) || !value.allowed_decisions.length
      || new Set(value.allowed_decisions).size !== value.allowed_decisions.length
      || value.allowed_decisions.some(choice => !["approve", "deny"].includes(choice))
      || (value.expires_at !== null && !Number.isFinite(expiry(value.expires_at)))) return null;
  try {
    if (!jsonValue(value.body)) return null;
    return { ...value, allowed_decisions: [...value.allowed_decisions] };
  } catch { return null; }
}
function stateValue(value) {
  if (!STATUS.has(value?.status) || !exact(value,
      value.status === "prompt" ? ["status", "prompt"] : ["status"])) return null;
  if (value.status !== "prompt") return { status: value.status };
  const prompt = promptValue(value.prompt);
  return prompt ? { status: "prompt", prompt } : null;
}

// These display helpers retain the raw body value from the protected JSON
// response. JSON.parse validates the document; numeric body tokens are not
// regenerated from JavaScript numbers, which could round native integers.
function stringEnd(source, start) {
  let escaped = false;
  for (let index = start + 1; index < source.length; index++) {
    if (!escaped && source[index] === '"') return index + 1;
    if (!escaped && source[index] === "\\") escaped = true;
    else escaped = false;
  }
  throw new Error("Incomplete JSON string");
}
function valueEnd(source, start) {
  if (source[start] === '"') return stringEnd(source, start);
  if (source[start] !== "{" && source[start] !== "[") {
    let end = start;
    while (end < source.length && !/[,}\]]/.test(source[end])) end++;
    return end;
  }
  let depth = 0;
  for (let index = start; index < source.length; index++) {
    if (source[index] === '"') { index = stringEnd(source, index) - 1; continue; }
    if (source[index] === "{" || source[index] === "[") depth++;
    if (source[index] === "}" || source[index] === "]") {
      depth--;
      if (!depth) return index + 1;
    }
  }
  throw new Error("Incomplete JSON value");
}
function objectValues(source) {
  const values = new Map();
  let index = 0;
  const space = () => { while (/\s/.test(source[index] ?? "") && index < source.length) index++; };
  space();
  if (source[index++] !== "{") throw new Error("Expected JSON object");
  space();
  while (source[index] !== "}") {
    if (source[index] !== '"') throw new Error("Expected JSON field");
    const end = stringEnd(source, index), key = JSON.parse(source.slice(index, end));
    if (values.has(key)) throw new Error("Repeated JSON field");
    index = end; space();
    if (source[index++] !== ":") throw new Error("Expected JSON field value");
    space();
    const start = index;
    index = valueEnd(source, start);
    values.set(key, source.slice(start, index).trim());
    space();
    if (source[index] === "}") break;
    if (source[index++] !== ",") throw new Error("Expected JSON separator");
    space();
  }
  return values;
}
function prettyLiteral(source) {
  let output = "", depth = 0;
  const line = () => "\n" + "  ".repeat(depth);
  for (let index = 0; index < source.length; index++) {
    const char = source[index];
    if (char === '"') {
      const end = stringEnd(source, index);
      output += source.slice(index, end); index = end - 1;
    } else if (char === "{" || char === "[") {
      output += char; depth++; output += line();
    } else if (char === "}" || char === "]") {
      depth--; output += line() + char;
    } else if (char === ",") output += "," + line();
    else if (char === ":") output += ": ";
    else if (!/\s/.test(char)) output += char;
  }
  return output;
}
function responseState(source) {
  const value = JSON.parse(source), fields = objectValues(source);
  const state = stateValue(value);
  if (!state || fields.size !== Object.keys(value).length) throw new Error("Invalid review state");
  if (state.status !== "prompt") return state;
  const promptFields = objectValues(fields.get("prompt"));
  if (promptFields.size !== 6) throw new Error("Invalid approval fields");
  return { ...state, bodyText: prettyLiteral(promptFields.get("body")) };
}
function element(document, tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = text(content);
  return node;
}
function expiryLabel(value) {
  if (value === null) return "Expiry not provided";
  return new Intl.DateTimeFormat("en-GB", {
    year: "numeric", month: "short", day: "numeric", hour: "2-digit",
    minute: "2-digit", second: "2-digit", timeZone: "UTC", timeZoneName: "short",
  }).format(new Date(expiry(value)));
}

/** Pure DOM renderer. The callback receives only the exact offered decision.
 * Disposal and disable retire detached controls synchronously. No fetch runs.
 */
function renderApproval(container, nativePrompt, {
  onDecision = () => {}, now = Date.now, locked = false, bodyText,
} = {}) {
  const document = container.ownerDocument;
  let prompt = promptValue(nativePrompt);
  if (!prompt) throw new TypeError("A complete native approval prompt is required");
  let alive = true, claimed = locked;
  const card = element(document, "section", "forge-review__card");
  card.setAttribute("aria-label", "Setup approval");
  card.append(element(document, "p", "forge-review__eyebrow", "SETUP APPROVAL"),
    element(document, "h2", "forge-review__summary", prompt.summary));
  const date = element(document, "p", "forge-review__expiry");
  date.append(element(document, "span", "", prompt.expires_at === null
    ? expiryLabel(null) : "Expires: " + expiryLabel(prompt.expires_at)));
  if (prompt.expires_at !== null) {
    const raw = element(document, "code", "forge-review__exact-expiry", prompt.expires_at);
    date.append(raw);
  }
  card.append(date, element(document, "p", "forge-review__body-label",
    "Full request, as sent by the runtime"));
  const body = element(document, "pre", "forge-review__body");
  body.setAttribute("aria-label", "Complete native request and warnings");
  body.tabIndex = 0;
  // The mounted client provides the raw JSON body's display representation.
  // Direct pure-renderer callers provide the actual JSON value.
  body.textContent = text(bodyText ?? JSON.stringify(prompt.body, null, 2));
  const expand = element(document, "button", "forge-review__expand", "Show all");
  expand.type = "button";
  expand.setAttribute("aria-expanded", "false");
  expand.addEventListener("click", () => {
    if (!alive) return;
    const expanded = expand.getAttribute("aria-expanded") !== "true";
    expand.setAttribute("aria-expanded", String(expanded));
    expand.textContent = expanded ? "Show less" : "Show all";
    body.classList.toggle("forge-review__body--expanded", expanded);
  });
  card.append(body, expand);
  const controls = element(document, "div", "forge-review__decisions");
  const buttons = [];
  const disable = () => { claimed = true; for (const button of buttons) button.disabled = true; };
  for (const decision of prompt.allowed_decisions) {
    const button = element(document, "button", "forge-review__decision",
      decision === "approve" ? "Approve this request" : "Do not approve");
    button.type = "button";
    button.dataset.decision = decision;
    button.disabled = locked || (prompt.expires_at !== null && expiry(prompt.expires_at) <= now());
    button.addEventListener("click", () => {
      if (!alive || claimed || button.disabled) return;
      if (prompt.expires_at !== null && expiry(prompt.expires_at) <= now()) { disable(); return; }
      // Claim and disable every choice before invoking a possibly async owner.
      disable();
      onDecision({ approval_id: prompt.approval_id, decision });
    });
    buttons.push(button); controls.append(button);
  }
  card.append(controls, element(document, "p", "forge-review__reference",
    "Approval reference: " + prompt.approval_id));
  container.replaceChildren(card);
  return { disable, dispose() {
    alive = false; disable(); prompt = null;
    // Retire content even in DOM nodes a caller held before this view cleared.
    for (const node of card.querySelectorAll("*")) node.textContent = "";
    card.replaceChildren(); card.remove();
  } };
}

export function renderControllerApproval(container, nativePrompt, {
  onDecision, now, locked,
} = {}) {
  return renderApproval(container, nativePrompt, { onDecision, now, locked });
}

const COPY = {
  starting: "Preparing the protected review",
  awaiting_prompt: "Waiting for the setup request",
  prompt_expired: "This request has expired",
  answer_submitted: "Answer submitted",
  answer_forwarded: "Answer forwarded",
  outcome_unknown: "The result could not be confirmed",
  review_completed: "Review complete",
  denied: "Review denied",
  unavailable: "The protected review is unavailable",
  pending_reconciliation: "The result needs reconciliation",
};

/** Dedicated authenticated view, using only the existing state/answer routes.
 * The login owner calls clearAuthentication synchronously at local sign-out.
 * Disposal clears presentation only; the protected presenter owns cleanup.
 */
export function mountControllerReview(container, {
  fetch: fetcher = globalThis.fetch?.bind(globalThis), now = Date.now,
  schedule = setTimeout, unschedule = clearTimeout,
  eventTarget = container.ownerDocument.defaultView,
} = {}) {
  if (typeof fetcher !== "function") throw new TypeError("A fetch function is required");
  mounts.get(container)?.dispose();
  const document = container.ownerDocument;
  let disposed = false, authenticationEnded = false, generation = 0;
  let state = { status: "starting" }, reading = null, timer = null, expiryTimer = null;
  let renderer = null, renderedPrompt = null, answerAttempted = false, sending = false;
  const current = () => !disposed && !authenticationEnded;
  const clearTimers = () => {
    if (timer !== null) unschedule(timer);
    if (expiryTimer !== null) unschedule(expiryTimer);
    timer = expiryTimer = null;
  };
  function clear() {
    clearTimers(); renderer?.dispose(); renderer = null; renderedPrompt = null;
    state = null; sending = false; reading = null; container.replaceChildren();
  }
  function clearAuthentication() {
    if (disposed || authenticationEnded) return;
    authenticationEnded = true; generation++; clear();
    container.append(element(document, "p", "forge-review__status", "Session ended"));
  }
  function render() {
    const promptKey = state?.status === "prompt" ? JSON.stringify([state.prompt, state.bodyText]) : null;
    // A checked repeat of the same retained prompt must not reset the reader's
    // disclosure, scroll or keyboard focus while they inspect its full body.
    if (current() && renderer && !answerAttempted && !sending && promptKey === renderedPrompt) return;
    if (expiryTimer !== null) unschedule(expiryTimer);
    expiryTimer = null;
    renderer?.dispose(); renderer = null; renderedPrompt = null; container.replaceChildren();
    if (!current()) return;
    if (state?.status === "prompt") {
      renderer = renderApproval(container, state.prompt, {
        now, locked: answerAttempted || sending, bodyText: state.bodyText, onDecision: submit,
      });
      renderedPrompt = promptKey;
      // Once an answer enters, only its protected response owns the outcome.
      // Expiry no longer offers actions and must not retire the POST token.
      if (!answerAttempted && state.prompt.expires_at !== null) {
        if (expiryTimer !== null) unschedule(expiryTimer);
        expiryTimer = schedule(() => {
          expiryTimer = null;
          if (current() && state?.status === "prompt" && expiry(state.prompt.expires_at) <= now()) {
            generation++; reading = null; state = { status: "prompt_expired" };
            clearTimers(); render();
          }
        }, Math.min(2_147_483_647, Math.max(0, expiry(state.prompt.expires_at) - now())));
      }
    }
    if (state?.status !== "prompt" || sending) {
      const status = element(document, "p", "forge-review__status",
        sending ? "Sending answer" : COPY[state?.status] ?? COPY.unavailable);
      status.setAttribute("role", "status");
      if (sending || ["starting", "awaiting_prompt", "answer_submitted", "answer_forwarded"].includes(state?.status))
        status.classList.add("forge-review__status--pending");
      container.append(status);
    }
  }
  function pollLater() {
    if (!current() || sending || !POLLING.has(state?.status)) return;
    if (timer !== null) unschedule(timer);
    timer = schedule(() => { timer = null; void refresh(); }, 1000);
  }
  async function request(path, payload) {
    const response = await fetcher(PREFIX + path, {
      method: payload === undefined ? "GET" : "POST",
      credentials: "same-origin", cache: "no-store", redirect: "error",
      headers: payload === undefined ? { Accept: "application/json" }
        : { Accept: "application/json", "Content-Type": "application/json" },
      ...(payload === undefined ? {} : { body: JSON.stringify(payload) }),
    });
    if (response.status === 401 || response.status === 403) {
      clearAuthentication(); throw new Error("Authentication ended");
    }
    const source = await response.text();
    if (!response.ok) {
      let code;
      try { code = JSON.parse(source)?.error?.code; } catch {}
      if (code === "invalid_binding") clearAuthentication();
      throw new Error("Protected review refused");
    }
    return { response, source };
  }
  function refresh() {
    if (!current() || sending) return Promise.resolve(false);
    if (reading) return reading.promise;
    const owner = { generation: ++generation };
    owner.promise = (async () => {
      try {
        const { response, source } = await request("/state");
        if (!current() || owner.generation !== generation) return false;
        const next = responseState(source);
        if (response.status !== (next.status === "starting" ? 202 : 200))
          throw new Error("Unexpected review response");
        if (answerAttempted && next.status === "prompt") state = { status: "outcome_unknown" };
        else state = next;
        if (state.status === "prompt" && state.prompt.expires_at !== null
            && expiry(state.prompt.expires_at) <= now()) state = { status: "prompt_expired" };
        render(); pollLater(); return true;
      } catch {
        if (current() && owner.generation === generation) {
          clearTimers(); state = { status: "outcome_unknown" }; render();
        }
        return false;
      } finally { if (reading === owner) reading = null; }
    })();
    reading = owner;
    return owner.promise;
  }
  async function submit(payload) {
    if (!current() || answerAttempted || state?.status !== "prompt"
        || payload.approval_id !== state.prompt.approval_id
        || !state.prompt.allowed_decisions.includes(payload.decision)) return false;
    if (state.prompt.expires_at !== null && expiry(state.prompt.expires_at) <= now()) {
      generation++; reading = null; state = { status: "prompt_expired" };
      clearTimers(); render(); return false;
    }
    // Local claim is synchronous and permanent for this view, before fetch.
    answerAttempted = sending = true;
    const token = ++generation;
    reading = null; clearTimers(); render();
    try {
      const { response, source } = await request("/answer", payload);
      if (!current() || token !== generation) return false;
      if (response.status !== 202 || responseState(source).status !== "answer_submitted")
        throw new Error("Unconfirmed answer");
      sending = false; state = { status: "answer_submitted" }; render(); pollLater(); return true;
    } catch {
      if (current() && token === generation) {
        sending = false; state = { status: "outcome_unknown" }; render();
      }
      return false;
    }
  }
  function dispose() {
    if (disposed) return;
    disposed = true; generation++; clear();
    eventTarget?.removeEventListener("pagehide", dispose);
    if (mounts.get(container) === view) mounts.delete(container);
  }
  const view = { refresh, clearAuthentication, dispose, ready: null };
  mounts.set(container, view);
  eventTarget?.addEventListener("pagehide", dispose);
  render(); view.ready = refresh();
  return view;
}
