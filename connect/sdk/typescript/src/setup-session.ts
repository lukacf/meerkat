import type { Session, SessionHistory, SessionMessage, StreamEvent } from "@rkat/sdk";
import { SetupConversationError } from "./ui/setup-model.js";
import type { SetupConversationMessage, SetupConversationHistory, SetupConversationEvent, SetupConversationPort } from "./ui/setup-model.js";
export { SetupConversationError } from "./ui/setup-model.js";
export type { SetupConversationMessage, SetupConversationHistory, SetupConversationEvent, SetupConversationPort, SetupConversationErrorCode } from "./ui/setup-model.js";

/** Captured by the authenticated host, never populated from a browser request.
 * The host aborts invalidated when this viewer/realm/session admission changes.
 * Aborting invalidates presentation only; it does not stop the native run.
 */
export interface NativeSetupBinding {
  /** Host must reserve exclusive streaming composition for this Session. The
   * released SDK has one event queue per session; this instance's local guard
   * cannot arbitrate other clients or other handles to the same native session.
   */
  readonly session: Session;
  readonly scope: string;
  readonly invalidated: AbortSignal;
  /** Admission must cover view, send AND exact-run control for this setup
   * session, not merely permission to read it. Use a separate read-only view
   * for an audience lacking any of these permissions.
   */
  readonly revalidate: () => boolean | Promise<boolean>;
}

interface ActiveSend {
  started: boolean;
  runId?: string;
  crossed: boolean;
  stop?: Promise<"stopped" | "not_current" | "not_stoppable">;
}

const PAGE_SIZE = 50;

function textFrom(message: SessionMessage): SetupConversationMessage | undefined {
  const role = message.role === "block_assistant" ? "assistant" : message.role;
  if (role !== "user" && role !== "assistant") return;
  // Tool results, reasoning, metadata and raw owner payloads never enter this view.
  let text = message.body;
  if (role === "assistant" && message.blocks.length > 0) {
    text = message.blocks.filter(block => block.blockType === "text")
      .map(block => block.text ?? "").join("");
  } else if (text === undefined && typeof message.content === "string") {
    text = message.content;
  } else if (text === undefined && Array.isArray(message.content)) {
    text = message.content.filter(block => block.type === "text")
      .map(block => block.type === "text" ? block.text : "").join("");
  }
  return text ? Object.freeze({ role, text }) : undefined;
}

/** Borrows one real native Session. This class owns only local call/presentation
 * custody, not the session, an install job, credentials, grants or persistence.
 */
export class NativeSetupConversation implements SetupConversationPort {
  readonly scope: string;
  readonly #session: Session;
  readonly #sessionId: string;
  readonly #signal: AbortSignal;
  readonly #revalidate: NativeSetupBinding["revalidate"];
  readonly #listeners = new Set<() => void>();
  readonly #stateListeners = new Set<() => void>();
  #retired = false;
  #uncertain = false;
  #active: ActiveSend | undefined;

