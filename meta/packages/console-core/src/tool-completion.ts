/** Evidence at the presentation boundary, never inferred from result prose. */
export type ToolCompletionOutcome = "unknown" | "running" | "success" | "error" | "cancelled" | "interrupted";
export type ToolCompletionEvidence = {
  outcome: ToolCompletionOutcome;
  source: "runtime-result" | "session-history" | "runtime-start" | "unknown";
  toolCallId: string;
};
export function unknownToolCompletion(toolCallId: string): ToolCompletionEvidence {
  return { outcome: "unknown", source: "unknown", toolCallId };
}
/** Whether evidence settles a call's outcome. A later frame without evidence
 * of its own, such as the execution fact, which carries no result body,
 * never replaces settled evidence. */
export function settledToolCompletion(evidence: ToolCompletionEvidence | undefined): boolean {
  return evidence !== undefined && evidence.outcome !== "unknown" && evidence.outcome !== "running";
}
export function toolCompletionFromFrame(frame: { event: string; sourceKind?: string; data?: unknown }, toolCallId: string): ToolCompletionEvidence {
  const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  const id = typeof data?.tool_call_id === "string" ? data.tool_call_id : typeof data?.id === "string" ? data.id : "";
  if (!id || id !== toolCallId) return unknownToolCompletion(toolCallId);
  const source = frame.sourceKind === "session_history" ? "session-history" : "runtime-result";
  const status = data?.status;
  if (status === "cancelled" || status === "canceled") return { outcome: "cancelled", source, toolCallId };
  if (status === "interrupted") return { outcome: "interrupted", source, toolCallId };
  if (frame.event === "tool_execution_timed_out") return { outcome: "error", source, toolCallId };
  if (frame.event !== "tool_execution_completed" && frame.event !== "tool_result_received") return unknownToolCompletion(toolCallId);
  const result = data?.result ?? data?.content;
  if (typeof data?.is_error !== "boolean" || result === undefined || result === null) return unknownToolCompletion(toolCallId);
  return { outcome: data.is_error ? "error" : "success", source, toolCallId };
}
