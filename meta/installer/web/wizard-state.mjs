import { canCombineOptions } from "./guided-state.mjs";
import { startingContextDetails, startingContextText } from "./forge-starting-context.mjs";

const INPUT_PREFIX = "mobkit-setup-input-v2:";
const MANUAL_PREFIX = "mobkit-setup-manual-v2:";
const RESUME_PREFIX = "mobkit-setup-guide-resume-v1:";
export const GUIDE_CREATION_KEY = "mobkit-setup-creation-v1";

/** A tab retains its first creation request until the saved guide is admitted. */
export function guideCreationRequest(storage) {
  const saved = storage.getItem(GUIDE_CREATION_KEY);
  if (saved !== null) {
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(saved))
      throw new Error("Your saved setup-start request could not be read. Keep your idea and choose New setup when ready.");
    return saved;
  }
  const request = crypto.randomUUID();
  storage.setItem(GUIDE_CREATION_KEY, request);
  return request;
}

/** Browser recovery is optional. Its failure must not reject saved server state. */
export function createGuideStorage(getStorage) {
  const values = new Map(), listeners = new Set();
  let storage, persistent = true;
  const unavailable = () => {
    if (!persistent) return;
    persistent = false;
    for (const listener of listeners) listener();
  };
  try {
    storage = getStorage();
    if (!storage?.getItem || !storage?.setItem || !storage?.removeItem) unavailable();
  } catch { unavailable(); }
  return {
    persistent: () => persistent,
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    getItem(key) {
      if (persistent) {
        try {
          const value = storage.getItem(key) ?? null;
          values.set(key, value);
          return value;
        } catch { unavailable(); }
      }
      return values.get(key) ?? null;
    },
    setItem(key, value) {
      values.set(key, value);
      if (persistent) {
        try {
          storage.setItem(key, value);
          if (storage.getItem(key) !== value) unavailable();
        } catch { unavailable(); }
      }
    },
    removeItem(key) {
      values.set(key, null);
      if (persistent) {
        try {
          storage.removeItem(key);
          if (storage.getItem(key) != null) unavailable();
        } catch { unavailable(); }
      }
    },
    keys() {
      if (persistent) {
        try {
          const keys = [];
          for (let index = 0; index < storage.length; index++) {
            const key = storage.key(index);
            if (key !== null) { keys.push(key); values.set(key, storage.getItem(key)); }
          }
          return keys;
        } catch { unavailable(); }
      }
      return [...values].filter(([, value]) => value != null).map(([key]) => key);
    },
  };
}

export function currentGuideActivity(snapshot, state) {
  return snapshot?.guideId === state?.id && snapshot?.guideVersion === state?.version
    ? snapshot.activity : null;
}

export function implementationHandoff(state) {
  const plan = state.proposal || state.blueprint;
  const lines = [`# ${plan?.project.name || "Agent system"}: implementation handoff`, "", "This is a design handoff, not a complete deployment package. The requested application work below remains unimplemented.", "", "## Agreed understanding", ""];
  const response = [...(state.messages || [])].reverse().find((message) => message.role === "assistant");
  if (response?.text) lines.splice(4, 0, "## Implementation guidance", "", response.text, "");
  for (const [topic, fact] of Object.entries(state.brief?.facts || {})) lines.push(`- ${topic.replaceAll("_", " ")}: ${fact.summary} (${fact.status})`);
  for (const item of state.brief?.requirements || []) {
    lines.push("", `## ${item.outcome}`, "", `Status: ${item.disposition}`, "", `Available: ${item.supported_subset || "Not yet specified"}`, "", `Remaining implementation: ${item.extension || "No extension recorded"}`);
  }
  lines.push("", "## Proposed native blueprint", "", "This blueprint is a draft subset. Resolve the required extensions before describing the full requested system as ready.", "", "```json", JSON.stringify(plan, null, 2), "```", "");
  return lines.join("\n");
}

