"""Original protected HTTP startup metadata, without a gateway or factory.

Only the trusted fixed launcher selects this tuple. It identifies the public
model route and SDK/source versions; it grants no native lifetime, state,
credentials or transport. Each agent still needs its own original admission.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass, field
import json
import os
import re


@dataclass(frozen=True)
class SelectedProtectedRuntime:
    version: str
    core_version: str
    meta_source: str
    core_source: str
    _model_route: dict = field(repr=False)
    _pid: int = field(default_factory=os.getpid, init=False, repr=False)
    _selection: tuple = field(default=(), init=False, repr=False)
    _route_bytes: bytes = field(default=b"", init=False, repr=False)

    def __post_init__(self):
        from . import guide_sdk
        from .bootstrap import GatewayInstallError

        selection = (self.version, self.core_version, self.meta_source, self.core_source)
        if (any(type(value) is not str for value in selection)
                or self.version != guide_sdk.SELECTED_VERSION
                or self.core_version != guide_sdk.declared_core_version()
                or any(re.fullmatch("[0-9a-f]{40}", value) is None
                       for value in (self.meta_source, self.core_source))):
            raise GatewayInstallError("Protected startup needs its exact selected SDK and source tuple.")
        route = deepcopy(self._model_route)
        if (type(route) is not dict or set(route) != {"provider", "model"}
                or any(type(value) is not str or not value or len(value) > 4096
                       or "\x00" in value for value in route.values())):
            raise GatewayInstallError("Protected startup accepts only its public provider and model route.")
        object.__setattr__(self, "_selection", selection)
        object.__setattr__(self, "_model_route", route)
        object.__setattr__(self, "_route_bytes", json.dumps(route, sort_keys=True,
            ensure_ascii=True, allow_nan=False).encode())

    def recheck(self, *, verify_gateway=False):
        from .bootstrap import GatewayInstallError

        if os.getpid() != self._pid:
            raise GatewayInstallError("Protected startup belongs to its original receiver process.")
        if ((self.version, self.core_version, self.meta_source, self.core_source) != self._selection
                or json.dumps(self._model_route, sort_keys=True,
                    ensure_ascii=True, allow_nan=False).encode() != self._route_bytes):
            raise GatewayInstallError("The original protected startup tuple changed.")
        if verify_gateway:
            raise GatewayInstallError("Protected startup needs per-role admission before a gateway exists.")

    @property
    def model_configuration(self):
        self.recheck()
        return deepcopy(self._model_route)

    def __reduce_ex__(self, protocol):
        raise TypeError("Protected startup must be selected by its original trusted launcher.")
