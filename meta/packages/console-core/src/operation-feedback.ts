import type { ConversationMessageEntry } from "./conversation";

type Feedback = NonNullable<ConversationMessageEntry["operationFeedback"]>;
type Frame = { event: string; data?: unknown };

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown> : null;
}

// The host refuses an operation (`operation_refused`) for a policy denial, for
// an authorization that changed after the operation was prepared and for
// request facts it could not evaluate, and sends every one of them alike: its
// runtime notice and tool error name the refusal and never its reason. The
// card states only what holds for all of them, so it claims no permission
// cause, and a reason field no host contract defines changes nothing.
const refused = (): Feedback => ({
  kind: "operation-refused",
  title: "Action refused",
  detail: "This action was refused under the current authorization, so it did not run. The agent can continue with other work.",
});

const confinementDetails = {
  invalid_requirement: "The confinement requirement is invalid.",
  invalid_launch: "The confined process launch is invalid.",
  unsupported_requirement: "This backend does not support the required confinement.",
  backend_unavailable: "The required confinement backend is unavailable.",
  preparation_failed: "Confined process preparation failed.",
} as const satisfies Record<NonNullable<Feedback["confinementRefusal"]>, string>;

const hookReasons = {
  policy_violation: "Policy violation",
  safety_violation: "Safety violation",
  schema_violation: "Schema violation",
  timeout: "Timeout",
  runtime_error: "Runtime error",
} as const satisfies Record<NonNullable<Feedback["hookReasonCode"]>, string>;

const reviewUnsatisfiedDetails = {
  denied: "The reviewer declined this action. Other permitted work can continue.",
  escalated: "This action requires further review. Other permitted work can continue.",
  context_changed: "The review no longer matches this action's current context. Other permitted work can continue.",
  human_consent_required: "This action requires human consent. Other permitted work can continue.",
  already_consumed: "This review was already used. This notice does not establish the action's execution result; do not repeat it based on this notice.",
} as const satisfies Record<NonNullable<Feedback["reviewUnsatisfied"]>, string>;

const reviewUnavailableDetails = {
  deadline_expired: "The review deadline elapsed. Other permitted work can continue.",
  reviewer_failed: "The reviewer could not provide a usable decision. Other permitted work can continue.",
  reviewer_missing: "No reviewer is configured for this action. Other permitted work can continue.",
  unsupported_entry: "This action cannot use the available review entry path. Other permitted work can continue.",
  owner_unavailable: "The review owner is unavailable. Other permitted work can continue.",
  dispatch_ended: "This review's dispatch has ended. This notice does not establish the action's execution result; do not repeat it based on this notice.",
} as const satisfies Record<NonNullable<Feedback["reviewUnavailable"]>, string>;

function reviewFeedback(error: unknown, reason: unknown, toolCallId?: string): Feedback | null {
  if (typeof reason !== "string") return null;
  if (error === "review_unsatisfied" && Object.hasOwn(reviewUnsatisfiedDetails, reason)) {
    const reviewUnsatisfied = reason as keyof typeof reviewUnsatisfiedDetails;
    return { kind: "review-unsatisfied", title: "Review not satisfied",
      detail: reviewUnsatisfiedDetails[reviewUnsatisfied], reviewUnsatisfied,
      ...(toolCallId === undefined ? {} : { toolCallId }),
    };
  }
  if (error === "review_unavailable" && Object.hasOwn(reviewUnavailableDetails, reason)) {
    const reviewUnavailable = reason as keyof typeof reviewUnavailableDetails;
    return { kind: "review-unavailable", title: "Review unavailable",
      detail: reviewUnavailableDetails[reviewUnavailable], reviewUnavailable,
      ...(toolCallId === undefined ? {} : { toolCallId }),
    };
  }
  return null;
}

