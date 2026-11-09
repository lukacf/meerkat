"""Typed outcomes of the meta-forge command line and their exit codes.

The codes keep 0, 1 and 2 with rkat's meaning, use 130 for Ctrl-C and put
Forge's own outcomes in 80-87, so they never collide with rkat's typed codes
(10-59). Automation reads error.code, not only the number.
"""
from __future__ import annotations

EXIT_CODES = {
    "operation_failed": 1,
    "operation_interrupted": 1,
    "internal_error": 1,
    "usage": 2,
    "invalid_request": 2,
    "server_unavailable": 80,
    "server_incompatible": 80,
    "not_found": 81,
    "stale_version": 82,
    "conflict": 82,
    "busy": 82,
    "build_selection_unavailable": 82,
    "outcome_unknown": 83,
    "approval_required": 84,
    "input_required": 85,
    "prerequisite_unavailable": 86,
    "wait_timeout": 87,
    "interrupted": 130,
}


def step(command, reason):
    """One suggested next command."""
    return {"command": command, "reason": reason}


class CliError(Exception):
    """A typed failure: what happened, its exit code and what to run next."""

    def __init__(self, code, detail, *, http_status=None, server_code=None, needs=(), next=(),
                 review=None, digest=None, result=None, request=None):
        super().__init__(detail)
        if code not in EXIT_CODES:
            raise ValueError("Unknown command-line outcome: " + str(code))
        self.code = code
        self.detail = detail
        self.http_status = http_status
        self.server_code = server_code
        self.needs = list(needs)
        self.next = list(next)
        self.review = review
        self.digest = digest
        self.result = result
        self.request = request

    @property
    def exit_code(self):
        return EXIT_CODES[self.code]

    def error_object(self):
        return {"code": self.code, "exit_code": self.exit_code, "http_status": self.http_status,
                "server_code": self.server_code, "detail": self.detail}


def _detail(payload):
    """The server's own refusal text. Validation lists name fields and reasons only."""
    if isinstance(payload, dict):
        value = payload.get("detail")
        if isinstance(value, str) and value.strip():
            return value.strip()
        if isinstance(value, list):
            parts = []
            for item in value[:20]:
                if isinstance(item, dict):
                    location = item.get("loc")
                    where = ".".join(str(part) for part in location) if isinstance(location, (list, tuple)) else ""
                    message = item.get("msg") if isinstance(item.get("msg"), str) else "invalid"
                    parts.append(f"{where}: {message}" if where else message)
                elif isinstance(item, str):
                    parts.append(item)
            if parts:
                return "; ".join(parts)
    return None


def from_response(status, payload):
    """Map an HTTP refusal onto a typed outcome without guessing from its text."""
    detail = _detail(payload) or f"The Forge server answered HTTP {status}."
    server_code = payload.get("code") if isinstance(payload, dict) and isinstance(payload.get("code"), str) else None
    if status == 404:
        code = "not_found"
    elif status == 409:
        code = server_code if server_code in EXIT_CODES and EXIT_CODES[server_code] == 82 else "conflict"
    elif status == 503:
        code = "prerequisite_unavailable"
    elif status in (500, 502, 504) or status >= 500:
        code = "outcome_unknown"
    else:
        code = "invalid_request"
    return CliError(code, detail, http_status=status, server_code=server_code)