export function workflowLayers(flow) {
  const pending = [...flow.steps], completed = new Set(), layers = [];
  while (pending.length) {
    const ready = pending.filter((step) => (step.depends_on || []).every((id) => completed.has(id)));
    if (!ready.length) return [];
    layers.push(ready);
    for (const step of ready) {
      completed.add(step.id);
      pending.splice(pending.indexOf(step), 1);
    }
  }
  return layers;
}

export function inputKey(state) {
  const question = state?.question;
  // Choices belong to the whole decision, including the meaning of its options.
  return `${INPUT_PREFIX}${state?.id || "new"}:${JSON.stringify(question ? [
    question.id, question.stage, question.title, question.description,
    question.multiple === true, question.allow_text !== false,
    (question.options || []).map((option) => [option.id, option.label, option.description]),
  ] : ["conversation"])}`;
}
/** Starting context belongs to this exact input, never a migrated draft. */
export function inputStartingHelp(input) {
  const hint = input?.starting_help;
  if (startingContextDetails(hint) && typeof input?.draft === "string") return hint;
  if (!hint || typeof hint !== "object" || Array.isArray(hint) ||
      Object.keys(hint).length !== 4 ||
      typeof hint.audience_id !== "string" || !hint.audience_id || hint.audience_id.length > 64 ||
      typeof hint.kind_id !== "string" || !hint.kind_id || hint.kind_id.length > 64 ||
      typeof hint.prefix !== "string" || typeof hint.inserted !== "string" ||
      hint.inserted.length <= hint.prefix.length || !hint.inserted.startsWith(hint.prefix) ||
      typeof input?.draft !== "string" || !input.draft.startsWith(hint.inserted)) return null;
  return hint;
}
/** Join optional choices at send time without rewriting the raw composer draft. */
export function inputSubmissionText(input) {
  const draft = typeof input?.draft === "string" ? input.draft : "";
  const context = startingContextText(inputStartingHelp(input));
  return context ? context + (draft.length ? `\n\n${draft}` : "") : draft;
}
function startingHelpInput(input, state) {
  if (state?.organization) return {};
  const hint = inputStartingHelp(input);
  return hint ? { starting_help: hint } : {};
}
export function readInput(storage, state) {
  const key = inputKey(state);
  let saved = {}, previous;
  try {
    saved = JSON.parse(storage.getItem(key) || "null") || {};
  } catch {}
  try {
    previous = JSON.parse(storage.getItem(`${INPUT_PREFIX}${state?.id || "new"}:latest`));
  } catch {}
  // Earlier keys did not bind choice meaning. Only their unsent text can migrate.
  const textSource = typeof saved.draft === "string" ? saved
    : ownedInput(previous, state?.id || "new") ? previous : {};
  return {
    key,
    draft: typeof textSource.draft === "string" ? textSource.draft : "",
    selected: inputSelections(state?.question, saved.selected, saved.combine_options),
    mode: textSource.mode === "chat" ? "chat" : "answer",
    ...combinedInput(state?.question, saved),
    ...(saved.key === key && textSource === saved ? startingHelpInput(saved, state) : {}),
  };
}
function combinedInput(question, input) {
  return input?.combine_options === true && canCombineOptions(question) ? { combine_options: true } : {};
}
function inputSelections(question, values, combineOptions = false) {
  const allowed = new Set((question?.options || []).map(option => option.id));
  const selected = Array.isArray(values) ? [...new Set(values.filter(id => allowed.has(id)))] : [];
  return question?.multiple === true || combineOptions === true && canCombineOptions(question) || selected.length <= 1 ? selected : [];
}
/** Reconcile a returned guide with live edits from that same setup only. */
export function guideInputForState(storage, state, current, { newlyCompleted = false } = {}) {
  const restored = readInput(storage, state);
  if (!ownedInput(current, state?.id || "new")) return restored;
  // Only a newly acknowledged completion may consume the still-visible send.
  // Historical completed turns must not erase a deliberately repeated draft.
  const live = newlyCompleted && state.operation?.status === "completed" &&
    !preservedGuideResume(storage, state)
    ? admitSubmittedInput(current, state.operation.request || {}) : current;
  if (live.key === restored.key) {
    const { combine_options, starting_help, ...ordinary } = restored;
    return { ...ordinary, draft: live.draft,
      selected: inputSelections(state?.question, live.selected, live.combine_options), mode: live.mode,
      ...combinedInput(state?.question, live), ...startingHelpInput(live, state) };
  }
  if (live.draft) {
    // Legacy inserted context may restore with its exact saved prose.
    // Separate choices never attach to text from another question.
    const { starting_help, ...ordinary } = restored;
    return { ...ordinary, draft: live.draft, mode: live.mode,
      ...(live.draft === restored.draft && !startingContextDetails(restored.starting_help) ? startingHelpInput(restored, state) : {}) };
  }
  return restored;
}
/** Only admission of a new setup can transfer its anonymous composer. */
export function admitNewGuideInput(storage, state, current) {
  if (!state?.id || !ownedInput(current, "new")) return guideInputForState(storage, state, current);
  const sameQuestion = current.key === inputKey({ ...state, id: null });
  const { combine_options, starting_help, ...restored } = readInput(storage, state);
  return { ...restored, draft: current.draft, mode: current.mode,
    selected: sameQuestion ? inputSelections(state.question, current.selected, current.combine_options) : [],
    ...(sameQuestion ? combinedInput(state.question, current) : {}),
    ...(sameQuestion && startingContextDetails(inputStartingHelp(current)) ? startingHelpInput(current, state) : {}) };
}
export function writeInput(storage, input) {
  try {
    const { starting_help, ...ordinary } = input;
    const saved = { ...ordinary, ...startingHelpInput(input) };
    storage.setItem(input.key, JSON.stringify(saved));
    const guideId = input.key.slice(INPUT_PREFIX.length).split(":", 1)[0];
    storage.setItem(`${INPUT_PREFIX}${guideId}:latest`, JSON.stringify(ordinary));
  } catch {}
}
export function admitSubmittedInput(input, submitted) {
  // Admission, rather than model completion, is the durable send boundary.
  // A new draft typed while the request was in flight belongs to the next turn.
  const separateContext = !!startingContextDetails(inputStartingHelp(input));
  const matchesChoices = submitted.action === "answer" &&
    JSON.stringify(input.selected) === JSON.stringify(submitted.selected || []) &&
    (input.combine_options === true) === (submitted.combine_options === true);
  const consumesDraft = ["answer", "chat"].includes(submitted.action) &&
    inputSubmissionText(input) === submitted.text &&
    (!separateContext || submitted.action === "chat" || matchesChoices);
  const consumesChoices = matchesChoices && (!separateContext || consumesDraft);
  const result = {
    ...input,
    draft: consumesDraft ? "" : input.draft,
    selected: consumesChoices || submitted.action === "back" ? [] : input.selected,
    mode: consumesDraft || submitted.action === "back" ? "answer" : input.mode,
  };
  if (consumesChoices || submitted.action === "back") delete result.combine_options;
  if (!inputStartingHelp(result) || separateContext && (consumesDraft || submitted.action === "back")) delete result.starting_help;
  return result;
}

