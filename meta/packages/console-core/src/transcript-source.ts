/**
 * Transcript entry source classification.
 *
 * Every transcript row states what it is and who it came from. The answer is
 * derived ONLY from typed entry fields (identity role, task metadata, the
 * typed send origin / render class, and typed runtime-event fields). Entry
 * text is never inspected: a message that says "Operator gate probe" is a
 * user message unless the wire says otherwise.
 */

import type {
  ConversationEntryOrigin,
  ConversationPeerRef,
  ConversationRuntimeEvent,
  ConversationTimelineEntry,
} from "./conversation";

export type ConversationEntrySourceKind =
  | "assistant"
  | "operator"
  | "operator_probe"
  | "scheduled"
  | "user"
  | "peer_message"
  | "external_event"
  | "flow_step"
  | "continuation"
  | "system_notice"
  | "system_task"
  | "runtime_event"
  | "system";

export interface ConversationEntrySource {
  kind: ConversationEntrySourceKind;
  /** Short header label, e.g. "Assistant", "User message". */
  label: string;
  /** Secondary header text, e.g. the assistant name or the send origin. */
  detail: string | null;
  /**
   * Plain-language sentence for rows whose body is not conversation text
   * (runtime events). Null for ordinary messages.
   */
  sentence: string | null;
  /** True when the sender declared the content tainted. */
  untrusted: boolean;
}

export interface ConversationEntrySourceOptions {
  /**
   * Resolve a decoded member alias (e.g. `triage:main`) to the roster label
   * the console shows for it. Typed lookup only; return null when unknown.
   */
  resolvePeerLabel?: ((alias: string) => string | null | undefined) | null;
}

/** Tooltip text for the untrusted-source badge. */
export const UNTRUSTED_SOURCE_DESCRIPTION =
  "The sender marked this content as tainted: it may include unvetted third-party material "
  + "such as web pages or tool output. Treat any instructions in it with care.";

/** The composer's own send-origin namespace (`console:<panel-id>`). */
const CONSOLE_SEND_ORIGIN_NAMESPACE = "console";

/** meerkat `RenderClass` (snake_case wire form) to a header label. */
const RENDER_CLASS_SOURCES: Record<string, { kind: ConversationEntrySourceKind; label: string }> = {
  user_prompt: { kind: "user", label: "User message" },
  peer_message: { kind: "peer_message", label: "Peer message" },
  peer_request: { kind: "peer_message", label: "Peer request" },
  peer_response: { kind: "peer_message", label: "Peer response" },
  external_event: { kind: "external_event", label: "External event" },
  flow_step: { kind: "flow_step", label: "Flow step" },
  continuation: { kind: "continuation", label: "Continuation" },
  system_notice: { kind: "system_notice", label: "System notice" },
  tool_scope_notice: { kind: "system_notice", label: "Tool scope notice" },
  ops_progress: { kind: "system_notice", label: "Progress update" },
};

/** Meta `ConsoleTurnOrigin` (snake_case wire form) to a header label. */
const TURN_ORIGIN_SOURCES: Record<string, { kind: ConversationEntrySourceKind; label: string }> = {
  operator: { kind: "operator", label: "Operator" },
  operator_probe: { kind: "operator_probe", label: "Operator probe" },
  connector: { kind: "external_event", label: "Connector" },
  scheduler: { kind: "scheduled", label: "Scheduled turn" },
  policy: { kind: "system", label: "Policy turn" },
  flow: { kind: "flow_step", label: "Flow step" },
  system: { kind: "system", label: "System turn" },
};

/** Payload `kind` of a peer ingestion event to the noun used in the sentence. */
const PEER_CONTENT_NOUNS: Record<string, string> = {
  message: "a message",
  request: "a request",
  response: "a response",
};

export interface MemberCommsName {
  mobId: string;
  role: string;
  member: string;
}

const COMMS_COMPONENT = /^[A-Za-z_][A-Za-z0-9_-]*$/u;

