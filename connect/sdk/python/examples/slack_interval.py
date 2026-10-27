"""Explicit host-selected creation-time retrieval. Importing performs no I/O."""

from meerkat_toolkit.connectors.slack import SlackIntervalResult, SlackOperations


async def read_selected_interval(
    selected_slack: SlackOperations,
    *,
    selected_channel: str,
    start: str,
    end: str,
    permitted_root_oldest: str,
    max_requests: int,
    max_messages: int,
) -> SlackIntervalResult:
    # The caller's existing authority owner permits the selected channel AND
    # the wider root-discovery range. This function obtains no credential/grant.
    result = await selected_slack.read_interval(
        selected_channel, start=start, end=end, root_oldest=permitted_root_oldest,
        max_requests=max_requests, max_messages=max_messages,
    )
    # Consumers must carry gaps, pending/stopped chains and limits into their
    # interpretation. Even finished with a zero floor is not a snapshot proof.
    return result
