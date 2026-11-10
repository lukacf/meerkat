"""SDK tuple for the separately selected protected application receiver.

The launcher verifies and installs its original application SDK wheel and
source closure before selecting a receiver. The receiver runs the workspace's
own Meta release, the one the Guide runs (guide_sdk), with the Core release of
the same version. These version constraints do not establish native
qualification, byte provenance or transport custody.
"""
import re

from . import guide_sdk

SELECTED_VERSION = guide_sdk.SELECTED_VERSION


def declared_core_version():
    """The Core version of the receiver's Meta release: the same workspace version."""
    if type(SELECTED_VERSION) is not str or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", SELECTED_VERSION) is None:
        raise ValueError("The receiver needs the workspace's exact Meta version.")
    return SELECTED_VERSION
