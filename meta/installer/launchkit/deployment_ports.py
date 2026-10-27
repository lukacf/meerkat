"""Immediate loopback bind observations, without reserving or contacting a service."""
from __future__ import annotations

from datetime import datetime, timezone
import errno
import os
import socket


def checked_console_port(value):
    if type(value) is not int or not 1024 <= value <= 65535:
        raise ValueError("Choose a Console port from 1024 to 65535.")
    return value


def observe_local_console_port(port, *, suggest_alternate=False):
    port = checked_console_port(port)
    status = "unknown"
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            # Match Unix Console listeners when a stopped connection is in TIME_WAIT.
            if os.name == "posix":
                probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                probe.bind(("127.0.0.1", port))
            except OSError as error:
                if error.errno == errno.EADDRINUSE:
                    status = "occupied"
            else:
                status = "available"
    except OSError:
        status = "unknown"
    detail = {
        "available": "This port was available when checked. Availability can change before startup.",
        "occupied": "This port is unavailable on this computer. Choose another port or check again before starting.",
        "unknown": "Port availability could not be checked. Check again before starting this installation.",
    }[status]
    observation = {"scope": "local-console-port-observation", "port": port, "status": status,
        "observed_at": datetime.now(timezone.utc).isoformat(), "detail": detail}
    if status == "occupied" and suggest_alternate:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
                if os.name == "posix":
                    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                probe.bind(("127.0.0.1", 0))
                alternate = checked_console_port(probe.getsockname()[1])
            if alternate != port:
                observation["alternate_port"] = alternate
        except (OSError, ValueError):
            pass
    return observation


class LocalConsolePortUnavailable(ValueError):
    def __init__(self, observation):
        self.observation = observation
        message = (f"Console port {observation['port']} is unavailable on this computer. "
                   "Choose another port in private setup or check again.") if observation["status"] == "occupied" else (
                   "Console port availability could not be checked. Check again before starting the installation.")
        super().__init__(message)


def require_available_console_port(port):
    observation = observe_local_console_port(port)
    if observation["status"] != "available":
        raise LocalConsolePortUnavailable(observation)
    return observation


def installed_console_port(destination, default_port):
    # Reuse the existing bounded, owned private read. Never project its values.
    from .deployment import private_configuration_snapshot
    values, _ = private_configuration_snapshot(destination)
    value = values.get("MOBKIT_PORT", str(default_port))
    if not isinstance(value, str) or not value.isascii() or not value.isdecimal():
        raise ValueError("Choose a Console port from 1024 to 65535 in private setup.")
    return checked_console_port(int(value))