  constructor(binding: NativeSetupBinding) {
    if (!binding.scope || !binding.session.id) throw new TypeError("Invalid setup binding");
    this.scope = binding.scope;
    this.#session = binding.session;
    this.#sessionId = binding.session.id;
    this.#signal = binding.invalidated;
    this.#revalidate = binding.revalidate;
    this.#signal.addEventListener("abort", () => this.#retire(), { once: true });
    if (this.#signal.aborted) this.#retire();
  }

  get available(): boolean { return !this.#retired && !this.#signal.aborted; }
  get canSend(): boolean { return this.available && !this.#active && !this.#uncertain; }
  get canStop(): boolean { return this.available && !!this.#active?.runId && !this.#active.stop; }
  get outcomeUncertain(): boolean { return this.#uncertain; }

  /** The host's permission invalidation clears all mounted presentations. */
  onUnavailable(listener: () => void): () => void {
    this.#listeners.add(listener);
    if (!this.available) this.#notify(listener);
    return () => { this.#listeners.delete(listener); };
  }

  /** Observes this owner's existing presentation state, never a second stream. */
  onStateChange(listener: () => void): () => void {
    if (!this.available) return () => {};
    this.#stateListeners.add(listener);
    return () => { this.#stateListeners.delete(listener); };
  }

  #stateChanged(): void {
    for (const listener of this.#stateListeners) this.#notify(listener);
  }

  #notify(listener: () => void): void {
    try { listener(); } catch { /* Presentation failure cannot control native work. */ }
  }

  #retire(): void {
    if (this.#retired) return;
    this.#retired = true;
    for (const listener of this.#listeners) this.#notify(listener);
    this.#listeners.clear();
    this.#stateListeners.clear();
  }

  async #check(): Promise<void> {
    if (!this.available) throw new SetupConversationError("unavailable");
    let allowed = false;
    try { allowed = await this.#revalidate(); } catch { /* No raw admission error. */ }
    if (!allowed || !this.available) {
      this.#retire();
      throw new SetupConversationError("unavailable");
    }
  }

  async #page(offset: number): Promise<SessionHistory> {
    await this.#check();
    const page = await this.#session.history({ offset, limit: PAGE_SIZE });
    await this.#check();
    if (page.sessionId !== this.#sessionId || page.offset !== offset ||
      !Number.isSafeInteger(page.messageCount) || page.messageCount < 0) {
      throw new SetupConversationError("history_unavailable");
    }
    return page;
  }

  /** Omitted offset selects the latest page. Paging never resends a message. */
  async history(offset?: number): Promise<SetupConversationHistory> {
    if (offset !== undefined && (!Number.isSafeInteger(offset) || offset < 0)) {
      throw new SetupConversationError("history_unavailable");
    }
    try {
      let page = await this.#page(offset ?? 0);
      if (offset === undefined && page.messageCount > PAGE_SIZE) {
        page = await this.#page(page.messageCount - PAGE_SIZE);
      }
      const messages = page.messages.map(textFrom).filter(message => message !== undefined);
      return Object.freeze({ messages: Object.freeze(messages), offset: page.offset,
        total: page.messageCount, hasMore: page.hasMore });
    } catch (error) {
      if (error instanceof SetupConversationError) throw error;
      throw new SetupConversationError("history_unavailable");
    }
  }

  /** Sends complete user text unchanged. A resolved call means this native turn
   * returned, not that authorization, installation or the original task finished.
   * An uncertain send cannot be replayed by this binding, including after remount.
   */
  async send(message: string, observe: (event: SetupConversationEvent) => void): Promise<void> {
    if (typeof message !== "string" || !message.trim()) throw new SetupConversationError("invalid_message");
    if (!this.available) throw new SetupConversationError("unavailable");
    if (this.#uncertain) throw new SetupConversationError("turn_uncertain");
    if (this.#active) throw new SetupConversationError("busy");
    const active: ActiveSend = { started: false, crossed: false };
    this.#active = active; // Reserve before asynchronous host admission.
    this.#stateChanged();
    let entered = false;
    let observer: typeof observe | undefined = observe;
    const emit = (event: SetupConversationEvent) => {
      if (!this.available || active.crossed || !observer) return;
      try { observer(Object.freeze(event)); } catch { observer = undefined; }
    };
    try {
      await this.#check();
      entered = true;
      const stream = this.#session.stream(message);
      if (stream.sessionId !== this.#sessionId) active.crossed = true;
      for await (const event of stream) {
        // Admission is owned by the host. This is its local invalidation signal,
        // not a new per-chunk policy call. Drain after retirement without display.
        if (!this.available || active.crossed) continue;
        this.#event(active, event, emit);
      }
      await this.#check();
      if (active.crossed || stream.result.sessionId !== this.#sessionId) {
        throw new SetupConversationError("turn_uncertain");
      }
    } catch (error) {
      if (entered) this.#uncertain = true;
      if (!this.available) throw new SetupConversationError("unavailable");
      if (error instanceof SetupConversationError && !entered) throw error;
      throw new SetupConversationError("turn_uncertain");
    } finally {
      if (this.#active === active) {
        this.#active = undefined;
        this.#stateChanged();
      }
    }
  }

  #event(active: ActiveSend, event: StreamEvent, emit: (event: SetupConversationEvent) => void): void {
    if (event.type === "run_started" || event.type === "run_completed" || event.type === "run_failed") {
      // StreamEvent deliberately includes UnknownEvent. Narrow display fields
      // instead of casting away the released SDK's unknown-event contract.
      const identity = event.identity;
      const runId = identity && typeof identity === "object" && "run_id" in identity &&
        typeof identity.run_id === "string" ? identity.run_id : undefined;
      if (event.sessionId !== this.#sessionId || (active.runId && runId && active.runId !== runId)) {
        active.crossed = true;
        delete active.runId;
        this.#stateChanged();
        return;
      }
      if (event.type === "run_started") {
        active.started = true;
        if (runId) active.runId = runId;
        emit({ kind: "run_observed" });
        this.#stateChanged();
      }
    } else if (active.started && event.type === "turn_started") {
      emit({ kind: "text_reset", ...(typeof event.assistantMessageId === "string" && event.assistantMessageId
        ? { messageId: event.assistantMessageId } : {}) });
    } else if (active.started && (event.type === "text_delta" || event.type === "text_complete")) {
      const message = typeof event.assistantMessageId === "string" && event.assistantMessageId
        ? { messageId: event.assistantMessageId } : {};
      if (event.type === "text_delta" && typeof event.delta === "string") {
        emit({ kind: "text_delta", text: event.delta, ...message });
      } else if (event.type === "text_complete" && "content" in event && typeof event.content === "string") {
        emit({ kind: "text_complete", text: event.content, ...message });
      }
    }
  }

  /** Stops only the run observed for this binding's still-active send. Native
   * not_current/not_stoppable receipts remain distinct and are never retried.
   */
  stopObservedRun(reason: string): Promise<"stopped" | "not_current" | "not_stoppable"> {
    const active = this.#active;
    if (!active?.runId || !this.available) return Promise.reject(new SetupConversationError("stop_unavailable"));
    if (active.stop) return active.stop;
    const runId = active.runId;
    active.stop = (async () => {
      await this.#check();
      if (this.#active !== active || active.crossed || active.runId !== runId) {
        throw new SetupConversationError("stop_unavailable");
      }
      try {
        const result = await this.#session.stopRun(runId, reason);
        await this.#check();
        if (result.session_id !== this.#sessionId || result.receipt.run_id !== runId) {
          throw new SetupConversationError("stop_uncertain");
        }
        return result.receipt.outcome;
      } catch (error) {
        if (error instanceof SetupConversationError) throw error;
        throw new SetupConversationError("stop_uncertain");
      }
    })();
    this.#stateChanged();
    return active.stop;
  }
}