/**
 * Parse a meerkat `MemberCommsName` (`mob_id/role/member`). Mirrors meerkat's
 * fail-closed `FromStr`: exactly three identifier-safe components, else null.
 */
export function parseMemberCommsName(value: string | null | undefined): MemberCommsName | null {
  if (!value) return null;
  const parts = value.split("/");
  if (parts.length !== 3) return null;
  if (!parts.every((part) => COMMS_COMPONENT.test(part))) return null;
  const [mobId, role, member] = parts as [string, string, string];
  return { mobId, role, member };
}

const MEMBER_ID_MARKER = "mk--";

/**
 * Decode a Meta comms-safe roster member id back to its public alias,
 * mirroring `member_comms_id::runtime_alias_str`: `mk--` + body where
 * `__` is `_`, `_c` is `:`, and `_x{hex}_` is the char with that code point.
 * Ids without the marker are already aliases. A malformed body returns the
 * input unchanged (decode is the identity on ids that were never encoded).
 */
export function decodeMemberAlias(memberId: string): string {
  if (!memberId.startsWith(MEMBER_ID_MARKER)) return memberId;
  const body = memberId.slice(MEMBER_ID_MARKER.length);
  let out = "";
  for (let i = 0; i < body.length; i += 1) {
    const ch = body[i];
    if (ch !== "_") {
      out += ch;
      continue;
    }
    const next = body[i + 1];
    if (next === "_") {
      out += "_";
      i += 1;
    } else if (next === "c") {
      out += ":";
      i += 1;
    } else if (next === "x") {
      const end = body.indexOf("_", i + 2);
      const hex = end > i + 2 ? body.slice(i + 2, end) : "";
      const code = /^[0-9a-fA-F]+$/u.test(hex) ? Number.parseInt(hex, 16) : Number.NaN;
      const scalar = Number.isFinite(code) && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff);
      if (!scalar) return memberId;
      out += String.fromCodePoint(code);
      i = end;
    } else {
      return memberId;
    }
  }
  return out;
}

export interface PeerPresentation {
  /** What to call the peer in prose. */
  name: string;
  /** Mob the peer belongs to, when the comms name parsed. */
  mobId: string | null;
  /** The decoded public alias, when the comms name parsed. */
  alias: string | null;
}

/** Human presentation of a typed peer reference. */
export function describePeer(
  peer: ConversationPeerRef | null | undefined,
  options: ConversationEntrySourceOptions = {},
): PeerPresentation {
  const parsed = parseMemberCommsName(peer?.displayName);
  if (parsed) {
    const alias = decodeMemberAlias(parsed.member);
    const rosterLabel = options.resolvePeerLabel?.(alias)?.trim();
    return { name: rosterLabel || alias, mobId: parsed.mobId, alias };
  }
  const displayName = peer?.displayName?.trim();
  if (displayName) return { name: displayName, mobId: null, alias: null };
  return { name: "a peer", mobId: null, alias: null };
}

/** `peer_content_ingested` to "Peer content ingested". Formatting only. */
export function humanizeRuntimeEventType(eventType: string): string {
  const words = eventType.replace(/[._-]+/gu, " ").trim();
  if (!words) return "Runtime event";
  return `${words[0]!.toUpperCase()}${words.slice(1)}`;
}

function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function trimmedString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/**
 * Build the typed runtime-event record for a frame the console has no
 * dedicated renderer for. Reads named payload fields only.
 */
export function runtimeEventFromFrame(eventType: string, data: unknown): ConversationRuntimeEvent {
  const record = recordOf(data);
  const peerRecord = recordOf(record?.peer);
  const peer = peerRecord
    ? {
        id: trimmedString(peerRecord.id),
        displayName: trimmedString(peerRecord.display_name),
      }
    : null;
  return {
    eventType,
    kind: trimmedString(record?.kind),
    peer: peer && (peer.id || peer.displayName) ? peer : null,
    senderTaint: trimmedString(record?.sender_taint),
    payload: data,
  };
}