function ownedInput(input, guideId) {
  return input && typeof input.key === "string" && input.key.startsWith(`${INPUT_PREFIX}${guideId}:`) &&
    typeof input.draft === "string" && Array.isArray(input.selected) && input.selected.every(id => typeof id === "string") &&
    (input.combine_options === undefined || typeof input.combine_options === "boolean") &&
    ["chat", "answer"].includes(input.mode);
}

/** Save an explicit button's input ownership locally, never in the API body. */
export function captureGuideResumeInput(storage, state, input, text, intent = null) {
  if (intent !== null && (intent.action !== "organization" ||
      !/^[a-f0-9]{64}$/.test(intent.organization_fingerprint))) return false;
  if (!state?.id || !Number.isSafeInteger(state.version) || input?.key !== inputKey(state) ||
      !ownedInput(input, state.id) || typeof text !== "string" || (!intent && !text.trim())) return false;
  const marker = { guide_id: state.id, guide_version: state.version, text, request_id: null,
    ...(intent ? { intent } : {}),
    input: { key: input.key, draft: input.draft, selected: [...input.selected], mode: input.mode,
      ...combinedInput(state.question, input), ...startingHelpInput(input) } };
  try {
    writeInput(storage, marker.input);
    const encoded = JSON.stringify(marker);
    storage.setItem(`${RESUME_PREFIX}${state.id}`, encoded);
    return storage.getItem(`${RESUME_PREFIX}${state.id}`) === encoded;
  } catch { return false; }
}

