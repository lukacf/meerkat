"""Host measurements for one build, separate from source/readiness authority."""
from __future__ import annotations

from contextlib import contextmanager
from datetime import datetime, timezone
from functools import wraps
import asyncio
import json
import math
from pathlib import Path
import threading
import time

from .templates.storage_setup import write_private


PHASES = {
    "preparation": "Preparing source",
    "gateway": "Preparing the native runtime",
    "storage": "Preparing native storage",
    "bootstrap": "Starting native agents",
    "builder_native": "Builder native run",
    "reviewer_native": "Reviewer native run",
    "native_admission": "Native request admission",
    "review_snapshot": "Freezing review source",
    "review_seal": "Verifying completed review",
    "final_checks": "Verifying the final package",
    "shutdown": "Stopping the build runtime",
    "tool": "Host tool callbacks",
}
TOOLS = {
    "inspect_implementation", "read_project_file", "read_acceptance_feedback",
    "write_project_file", "edit_project_file", "apply_project_batch", "check_implementation", "read_runtime_reference",
    "publish_implementation", "inspect_acceptance", "read_acceptance_source",
    "write_acceptance_test", "read_acceptance_test", "check_acceptance",
    "publish_acceptance",
}
IDENTITIES = {"implementation:main", "acceptance:main"}
MAX_SPANS = 1000


class ImplementationTiming:
    def __init__(self, directory, *, started_ns=None, clock=None):
        self.directory = Path(directory)
        self.path = self.directory / "timing.json"
        self.clock = clock or time.monotonic_ns
        self.lock = threading.RLock()
        self.disabled = False
        self.last_saved = 0
        self.state = None
        reset = False
        if started_ns is None and self.path.exists():
            reset = True
            try:
                if self.path.stat().st_size > 1_000_000:
                    raise ValueError("Timing metadata exceeds its bound.")
                saved = json.loads(self.path.read_text())
                if (isinstance(saved, dict) and saved.get("version") == 1
                        and saved.get("build_id") == self.directory.name
                        and type(saved.get("origin_ns")) is int
                        and 0 <= saved["origin_ns"] <= self.clock()
                        and isinstance(saved.get("spans"), list)
                        and len(saved["spans"]) <= MAX_SPANS
                        and isinstance(saved.get("totals"), dict)
                        and all(phase in PHASES and isinstance(row, dict)
                            and type(row.get("count")) is int and 0 <= row["count"] <= 1_000_000
                            and type(row.get("duration_ms")) in (int, float)
                            and math.isfinite(row["duration_ms"]) and 0 <= row["duration_ms"] <= 86_400_000
                            for phase, row in saved["totals"].items())
                        and type(saved.get("next_id")) is int and saved["next_id"] >= 1
                        and type(saved.get("dropped_spans")) is int and saved["dropped_spans"] >= 0):
                    self.state = saved
            except (OSError, ValueError):
                pass
        if self.state is None:
            self.state = {"version": 1, "build_id": self.directory.name,
                "origin_ns": self.clock() if started_ns is None else started_ns,
                "spans": [], "totals": {}, "next_id": 1, "dropped_spans": 0,
                "status": "running", "partial": reset}
        self.active = {}

    def _offset(self):
        return max(0, (self.clock() - self.state["origin_ns"]) / 1_000_000)

    def _save(self, *, force=True):
        tick = self.clock()
        if self.disabled or not force and tick - self.last_saved < 1_000_000_000:
            return
        self.state["elapsed_ms"] = round(self._offset(), 3)
        self.state["observed_at"] = datetime.now(timezone.utc).isoformat()
        self.state["active"] = [{**row, "elapsed_ms": round(self._offset() - row["start_ms"], 3)}
                                 for row in self.active.values()]
        try:
            write_private(self.path, self.state)
        except OSError:
            # Observation must never replace a native/tool result or exception.
            self.disabled = True
        self.last_saved = tick

    def preparation(self):
        """Called only after the service's final fences admit this build."""
        with self.lock:
            self._complete({"id": 0, "phase": "preparation", "start_ms": 0}, "completed")

    def _complete(self, row, status):
        row = {**row, "status": status, "duration_ms": round(self._offset() - row["start_ms"], 3)}
        self.state["spans"].append(row)
        if len(self.state["spans"]) > MAX_SPANS:
            del self.state["spans"][0]
            self.state["dropped_spans"] += 1
        total = self.state["totals"].setdefault(row["phase"], {"count": 0, "duration_ms": 0})
        total["count"] += 1
        total["duration_ms"] = round(total["duration_ms"] + row["duration_ms"], 3)
        self._save()

    @contextmanager
    def measure(self, phase, *, identity=None, interaction_id=None, tool=None):
        if phase not in PHASES or identity is not None and identity not in IDENTITIES or tool is not None and tool not in TOOLS:
            raise ValueError("Use a registered host timing phase, identity and tool.")
        with self.lock:
            row = {"id": self.state["next_id"], "phase": phase, "start_ms": self._offset()}
            self.state["next_id"] += 1
            if identity:
                row["identity"] = identity
            if interaction_id:
                row["interaction_id"] = interaction_id
            if tool:
                row["tool"] = tool
                native = next((item for item in self.active.values()
                    if item["phase"] in {"builder_native", "reviewer_native"}
                    and item.get("identity") == identity), None)
                if native and native.get("interaction_id"):
                    row["interaction_id"] = native["interaction_id"]
            self.active[row["id"]] = row
            self._save()
        status = "completed"
        try:
            yield row
        except BaseException as error:
            status = "interrupted" if isinstance(error, asyncio.CancelledError) else "failed"
            raise
        finally:
            with self.lock:
                self.active.pop(row["id"], None)
                self._complete(row, status)

    def admitted(self, identity, interaction_id):
        with self.lock:
            for row in self.active.values():
                if row["phase"] in {"builder_native", "reviewer_native", "native_admission"} and row.get("identity") == identity:
                    row["interaction_id"] = interaction_id
            self._save()

    def observe(self):
        with self.lock:
            self._save(force=False)

    def finish(self, status):
        with self.lock:
            self.state["status"] = status if status in {"completed", "failed", "interrupted"} else "interrupted"
            self._save()


