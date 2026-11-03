# SPDX-License-Identifier: MIT OR Apache-2.0
"""Compose bounded, deterministic plain-text Gmail messages with the stdlib."""

from __future__ import annotations

from email import policy
from email.errors import NonASCIILocalPartDefect
from email.header import Header
from email.headerregistry import Address
from email.message import EmailMessage
from email.parser import Parser


class RecipientPermissionError(ValueError):
    """A fixed recipient ceiling refused a locally composed message."""

    def __init__(self) -> None:
        super().__init__("Recipient is outside the configured limit")


def _invalid_header(value: str) -> bool:
    return any((ord(char) < 32 and char != "\t") or ord(char) == 127 for char in value)


def _mailbox(value: str) -> Address:
    if type(value) is not str or _invalid_header(value) or value.rstrip().endswith(","):
        raise ValueError("Invalid mailbox")
    parsed = policy.SMTPUTF8.header_factory("To", value)
    # SMTPUTF8 permits international local parts; every other defect fails.
    if (any(not isinstance(defect, NonASCIILocalPartDefect) for defect in parsed.defects)
            or len(parsed.addresses) != 1 or len(parsed.groups) != 1
            or parsed.groups[0].display_name is not None
            or not parsed.addresses[0].domain):
        raise ValueError("Invalid mailbox")
    address = parsed.addresses[0]
    if any(_invalid_header(part) for part in (address.display_name, address.username, address.domain)):
        raise ValueError("Invalid mailbox")
    return address


def _recipient_key(address: Address) -> tuple[str, str]:
    domain = address.domain.lower() if address.domain.isascii() else address.domain
    return address.username, domain


def normalize_recipient_addresses(values: list[str]) -> tuple[str, ...]:
    """Validate a fixed ceiling of bare mailboxes without inferring aliases."""
    if type(values) is not list or len(values) > 256:
        raise ValueError("Invalid recipient limit")
    normalized = []
    seen = set()
    for value in values:
        if (type(value) is not str or not 1 <= len(value) <= 2048
                or any(ord(char) < 32 or ord(char) == 127 for char in value)):
            raise ValueError("Invalid recipient limit")
        value.encode("utf-8")
        address = _mailbox(value)
        # No display names, comments, groups, brackets or surrounding space.
        if value != address.addr_spec:
            raise ValueError("Recipient limit requires bare mailboxes")
        key = _recipient_key(address)
        if key in seen:
            raise ValueError("Duplicate recipient limit")
        seen.add(key)
        normalized.append(Address(username=key[0], domain=key[1]).addr_spec)
    return tuple(normalized)


def compose_message(
    *,
    from_address: str,
    to: list[str],
    subject: str,
    body_text: str,
    max_message_bytes: int,
    cc: list[str] | None = None,
    bcc: list[str] | None = None,
    allowed_recipient_addresses: tuple[str, ...] | None = None,
) -> bytes:
    """Return SMTPUTF8 MIME, without inferring identity or authorizing a send.

    Each address input represents one mailbox, optionally with a display name.
    Text line endings become CRLF and the text part has a final newline. No
    clock, random identifier, external state, attachment or multipart boundary
    participates in composition.
    """
    if type(max_message_bytes) is not int or max_message_bytes < 1:
        raise ValueError("Invalid message limit")
    remaining = max_message_bytes

    def bounded_text(value: str, *, header: bool) -> str:
        nonlocal remaining
        if type(value) is not str or len(value) > remaining:
            raise ValueError("Invalid message input")
        remaining -= len(value.encode("utf-8"))
        if remaining < 0:
            raise ValueError("Message exceeds byte limit")
        if header and _invalid_header(value):
            raise ValueError("Invalid message header")
        return value

    def mailbox(value: str) -> Address:
        return _mailbox(bounded_text(value, header=True))

    def recipients(values: list[str] | None, *, required: bool = False) -> list[Address]:
        if values is None and not required:
            return []
        if type(values) is not list or len(values) > remaining or (required and not values):
            raise ValueError("Invalid recipient list")
        return [mailbox(value) for value in values]

    sender = mailbox(from_address)
    to_addresses = recipients(to, required=True)
    cc_addresses = recipients(cc)
    bcc_addresses = recipients(bcc)
    if allowed_recipient_addresses is not None:
        if type(allowed_recipient_addresses) is not tuple:
            raise ValueError("Invalid recipient limit")
        allowed = {_recipient_key(_mailbox(value)) for value in
                   normalize_recipient_addresses(list(allowed_recipient_addresses))}
        if any(_recipient_key(address) not in allowed
               for address in (*to_addresses, *cc_addresses, *bcc_addresses)):
            raise RecipientPermissionError()
    subject = bounded_text(subject, header=True)
    body_text = bounded_text(body_text, header=False)

    message = EmailMessage(policy=policy.SMTPUTF8)
    message["From"] = sender
    message["To"] = to_addresses
    if cc_addresses:
        message["Cc"] = cc_addresses
    if bcc_addresses:
        # Gmail derives the envelope from MIME, including Bcc recipients.
        message["Bcc"] = bcc_addresses
    message.set_content(body_text, subtype="plain", charset="utf-8", cte="base64")
    # EmailMessage interprets assigned Subject strings as header syntax. Encode
    # the literal text with Header so even encoded-word-looking input remains
    # exactly the requested subject after the recipient decodes it once.
    encoded_subject = Header(subject, charset="utf-8", header_name="Subject").encode(linesep="\r\n")
    serialized = b"Subject: " + encoded_subject.encode("ascii") + b"\r\n" + message.as_bytes()
    if (len(serialized) > max_message_bytes
            or any(len(line) > 998 for line in serialized.split(b"\r\n"))):
        raise ValueError("Message exceeds byte limit")
    # The stdlib can interpret encoded words again while folding an address.
    # Bind the emitted headers back to the explicit parsed mailboxes so that
    # such reinterpretation cannot alter an address or introduce a header.
    observed = Parser(policy=policy.SMTPUTF8).parsestr(serialized.decode("utf-8"), headersonly=True)
    expected_addresses = {"From": [sender], "To": to_addresses}
    if cc_addresses:
        expected_addresses["Cc"] = cc_addresses
    if bcc_addresses:
        expected_addresses["Bcc"] = bcc_addresses
    expected_headers = ["Subject", *expected_addresses, "Content-Type", "Content-Transfer-Encoding", "MIME-Version"]
    if list(observed.keys()) != expected_headers or str(observed["Subject"]) != subject:
        raise ValueError("Invalid serialized headers")

    def parts(address: Address) -> tuple[str, str, str]:
        return address.display_name, address.username, address.domain

    for name, addresses in expected_addresses.items():
        if [parts(address) for address in observed[name].addresses] != [parts(address) for address in addresses]:
            raise ValueError("Invalid serialized mailbox")
    return serialized
