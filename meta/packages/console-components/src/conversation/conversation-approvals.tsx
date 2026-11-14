import { approvalMatchesConversation, type ApprovalAction, type PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";
import { hostDecideLinks } from "../../../console-core/src/host-attention";
import { ApprovalCard, useHostDecideSources } from "./approval-card";
import { ConversationQuestionSets } from "./question-set-card";

export type ConversationApprovalProps = {
  approvalSnapshot?: PendingApprovalSnapshot;
  /** Canonical host-authorized target, never a label or actor ID. */
  approvalIdentity?: string;
  onApprovalDecision?: (pendingId: string, action: ApprovalAction) => void | Promise<void>;
};

/** A continued interaction belongs beside its latest matching visible turn. */
export function approvalInteractionIdsByTurn(turns: readonly (readonly string[])[]): string[][] {
  const latest = new Map<string, number>();
  turns.forEach((ids, index) => ids.forEach((id) => latest.set(id, index)));
  return turns.map((ids, index) => [...new Set(ids)].filter((id) => latest.get(id) === index));
}

/** Indexes of the turns that hold a pending request, placed as
 * `ConversationApprovals` places them with `approvalInteractionIdsByTurn`. */
export function pendingApprovalTurns(
  turns: readonly (readonly string[])[],
  { approvalSnapshot, approvalIdentity, conversationId }: Pick<ConversationApprovalProps, "approvalSnapshot" | "approvalIdentity"> & { conversationId?: string },
): number[] {
  if (!approvalSnapshot || !approvalIdentity || approvalSnapshot.requests.length === 0) return [];
  return approvalInteractionIdsByTurn(turns).flatMap((interactionIds, index) => (
    interactionIds.length > 0 && approvalSnapshot.requests.some((request) => Boolean(request.origin?.interactionId)
      && approvalMatchesConversation(request, { identity: approvalIdentity, conversationId, interactionIds }))
      ? [index] : []));
}

export function ConversationApprovals({ approvalSnapshot, approvalIdentity, onApprovalDecision, conversationId, interactionIds }: ConversationApprovalProps & {
  conversationId?: string;
  /** Omit for conversation-only provenance, supply exact IDs for turn placement. */
  interactionIds?: readonly string[];
}) {
  const decideSources = useHostDecideSources();
  // The question sets the conversation's agent asked sit with the
  // conversation's own approvals (the placement without interaction IDs),
  // above them, once per conversation. They come from QuestionSetsProvider.
  const questionSets = interactionIds ? null : <ConversationQuestionSets identity={approvalIdentity} />;
  if (!approvalSnapshot || !approvalIdentity) return questionSets;
  const requests = approvalSnapshot.requests.filter((request) =>
    Boolean(request.origin?.interactionId) === Boolean(interactionIds)
    && approvalMatchesConversation(request, { identity: approvalIdentity, conversationId, interactionIds }));
  const readOnly = approvalSnapshot.readOnly || !onApprovalDecision;
  const readOnlyReason = approvalSnapshot.readOnly ? approvalSnapshot.readOnlyReason : undefined;
  return <>{questionSets}{requests.map((request) => <ApprovalCard key={request.pendingId} request={request} resourceStatus={approvalSnapshot.status} decision={approvalSnapshot.decisions[request.pendingId]} readOnly={readOnly}
    readOnlyReason={readOnlyReason}
    decideLinks={readOnly ? hostDecideLinks(decideSources, request.pendingId) : undefined} onDecide={onApprovalDecision ?? (() => {})} />)}</>;
}
