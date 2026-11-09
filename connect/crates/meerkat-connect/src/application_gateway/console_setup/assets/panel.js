import { validateProjection } from "./projection.js";
import { mountToolkitView } from "./view.js";
export const SOURCE_STATUS_DATA_PATH = "/host/toolkit/setup/data";
const presentationIds = new WeakMap();
function labelManagementAction(button, context) {
    const doc = button.ownerDocument;
    const ids = [button, ...context].map(node => {
        let next = presentationIds.get(doc) ?? 0;
        let id;
        do { id = `toolkit-setup-label-${++next}`; } while (doc.getElementById(id));
        node.id = id;
        presentationIds.set(doc, next);
        return id;
    });
    // Use visible facts, never an opaque offer/row key or inferred authority.
    button.setAttribute("aria-labelledby", ids.join(" "));
}
function checkedPage(value) {
    if (value === undefined) return undefined; // Earlier whole-view envelopes.
    if (!value || Object.keys(value).some(key => !["page_id", "first", "has_next"].includes(key))
        || !Number.isSafeInteger(value.page_id) || value.page_id < 1
        || typeof value.first !== "boolean" || typeof value.has_next !== "boolean") throw new TypeError("Invalid status page");
    return { page_id: value.page_id, first: value.first, has_next: value.has_next };
}
/** One current page, replaced after every authenticated read. Page positions
 * carry no principal, grant or approval and are discarded on view retirement.
 */
