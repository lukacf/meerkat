import { validateProjection } from "./projection.js";
const LABELS = {
    open_authorization: "Review sign-in", continue: "Continue",
    stop_setup: "Stop connecting for this task", cancel_task: "Cancel whole task",
    change_account: "Change account", retry: "Retry", choose_strategy: "Try another strategy",
    reconnect: "Reconnect", revoke_grant: "Revoke this access", disconnect: "Disconnect account",
    pause_frontend: "Pause frontend", resume_frontend: "Resume frontend",
    remove_frontend: "Remove frontend", review_activation: "Review activation",
};
const ACTION_STATES = {
    queued: "Queued for this person", action_required: "Action needed", verifying: "Verifying",
    complete: "Complete", expired: "Expired", no_longer_needed: "No longer needed",
};
const ACTION_KINDS = {
    browser_authorization: "Browser authorization", device_authorization: "Device authorization",
    secret_entry: "Secure credential entry", interactive_setup: "Interactive setup",
    administrator_action: "Administrator action",
};
let nextViewId = 0;
function controlKey(control) {
    return tuple(control.target.owner, control.offer);
}
function tuple(...parts) { return JSON.stringify(parts); }
/** Framework-free UI over owner-provided projections. No network, storage,
 * secret fields, browser automation, timers, lifecycle transitions or retries.
 */
