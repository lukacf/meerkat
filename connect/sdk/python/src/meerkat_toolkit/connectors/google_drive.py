"""Explicit Drive operations with borrowed HTTP, streaming media and one attempt.

The host owns credentials, account selection, authorization and reconciliation.
No OAuth, automatic retry, redirect, page traversal or resumable upload session.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from enum import Enum
import functools
import json
import math
import mimetypes
import os
from pathlib import Path
import re
import stat
import tempfile
from typing import Any, BinaryIO
from urllib.parse import quote
import uuid

import anyio
import httpx

from ..httpx_client import HttpxClientNotPrepared, require_prepared_httpx_client
from ._google_errors import is_rate_limit
from ._httpx_stream import send_with_custody


class FailureKind(str, Enum):
    AUTHENTICATION = "authentication"
    PERMISSION = "permission"
    NOT_FOUND = "not_found"
    CONFLICT = "conflict"
    PRECONDITION = "precondition"
    RATE_LIMITED = "rate_limited"
    REDIRECT = "redirect"
    HTTP_STATUS = "http_status"
    INVALID_RESPONSE = "invalid_response"
    RESPONSE_TOO_LARGE = "response_too_large"
    UNSUPPORTED_ENCODING = "unsupported_encoding"
    TIMEOUT = "timeout"
    TRANSPORT = "transport"
    CLIENT = "client"
    LOCAL_IO = "local_io"


class WriteOutcome(str, Enum):
    NOT_APPLICABLE = "not_applicable"
    NOT_ATTEMPTED = "not_attempted"
    UNKNOWN = "unknown"


class DriveError(Exception):
    """Safe metadata only, never provider prose, credentials, paths or URLs."""

    def __init__(
        self,
        kind: FailureKind,
        *,
        write_outcome: WriteOutcome,
        status_code: int | None = None,
        retry_after_seconds: int | None = None,
    ) -> None:
        self.kind = kind
        self.write_outcome = write_outcome
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds
        status = "" if status_code is None else f" (HTTP {status_code})"
        super().__init__(f"Google Drive {kind.value}{status}; write outcome: {write_outcome.value}")


class DriveWriteCancelled(asyncio.CancelledError):
    """Entered work has joined; cancellation cannot undo an external mutation."""

    write_outcome = WriteOutcome.UNKNOWN

    def __init__(self) -> None:
        super().__init__("Google Drive write cancelled; write outcome: unknown")


def _query_data(value: Any, *, empty: bool = False, maximum: int = 2_097_152) -> str:
    if not isinstance(value, str) or (not empty and not value):
        raise ValueError("expected text")
    try:
        if len(value.encode("utf-8")) <= maximum:
            return value
    except UnicodeError:
        pass
    raise ValueError("text exceeds the supported UTF-8 limit") from None


def _text(value: Any, *, empty: bool = False, maximum: int = 2_097_152) -> str:
    value = _query_data(value, empty=empty, maximum=maximum)
    if any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ValueError("text must not contain control characters")
    return value


def _segment(value: str) -> str:
    value = _text(value, maximum=8192)
    if value in {".", ".."}:
        raise ValueError("dot path segments are not identifiers")
    return quote(value, safe="")


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def _number(value: str) -> float:
    result = float(value)
    if not math.isfinite(result):
        raise ValueError("non-finite JSON number")
    return result


def _constant(_value: str) -> Any:
    raise ValueError("non-JSON numeric constant")


async def _io(function, *args):
    # The send owner can receive raw Task.cancel during a multipart read.
    # AnyIO's worker cancellation policy alone does not cover that edge. Retain
    # the worker until it finishes, before its surrounding owner closes files.
    with anyio.CancelScope(shield=True):
        owner = asyncio.create_task(
            anyio.to_thread.run_sync(functools.partial(function, *args), abandon_on_cancel=False)
        )
        cancellation = None
        while not owner.done():
            try:
                await asyncio.shield(owner)
            except asyncio.CancelledError as exc:
                cancellation = exc
            except Exception:
                break
        try:
            result = owner.result()
        except Exception:
            if cancellation is None:
                raise
        if cancellation is not None:
            raise cancellation
        return result


@dataclass
class _Attempt:
    write: bool
    entered: bool = False


@dataclass(frozen=True)
class DriveContent:
    """Complete bounded response bytes and its validated media type, not a revision snapshot."""

    data: bytes
    mime_type: str


class GoogleDriveOperations:
    """Explicit operations on a borrowed client and selected API root.

    The root normally is https://www.googleapis.com/; both drive/v3 and
    upload/drive/v3 are resolved below it. An explicit auth object is per-request;
    None retains client auth. Host hooks/auth/transports must not retarget or
    replay requests. The client is never closed. JSON limits do not limit disk
    media; max_content_bytes bounds the explicit in-memory content methods.
    """

    def __init__(
        self,
        client: httpx.AsyncClient,
        *,
        base_url: str | httpx.URL | None = None,
        auth: httpx.Auth | None = None,
        timeout_seconds: float = 30.0,
        max_response_bytes: int = 2_097_152,
        max_request_bytes: int = 2_097_152,
        max_content_bytes: int = 2_097_152,
    ) -> None:
        base = client.base_url if base_url is None else httpx.URL(base_url)
        if (
            base.scheme not in {"http", "https"}
            or not base.host
            or base.username
            or base.password
            or base.query
            or base.fragment
        ):
            raise ValueError("select one HTTP(S) API root without userinfo or query")
        if (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        if any(type(n) is not int or n <= 0 for n in (max_response_bytes, max_request_bytes, max_content_bytes)):
            raise ValueError("byte limits must be positive integers")
        self._client, self._base, self._auth = client, base, auth
        self._timeout = float(timeout_seconds)
        self._max_bytes, self._max_request_bytes = max_response_bytes, max_request_bytes
        self._max_content_bytes = max_content_bytes

    def _failure(
        self, kind: FailureKind, attempt: _Attempt, response: httpx.Response | None = None
    ) -> DriveError:
        retry_after = None
        if response is not None:
            value = response.headers.get("retry-after", "")
            if value.isascii() and value.isdecimal() and len(value) <= 9:
                retry_after = int(value)
        outcome = WriteOutcome.UNKNOWN if attempt.entered else WriteOutcome.NOT_ATTEMPTED
        return DriveError(
            kind,
            write_outcome=outcome if attempt.write else WriteOutcome.NOT_APPLICABLE,
            status_code=None if response is None else response.status_code,
            retry_after_seconds=retry_after,
        )

    def _body(self, value: dict[str, Any]) -> bytes:
        if not isinstance(value, dict):
            raise ValueError("expected a JSON body object")
        try:

            def validate(item: Any) -> None:
                if isinstance(item, dict):
                    if any(not isinstance(key, str) for key in item):
                        raise ValueError("expected string JSON keys")
                    for child in item.values():
                        validate(child)
                elif isinstance(item, list):
                    for child in item:
                        validate(child)
                elif item is not None and type(item) not in (str, int, float, bool):
                    raise ValueError("expected JSON values")

            validate(value)
            encoded = json.dumps(value, allow_nan=False, separators=(",", ":")).encode("utf-8")
        except (ValueError, TypeError, UnicodeError, RecursionError):
            raise ValueError("expected a finite JSON body") from None
        if len(encoded) > self._max_request_bytes:
            raise ValueError("JSON body exceeds the request limit")
        return encoded

    async def _call(
        self,
        method: str,
        path: str,
        *,
        params: list[tuple[str, str]] | None = None,
        body: dict[str, Any] | None = None,
        upload_path: str | Path | None = None,
        upload_content_type: str | None = None,
        output_path: str | Path | None = None,
        expected_id: str | None = None,
        page: bool = False,
        media: bool = False,
    ) -> Any:
        attempt = _Attempt(write=method != "GET")
        try:
            require_prepared_httpx_client(self._client)
        except HttpxClientNotPrepared:
            raise self._failure(FailureKind.CLIENT, attempt) from None
        content = None if body is None else self._body(body)
        upload = None if upload_path is None else Path(upload_path).absolute()
        output_label = None if output_path is None else str(output_path)
        output = None if output_label is None else Path(output_label).absolute()
        if upload_content_type is not None and upload is None:
            raise ValueError("upload_content_type requires upload_path")
        mime = None
        if upload is not None:
            frozen_metadata = json.loads(content) if content is not None else {}
            metadata_mime = frozen_metadata.get("mimeType")
            mime = (
                _text(upload_content_type)
                if upload_content_type is not None
                else (
                    mimetypes.guess_type(str(upload))[0]
                    or (metadata_mime if isinstance(metadata_mime, str) and metadata_mime else None)
                    or "application/octet-stream"
                )
            )
            mime = _text(mime)
            if "/" not in mime or not mime.isascii():
                raise ValueError("expected a media content type")
        url = self._base.join(("upload/" if upload is not None else "") + "drive/v3/" + path)
        query = list(params or ())
        if upload is not None:
            query.append(("uploadType", "multipart"))
        if query:
            url = url.copy_with(query=str(httpx.QueryParams(tuple(query))).encode("ascii"))
        scope = anyio.CancelScope(deadline=anyio.current_time() + self._timeout)
        owner = asyncio.create_task(
            self._request(
                method, url, content, upload, mime, output, expected_id, page, media, attempt, scope
            )
        )
        cancelled = False
        while not owner.done():
            try:
                await asyncio.shield(owner)
            except asyncio.CancelledError:
                cancelled = True
                scope.cancel()
            except Exception:
                break
        if cancelled:
            try:
                owner.result()
            except (Exception, asyncio.CancelledError):
                pass
            raise (
                DriveWriteCancelled()
                if attempt.write and attempt.entered
                else asyncio.CancelledError()
            ) from None
        result = owner.result()
        if output_label is not None:
            result["output_path"] = output_label
        return result

    async def _request(
        self,
        method,
        url,
        content,
        upload,
        mime,
        output,
        expected_id,
        page,
        media,
        attempt: _Attempt,
        scope: anyio.CancelScope,
    ):
        response: httpx.Response | None = None
        source: BinaryIO | None = None
        destination: BinaryIO | None = None
        staging: str | None = None
        failure: BaseException | None = None
        payload: Any = None
        try:
            with scope:
                headers = {
                    "Accept": "*/*" if output is not None or media else "application/json",
                    "Accept-Encoding": "identity",
                }
                request_content = content
                if upload is not None:
                    try:

                        def open_source():
                            fd = os.open(upload, os.O_RDONLY | os.O_NONBLOCK)
                            try:
                                info = os.fstat(fd)
                                if not stat.S_ISREG(info.st_mode):
                                    raise OSError("expected regular upload file")
                                return os.fdopen(fd, "rb"), info
                            except BaseException:
                                os.close(fd)
                                raise

                        source, info = await _io(open_source)
                    except OSError:
                        raise self._failure(FailureKind.LOCAL_IO, attempt) from None
                    boundary = "toolkit_" + uuid.uuid4().hex
                    prefix = (
                        f"--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n".encode(
                            "ascii"
                        )
                        + (content or b"{}")
                        + f"\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n".encode("utf-8")
                    )
                    suffix = f"\r\n--{boundary}--\r\n".encode("ascii")
                    headers["Content-Type"] = f"multipart/related; boundary={boundary}"
                    headers["Content-Length"] = str(len(prefix) + info.st_size + len(suffix))

                    async def media():
                        yield prefix
                        remaining = info.st_size
                        while remaining:
                            chunk = await _io(source.read, min(65_536, remaining))
                            if not chunk:
                                raise OSError("upload file changed length")
                            remaining -= len(chunk)
                            yield chunk
                        if await _io(source.read, 1):
                            raise OSError("upload file changed length")
                        yield suffix

                    request_content = media()
                elif content is not None:
                    headers["Content-Type"] = "application/json"
                    headers["Content-Length"] = str(len(content))
                if output is not None:

                    def make_staging():
                        fd, name = tempfile.mkstemp(prefix=".toolkit-drive-", dir=output.parent)
                        try:
                            return os.fdopen(fd, "wb"), name
                        except BaseException:
                            try:
                                os.close(fd)
                            finally:
                                os.unlink(name)
                            raise

                    try:
                        destination, staging = await _io(make_staging)
                    except OSError:
                        raise self._failure(FailureKind.LOCAL_IO, attempt) from None
                request = self._client.build_request(
                    method, url, content=request_content, headers=headers, timeout=self._timeout
                )
                request.url = url  # No inherited pagination, field masks or routing parameters.
                for header in list(request.headers):
                    if header.lower().startswith("if-") or header.lower() in {
                        "range",
                        "x-http-method-override",
                        "x-method-override",
                        "x-http-method",
                        "content-range",
                        "transfer-encoding",
                        "content-length",
                        "content-type",
                    }:
                        request.headers.pop(header, None)
                for header in ("Content-Type", "Content-Length"):
                    if header in headers:
                        request.headers[header] = headers[header]
                # Local allocation is shielded until file ownership returns.
                # Honor cancellation already observed during that preparation
                # before starting credentials, transport or a provider effect.
                await anyio.lowlevel.checkpoint()
                if scope.cancel_called or anyio.current_time() >= scope.deadline:
                    raise self._failure(FailureKind.TIMEOUT, attempt)
                try:
                    require_prepared_httpx_client(self._client)
                except HttpxClientNotPrepared:
                    raise self._failure(FailureKind.CLIENT, attempt) from None
                attempt.entered = True
                try:
                    response = await send_with_custody(self._client, request, auth=self._auth)
                except HttpxClientNotPrepared:
                    # Only the shared send owner's pre-entry validator can
                    # return this type. After entry it sanitizes the same type
                    # from host hooks as an ordinary uncertain client failure.
                    attempt.entered = False
                    raise self._failure(FailureKind.CLIENT, attempt) from None
                try:
                    payload = await self._read(
                        response, method, output, destination, expected_id, page, media, attempt
                    )
                finally:
                    with anyio.CancelScope(shield=True):
                        await response.aclose()
                if destination is not None:
                    await _io(destination.flush)
                    await _io(os.fsync, destination.fileno())
                    await _io(destination.close)
                    destination = None
                # This checkpoint prevents publishing after a cancellation/deadline
                # observed during response cleanup. A replace already entered is
                # atomic but cannot be undone by a racing cancellation.
                await anyio.lowlevel.checkpoint()
                if scope.cancel_called or anyio.current_time() >= scope.deadline:
                    raise self._failure(FailureKind.TIMEOUT, attempt)
                if staging is not None:
                    await _io(os.replace, staging, output)
                    staging = None
            if scope.cancel_called:
                failure = self._failure(FailureKind.TIMEOUT, attempt)
        except DriveError as exc:
            failure = exc
        except asyncio.CancelledError:
            failure = (
                DriveWriteCancelled()
                if attempt.write and attempt.entered
                else asyncio.CancelledError()
            )
        except (TimeoutError, httpx.TimeoutException):
            failure = self._failure(FailureKind.TIMEOUT, attempt)
        except httpx.HTTPError:
            failure = self._failure(FailureKind.TRANSPORT, attempt)
        except OSError:
            failure = self._failure(FailureKind.LOCAL_IO, attempt)
        except Exception:
            failure = self._failure(FailureKind.CLIENT, attempt)
        finally:
            with anyio.CancelScope(shield=True):
                for handle in (source, destination):
                    if handle is not None:
                        try:
                            await _io(handle.close)
                        except Exception:
                            if failure is None:
                                failure = self._failure(FailureKind.LOCAL_IO, attempt)
                if staging is not None:
                    try:
                        await _io(os.unlink, staging)
                    except FileNotFoundError:
                        pass
                    except Exception:
                        if failure is None:
                            failure = self._failure(FailureKind.LOCAL_IO, attempt)
        if failure is not None:
            raise failure from None
        return payload

    async def _read(self, response, method, output, destination, expected_id, page, media, attempt):
        status = response.status_code
        allowed = {200, 204} if method == "DELETE" else {200, 201} if method == "POST" else {200}
        if status not in allowed:
            kind = {
                401: FailureKind.AUTHENTICATION,
                403: FailureKind.PERMISSION,
                404: FailureKind.NOT_FOUND,
                409: FailureKind.CONFLICT,
                412: FailureKind.PRECONDITION,
                429: FailureKind.RATE_LIMITED,
            }.get(status, FailureKind.HTTP_STATUS)
            if 300 <= status < 400:
                kind = FailureKind.REDIRECT
            if status == 403 and await is_rate_limit(response, self._max_bytes):
                kind = FailureKind.RATE_LIMITED
            raise self._failure(kind, attempt, response)
        if response.headers.get("content-encoding", "identity").lower() != "identity":
            raise self._failure(FailureKind.UNSUPPORTED_ENCODING, attempt, response)
        sizes = response.headers.get_list("content-length")
        if len(sizes) > 1 or (
            sizes and (not sizes[0].isascii() or not sizes[0].isdecimal() or len(sizes[0]) > 20)
        ):
            raise self._failure(FailureKind.INVALID_RESPONSE, attempt, response)
        expected_size = int(sizes[0]) if sizes else None
        maximum = self._max_content_bytes if media else self._max_bytes
        if output is None and expected_size is not None and expected_size > maximum:
            raise self._failure(FailureKind.RESPONSE_TOO_LARGE, attempt, response)
        data = bytearray()
        total = 0
        chunks = response.aiter_bytes() if response.is_stream_consumed else response.aiter_raw()
        async for chunk in chunks:
            total += len(chunk)
            if expected_size is not None and total > expected_size:
                raise self._failure(FailureKind.INVALID_RESPONSE, attempt, response)
            if output is None:
                if total > maximum:
                    raise self._failure(FailureKind.RESPONSE_TOO_LARGE, attempt, response)
                data.extend(chunk)
            else:

                def write_chunk():
                    remaining = memoryview(chunk)
                    while remaining:
                        written = destination.write(remaining)
                        if written is None or written <= 0:
                            raise OSError("incomplete local write")
                        remaining = remaining[written:]

                await _io(write_chunk)
        if expected_size is not None and total != expected_size:
            raise self._failure(FailureKind.INVALID_RESPONSE, attempt, response)
        if output is not None:
            return {"output_path": str(output), "bytes": total}
        if media:
            # Keep the provider's parameters (including charset). Missing type
            # means opaque bytes, not a guessed text encoding or file format.
            mime = response.headers.get("content-type", "application/octet-stream")
            if (len(mime) > 1024 or not mime.isascii()
                    or any(ord(char) < 32 or ord(char) == 127 for char in mime)
                    or not re.fullmatch(r"[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+(?:;[^\r\n]*)?", mime)):
                raise self._failure(FailureKind.INVALID_RESPONSE, attempt, response)
            return DriveContent(bytes(data), mime)
        try:
            if method == "DELETE" and status == 204:
                if data:
                    raise ValueError("expected empty response")
                return None
            if (
                response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
                != "application/json"
            ):
                raise ValueError("expected JSON media type")
            value = json.loads(
                data.decode("utf-8"),
                object_pairs_hook=_object,
                parse_float=_number,
                parse_constant=_constant,
            )
            if not isinstance(value, dict):
                raise ValueError("expected provider result object")
            if method == "DELETE":
                if value:
                    raise ValueError("expected empty deletion object")
                return None
            if expected_id is not None and "id" in value and value["id"] != expected_id:
                raise ValueError("provider identity mismatch")
            if page:
                if "files" in value and (
                    not isinstance(value["files"], list)
                    or any(not isinstance(x, dict) for x in value["files"])
                ):
                    raise ValueError("expected file rows")
                if "nextPageToken" in value:
                    _text(value["nextPageToken"])
            return value
        except (ValueError, TypeError, RecursionError):
            raise self._failure(FailureKind.INVALID_RESPONSE, attempt, response) from None

    async def list_files_page(
        self,
        *,
        query: str | None = None,
        page_size: int | None = None,
        fields: str | None = None,
        order_by: str | None = None,
        page_token: str | None = None,
    ) -> dict[str, Any]:
        params = []
        for name, value in (
            ("q", query),
            ("fields", fields),
            ("orderBy", order_by),
            ("pageToken", page_token),
        ):
            if value is not None:
                validator = _query_data if name == "q" else _text
                params.append((name, validator(value, empty=name in {"q", "fields"})))
        if page_size is not None:
            if type(page_size) is not int or not 1 <= page_size <= 1000:
                raise ValueError("page_size must be an integer from 1 to 1000")
            params.append(("pageSize", str(page_size)))
        return await self._call("GET", "files", params=params, page=True)

    async def get_file(self, file_id: str, *, fields: str | None = None) -> dict[str, Any]:
        params = [] if fields is None else [("fields", _text(fields, empty=True))]
        return await self._call(
            "GET", "files/" + _segment(file_id), params=params, expected_id=file_id
        )

    async def download_file(self, file_id: str, output_path: str | Path) -> dict[str, Any]:
        return await self._call(
            "GET", "files/" + _segment(file_id), params=[("alt", "media")], output_path=output_path
        )

    async def download_content(self, file_id: str) -> DriveContent:
        """Read one blob into bounded memory. Never writes a file or retries."""
        return await self._call(
            "GET", "files/" + _segment(file_id), params=[("alt", "media")], media=True
        )

    async def export_content(self, file_id: str, mime_type: str) -> DriveContent:
        """Read one explicit Workspace export into bounded memory, without conversion."""
        mime_type = _text(mime_type, maximum=256)
        if not re.fullmatch(r"[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+", mime_type):
            raise ValueError("expected an export media type without parameters")
        return await self._call(
            "GET", "files/" + _segment(file_id) + "/export",
            params=[("mimeType", mime_type)], media=True,
        )

    async def export_file(
        self, file_id: str, mime_type: str, output_path: str | Path
    ) -> dict[str, Any]:
        return await self._call(
            "GET",
            "files/" + _segment(file_id) + "/export",
            params=[("mimeType", _text(mime_type))],
            output_path=output_path,
        )

    async def create_file(
        self,
        metadata: dict[str, Any],
        *,
        upload_path: str | Path | None = None,
        upload_content_type: str | None = None,
        fields: str | None = None,
    ) -> dict[str, Any]:
        params = [] if fields is None else [("fields", _text(fields, empty=True))]
        return await self._call(
            "POST",
            "files",
            params=params,
            body=metadata,
            upload_path=upload_path,
            upload_content_type=upload_content_type,
        )

    async def update_file(
        self,
        file_id: str,
        metadata: dict[str, Any],
        *,
        upload_path: str | Path | None = None,
        upload_content_type: str | None = None,
        add_parents: str | None = None,
        remove_parents: str | None = None,
        fields: str | None = None,
    ) -> dict[str, Any]:
        params = []
        for name, value in (
            ("addParents", add_parents),
            ("removeParents", remove_parents),
            ("fields", fields),
        ):
            if value is not None:
                params.append((name, _text(value, empty=name == "fields")))
        return await self._call(
            "PATCH",
            "files/" + _segment(file_id),
            params=params,
            body=metadata,
            upload_path=upload_path,
            upload_content_type=upload_content_type,
            expected_id=file_id,
        )

    async def create_permission(self, file_id: str, permission: dict[str, Any]) -> dict[str, Any]:
        return await self._call(
            "POST", "files/" + _segment(file_id) + "/permissions", body=permission
        )

    async def delete_file(self, file_id: str) -> None:
        return await self._call("DELETE", "files/" + _segment(file_id))
