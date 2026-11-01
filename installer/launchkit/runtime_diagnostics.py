"""Opt-in process diagnostics, independent of HTTP and native model execution."""
from __future__ import annotations

import asyncio
from contextlib import contextmanager
import faulthandler
import os
import sys
import time


class RuntimeDiagnostics:
    """Keep one C-level timeout armed while the application loop makes progress.

    faulthandler prints Python thread stacks without locals or request content.
    Its timer still works when Python cannot schedule another task or thread.
    This opt-in owner uses the process-wide delayed traceback timer; it does
    not install signals, kill a process, retry work, or change native receipts.
    """

    def __init__(self, *, timeout=30.0, pulse_interval=5.0,
                 report_interval=60.0, max_dumps=3, file=None):
        if not 0 < pulse_interval < timeout or report_interval <= 0 or max_dumps < 1:
            raise ValueError("Invalid runtime diagnostic intervals.")
        self.loop = asyncio.get_running_loop()
        self.file = sys.stderr if file is None else file
        self.timeout = timeout
        self.pulse_interval = pulse_interval
        self.report_interval = report_interval
        self.remaining = max_dumps
        self.last_pulse = self.last_report = time.monotonic()
        self.handle = None
        self.closed = False

    def _marker(self, event, *, lag=None):
        # Fixed vocabulary and numeric process facts only. Never accept request
        # paths, exception messages, configuration, arguments or user content.
        message = f"[forge-runtime-watchdog] {event} pid={os.getpid()}"
        if lag is not None:
            message += f" loop_gap_seconds={lag:.3f}"
        try:
            os.write(self.file.fileno(), (message + "\n").encode("ascii"))
        except (OSError, ValueError):
            pass

    def start(self):
        self._arm()
        self._marker("started")
        self.handle = self.loop.call_later(self.pulse_interval, self._pulse)
        return self

    def _arm(self):
        if self.remaining:
            # Non-repeating: at most one stack dump per uninterrupted stall.
            faulthandler.dump_traceback_later(self.timeout, repeat=False,
                                            file=self.file, exit=False)

    def _pulse(self):
        if self.closed:
            return
        now = time.monotonic()
        lag = now - self.last_pulse
        if lag >= self.timeout:
            self.remaining = max(0, self.remaining - 1)
            self._marker("loop_resumed", lag=lag)
        self.last_pulse = now
        try:
            self._arm()
        except (OSError, ValueError, RuntimeError):
            self._marker("timer_unavailable")
            self.close()
            return
        if now - self.last_report >= self.report_interval:
            self._marker("loop_progress", lag=lag)
            self.last_report = now
        self.handle = self.loop.call_later(self.pulse_interval, self._pulse)

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.handle is not None:
            self.handle.cancel()
        faulthandler.cancel_dump_traceback_later()
        self._marker("stopped")


@contextmanager
def runtime_diagnostics():
    """The host opts in at process startup; browser input cannot enable it."""
    diagnostics = None
    if os.environ.get("META_FORGE_RUNTIME_DIAGNOSTICS") == "1":
        diagnostics = RuntimeDiagnostics()
        try:
            diagnostics.start()
        except (OSError, ValueError, RuntimeError):
            diagnostics._marker("timer_unavailable")
            diagnostics.close()
            diagnostics = None
    try:
        yield diagnostics
    finally:
        if diagnostics is not None:
            diagnostics.close()
