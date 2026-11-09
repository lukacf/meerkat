import type { ConversationTimelineEntry } from "@console-core";

function richBlockHasVisibleContent(block: unknown): boolean {
  if (!block || typeof block !== "object") return false;
  const record = block as Record<string, unknown>;
  // A typed job has a visible header even when its outcome detail is empty.
  if (record.type === "background-job") {
    return typeof record.jobId === "string" && record.jobId.trim().length > 0
      && typeof record.status === "string" && record.status.trim().length > 0;
  }
  // A typed member-kickoff status is visible by its member and phase.
  if (record.type === "member-kickoff") {
    return typeof record.member === "string" && record.member.trim().length > 0
      && typeof record.phase === "string" && record.phase.trim().length > 0;
  }
  if (record.type === "markdown") return typeof record.source === "string" && record.source.trim().length > 0;
  const scalarText = [
    typeof record.text === "string" ? record.text : "",
    typeof record.label === "string" ? record.label : "",
    typeof record.result === "string" ? record.result : "",
    typeof record.body === "string" ? record.body : "",
    typeof record.title === "string" ? record.title : "",
    typeof record.name === "string" ? record.name : "",
  ]
    .join(" ")
    .trim();
  if (scalarText.length > 0) return true;
  if (
    record.type === "image" &&
    (typeof record.src === "string" || typeof record.blobId === "string")
  )
    return true;
  if (
    Array.isArray(record.headers) &&
    record.headers.some((v) => String(v || "").trim().length > 0)
  )
    return true;
  if (
    Array.isArray(record.rows) &&
    record.rows.some(
      (row) =>
        Array.isArray(row) &&
        row.some((v) => String(v || "").trim().length > 0),
    )
  )
    return true;
  return false;
}

// Sanitized form per entry (null when nothing is visible). An entry whose
// blocks are all visible is returned as is, so unchanged entries keep their
// identity from render to render and memoised rows downstream can bail out.
const sanitizedEntries = new WeakMap<ConversationTimelineEntry, ConversationTimelineEntry | null>();

function sanitizeConversationEntry(entry: ConversationTimelineEntry): ConversationTimelineEntry | null {
  if (entry.kind !== "message") return entry;
  if (entry.variant === "rich" && Array.isArray(entry.blocks)) {
    const blocks = entry.blocks.filter(richBlockHasVisibleContent);
    if (!blocks.length) return null;
    return blocks.length === entry.blocks.length ? entry : { ...entry, blocks };
  }
  return entry.text && entry.text.trim().length > 0 ? entry : null;
}

export function sanitizeConversationEntries(
  entries: ConversationTimelineEntry[],
): ConversationTimelineEntry[] {
  const sanitized: ConversationTimelineEntry[] = [];
  for (const entry of entries) {
    let result = sanitizedEntries.get(entry);
    if (result === undefined) {
      result = sanitizeConversationEntry(entry);
      sanitizedEntries.set(entry, result);
    }
    if (result) sanitized.push(result);
  }
  return sanitized;
}
