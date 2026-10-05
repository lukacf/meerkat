"""The generated connector target keeps the typed account-selection alias."""

import typing

from meerkat.generated import types as generated


def test_account_selection_is_the_named_union_alias() -> None:
    hints = typing.get_type_hints(generated.WireConnectorAuthTarget)
    assert hints["account_selection"] == generated.WireConnectorAccountSelection
    arms = typing.get_args(generated.WireConnectorAccountSelection)
    assert set(arms) == {
        generated.WireConnectorAccountSelectionKnown,
        generated.WireConnectorAccountSelectionDiscover,
    }
    known = typing.get_type_hints(generated.WireConnectorAccountSelectionKnown, include_extras=True)
    assert known["mode"] == typing.Required[typing.Literal["known"]]
    assert known["account"] == typing.Required[str]
    discover = typing.get_type_hints(
        generated.WireConnectorAccountSelectionDiscover, include_extras=True
    )
    assert discover == {"mode": typing.Required[typing.Literal["discover"]]}
