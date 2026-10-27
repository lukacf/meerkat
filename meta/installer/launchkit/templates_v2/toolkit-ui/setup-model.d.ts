export interface SetupConversationMessage {
    readonly role: "user" | "assistant";
    readonly text: string;
}
export interface SetupConversationHistory {
    readonly messages: readonly SetupConversationMessage[];
    readonly offset: number;
    readonly total: number;
    readonly hasMore: boolean;
}
export type SetupConversationEvent = {
    readonly kind: "run_observed";
} | {
    readonly kind: "text_reset";
    readonly messageId?: string;
} | {
    readonly kind: "text_delta";
    readonly text: string;
    readonly messageId?: string;
} | {
    readonly kind: "text_complete";
    readonly text: string;
    readonly messageId?: string;
};
export type SetupConversationErrorCode = "unavailable" | "busy" | "invalid_message" | "history_unavailable" | "turn_uncertain" | "stop_unavailable" | "stop_uncertain";
/** Safe presentation errors intentionally omit raw native/provider details. */
export declare class SetupConversationError extends Error {
    readonly code: SetupConversationErrorCode;
    constructor(code: SetupConversationErrorCode);
}
/** Safe view-facing methods. An application's authenticated bridge may provide
 * this shape; it never transfers a native Session or invents native authority.
 */
export interface SetupConversationPort {
    readonly scope: string;
    /** The owner controls send admission. While available, !canSend does not
     * prove a send is active: work or a private decision may pause sending.
     * outcomeUncertain requires host reconciliation; UI history reads do not
     * change send admission. Admission loss makes the port unavailable.
     */
    readonly available: boolean;
    readonly canSend: boolean;
    readonly canStop: boolean;
    readonly outcomeUncertain: boolean;
    /** Optional display-only projection for a completed send awaiting private
     * notice dismissal. A v2 stdio bridge maps private_notice_pending directly,
     * together with active_send:null and can_send/can_stop:false. It must publish
     * the coherent getters before notifying onStateChange. No notice body or
     * dismissal authority is exposed here. V1 ports omit this property.
     */
    readonly privateNoticePending?: boolean;
    onUnavailable(listener: () => void): () => void;
    /** Read-only presentation invalidation. Emits no data and opens no stream.
     * Hosts notify after these getters change, including an existing send's
     * settlement while its original view is unmounted.
     */
    onStateChange(listener: () => void): () => void;
    history(offset?: number): Promise<SetupConversationHistory>;
    send(message: string, observe: (event: SetupConversationEvent) => void): Promise<void>;
    stopObservedRun(reason: string): Promise<"stopped" | "not_current" | "not_stoppable">;
}
