"""The client's settlement companion validator accepts every value of its
generated literal domains, including domains generated as unions."""

from typing import Literal, Optional

import pytest

from meerkat.client import MeerkatClient, _literal_values
from meerkat.errors import MeerkatError
from meerkat.generated.types import ToolDispatchTerminalErrorKind


def _result(failure_kind: str) -> dict:
    return {
        "settlement_failures": [
            {
                "admission_source": "configured_gate",
                "effect_kind": "tool_dispatch",
                "physical_outcome": "failed",
                "failure_kind": failure_kind,
            }
        ]
    }


@pytest.mark.parametrize("failure_kind", ["hook_denied", "outcome_uncertain"])
def test_released_and_new_terminal_kinds_are_accepted(failure_kind: str) -> None:
    MeerkatClient._validate_tool_dispatch_settlement_failures(
        _result(failure_kind), "tool result"
    )


def test_an_unknown_terminal_kind_is_refused() -> None:
    with pytest.raises(MeerkatError) as refused:
        MeerkatClient._validate_tool_dispatch_settlement_failures(
            _result("future_kind"), "tool result"
        )
    assert refused.value.code == "INVALID_RESPONSE"


def test_the_terminal_kind_domain_is_every_generated_string() -> None:
    domain = _literal_values(ToolDispatchTerminalErrorKind)
    assert {"hook_denied", "outcome_uncertain", "other"} <= domain
    assert all(isinstance(value, str) for value in domain)


def test_union_literal_domains_are_flattened() -> None:
    union = Literal["a", "b"] | Literal["c"]
    assert _literal_values(union) == {"a", "b", "c"}
    assert _literal_values(Optional[Literal["a"]]) == {"a"}
    assert _literal_values(Literal["a"]) == {"a"}