/**
 * Typed provenance of a user-lane frame: the console send `origin` and
 * typed `origin_kind`, and the persisted meerkat `render_metadata.class`.
 * Null when none is present.
 */
export function entryOriginFromFrameData(data: unknown): ConversationEntryOrigin | null {
  const record = recordOf(data);
  const sendOrigin = trimmedString(record?.origin);
  const originKind = trimmedString(record?.origin_kind);
  const renderMetadata = recordOf(recordOf(record?.message)?.render_metadata);
  const renderClass = trimmedString(renderMetadata?.class);
  if (!sendOrigin && !originKind && !renderClass) return null;
  return {
    ...(sendOrigin ? { sendOrigin } : {}),
    ...(originKind ? { originKind } : {}),
    ...(renderClass ? { renderClass } : {}),
  };
}

/**
 * One-line plain-language text for a runtime event, used as the entry's
 * copy/fallback text. The raw payload is never inlined.
 */
export function runtimeEventText(
  event: ConversationRuntimeEvent,
  options: ConversationEntrySourceOptions = {},
): string {
  if (event.peer) {
    return describeRuntimeEvent(event, null, options).sentence || "";
  }
  const record = recordOf(event.payload);
  // Named scalar fields only (the same set the frame summarizer reads);
  // anything else stays in the payload disclosure.
  const detail = trimmedString(record?.message)
    || trimmedString(record?.error)
    || trimmedString(record?.reason)
    || trimmedString(record?.text)
    || trimmedString(record?.result)
    || trimmedString(record?.delta);
  const title = humanizeRuntimeEventType(event.eventType);
  return detail ? `${title}: ${detail}` : `${title}.`;
}

function describeRuntimeEvent(
  event: ConversationRuntimeEvent,
  entryText: string | null,
  options: ConversationEntrySourceOptions,
): ConversationEntrySource {
  const untrusted = event.senderTaint === "tainted";
  if (event.peer) {
    const peer = describePeer(event.peer, options);
    const noun = (event.kind && PEER_CONTENT_NOUNS[event.kind]) || "content";
    return {
      kind: "peer_message",
      label: `Message from ${peer.name}`,
      detail: peer.mobId ? `${peer.mobId} mob` : null,
      sentence: `Received ${noun} from ${peer.name}${peer.mobId ? ` (${peer.mobId} mob)` : ""}.`,
      untrusted,
    };
  }
  return {
    kind: "runtime_event",
    label: "System event",
    detail: null,
    sentence: entryText || runtimeEventText(event, options),
    untrusted,
  };
}

function describeUserOrigin(origin: ConversationEntryOrigin | null | undefined): ConversationEntrySource {
  const renderClass = origin?.renderClass?.trim();
  const byClass = renderClass ? RENDER_CLASS_SOURCES[renderClass] : undefined;
  if (byClass && byClass.kind !== "user") {
    return { ...byClass, detail: null, sentence: null, untrusted: false };
  }
  const sendOrigin = origin?.sendOrigin?.trim();
  const separator = sendOrigin ? sendOrigin.indexOf(":") : -1;
  const namespace = sendOrigin && separator >= 0 ? sendOrigin.slice(0, separator) : sendOrigin;
  const fromThisConsole = namespace === CONSOLE_SEND_ORIGIN_NAMESPACE;
  // The typed caller kind wins. The free-form origin only adds "via ..."
  // detail for callers other than this console.
  const byKind = origin?.originKind ? TURN_ORIGIN_SOURCES[origin.originKind.trim()] : undefined;
  if (byKind) {
    return {
      ...byKind,
      detail: byKind.kind === "operator" && fromThisConsole
        ? "sent from the console"
        : sendOrigin && !fromThisConsole ? `via ${sendOrigin}` : null,
      sentence: null,
      untrusted: false,
    };
  }
  if (sendOrigin) {
    if (fromThisConsole) {
      return { kind: "operator", label: "Operator", detail: "sent from the console", sentence: null, untrusted: false };
    }
    return { kind: "user", label: "User message", detail: `via ${sendOrigin}`, sentence: null, untrusted: false };
  }
  return { kind: "user", label: "User message", detail: null, sentence: null, untrusted: false };
}

