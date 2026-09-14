import copy
import json
from pathlib import Path
from typing import get_type_hints
from unittest.mock import AsyncMock

import pytest

from meerkat import MeerkatClient
from meerkat.errors import MeerkatError
from meerkat.generated.types import (
    DelegatedRequestProvenance,
    TranscriptUserRoleDelegatedRequestPayload,
)


def message_fixture():
    root = Path(__file__).resolve().parents[3]
    return json.loads(
        (root / "meerkat-contracts/tests/fixtures/delegated-request-message-v1.json").read_text()
    )


def history(message):
    return {
        "session_id": "session",
        "message_count": 1,
        "offset": 0,
        "has_more": False,
        "messages": [message],
    }


@pytest.mark.asyncio
async def test_session_history_preserves_typed_provisional_request():
    client = MeerkatClient()
    client._request = AsyncMock(return_value=history(message_fixture()))
    result = await client.read_session_history("session")
    message = result.messages[0]
    assert message.content == " exact provisional request "
    payload = message.transcript_role["delegated_request"]
    assert isinstance(payload, TranscriptUserRoleDelegatedRequestPayload)
    assert isinstance(payload.provenance, DelegatedRequestProvenance)
    assert payload.provenance.evidence_kind == "application_snapshot"
    assert payload.provenance.source.source == {"kind": "client_delegation", "delegation": "d"}
    assert get_type_hints(TranscriptUserRoleDelegatedRequestPayload)["provenance"] is DelegatedRequestProvenance
    client._request.assert_awaited_once_with("session/history", {"session_id": "session", "offset": 0})


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", [
    lambda role: role.update(permission=True),
    lambda role: role["delegated_request"].update(grant=True),
    lambda role: role["delegated_request"]["provenance"].update(final_user_transcript=True),
    lambda role: role["delegated_request"]["provenance"].pop("evidence_kind"),
    lambda role: role["delegated_request"]["provenance"].update(source="wrong shape"),
    lambda role: role["delegated_request"]["provenance"]["source"]["source"].update(grant=True),
    lambda role: role["delegated_request"]["provenance"]["request_digest"].pop(),
    lambda role: role["delegated_request"]["provenance"]["request_digest"].__setitem__(0, -1),
    lambda role: role["delegated_request"]["provenance"]["request_digest"].__setitem__(0, 256),
])
async def test_history_rejects_malformed_or_permission_bearing_provenance(mutation):
    message = copy.deepcopy(message_fixture())
    mutation(message["transcript_role"])
    client = MeerkatClient()
    client._request = AsyncMock(return_value=history(message))
    with pytest.raises(MeerkatError) as error:
        await client.read_session_history("session")
    assert error.value.code == "INVALID_RESPONSE"
