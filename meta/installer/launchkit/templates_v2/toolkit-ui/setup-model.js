/** Safe presentation errors intentionally omit raw native/provider details. */
export class SetupConversationError extends Error {
    code;
    constructor(code) {
        const messages = {
            unavailable: "This setup conversation is no longer available in this view.",
            busy: "A message from this view is still being handled.",
            invalid_message: "Enter a nonempty message.",
            history_unavailable: "Conversation history is unavailable. Refresh history to try again.",
            turn_uncertain: "The message outcome is unconfirmed. The host must reconcile it before another message is sent.",
            stop_unavailable: "There is no current observed run to stop from this view.",
            stop_uncertain: "The stop outcome is unconfirmed. Check the native run through the host.",
        };
        super(messages[code]);
        this.code = code;
        this.name = "SetupConversationError";
    }
}
