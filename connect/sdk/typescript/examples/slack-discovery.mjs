// The host supplies its selected SlackOperations and independently selected IDs.
// Comparison detects a mismatch; it does not create a native binding, grant,
// audience policy or durable association. Importing this file makes no request.
export async function observeSelectedIdentity(slack, { teamId, userId }, options = {}) {
  const observation = await slack.authTest(options);
  if (observation.team_id !== teamId || observation.user_id !== userId) {
    throw new Error("selected Slack identity does not match provider observation");
  }
  return observation;
}

// Inside an existing authenticated host after its own permission checks:
// await observeSelectedIdentity(slack, { teamId: selectedTeamId, userId: selectedUserId }, { signal });
// const page = await slack.listConversationsPage({ types: ["public_channel", "private_channel"], signal });
// The caller inspects this provider page and explicitly chooses further pages:
// const next = await slack.listConversationsPage({
//   types: ["public_channel", "private_channel"], cursor: page.response_metadata.next_cursor, signal,
// }); // Only when the returned cursor is a nonempty string and the caller chooses continuation.
// const detail = await slack.getConversation(chosenPermittedConversationId, { signal });
// const messages = await slack.historyPage(chosenPermittedConversationId, { oldest, latest, signal });
// const replies = await slack.repliesPage(chosenPermittedConversationId, chosenRootTimestamp, { oldest, latest, signal });
// No short/empty page proves completeness. Access, retention, concurrent edits,
// old thread roots and unvisited cursors remain explicit coverage limitations.
