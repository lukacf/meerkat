/** Presentation values from an authenticated host. These are not authority tokens.
 * The host revalidates the current viewer, target, attempt and policy on dispatch.
 */
export interface OwnerReference {
    readonly owner: string;
    readonly reference: string;
    readonly revision: string;
}
export type ControlKind = "open_authorization" | "continue" | "stop_setup" | "cancel_task" | "change_account" | "retry" | "choose_strategy" | "reconnect" | "revoke_grant" | "disconnect" | "pause_frontend" | "resume_frontend" | "remove_frontend" | "review_activation";
/** Include only controls currently offered by the named owner. In particular,
 * an expired ceremony does not cause this library to manufacture a retry.
 */
export interface OwnerControl {
    /** Host-issued presentation offer, stable across replayed snapshots. A fresh
     * offer may reopen the same permitted ceremony without a new native attempt.
     * This is a correlation value, never permission or an idempotency guarantee.
     */
    readonly offer: string;
    readonly kind: ControlKind;
    readonly target: OwnerReference;
    readonly explanation?: string;
}
export interface StatusView {
    readonly tone: "pending" | "ready" | "attention" | "inactive";
    readonly text: string;
}
export interface ServiceAccessView {
    readonly kind: "service";
    readonly id: string;
    readonly label: string;
    readonly status: StatusView;
    readonly account: string;
    readonly usedBy: string;
    readonly operations: readonly string[];
    readonly resources: readonly string[];
    readonly lifetime: string;
    readonly standingAuthority?: string;
    readonly controls: readonly OwnerControl[];
}
export interface FrontendView {
    readonly kind: "frontend";
    readonly id: string;
    readonly label: string;
    readonly packageId: string;
    readonly status: StatusView;
    readonly endpoint: string;
    readonly audience: string;
    readonly target: string;
    readonly conversation: string;
    readonly output: string;
    readonly serviceAccess: string;
    readonly lifetime: string;
    readonly controls: readonly OwnerControl[];
}
/** Only safe host-selected presentation belongs in this shared surface.
 * Native auth URLs, codes, state and callbacks remain in a protected host presenter.
 */
export interface UserActionView {
    readonly id: string;
    readonly integration: string;
    readonly kind: "browser_authorization" | "device_authorization" | "secret_entry" | "interactive_setup" | "administrator_action";
    readonly state: "queued" | "action_required" | "verifying" | "complete" | "expired" | "no_longer_needed";
    readonly instructions: string;
    readonly expiresAt?: string;
    /** Canonical HTTP(S) origin only, not the native authorize URL or issuer proof. */
    readonly authorizationOrigin?: string;
    /** Requested account until its native owner reports verified account identity. */
    readonly requestedAccountLabel?: string;
    readonly controls: readonly OwnerControl[];
}
export interface SetupTaskView {
    readonly id: string;
    readonly title: string;
    readonly application: string;
    readonly requester: string;
    readonly reason: string;
    readonly phase: StatusView;
    readonly nextAction: string;
    readonly roles: readonly (ServiceAccessView | FrontendView)[];
    readonly userActions: readonly UserActionView[];
    readonly controls: readonly OwnerControl[];
    readonly diagnostics?: string;
}
export interface ConnectionView {
    readonly id: string;
    readonly packageId: string;
    readonly service: string;
    readonly account: string;
    /** Provider credential scope can be broader than the effective grants below. */
    readonly credentialScope: string;
    readonly status: StatusView;
    readonly grants: readonly ServiceAccessView[];
    readonly pendingTaskIds: readonly string[];
    readonly controls: readonly OwnerControl[];
    readonly diagnostics?: string;
}
export interface PackageView {
    readonly id: string;
    readonly label: string;
    readonly version: string;
    readonly status: StatusView;
    readonly exports: readonly {
        readonly kind: "service" | "frontend";
        /** Package export identity, separate from connection/frontend instance IDs. */
        readonly id: string;
        readonly label: string;
        readonly status: StatusView;
        /** Explicit host-projected connection IDs for a service export, or frontend
         * IDs for a frontend export. Every target must be present in this snapshot
         * and belong to this package. A same-role instance belongs to one export.
         * Omission preserves display-only legacy metadata; it never infers links.
         */
        readonly instanceIds?: readonly string[];
    }[];
    readonly components: readonly {
        readonly label: string;
        readonly status: StatusView;
    }[];
    readonly diagnostics?: string;
}
/** Inert catalog descriptions supplied by the authenticated host. These are
 * neither activated packages nor evidence of current connectivity or access.
 */
export interface CatalogCategoryView {
    readonly id: string;
    readonly label: string;
}
export interface CatalogEntryView {
    readonly id: string;
    readonly label: string;
    readonly description: string;
    readonly roles: readonly ("service" | "frontend")[];
    readonly categoryIds: readonly string[];
    readonly support: string;
    readonly prerequisites: readonly string[];
    readonly evidence?: string;
}
export interface CatalogView {
    readonly categories: readonly CatalogCategoryView[];
    readonly entries: readonly CatalogEntryView[];
}
/** A complete projection, not a patch. Revision strictly increases within one
 * mounted scope. Reopening the view uses the host's durable read, not local state.
 */
export interface ToolkitSnapshot {
    readonly scope: string;
    readonly revision: number;
    readonly observedAt: string;
    /** Concise changed-status announcement, including changes beyond the first task. */
    readonly announcement: string;
    readonly tasks: readonly SetupTaskView[];
    readonly connections: readonly ConnectionView[];
    readonly frontends: readonly FrontendView[];
    readonly packages: readonly PackageView[];
    readonly catalog?: CatalogView;
}
export interface ControlRequest {
    readonly scope: string;
    readonly viewRevision: number;
    readonly control: OwnerControl;
}
export interface ToolkitViewOptions {
    readonly scope: string;
    /** Admission acknowledgement only. Neither success nor rejection changes
     * role, connection, ceremony or task state in the view.
     */
    readonly dispatch: (request: ControlRequest) => Promise<void>;
    /** Trusted host UI navigation only. Must not begin authentication, install,
     * approve work or create an attempt. The callback receives no authority.
     * Its completion does not change the owner-provided snapshot.
     */
    readonly onManualConfiguration?: () => void | Promise<void>;
}