export function createGuideLoadGate() {
  let selected = null;
  return {
    begin() { selected = { pending: true }; return selected; },
    token() { return selected; },
    pending() { return selected?.pending === true; },
    current(token) { return token === selected; },
    finish(token) { if (token !== selected) return false; token.pending = false; return true; },
  };
}

export function recoverMissingGuideSelection(storage, selectedKey, guideId, gate, token) {
  if (!gate.current(token)) return false;
  if (storage.getItem(selectedKey) === guideId) storage.removeItem(selectedKey);
  return true;
}

export function guideOperationReadMatches(watched, current, next) {
  return current?.id === watched?.guide_id && current.version === watched.guide_version &&
    current.operation?.status === "running" && current.operation.request_id === watched.request_id &&
    current.operation.version === watched.operation_version && next?.id === watched.guide_id &&
    Number.isSafeInteger(next.version) && next.version >= current.version &&
    next.operation?.request_id === watched.request_id && next.operation.version === watched.operation_version &&
    ["running", "completed", "failed", "interrupted"].includes(next.operation.status);
}

/** Bind only the returned host operation matching the exact scoped button intent. */
export function preservedGuideResume(storage, state) {
  let marker;
  try { marker = JSON.parse(storage.getItem(`${RESUME_PREFIX}${state?.id}`)); } catch { return null; }
  const operation = state?.operation, request = operation?.request;
  const organizationIntent = marker?.intent;
  if (organizationIntent && (organizationIntent.action !== "organization" ||
      !/^[a-f0-9]{64}$/.test(organizationIntent.organization_fingerprint) ||
      request?.organization_fingerprint !== organizationIntent.organization_fingerprint)) return null;
  if (marker?.guide_id !== state?.id || !Number.isSafeInteger(marker?.guide_version) ||
      !ownedInput(marker?.input, state.id) || typeof marker.text !== "string" ||
      typeof operation?.request_id !== "string" || !/^[a-f0-9]{64}$/.test(operation.request_id) ||
      operation.version !== marker.guide_version || request?.version !== marker.guide_version ||
      request.action !== (organizationIntent ? "organization" : "chat") || request.text !== marker.text ||
      request.combine_options === true ||
      !Array.isArray(request.selected) || request.selected.length || request.stage != null || request.blueprint != null ||
      marker.request_id !== null && marker.request_id !== operation.request_id) return null;
  if (marker.request_id === null) {
    marker.request_id = operation.request_id;
    try { storage.setItem(`${RESUME_PREFIX}${state.id}`, JSON.stringify(marker)); } catch { return null; }
  }
  return marker;
}

