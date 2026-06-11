"""Generated contract-version compatibility helper for the Meerkat SDK.

Source: meerkat-contracts ContractVersion::is_compatible_with (version.rs)

Compatibility: same major version (for 1.0+), or same major+minor (for 0.x). Mirrors `meerkat_contracts::ContractVersion::is_compatible_with`.
"""

from __future__ import annotations


def _core_parts(version: str) -> list[int]:
    core = version.split("-", 1)[0].split("+", 1)[0]
    return [int(part) for part in core.split(".")]


def is_compatible_with(server: str, client: str) -> bool:
    """True iff a server contract version is compatible with a client version.

    Canonical rule: 0.x requires same major+minor; >=1.0 requires same major.
    """
    try:
        s_parts = _core_parts(server)
        c_parts = _core_parts(client)
    except (ValueError, IndexError):
        return False
    if len(s_parts) < 2 or len(c_parts) < 2:
        return False
    if s_parts[0] == 0 and c_parts[0] == 0:
        return s_parts[1] == c_parts[1]
    return s_parts[0] == c_parts[0]
