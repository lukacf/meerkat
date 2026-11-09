"""Narrow, single-assignment handles selected by a host before package import.

The package inspector owns compatibility. The host supplies names from that
validated selection and already scoped handles from its policy/lifecycle owners.
This class does not interpret manifests, authorize calls, or revoke handles.
"""

from __future__ import annotations

from collections.abc import Collection, Mapping
from threading import Lock
from types import MappingProxyType
from typing import Any


class HostServiceError(RuntimeError):
    """Stable failure kind for a staged host-service view."""

    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


class StagedHostServices:
    """Expose exactly the available subset of declared service handles.

    Unavailable optional services stay unavailable even if the host later gains
    the capability. A new selection needs a new view. Retained handles must
    enforce revocation themselves; the host owns their lifetime and authority.
    """

    def __init__(self, *, declared: Collection[str], available: Collection[str]) -> None:
        self._declared = self._names(declared)
        self._available = self._names(available)
        if not self._available <= self._declared:
            raise HostServiceError("available_service_not_declared")
        self._handles: Mapping[str, Any] | None = None
        self._bind_lock = Lock()

    @staticmethod
    def _names(values: Collection[str]) -> frozenset[str]:
        if isinstance(values, (str, bytes)) or any(
            not isinstance(value, str) or not value for value in values
        ):
            raise HostServiceError("invalid_service_names")
        return frozenset(values)

    @property
    def declared(self) -> frozenset[str]:
        return self._declared

    @property
    def available(self) -> frozenset[str]:
        return self._available

    @property
    def ready(self) -> bool:
        return self._handles is not None

    def bind(self, handles: Mapping[str, Any]) -> None:
        # Copy outside the lock: a host mapping may call other host code while
        # iterating. The final assignment still has exactly one winner.
        selected = dict(handles)
        if set(selected) != self._available or any(
            handle is None for handle in selected.values()
        ):
            raise HostServiceError("service_handles_differ_from_selection")
        with self._bind_lock:
            if self._handles is not None:
                raise HostServiceError("services_already_bound")
            self._handles = MappingProxyType(selected)

    def get(self, name: str) -> Any:
        if name not in self._declared:
            raise HostServiceError("service_not_declared")
        if name not in self._available:
            raise HostServiceError("service_unavailable")
        if self._handles is None:
            raise HostServiceError("services_not_bound")
        return self._handles[name]
