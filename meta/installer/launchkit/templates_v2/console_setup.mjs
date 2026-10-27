// Presentation beside the native Console. The authenticated host owns all work.
const PREFIX = "/console/forge/connect";
const COMPLETIONS = new Set(["completed", "completed_without_result", "callback_pending", "callback_batch_pending", "cancelled", "abandoned", "abandoned_with_error", "completed_with_finalization_failure", "runtime_terminated"]);
const COMMITTED = new Set(["completed", "completed_without_result"]);
const STARTUP_REFUSALS = new Set(["native_auth_required", "required_context_missing", "unsupported_presenter"]);
const PROVIDER_ACTIONS = new Set(["use", "enter_key", "sign_in", "back"]);
const pages = new WeakMap();

export function visibleText(value) {
  return String(value).replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u200b-\u200f\u2028-\u202e\u2060-\u206f\ufeff]/gu,
    char => `[U+${char.codePointAt(0).toString(16).toUpperCase().padStart(4, "0")}]`);
}

function stateValue(value) {
  if (!value || ["available", "can_send", "can_stop", "outcome_uncertain"].some(key => typeof value[key] !== "boolean")) return null;
  if (value.private_notice_pending !== undefined && typeof value.private_notice_pending !== "boolean") return null;
  const active = value.active_send;
  const last = value.last_completion;
  if (active !== null && (!active || typeof active.send_request_id !== "string" || !active.send_request_id.startsWith("h:") || typeof active.cancellation_requested !== "boolean")) return null;
  if (last !== null && (!last || typeof last.send_request_id !== "string" || !last.send_request_id.startsWith("h:") || !COMPLETIONS.has(last.completion))) return null;
  return { available: value.available, can_send: value.can_send, can_stop: value.can_stop, outcome_uncertain: value.outcome_uncertain, active_send: active && { ...active }, last_completion: last && { ...last }, ...(value.private_notice_pending === undefined ? {} : { private_notice_pending: value.private_notice_pending }) };
}

