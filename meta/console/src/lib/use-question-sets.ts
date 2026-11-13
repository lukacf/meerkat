import React from "react";
import {
  createQuestionSetResource,
  type QuestionSetAnswerParams,
  type QuestionSetDismissParams,
  type QuestionSetResource,
  type QuestionSetSnapshot,
} from "../../../packages/console-core/src/pending-questions";
import type { QuestionSetsView } from "../../../packages/console-components/src/conversation/question-set-card";

/**
 * The agents' open question sets (`meta/gating/questions`) for one scope
 * (principal and runtime), and how this console answers them: one resource
 * feeds the inbox and every conversation. The gateway decides as the
 * signed-in person the connection names, so a caller who is no person reads
 * the sets and sends nothing.
 *
 * Undefined while disabled and until the scope's own resource has published:
 * a scope change hides the previous scope's sets before effects run.
 */
export function useQuestionSets(input: {
  scopeKey: string;
  /** The controller the reads go through; a new one starts a new resource. */
  owner: unknown;
  enabled: boolean;
  readOnly: boolean;
  /** Why `readOnly` holds, when the caller could answer by signing in. */
  readOnlyReason?: "sign_in";
  /** Reads the open sets (the `listQuestionSets` command's result). */
  load(signal: AbortSignal): Promise<unknown>;
  /** Sends one decision (the `decideGating` command's result). */
  decide(params: QuestionSetAnswerParams | QuestionSetDismissParams, signal: AbortSignal): Promise<unknown>;
}): QuestionSetsView | undefined {
  const { scopeKey, owner, enabled, readOnly, readOnlyReason } = input;
  const calls = React.useRef(input);
  calls.current = input;
  const resourceRef = React.useRef<QuestionSetResource | null>(null);
  const [published, setPublished] = React.useState<{ owner: unknown; snapshot: QuestionSetSnapshot }>();
  React.useEffect(() => {
    setPublished(undefined);
    if (!enabled) return;
    const resource = createQuestionSetResource({
      scopeKey,
      readOnly,
      ...(readOnly && readOnlyReason ? { readOnlyReason } : {}),
      load: (signal) => calls.current.load(signal),
      decide: (params, signal) => calls.current.decide(params, signal),
    });
    resourceRef.current = resource;
    const publish = () => setPublished({ owner, snapshot: resource.getSnapshot() });
    const unsubscribe = resource.subscribe(publish);
    publish();
    return () => {
      unsubscribe(); resource.dispose();
      if (resourceRef.current === resource) resourceRef.current = null;
    };
  }, [scopeKey, owner, enabled, readOnly, readOnlyReason]);
  const snapshot = published?.owner === owner && published.snapshot.scopeKey === scopeKey ? published.snapshot : undefined;
  return React.useMemo<QuestionSetsView | undefined>(() => (snapshot ? {
    snapshot,
    onAnswer: (pendingId, answers) => resourceRef.current?.answer(pendingId, answers),
    onDismiss: (pendingId) => resourceRef.current?.dismiss(pendingId),
  } : undefined), [snapshot]);
}
