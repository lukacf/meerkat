import type { SetupConversationPort } from "./setup-model.js";
export interface SetupConversationView {
    /** Reads native history only. It never retries an uncertain message. */
    refresh(): Promise<void>;
    /** Detaches presentation only, without stopping or closing the host Session. */
    dispose(): void;
}
/** Presentation for an authenticated host embedding. A web host must provide
 * its admitted application bridge; a privileged Session is not browser data.
 * Assistant text supports Markdown with inert raw HTML, absolute HTTP(S) links
 * and text-only images. Copy message preserves the exact original source.
 * User messages stay literal. Empty display rows do not change native paging.
 */
export declare function mountSetupConversation(container: HTMLElement, options: {
    readonly scope: string;
    readonly conversation: SetupConversationPort;
}): SetupConversationView;
