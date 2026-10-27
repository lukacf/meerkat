import type { ConversationIdentity, ConversationMessageEntry } from "./conversation";
import type { ConsoleFrame } from "./runtime-types";

/** Project only a native invocation locator. UI data stays behind the host API. */
export function mcpAppEntryFromFrame(
  frame: ConsoleFrame,
  entryId: string,
  identity: ConversationIdentity,
): (ConversationMessageEntry & { renderKey: string }) | null {
  if (frame.event !== "mcp_app"
    || (frame.sourceKind !== "session_history" && frame.sourceKind !== "tool_application")) return null;
  const data = frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown> : null;
  const memberIdentity = frame.identity?.trim();
  const sessionId = typeof data?.session_id === "string" ? data.session_id.trim() : "";
  const toolCallId = typeof data?.tool_call_id === "string" ? data.tool_call_id.trim() : "";
  if (!memberIdentity || !sessionId || !toolCallId || (frame.sessionId && frame.sessionId !== sessionId)) return null;
  const date = typeof frame.timestampMs === "number" ? new Date(frame.timestampMs) : null;
  return {
    kind: "message", id: entryId, variant: "plain",
    renderKey: JSON.stringify(["mcp-app", frame.runtimeKey || "", memberIdentity, sessionId, toolCallId]),
    identity: { ...identity, id: memberIdentity },
    createdAt: date && Number.isFinite(date.getTime()) ? date.toISOString() : undefined,
    interactionId: frame.interactionId?.trim() || undefined,
    runId: frame.runId?.trim() || undefined,
    mcpApp: { sessionId, toolCallId },
    text: typeof data?.fallback === "string" ? data.fallback : "Tool result",
  };
}