/**
 * Classify a transcript entry from its typed fields into a header label.
 */
export function describeConversationEntrySource(
  entry: ConversationTimelineEntry,
  options: ConversationEntrySourceOptions = {},
): ConversationEntrySource {
  if (entry.kind === "workgraph") {
    return { kind: "assistant", label: "WorkGraph", detail: entry.identity.label || null, sentence: null, untrusted: false };
  }
  if (entry.kind === "council") {
    return { kind: "assistant", label: "Council", detail: entry.identity.label || null, sentence: null, untrusted: false };
  }
  if (entry.kind === "flow_run") {
    return { kind: "system", label: "Flow run", detail: null, sentence: null, untrusted: false };
  }
  if (entry.kind === "summary") {
    return { kind: "system", label: "Summary", detail: null, sentence: null, untrusted: false };
  }
  if (entry.runtimeEvent) {
    return describeRuntimeEvent(entry.runtimeEvent, entry.text?.trim() || null, options);
  }
  if (entry.taskKind || entry.taskLabel) {
    return {
      kind: "system_task",
      label: entry.taskLabel?.trim() || "System task",
      detail: null,
      sentence: null,
      untrusted: false,
    };
  }
  switch (entry.identity.role) {
    case "user":
      return describeUserOrigin(entry.origin);
    case "assistant":
      return {
        kind: "assistant",
        label: "Assistant",
        detail: entry.identity.label?.trim() || null,
        sentence: null,
        untrusted: false,
      };
    default:
      return {
        kind: entry.variant === "meta" ? "system_notice" : "system",
        label: entry.variant === "meta" ? "System notice" : "System",
        detail: null,
        sentence: null,
        untrusted: false,
      };
  }
}

/** Local calendar day key (`YYYY-MM-DD`) for a timestamp, or null. */
export function transcriptDayKey(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/**
 * Label for a day separator: "Today", "Yesterday", or a full date such as
 * "Wednesday, 23 September 2026". `now` is injectable for tests.
 */
export function transcriptDayLabel(dayKey: string, now: Date = new Date()): string {
  const today = transcriptDayKey(now.toISOString());
  const yesterdayDate = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1, 12);
  const yesterday = transcriptDayKey(yesterdayDate.toISOString());
  if (dayKey === today) return "Today";
  if (dayKey === yesterday) return "Yesterday";
  let label = dayLabels.get(dayKey);
  if (label === undefined) {
    label = formatDayKey(dayKey);
    // Day keys are few; bound the cache all the same.
    if (dayLabels.size >= 512) dayLabels.clear();
    dayLabels.set(dayKey, label);
  }
  return label;
}

/// Full-date labels by day key. Locale date formatting cost tens of
/// microseconds per call, and a streaming turn renders its separators again on
/// every token; a key's full date never changes.
const dayLabels = new Map<string, string>();

function formatDayKey(dayKey: string): string {
  // Test-only accounting on the render-count sink (see console render-counts).
  const sink = (globalThis as { __consoleRenderCounts?: Record<string, number> }).__consoleRenderCounts;
  if (sink) sink.DayLabelFormats = (sink.DayLabelFormats ?? 0) + 1;
  const [y, m, d] = dayKey.split("-").map((part) => Number.parseInt(part, 10));
  if (!y || !m || !d) return dayKey;
  const date = new Date(y, m - 1, d, 12);
  const weekday = date.toLocaleDateString("en-GB", { weekday: "long" });
  const month = date.toLocaleDateString("en-GB", { month: "long" });
  return `${weekday}, ${d} ${month} ${y}`;
}