export function mountSourceStatusPanel(container, read, account = { clear() {}, update() {} }, access = { clear() {}, update() {} }, submission = { sending: false }, recovery = { clear() {}, update() {} }) {
    const doc = container.ownerDocument;
    const status = doc.createElement("p");
    status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
    const content = doc.createElement("div");
    const refresh = doc.createElement("button"); refresh.type = "button"; refresh.textContent = "Refresh status";
    const first = doc.createElement("button"); first.type = "button"; first.textContent = "First page";
    const next = doc.createElement("button"); next.type = "button"; next.textContent = "Next page";
    const navigation = doc.createElement("div"); navigation.append(first, next);
    const root = doc.createElement("section"); root.append(status, refresh, content); container.append(root);
    let request = 0; let disposed = false; let view; let page; let pending; let needsFirst = false;
    let layout;
    const releaseLayout = () => {
        if (!layout) return;
        container.style.setProperty("min-height", layout.value, layout.priority); layout = undefined;
    };
    const retainLayout = () => {
        const height = container.getBoundingClientRect().height;
        if (!Number.isFinite(height) || height <= 0) return;
        layout = { value: container.style.getPropertyValue("min-height"), priority: container.style.getPropertyPriority("min-height") };
        // Keep only empty layout space while the old private DOM is removed.
        // Shrinking the document during the read would clamp its scroll position.
        container.style.setProperty("min-height", `${height}px`);
    };
    const clear = (forRead = false) => {
        view?.dispose(); view = undefined; content.replaceChildren();
        if (forRead && typeof account.suspend === "function") account.suspend(); else account.clear();
        if (forRead && typeof access.suspend === "function") access.suspend(); else access.clear();
        if (forRead && typeof recovery.suspend === "function") recovery.suspend(); else recovery.clear();
    };
    const buttons = busy => {
        refresh.disabled = busy || needsFirst;
        first.disabled = busy;
        next.disabled = busy || needsFirst || page?.has_next !== true;
        if (page || needsFirst) root.insertBefore(navigation, content); else navigation.remove();
    };
    function update(direction = "refresh") {
        if (disposed) return Promise.resolve();
        // The same-login read replaces the server's current access offer.
        // Keep it until an entered protected POST settles, including manual
        // navigation and history restoration. This flag grants no authority.
        if (submission.sending) {
            status.textContent = "Waiting for the current setup response before refreshing status.";
            buttons(true);
            return Promise.resolve();
        }
        if (pending) return pending; // Poll/manual navigation share one entered read.
        if (needsFirst && direction !== "first") return Promise.resolve();
        if (direction === "next" && page?.has_next !== true) return Promise.resolve();
        const selection = direction === "first" || !page ? undefined : { page_id: page.page_id, direction };
        const current = ++request;
        retainLayout();
        clear(true); status.textContent = "Reading current task setup status..."; buttons(true);
        // Start in a microtask so even a synchronous throw cannot race assignment
        // of the shared pending read. No later poll creates another request.
        pending = Promise.resolve().then(async () => {
            try {
                if (disposed || current !== request) return;
                const envelope = await read(selection);
                if (disposed || current !== request) return;
                const snapshot = envelope.projection; const selected = checkedPage(envelope.page);
                validateProjection(snapshot);
                if (snapshot.tasks.some(task => task.controls.length || task.userActions.length || task.roles.length)
                    || snapshot.connections.length || snapshot.frontends.length || snapshot.packages.length
                    || snapshot.catalog !== undefined) throw new TypeError("The source status panel only accepts read-only task status");
                // A page response is a full replacement within this mount, not
                // a patch or a claim of complete inventory across requests.
                view = mountToolkitView(content, { scope: snapshot.scope, taskOnly: true, taskPage: selected !== undefined,
                    dispatch: async () => { throw new Error("This panel has no setup actions"); } });
                view.update(snapshot);
                account.update(envelope.review, snapshot.scope);
                access.update(envelope.application_access, snapshot.scope, selected !== undefined);
                recovery.update(envelope.operator_recovery, snapshot.scope, selected !== undefined);
                page = selected; needsFirst = false;
                status.textContent = selected ? selected.has_next
                    ? "Showing one page. More history remains to inspect; later pages may have no matching entries."
                    : "End of the history observed in this read. Use First page to check for new entries." : "";
            } catch (error) {
                if (disposed || current !== request) return;
                clear();
                if (error?.status === 503) {
                    needsFirst = false;
                    // The host disclosed no page. Retain only its opaque position
                    // for the existing next status poll, never an action or view.
                    status.textContent = "Task setup status is temporarily unavailable. The next status refresh will check again.";
                } else {
                    page = undefined; needsFirst = true;
                    status.textContent = "Task setup status is unavailable. Use First page to read current status. One status view is current per sign-in; another tab may have replaced this page. Your agent reports the task result separately.";
                }
            }
        }).finally(() => { releaseLayout(); pending = undefined; if (!disposed) buttons(false); });
        return pending;
    }
    refresh.addEventListener("click", () => { void update(); });
    first.addEventListener("click", () => { void update("first"); });
    next.addEventListener("click", () => { void update("next"); });
    return { refresh: () => update(), first: () => update("first"), next: () => update("next"),
        dispose() { disposed = true; ++request; page = undefined; releaseLayout(); clear(); root.remove(); } };
}
/** Same-origin authentication remains with the actual host on every read. */
export async function readSourceStatus(fetcher = fetch, selection) {
    if (selection !== undefined && (!selection || Object.keys(selection).some(key => !["page_id", "direction"].includes(key))
        || !Number.isSafeInteger(selection.page_id) || selection.page_id < 1
        || !["refresh", "next"].includes(selection.direction))) throw new TypeError("Invalid page selection");
    const controller = new AbortController();
    let timer;
    const deadline = new Promise((_, reject) => {
        timer = setTimeout(() => {
            // Abort only this status observation, never setup or a protected
            // action. A late server result cannot establish an effect outcome.
            controller.abort();
            reject(new Error("Task setup status read timed out"));
        }, 30000);
    });
    try {
        const observation = (async () => {
            const response = await fetcher(SOURCE_STATUS_DATA_PATH, {
                method: selection ? "POST" : "GET", credentials: "same-origin", cache: "no-store", redirect: "error",
                headers: selection ? { Accept: "application/json", "Content-Type": "application/json" } : { Accept: "application/json" },
                ...(selection ? { body: JSON.stringify(selection) } : {}), signal: controller.signal,
            });
            if (controller.signal.aborted) throw new Error("Task setup status read expired");
            if (!response.ok) {
                const error = new Error("Task setup status read was not authorized or available");
                error.status = response.status;
                throw error;
            }
            if (!response.headers.get("content-type")?.startsWith("application/json")) throw new Error("Task setup status read was not authorized or available");
            const envelope = await response.json();
            if (controller.signal.aborted) throw new Error("Task setup status read expired");
            validateProjection(envelope.projection); checkedPage(envelope.page);
            return envelope;
        })();
        // One deadline covers both headers and body. The race observes late
        // rejection too; only its first result can reach the mounted panel.
        return await Promise.race([observation, deadline]);
    } finally {
        clearTimeout(timer);
    }
}
// This transport projects the existing retained prompt. It creates no task,
// native approval or account selector and stores no bearer or callback query.
export function mountAccountReview(container, refresh, fetcher = fetch, submission = { sending: false }) {
    const document = container.ownerDocument;
    const root = document.createElement("section");
    root.className = "toolkit-view toolkit-account-review";
    container.append(root);
    let epoch = 0;
    let disposed = false;
    let rendered;
    let hint;
    const movedFocus = event => {
        if (hint && event.target !== document.body && event.target !== document.documentElement)
            hint.focus = undefined;
    };
    document.addEventListener("focusin", movedFocus);
    const clearPrivateFields = () => {
        for (const input of root.querySelectorAll('input[type="password"]')) input.value = "";
    };
    const clear = () => { ++epoch; rendered = undefined; hint = undefined; clearPrivateFields(); root.replaceChildren(); };
    return {
        hasPrivateInput() {
            // Reuse only the native review's expiry. This controls presentation,
            // never approval authority; the next normal read clears stale input.
            if (Number.isFinite(rendered?.credentialExpiresAt) && Date.now() >= rendered.credentialExpiresAt) return false;
            return [...root.querySelectorAll('input[type="password"]')]
                .some(input => input.value.length > 0 || input === document.activeElement);
        },
        suspend() {
            if (disposed) return;
            if (rendered) {
                hint = { identity: rendered.identity, open: rendered.details?.open === true,
                    focus: [...rendered.controls].find(([, element]) => element === document.activeElement)?.[0] };
            }
            // Retain only presentation scalars, never the previous prompt or DOM.
            // An overlapping read must not replace the first hint with empty DOM.
            ++epoch; rendered = undefined; clearPrivateFields(); root.replaceChildren();
        },
        update(review, scope) {
            if (disposed) return;
            const current = ++epoch;
            rendered = undefined;
            clearPrivateFields();
            root.replaceChildren();
            if (review == null) { hint = undefined; return; }
            if (!Number.isSafeInteger(review.review_id) || review.review_id < 1
                || typeof review.can_start !== "boolean") throw new TypeError("Invalid review");
            const controls = new Map();
            const present = (kind, approvalId, details) => {
                const previous = hint; hint = undefined;
                if (typeof scope !== "string" || !scope.length) return;
                const identity = JSON.stringify([scope, review.review_id, kind, approvalId]);
                rendered = { identity, details, controls,
                    credentialExpiresAt: review.credential_input == null ? undefined : Date.parse(review.prompt?.expires_at) };
                if (previous?.identity !== identity) return;
                if (details) details.open = previous.open;
                const focus = controls.get(previous.focus);
                if (focus && !focus.disabled && document.activeElement === document.body)
                    focus.focus({ preventScroll: true });
            };
            const status = document.createElement("p"); status.setAttribute("role", "status");
            status.textContent = "Service setup needs your attention."; root.append(status);
            const send = async (path, body) => {
                if (disposed || submission.sending || epoch !== current) return;
                submission.sending = true;
                for (const control of root.querySelectorAll("button, input")) control.disabled = true;
                try {
                    const result = await fetcher(path, { method: "POST", credentials: "same-origin",
                        cache: "no-store", redirect: "error", headers: { "Content-Type": "application/json" },
                        body: JSON.stringify(body) });
                    if (!result.ok) throw new Error("Response unavailable");
                    if (epoch === current) status.textContent = "Response forwarded. Checking the current setup state...";
                } catch {
                    if (epoch === current) status.textContent = "The response could not be confirmed. Refresh setup status before continuing.";
                }
                // Release busy only after the entered POST settles. A live view
                // refreshes; a retired view cannot refresh its replacement.
                // The replacement's next read presents the current busy state.
                submission.sending = false;
                if (!disposed) await refresh();
            };
            if (review.can_start) {
                const start = document.createElement("button"); start.type = "button";
                start.disabled = submission.sending; controls.set("start", start);
                start.textContent = "Start service setup";
                start.addEventListener("click", () => { void send("/host/toolkit/setup/start", { review_id: review.review_id }); });
                root.append(start); present("start", null); return;
            }
            const prompt = review.prompt;
            if (prompt == null) { hint = undefined; status.textContent = "Setup is working. Refresh for the next step."; return; }
            if (prompt.kind === "provider_browser") {
                // This prompt has no stable approval ID. Do not retain its URL
                // as a presentation key or carry focus from an earlier prompt.
                hint = undefined;
                const url = new URL(prompt.authorization_url);
                if (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)))
                    throw new TypeError("Invalid provider URL");
                const link = document.createElement("a"); link.href = url.href;
                link.target = "_blank"; link.rel = "noopener noreferrer";
                link.textContent = "Open provider sign-in"; root.append(link);
                const cancel = document.createElement("button"); cancel.type = "button";
                cancel.textContent = "Cancel sign-in"; cancel.disabled = submission.sending;
                controls.set("cancel-sign-in", cancel);
                cancel.addEventListener("click", () => { void send("/host/toolkit/setup/cancel-sign-in", {
                    review_id: review.review_id }); });
                root.append(cancel); return;
            }
            if (prompt.kind !== "approval" || typeof prompt.approval_id !== "string"
                || !Array.isArray(prompt.allowed_decisions)) throw new TypeError("Invalid prompt");
            const suppliedSummary = typeof prompt.summary === "string" && prompt.summary.trim().length > 0;
            const heading = document.createElement("h2");
            heading.textContent = suppliedSummary ? prompt.summary : "Review the requested access";
            root.append(heading);
            if (!suppliedSummary) {
                const missing = document.createElement("p");
                missing.textContent = "No additional description was supplied. Review the exact details and requested access below.";
                root.append(missing);
            }
            const nestedPhase = prompt.body?.access_request?.phase;
            const body = ["installed_account_access", "installed_public_service_access", "installed_credential_service_access", "installed_public_service_batch_access"].includes(nestedPhase)
                ? prompt.body.access_request : prompt.body;
            const facts = document.createElement("dl");
            const fact = (label, value, destination = facts) => {
                if (typeof value !== "string" || !value.length) return;
                const row = document.createElement("div");
                const term = document.createElement("dt"); term.textContent = label;
                const detail = document.createElement("dd"); detail.textContent = value;
                row.append(term, detail); destination.append(row);
            };
            const publicBatch = body?.phase === "installed_public_service_batch_access"
                && ["public_services_v1", "public_services_with_setup_v2"].includes(prompt.body?.kind) && Array.isArray(prompt.body.members);
            let batchDetails, buildDetails;
            if (publicBatch) {
                const members = prompt.body.members;
                const policy = body.member_tool_policy;
                fact("Reviewed service set", `${members.length} services`);
                fact("Agent", policy?.proposal?.member);
                fact("New instruction", body.service_instruction?.proposed_fresh_instruction);
                fact("Instruction origin", body.service_instruction?.instruction_origin);
                fact("Live attachment", body.service_instruction?.live_attachment_effect);
                fact("Configuration persistence", body.service_instruction?.configuration_persistence);
                batchDetails = document.createElement("div");
                const section = (title, className) => {
                    const section = document.createElement("section"); section.className = className;
                    const heading = document.createElement("h3"); heading.textContent = title;
                    const rows = document.createElement("dl"); section.append(heading, rows); batchDetails.append(section);
                    return rows;
                };
                const accessLimit = (lifetime, rows) => {
                    const task = lifetime?.scope?.kind === "task";
                    if (task) fact("Access scope", "This task", rows);
                    const expires = lifetime?.expires_at_ms;
                    const deadline = Number.isSafeInteger(expires) && expires > 0 ? new Date(expires) : null;
                    if (deadline && !Number.isNaN(deadline.valueOf())) fact("Access expires", deadline.toISOString(), rows);
                    if (!task || !deadline || Number.isNaN(deadline.valueOf()))
                        fact("Access limit", "Reviewed access limit unavailable; inspect the exact review details", rows);
                };
                for (const [index, member] of members.entries()) {
                    const rows = section(`Service ${index + 1}`, "toolkit-batch-member");
                    const parent = member?.parent;
                    const document = parent?.access_request;
                    fact("Connector", member?.server, rows);
                    fact("Profile", document?.software?.profile, rows);
                    const remote = document?.operation_basis?.kind === "remote_recipe_declarations";
                    fact("Operation source", remote ? "Researched remote recipe declarations"
                        : ["selected_manifest_declarations", "selected_software_declarations"].includes(document?.operation_basis?.kind)
                            ? "Selected package declarations" : document?.operation_basis?.kind === "package_published_metadata"
                                ? "Package publisher metadata" : "Operation provenance is unconfirmed", rows);
                    if (remote) {
                        const exported = document.remote_export?.export;
                        const name = exported?.declared_remote_service?.mcp_name;
                        fact("Declared remote connector", name, rows);
                        fact("Endpoint", exported?.mcp_configuration?.mcpServers?.[name]?.url, rows);
                    }
                    if (document?.authentication?.kind === "none")
                        fact("Authentication", "No sign-in required by the selected service declaration", rows);
                    // Use the reviewed native permission, not the proposal TTL
                    // or an alias from the helper's application selection.
                    const permission = parent?.service_access?.permission;
                    if (Array.isArray(permission?.operations) && permission.operations.every(name => typeof name === "string"))
                        fact("Reviewed operations", permission.operations.join(", "), rows);
                    accessLimit(permission?.lifetime, rows);
                }
                const catalog = prompt.body.catalog?.catalog_access;
                const catalogRows = section("Tool catalog", "toolkit-batch-catalog");
                fact("Session", catalog?.session, catalogRows);
                fact("Catalog visibility", policy?.catalog_visibility, catalogRows);
                accessLimit(catalog?.lifetime, catalogRows);
                if (prompt.body.kind === "public_services_with_setup_v2") {
                    const setup = body.configured_setup;
                    const setupRows = section("Later integration setup", "toolkit-batch-setup");
                    fact("Setup authority", setup?.authority, setupRows);
                    fact("Catalog scope", setup?.catalog_scope, setupRows);
                    if (Array.isArray(setup?.processors))
                        fact("Catalog agents", setup.processors.map(value => JSON.stringify(value)).join(", "), setupRows);
                    if (Array.isArray(setup?.sessions) && setup.sessions.every(value => typeof value === "string"))
                        fact("Catalog sessions", setup.sessions.join(", "), setupRows);
                    if (setup?.audience) fact("Catalog audience", JSON.stringify(setup.audience), setupRows);
                    fact("Later acquisition", setup?.continuation, setupRows);
                    accessLimit(catalog?.lifetime, setupRows);
                }
                const policyRows = section("Separately reviewed member tool policy", "toolkit-batch-policy");
                fact("Policy scope", policy?.approval_scope, policyRows);
                fact("Policy lifetime", policy?.lifetime, policyRows);
                const tools = policy?.proposal?.selection?.tools;
                if (Array.isArray(tools) && tools.every(tool => typeof tool?.name === "string"))
                    fact("Selected policy tools", tools.map(tool => tool.name).join(", "), policyRows);
            }
            const publicService = body?.phase === "installed_public_service_access";
            const credentialService = body?.phase === "installed_credential_service_access";
            if (credentialService && Number.isFinite(Date.parse(prompt.expires_at))
                && Date.now() >= Date.parse(prompt.expires_at)) {
                status.textContent = "This review has expired and its credential fields were cleared. Refresh setup status and wait for a fresh review before continuing.";
                return;
            }
            if (publicService || credentialService || body?.phase === "installed_account_access") {
                if (credentialService) {
                    fact("Authentication", "Credentials for the installed package's declared secret slots");
                    fact("Profile", body.software?.profile);
                } else if (publicService) {
                    const remote = body.operation_basis?.kind === "remote_recipe_declarations";
                    fact("Authentication", remote ? "Researched service declaration: no sign-in required"
                        : "No account or sign-in required by this package");
                    fact("Profile", body.software?.profile);
                    fact("Operation source", remote ? "Researched remote recipe declarations"
                        : ["selected_manifest_declarations", "selected_software_declarations"].includes(body.operation_basis?.kind)
                            ? "Selected package declarations" : body.operation_basis?.kind === "package_published_metadata"
                                ? "Package publisher metadata" : "Operation provenance is unconfirmed");
                    if (remote) {
                        const exported = body.remote_export?.export;
                        const name = exported?.declared_remote_service?.mcp_name;
                        fact("Connector", name);
                        fact("Endpoint", exported?.mcp_configuration?.mcpServers?.[name]?.url);
                    }
                } else {
                    fact("Provider", body.account?.issuer);
                    fact("Account", body.account?.subject);
                }
                fact("Connector", body.software?.server_name);
                fact("Resource", body.connector?.resource);
                fact("Agent", body.application_selection?.agent);
                fact("New instruction", body.service_instruction?.proposed_fresh_instruction);
                fact("Instruction origin", body.service_instruction?.instruction_origin);
                fact("Live attachment", body.service_instruction?.live_attachment_effect);
                const persistence = body.service_instruction?.configuration_persistence;
                if (credentialService && persistence?.kind === "requested_defining_realm_persistence") {
                    fact("Configuration persistence", "Save this connector in the host's defining realm for later sessions");
                    fact("Defining realm", persistence.defining_realm);
                    fact("Credentials", "Store only the declared credentials with the native secret owner. Values are not shown to the agent.");
                    fact("Partial completion", "Configuration or credentials already saved can remain if a later step fails or setup is cancelled. Task access is published only after every required save succeeds.");
                } else fact("Configuration persistence", persistence);
                const lifetime = prompt.body?.service_access?.permission?.lifetime;
                if (lifetime && typeof lifetime === "object") {
                    // The reviewed native permission owns the effective limit.
                    // A null proposal TTL does not mean standing permission.
                    if (lifetime.scope?.kind === "task")
                        fact("Access scope", "This task");
                    const expires = lifetime.expires_at_ms;
                    if (Number.isSafeInteger(expires) && expires > 0) {
                        const deadline = new Date(expires);
                        if (!Number.isNaN(deadline.valueOf())) fact("Access expires", deadline.toISOString());
                    }
                } else {
                    const requested = body.application_selection?.expires_in_ms;
                    if (Number.isSafeInteger(requested) && requested > 0) fact("Requested duration", `${requested} milliseconds`);
                    else fact("Requested duration", "No additional duration supplied; the reviewed permission determines its limit");
                }
                const scopes = body.connector?.requested_scopes;
                if (Array.isArray(scopes) && scopes.every(scope => typeof scope === "string"))
                    fact("Requested scopes", scopes.length ? scopes.join(", ") : "No scopes supplied");
                const operations = body.application_selection?.operations;
                if ((publicService || credentialService) && Array.isArray(operations) && operations.every(name => typeof name === "string")) {
                    fact("Requested operations", operations.join(", "));
                } else if (!publicService && !credentialService && operations && typeof operations === "object" && !Array.isArray(operations)) {
                    const heading = document.createElement("h3"); heading.textContent = "Requested operations"; root.append(heading);
                    const list = document.createElement("dl");
                    for (const [name, operation] of Object.entries(operations)) {
                        const row = document.createElement("div");
                        const term = document.createElement("dt"); term.textContent = name;
                        const detail = document.createElement("dd");
                        const destination = document.createElement("p"); destination.textContent = typeof operation?.destination_origin === "string" ? operation.destination_origin : "Destination not supplied";
                        detail.append(destination);
                        if (Array.isArray(operation?.required_scopes) && operation.required_scopes.every(scope => typeof scope === "string")) {
                            const required = document.createElement("p"); required.textContent = `Scopes: ${operation.required_scopes.length ? operation.required_scopes.join(", ") : "none supplied"}`; detail.append(required);
                        }
                        row.append(term, detail); list.append(row);
                    }
                    root.append(list);
                }
            }
            if (body?.kind === "service_software_install" && body.scope === "software_installation_only") {
                const proposed = body.owner_proposal;
                fact("Review scope", "Software installation only");
                fact("Profile", proposed?.profile);
                fact("Connector", proposed?.mcp_name);
                fact("Component", proposed?.component);
                fact("Publisher", proposed?.package?.publisher);
                fact("Package", proposed?.package?.name);
                fact("Version", proposed?.package?.version);
                fact("Install destination", proposed?.destination);
                fact("Executable", proposed?.entrypoint?.executable);
                fact("Configuration digest", proposed?.configuration_sha256);
                fact("Selected software digest", proposed?.selection_sha256);
                const build = proposed?.build;
                if (build != null) {
                    buildDetails = document.createElement("section"); buildDetails.className = "toolkit-build-review";
                    const title = document.createElement("h3"); title.textContent = "Build source on this host";
                    const rows = document.createElement("dl"); buildDetails.append(title, rows);
                    if (build.scope === "selected_node_build" && build.schema_version === 1) {
                        const note = document.createElement("p");
                        note.textContent = "These commands run locally as trusted software, without sandboxing. Exact review details contains the selected toolchain, inputs and limits.";
                        buildDetails.insertBefore(note, rows);
                        const target = build.target;
                        fact("Node version", target?.node_version, rows);
                        fact("Node ABI", target?.node_abi, rows);
                        fact("Target", [target?.platform, target?.arch, target?.libc?.family, target?.libc?.version]
                            .filter(value => typeof value === "string").join(" "), rows);
                        if (Array.isArray(build.commands)) for (const [index, command] of build.commands.entries()) {
                            if (Array.isArray(command?.argv) && command.argv.every(value => typeof value === "string"))
                                // Quoted tokens preserve argument boundaries; this is not a shell command.
                                fact(`Command ${index + 1} literal arguments`, command.argv.map(value => JSON.stringify(value)).join(" "), rows);
                        }
                        if (Array.isArray(build.outputs)) for (const [index, output] of build.outputs.entries()) {
                            const change = output?.replaces_sha256 === null ? "Add a new file"
                                : typeof output?.replaces_sha256 === "string" ? "Replace the existing file"
                                    : "Replacement information unavailable; inspect Exact review details";
                            if (typeof output?.destination === "string")
                                fact(`Output ${index + 1} (relative to install destination)`, `${output.destination} - ${change}`, rows);
                        }
                    } else fact("Build summary", "This build format cannot be summarized here; inspect Exact review details", rows);
                }
            }
            root.append(facts);
            if (batchDetails) root.append(batchDetails);
            if (buildDetails) root.append(buildDetails);
            if (typeof body?.notice === "string") { const notice = document.createElement("p"); notice.textContent = body.notice; root.append(notice); }
            const credentialFields = new Map();
            if (review.credential_input != null) {
                const input = review.credential_input;
                if (!credentialService || input.kind !== "declared_secret_slots"
                    || Object.keys(input).some(key => !["kind", "slots"].includes(key))
                    || !Array.isArray(input.slots) || !input.slots.length || input.slots.length > 256)
                    throw new TypeError("Invalid credential input");
                const guidance = document.createElement("p");
                guidance.textContent = "Enter the declared credentials below. Approve once to save the configuration and credentials, then grant the reviewed task access. These fields are cleared on submission, refresh or navigation. The response can be uncertain; do not resubmit until current status is known.";
                root.append(guidance);
                for (const slot of input.slots) {
                    if (!slot || Object.keys(slot).some(key => !["slot", "label"].includes(key))
                        || typeof slot.slot !== "string" || !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(slot.slot)
                        || typeof slot.label !== "string" || !slot.label.length || slot.label.length > 4096
                        || credentialFields.has(slot.slot)) throw new TypeError("Invalid credential slot");
                    const label = document.createElement("label");
                    const text = document.createElement("span"); text.textContent = slot.label;
                    const field = document.createElement("input"); field.type = "password";
                    field.autocomplete = "new-password"; field.spellcheck = false;
                    field.maxLength = 16384; field.disabled = submission.sending;
                    field.setAttribute("aria-label", slot.label);
                    label.append(text, field); root.append(label);
                    credentialFields.set(slot.slot, field);
                }
            } else if (credentialService) throw new TypeError("Missing protected credential input");
            // Preserve every exact reviewed field without making protocol JSON
            // the ordinary approval explanation. No interpretation of scopes
            // or inferred provider permissions is added by this renderer.
            const details = document.createElement("details");
            const label = document.createElement("summary"); label.textContent = "Exact review details";
            controls.set("details", label);
            const encoded = document.createElement("pre"); encoded.textContent = JSON.stringify(prompt, null, 2);
            details.append(label, encoded); root.append(details);
            for (const decision of ["approve", "deny"]) {
                if (!prompt.allowed_decisions.includes(decision)) continue;
                const button = document.createElement("button"); button.type = "button"; button.textContent = decision === "approve"
                    ? (body?.kind === "service_software_install" && body.scope === "software_installation_only" ? "Approve & Install" : "Approve") : "Deny";
                if (credentialService && decision === "approve") button.textContent = "Approve & Save";
                button.disabled = submission.sending; controls.set(decision, button);
                button.addEventListener("click", () => {
                    if (disposed || submission.sending || epoch !== current) return;
                    if (credentialService) {
                        if (decision === "approve" && [...credentialFields.values()].some(field => !field.value.length)) {
                            status.textContent = "Enter every declared credential before approving.";
                            return;
                        }
                        const credentials = decision === "approve" ? [...credentialFields].map(([slot, field]) => ({ slot, value: field.value })) : [];
                        for (const field of credentialFields.values()) field.value = "";
                        void send("/host/toolkit/setup/credentials", {
                            review_id: review.review_id, approval_id: prompt.approval_id, decision, credentials });
                    } else void send("/host/toolkit/setup/answer", {
                        review_id: review.review_id, approval_id: prompt.approval_id, decision });
                });
                root.append(button);
            }
            present("approval", prompt.approval_id, details);
        },
        clear,
        dispose() { disposed = true; clear(); document.removeEventListener("focusin", movedFocus); root.remove(); },
    };
}
/** A separate, explicitly selected application policy exposes only grants
 * issued by this application for the current requester. No account connection,
 * credential removal, active-work stop or complete inventory is inferred.
 */