/** Preserve live edits and original-question choices across this button's turn. */
export function recoverGuideResumeInput(storage, state, liveInput = null) {
  const marker = preservedGuideResume(storage, state);
  if (!marker) return null;
  let latest;
  try { latest = JSON.parse(storage.getItem(`${INPUT_PREFIX}${state.id}:latest`)); } catch {}
  const live = ownedInput(liveInput, state.id);
  let source = live ? liveInput : ownedInput(latest, state.id) ? latest : marker.input;
  if (!live && source === latest) {
    // Latest carries writing only. Restore context from the matching full input,
    // never from an older capture or a record with different unsent edits.
    try {
      const exact = JSON.parse(storage.getItem(source.key));
      if (ownedInput(exact, state.id) && exact.key === source.key &&
          exact.draft === source.draft && exact.mode === source.mode &&
          JSON.stringify(exact.selected) === JSON.stringify(source.selected) &&
          exact.combine_options === source.combine_options)
        source = { ...source, ...startingHelpInput(exact) };
    } catch {}
  }
  // Old answer IDs remain under their old question key, never in a new question.
  try { if (!storage.getItem(marker.input.key)) writeInput(storage, marker.input); } catch {}
  writeInput(storage, source);
  const restored = readInput(storage, state);
  if (source.key === restored.key || source.draft) {
    restored.draft = source.draft;
    restored.mode = source.mode;
    delete restored.starting_help;
    if (source.key === restored.key) Object.assign(restored, startingHelpInput(source));
  }
  writeInput(storage, restored);
  return restored;
}

export function recoverCompletedInput(storage, state) {
  const restored = readInput(storage, state);
  const operation = state.operation;
  if (operation?.status !== "completed") return restored;
  const preserved = recoverGuideResumeInput(storage, state);
  if (preserved) return preserved;
  let previous;
  try {
    previous = JSON.parse(storage.getItem(`${INPUT_PREFIX}${state.id}:latest`));
  } catch {}
  // Only text carries across a completed question. Choice IDs belong to that question.
  if (!restored.draft && previous?.key !== restored.key &&
      previous?.key?.startsWith(`${INPUT_PREFIX}${state.id}:`) &&
      typeof previous.draft === "string") {
    restored.draft = previous.draft;
    restored.mode = previous.mode === "chat" ? "chat" : "answer";
    delete restored.starting_help;
  }
  if (restored.draft === (operation.request?.text || "")) restored.draft = "";
  return admitSubmittedInput(restored, operation.request || {});
}
export function manualKey(id) {
  return `${MANUAL_PREFIX}${id || "new"}`;
}
export function installSteps(plan) {
  if (!plan) return [];
  const localKnowledge = plan.knowledge?.mode === "local";
  const anyKnowledge = plan.knowledge?.mode !== "none";
  const hasConnections = plan.connections?.some((connection) =>
    plan.agent_classes.some((agent) =>
      agent.tools?.mcp?.some((grant) => grant.connection_id === connection.id),
    ),
  );
  const steps = [
    {
      title: "Unzip into your own folder",
      detail: `For example ~/src/${plan.project.slug}.`,
    },
  ];
  if (plan.target === "gcp") {
    steps.push({
      title: "Review and create your cloud host",
      commands: [
        "bash deploy/deploy-gcp.sh --dry-run",
        "bash deploy/deploy-gcp.sh",
      ],
      detail:
        "The script asks for your exact project ID before creating billed resources. It prints recovery commands if a step fails.",
    });
    steps.push({
      title: "Connect and configure credentials on the VM",
      commands: [
        `gcloud compute ssh ${plan.project.slug} --project ${plan.gcp.project} --zone ${plan.gcp.zone} --tunnel-through-iap`,
        `cd ~/src/${plan.project.slug}`,
        ".venv/bin/python setup.py",
      ],
      detail:
        "Run the remaining setup commands in this SSH session. Credentials stay on the VM.",
    });
  } else
    steps.push({
      title: "Install and add your credentials",
      commands: ["bash install.sh"],
      detail:
        "The local setup asks for provider keys and the connection settings your plan needs.",
    });
  if (hasConnections)
    steps.push({
      title: "Install your connection prerequisites",
      detail:
        "Check config/connections.json and README.md for the selected services. Install their required Node, uv, Docker, or other executable on this deployment machine.",
    });
  if (localKnowledge)
    steps.push({
      title: "Start and verify the knowledge service",
      commands: [
        ".venv/bin/python knowledge/manage.py up",
        ".venv/bin/python knowledge/manage.py verify",
      ],
      detail:
        "Install Docker first. Setup has already prepared this deployment's private knowledge space and credentials.",
    });
  else if (anyKnowledge)
    steps.push({
      title: "Verify your existing knowledge service connection",
      commands: [".venv/bin/python knowledge/manage.py verify"],
      detail: "The configured endpoint, token, and space must match.",
    });
  steps.push({
    title: "Check the installation",
    commands: [
      "bash start.sh --doctor",
      ...(hasConnections || anyKnowledge
        ? ["bash start.sh --connections-check"]
        : []),
    ],
    detail:
      "These checks validate the installation and selected tool names before agents start.",
  });
  steps.push(
    plan.target === "gcp"
      ? {
          title: "Start the service, then open a local tunnel",
          commands: ["bash deploy/install-service.sh"],
          detail:
            "Back on your own computer, run bash deploy/tunnel-gcp.sh. Keep that tunnel open while using the console.",
        }
      : {
          title: "Start your agents",
          commands: ["bash start.sh"],
          detail: "Keep this terminal open. Stop the system with Control-C.",
        },
  );
  steps.push({
    title: "Open your MobKit console",
    detail: `Open http://127.0.0.1:${plan.advanced.port}/console in your browser. Your agents wait for your first message.`,
  });
  return steps;
}
export function unresolvedRequirements(state, plan) {
  const gaps = state?.brief?.requirements || state?.feature_gaps || [];
  const labels = gaps
    .filter(
      (gap) =>
        !["resolved", "supported", "accepted", "deferred", "cancelled"].includes(
          gap.disposition || gap.status,
        ),
    )
    .map((gap) =>
      typeof gap === "string"
        ? gap
        : gap.extension
          ? `${gap.outcome}: ${gap.extension}`
          : gap.outcome ||
            gap.description ||
            gap.reason ||
            gap.feature ||
            "A requested capability still needs a decision",
    );
  return [...new Set([...labels, ...(state?.warnings || [])])];
}

