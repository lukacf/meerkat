"""Tests for meerkat_meta.config.memory backend configs."""
from __future__ import annotations

from meerkat_meta.config import memory


class TestLocalJson:
    def test_to_dict_without_health_endpoint(self):
        config = memory.local_json()
        assert config.to_dict() == {"backend": "local_json"}

    def test_to_dict_with_health_endpoint(self):
        config = memory.local_json(health_check_endpoint="http://localhost:3000")
        assert config.to_dict() == {
            "backend": "local_json",
            "health_check_endpoint": "http://localhost:3000",
        }


def test_local_json_is_the_only_backend_helper():
    # The predecessor's backend helper and config class are removed with no
    # alias; the gateway refuses that backend value at init.
    defined = {
        name
        for name, value in vars(memory).items()
        if not name.startswith("_")
        and getattr(value, "__module__", None) == memory.__name__
    }
    assert defined == {"LocalJsonMemoryConfig", "local_json"}
