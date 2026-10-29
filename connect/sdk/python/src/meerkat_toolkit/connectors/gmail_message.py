"""Pure Gmail message formatting, derived from the author's own host helpers.

SPDX-License-Identifier: MIT OR Apache-2.0
See LICENSE-EXTRACTED and GMAIL.md. These records confer no authority.
"""

from __future__ import annotations

import base64
import binascii
from dataclasses import dataclass
from email import policy
from email.header import decode_header
from email.message import EmailMessage, Message
import re
from typing import Any, Callable, TypeVar


_CODES = {"invalid_message", "unsupported_content", "limit_exceeded",
          "invalid_reply", "invalid_composition"}


class GmailMessageFormatError(ValueError):
    """Content-free refusal; no parser/provider exception is retained."""

    def __init__(self, code: str) -> None:
        self.code = code if type(code) is str and code in _CODES else "invalid_message"
        super().__init__("Gmail message format refused: " + self.code)


class _Fault(Exception):
    pass


def _fail(code: str = "invalid_message") -> None:
    raise _Fault(code)


_T = TypeVar("_T")


def _public(operation: Callable[[], _T], fallback: str) -> _T:
    # Raise outside the except suite so raw cause AND context are absent.
    code = fallback
    try:
        return operation()
    except _Fault as error:
        code = error.args[0]
    except (ValueError, TypeError, UnicodeError, LookupError, RecursionError, binascii.Error):
        pass
    raise GmailMessageFormatError(code)


@dataclass(frozen=True)
class GmailDecodeLimits:
    max_depth: int = 32
    max_parts: int = 1000
    max_headers: int = 2000
    max_header_bytes: int = 262144
    max_decoded_bytes: int = 2097152


@dataclass(frozen=True)
class GmailHeader:
    name: str
    value: str


@dataclass(frozen=True)
class GmailDecodedPart:
    path: tuple[int, ...]
    part_id: str | None
    mime_type: str
    filename: str | None
    headers: tuple[GmailHeader, ...]
    attachment_id: str | None
    size: int | None
    text: str | None
    inline_body: bool
    is_attachment: bool
    unavailable_reason: str | None


@dataclass(frozen=True)
class GmailDecodedMessage:
    message_id: str | None
    thread_id: str | None
    label_ids: tuple[str, ...] | None
    snippet: str | None
    internal_date: str | None
    headers: tuple[GmailHeader, ...]
    parts: tuple[GmailDecodedPart, ...]
    complete: bool

    @property
    def plain_text(self) -> tuple[str, ...]:
        return tuple(p.text for p in self.parts if p.inline_body
                     and p.mime_type.lower() == "text/plain" and p.text is not None)

    @property
    def html_text(self) -> tuple[str, ...]:
        return tuple(p.text for p in self.parts if p.inline_body
                     and p.mime_type.lower() == "text/html" and p.text is not None)

    @property
    def attachments(self) -> tuple[GmailDecodedPart, ...]:
        return tuple(p for p in self.parts if p.is_attachment)


@dataclass(frozen=True)
class GmailReplyLinkage:
    parent_message_id: str
    references: tuple[str, ...]


_CHARSETS = {
    "us-ascii": "ascii", "ascii": "ascii", "utf-8": "utf-8", "utf8": "utf-8",
    "iso-8859-1": "iso-8859-1", "latin1": "iso-8859-1", "latin-1": "iso-8859-1",
    "windows-1252": "cp1252", "cp1252": "cp1252",
}
_MIME = re.compile(r"[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+\Z")
_HEADER = re.compile(r"[!-9;-~]+\Z")
_B64 = re.compile(r"[A-Za-z0-9_-]*={0,2}\Z")


def _object(value: Any) -> dict[str, Any]:
    if type(value) is not dict:
        _fail()
    return value


def _string(value: Any) -> str:
    if type(value) is not str:
        _fail()
    value.encode("utf-8", errors="strict")
    return value


def _optional(obj: dict[str, Any], name: str) -> str | None:
    return None if name not in obj else _string(obj[name])


