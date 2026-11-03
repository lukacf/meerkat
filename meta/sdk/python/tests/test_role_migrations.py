"""Tests for boot-scoped member role migration declarations.

A durable member whose role changed refuses to resume until the host declares
the migration. These tests pin the only two things the SDK owes that contract:
the declaration is REACHABLE from Python (it once was not, which made the
whole carrier unusable from the SDK), and it is ABSENT unless the
host asked for it.
"""
import json
from pathlib import Path

import pytest

from meerkat_meta import RoleMigrationDeclaration
from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime


def _init_params(builder):
    return MetaRuntime(builder._config)._build_init_params()


def test_declaration_reaches_the_init_params():
    builder = Meta.builder().role_migrations(
        [
            RoleMigrationDeclaration(
                identity="domain:automation", from_role="domain"
            )
        ]
    )
    params = _init_params(builder)
    assert params["role_migrations"] == [
        {"identity": "domain:automation", "from_role": "domain"}
    ]


def test_a_plain_dict_is_accepted_and_still_validated():
    builder = Meta.builder().role_migrations(
        [{"identity": "domain:automation", "from_role": "domain"}]
    )
    assert _init_params(builder)["role_migrations"] == [
        {"identity": "domain:automation", "from_role": "domain"}
    ]

    with pytest.raises(ValueError):
        Meta.builder().role_migrations([{"identity": "has a space", "from_role": "d"}])


def test_no_declaration_means_no_key_at_all():
    """Absent by default: a boot that declares nothing must arm nothing.

    The key must be absent rather than an empty list, so a gateway can never
    read an armed-but-empty declaration as "migrations were considered".
    """
    assert "role_migrations" not in _init_params(Meta.builder())
    assert "role_migrations" not in _init_params(Meta.builder().role_migrations([]))


# The one committed wire contract, shared with the Rust parser's
# `the_committed_wire_fixture_deserializes_into_declarations`. Renaming a key on
# either side goes red here or there, instead of both sides staying green while
# a host arms nothing.
FIXTURE = (
    Path(__file__).resolve().parents[3]
    / "crates"
    / "meerkat-meta"
    / "tests"
    / "fixtures"
    / "role_migrations_init_params.json"
)


def test_builder_output_matches_the_committed_wire_fixture():
    expected = json.loads(FIXTURE.read_text())["role_migrations"]
    builder = Meta.builder().role_migrations(
        [
            RoleMigrationDeclaration(
                identity=expected[0]["identity"], from_role=expected[0]["from_role"]
            )
        ]
    )
    assert _init_params(builder)["role_migrations"] == expected


def test_conflicting_declarations_are_refused_and_repeats_are_not():
    """A conflicting pair has no defensible resolution; a repeat is harmless.

    Refusing the identical repeat would be the failure mode of treating every
    irregularity as fatal.
    """
    with pytest.raises(ValueError, match="cannot be resolved by order"):
        Meta.builder().role_migrations(
            [
                {"identity": "domain:automation", "from_role": "domain"},
                {"identity": "domain:automation", "from_role": "other"},
            ]
        )

    repeated = Meta.builder().role_migrations(
        [
            {"identity": "domain:automation", "from_role": "domain"},
            {"identity": "domain:automation", "from_role": "domain"},
        ]
    )
    assert len(_init_params(repeated)["role_migrations"]) == 2