export function mountToolkitView(container, options) {
    if (!options.scope)
        throw new TypeError("An authenticated view scope is required");
    const scope = options.scope;
    const dispatch = options.dispatch;
    const onManualConfiguration = options.onManualConfiguration;
    if (onManualConfiguration !== undefined && typeof onManualConfiguration !== "function") {
        throw new TypeError("Manual configuration navigation must be a function");
    }
    const doc = container.ownerDocument;
    const viewId = ++nextViewId;
    let messageId = 0;
    let disposed = false;
    let revision = -1;
    let generation = Symbol();
    let catalogQuery = "";
    let catalogCategory;
    let categoryReset = false;
    let manualPending = false;
    let manualFeedback = "";
    let manualPresentation;
    const submissions = new Map();
    const root = doc.createElement("div");
    root.className = "toolkit-view";
    const live = doc.createElement("p");
    live.setAttribute("role", "status");
    live.setAttribute("aria-live", "polite");
    live.setAttribute("aria-atomic", "true");
    const content = doc.createElement("div");
    root.append(live, content);
    container.append(root);
    function element(tag, text) {
        const node = doc.createElement(tag);
        if (text !== undefined)
            node.textContent = text;
        return node;
    }
    function fact(parent, name, value) {
        const row = element("div");
        const displayed = element("dd", value);
        row.append(element("dt", name), displayed);
        parent.append(row);
        return displayed;
    }
    function status(parent, value) {
        const node = element("p", value.text);
        node.className = `toolkit-status toolkit-status-${value.tone}`;
        parent.append(node);
    }
    function card(state, parent, key, title) {
        const node = element("article");
        node.className = "toolkit-card";
        const heading = element("h3", title);
        heading.tabIndex = -1;
        // Values stay in JS, never HTML, attributes, URLs or analytics.
        state.focusTargets.set(key, heading);
        state.focusKeys.set(heading, { target: key, row: key });
        node.append(heading);
        parent.append(node);
        return node;
    }
    const disclosures = new Map();
    function diagnostics(state, parent, text, row) {
        if (!text)
            return;
        const details = element("details");
        details.open = disclosures.get(row) ?? false;
        details.addEventListener("toggle", () => {
            if (!disposed && details.isConnected)
                disclosures.set(row, details.open);
        });
        const summary = element("summary", "Technical details");
        state.focusTargets.set(tuple("details", row), summary);
        state.focusKeys.set(summary, { target: tuple("details", row), row });
        details.append(summary, element("pre", text));
        parent.append(details);
    }
    let focusTargets = new Map();
    let occurrences = new Map();
    let focusKeys = new WeakMap();
    function navigation(state, parent, label, target, row, currentGeneration, context = [], occurrence = row) {
        const button = element("button", label);
        button.type = "button";
        if (context.length) {
            button.id = `toolkit-${viewId}-navigation-${++state.messageId}`;
            const contextIds = context.map(node => {
                if (!node.id)
                    node.id = `toolkit-${viewId}-context-${++state.messageId}`;
                return node.id;
            });
            button.setAttribute("aria-labelledby", [button.id, ...contextIds].join(" "));
        }
        const key = tuple("navigation", occurrence, target);
        state.focusTargets.set(key, button);
        state.focusKeys.set(button, { target: key, row });
        button.addEventListener("click", () => {
            if (!disposed && generation === currentGeneration)
                focusTargets.get(target)?.focus();
        });
        parent.append(button);
    }
    function controls(state, parent, values, row, currentGeneration, context = []) {
        if (!values.length)
            return;
        const group = element("div");
        group.className = "toolkit-controls";
        // Associate already-visible context, never opaque owner/action references.
        // Keep the outcome description separate so pending/uncertain text cannot
        // replace the account or destination part of the control's accessible name.
        const contextIds = [state.focusTargets.get(row), ...context].filter(node => node !== undefined).map(node => {
            if (!node.id)
                node.id = `toolkit-${viewId}-context-${++state.messageId}`;
            return node.id;
        });
        for (const value of values) {
            const key = controlKey(value);
            const button = element("button", LABELS[value.kind]);
            button.type = "button";
            button.id = `toolkit-${viewId}-control-${++state.messageId}`;
            button.setAttribute("aria-labelledby", [button.id, ...contextIds].join(" "));
            const message = element("span", submissions.get(key) ?? value.explanation ?? "");
            message.id = `toolkit-${viewId}-message-${++state.messageId}`;
            button.setAttribute("aria-describedby", message.id);
            const previous = state.occurrences.get(key) ?? [];
            previous.push({ button, message });
            state.occurrences.set(key, previous);
            const focusKey = tuple("control", row, key);
            state.focusTargets.set(focusKey, button);
            state.focusKeys.set(button, { target: focusKey, row });
            button.disabled = submissions.has(key);
            button.addEventListener("click", () => {
                if (disposed || currentGeneration !== generation || button.disabled || submissions.has(key))
                    return;
                // Capture before dispatch, disabling or focus can reenter through host
                // listeners. Duplicate offers share custody, but focus belongs to the
                // actual active occurrence rather than the programmatically clicked one.
                const request = { scope, viewRevision: revision, control: {
                        offer: value.offer, kind: value.kind,
                        target: { owner: value.target.owner, reference: value.target.reference, revision: value.target.revision },
                        ...(value.explanation === undefined ? {} : { explanation: value.explanation }),
                    } };
                const active = (occurrences.get(key) ?? []).find(item => item.button === doc.activeElement);
                const activeRow = active ? focusKeys.get(active.button)?.row : undefined;
                const pending = "Request sent. Waiting for owner status.";
                submissions.set(key, pending);
                let acknowledged;
                try {
                    acknowledged = Promise.resolve(dispatch(request));
                }
                catch {
                    acknowledged = Promise.reject(new Error("Dispatch did not acknowledge"));
                }
                void acknowledged.catch(() => {
                    if (disposed)
                        return;
                    const unknown = "Could not confirm this request. Check its current owner status before taking further action.";
                    submissions.set(key, unknown);
                    // An update may retain this exact offer. Update only its current
                    // occurrences, never a newly offered control for the same target.
                    const current = occurrences.get(key) ?? [];
                    for (const sibling of current)
                        sibling.message.textContent = unknown;
                    if (current.length)
                        live.textContent = unknown;
                });
                // Dispatch may synchronously update, move focus or dispose the view.
                // Its captured request is already retained; only current presentation
                // may be changed from this point onward.
                if (disposed || currentGeneration !== generation)
                    return;
                for (const sibling of occurrences.get(key) ?? [])
                    sibling.message.textContent = pending;
                live.textContent = pending;
                if (active && activeRow && doc.activeElement === active.button) {
                    // Blur first so a host blur listener can move focus or adopt a new
                    // projection before choosing a connected heading. Calling heading.focus
                    // directly could otherwise finish on an obsolete node after reentry.
                    active.button.blur();
                    if (!disposed && doc.activeElement === doc.body) {
                        (focusTargets.get(activeRow) ?? focusTargets.get(tuple("panel")))?.focus();
                    }
                }
                if (disposed || currentGeneration !== generation)
                    return;
                // A host callback may have focused a different occurrence of this offer.
                // Handle each active occurrence while it is still focusable: browsers
                // may defer the blur caused by disabling, or retain disabled focus.
                // The finite pass honors host focus moves without scheduling a repair.
                for (const sibling of occurrences.get(key) ?? []) {
                    if (doc.activeElement === sibling.button) {
                        const row = focusKeys.get(sibling.button)?.row;
                        sibling.button.blur();
                        if (!disposed && row && doc.activeElement === doc.body) {
                            (focusTargets.get(row) ?? focusTargets.get(tuple("panel")))?.focus();
                        }
                        if (disposed || currentGeneration !== generation)
                            return;
                    }
                    sibling.button.disabled = true;
                    if (disposed || currentGeneration !== generation)
                        return;
                }
            });
            const item = element("div");
            item.append(button, message);
            group.append(item);
        }
        parent.append(group);
    }
    function service(state, parent, value, key, currentGeneration) {
        const node = card(state, parent, key, value.label);
        node.append(element("p", "Service access"));
        status(node, value.status);
        const facts = element("dl");
        const account = fact(facts, "Account", value.account);
        fact(facts, "Used by", value.usedBy);
        fact(facts, "Operations", value.operations.join(", "));
        fact(facts, "Resources", value.resources.join(", "));
        fact(facts, "Access lifetime", value.lifetime);
        if (value.standingAuthority)
            fact(facts, "Standing authority", value.standingAuthority);
        node.append(facts);
        controls(state, node, value.controls, key, currentGeneration, [account]);
    }
    function frontend(state, parent, value, key, currentGeneration) {
        const node = card(state, parent, key, value.label);
        node.append(element("p", "Agent frontend"));
        status(node, value.status);
        const facts = element("dl");
        const endpoint = fact(facts, "Entrypoint", value.endpoint);
        const audience = fact(facts, "Admitted audience", value.audience);
        fact(facts, "Target agent", value.target);
        fact(facts, "Conversation", value.conversation);
        fact(facts, "Output visibility", value.output);
        fact(facts, "Service authority", value.serviceAccess);
        fact(facts, "Lifetime", value.lifetime);
        fact(facts, "Package", value.packageId);
        node.append(facts);
        controls(state, node, value.controls, key, currentGeneration, [endpoint, audience]);
    }
    function userAction(state, parent, value, key, currentGeneration) {
        const node = card(state, parent, key, `${ACTION_KINDS[value.kind]}: ${value.integration}`);
        node.append(element("p", ACTION_STATES[value.state]), element("p", value.instructions));
        if (value.expiresAt)
            node.append(element("p", `Expires: ${value.expiresAt}`));
        const context = [];
        if (value.authorizationOrigin !== undefined) {
            const address = element("p", `Destination origin: ${value.authorizationOrigin}`);
            address.dir = "ltr";
            address.className = "toolkit-destination";
            node.append(address);
            context.push(address);
        }
        if (value.requestedAccountLabel !== undefined) {
            const account = element("p", `Requested account: ${value.requestedAccountLabel}`);
            node.append(account);
            context.push(account);
        }
        controls(state, node, value.controls, key, currentGeneration, context);
    }
    function manualNavigation(state, parent, row, currentGeneration) {
        if (!onManualConfiguration) {
            parent.append(element("p", "Advanced manual configuration is not provided by this host."));
            return undefined;
        }
        const navigate = onManualConfiguration;
        const button = element("button", "Advanced manual configuration");
        button.type = "button";
        button.setAttribute("aria-disabled", String(manualPending));
        const feedback = element("p", manualFeedback);
        feedback.setAttribute("role", "status");
        feedback.setAttribute("aria-live", "polite");
        feedback.id = `toolkit-${viewId}-manual-${++state.messageId}`;
        button.setAttribute("aria-describedby", feedback.id);
        const key = tuple("manual-navigation");
        state.focusTargets.set(key, button);
        state.focusKeys.set(button, { target: key, row });
        function present() {
            const current = manualPresentation;
            if (disposed || !current || current.generation !== generation)
                return;
            current.button.setAttribute("aria-disabled", String(manualPending));
            current.feedback.textContent = manualFeedback;
        }
        function settled(message) {
            if (disposed)
                return;
            manualPending = false;
            manualFeedback = message;
            present();
        }
        button.addEventListener("click", () => {
            if (disposed || generation !== currentGeneration || !button.isConnected || manualPending)
                return;
            // Reserve before host code can update, dispose or click reentrantly. This
            // is navigation custody only, with no OwnerControl or domain transition.
            manualPending = true;
            manualFeedback = "Opening advanced manual configuration...";
            present();
            let navigation;
            try {
                navigation = Promise.resolve(navigate());
            }
            catch {
                navigation = Promise.reject(new Error("Navigation did not acknowledge"));
            }
            void navigation.then(() => settled("Manual configuration navigation requested."), () => settled("Could not confirm manual configuration navigation. You can try opening it again."));
        });
        parent.append(button, feedback);
        return { generation: currentGeneration, button, feedback };
    }
    function catalog(state, parent, value, currentGeneration) {
        const key = tuple("catalog");
        const heading = element("h2", "Catalog");
        heading.tabIndex = -1;
        state.focusTargets.set(key, heading);
        state.focusKeys.set(heading, { target: key, row: key });
        parent.append(heading, element("p", "Catalog descriptions do not establish installation, connectivity or permission to use an integration."));
        let selected = catalogCategory;
        let reset = categoryReset;
        if (selected !== undefined && !value.categories.some(item => item.id === selected)) {
            selected = undefined;
            reset = true;
        }
        const filters = element("div");
        filters.className = "toolkit-catalog-filters";
        const searchLabel = element("label", "Search catalog");
        const search = element("input");
        search.type = "search";
        search.value = catalogQuery;
        search.autocomplete = "off";
        searchLabel.append(search);
        const categoryLabel = element("label", "Category");
        const select = element("select");
        // Option positions are local presentation keys. No host category ID is a
        // reserved sentinel, and opaque IDs are not displayed or searched.
        const all = element("option", "All categories");
        all.value = "";
        select.append(all);
        value.categories.forEach((item, index) => {
            const option = element("option", `Category: ${item.label}`);
            option.value = String(index);
            option.selected = item.id === selected;
            select.append(option);
        });
        categoryLabel.append(select);
        filters.append(searchLabel, categoryLabel);
        parent.append(filters);
        for (const [suffix, node] of [["search", search], ["category", select]]) {
            const target = tuple("catalog", suffix);
            state.focusTargets.set(target, node);
            state.focusKeys.set(node, { target, row: key });
        }
        const resetText = element("p", "The selected category is no longer available. Showing all categories.");
        resetText.hidden = !reset;
        const count = element("p");
        count.setAttribute("role", "status");
        count.setAttribute("aria-live", "polite");
        count.setAttribute("aria-atomic", "true");
        const empty = element("p");
        parent.append(resetText, count, empty);
        const categoryLabels = new Map(value.categories.map(item => [item.id, item.label]));
        const entries = value.entries.map(item => {
            const entryKey = tuple("catalog-entry", item.id);
            const node = card(state, parent, entryKey, item.label);
            const title = state.focusTargets.get(entryKey);
            state.focusKeys.set(title, { target: entryKey, row: key });
            const roles = item.roles.map(role => role === "service" ? "Service access" : "Agent frontend");
            const categories = item.categoryIds.map(id => categoryLabels.get(id));
            node.append(element("p", item.description), element("p", roles.join(", ")));
            if (categories.length)
                node.append(element("p", categories.join(", ")));
            const facts = element("dl");
            fact(facts, "Support description", item.support);
            const prerequisites = element("div");
            prerequisites.append(element("dt", "Prerequisites"));
            const requirements = element("dd");
            if (item.prerequisites.length) {
                const list = element("ul");
                for (const prerequisite of item.prerequisites)
                    list.append(element("li", prerequisite));
                requirements.append(list);
            }
            else
                requirements.append(element("p", "No prerequisites supplied."));
            prerequisites.append(requirements);
            facts.append(prerequisites);
            fact(facts, "Support evidence", item.evidence ?? "No support evidence supplied.");
            node.append(facts);
            const searchable = [item.label, item.description, item.support, ...item.prerequisites,
                ...(item.evidence !== undefined ? [item.evidence] : []), ...categories, ...roles].map(text => text.toLowerCase());
            return { item, node, title, entryKey, searchable };
        });
        function filter(restoreFocus) {
            const query = search.value.toLowerCase();
            let visible = 0;
            let hiddenFocus;
            for (const entry of entries) {
                const show = (selected === undefined || entry.item.categoryIds.includes(selected)) &&
                    entry.searchable.some(text => text.includes(query));
                if (!show && doc.activeElement === entry.title)
                    hiddenFocus = entry.title;
                entry.node.hidden = !show;
                if (show) {
                    visible++;
                    state.focusTargets.set(entry.entryKey, entry.title);
                }
                else
                    state.focusTargets.delete(entry.entryKey);
            }
            count.textContent = `${visible} of ${entries.length} catalog entries`;
            empty.textContent = !entries.length ? "No catalog entries supplied." : "No catalog entries match these filters.";
            empty.hidden = visible > 0;
            // A filter event can be dispatched programmatically while an entry heading
            // has focus. Restore only that hidden focus, never external host focus.
            if (restoreFocus && hiddenFocus && !disposed && generation === currentGeneration &&
                (doc.activeElement === hiddenFocus || doc.activeElement === doc.body)) {
                // Blurring may synchronously adopt a new snapshot, choose another host
                // target or dispose the view. Resolve fallback from the current maps
                // only after that callback, never finish focusing an obsolete heading.
                if (doc.activeElement === hiddenFocus)
                    hiddenFocus.blur();
                if (!disposed && doc.activeElement === doc.body) {
                    (focusTargets.get(key) ?? focusTargets.get(tuple("panel")))?.focus();
                }
            }
        }
        search.addEventListener("input", () => {
            if (disposed || generation !== currentGeneration || !search.isConnected)
                return;
            catalogQuery = search.value;
            categoryReset = false;
            resetText.hidden = true;
            filter(true);
        });
        select.addEventListener("change", () => {
            if (disposed || generation !== currentGeneration || !select.isConnected)
                return;
            selected = value.categories[select.selectedIndex - 1]?.id;
            catalogCategory = selected;
            categoryReset = false;
            resetText.hidden = true;
            filter(true);
        });
        filter(false);
        return { category: selected, reset, manual: manualNavigation(state, parent, key, currentGeneration) };
    }
    return {
        update(input) {
            if (disposed)
                throw new Error("Toolkit view has been disposed");
            // Read caller-owned getters only while cloning, then use that same value
            // for scope, ordering, validation, rendering and eventual dispatch.
            const snapshot = structuredClone(input);
            if (disposed)
                throw new Error("Toolkit view has been disposed");
            if (snapshot.scope !== scope)
                throw new TypeError("Projection belongs to a different view scope");
            if (!Number.isSafeInteger(snapshot.revision) || snapshot.revision < 0)
                throw new TypeError("Invalid view revision");
            if (snapshot.revision <= revision)
                return false;
            validateProjection(snapshot);
            const focused = doc.activeElement instanceof doc.defaultView.HTMLElement
                ? focusKeys.get(doc.activeElement) : undefined;
            const searchSelection = focused?.target === tuple("catalog", "search") &&
                doc.activeElement instanceof doc.defaultView.HTMLInputElement
                ? { start: doc.activeElement.selectionStart, end: doc.activeElement.selectionEnd,
                    direction: doc.activeElement.selectionDirection } : undefined;
            // Rejected detached controls must never share a later accepted identity.
            const currentGeneration = Symbol();
            const state = {
                messageId, focusTargets: new Map(), occurrences: new Map(), focusKeys: new WeakMap(),
            };
            const next = element("div");
            const title = element("h2", "Connections and agent frontends");
            title.tabIndex = -1;
            const panelKey = tuple("panel");
            state.focusTargets.set(panelKey, title);
            state.focusKeys.set(title, { target: panelKey, row: panelKey });
            next.append(title, element("p", `Status observed: ${snapshot.observedAt}`));
            function section(title) {
                const node = element("section");
                node.append(element("h2", title));
                next.append(node);
                return node;
            }
            const tasks = section("Setup requests");
            if (!snapshot.tasks.length)
                tasks.append(element("p", "No setup requests."));
            for (const task of snapshot.tasks) {
                const key = tuple("task", task.id);
                const node = card(state, tasks, key, task.title);
                status(node, task.phase);
                const facts = element("dl");
                fact(facts, "Requested through", task.application);
                fact(facts, "Requested by", task.requester);
                fact(facts, "Reason", task.reason);
                fact(facts, "Next action", task.nextAction);
                node.append(facts);
                for (const role of task.roles) {
                    const roleKey = tuple("role", key, role.kind, role.id);
                    if (role.kind === "service")
                        service(state, node, role, roleKey, currentGeneration);
                    else
                        frontend(state, node, role, roleKey, currentGeneration);
                }
                for (const action of task.userActions)
                    userAction(state, node, action, tuple("action", key, action.id), currentGeneration);
                controls(state, node, task.controls, key, currentGeneration);
                diagnostics(state, node, task.diagnostics, key);
            }
            const connections = section("Service connections");
            if (!snapshot.connections.length)
                connections.append(element("p", "No service connections."));
            for (const connection of snapshot.connections) {
                const key = tuple("connection", connection.id);
                const node = card(state, connections, key, connection.service);
                status(node, connection.status);
                const facts = element("dl");
                const account = fact(facts, "Connected account", connection.account);
                fact(facts, "Provider credential scope", connection.credentialScope);
                fact(facts, "Package", connection.packageId);
                node.append(facts, element("p", "Account connectivity and permission to use it are separate."));
                if (snapshot.packages.some(item => item.id === connection.packageId)) {
                    navigation(state, node, "View package", tuple("package", connection.packageId), key, currentGeneration);
                }
                for (const taskId of connection.pendingTaskIds) {
                    const task = snapshot.tasks.find(candidate => candidate.id === taskId);
                    if (!task)
                        continue;
                    navigation(state, node, `View setup: ${task.title}`, tuple("task", taskId), key, currentGeneration);
                }
                for (const grant of connection.grants)
                    service(state, node, grant, tuple("grant", key, grant.id), currentGeneration);
                controls(state, node, connection.controls, key, currentGeneration, [account]);
                diagnostics(state, node, connection.diagnostics, key);
            }
            const frontends = section("Agent frontends");
            if (!snapshot.frontends.length)
                frontends.append(element("p", "No agent frontends."));
            for (const item of snapshot.frontends)
                frontend(state, frontends, item, tuple("frontend", item.id), currentGeneration);
            const packages = section("Packages");
            if (!snapshot.packages.length)
                packages.append(element("p", "No packages."));
            for (const item of snapshot.packages) {
                const node = card(state, packages, tuple("package", item.id), `${item.label} ${item.version}`);
                status(node, item.status);
                for (const exported of item.exports) {
                    const row = element("div");
                    row.append(element("p", `${exported.kind === "service" ? "Service access" : "Agent frontend"}: ${exported.label}`));
                    status(row, exported.status);
                    const exportKey = tuple("package-export", item.id, exported.kind, exported.id);
                    for (const instanceId of exported.instanceIds ?? []) {
                        // Validation binds every explicit target to this package and role.
                        // Export IDs never double as inferred instance references.
                        if (exported.kind === "frontend") {
                            const instance = snapshot.frontends.find(candidate => candidate.id === instanceId);
                            const context = element("p", `${instance.label}. Entrypoint: ${instance.endpoint}. Admitted audience: ${instance.audience}`);
                            row.append(context);
                            navigation(state, row, "View frontend", tuple("frontend", instanceId), tuple("package", item.id), currentGeneration, [context], exportKey);
                        }
                        else {
                            const instance = snapshot.connections.find(candidate => candidate.id === instanceId);
                            const context = element("p", `${instance.service}. Connected account: ${instance.account}`);
                            row.append(context);
                            navigation(state, row, "View connection", tuple("connection", instanceId), tuple("package", item.id), currentGeneration, [context], exportKey);
                        }
                    }
                    node.append(row);
                }
                for (const component of item.components) {
                    node.append(element("p", `Supporting component: ${component.label}`));
                    status(node, component.status);
                }
                diagnostics(state, node, item.diagnostics, tuple("package", item.id));
            }
            let nextCatalog;
            let nextManual;
            if (snapshot.catalog !== undefined) {
                const node = element("section");
                next.append(node);
                nextCatalog = catalog(state, node, snapshot.catalog, currentGeneration);
                nextManual = nextCatalog.manual;
            }
            else if (onManualConfiguration) {
                const node = section("Manual configuration");
                const heading = node.firstElementChild;
                const key = tuple("manual-section");
                heading.tabIndex = -1;
                state.focusTargets.set(key, heading);
                state.focusKeys.set(heading, { target: key, row: key });
                nextManual = manualNavigation(state, node, key, currentGeneration);
            }
            // Nothing observable is adopted until the complete detached render exists.
            // A rejected projection retains old callbacks and pending offer custody.
            content.replaceChildren(next);
            revision = snapshot.revision;
            generation = currentGeneration;
            messageId = state.messageId;
            focusTargets = state.focusTargets;
            occurrences = state.occurrences;
            focusKeys = state.focusKeys;
            if (nextCatalog) {
                catalogCategory = nextCatalog.category;
                categoryReset = nextCatalog.reset;
            }
            manualPresentation = nextManual;
            // Focusing can synchronously call host listeners that update or dispose
            // this view. Publish all accepted state first; do not overwrite it later.
            live.textContent = snapshot.announcement;
            if (focused) {
                const candidate = focusTargets.get(focused.target);
                const same = candidate?.hasAttribute("disabled") ? undefined : candidate;
                // Publish the captured caret before focus listeners can synchronously
                // adopt another snapshot. Never restore an old range after host code.
                if (searchSelection && searchSelection.start !== null && searchSelection.end !== null &&
                    same instanceof doc.defaultView.HTMLInputElement) {
                    same.setSelectionRange(searchSelection.start, searchSelection.end, searchSelection.direction ?? undefined);
                }
                if (!disposed && generation === currentGeneration) {
                    (same ?? focusTargets.get(focused.row) ?? title).focus();
                }
            }
            return true;
        },
        dispose() {
            if (disposed)
                return;
            disposed = true;
            generation = Symbol();
            root.remove();
            occurrences.clear();
            focusTargets.clear();
            submissions.clear();
            disclosures.clear();
            manualPresentation = undefined;
        },
    };
}