function identifier(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function confinementFeedback(value: unknown, toolCallId: string): Feedback | null {
  if (typeof value !== "string" || !Object.hasOwn(confinementDetails, value)) return null;
  const refusal = value as keyof typeof confinementDetails;
  return {
    kind: "confinement-refused",
    title: "Action could not start",
    detail: `${confinementDetails[refusal]} This action did not run. The agent can continue with other work.`,
    toolCallId,
    confinementRefusal: refusal,
  };
}

/** Display projection only. No decision is inferred from arbitrary tool prose. */
export function operationFeedbackFromFrame(frame: Frame): Feedback | null {
  const data = record(frame.data);
  if (!data) return null;
  if (frame.event === "hook_launch_refused" && data.point === "pre_tool_execution") {
    const toolCallId = identifier(data.tool_use_id);
    const hookId = identifier(data.hook_id);
    const reason = record(data.reason);
    if (!toolCallId || !hookId || !reason) return null;
    if (reason.reason_code === "confinement_refused") {
      return confinementFeedback(reason.refusal, toolCallId);
    }
    if (reason.reason_code === "execution_failed" && typeof reason.message === "string") {
      return {
        kind: "hook-launch-failed",
        title: "Hook could not start",
        detail: "The hook could not start, so this action did not run. The agent can continue with other work.",
        toolCallId,
        hookId,
      };
    }
    return null;
  }
  if (frame.event === "operation_observation_failed" && data.phase === "outcome"
      && typeof data.operation_id === "string" && data.operation_id.length > 0
      && data.operation_id.length <= 256) {
    return {
      kind: "audit-unavailable",
      title: "Audit update unavailable",
      detail: "The action's outcome could not be recorded. Its actual result is unchanged; do not repeat it based on this notice.",
      operationId: data.operation_id,
    };
  }
  if (frame.event === "system_notice") {
    const message = record(data.message) ?? data;
    if (Array.isArray(message.blocks) && message.blocks.some((value) => {
      const block = record(value);
      return block?.type === "runtime_notice" && block.category === "operation_refused"
        && record(block.payload)?.code === "operation_refused";
    })) return refused();
    if (Array.isArray(message.blocks)) {
      for (const value of message.blocks) {
        const block = record(value);
        if (block?.type !== "runtime_notice" || block.category !== "operation_review_required") continue;
        const payload = record(block.payload);
        const review = record(payload?.review);
        const error = payload?.code === "REVIEW_UNSATISFIED" && review?.review === "unsatisfied"
          ? "review_unsatisfied"
          : payload?.code === "REVIEW_UNAVAILABLE" && review?.review === "unavailable" ? "review_unavailable" : null;
        const feedback = reviewFeedback(error, review?.kind);
        if (feedback) return feedback;
      }
    }
  }
  if ((frame.event === "tool_result_received" || frame.event === "tool_execution_completed")
      && data.is_error === true && Array.isArray(data.content) && data.content.length === 1) {
    const block = record(data.content[0]);
    if (block?.type !== "text" || typeof block.text !== "string") return null;
    try {
      const payload = record(JSON.parse(block.text));
      if (payload?.error === "operation_refused") return refused();
      const toolCallId = identifier(data.tool_call_id ?? data.id);
      if (!toolCallId) return null;
      const detail = record(payload?.data);
      if (payload?.error === "review_unsatisfied" || payload?.error === "review_unavailable") {
        if (typeof payload.message !== "string") return null;
        const feedback = reviewFeedback(payload.error, detail?.kind, toolCallId);
        const approvalId = detail?.approval_id;
        // Native ApprovalId serializes as a UUID. Preserve its exact value as display data only.
        if (feedback?.kind === "review-unsatisfied" && typeof approvalId === "string" && approvalId.length === 36
            && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(approvalId)) {
          return { ...feedback, approvalId };
        }
        return feedback;
      }
      if (payload?.error === "confinement_refused") {
        return confinementFeedback(detail?.refusal, toolCallId);
      }
      if (payload?.error === "hook_denied" && detail?.point === "pre_tool_execution") {
        const hookId = identifier(detail.hook_id);
        const reason = detail.reason_code;
        if (!hookId || typeof reason !== "string" || !Object.hasOwn(hookReasons, reason)) return null;
        const hookReasonCode = reason as keyof typeof hookReasons;
        return {
          kind: "hook-denied",
          title: "Action blocked by hook",
          detail: `A hook denied this action (${hookReasons[hookReasonCode]}). The agent can continue with other work.`,
          toolCallId,
          hookId,
          hookReasonCode,
        };
      }
    } catch { /* Non-JSON tool output stays ordinary output. */ }
  }
  return null;
}
