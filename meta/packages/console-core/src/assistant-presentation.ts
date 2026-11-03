import type { ConversationMessageEntry, ConversationTimelineEntry } from "./conversation";
import type { ConversationRichBlock } from "./rich-content";
import type { ConsoleFrame } from "./runtime-types";

function toolScope(frame: ConsoleFrame | undefined): string[] | undefined {
  if (!frame) return undefined;
  const values = [frame.runtimeKey, frame.identity, frame.sessionId, frame.runId];
  return values.every((value): value is string => typeof value === "string" && Boolean(value.trim()))
    ? values as string[] : undefined;
}

function typedToolIds(frame: ConsoleFrame): string[] {
  const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
  if (["tool_call_requested", "tool_call", "tool_execution_started"].includes(frame.event)) {
    const id = data.tool_call_id ?? data.id;
    return typeof id === "string" && id.trim() ? [id] : [];
  }
  const message = data.message && typeof data.message === "object" ? data.message as Record<string, unknown> : {};
  if (frame.sourceKind !== "session_history" || !Array.isArray(message.blocks)) return [];
  return message.blocks.flatMap(value => {
    if (!value || typeof value !== "object") return [];
    const block = value as Record<string, unknown>;
    const data = block.data && typeof block.data === "object" ? block.data as Record<string, unknown> : block;
    return (block.block_type ?? block.type) === "tool_use" && typeof data.id === "string" && data.id.trim() ? [data.id] : [];
  });
}

/** Presentation segments do not replace the canonical entry or its source ID.
 * A saved assistant message bundles blocks that arrived as separate live rows. */
/** Typed tool ownership across the source frames, read by
 * `assistantPresentationEntries`. Callers that render repeatedly over a
 * growing frame list build it once and extend it per new frame. */
export interface AssistantToolOwnership {
  scopedLiveTools: Set<string>;
  canonicalOwners: Map<string, Set<string>>;
}

export function assistantToolOwnership(frames: Iterable<ConsoleFrame> = []): AssistantToolOwnership {
  const ownership: AssistantToolOwnership = { scopedLiveTools: new Set(), canonicalOwners: new Map() };
  for (const frame of frames) extendAssistantToolOwnership(ownership, frame);
  return ownership;
}

export function extendAssistantToolOwnership(ownership: AssistantToolOwnership, frame: ConsoleFrame): void {
  const scope = toolScope(frame);
  if (!scope) return;
  for (const id of typedToolIds(frame)) {
    const key = JSON.stringify([...scope, id]);
    if (frame.sourceKind !== "session_history") ownership.scopedLiveTools.add(key);
    else if (["assistant_message", "text_complete"].includes(frame.event)) {
      const owners = ownership.canonicalOwners.get(key) ?? new Set<string>();
      owners.add(frame.id);
      ownership.canonicalOwners.set(key, owners);
    }
  }
}

export function assistantPresentationEntries(
  entries: ConversationTimelineEntry[],
  occurrenceKeys: ReadonlyMap<string, string | undefined>,
  sourceFrames: ReadonlyMap<string, ConsoleFrame> = new Map(),
  ownership: AssistantToolOwnership = assistantToolOwnership(sourceFrames.values()),
): ConversationTimelineEntry[] {
  const presenter = assistantPresenter(occurrenceKeys, sourceFrames, ownership);
  return entries.map(entry => presenter.present(entry));
}

/** Presentation ordinals after some entries: per owner, per lane. */
export type AssistantPresenterState = ReadonlyMap<string, ReadonlyMap<string, number>>;

/** Presents entries in order. Ordinals carry across entries; a presenter
 * created from a snapshot continues as if it had presented the entries
 * before it, without presenting them again. */
export interface AssistantPresenter {
  present(entry: ConversationTimelineEntry): ConversationTimelineEntry;
  snapshot(): AssistantPresenterState;
}

