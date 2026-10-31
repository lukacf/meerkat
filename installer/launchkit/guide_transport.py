"""One trusted, process-local transport selection for a Forge runtime.

The original launcher supplies the actual SDK factory. Forge neither creates a
channel nor transfers its custody. The SDK invokes the factory during build,
installs callbacks and owns shutdown while the original event loop stays alive.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import re
import threading


_LEGACY_WORKER_STATE = object()


@dataclass(frozen=True)
class SelectedGuideTransport:
    """Trusted launcher input, never a browser, environment or saved record.

    One selection binds one builder in the receiver where it was constructed.
    A second runtime needs another actual owner selection; this object cannot
    allocate it or silently substitute a subprocess gateway.
    """

    factory: object = field(repr=False)
    version: str
    core_version: str
    meta_source: str
    core_source: str
    _model_route: dict = field(repr=False)
    persistent_state: object = field(default=_LEGACY_WORKER_STATE, repr=False)
    _pid: int = field(default_factory=os.getpid, init=False, repr=False)
    _original_factory: object = field(default=None, init=False, repr=False, compare=False)
    _route_bytes: bytes = field(default=b"", init=False, repr=False)
    _binding_lock: object = field(default_factory=threading.Lock, init=False, repr=False, compare=False)
    _bound: bool = field(default=False, init=False, repr=False, compare=False)
    _original_state: object = field(default=None, init=False, repr=False, compare=False)

    def __post_init__(self):
        from . import guide_sdk
        from .bootstrap import GatewayInstallError

        if (not callable(self.factory) or self.version != guide_sdk.SELECTED_VERSION
                or self.core_version != guide_sdk.declared_core_version()
                or any(type(value) is not str or re.fullmatch("[0-9a-f]{40}", value) is None
                       for value in (self.meta_source, self.core_source))):
            raise GatewayInstallError("The guide needs its selected SDK and original transport factory.")
        route = deepcopy(self._model_route)
        if (type(route) is not dict or set(route) != {"provider", "model"}
                or any(type(value) is not str or not value or len(value) > 4096
                       or "\x00" in value for value in route.values())):
            raise GatewayInstallError("The restricted guide needs its public provider and model route.")
        state = self.persistent_state
        if state is not _LEGACY_WORKER_STATE and (type(state) is not str or len(state) > 4096
                or not Path(state).is_absolute() or state == "/"
                or str(Path(state)) != state
                or any(part in ("", ".", "..") for part in state.split("/")[1:])
                or any(ord(char) < 32 for char in state)):
            raise GatewayInstallError("Explicit worker state must be its selected canonical absolute path.")
        object.__setattr__(self, "_model_route", route)
        object.__setattr__(self, "_route_bytes", json.dumps(route, sort_keys=True,
            ensure_ascii=True, allow_nan=False).encode())
        object.__setattr__(self, "_original_factory", self.factory)
        object.__setattr__(self, "_original_state", state)

    def recheck(self, *, verify_gateway=False):
        from .bootstrap import GatewayInstallError

        # Check PID before touching a possibly inherited, locked mutex.
        if os.getpid() != self._pid or self.factory is not self._original_factory:
            raise GatewayInstallError("The original guide transport cannot cross receiver processes.")
        if self.persistent_state != self._original_state:
            raise GatewayInstallError("The original worker persistent-state selection changed.")
        if json.dumps(self._model_route, sort_keys=True,
                ensure_ascii=True, allow_nan=False).encode() != self._route_bytes:
            raise GatewayInstallError("The selected guide model route changed.")

    @property
    def model_configuration(self):
        self.recheck()
        return deepcopy(self._model_route)

    def worker_state_path(self, legacy_path):
        """Choose state before the worker calls the SDK's public state setter.

        The root owns the selected path and its backing state. Looking at a
        legacy path here only refuses conflicting saved state; it never creates,
        copies, migrates or adopts either store.
        """
        from .bootstrap import GatewayInstallError

        self.recheck()
        legacy = str(legacy_path)
        if self.persistent_state is _LEGACY_WORKER_STATE:
            return legacy
        if legacy != self.persistent_state:
            try:
                Path(legacy).lstat()
            except FileNotFoundError:
                pass
            else:
                raise GatewayInstallError("Existing worker state conflicts with the protected selection; saved state is preserved.")
        return self.persistent_state

    def configure_builder(self, builder):
        from .bootstrap import GatewayInstallError

        self.recheck()
        with self._binding_lock:
            self.recheck()
            if self._bound:
                raise GatewayInstallError("This guide transport already belongs to one builder.")
            # Retain this choice even if SDK configuration fails. No retry on
            # another builder, factory invocation here, or local spawn fallback.
            object.__setattr__(self, "_bound", True)
            builder.gateway_transport(self.factory)
        return builder

    def __reduce_ex__(self, protocol):
        raise TypeError("An original guide transport cannot be serialized for another worker.")