def _charset(value: Any) -> str:
    if type(value) is not str or value.lower() not in _CHARSETS:
        _fail("unsupported_content")
    return _CHARSETS[value.lower()]


def _content_profile(headers: tuple[GmailHeader, ...], mime: str) -> tuple[str | None, str | None]:
    selected: dict[str, str] = {}
    for header in headers:
        name = header.name.lower()
        if name in {"content-type", "content-disposition"}:
            if name in selected:
                _fail()
            selected[name] = header.value
    charset = None
    disposition = None
    for name, value in selected.items():
        parsed = policy.default.header_factory(name, value)
        if parsed.defects:
            _fail()
        legacy = Message(policy=policy.compat32)
        legacy[name] = value
        if name == "content-type":
            if legacy.get_content_type() != mime.lower():
                _fail()
            declarations = [v for k, v in (legacy.get_params(header=name) or [])[1:]
                            if k.lower() == "charset"]
            if len(declarations) > 1:
                _fail()
            if declarations:
                if type(declarations[0]) is not str:
                    _fail("unsupported_content")
                charset = declarations[0]
        else:
            disposition = legacy.get_content_disposition()
    return charset, disposition


def _decode_bytes(value: Any, remaining: int) -> bytes:
    data = _string(value)
    if len(data) > ((remaining + 2) // 3) * 4 + 4:
        _fail("limit_exceeded")
    if not _B64.fullmatch(data) or len(data.rstrip("=")) % 4 == 1:
        _fail()
    if "=" in data and len(data) % 4:
        _fail()
    raw = base64.b64decode(data + "=" * (-len(data) % 4), altchars=b"-_", validate=True)
    if base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=") != data.rstrip("="):
        _fail()
    if len(raw) > remaining:
        _fail("limit_exceeded")
    return raw


def decode_message(
    message: dict[str, Any], *, default_charset: str = "us-ascii",
    limits: GmailDecodeLimits = GmailDecodeLimits(),
) -> GmailDecodedMessage:
    """Decode a full Gmail payload, with no HTML rendering or attachment reads."""
    return _public(lambda: _decode_message(message, default_charset, limits), "invalid_message")


def _decode_message(message: Any, default: str, limits: GmailDecodeLimits) -> GmailDecodedMessage:
    obj = _object(message)
    default_encoding = _charset(default)
    if not isinstance(limits, GmailDecodeLimits) or any(
        type(v) is not int or v <= 0 for v in vars(limits).values()
    ):
        _fail()
    labels = obj.get("labelIds")
    if labels is not None and type(labels) is not list:
        _fail()
    label_ids = None if labels is None else tuple(_string(v) for v in labels)
    metadata = tuple(_optional(obj, k) for k in ("id", "threadId", "snippet", "internalDate"))
    stack = [(obj.get("payload"), (), False, frozenset())]
    parts: list[GmailDecodedPart] = []
    header_count = header_bytes = decoded_bytes = 0
    while stack:
        node, path, attached_parent, ancestors = stack.pop()
        node = _object(node)
        if id(node) in ancestors:
            _fail()
        if len(path) > limits.max_depth or len(parts) >= limits.max_parts:
            _fail("limit_exceeded")
        mime = _string(node.get("mimeType"))
        if not _MIME.fullmatch(mime):
            _fail()
        filename = _optional(node, "filename")
        part_id = _optional(node, "partId")
        raw_headers = node.get("headers", [])
        if type(raw_headers) is not list:
            _fail()
        header_count += len(raw_headers)
        if header_count > limits.max_headers:
            _fail("limit_exceeded")
        headers = []
        for value in raw_headers:
            value = _object(value)
            name, text = _string(value.get("name")), _string(value.get("value"))
            if not _HEADER.fullmatch(name):
                _fail()
            header_bytes += len(name.encode("utf-8")) + len(text.encode("utf-8"))
            if header_bytes > limits.max_header_bytes:
                _fail("limit_exceeded")
            headers.append(GmailHeader(name, text))
        frozen_headers = tuple(headers)
        charset, disposition = _content_profile(frozen_headers, mime)
        is_attachment = bool(filename) or disposition == "attachment" or mime.lower() == "message/rfc822"
        attached = attached_parent or is_attachment
        body = _object(node.get("body", {}))
        attachment_id = _optional(body, "attachmentId")
        if attachment_id == "":
            _fail()
        size = body.get("size")
        if "size" in body and (type(size) is not int or size < 0):
            _fail()
        raw = None
        if "data" in body:
            raw = _decode_bytes(body["data"], limits.max_decoded_bytes - decoded_bytes)
            decoded_bytes += len(raw)
            if attachment_id is None and size is not None and size != len(raw):
                _fail()
            if raw and attachment_id is not None:
                _fail()
        children = node.get("parts", [])
        if type(children) is not list:
            _fail()
        container = mime.lower().startswith("multipart/")
        if container and (raw or attachment_id is not None):
            _fail()
        if children and not container and mime.lower() != "message/rfc822":
            _fail()
        inline = not attached and mime.lower() in {"text/plain", "text/html"}
        text = None
        unavailable = None
        if inline:
            encoding = _charset(charset) if charset is not None else default_encoding
            if attachment_id is not None:
                unavailable = "external_body"
            elif raw is None and size != 0:
                unavailable = "missing_body_data"
            else:
                text = (raw or b"").decode(encoding, errors="strict")
        elif not attached and not container:
            unavailable = "unsupported_body"
        parts.append(GmailDecodedPart(path, part_id, mime, filename, frozen_headers,
                                      attachment_id, size, text, inline, is_attachment, unavailable))
        active = ancestors | {id(node)}
        for index in range(len(children) - 1, -1, -1):
            stack.append((children[index], path + (index,), attached, active))
    return GmailDecodedMessage(metadata[0], metadata[1], label_ids, metadata[2], metadata[3],
                               parts[0].headers, tuple(parts),
                               not any(p.unavailable_reason for p in parts))


_SELECTOR = re.compile(r"[A-Za-z0-9_-]{1,1024}\Z")
_ATOM = r"[A-Za-z0-9!#$%&'*+/=?^_\x60{|}~-]+"
_ID = re.compile(rf"<{_ATOM}(?:\.{_ATOM})*@{_ATOM}(?:\.{_ATOM})*>\Z")


def _subject(value: Any) -> None:
    if type(value) is not str or any(ord(c) < 32 or ord(c) == 127 for c in value):
        _fail("invalid_reply")
    if len(value.encode("utf-8")) > 998:
        _fail("invalid_reply")


def _selection(parent_id: Any, thread_id: Any, subject: Any) -> None:
    if any(type(v) is not str or not _SELECTOR.fullmatch(v) for v in (parent_id, thread_id)):
        _fail("invalid_reply")
    _subject(subject)


def validate_reply_selection(parent_id: str, thread_id: str, subject: str) -> None:
    """Validate the explicit selection before the host fetches its parent."""
    return _public(lambda: _selection(parent_id, thread_id, subject), "invalid_reply")


def _ids(value: Any) -> tuple[str, ...]:
    if (type(value) is not str or len(value) > 8192
            or any(ord(c) > 126 or (ord(c) < 32 and c != "\t") for c in value)):
        _fail("invalid_reply")
    ids = tuple(value.split())
    if not ids or len(ids) > 100 or any(len(v) > 254 or not _ID.fullmatch(v) for v in ids):
        _fail("invalid_reply")
    return ids


def _decoded_subject(value: str) -> str:
    _subject(value)
    parsed = policy.default.header_factory("Subject", value)
    if parsed.defects:
        _fail("invalid_reply")
    pieces = []
    for text, charset in decode_header(value):
        if isinstance(text, bytes):
            encoding = (charset or "ascii").lower()
            if encoding not in {"ascii", "us-ascii", "utf-8"}:
                _fail("invalid_reply")
            text = text.decode(encoding, errors="strict")
        pieces.append(text)
    decoded = "".join(pieces)
    if "=?" in decoded or str(parsed) != decoded:
        _fail("invalid_reply")
    _subject(decoded)
    return decoded


def reply_linkage(parent: dict[str, Any], *, parent_id: str, thread_id: str,
                  subject: str) -> GmailReplyLinkage:
    """Check the fetched parent's format and exact host-selected identifiers."""
    return _public(lambda: _reply_linkage(parent, parent_id, thread_id, subject), "invalid_reply")


def _reply_linkage(parent: Any, parent_id: str, thread_id: str, subject: str) -> GmailReplyLinkage:
    _selection(parent_id, thread_id, subject)
    if type(parent) is not dict or parent.get("id") != parent_id or parent.get("threadId") != thread_id:
        _fail("invalid_reply")
    payload = parent.get("payload")
    headers = payload.get("headers") if type(payload) is dict else None
    if type(headers) is not list or len(headers) > 100:
        _fail("invalid_reply")
    selected: dict[str, str] = {}
    for header in headers:
        if type(header) is not dict or type(header.get("name")) is not str:
            _fail("invalid_reply")
        name = header["name"].lower()
        if name not in {"message-id", "references", "in-reply-to", "subject"}:
            continue
        if name in selected or type(header.get("value")) is not str:
            _fail("invalid_reply")
        selected[name] = header["value"]
    if "message-id" not in selected or "subject" not in selected:
        _fail("invalid_reply")
    if _decoded_subject(selected["subject"]) != subject:
        _fail("invalid_reply")
    parent_ids = _ids(selected["message-id"])
    if len(parent_ids) != 1:
        _fail("invalid_reply")
    references = _ids(selected["references"]) if "references" in selected else ()
    fallback = _ids(selected["in-reply-to"]) if "in-reply-to" in selected else ()
    if not references and len(fallback) > 1:
        _fail("invalid_reply")
    chain = (references or fallback) + parent_ids
    if len(chain) > 100 or len(" ".join(chain)) > 8192:
        _fail("invalid_reply")
    return GmailReplyLinkage(parent_ids[0], chain)


def compose_message(*, to: str, subject: str, body_text: str, cc: str = "", bcc: str = "",
                    reply: GmailReplyLinkage | None = None) -> bytes:
    """Compose the host's exact choices; this neither approves nor sends bytes."""
    return _public(lambda: _compose(to, subject, body_text, cc, bcc, reply), "invalid_composition")


def _compose(to: str, subject: str, body_text: str, cc: str, bcc: str,
             reply: GmailReplyLinkage | None) -> bytes:
    for value in (to, subject, cc, bcc):
        if type(value) is not str or any(ord(c) < 32 or ord(c) == 127 for c in value):
            _fail("invalid_composition")
        value.encode("utf-8")
    if type(body_text) is not str:
        _fail("invalid_composition")
    body_text.encode("utf-8")
    message = EmailMessage()
    message["To"] = to
    message["Subject"] = subject
    if cc:
        message["Cc"] = cc
    if bcc:
        message["Bcc"] = bcc
    message.set_content(body_text)
    if reply is None:
        return message.as_bytes()
    if (not isinstance(reply, GmailReplyLinkage) or type(reply.references) is not tuple
            or len(_ids(reply.parent_message_id)) != 1 or not reply.references
            or reply.references[-1] != reply.parent_message_id):
        _fail("invalid_reply")
    references = _ids(" ".join(reply.references))
    if references != reply.references:
        _fail("invalid_reply")
    lines = [f"In-Reply-To: {reply.parent_message_id}", "References:"]
    for identifier in references:
        if len(lines[-1]) + len(identifier) + 1 > 78:
            lines.append(" " + identifier)
        else:
            lines[-1] += " " + identifier
    return ("\r\n".join(lines) + "\r\n").encode("ascii") + message.as_bytes(policy=policy.SMTP)