export function assistantPresenter(
  occurrenceKeys: ReadonlyMap<string, string | undefined>,
  sourceFrames: ReadonlyMap<string, ConsoleFrame>,
  ownership: AssistantToolOwnership,
  from?: AssistantPresenterState,
): AssistantPresenter {
  const ordinals = new Map<string, Map<string, number>>();
  const { scopedLiveTools, canonicalOwners } = ownership;
  // A snapshot is never written: copy an owner's counters on first use.
  const countersFor = (owner: string) => {
    let counters = ordinals.get(owner);
    if (!counters) {
      counters = new Map(from?.get(owner) ?? []);
      ordinals.set(owner, counters);
    }
    return counters;
  };
  const present = (entry: ConversationTimelineEntry): ConversationTimelineEntry => {
    const occurrence = entry.kind === "message" && entry.identity.role === "assistant"
      ? occurrenceKeys.get(entry.id) : undefined;
    if (entry.kind !== "message" || entry.identity.role !== "assistant") return entry;
    const source = sourceFrames.get(entry.id);
    const scope = toolScope(source);
    const ownedToolIds = new Set(source ? typedToolIds(source) : []);
    const toolKey = (block: ConversationRichBlock): string | undefined => {
      if (block.type !== "tool-call" || !scope || !block.toolCallId.trim()) return undefined;
      // Grouped live calls can share one entry. Require the exact typed call
      // in that entry's scope, never a synthesized display ID or tool name.
      const key = JSON.stringify([...scope, block.toolCallId]);
      if ((canonicalOwners.get(key)?.size ?? 0) > 1) return undefined;
      const owned = source && (ownedToolIds.has(block.toolCallId)
        || (source.sourceKind !== "session_history" && ownedToolIds.size > 0 && scopedLiveTools.has(key)));
      return owned ? `tool:${key}` : undefined;
    };
    if (!occurrence && !entry.blocks?.some(block => toolKey(block))) return entry;
    const counters = countersFor(occurrence ?? entry.id);
    const nextKey = (lane: string) => {
      const ordinal = counters.get(lane) ?? 0;
      counters.set(lane, ordinal + 1);
      const owner = occurrence ?? entry.renderKey ?? entry.id;
      return lane === "text"
        ? ordinal === 0 ? owner : `${owner}:part:${ordinal}`
        : `${owner}:${lane}:${ordinal}`;
    };
    if (!entry.blocks?.length) return { ...entry, assistantOccurrenceKey: occurrence, renderKey: nextKey("text") };
    const segments: { lane: string; blocks: ConversationRichBlock[] }[] = [];
    for (const block of entry.blocks) {
      const lane = block.type === "thinking" ? "thinking"
        : block.type === "tool-call" ? "tool" : block.type === "image" ? "image" : "text";
      const previous = segments.at(-1);
      // Each reasoning occurrence stays independent, even with equal text.
      // Markdown blocks retain the source message's text-segment boundaries.
      if (lane === "text" && block.type !== "markdown" && previous?.lane === lane) {
        previous.blocks.push(block);
      } else {
        segments.push({ lane, blocks: [block] });
      }
    }
    const presentationRows = segments.map(segment => {
      const renderKey = toolKey(segment.blocks[0]) ?? nextKey(segment.lane);
      let textIndex = 0;
      return { renderKey, blocks: segment.blocks.map(block => block.type === "markdown"
        ? { ...block, id: `${renderKey}:text:${textIndex++}` } : block) };
    });
    const onlyScopedTools = segments.every(segment => segment.lane === "tool" && segment.blocks.every(block => toolKey(block)));
    return { ...entry, assistantOccurrenceKey: onlyScopedTools ? undefined : occurrence, renderKey: presentationRows[0].renderKey,
      blocks: presentationRows.flatMap(row => row.blocks), presentationRows };
  };
  return {
    present,
    snapshot() {
      const state = new Map<string, ReadonlyMap<string, number>>(from ?? []);
      for (const [owner, counters] of ordinals) state.set(owner, new Map(counters));
      return state;
    },
  };
}

/** Expand only at the rendering boundary. Quotes still identify the source frame. */
export function conversationPresentationRows(entries: ConversationTimelineEntry[]): ConversationTimelineEntry[] {
  return entries.flatMap(entry => {
    if (entry.kind !== "message" || !entry.presentationRows) return [entry];
    const { presentationRows, ...source } = entry;
    return presentationRows.map(row => ({ ...source, ...row } satisfies ConversationMessageEntry));
  });
}
