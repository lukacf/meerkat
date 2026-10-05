"""Generated connector account selection remains a usable consumer union."""

from typing import TYPE_CHECKING, Literal, get_type_hints

from meerkat.generated.types import (
    InstructionActivationIdentity,
    InstructionActivationReceipt,
    WireConnectorAccountSelection,
    WireConnectorAccountSelectionDiscover,
    WireConnectorAccountSelectionKnown,
    WireConnectorAuthTarget,
    WireConnectorSlot,
)


def test_connector_target_preserves_named_account_selection() -> None:
    assert (
        get_type_hints(WireConnectorAuthTarget)["account_selection"]
        == WireConnectorAccountSelection
    )


def test_local_instruction_annotations_do_not_widen_to_placeholder_aliases() -> None:
    assert {
        "activation_id": get_type_hints(InstructionActivationIdentity)["activation_id"],
        "disposition": get_type_hints(InstructionActivationReceipt)["disposition"],
    } == {"activation_id": str, "disposition": Literal["applied", "duplicate"]}


def test_connector_target_accepts_known_and_discover_consumers() -> None:
    slot = WireConnectorSlot(realm_id="test", slot_id="calendar")
    known = WireConnectorAuthTarget(
        account_selection={"mode": "known", "account": "account-1"},
        client="client-1",
        issuer="https://issuer.invalid",
        resource="calendar",
        scopes=["calendar.read"],
        slot=slot,
        strategy_id="oidc-userinfo-v1",
    )
    discover = WireConnectorAuthTarget(
        account_selection={"mode": "discover"},
        client="client-1",
        issuer="https://issuer.invalid",
        resource="calendar",
        scopes=["calendar.read"],
        slot=slot,
        strategy_id="oidc-userinfo-v1",
    )
    assert known.account_selection == {"mode": "known", "account": "account-1"}
    assert discover.account_selection == {"mode": "discover"}


if TYPE_CHECKING:
    from typing import assert_type

    # Direct mypy with --warn-unused-ignores verifies that these two invalid
    # consumer assignments are rejected. Pytest checks the generated annotation
    # above; runtime dataclasses intentionally do not validate these payloads.
    def _invalid_selections(target: WireConnectorAuthTarget) -> None:
        target.account_selection = {"mode": "known"}  # type: ignore[typeddict-item]
        target.account_selection = {"mode": "unknown", "account": "account-1"}  # type: ignore[typeddict-item]

    def _selected_account(target: WireConnectorAuthTarget) -> str | None:
        selection = target.account_selection
        if selection["mode"] == "known":
            assert_type(selection, WireConnectorAccountSelectionKnown)
            assert_type(selection["account"], str)
            return selection["account"]
        assert_type(selection, WireConnectorAccountSelectionDiscover)
        return None
