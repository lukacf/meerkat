"""Pure formatting; the application supplies its already-fetched provider data."""
from meerkat_toolkit.connectors.gmail_message import (
    compose_message,
    decode_message,
    reply_linkage,
    validate_reply_selection,
)


def read_full_message(message):
    decoded = decode_message(message)
    if not decoded.complete:
        # The host chooses whether to retrieve referenced data or show a warning.
        return decoded, None
    return decoded, "\n\n".join(decoded.plain_text)


def compose_selected_reply(parent, *, parent_id, thread_id, to, subject, text,
                           cc="", bcc=""):
    # Applications should perform this check before their selected-account read.
    validate_reply_selection(parent_id, thread_id, subject)
    linkage = reply_linkage(parent, parent_id=parent_id, thread_id=thread_id,
                            subject=subject)
    return compose_message(to=to, cc=cc, bcc=bcc, subject=subject,
                           body_text=text, reply=linkage)
