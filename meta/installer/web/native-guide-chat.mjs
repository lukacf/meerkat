import {
  buildConversationViewState,
  parseConversationRichBlocks,
  parseSseFrames,
  mapFramesToTimelineEntries,
  sortConversationTimelineEntries,
  inferResponsePhaseFromFrames,
} from "../../packages/console-core/src/index.ts";
import {
  createIdentityLogCore,
  pushFrame,
  mergeFrameUpdate,
  sortedEvents,
  cursorSeq,
} from "../../console/src/lib/identity-log.ts";

const GUIDE_AGENT = {
  identity: "architect:main", agent_id: "architect:main", member_id: "architect:main",
  label: "Your setup guide", kind: "host", role: "Forge architect",
};
const GUIDE_IDENTITY = {
  id: "architect:main", label: "Your setup guide", role: "assistant",
  presentation: "assistant", showLabel: false,
};
const USER_IDENTITY = { id: "user", label: "You", role: "user", presentation: "user" };

function currentGuideInteraction(state, bindings) {
  const operation = state?.operation;
  const identities = new Set(typeof operation?.request_id === "string" && operation.request_id &&
    typeof operation.started_at === "string" && operation.started_at
    ? bindings.filter((binding) => binding.operation_request_id === operation.request_id &&
      binding.operation_started_at === operation.started_at && typeof binding.interaction_id === "string" && binding.interaction_id)
      .map((binding) => binding.interaction_id) : []);
  return identities.size === 1 ? [...identities][0] : null;
}

export function guideCurrentToolActivity({ state, frames = [], bindings = [] }) {
  if (state?.operation?.status !== "running") return null;
  const interactionId = currentGuideInteraction(state, bindings);
  if (!interactionId) return null;
  const entries = mapFramesToTimelineEntries(GUIDE_AGENT,
    frames.filter((frame) => frame.interactionId === interactionId));
  const tools = entries.flatMap((entry) => entry.blocks || []).filter((block) => block.type === "tool-call");
  const last = tools.at(-1);
  // Project only the Console's public name and status. Arguments, results and
  // unpublished proposal content remain in their existing transcript cards.
  return last && typeof last.name === "string" && last.name
    ? { name: last.name, status: last.status } : null;
}

export function createGuideFrameLog() {
  return createIdentityLogCore();
}

export function nextGuideTimelineCursor(after, page, seenBindings) {
  const current = cursorSeq(after);
  let replayFrom = null;
  for (const binding of page.bindings || []) {
    if (!binding.interaction_id || seenBindings.has(binding.interaction_id)) continue;
    seenBindings.add(binding.interaction_id);
    const admitted = cursorSeq(binding.cursor);
    // A native send can reach the timeline before its setup binding is saved.
    // Recover that newly bound interaction even when a previous poll advanced
    // past its first frame. Existing frame IDs make this one replay harmless.
    if (current !== null && admitted !== null && admitted <= current) {
      const beforeAdmission = Math.max(0, admitted - 1);
      replayFrom = replayFrom === null ? beforeAdmission : Math.min(replayFrom, beforeAdmission);
    }
  }
  return replayFrom !== null
    ? { after: `console:${replayFrom}`, replay: true }
    : { after: page.next_cursor || page.latest_cursor || after, replay: false };
}

export function ingestGuideTimeline(log, page) {
  // Use the Console wire parser and update semantics, including versioned
  // frames whose tool status changes in place after the first delivery.
  const frames = parseSseFrames((page.frames || []).map((frame) =>
    `event: ${frame.kind || "message"}\ndata: ${JSON.stringify({ type: "console_frame", frame })}\n\n`,
  ).join(""));
  for (const frame of frames) {
    if (frame.event === "frame_updated") {
      const updated = frame.data?.frame;
      if (updated && !mergeFrameUpdate(log, updated)) pushFrame(log, updated.id, updated);
    } else if (!mergeFrameUpdate(log, frame)) {
      pushFrame(log, frame.id || frame.cursor, frame);
    }
  }
  return sortedEvents(log);
}

