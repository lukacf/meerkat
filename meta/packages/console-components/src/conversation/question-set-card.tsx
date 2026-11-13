import * as React from "react";
import type {
  ClosedQuestionSetOutcome,
  QuestionSet,
  QuestionSetAnswers,
  QuestionSetDecisionState,
  QuestionSetSnapshot,
} from "../../../console-core/src/pending-questions";

export interface QuestionSetCardProps {
  set: QuestionSet;
  resourceStatus: QuestionSetSnapshot["status"];
  decision?: QuestionSetDecisionState;
  closed?: ClosedQuestionSetOutcome;
  readOnly?: boolean;
  readOnlyReason?: QuestionSetSnapshot["readOnlyReason"];
  onAnswer(pendingId: string, answers: QuestionSetAnswers): void | Promise<void>;
  onDismiss(pendingId: string): void | Promise<void>;
}
export interface QuestionSetsView {
  snapshot: QuestionSetSnapshot;
  onAnswer(pendingId: string, answers: QuestionSetAnswers): void | Promise<void>;
  onDismiss(pendingId: string): void | Promise<void>;
}

const QuestionSetsContext = React.createContext<QuestionSetsView | undefined>(undefined);
export const QuestionSetsProvider = QuestionSetsContext.Provider;

export function QuestionSetCard(_props: QuestionSetCardProps): React.JSX.Element | null {
  return null;
}
export function QuestionSetList(_props: { view?: QuestionSetsView; identity?: string; readOnly?: boolean }): React.JSX.Element | null {
  return null;
}
export function ConversationQuestionSets(_props: { identity?: string }): React.JSX.Element | null {
  return null;
}
