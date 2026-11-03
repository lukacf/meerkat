"""One replacement-process startup under the existing application transaction.

The source owner supplies the immutable selected update, prior accepted desired
policy and output custody under its lease. Native init owns spec declaration,
resume, new member construction and settlement. No effect is inferred absent
from an error and this runtime never retries a replacement automatically.
"""
from __future__ import annotations

import base64
import hashlib
import os
import stat
from typing import Any

from .host_control import ApplicationUpdateStartup, HostControlError, ToolkitMobKitRuntime, ToolkitPersistentTransport


class ToolkitApplicationUpdateRuntime(ToolkitMobKitRuntime):
    """Consume a trusted replacement selection in the same real native child.

    Use the application's original persistent storage and native expected
    revision option. The configured callbacks must read this exact candidate
    source under the same transaction lease. Existing helper resources and
    authority selections are restored independently of the candidate policy.
    """

    def __init__(self, config: Any, startup: ApplicationUpdateStartup):
        if not isinstance(startup, ApplicationUpdateStartup):
            raise HostControlError("application update startup is required")
        startup.__post_init__()
        descriptor = os.open(startup.selection.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise HostControlError("application update selection must be a regular file")
            with os.fdopen(descriptor, "rb", closefd=False) as source:
                selected = source.read(16 * 1024 * 1024 + 1)
            if len(selected) > 16 * 1024 * 1024 or hashlib.sha256(selected).hexdigest() != startup.selection.sha256:
                raise HostControlError("application update selection changed or exceeds its limit")
        finally:
            os.close(descriptor)
        super().__init__(config, startup)
        self._update_startup = startup
        self._update_bytes = selected
        self._update_started = False

    def _make_gateway_transport(self) -> ToolkitPersistentTransport:
        if self._update_started:
            raise HostControlError("application update already started; inspect the original transaction")
        if self._config.gateway_bin is None:
            raise HostControlError("application update requires its local gateway")
        self._update_started = True
        return ToolkitPersistentTransport(self._config.gateway_bin, self._update_startup)

    def _build_init_params(self) -> dict[str, Any]:
        params = super()._build_init_params()
        if any(key in params for key in ("mob_config", "application_tool_policies", "prepared_application")):
            raise HostControlError("application update conflicts with a second definition or policy")
        params["prepared_application"] = {
            "bundle_base64": base64.b64encode(self._update_bytes).decode("ascii"),
            "digest": self._update_startup.selection.sha256,
        }
        return params
