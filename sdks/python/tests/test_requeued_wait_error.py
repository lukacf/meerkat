"""A wait answer the SDK names no accessor for keeps its typed data.

A graceful mob Shutdown that keeps a queued input answers its waiting
``turn/start`` with JSON-RPC ``-32003`` (session not running) and
``error.data.kind`` and ``reason`` ``"requeued_across_stop"``. The SDK has no
accessor of its own for that reason, so the error keeps its code and the
whole data payload, and no typed accessor misreads it.
"""

from meerkat import MeerkatError, auth_error_reason, comms_send_error
from meerkat.errors import meerkat_error_from_jsonrpc_code

REQUEUED = {
    "kind": "requeued_across_stop",
    "reason": "requeued_across_stop",
    "session_id": "01936f8b-0000-7000-8000-000000000001",
    "input_id": "01936f8b-0000-7000-8000-000000000002",
    "stopped_at": "2026-10-22T22:00:00Z",
    "resumable": True,
}


def test_a_requeued_wait_keeps_its_code_and_whole_data() -> None:
    error = meerkat_error_from_jsonrpc_code(
        -32003,
        None,
        "the input stays queued across the runtime stop or store fence gap and runs when the "
        "fence is re-acquired or at the next start",
        REQUEUED,
    )
    assert type(error) is MeerkatError
    assert error.code == "-32003"
    assert error.details == REQUEUED
    assert auth_error_reason(error) is None
    assert comms_send_error(error) is None
