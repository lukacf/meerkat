"""Private child state for tests that launch a real rpc_gateway."""
from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from meerkat_meta._transport import PersistentTransport


def gateway_environment(directory: str | Path) -> dict[str, str]:
    """Keep gateway identities in the test directory across child restarts."""
    directory = Path(directory)
    state_home = directory / "state-home"
    state_home.mkdir(parents=True, exist_ok=True)
    # Linux resolves comms identities from XDG_STATE_HOME. Keep the host's
    # HOME and the rest of the inherited environment unchanged.
    return {"XDG_STATE_HOME": str(state_home)}


def gateway_transport_factory(
    gateway_bin: str, directory: str | Path,
) -> Callable[[], PersistentTransport]:
    environment = gateway_environment(directory)
    return lambda: PersistentTransport(gateway_bin, env=environment)
