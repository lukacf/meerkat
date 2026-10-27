import { renderSetupMarkdown } from "./setup-markdown.js";
import { SetupConversationError } from "./setup-model.js";
/** Presentation for an authenticated host embedding. A web host must provide
 * its admitted application bridge; a privileged Session is not browser data.
 * Assistant text supports Markdown with inert raw HTML, absolute HTTP(S) links
 * and text-only images. Copy message preserves the exact original source.
 * User messages stay literal. Empty display rows do not change native paging.
 */
export function mountSetupConversation(container, options) {
    const conversation = options.conversation;
    if (!options.scope || options.scope !== conversation.scope)
        throw new TypeError("Setup scope mismatch");
    const doc = container.ownerDocument;
    const root = doc.createElement("section");
    root.className = "toolkit-view toolkit-conversation";
    root.setAttribute("aria-label", "Setup conversation");
    const heading = doc.createElement("h2");
    heading.textContent = "Talk with the setup agent";
    const help = doc.createElement("p");
    help.textContent = "Describe what you need or answer the agent's questions. Sending a message does not approve an installation. Use the host's sign-in and approval controls when offered.";
    const status = doc.createElement("p");
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    const history = doc.createElement("ol");
    history.setAttribute("aria-label", "Saved conversation");
    const admitted = doc.createElement("div");
    admitted.setAttribute("aria-label", "Message accepted for current run");
    const live = doc.createElement("div");
    live.setAttribute("aria-label", "Agent response in progress");
    const paging = doc.createElement("div");
    paging.className = "toolkit-controls";
    const previous = doc.createElement("button");
    previous.type = "button";
    previous.textContent = "Earlier messages";
    const next = doc.createElement("button");
    next.type = "button";
    next.textContent = "Later messages";
    const refresh = doc.createElement("button");
    refresh.type = "button";
    refresh.textContent = "Refresh history";
    paging.append(previous, next, refresh);
    const form = doc.createElement("form");
    const label = doc.createElement("label");
    label.textContent = "Message the setup agent";
    const input = doc.createElement("textarea");
    input.rows = 4;
    input.autocomplete = "off";
    label.append(input);
    const controls = doc.createElement("div");
    controls.className = "toolkit-controls";
    const send = doc.createElement("button");
    send.type = "submit";
    send.textContent = "Send message";
    const stop = doc.createElement("button");
    stop.type = "button";
    stop.textContent = "Stop this agent run";
    controls.append(send, stop);
    form.append(label, controls);
    root.append(heading, help, status, history, admitted, live, paging, form);
    container.replaceChildren(root);
    let disposed = false, pending = false, loading = false, historyReady = false;
    // Presentation transition only; the port remains the owner of active work.
    const activeSend = () => conversation.available && !conversation.canSend &&
        !conversation.outcomeUncertain && !conversation.privateNoticePending;
    let observedBusy = activeSend();
    let observedPrivateNotice = conversation.privateNoticePending === true;
    let generation = 0, offset = 0, hasMore = false;
    const drafts = new Map();
    let draftRevision = 0;
    input.addEventListener("input", () => { draftRevision += 1; });
    function updateControls() {
        send.disabled = disposed || loading || !historyReady || !conversation.canSend || pending;
        input.readOnly = disposed || !conversation.available || conversation.outcomeUncertain;
        stop.disabled = disposed || !conversation.canStop;
        refresh.disabled = disposed || !conversation.available || loading || pending;
        previous.disabled = refresh.disabled || offset === 0;
        next.disabled = refresh.disabled || !hasMore;
    }
    function safeError(error) {
        return error instanceof SetupConversationError ? error.message : "The setup conversation is unavailable. Check it through the host.";
    }
    function savedStatus(completedSend = false) {
        if (conversation.outcomeUncertain)
            return "History read. The earlier message outcome still requires host reconciliation. It has not been retried.";
        if (conversation.privateNoticePending)
            return "A private setup notice is waiting in the host's controls. Review and dismiss it before sending another message.";
        if (completedSend)
            return "Response received. Complete any requested sign-in or approval to continue setup.";
        return activeSend()
            ? "Sending is paused. Check the protected setup panel or current host status."
            : "Saved conversation loaded. You can send a message to the setup agent.";
    }
    function failedHistoryStatus(completedSend = false) {
        if (conversation.outcomeUncertain)
            return "Conversation history is unavailable. The earlier message outcome still requires host reconciliation. It has not been retried.";
        if (conversation.privateNoticePending)
            return "Conversation history is unavailable. A private setup notice is still waiting in the host's controls.";
        return completedSend
            ? "Your message was sent, but saved conversation could not be refreshed. Use Refresh history before sending another message."
            : "Conversation history is unavailable. Refresh history to try again.";
    }
    function clearUnavailable() {
        generation += 1;
        history.replaceChildren();
        admitted.replaceChildren();
        live.replaceChildren();
        drafts.clear();
        input.value = "";
        historyReady = false;
        status.textContent = "This setup conversation is no longer available in this view.";
        updateControls();
    }
    const detach = conversation.onUnavailable(clearUnavailable);
    const detachState = conversation.onStateChange(() => {
        if (disposed || !conversation.available)
            return;
        const busy = activeSend();
        const notice = conversation.privateNoticePending === true;
        const noticeChanged = notice !== observedPrivateNotice;
        const settled = observedBusy && !busy;
        observedBusy = busy;
        observedPrivateNotice = notice;
        updateControls();
        if (pending)
            return; // The original sending view owns its post-send read.
        if (settled) {
            // Supersede any older in-flight read through the existing generation fence.
            void read();
        }
        else if (busy && !loading) {
            status.textContent = "Sending is paused. Check the protected setup panel or current host status.";
        }
        else if (noticeChanged && !loading) {
            // Dismissal changes presentation and the owner's Send availability only.
            // It cannot repair a failed history read or resolve an uncertain outcome.
            status.textContent = historyReady ? savedStatus() : failedHistoryStatus();
        }
    });
    async function read(offsetToRead, completedSend = false) {
        if (disposed || !conversation.available)
            return;
        const readGeneration = ++generation;
        loading = true;
        updateControls();
        status.textContent = "Reading saved conversation...";
        try {
            const page = await conversation.history(offsetToRead);
            if (disposed || readGeneration !== generation || !conversation.available)
                return;
            const items = page.messages.filter(message => message.text.trim()).map(message => {
                const item = doc.createElement("li");
                const name = doc.createElement("strong");
                name.textContent = message.role === "user" ? "You" : "Setup agent";
                const text = doc.createElement(message.role === "assistant" ? "div" : "p");
                text.className = message.role === "assistant" ? "toolkit-markdown" : "toolkit-plain-message";
                if (message.role === "assistant")
                    renderSetupMarkdown(text, message.text);
                else
                    text.textContent = message.text;
                const copy = doc.createElement("button");
                copy.type = "button";
                copy.textContent = "Copy message";
                copy.setAttribute("aria-label", `Copy ${message.role === "user" ? "your" : "setup agent"} message`);
                copy.addEventListener("click", () => {
                    if (disposed || !conversation.available || !history.contains(item))
                        return;
                    const clipboard = doc.defaultView?.navigator.clipboard;
                    if (!clipboard) {
                        copy.textContent = "Select text to copy";
                        return;
                    }
                    void (async () => clipboard.writeText(message.text))().then(() => {
                        if (!disposed && conversation.available && history.contains(item))
                            copy.textContent = "Copied";
                    }, () => {
                        if (!disposed && conversation.available && history.contains(item))
                            copy.textContent = "Select text to copy";
                    });
                });
                item.append(name, text, copy);
                return item;
            });
            history.replaceChildren(...items);
            admitted.replaceChildren();
            live.replaceChildren();
            drafts.clear();
            offset = page.offset;
            hasMore = page.hasMore;
            historyReady = true;
            status.textContent = savedStatus(completedSend);
        }
        catch (error) {
            if (!disposed && readGeneration === generation) {
                historyReady = false;
                let message = safeError(error);
                if (error instanceof SetupConversationError && error.code === "history_unavailable") {
                    message = failedHistoryStatus(completedSend);
                }
                status.textContent = message;
            }
        }
        finally {
            if (!disposed && readGeneration === generation) {
                loading = false;
                updateControls();
            }
        }
    }
    function show(event, sendGeneration) {
        if (disposed || generation !== sendGeneration || !conversation.available)
            return;
        if (event.kind === "run_observed") {
            status.textContent = "The setup agent is responding.";
        }
        else {
            const key = event.messageId ?? "unidentified-current-message";
            let draft = drafts.get(key);
            if (!draft) {
                draft = { element: doc.createElement("div"), source: "" };
                draft.element.className = "toolkit-markdown";
                drafts.set(key, draft);
            }
            draft.source = event.kind === "text_reset" ? ""
                : event.kind === "text_complete" ? event.text : draft.source + event.text;
            if (draft.source.trim()) {
                renderSetupMarkdown(draft.element, draft.source);
                if (!live.contains(draft.element))
                    live.append(draft.element);
            }
            else
                draft.element.remove();
        }
        updateControls();
    }
    form.addEventListener("submit", event => {
        event.preventDefault();
        if (send.disabled || pending || disposed)
            return;
        const message = input.value;
        if (!message.trim()) {
            status.textContent = "Enter a nonempty message.";
            input.focus();
            return;
        }
        pending = true;
        const sendGeneration = ++generation;
        const sentRevision = draftRevision;
        let observed = false;
        const clearAcceptedDraft = () => {
            if (observed)
                return;
            observed = true;
            const name = doc.createElement("strong");
            name.textContent = "You (sent)";
            const text = doc.createElement("p");
            text.className = "toolkit-plain-message";
            text.textContent = message;
            admitted.replaceChildren(name, text);
            if (draftRevision === sentRevision && input.value === message)
                input.value = "";
        };
        admitted.replaceChildren();
        live.replaceChildren();
        drafts.clear();
        status.textContent = "Sending your message...";
        updateControls();
        void (async () => {
            try {
                await conversation.send(message, event => {
                    if (disposed || generation !== sendGeneration || !conversation.available)
                        return;
                    if (event.kind === "run_observed")
                        clearAcceptedDraft();
                    show(event, sendGeneration);
                });
                if (disposed || generation !== sendGeneration || !conversation.available)
                    return;
                // Compatibility for ports that resolve an exact native turn without
                // streaming a run event. Never clear a draft edited in the meantime.
                clearAcceptedDraft();
                pending = false;
                await read(undefined, true);
            }
            catch (error) {
                if (!disposed && generation === sendGeneration)
                    status.textContent = safeError(error);
            }
            finally {
                if (!disposed) {
                    pending = false;
                    updateControls();
                }
            }
        })();
    });
    stop.addEventListener("click", () => {
        if (stop.disabled || disposed)
            return;
        const stopGeneration = generation;
        const stopping = conversation.stopObservedRun("User selected Stop this agent run in the setup conversation");
        updateControls();
        void stopping.then(outcome => {
            if (disposed || generation !== stopGeneration || !conversation.available)
                return;
            status.textContent = outcome === "stopped" ? "The agent stopped. Checking the final result."
                : outcome === "not_current" ? "That run is no longer current. No other run was stopped."
                    : "The agent cannot be stopped at this point.";
        }, error => {
            if (!disposed && generation === stopGeneration)
                status.textContent = safeError(error);
        }).finally(() => { if (!disposed)
            updateControls(); });
    });
    previous.addEventListener("click", () => { if (!previous.disabled)
        void read(Math.max(0, offset - 50)); });
    next.addEventListener("click", () => { if (!next.disabled)
        void read(offset + 50); });
    refresh.addEventListener("click", () => { if (!refresh.disabled)
        void read(); });
    updateControls();
    void read();
    return {
        async refresh() { if (!pending)
            await read(); },
        dispose() {
            if (disposed)
                return;
            disposed = true;
            generation += 1;
            detach();
            detachState();
            drafts.clear();
            root.remove();
        },
    };
}
