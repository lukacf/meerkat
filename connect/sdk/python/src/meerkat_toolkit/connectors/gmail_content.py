"""Original public Gmail JSON content projection using stdlib MIME/base64 rules.

SPDX-License-Identifier: MIT OR Apache-2.0
No network, HTML rendering, URL fetching, credential access or private formatter.
Source contracts and limits are documented in sdk/services/README.md.
"""
from __future__ import annotations

import base64
from copy import deepcopy
from dataclasses import dataclass
from email.message import Message
import json
import re
from typing import Any


_CHARSETS = {"ascii": "ascii", "us-ascii": "ascii", "utf-8": "utf-8", "utf8": "utf-8",
             "iso-8859-1": "latin-1", "latin-1": "latin-1", "latin1": "latin-1",
             "windows-1252": "cp1252", "cp1252": "cp1252"}
_CODES = {"invalid_content", "content_limit", "part_not_found", "ambiguous_part",
          "unsupported_part", "unsupported_charset", "invalid_text", "missing_body"}
_ID = re.compile(r"[A-Za-z0-9_-]{1,1024}\Z")
_PART = re.compile(r"[A-Za-z0-9_.-]{0,1024}\Z")
_MIME = re.compile(r"[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+\Z")


class GmailContentError(ValueError):
    """Only a fixed content-free code is exposed to a caller."""
    def __init__(self, code: str):
        self.code = code if code in _CODES else "invalid_content"
        super().__init__("Gmail content refused: " + self.code)


def _require(condition: bool, code: str = "invalid_content") -> None:
    if not condition:
        raise GmailContentError(code)


def _safe(function, *args):
    code = "invalid_content"
    try:
        return function(*args)
    except GmailContentError as error:
        code = error.code
    except (ValueError, TypeError, KeyError, UnicodeError, RecursionError, OverflowError):
        pass
    # Do not retain provider content in a parser exception or its context.
    raise GmailContentError(code)


def _json_bound(value: Any, limit: int) -> None:
    _require(type(limit) is int and 1 <= limit <= 16_777_216, "content_limit")
    total = 0
    for chunk in json.JSONEncoder(ensure_ascii=True, allow_nan=False).iterencode(value):
        total += len(chunk)
        _require(total <= limit, "content_limit")


def _headers(value: Any) -> list[dict[str, str]]:
    _require(type(value) is list and len(value) <= 2000, "content_limit")
    result = []
    for item in value:
        _require(type(item) is dict and set(item) == {"name", "value"})
        _require(type(item['name']) is str and re.fullmatch(r"[!-9;-~]{1,998}", item['name']) is not None)
        _require(type(item['value']) is str)
        result.append(dict(item))
    return result


def _mime(headers: list[dict[str, str]], mime: str) -> tuple[str, str | None]:
    values = {}
    for header in headers:
        name = header['name'].lower()
        if name in {"content-type", "content-disposition"}:
            _require(name not in values)
            values[name] = header['value']
    message = Message()
    for name, value in values.items():
        message[name] = value
    if 'content-type' in values:
        _require(values['content-type'].split(';', 1)[0].strip().lower() == mime.lower())
        _require(message.get_content_type() == mime.lower())
    charset = 'us-ascii'
    params = message.get_params(header='content-type', failobj=[]) or []
    charsets = [value for name, value in params[1:] if name.lower() == 'charset']
    _require(len(charsets) <= 1)
    if charsets:
        _require(type(charsets[0]) is str and bool(charsets[0]))
        charset = charsets[0].lower()
    disposition = message.get_content_disposition()
    _require(disposition in {None, 'inline', 'attachment'})
    _require('content-disposition' not in values or disposition is not None)
    return charset, disposition


def _body(value: Any, limit: int) -> tuple[dict[str, Any], bytes | None]:
    _require(type(value) is dict)
    size = value.get('size')
    attachment = value.get('attachmentId')
    _require('size' not in value or (type(size) is int and size >= 0))
    _require('attachmentId' not in value or (type(attachment) is str and _ID.fullmatch(attachment) is not None))
    data = value.get('data')
    if 'data' not in value:
        return value, b'' if size == 0 and attachment is None else None
    _require(type(data) is str and re.fullmatch(r'[A-Za-z0-9_-]*={0,2}', data) is not None)
    _require(not attachment or not data)
    if attachment:
        return value, None
    unpadded = data.rstrip('=')
    expected = len(unpadded) * 3 // 4
    _require(expected <= limit, 'content_limit')
    decoded = base64.b64decode(unpadded + '=' * (-len(unpadded) % 4), altchars=b'-_', validate=True)
    _require(base64.urlsafe_b64encode(decoded).decode().rstrip('=') == unpadded)
    _require(not data.endswith('=') or data == base64.urlsafe_b64encode(decoded).decode())
    _require(size is None or size == len(decoded))
    return value, decoded


def _text(data: bytes, charset: str) -> tuple[str | None, str | None]:
    codec = _CHARSETS.get(charset)
    if codec is None:
        return None, 'unsupported_charset'
    try:
        return data.decode(codec, errors='strict'), None
    except UnicodeError:
        return None, 'invalid_text'


@dataclass(frozen=True)
class SelectedTextPart:
    """Fresh local selection, not a reusable account or access receipt."""
    message: dict[str, Any]
    part: dict[str, Any]
    body: dict[str, Any]


