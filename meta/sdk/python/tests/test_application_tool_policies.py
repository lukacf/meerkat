"""Tests for compiled application tool policy declarations.

These pin the same two things the SDK owed the role-migration carrier, for the
same reason: the declaration must be REACHABLE from Python, and ABSENT unless
the host asked for it.

This is the second time that first claim has needed a test. The role-migration
carrier shipped with a gateway that parsed the parameter and a Python SDK that
could not send it, and test_role_migrations.py exists because of it. The
tool-policy lowering then shipped exactly the same way. A host that composes
through this SDK only cannot use a parameter the SDK cannot send.
"""

import json
from pathlib import Path

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime


def _init_params(builder):
    return MetaRuntime(builder._config)._build_init_params()


# ONE wire fixture, shared with the Rust side. member_tool_policy's
# `the_committed_wire_fixture_installs_its_carried_provider` regenerates it and
# proves those exact bytes install and bind; this file proves the builder emits
# them. Renaming the key on either side goes red here or there, instead of both
# sides staying green while a host arms nothing.
#
# The digest inside is computed from the canonical bytes by the Rust generator,
# so this cannot be hand-edited into validity.
FIXTURE = (
    Path(__file__).resolve().parents[3]
    / "crates"
    / "meerkat-meta"
    / "tests"
    / "fixtures"
    / "application_tool_policies_init_params.json"
)
POLICY_PRIMARY = json.loads(FIXTURE.read_text())["application_tool_policies"][0]
# A second, deliberately NOT digest-valid, for order and refusal checks only:
# the SDK is a carrier and never validates policy contents.
POLICY_GUEST = POLICY_PRIMARY.replace("team-tools", "guest-tools")


def test_a_declared_policy_reaches_the_init_params():
    builder = Meta.builder().application_tool_policies([POLICY_PRIMARY])
    assert _init_params(builder)["application_tool_policies"] == [POLICY_PRIMARY]


def test_the_compilers_bytes_travel_verbatim():
    # Parsing verifies the digest against the bytes it is handed, so anything
    # that reformats here would make that check meaningless. Trailing newline
    # and key order must survive exactly.
    builder = Meta.builder().application_tool_policies([POLICY_PRIMARY])
    sent = _init_params(builder)["application_tool_policies"][0]
    assert sent == POLICY_PRIMARY
    assert sent.endswith("\n")


def test_bytes_are_accepted_and_decoded_as_utf8():
    builder = Meta.builder().application_tool_policies([POLICY_PRIMARY.encode()])
    assert _init_params(builder)["application_tool_policies"] == [POLICY_PRIMARY]


def test_several_policies_keep_their_order():
    builder = Meta.builder().application_tool_policies(
        [POLICY_PRIMARY, POLICY_GUEST]
    )
    assert _init_params(builder)["application_tool_policies"] == [
        POLICY_PRIMARY,
        POLICY_GUEST,
    ]


def test_the_parameter_is_absent_unless_the_host_asked():
    assert "application_tool_policies" not in _init_params(Meta.builder())


def test_an_empty_entry_is_refused_rather_than_forwarded():
    # Forwarding a blank entry surfaces at boot as a parse error with nothing
    # pointing at which entry was blank.
    with pytest.raises(ValueError, match=r"application_tool_policies\[1\] is empty"):
        Meta.builder().application_tool_policies([POLICY_PRIMARY, "   "])


def test_a_non_string_entry_is_refused_with_its_index():
    with pytest.raises(TypeError, match=r"application_tool_policies\[0\] must be str"):
        Meta.builder().application_tool_policies([{"provider_id": "example-provider"}])


def test_invalid_utf8_bytes_are_refused_with_their_index():
    with pytest.raises(ValueError, match=r"application_tool_policies\[0\] is not valid UTF-8"):
        Meta.builder().application_tool_policies([b"\xff\xfe not json"])


def test_the_builder_emits_exactly_the_committed_wire_payload():
    expected = json.loads(FIXTURE.read_text())["application_tool_policies"]
    builder = Meta.builder().application_tool_policies(expected)
    assert _init_params(builder)["application_tool_policies"] == expected