export function mountApplicationAccess(container, refresh, fetcher = fetch, submission = { sending: false }) {
    const document = container.ownerDocument;
    const root = document.createElement("section");
    root.className = "toolkit-view toolkit-application-access";
    container.append(root);
    let disposed = false; let epoch = 0; let currentScope;
    let status; let rendered; let hint; let lastAction;
    const movedFocus = event => {
        if (hint && event.target !== document.body && event.target !== document.documentElement) hint.focus = undefined;
    };
    document.addEventListener("focusin", movedFocus);
    const focusHint = () => rendered && { scope: currentScope,
        focus: [...rendered].find(([, element]) => element === document.activeElement)?.[0] };
    const clear = () => {
        ++epoch; currentScope = undefined; rendered = undefined; hint = undefined; lastAction = undefined;
        root.replaceChildren(); status = undefined;
    };
    return {
        clear,
        suspend() {
            if (disposed) return;
            if (rendered) hint = focusHint();
            // Presentation identity only. Neither an old offer nor a DOM
            // action survives the read; overlapping reads keep the first hint.
            ++epoch; currentScope = undefined; rendered = undefined;
            root.replaceChildren(); status = undefined;
        },
        update(value, scope, paged = false) {
            if (disposed) return;
            const previous = hint ?? focusHint();
            const previousAction = lastAction;
            clear(); currentScope = scope; const current = epoch;
            const heading = document.createElement("h2"); heading.textContent = "Access issued by this application";
            status = document.createElement("p"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
            root.append(heading, status);
            if (value == null) { status.textContent = "This application has not enabled access management."; return; }
            if (value.scope !== "application_issued_access") throw new TypeError("Invalid access scope");
            if (value.state === "unavailable") { status.textContent = "Application-issued access status is unavailable."; return; }
            if (!Number.isSafeInteger(value.offer_id) || value.offer_id < 1 || !Array.isArray(value.rows)
                || value.rows.length > 64 || typeof scope !== "string" || !scope.length) throw new TypeError("Invalid access offer");
            const labels = { grant_issued: "Access is currently available", approval_denied: "Access approval was denied",
                grant_unavailable: "Access is unavailable (for example, expired or revoked)",
                pending_reconciliation: "Access issuance could not be confirmed" };
            const ids = new Set();
            for (const row of value.rows) {
                if (!row || !/^[a-f0-9]{64}$/.test(row.id) || ids.has(row.id)
                    || !Object.hasOwn(labels, row.state) || typeof row.can_revoke !== "boolean"
                    || row.can_revoke !== (row.state === "grant_issued")
                    || ![row.service, row.target, row.resource].every(text => typeof text === "string" && text.length)
                    || !(row.selected_account === null || typeof row.selected_account === "string")
                    || !Array.isArray(row.operations) || !row.operations.length
                    || !row.operations.every(text => typeof text === "string" && text.length)
                    || !["task", "member", "deployment"].includes(row.lifetime?.scope?.kind)
                    || !(row.lifetime.expires_at_ms === null || Number.isSafeInteger(row.lifetime.expires_at_ms) && row.lifetime.expires_at_ms > 0))
                    throw new TypeError("Invalid application access row");
                ids.add(row.id);
            }
            const scopeNote = document.createElement("p");
            scopeNote.textContent = "These are the permissions this application issued for your requests. Account connectivity and credentials are managed separately. Revocation does not stop work already running.";
            root.append(scopeNote);
            if (!value.rows.length) { status.textContent = paged
                ? "No matching application-issued access records on this page."
                : "No matching application-issued access records were found."; return; }
            // Historical feedback only: fresh owner rows still control status
            // and actions. Dropped viewer/target identities cannot return it.
            if (previousAction?.scope === scope && ids.has(previousAction.row)) {
                lastAction = previousAction; status.textContent = lastAction.text ?? "";
            }
            const controls = new Map();
            for (const row of value.rows) {
                const card = document.createElement("article");
                const title = document.createElement("h3"); title.textContent = row.service;
                const facts = document.createElement("dl");
                const context = [title];
                const fact = (name, value, describesTarget = true) => { const dt = document.createElement("dt"); dt.textContent = name;
                    const dd = document.createElement("dd"); dd.textContent = value; facts.append(dt, dd);
                    if (describesTarget) context.push(dt, dd); };
                fact("Agent", row.target); fact("Operations", row.operations.join(", ")); fact("Resource", row.resource);
                if (row.selected_account !== null) fact("Selected account", row.selected_account);
                fact("Access scope", row.lifetime.scope.kind);
                fact("Expiry", row.lifetime.expires_at_ms === null ? "No expiry declared in this scope"
                    : new Date(row.lifetime.expires_at_ms).toISOString());
                fact("Status", labels[row.state], false); card.append(title, facts);
                if (lastAction?.row === row.id) card.append(status);
                if (row.can_revoke) {
                    const button = document.createElement("button"); button.type = "button";
                    button.textContent = "Revoke this access"; button.disabled = submission.sending;
                    labelManagementAction(button, context);
                    controls.set(row.id, button);
                    button.addEventListener("click", async () => {
                        if (disposed || epoch !== current || submission.sending) return;
                        submission.sending = true;
                        const action = { scope, row: row.id }; lastAction = action; status.textContent = ""; card.append(status);
                        for (const control of root.querySelectorAll("button")) control.disabled = true;
                        let text = "The revocation result could not be confirmed. Read current status before a new decision.";
                        try {
                            const response = await fetcher("/host/toolkit/setup/access/revoke", { method: "POST", credentials: "same-origin",
                                cache: "no-store", redirect: "error", headers: { "Content-Type": "application/json" },
                                body: JSON.stringify({ offer_id: value.offer_id, row_id: row.id }) });
                            const notCurrent = "This access view is no longer current. This request revoked nothing. Review current status before a new decision.";
                            // This endpoint returns 409 only before entering the
                            // native revoke owner: the selected offer is gone.
                            if (response.status === 409) text = notCurrent;
                            else {
                                if (!response.ok || !response.headers.get("content-type")?.startsWith("application/json")) throw new Error("Response unavailable");
                                const result = await response.json();
                                const outcomes = { revoked: "This access was revoked. Work already running was not stopped.",
                                    not_current: notCurrent,
                                    controller_in_use: "This access is required by unfinished controller work.",
                                    refused: "This revocation was refused.", unavailable: text };
                                if (result.scope !== "application_issued_access" || !Object.hasOwn(outcomes, result.state)) throw new Error("Unknown result");
                                text = outcomes[result.state];
                            }
                        } catch { /* No retry: an entered native mutation may have completed. */ }
                        submission.sending = false;
                        if (!disposed) {
                            if (lastAction === action) action.text = `Last revocation attempt: ${text}`;
                            await refresh();
                            if (!disposed && lastAction === action && currentScope === scope && status) status.textContent = action.text;
                        }
                    });
                    card.append(button);
                }
                root.append(card);
            }
            rendered = controls;
            const focus = previous?.scope === scope ? controls.get(previous.focus) : undefined;
            if (focus && !focus.disabled && document.activeElement === document.body) focus.focus();
        },
        dispose() { disposed = true; clear(); document.removeEventListener("focusin", movedFocus); root.remove(); },
    };
}
/** Explicit operator recovery of existing approvals whose source publication
 * is unresolved. The browser holds only an opaque, one-use offer selection.
 */
export function mountOperatorRecovery(container, refresh, fetcher = fetch, submission = { sending: false }) {
    const doc = container.ownerDocument;
    const root = doc.createElement("section"); root.className = "toolkit-view toolkit-operator-recovery";
    let epoch = 0; let disposed = false; let currentScope; let status; let controls; let hint; let lastAction;
    const focusHint = () => controls && { scope:currentScope, focus:[...controls].find(([, element]) => element === doc.activeElement)?.[0] };
    const movedFocus = event => { if (hint && event.target !== doc.body && event.target !== doc.documentElement) hint.focus = undefined; };
    doc.addEventListener("focusin", movedFocus);
    const clear = () => { ++epoch; currentScope = undefined; status = undefined; controls = undefined; hint = undefined; lastAction = undefined; root.replaceChildren(); root.remove(); };
    return {
        clear,
        suspend() {
            if (disposed) return;
            if (controls) hint = focusHint();
            ++epoch; currentScope = undefined; status = undefined; controls = undefined; root.replaceChildren(); root.remove();
        },
        update(value, scope, paged = false) {
            if (disposed) return;
            const previous = hint ?? focusHint(); const previousAction = lastAction; clear();
            // Absent policy retains the prior presentation, including no new
            // operator section for an ordinary requester-only installation.
            if (value == null) return;
            container.append(root);
            currentScope = scope; const current = epoch;
            if (value.scope !== "operator_service_recovery") throw new TypeError("Invalid recovery scope");
            const heading = doc.createElement("h2"); heading.textContent = "Unresolved service approvals";
            status = doc.createElement("p"); status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
            root.append(heading, status);
            if (value.state === "unavailable") { status.textContent = "Operator recovery status is unavailable."; return; }
            if (!Number.isSafeInteger(value.offer_id) || value.offer_id < 1 || !Array.isArray(value.rows)
                || value.rows.length > 64 || typeof scope !== "string" || !scope.length) throw new TypeError("Invalid recovery offer");
            const states = { publication_present: "A matching task source is retained; cleanup is unavailable here",
                publication_unresolved: "Task source publication is unresolved" };
            const parents = { issued: "Currently available", unavailable: "Not currently available or could not be confirmed", not_observed: "No issuance observed; an approval worker may still enter" };
            const children = { issued: "Currently available", unavailable: "Not currently available or could not be confirmed", not_observed: "No issuance observed", another_approval: "Belongs to another approval and will be preserved", pending_reconciliation: "Issuance needs reconciliation", denied: "Approval denied" };
            const policies = { not_requested: "No member tool policy requested", pending: "Retained policy remains pending", retained_current: "Approved policy is retained as current", superseded_or_unobserved: "No matching current or pending policy observed", unavailable: "Policy observation unavailable" };
            const ids = new Set();
            for (const row of value.rows) {
                if (!row || !/^[a-f0-9]{64}$/.test(row.id) || ids.has(row.id) || typeof row.can_recover !== "boolean"
                    || !Object.hasOwn(states, row.state) || !Object.hasOwn(parents, row.parent)
                    || !Object.hasOwn(children, row.child) || !Object.hasOwn(policies, row.policy)
                    || row.can_recover && (row.state !== "publication_unresolved" || row.parent === "not_observed")
                    || ![row.service, row.target, row.resource].every(text => typeof text === "string" && text.length)
                    || !Array.isArray(row.operations) || !row.operations.length || !row.operations.every(text => typeof text === "string" && text.length))
                    throw new TypeError("Invalid recovery row");
                ids.add(row.id);
            }
            const note = doc.createElement("p");
            note.textContent = "These are your operator approvals. Missing task publication does not mean the work was abandoned. Choosing cleanup attempts to revoke this approval's native access; it does not remove credentials, uninstall software, roll back tool policy, or stop running work.";
            root.append(note);
            if (!value.rows.length) { status.textContent = paged ? "No matching service approvals on this page." : "No matching service approvals found."; return; }
            // Keep only the last explanation for this still-visible owner row.
            // It cannot restore an offer, a current state or a cleanup action.
            if (previousAction?.scope === scope && ids.has(previousAction.row)) {
                lastAction = previousAction; status.textContent = lastAction.text ?? "";
            }
            controls = new Map();
            for (const row of value.rows) {
                const card = doc.createElement("article"); const title = doc.createElement("h3"); title.textContent = row.service;
                const facts = doc.createElement("dl");
                const context = [title];
                const fact = (name, text, describesTarget = false) => { const dt = doc.createElement("dt"); dt.textContent = name;
                    const dd = doc.createElement("dd"); dd.textContent = text; facts.append(dt, dd);
                    if (describesTarget) context.push(dt, dd); };
                fact("Agent", row.target, true); fact("Operations", row.operations.join(", "), true); fact("Resource", row.resource, true);
                fact("Publication", states[row.state]); fact("Parent access", parents[row.parent]); fact("Application access", children[row.child]); fact("Tool policy", policies[row.policy]);
                card.append(title, facts);
                if (lastAction?.row === row.id) card.append(status);
                if (row.can_recover) {
                    const button = doc.createElement("button"); button.type = "button"; button.textContent = "Revoke this unresolved approval"; button.disabled = submission.sending;
                    labelManagementAction(button, context);
                    controls.set(row.id, button);
                    button.addEventListener("click", async () => {
                        if (disposed || current !== epoch || submission.sending) return;
                        submission.sending = true; for (const control of root.querySelectorAll("button")) control.disabled = true;
                        const action = { scope, row: row.id }; lastAction = action; status.textContent = ""; card.append(status);
                        let text = "The cleanup result could not be confirmed. Read current status before making a new decision.";
                        try {
                            const response = await fetcher("/host/toolkit/setup/recovery/revoke", { method: "POST", credentials: "same-origin", cache: "no-store", redirect: "error",
                                headers: { "Content-Type": "application/json" }, body: JSON.stringify({ offer_id: value.offer_id, row_id: row.id }) });
                            if (response.status === 409) text = "This offer is no longer current. This request changed no access. Read current status.";
                            else {
                                if (!response.ok || !response.headers.get("content-type")?.startsWith("application/json")) throw new Error("Response unavailable");
                                const result = await response.json();
                                const parent = { revoked: "The operator parent was revoked.", not_current: "The access view changed before cleanup entered.", controller_in_use: "Unfinished controller work still requires this parent.", refused: "Parent cleanup was refused.", unavailable: "Parent cleanup could not be confirmed." };
                                const child = { revoked: "Its application access was revoked.", not_entered: "Application cleanup was not entered.", not_issued: "No application access was issued under the revoked parent.", another_approval_preserved: "The application access belongs to another approval and was preserved.", unresolved: "Application access ownership needs reconciliation.", unavailable: "Application cleanup could not be confirmed.", controller_in_use: "Unfinished controller work still requires the application access.", refused: "Application cleanup was refused." };
                                if (result.scope !== "operator_service_recovery" || !Object.hasOwn(parent, result.parent) || !Object.hasOwn(child, result.child) || typeof result.uncertain !== "boolean" || result.policy_changed !== false) throw new Error("Unknown recovery result");
                                text = `${parent[result.parent]} ${child[result.child]} Tool policy was not changed.${result.uncertain ? " Some effects remain uncertain." : ""}`;
                            }
                        } catch { /* Never replay an operation whose response may have been lost. */ }
                        submission.sending = false;
                        if (!disposed) {
                            if (lastAction === action) action.text = `Last approval cleanup attempt: ${text}`;
                            await refresh();
                            if (!disposed && lastAction === action && currentScope === scope && status) status.textContent = action.text;
                        }
                    });
                    card.append(button);
                }
                root.append(card);
            }
            const focus = previous?.scope === scope ? controls.get(previous.focus) : undefined;
            if (focus && !focus.disabled && doc.activeElement === doc.body) focus.focus();
        },
        dispose() { disposed = true; clear(); doc.removeEventListener("focusin", movedFocus); root.remove(); },
    };
}
/** A cached document must read the current authenticated projection again when
 * the browser restores it. Native setup work stays with the existing driver.
 */
export function mountSourceStatusPage(container, read, fetcher = fetch) {
    read ??= selection => readSourceStatus(fetcher, selection);
    const page = container.ownerDocument.defaultView;
    // A browser history restore keeps an entered POST alive. Retain only its
    // busy flag across view lifetimes, never the old prompt, account or DOM.
    const submission = { sending: false };
    let current;
    let disposed = false;
    const stop = () => {
        const mounted = current;
        current = undefined;
        if (!mounted) return;
        if (mounted.timer !== undefined) page.clearTimeout(mounted.timer);
        mounted.account.dispose();
        mounted.access.dispose();
        mounted.recovery.dispose();
        mounted.panel.dispose();
    };
    const start = () => {
        if (disposed || current) return;
        let panel;
        const account = mountAccountReview(container, () => panel.refresh(), fetcher, submission);
        const access = mountApplicationAccess(container, () => panel.refresh(), fetcher, submission);
        const recovery = mountOperatorRecovery(container, () => panel.refresh(), fetcher, submission);
        panel = mountSourceStatusPanel(container, read, account, access, submission, recovery);
        const mounted = { panel, account, access, recovery, timer: undefined };
        current = mounted;
        const poll = async () => {
            if (current !== mounted) return;
            // Values remain only in the visible password DOM. Automatic reads
            // must not erase an in-progress entry; explicit reads and disposal
            // still clear it. Every protected action independently rechecks auth.
            if (!account.hasPrivateInput()) await panel.refresh();
            // A read from the previous page lifetime cannot schedule work in
            // the restored page or replace its freshly authenticated content.
            if (current === mounted) mounted.timer = page.setTimeout(poll, 3000);
        };
        void poll();
    };
    page.addEventListener("pagehide", stop);
    page.addEventListener("pageshow", start);
    start();
    return {
        dispose() {
            disposed = true;
            stop();
            page.removeEventListener("pagehide", stop);
            page.removeEventListener("pageshow", start);
        },
    };
}
if (typeof document !== "undefined") {
    const container = document.getElementById("toolkit-source-status");
    if (container) mountSourceStatusPage(container);
}
