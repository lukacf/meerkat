import type { ToolkitSnapshot, ToolkitViewOptions } from "./model.js";
export interface ToolkitView {
    /** Returns false for an old/equal revision. A different viewer scope throws.
     * This ordering protects presentation only; the host still authorizes every action.
     */
    update(snapshot: ToolkitSnapshot): boolean;
    /** Removes the view. Never cancels setup, auth, a task or an in-flight dispatch. */
    dispose(): void;
}
/** Framework-free UI over owner-provided projections. No network, storage,
 * secret fields, browser automation, timers, lifecycle transitions or retries.
 */
export declare function mountToolkitView(container: HTMLElement, options: ToolkitViewOptions): ToolkitView;
