"""Issue #159: the temporary-council wrappers issue the exact RPC method
literals with snake_case params, keep the one-time host bootstrap OUT of the
fingerprinted request, treat an unknown council as a typed absence, and fail
closed on a malformed envelope."""

from __future__ import annotations

from typing import Any

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError

REQUEST: dict[str, Any] = {
    "council_id": "design-review-42",
    "definition_template": {"id": "ignored", "profiles": {}},
    "participants": [
        {
            "order": 0,
            "role": "critic",
            "source_mob_id": "source",
            "source_identity": "alice",
            "target_identity": "alice-branch",
            "target_profile": "council",
            "scope": "invoke_and_observe",
        }
    ],
    "topic": "should we ship?",
    "bounds": {
        "deadline": {"kind": "relative", "after_millis": 60_000},
        "max_rounds": 1,
        "max_exchanges": 2,
        "max_result_bytes": 4096,
    },
    "merge_back": {"policy": "no_merge"},
    "durability": "durable",
}

RUN_RESULT: dict[str, Any] = {
    "result": {
        "council_id": "design-review-42",
        "request_fingerprint": "sha256:abc",
        "temporary_mob_id": "council--design-review-42",
        "exit_reason": {"reason": "completed"},
        "rounds_completed": 1,
        "exchanges": [],
        "merge": {"kind": "no_merge", "confirmed_participants": ["alice-branch"]},
        "participants": [],
        "truncated_exchange_count": 0,
        "merge_truncated": False,
        "durability": "durable",
        "concluded_at": "2026-08-28T10:00:00.000Z",
    },
    "cleanup": {
        "status": "settled",
        "attempted_at": "2026-08-28T10:00:01.000Z",
        "attempts": 1,
        "temporary_mob_destroyed": True,
        "released_participants": [0],
        "revoked_participants": [],
        "debts": [],
        "budget_exhausted": False,
    },
    "replayed": False,
}


def canned_client(
    result: dict[str, Any],
) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    calls: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method: str, params: dict[str, Any]) -> dict[str, Any]:
        calls.append((method, params))
        return dict(result)

    client._request = fake_request  # type: ignore[assignment]
    return client, calls


def test_temporary_council_client_surface_is_present() -> None:
    for name in (
        "run_temporary_council",
        "get_temporary_council",
        "recover_temporary_councils",
    ):
        assert callable(getattr(MeerkatClient, name, None))


@pytest.mark.asyncio
async def test_run_issues_the_method_without_a_bootstrap() -> None:
    client, calls = canned_client(RUN_RESULT)
    outcome = await client.run_temporary_council(REQUEST)
    assert calls == [("mob/temporary_council_run", {"request": REQUEST})]
    assert outcome.replayed is False
    assert outcome.result["exit_reason"]["reason"] == "completed"
    assert outcome.cleanup["status"] == "settled"


@pytest.mark.asyncio
async def test_host_bootstrap_stays_outside_the_request() -> None:
    client, calls = canned_client(RUN_RESULT)
    descriptor = {
        "kind": "host",
        "address": "tcp://10.0.0.2:7100",
        "identity": {"public_key": "AAAA"},
        "bootstrap_token": "one-time",
    }
    await client.run_temporary_council(REQUEST, host_bindings=[descriptor])
    method, params = calls[0]
    assert method == "mob/temporary_council_run"
    assert params["host_bindings"] == [descriptor]
    assert "host_bindings" not in params["request"], (
        "a one-time ceremony token must never be folded into the council request"
    )


@pytest.mark.asyncio
async def test_get_reports_typed_absence() -> None:
    client, calls = canned_client({})
    result = await client.get_temporary_council("never-created")
    assert calls == [("mob/temporary_council_get", {"council_id": "never-created"})]
    assert result.council is None


@pytest.mark.asyncio
async def test_recover_issues_the_sweep_with_no_params() -> None:
    client, calls = canned_client({"reports": []})
    result = await client.recover_temporary_councils()
    assert calls == [("mob/temporary_council_recover", {})]
    assert result.reports == []


@pytest.mark.asyncio
async def test_run_fails_closed_on_a_malformed_envelope() -> None:
    for malformed in (
        {"cleanup": RUN_RESULT["cleanup"], "replayed": False},
        {"result": RUN_RESULT["result"], "replayed": False},
        {"result": RUN_RESULT["result"], "cleanup": RUN_RESULT["cleanup"]},
    ):
        client, _ = canned_client(malformed)
        with pytest.raises(MeerkatError):
            await client.run_temporary_council(REQUEST)


@pytest.mark.asyncio
async def test_get_fails_closed_on_a_non_object_projection() -> None:
    client, _ = canned_client({"council": "not-a-record"})
    with pytest.raises(MeerkatError):
        await client.get_temporary_council("design-review-42")
