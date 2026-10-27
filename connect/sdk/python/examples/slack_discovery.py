"""Explicit host selection and caller-chosen pages. No credentials or auto-run.

The host supplies a prepared, authenticated SlackOperations and its independently
selected IDs. Comparison detects a mismatch; it does not create a native binding,
grant, audience policy, or durable identity association.
"""

from typing import Any

from meerkat_toolkit.connectors.slack import SlackOperations


async def observe_selected_identity(
    slack: SlackOperations, *, team_id: str, user_id: str
) -> dict[str, Any]:
    observation = await slack.auth_test()
    if observation["team_id"] != team_id or observation["user_id"] != user_id:
        raise ValueError("selected Slack identity does not match provider observation")
    return observation


# Usage inside an existing authenticated host, after its own permission checks:
# observed = await observe_selected_identity(slack, team_id=selected_team_id,
#                                            user_id=selected_user_id)
# page = await slack.list_conversations_page(types=("public_channel", "private_channel"))
# Present/inspect this provider page; the caller decides whether another is needed.
# next_page = await slack.list_conversations_page(
#     types=("public_channel", "private_channel"),
#     cursor=page["response_metadata"]["next_cursor"],  # only if nonempty
# )
# The caller chooses an exact permitted ID, not a fuzzy name match:
# detail = await slack.get_conversation(chosen_channel)
# messages = await slack.history_page(chosen_channel, oldest=chosen_start, latest=chosen_end)
# A separately chosen known thread is read explicitly:
# replies = await slack.replies_page(chosen_channel, chosen_root_ts,
#                                    oldest=chosen_start, latest=chosen_end)
# A short/empty page is not completeness; old roots, provider retention, access,
# concurrent edits and remaining cursors can limit the promised coverage.
