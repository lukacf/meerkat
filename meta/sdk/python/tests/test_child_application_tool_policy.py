"""Tests for the child application tool policy declaration.

Once compiled application tool policies are served, Meerkat refuses a
member's ``mob_create`` and ``delegate`` until the host chooses the policy
child mob members run under. A downstream app composes through this SDK only, so the
key must be reachable from the builder and absent unless the host chose one.
"""

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime


def _init_params(builder):
    return MetaRuntime(builder._config)._build_init_params()


PROVIDER_BINDING = {
    "kind": "provider",
    "provider_id": "example",
    "policy_id": "team-tools",
}


def test_a_provider_binding_reaches_the_init_params():
    builder = Meta.builder().child_application_tool_policy(PROVIDER_BINDING)
    assert _init_params(builder)["child_application_tool_policy"] == PROVIDER_BINDING


def test_the_explicit_unmanaged_opt_out_reaches_the_init_params():
    builder = Meta.builder().child_application_tool_policy({"kind": "unmanaged"})
    assert _init_params(builder)["child_application_tool_policy"] == {
        "kind": "unmanaged"
    }


def test_the_parameter_is_absent_unless_the_host_chose_one():
    assert "child_application_tool_policy" not in _init_params(Meta.builder())


def test_the_builder_keeps_its_own_copy():
    binding = dict(PROVIDER_BINDING)
    builder = Meta.builder().child_application_tool_policy(binding)
    binding["policy_id"] = "guest-tools"
    assert (
        _init_params(builder)["child_application_tool_policy"]["policy_id"]
        == "team-tools"
    )


@pytest.mark.parametrize(
    ("binding", "message"),
    [
        ({"kind": "inherit"}, "inherit is not valid for child mobs"),
        ({}, "kind must be 'provider' or 'unmanaged'"),
        ({"kind": "unmanaged", "provider_id": "example"}, "takes no other keys"),
        ({"kind": "provider", "provider_id": "example"}, "needs exactly"),
        (
            {"kind": "provider", "provider_id": "example", "policy_id": " "},
            "policy_id must be a non-empty string",
        ),
        (
            {"kind": "provider", "provider_id": 7, "policy_id": "team-tools"},
            "provider_id must be a non-empty string",
        ),
    ],
)
def test_a_binding_meerkat_cannot_use_is_refused(binding, message):
    with pytest.raises(ValueError, match=message):
        Meta.builder().child_application_tool_policy(binding)


def test_a_non_mapping_is_refused():
    with pytest.raises(TypeError, match="must be a mapping"):
        Meta.builder().child_application_tool_policy("unmanaged")