class TimedDraft:
    """Measure registered callbacks without changing schemas, arguments or results."""
    def __init__(self, draft, timing, identity):
        self.draft, self.timing, self.identity = draft, timing, identity

    def register_tool(self, name, handler, **options):
        if name not in TOOLS:
            return self.draft.register_tool(name, handler, **options)

        @wraps(handler)
        async def measured(args):
            with self.timing.measure("tool", identity=self.identity, tool=name):
                return await handler(args)
        return self.draft.register_tool(name, measured, **options)

    def __getattr__(self, name):
        return getattr(self.draft, name)


def timing_projection(directory, *, terminal=False):
    """Return bounded metrics only. Old builds have no invented measurements."""
    directory = Path(directory)
    path = directory / "timing.json"
    if not path.exists():
        return None
    try:
        if path.stat().st_size > 1_000_000:
            return None
        state = json.loads(path.read_text())
        if state.get("version") != 1 or state.get("build_id") != directory.name:
            return None
        elapsed = state.get("elapsed_ms")
        if type(elapsed) not in (int, float) or not 0 <= elapsed <= 86_400_000:
            return None
        totals = []
        for phase, row in state.get("totals", {}).items():
            if phase not in PHASES or not isinstance(row, dict):
                continue
            duration, count = row.get("duration_ms"), row.get("count")
            if type(duration) in (int, float) and 0 <= duration <= 86_400_000 and type(count) is int and 0 < count <= 1_000_000:
                totals.append({"phase": phase, "label": PHASES[phase], "duration_ms": duration, "count": count})
        observed_at = state.get("observed_at")
        if not isinstance(observed_at, str) or len(observed_at) > 40:
            return None
        datetime.fromisoformat(observed_at)
        complete = state.get("status") in {"completed", "failed", "interrupted"}
        return {"version": 1, "build_id": directory.name, "elapsed_ms": elapsed,
            "observed_at": observed_at, "finished": complete,
            "partial": bool(state.get("partial") is True or terminal and not complete), "phases": totals,
            "overlap": True, "scope": "host_build",
            "unmeasured": ["provider_inference", "provider_queue", "network", "cache", "browser_delivery"]}
    except (OSError, ValueError, TypeError, AttributeError):
        return None