/** Project host categories, or exact saved feature fields, without guessing from prose.
 * @returns {{decisions: string[], implementation: string[], checks: string[]}}
 */
export function setupRequirementGroups(state) {
  const labels = (values) => [...new Set((Array.isArray(values) ? values : [])
    .filter((value) => typeof value === "string" && value.trim()))];
  const fallback = { decisions: [], implementation: [], checks: [] };
  const requirements = Array.isArray(state?.brief?.requirements)
    ? state.brief.requirements : Array.isArray(state?.feature_gaps) ? state.feature_gaps : [];
  for (const item of requirements) {
    if (typeof item === "string") {
      fallback.checks.push(item);
      continue;
    }
    if (!item || typeof item !== "object") continue;
    const disposition = item.disposition || item.status;
    if (["supported", "deferred", "cancelled", "resolved", "accepted"].includes(disposition)) continue;
    const outcome = typeof item.outcome === "string" ? item.outcome : item.description || item.reason || item.feature;
    const extension = typeof item.extension === "string" ? item.extension : "";
    const label = extension && typeof outcome === "string" ? `${outcome}: ${extension}` : outcome;
    if (disposition === "open") {
      fallback[extension ? "implementation" : "decisions"].push(label);
    } else if (disposition === "handoff") {
      fallback.implementation.push(label);
    } else {
      // Prerequisites and older unclassified checks have no design-decision authority.
      fallback.checks.push(label);
    }
  }
  return {
    decisions: labels(Array.isArray(state?.readiness?.decision_blockers)
      ? state.readiness.decision_blockers : fallback.decisions),
    implementation: labels(Array.isArray(state?.readiness?.implementation_blockers)
      ? state.readiness.implementation_blockers : fallback.implementation),
    checks: labels([...fallback.checks, ...(Array.isArray(state?.warnings) ? state.warnings : [])]),
  };
}
