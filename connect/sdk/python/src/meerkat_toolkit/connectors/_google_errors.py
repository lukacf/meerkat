"""Optional Calendar/Drive 403 evidence, read by the existing request owner."""
from __future__ import annotations

import json
import math

import httpx


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def _number(raw):
    value = float(raw)
    if not math.isfinite(value):
        raise ValueError("nonfinite JSON number")
    return value


def _constant(_raw):
    raise ValueError("nonfinite JSON constant")


async def is_rate_limit(response: httpx.Response, maximum: int) -> bool:
    """Recognize only consistent documented quota evidence, never provider prose.

    This function owns neither a request nor its deadline/close. Unusable optional
    diagnostics preserve the HTTP refusal; cancellation reaches the caller.
    """
    try:
        if (response.status_code != 403
                or response.headers.get("content-encoding", "identity").lower() != "identity"
                or response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
                != "application/json"):
            return False
        limit = min(maximum, 65_536)
        size = response.headers.get("content-length")
        if size is not None and (not size.isascii() or not size.isdecimal()
                                 or len(size) > 12 or int(size) > limit):
            return False
        body = bytearray()
        chunks = response.aiter_bytes() if response.is_stream_consumed else response.aiter_raw()
        async for chunk in chunks:
            if len(body) + len(chunk) > limit:
                return False
            body.extend(chunk)
        if size is not None and len(body) != int(size):
            return False
        value = json.loads(body.decode("utf-8"), object_pairs_hook=_object,
                           parse_float=_number, parse_constant=_constant)
        error = value.get("error") if type(value) is dict else None
        if type(error) is not dict or type(error.get("code")) is not int or error["code"] != 403:
            return False
        entries = error.get("errors")
        if type(entries) is not list or not 1 <= len(entries) <= 32:
            return False
        selected = None
        for entry in entries:
            if type(entry) is not dict:
                return False
            reason = entry.get("reason")
            if (type(reason) is not str or reason not in {"rateLimitExceeded", "userRateLimitExceeded"}
                    or entry.get("domain") != "usageLimits"
                    or (selected is not None and selected != reason)):
                return False
            selected = reason
        return True
    except Exception:
        # Optional body I/O or parsing failure must not erase the known refusal.
        # CancelledError is a BaseException and retains the outer request custody.
        return False
