import { guideStartupPresentation } from "./guide-startup.mjs";
import { guideWaitBinding } from "./guide-wait.mjs";

// Presentation for the initial empty proposal pane. Recovery stays in Conversation.
export function firstProposalStatus(state, timeline) {
  if (!state?.id || state.proposal || state.blueprint) return null;
  const status = state.operation?.status;
  if (status === "failed" || status === "interrupted") {
    return {
      title: status === "failed"
        ? "Your guide could not finish this response."
        : "Your guide response was interrupted.",
      detail: "Open Conversation to review your saved answer and recovery options.",
      tone: "attention",
      showConversation: true,
    };
  }
  if (!guideWaitBinding(state)) return null;

  const recovery = guideStartupPresentation(state, timeline);
  if (recovery) {
    const title = {
      failed: "Setup team could not start.",
      retrying: "Retrying setup team startup.",
      starting: "Setup team is starting.",
      unavailable: "Setup team is unavailable.",
      reconnecting: "Live updates interrupted. Reconnecting...",
    }[recovery];
    return {
      title,
      detail: recovery === "starting"
        ? "Waiting for live updates. Open Conversation to follow startup."
        : "Your conversation is retained. Open Conversation to check progress and recovery options.",
      tone: recovery === "failed" || recovery === "unavailable" ? "attention" : "notice",
      showConversation: true,
    };
  }
  return {
    title: "Your guide is working on your idea.",
    detail: "Your first proposal will appear here. You can keep writing.",
    tone: "working",
    showConversation: false,
  };
}
