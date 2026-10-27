"""The Guide's one exact selected Meta release and its reviewed client wheel.

SELECTED_VERSION names the single Meta release and client wheel the Guide runs.
CORE_REQUIREMENT retains that Meta source's exact declared Core dependency.
Runtime admission and the product verifier check each side independently;
equal release numbers are not a compatibility rule.

Packaged products carry the published client wheel of that release, which
includes the public helper transport wait option. The pin below binds its
exact release commit, that commit's sdk/python tree and the published wheel's
SHA-256 and byte size. A selection whose release has not published yet keeps
these fields None, and every use of the pin refuses until all four are filled
together. The wheel confers no application SDK, native gateway or
persisted-state compatibility.
"""

import hashlib
import re

SELECTED_VERSION = "0.9.3"
WHEEL = f"vendor/guide/meerkat_meta-{SELECTED_VERSION}-py3-none-any.whl"
# Meta v0.9.3: its release commit, that commit's sdk/python tree, and the
# SHA-256 and byte size of meerkat_meta-0.9.3-py3-none-any.whl on PyPI.
SOURCE = "3b0073a834b93725c20cfa97bbd6db099348ece6"
# From SOURCE's crates/meerkat-meta/Cargo.toml, dependencies.meerkat-core.version.
# Update with the selected Meta source, never infer from its version number.
CORE_REQUIREMENT = "=0.9.3"
TREE = "d41781b733c506ed0355bd7aad4323dc77ebcda6"
SHA256 = "3e87446e560b94ab6d9d0b3dfac5868b2603db7739316e251aefb5e3b4b4d610"
SIZE = 166988


def declared_core_version():
    if type(CORE_REQUIREMENT) is not str or re.fullmatch(r"=[0-9]+\.[0-9]+\.[0-9]+", CORE_REQUIREMENT) is None:
        raise ValueError("The Guide needs its selected Meta source's exact Core requirement.")
    return CORE_REQUIREMENT[1:]


def require_pinned():
    declared_core_version()
    if not (type(SOURCE) is str and re.fullmatch("[0-9a-f]{40}", SOURCE)
            and type(TREE) is str and re.fullmatch("[0-9a-f]{40}", TREE)
            and type(SHA256) is str and re.fullmatch("[0-9a-f]{64}", SHA256)
            and type(SIZE) is int and SIZE > 0):
        raise ValueError(
            f"The Guide client SDK pin is pending the published Meta {SELECTED_VERSION} wheel.")


def selection():
    require_pinned()
    return {"path": WHEEL, "version": SELECTED_VERSION, "source": SOURCE,
        "tree": TREE, "sha256": SHA256, "bytes": SIZE}


def verify(content):
    require_pinned()
    if len(content) != SIZE or hashlib.sha256(content).hexdigest() != SHA256:
        raise ValueError("The Guide client SDK differs from its reviewed wheel.")
