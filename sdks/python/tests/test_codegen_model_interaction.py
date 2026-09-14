from typing import Literal, get_args, get_origin, get_type_hints

from meerkat.generated.types import (
    CustomModelConfig,
    LiveObservationRecord,
    WireLiveAdapterObservation,
)


def test_custom_model_interaction_keeps_closed_generated_vocabulary() -> None:
    hint = get_type_hints(CustomModelConfig)["interaction_kind"]
    alternatives = [value for value in get_args(hint) if value is not type(None)]
    assert len(alternatives) == 1
    assert get_origin(alternatives[0]) is Literal
    assert set(get_args(alternatives[0])) == {
        "text",
        "turn_based_realtime",
        "continuous_live",
    }
    assert CustomModelConfig(provider="openai").interaction_kind is None


def test_committed_live_transport_record_remains_typed() -> None:
    variants = [
        variant
        for variant in get_args(WireLiveAdapterObservation)
        if get_args(get_type_hints(variant).get("observation"))
        == ("live_observation_committed",)
    ]
    assert len(variants) == 1
    assert get_type_hints(variants[0])["record"] is LiveObservationRecord
