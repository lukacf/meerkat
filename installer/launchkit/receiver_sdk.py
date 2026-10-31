"""SDK tuple for the separately selected protected application receiver.

The launcher verifies and installs its original application SDK wheel and
source closure before selecting a receiver. These version constraints do not
establish native qualification, byte provenance or transport custody. The
ordinary packaged Guide retains its independent guide_sdk selection.
"""
import re

SELECTED_VERSION = "0.9.7"
# Selected Meta v0.9.7 source df3f8a653117414a9399a69e0567413ce0dd3aaf,
# crates/meerkat-meta/Cargo.toml dependencies.meerkat-core.version.
CORE_REQUIREMENT = "=0.9.8"


def declared_core_version():
    if type(CORE_REQUIREMENT) is not str or re.fullmatch(r"=[0-9]+\.[0-9]+\.[0-9]+", CORE_REQUIREMENT) is None:
        raise ValueError("The receiver needs its selected Meta source's exact Core requirement.")
    return CORE_REQUIREMENT[1:]