def _inspect(message: Any, limit: int):
    _json_bound(message, limit)
    _require(type(message) is dict and type(message.get('id')) is str and _ID.fullmatch(message['id']) is not None)
    if 'threadId' in message:
        _require(type(message['threadId']) is str and _ID.fullmatch(message['threadId']) is not None)
    payload = message.get('payload')
    _require(type(payload) is dict)
    metadata = deepcopy({key: value for key, value in message.items() if key not in {'payload', 'raw'}})
    outer_headers = _headers(payload.get('headers', []))
    pending = [(payload, [], False, False)]
    rows, selections, seen = [], [], set()
    total_bytes = total_headers = header_bytes = 0
    while pending:
        node, path, inherited_attachment, embedded = pending.pop()
        _require(type(node) is dict and id(node) not in seen)
        seen.add(id(node))
        _require(len(path) <= 32 and len(rows) < 1000, 'content_limit')
        mime, part_id, filename = node.get('mimeType'), node.get('partId'), node.get('filename', '')
        _require(type(mime) is str and _MIME.fullmatch(mime) is not None)
        _require(part_id is None or (type(part_id) is str and _PART.fullmatch(part_id) is not None))
        _require(type(filename) is str)
        headers = _headers(node.get('headers', []))
        total_headers += len(headers)
        header_bytes += sum(len((x['name'] + x['value']).encode('utf-8')) for x in headers)
        _require(total_headers <= 2000 and header_bytes <= min(limit, 262144), 'content_limit')
        charset, disposition = _mime(headers, mime)
        body, data = _body(node.get('body', {}), limit - total_bytes)
        total_bytes += len(data) if data is not None else 0
        _require(total_bytes <= limit, 'content_limit')
        children = node.get('parts', [])
        _require(type(children) is list)
        container = mime.lower().startswith('multipart/')
        encapsulated = mime.lower() == 'message/rfc822'
        _require(not children or container or encapsulated)
        _require(not container or (not data and body.get('attachmentId') is None))
        attachment = inherited_attachment or bool(filename) or disposition == 'attachment' or encapsulated
        text_leaf = mime.lower() in {'text/plain', 'text/html'} and not children
        outer = text_leaf and not attachment and not embedded
        text, reason = None, None
        if embedded or encapsulated:
            reason = 'embedded_message'
        elif container:
            reason = 'container'
        elif attachment:
            reason = 'attachment'
        elif not text_leaf:
            reason = 'nontext'
        elif body.get('attachmentId') is not None:
            reason = 'external_body'
        elif data is None:
            reason = 'missing_body'
        else:
            text, reason = _text(data, charset)
        row = {'path': path, 'partId': part_id, 'mimeType': mime, 'filename': filename,
               'disposition': disposition, 'charset': charset if text_leaf else None,
               'is_attachment': attachment, 'is_outer_body': outer, 'embedded_message': embedded or encapsulated,
               'attachmentId': body.get('attachmentId'), 'size': body.get('size'),
               'text': text, 'omitted_reason': reason}
        rows.append(row)
        selections.append((row, body, data, text_leaf))
        for index in reversed(range(len(children))):
            pending.append((children[index], path + [index], attachment, embedded or encapsulated))
    return metadata, outer_headers, rows, selections


def _readable(message, limit):
    metadata, headers, rows, _ = _inspect(message, limit)
    outer = [row for row in rows if row['is_outer_body']]
    omitted_outer = any(row['omitted_reason'] not in {None, 'container'}
                        and not row['is_attachment'] and not row['embedded_message'] for row in rows)
    result = {'message': metadata, 'headers': headers, 'parts': rows,
              'body_complete': bool(outer) and not omitted_outer}
    _json_bound(result, limit)
    return result


def readable_message(message: Any, *, max_bytes: int) -> dict[str, Any]:
    """Decode available outer text only; retain explicit omissions and part identities."""
    return _safe(_readable, message, max_bytes)


def _select(message, part_id, limit):
    _require(type(part_id) is str and _PART.fullmatch(part_id) is not None)
    metadata, _, _, selections = _inspect(message, limit)
    matches = [item for item in selections if item[0]['partId'] == part_id]
    _require(bool(matches), 'part_not_found')
    _require(len(matches) == 1, 'ambiguous_part')
    row, body, data, text_leaf = matches[0]
    _require(text_leaf and not row['embedded_message'], 'unsupported_part')
    _require(row['charset'] in _CHARSETS, 'unsupported_charset')
    _require(body.get('size') is None or body['size'] <= limit, 'content_limit')
    if body.get('attachmentId') is None:
        _require(data is not None, 'missing_body')
        _, reason = _text(data, row['charset'])
        _require(reason is None, reason or 'invalid_text')
    return SelectedTextPart(deepcopy(metadata), deepcopy(row), deepcopy(body))


def select_text_part(message: Any, part_id: str, *, max_bytes: int) -> SelectedTextPart:
    """Resolve one exact fresh part, including root partId='', before any body fetch."""
    return _safe(_select, message, part_id, max_bytes)


def _selected_result(selection, external_body, limit):
    _require(isinstance(selection, SelectedTextPart))
    body = selection.body if selection.body.get('attachmentId') is None else external_body
    _require(body is not None, 'missing_body')
    body, data = _body(body, limit)
    _require(data is not None and body.get('attachmentId') is None, 'missing_body')
    original_size = selection.body.get('size')
    _require(original_size is None or original_size == len(data))
    text, reason = _text(data, selection.part['charset'])
    _require(reason is None, reason or 'invalid_text')
    row = deepcopy(selection.part)
    row.update(text=text, omitted_reason=None)
    result = {'message': deepcopy(selection.message), 'part': row,
              'body_source': 'attachment' if selection.body.get('attachmentId') is not None else 'inline'}
    _json_bound(result, limit)
    return result


def readable_text_part(selection: SelectedTextPart, *, external_body: Any = None, max_bytes: int) -> dict[str, Any]:
    """Decode only the selected body; supplied external bytes must match its declared size."""
    return _safe(_selected_result, selection, external_body, max_bytes)