function exactFields(value, fields) {
  return value && typeof value === "object" && !Array.isArray(value) && Object.keys(value).length === fields.length && fields.every(key => Object.hasOwn(value, key));
}
function targetValue(value) {
  if (exactFields(value, ["initialize_request_id"]) && /^h:[A-Za-z0-9_-]{1,64}$/.test(value.initialize_request_id)) return { initialize_request_id: value.initialize_request_id };
  if (exactFields(value, ["scope"]) && typeof value.scope === "string" && value.scope) return { scope: value.scope };
  return null;
}
const sameTarget = (left, right) => !!left && !!right && left.scope === right.scope && left.initialize_request_id === right.initialize_request_id;
function shownTarget(value) {
  return exactFields(value, value?.profile === undefined ? ["model", "provider", "realm", "binding"] : ["model", "provider", "realm", "binding", "profile"]) && Object.values(value).every(part => typeof part === "string" && part.length > 0);
}
function providerPrompt(value) {
  if (!value) return false;
  if (value.kind === "provider_target") return Array.isArray(value.choices) && value.choices.length > 0 && value.choices.every(choice => typeof choice.choice_id === "string" && choice.choice_id && shownTarget(choice.target) && typeof choice.status === "string") && new Set(value.choices.map(choice => choice.choice_id)).size === value.choices.length;
  if (value.kind === "provider_action") return shownTarget(value.target) && Array.isArray(value.actions) && value.actions.length > 0 && value.actions.every(action => PROVIDER_ACTIONS.has(action)) && new Set(value.actions).size === value.actions.length;
  if (value.kind === "provider_api_key") return shownTarget(value.target) && value.max_bytes === 8192;
  if (value.kind === "provider_browser") return shownTarget(value.target) && typeof value.authorization_url === "string" && value.authorization_url.length > 0;
  if (value.kind === "provider_device") return shownTarget(value.target) && typeof value.verification_uri === "string" && value.verification_uri.length > 0 && typeof value.user_code === "string" && value.user_code.length > 0;
  return value.kind === "approval" && Array.isArray(value.allowed_decisions);
}
function validKey(key) {
  return typeof key === "string" && !!key.trim() && !/[\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(key) && new TextEncoder().encode(key).length <= 8192;
}
function serviceAnswer(prompt, answer) {
  if (!answer || typeof answer !== "object") return null;
  if (answer.kind === "cancel" && exactFields(answer, ["kind"]) && ["provider_browser", "provider_device"].includes(prompt.kind)) return { kind: "cancel" };
  if (answer.kind !== prompt.kind) return null;
  if (prompt.kind === "provider_target" && exactFields(answer, ["kind", "choice_id"]) && prompt.choices.some(choice => choice.choice_id === answer.choice_id)) return { kind: answer.kind, choice_id: answer.choice_id };
  if (prompt.kind === "provider_action" && exactFields(answer, ["kind", "action"]) && prompt.actions.includes(answer.action)) return { kind: answer.kind, action: answer.action };
  if (prompt.kind === "provider_api_key" && exactFields(answer, ["kind", "key"]) && validKey(answer.key)) return { kind: answer.kind, key: answer.key };
  if (prompt.kind === "approval" && exactFields(answer, ["kind", "decision"]) && ["approve", "deny"].includes(answer.decision) && prompt.allowed_decisions.includes(answer.decision)) return { kind: answer.kind, decision: answer.decision };
  return null;
}

/** Page-lived presentation bridge, with no transcript, secret or native storage.
 * Disposal detaches streams; only the host may retire a login or application.
 */
export function createConsoleSetupPort({ fetch: fetcher, EventSource: Events, SetupConversationError, schedule = setTimeout, unschedule = clearTimeout } = {}) {
  fetcher ??= globalThis.fetch?.bind(globalThis);
  Events ??= globalThis.EventSource;
  let scope = "", state = null, revision = 0, views = 0, started = false, initialized = false, poll = null;
  let privateStream = null, privateReady = false, prompt = null;
  let unavailable = false, reading = null, sending = null, stopping = null, noIdUncertain = false;
  let connection = "starting", answerState = "idle", answerId = null, historyUnavailable = false;
  let protocolVersion = 1, bindingTarget = null, startupRefusal = null;
  const listeners = new Set(), unavailableListeners = new Set(), promptListeners = new Set();
  const closedPrompts = new Set();
  const safeError = code => SetupConversationError ? new SetupConversationError(code) : Object.assign(new Error("Setup status is unavailable. Check it through the host."), { code });
  const notify = () => { for (const listener of listeners) listener(); };
  const notifyPrompt = () => { for (const listener of promptListeners) listener(); };
  const stopStreams = () => {
    privateStream?.close(); privateStream = null;
    privateReady = false; prompt = null; notifyPrompt();
    if (poll !== null) unschedule(poll); poll = null;
  };
  function retire(reason = "unavailable") {
    unavailable = true; connection = reason; state = null; startupRefusal = null; revision += 1;
    stopStreams(); answerState = "idle"; answerId = null; historyUnavailable = false; closedPrompts.clear(); initialized = false;
    sending?.reject(safeError("unavailable")); sending = null;
    stopping?.resolve("not_current"); stopping = null; noIdUncertain = false;
    bindingTarget = null;
    for (const listener of unavailableListeners) listener(); notify();
  }
  async function request(path, body) {
    let response;
    try {
      response = await fetcher(`${PREFIX}${path}`, {
        method: body === undefined ? "GET" : "POST", credentials: "same-origin", cache: "no-store", redirect: "error",
        headers: body === undefined ? { Accept: "application/json" } : { Accept: "application/json", "Content-Type": "application/json" },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
    } catch { throw safeError("turn_uncertain"); }
    if (response.status === 401 || response.status === 403) { retire("sign-in"); throw safeError("unavailable"); }
    let value;
    try { value = await response.json(); } catch { throw safeError("turn_uncertain"); }
    if (!response.ok) throw Object.assign(safeError("unavailable"), {
      refusal: ["invalid_message", "busy", "invalid_binding", "binding_retired", "unsupported_presenter"].includes(value?.error?.code), status: response.status,
      ...(["/initialize", "/state"].includes(path) && STARTUP_REFUSALS.has(value?.error?.code) ? { startupRefusal: value.error.code } : {}),
    });
    return value;
  }
  function refuseStartup(error, at) {
    if (unavailable || initialized || at !== revision || !STARTUP_REFUSALS.has(error?.startupRefusal)) return false;
    startupRefusal = error.startupRefusal; connection = "refused"; revision += 1;
    stopStreams(); notify(); return true;
  }
  function completion(last) {
    if (!last) return;
    if (sending?.id === last.send_request_id) {
      const owned = sending; sending = null;
      if (COMMITTED.has(last.completion)) owned.resolve();
      else owned.reject(Object.assign(new Error("The original run has a known completion without a committed response."), { code: "known_completion" }));
    }
    if (stopping?.id === last.send_request_id) { const owned = stopping; stopping = null; owned.resolve(last.completion === "cancelled" ? "stopped" : "not_current"); }
  }
  function applyBinding(value) {
    if (unavailable || !exactFields(value, ["type", "protocol_version", "target"]) || value.type !== "binding" || value.protocol_version !== 3) return false;
    const next = targetValue(value.target);
    if (!next) return false;
    if (sameTarget(bindingTarget, next)) { privateReady = true; return true; }
    if (bindingTarget?.scope && next.initialize_request_id) return false; // A delayed starting observation cannot rewind the scoped child.
    if ((bindingTarget && !(bindingTarget.initialize_request_id && next.scope)) || (scope && next.scope !== scope)) { retire("replaced"); return false; }
    protocolVersion = 3; bindingTarget = next; privateReady = true; revision += 1;
    // A normal initialization-to-scope transition changes disclosure target,
    // not custody of an already submitted one-shot prompt answer.
    if (prompt && !sameTarget(prompt.target, next)) prompt = null;
    notify(); notifyPrompt(); return true;
  }
  function applyState(value, selectedScope = scope, startupRead = false) {
    const next = stateValue(value);
    if (!next || !selectedScope) return false;
    if (protocolVersion === 3 && (bindingTarget?.scope !== selectedScope || typeof next.private_notice_pending !== "boolean")) return false;
    if (unavailable && selectedScope === scope) return false;
    if (scope && selectedScope !== scope) {
      retire("replaced"); started = true; unavailable = false; noIdUncertain = false;
    }
    if (startupRead) startupRefusal = null;
    scope = selectedScope; state = next; revision += 1;
    // available:false is also the native initialization projection. Only an
    // explicit unavailable event or host binding retirement is terminal.
    connection = next.available ? "ready" : initialized ? "not-ready" : "starting";
    // Shared activity cannot associate this tab's unconfirmed POST. Only the
    // host's original request receipt can bind its exact browser association.
    completion(next.last_completion);
    notify(); streams();
    // A later attachment snapshot can follow an initialized HTTP observation.
    // Keep reading the same host while unavailable; do not restart its child.
    if (!next.available) pollLater();
    return true;
  }
  function event(item) {
    if (!item || unavailable || !views) return;
    if (item.type === "binding") { applyBinding(item); return; }
    if (item.type === "prompt") {
      const matched = protocolVersion === 3
        ? item.protocol_version === 3 && sameTarget(targetValue(item.target), bindingTarget) && providerPrompt(item.prompt)
        : item.protocol_version === undefined && item.scope === scope && !item.prompt?.kind?.startsWith("provider_");
      if (!matched || typeof item.id !== "string" || !item.id.startsWith("p:") || closedPrompts.has(item.id) || !item.prompt) return;
      privateReady = true;
      if (answerId !== item.id) { answerState = "idle"; answerId = null; }
      prompt = item; notifyPrompt(); notify(); return;
    }
    const value = item.type === "event" && item.event;
    if (!value) return;
    const target = protocolVersion === 3 ? targetValue(value.target) : null;
    if (protocolVersion === 3 && (Object.hasOwn(value, "scope") || !sameTarget(target, bindingTarget))) return;
    if (value.kind === "state") {
      if (target?.initialize_request_id) {
        const next = stateValue(value.state);
        if (!next || next.available || next.can_send || next.can_stop || next.outcome_uncertain || next.active_send !== null || next.last_completion !== null || next.private_notice_pending !== false) return;
        state = next; revision += 1; connection = "starting"; privateReady = true;
        notify(); notifyPrompt(); pollLater(); return;
      }
      const selectedScope = target?.scope ?? value.scope;
      if (applyState(value.state, selectedScope)) { privateReady = selectedScope === scope; notifyPrompt(); }
      return;
    }
    if (protocolVersion !== 3 && value.scope !== scope) return;
    if (value.kind === "unavailable") { retire("unavailable"); return; }
    if (value.kind === "prompt_closed" && typeof value.prompt_id === "string") {
      if (protocolVersion === 3 && (!["answered", "decision_recorded", "refused", "expired", "retired", "cancelled", "owner_completed"].includes(value.disposition) ||
          (value.disposition === "owner_completed" && prompt?.id === value.prompt_id && !["provider_browser", "provider_device"].includes(prompt.prompt.kind)))) return;
      closedPrompts.add(value.prompt_id);
      if (prompt?.id === value.prompt_id) { prompt = null; answerState = "idle"; answerId = null; notifyPrompt(); notify(); }
    }
    if (value.kind === "conversation" && (!target || target.scope === scope) && sending?.id === value.send_request_id) sending.observe?.(value.event);
  }
  function streams() {
    if (!views || !started || unavailable || startupRefusal || !Events || privateStream) return;
    // The authenticated private observer already carries every public frame
    // and the retained prompts in one pipe order. A second stream duplicates
    // observation and consumes another per-origin browser connection.
    const stream = new Events(`${PREFIX}/events/private`);
    privateStream = stream;
    stream.onmessage = message => {
      if (privateStream !== stream) return;
      let item; try { item = JSON.parse(message.data); } catch { return; }
      event(item);
    };
    stream.onerror = () => {
      if (privateStream !== stream) return;
      privateReady = false; prompt = null; notifyPrompt();
      if (stream.readyState === 2) { stream.close(); privateStream = null; }
      connection = "reconnecting"; notify();
      void refresh(); // GET only; EventSource reconnects the same observation.
    };
  }
  function pollLater() {
    if (!views || unavailable || startupRefusal || poll !== null) return;
    if (!initialized || state?.available === false || sending || stopping) poll = schedule(() => { poll = null; void refresh(); }, 1000);
  }
  async function result(id) {
    const at = revision, bound = scope;
    const value = await request(`/result/${encodeURIComponent(id)}`);
    if (scope !== bound || unavailable || value.request_id !== id) return;
    if (value.status === "completed" && value.result?.scope === bound && value.result.send_request_id === id && COMPLETIONS.has(value.result.completion)) {
      if (at === revision) applyState(value.result.state);
      completion({ send_request_id: id, completion: value.result.completion }); notify();
    } else if (value.status === "refused" && sending?.id === id) {
      const owned = sending; sending = null; owned.reject(safeError("unavailable")); notify();
    }
  }
  function sendReceipt(value, owned, at) {
    if (value.association !== owned.association || !/^h:[A-Za-z0-9_-]{1,64}$/.test(value.request_id ?? "") ||
        !["pending", "completed", "refused"].includes(value.status) || owned.id && value.request_id !== owned.id) throw safeError("turn_uncertain");
    owned.id = value.request_id; noIdUncertain = false; notify();
    if (value.status === "completed" && value.result?.scope === owned.scope && value.result.send_request_id === owned.id && COMPLETIONS.has(value.result.completion)) {
      if (at === revision) applyState(value.result.state);
      completion({ send_request_id: owned.id, completion: value.result.completion });
    } else if (value.status === "refused") { sending = null; owned.reject(safeError("unavailable")); notify(); }
  }
  async function associatedSend(owned) {
    const at = revision;
    const value = await request(`/send/${encodeURIComponent(owned.association)}`);
    if (scope !== owned.scope || unavailable || sending !== owned || owned.id) return;
    if (value.association === owned.association && value.status === "unknown") return;
    sendReceipt(value, owned, at);
  }
  async function refresh() {
    if (reading) return reading;
    const at = revision;
    reading = (async () => {
      try {
        const value = await request("/state");
        const current = at === revision;
        const bound = !value.binding || (current && applyBinding(value.binding));
        if (current && bound && value.status === "ready" && value.state?.kind === "state") {
          const selected = protocolVersion === 3 && !Object.hasOwn(value.state, "scope") ? targetValue(value.state.target)?.scope : protocolVersion === 3 ? null : value.state.scope;
          if (applyState(value.state.state, selected, true)) initialized = true;
        }
        else if (current && !unavailable && !startupRefusal && value.status === "starting") { connection = "starting"; notify(); }
        if (sending && !sending.id) await associatedSend(sending);
        if (sending?.id) await result(sending.id);
        if (stopping?.id && stopping.id !== sending?.id) await result(stopping.id);
        streams();
      } catch (error) {
        if (!refuseStartup(error, at) && !unavailable && !startupRefusal && (!error?.startupRefusal || at === revision)) { connection = "reconnecting"; notify(); }
      }
      finally { reading = null; pollLater(); }
    })();
    return reading;
  }
  async function initialize() {
    if (started) return refresh();
    started = true; const at = revision;
    try {
      const value = await request("/initialize", {});
      const current = at === revision;
      const bound = !value.binding || (current && applyBinding(value.binding));
      if (current && bound && value.status === "ready" && value.state?.kind === "state") {
        const selected = protocolVersion === 3 && !Object.hasOwn(value.state, "scope") ? targetValue(value.state.target)?.scope : protocolVersion === 3 ? null : value.state.scope;
        if (applyState(value.state.state, selected, true)) initialized = true;
      }
      streams();
    } catch (error) {
      if (!refuseStartup(error, at) && !unavailable && !startupRefusal && (!error?.startupRefusal || at === revision)) { connection = "reconnecting"; notify(); }
    }
    // Lost initialization is recovered by observation, never another POST.
    streams(); if (!startupRefusal) await refresh(); pollLater();
  }
  const port = {
    get scope() { return scope; }, get available() { return !!state?.available && !unavailable; },
    get canSend() { return port.available && state.can_send && !sending && !noIdUncertain; },
    get canStop() { return port.available && state.can_stop && !stopping; },
    get outcomeUncertain() { return !!state?.outcome_uncertain || noIdUncertain; },
    get snapshot() { return { scope, state, connection, startupRefusal, protocolVersion, binding: bindingTarget && { ...bindingTarget }, admissionUncertain: noIdUncertain, admitting: !!sending && !sending.id && !noIdUncertain, stopping: !!stopping, historyUnavailable }; },
    get protectedPrompt() { return privateReady && !unavailable ? prompt : null; },
    get answerState() { return answerState; },
    onUnavailable(listener) { unavailableListeners.add(listener); return () => unavailableListeners.delete(listener); },
    onStateChange(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    onPromptChange(listener) { promptListeners.add(listener); return () => promptListeners.delete(listener); },
    attachView() {
      views += 1; void initialize();
      let detached = false;
      return () => { if (detached) return; detached = true; views -= 1; if (!views) { if (sending) sending.observe = null; stopStreams(); } };
    },
    refresh,
    async history(offset) {
      if (!port.available) throw safeError("unavailable");
      const bound = scope, at = revision;
      const path = offset === undefined ? "/history" : `/history?offset=${Number.isSafeInteger(offset) && offset >= 0 ? offset : 0}`;
      let value; try { value = await request(path); } catch {
        if (scope === bound && port.available) { historyUnavailable = true; notify(); }
        throw safeError("history_unavailable");
      }
      if (scope !== bound || !port.available) throw safeError("unavailable");
      if (at === revision) applyState(value.state);
      const page = value.history;
      if (!page || !Array.isArray(page.messages)) { historyUnavailable = true; notify(); throw safeError("history_unavailable"); }
      historyUnavailable = false; notify();
      return page;
    },
    send(text, observe) {
      if (!port.canSend) return Promise.reject(safeError("busy"));
      if (typeof text !== "string" || !text.trim()) return Promise.reject(safeError("invalid_message"));
      const identity = globalThis.crypto?.randomUUID?.();
      if (!identity) return Promise.reject(safeError("unavailable"));
      const bound = scope, at = revision;
      return new Promise((resolve, reject) => {
        const owned = { scope: bound, id: null, association: `b:${identity.replaceAll("-", "")}`, observe, resolve, reject };
        sending = owned; noIdUncertain = false; notify();
        void request("/send", { text, association: owned.association }).then(value => {
          if (scope !== bound || sending !== owned) return;
          sendReceipt(value, owned, at);
          if (sending === owned) void refresh();
        }).catch(error => {
          if (scope !== bound || sending !== owned) return;
          if (error.refusal) { sending = null; noIdUncertain = false; reject(safeError("unavailable")); notify(); }
          else { noIdUncertain = !owned.id; connection = "reconnecting"; notify(); void refresh(); }
        }).finally(pollLater);
      });
    },
    stopObservedRun() {
      const id = state?.active_send?.send_request_id;
      if (!port.canStop || !id) return Promise.resolve("not_stoppable");
      const bound = scope;
      return new Promise((resolve, reject) => {
        const owned = { id, resolve, reject }; stopping = owned; notify();
        void request("/cancel", { send_request_id: id }).then(value => {
          if (scope !== bound || stopping !== owned) return;
          if (value.scope !== bound || value.send_request_id !== id) throw safeError("stop_uncertain");
          if (value.status === "not_current") { stopping = null; resolve("not_current"); notify(); }
          else if (value.status !== "cancellation_requested") throw safeError("stop_uncertain");
          void refresh();
        }).catch(() => {
          if (stopping !== owned) return;
          // Keep the exact stop intent visible. Do not turn transport loss into
          // a stopped result, or automatically repeat the cancellation POST.
          reject(safeError("stop_uncertain")); connection = "reconnecting"; notify(); void refresh();
        }).finally(pollLater);
      });
    },
    async answer(id, answer) {
      if (!port.protectedPrompt || port.protectedPrompt.id !== id || answerState !== "idle") return false;
      const owned = prompt, bound = scope;
      if (protocolVersion === 3) { answer = serviceAnswer(owned.prompt, answer); if (!answer) return false; }
      const samePrompt = () => scope === bound && prompt?.id === id && (protocolVersion !== 3 || sameTarget(prompt.target, owned.target)) && answerId === id;
      answerState = "sending"; answerId = id; notifyPrompt();
      try {
        const value = await request("/answer", { prompt_id: id, answer });
        if (!samePrompt()) return false;
        answerState = value.status === "forwarded" ? "forwarded" : "uncertain";
      } catch { if (samePrompt()) answerState = "uncertain"; }
      notifyPrompt(); return answerState === "forwarded";
    },
  };
  return port;
}

const startupRefusalLabels = {
  native_auth_required: "The setup agent needs native authentication before it can open. Signing in to the Console does not provide this authentication. Review the app's authentication setup in Forge.",
  required_context_missing: "The setup agent is missing required application context. Review the app's setup in Forge.",
  unsupported_presenter: "This app's runtime does not support the setup conversation. Review the app's runtime setup in Forge.",
};
const completionLabels = {
  completed: "Response completed", completed_without_result: "Run completed without a reply", callback_pending: "Waiting for a protected decision",
  callback_batch_pending: "Waiting for protected decisions", cancelled: "Agent run cancelled", abandoned: "Run abandoned",
  abandoned_with_error: "Run ended with an error", completed_with_finalization_failure: "Run finished, finalization failed", runtime_terminated: "Runtime terminated",
};

export function mountConsoleSetup(container, options) {
  if (typeof options?.mountSetupConversation !== "function") throw new TypeError("The host must supply the public Toolkit conversation view");
  const doc = container.ownerDocument;
  const priorFocus = doc.activeElement;
  let port = options.port ?? pages.get(doc);
  if (!port) { port = createConsoleSetupPort({ fetch: doc.defaultView?.fetch?.bind(doc.defaultView), EventSource: doc.defaultView?.EventSource, ...options }); pages.set(doc, port); }
  const el = (tag, className, text) => { const node = doc.createElement(tag); node.className = className; if (text !== undefined) node.textContent = visibleText(text); return node; };
  const root = el("aside", "forge-setup"); root.setAttribute("aria-label", "Set up your app");
  const header = el("header", "forge-setup__header");
  const brand = el("span", "forge-setup__eyebrow", "FORGE / SETUP");
  const close = el("button", "forge-setup__close", "Close"); close.type = "button"; close.setAttribute("aria-label", "Close setup drawer");
  header.append(brand, close);
  const intro = el("div", "forge-setup__intro");
  const mark = el("span", "forge-setup__orbit"); mark.setAttribute("aria-hidden", "true");
  const title = el("h1", "forge-setup__title", "Set up your app");
  title.tabIndex = -1;
  const help = el("p", "forge-setup__help", "Your setup agent will explain the options this app supports. Review private sign-in and configuration decisions here.");
  const introduction = el("div", "forge-setup__introduction"); introduction.append(title, help); intro.append(mark, introduction);
  const status = el("div", "forge-setup__status"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
  const statusText = el("p", "forge-setup__status-text");
  const completion = el("p", "forge-setup__completion");
  const historyNotice = el("p", "forge-setup__completion");
  const check = el("button", "forge-setup__check", "Check current status"); check.type = "button";
  status.append(statusText, completion, historyNotice, check);
  const protectedPanel = el("section", "forge-setup__protected"); protectedPanel.setAttribute("aria-label", "Protected setup decision");
  const chat = el("div", "forge-setup__chat");
  const footer = el("p", "forge-setup__footer", "Closing this drawer leaves the setup agent running. Private decisions stay separate from the conversation.");
  root.append(header, intro, status, protectedPanel, chat, footer); container.replaceChildren(root);
  let disposed = false, view = null, viewScope = "", viewCompletion = "", renderedPrompt = null, renderedAnswerState = null;
  function clearPrivate() { for (const input of protectedPanel.querySelectorAll("input")) input.value = ""; protectedPanel.replaceChildren(); renderedPrompt = null; }
  function renderStatus() {
    if (disposed) return;
    const snapshot = port.snapshot;
    const active = snapshot.state?.active_send;
    const provider = port.protectedPrompt?.protocol_version === 3 && port.protectedPrompt.prompt.kind.startsWith("provider_");
    statusText.textContent = snapshot.connection === "sign-in" ? "Sign in to the Console again to continue."
      : snapshot.connection === "replaced" ? "This setup session was replaced."
        : snapshot.startupRefusal ? startupRefusalLabels[snapshot.startupRefusal]
        : snapshot.admissionUncertain || port.outcomeUncertain ? "Message outcome unconfirmed. Checking the original request only; it has not been sent again."
          : snapshot.admitting ? "Sending your message..."
            : snapshot.stopping || active?.cancellation_requested ? "Stop requested. Waiting for confirmed cancellation."
            : provider ? "Complete the private provider setup to continue."
              : snapshot.state?.private_notice_pending || (snapshot.protocolVersion === 3 && port.protectedPrompt) ? (snapshot.state?.last_completion ? "Waiting for a private decision. The confirmed response is retained." : "Complete the private setup decision to continue.")
            : active ? "The setup agent is responding."
              : ["starting", "not-ready"].includes(snapshot.connection) || (snapshot.connection === "reconnecting" && !port.available) ? "Opening the setup conversation..."
                : !port.available ? "Setup is unavailable. Check current status to try reading it again."
                  : snapshot.connection === "reconnecting" ? "Connection interrupted. Current work is retained by the host."
                    : "Ready when you are";
    footer.textContent = snapshot.startupRefusal
      ? "Closing this drawer keeps the original startup result with the host. Checking status does not restart the agent."
      : "Closing this drawer leaves the setup agent running. Private decisions stay separate from the conversation.";
    const last = snapshot.state?.last_completion;
    completion.textContent = last ? `Last confirmed result: ${completionLabels[last.completion]}` : "";
    historyNotice.textContent = snapshot.historyUnavailable ? "Saved conversation is unavailable. Current status and confirmed results are retained. Refresh history to read it again." : "";
    root.dataset.state = snapshot.connection;
    const completionIdentity = last ? JSON.stringify([port.scope, last.send_request_id, last.completion]) : "";
    if (view && (!port.available || viewScope !== port.scope)) { view.dispose(); view = null; viewScope = ""; viewCompletion = ""; }
    if (!view && port.available && port.scope) {
      viewScope = port.scope; viewCompletion = completionIdentity;
      view = options.mountSetupConversation(chat, { scope: viewScope, conversation: port });
    } else if (view && last && completionIdentity !== viewCompletion) {
      viewCompletion = completionIdentity;
      // The existing view guard leaves its own in-flight send's read with that view.
      if (snapshot.state?.private_notice_pending) void view.refresh();
    }
  }
  function renderPrompt() {
    if (disposed) return;
    const item = port.protectedPrompt;
    if (item === renderedPrompt && renderedAnswerState === port.answerState) return;
    clearPrivate(); renderedPrompt = item; renderedAnswerState = port.answerState;
    if (!item) { protectedPanel.hidden = true; return; }
    protectedPanel.hidden = false;
    const prompt = item.prompt;
    const badge = el("div", "forge-setup__private-badge");
    badge.append(el("p", "forge-setup__eyebrow", "PRIVATE STEP"), el("span", "forge-setup__private-note", "Handled by the Console, not the chat"));
    protectedPanel.append(badge);
    const heading = el("h2", "forge-setup__decision-title");
    heading.setAttribute("aria-live", "polite");
    const detail = el("p", "forge-setup__decision-help");
    const controls = el("div", "forge-setup__decisions");
    const locked = port.answerState !== "idle";
    function button(label, answer, secondary = false) {
      const node = el("button", `forge-setup__decision${secondary ? " forge-setup__decision--secondary" : ""}`, label);
      node.type = "button"; node.disabled = locked;
      node.addEventListener("click", () => { if (!node.disabled && !disposed) void port.answer(item.id, answer); }); controls.append(node); return node;
    }
    function targetDetails(target, parent = protectedPanel) {
      parent.append(el("p", "forge-setup__decision-help", `${target.model} / ${target.provider}`));
      parent.append(el("code", "forge-setup__origin", `Realm: ${target.realm}\nBinding: ${target.binding}${target.profile === undefined ? "" : `\nProfile: ${target.profile}`}`));
    }
    protectedPanel.append(heading, detail);
    if (item.protocol_version === 3 && prompt.kind === "provider_target") {
      heading.textContent = "Choose a model connection"; detail.textContent = "Choose one of the exact targets offered by the provider owner. Its status describes the saved connection.";
      for (const choice of prompt.choices) {
        const node = button(`${choice.target.model} / ${choice.target.provider}`, { kind: "provider_target", choice_id: choice.choice_id }, true);
        node.append(el("small", "forge-setup__choice-origin", choice.status));
        targetDetails(choice.target, node);
      }
    } else if (item.protocol_version === 3 && prompt.kind === "provider_action") {
      heading.textContent = "Connect this provider"; detail.textContent = "Choose one of the actions offered for this exact provider connection."; targetDetails(prompt.target);
      const labels = { use: "Use this connection", enter_key: "Enter an API key privately", sign_in: "Sign in to this provider", back: "Choose another connection" };
      for (const action of prompt.actions) button(labels[action], { kind: "provider_action", action }, action === "back");
    } else if (item.protocol_version === 3 && prompt.kind === "provider_api_key") {
      heading.textContent = "Enter your provider API key"; detail.textContent = "The provider owner receives this key through the private control. It never becomes a chat message."; targetDetails(prompt.target);
      const form = el("form", "forge-setup__password-form");
      const label = el("label", "forge-setup__password-label", "Private provider API key");
      const input = el("input", "forge-setup__password"); input.type = "password"; input.autocomplete = "off"; input.disabled = locked;
      input.spellcheck = false; input.setAttribute("autocapitalize", "off");
      const notice = el("p", "forge-setup__decision-help"); notice.setAttribute("role", "status");
      const submit = el("button", "forge-setup__decision", "Submit API key privately"); submit.type = "submit"; submit.disabled = locked;
      label.append(input); form.append(label, notice, submit); protectedPanel.append(form);
      form.addEventListener("submit", event => {
        event.preventDefault(); if (disposed || locked || port.protectedPrompt?.id !== item.id || port.answerState !== "idle") return;
        const key = input.value;
        if (!validKey(key)) { notice.textContent = "Use a nonblank API key of at most 8192 UTF-8 bytes, without control characters. Spaces are preserved exactly."; input.focus(); return; }
        input.value = ""; void port.answer(item.id, { kind: "provider_api_key", key });
      });
    } else if (item.protocol_version === 3 && ["provider_browser", "provider_device"].includes(prompt.kind)) {
      heading.textContent = "Finish provider sign-in"; detail.textContent = "Use the exact address shown below in your browser. The provider owner is waiting for its own sign-in result; visiting the address does not confirm success."; targetDetails(prompt.target);
      protectedPanel.append(el("code", "forge-setup__origin", prompt.kind === "provider_browser" ? prompt.authorization_url : prompt.verification_uri));
      if (prompt.kind === "provider_device") {
        protectedPanel.append(el("p", "forge-setup__decision-help", "Enter this exact code on the provider's page:"));
        protectedPanel.append(el("code", "forge-setup__origin", prompt.user_code));
      }
      button("Cancel provider sign-in", { kind: "cancel" }, true);
    } else if (prompt.kind === "forge_recovery" && Array.isArray(prompt.choices)) {
      heading.textContent = "Pick up where you left off"; detail.textContent = "Choose one of the setups offered by this host, or start a new one.";
      for (const choice of prompt.choices) if (typeof choice.choice_id === "string") {
        const node = button(choice.application_id, { kind: "forge_recovery", choice: { kind: "existing", choice_id: choice.choice_id } }, true);
        node.append(el("small", "forge-setup__choice-origin", `${choice.origin} / ${choice.change_id}`));
      }
      if (prompt.more === true) button("More saved setups", { kind: "forge_recovery", choice: { kind: "more" } }, true);
      button("Start a new setup", { kind: "forge_recovery", choice: { kind: "new" } });
      if (prompt.skipped_entries) protectedPanel.append(el("p", "forge-setup__decision-help", `${prompt.skipped_entries} saved entries could not be offered by the host.`));
    } else if (prompt.kind === "forge_origin" && typeof prompt.permitted_origin === "string") {
      heading.textContent = "Use this Forge"; detail.textContent = "The host offered this exact address. You do not need to enter or choose another URL.";
      protectedPanel.append(el("code", "forge-setup__origin", prompt.permitted_origin));
      button("Continue with this Forge", { kind: "forge_origin", origin: prompt.permitted_origin });
    } else if (prompt.kind === "forge_password" && typeof prompt.origin === "string" && prompt.min_chars === 12 && prompt.max_chars === 4096) {
      heading.textContent = prompt.mode === "reauthenticate" ? "Sign in again to continue" : "Sign in to Forge";
      detail.textContent = "Use this Forge's private Console password, not an API key. It is sent only through the protected sign-in control.";
      protectedPanel.append(el("code", "forge-setup__origin", prompt.origin));
      const form = el("form", "forge-setup__password-form");
      const label = el("label", "forge-setup__password-label", "Private Console password");
      const input = el("input", "forge-setup__password"); input.type = "password"; input.autocomplete = "current-password"; input.disabled = locked;
      input.spellcheck = false; input.setAttribute("autocapitalize", "off");
      const notice = el("p", "forge-setup__decision-help"); notice.setAttribute("role", "status");
      const submit = el("button", "forge-setup__decision", "Sign in privately"); submit.type = "submit"; submit.disabled = locked;
      label.append(input); form.append(label, notice, submit); protectedPanel.append(form);
      form.addEventListener("submit", event => {
        event.preventDefault(); if (disposed || locked || port.protectedPrompt?.id !== item.id || port.answerState !== "idle") return;
        const password = input.value, length = Array.from(password).length;
        if (length < 12 || length > 4096) { notice.textContent = "Use 12-4096 characters. Spaces are preserved exactly."; input.focus(); return; }
        input.value = ""; void port.answer(item.id, { kind: "forge_password", password });
      });
    } else if (prompt.kind === "approval" && Array.isArray(prompt.allowed_decisions)) {
      heading.textContent = "Review the exact change"; detail.textContent = visibleText(prompt.summary);
      protectedPanel.append(el("p", "forge-setup__decision-help", "Approval applies to the complete proposal below, including its warnings. A recorded decision is separate from activation or delivery."));
      const proposal = el("pre", "forge-setup__proposal"); proposal.setAttribute("aria-label", "Complete proposed configuration and warnings"); proposal.tabIndex = 0;
      let complete = false;
      try { const json = JSON.stringify(prompt.body, null, 2); complete = typeof json === "string"; proposal.textContent = complete ? visibleText(json) : "The complete proposal could not be displayed. Do not approve it."; }
      catch { proposal.textContent = "The complete proposal could not be displayed. Do not approve it."; }
      protectedPanel.append(proposal);
      if (prompt.expires_at) protectedPanel.append(el("p", "forge-setup__decision-help", `Decision expires: ${prompt.expires_at}`));
      const expired = prompt.expires_at && (!Number.isFinite(Date.parse(prompt.expires_at)) || Date.parse(prompt.expires_at) <= Date.now());
      for (const decision of ["approve", "deny"]) if (prompt.allowed_decisions.includes(decision)) {
        const decisionButton = button(decision === "approve" ? "Approve this exact change" : "Do not approve", { kind: "approval", decision }, decision === "deny");
        if (expired || (decision === "approve" && !complete)) decisionButton.disabled = true;
      }
    } else { heading.textContent = "A protected decision is unavailable"; detail.textContent = "This prompt cannot be displayed safely in this view."; }
    if (item.protocol_version !== 3) button("Cancel this setup interaction", { kind: "cancel" }, true);
    protectedPanel.append(controls);
    if (locked) protectedPanel.append(el("p", "forge-setup__answer-status", port.answerState === "uncertain"
      ? "The answer outcome is unconfirmed. It has not been submitted again. Check current status."
      : port.answerState === "sending" ? "Submitting your protected answer..."
        : "Answer sent for forwarding. Waiting for the host's recorded decision; no effect is confirmed yet."));
  }
  check.addEventListener("click", () => { if (!disposed) void port.refresh(); });
  close.addEventListener("click", () => { if (!disposed) { dispose(); options.onClose?.(); } });
  const detachState = port.onStateChange(renderStatus), detachPrompt = port.onPromptChange(renderPrompt);
  const detachUnavailable = port.onUnavailable(() => { clearPrivate(); renderStatus(); });
  const detachView = port.attachView();
  renderStatus(); renderPrompt();
  if (options.focus !== false) title.focus();
  root.addEventListener("keydown", event => { if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); close.click(); } });
  function dispose() {
    if (disposed) return;
    const restoreFocus = root.contains(doc.activeElement);
    disposed = true; clearPrivate(); view?.dispose(); detachState(); detachPrompt(); detachUnavailable(); detachView(); root.remove();
    if (restoreFocus && priorFocus?.isConnected) priorFocus.focus?.();
  }
  return { dispose, refresh: () => port.refresh(), conversation: port };
}