function savedMessageEntry(message, index) {
  const role = message.role || "assistant";
  const identity = role === "user" ? USER_IDENTITY : role === "system"
    ? { id: "setup-system", label: "Setup", role: "system", presentation: "system" }
    : GUIDE_IDENTITY;
  const attachments = role === "user" ? message.attachments || [] : [];
  const richUser = attachments.length > 0;
  const labels = attachments.map((ref) =>
    `[${ref.filename.replace(/[\\[\]()*_`]/g, "\\$&")}](${ref.url})`);
  const blocks = richUser ? [
    ...parseConversationRichBlocks(`${message.text || ""}\n\n${labels.join("\n\n")}`, { displayNormalization: false }),
    ...attachments.filter((ref) => ref.media_type.startsWith("image/")).map((ref) =>
      ({type: "image", src: ref.url, mediaType: ref.media_type, alt: ref.filename})),
  ] : parseConversationRichBlocks(message.text || "", { displayNormalization: false });
  return {
    id: `setup-message:${message.id || index}`,
    kind: "message", identity, createdAt: message.created_at,
    variant: role === "user" && !richUser ? "plain" : "rich",
    ...(role === "user" && !richUser ? { text: message.text || "" } : { blocks }),
  };
}

export function guideConversation({ state, frames = [], bindings = [], pendingMessage = null }) {
  const savedIds = new Set((state.messages || []).map((message) => message.id));
  const published = new Set(bindings.filter((binding) => savedIds.has(binding.assistant_message_id))
    .map((binding) => binding.interaction_id));
  const interactions = new Map();
  for (const frame of frames) {
    const key = frame.interactionId || "";
    if (!interactions.has(key)) interactions.set(key, []);
    interactions.get(key).push(frame);
  }
  // The reusable adapter does not yet retain interaction IDs on every user
  // and tool entry. Adapt each native interaction as a unit, then preserve
  // that authoritative identity for saved-publication reconciliation.
  const nativeEntries = [...interactions].flatMap(([interactionId, events]) =>
    mapFramesToTimelineEntries(GUIDE_AGENT, events, {
      renderInteractionStartsAsUser: true,
      renderTextDeltas: true,
      suppressEmbeddedRunStartedPrompt: true,
    }).map((entry) => ({ ...entry, ...(interactionId ? { interactionId } : {}) })),
  ).map((entry) => ({ ...entry, ...(entry.blocks ? { blocks: entry.blocks.map((block) =>
    block.type === "thinking" ? { ...block, label: block.label || "Reasoning summary",
      ...(published.has(entry.interactionId) ? { final: true, persisted: true } : {}) }
      : block) } : {}) })).flatMap((entry) => {
    if (entry.identity.role !== "assistant" || !published.has(entry.interactionId)) return [entry];
    // The guide publishes validated conversation prose separately from the
    // native structured proposal. Keep native activity and its provenance.
    const activity = entry.blocks?.filter((block) => ["thinking", "tool-call"].includes(block.type));
    return activity?.length ? [{ ...entry, blocks: activity }] : [];
  });
  const nativeUsers = new Set(nativeEntries.filter((entry) => entry.identity.role === "user")
    .map((entry) => entry.interactionId));
  const boundSavedUsers = new Set(bindings.filter((binding) => nativeUsers.has(binding.interaction_id))
    .map((binding) => binding.user_message_id));
  const savedEntries = (state.messages || []).flatMap((message, index) => {
    if (boundSavedUsers.has(message.id)) return [];
    // The initial question already supplies the opening invitation.
    if (index === 0 && message.role === "assistant" && !state.messages.some((item) => item.role === "user")) return [];
    return [savedMessageEntry(message, index)];
  });
  const operation = state.operation;
  const currentInteraction = currentGuideInteraction(state, bindings);
  const pendingIsBound = pendingMessage && operation?.status === "running" && currentInteraction && bindings.some((binding) =>
    binding.interaction_id === currentInteraction && !published.has(binding.interaction_id) && binding.user_text === pendingMessage.text &&
    JSON.stringify((binding.attachments || []).map((ref) => ref.id)) ===
      JSON.stringify((pendingMessage.attachments || []).map((ref) => ref.id)) &&
    nativeUsers.has(binding.interaction_id));
  const pending = pendingMessage?.text && !pendingIsBound ? [{
    ...savedMessageEntry(pendingMessage, "pending"),
    id: "setup-message:pending", createdAt: pendingMessage.created_at,
  }] : [];
  const entries = sortConversationTimelineEntries([...savedEntries, ...nativeEntries, ...pending]);
  const currentFrames = currentInteraction ? interactions.get(currentInteraction) || [] : [];
  return {
    viewState: {
      ...buildConversationViewState({ memberId: "architect:main",
        agentLabel: "Your setup guide", agent: GUIDE_AGENT, entries }),
      conversationId: `setup:${state.id}`,
    },
    phase: operation?.status === "running"
      ? inferResponsePhaseFromFrames(currentFrames, "waiting") || "waiting" : null,
    entries,
  };
}
