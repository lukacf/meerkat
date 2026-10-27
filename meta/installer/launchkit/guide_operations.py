"""Session-owned receipts keep a browser disconnect from losing native work."""
from __future__ import annotations

import asyncio
import json
import logging
import traceback
from datetime import datetime, timezone
from pathlib import Path

from .projects import DraftConflict, fingerprint
from .guide_models import GuidePublicationMissing


LOG = logging.getLogger(__name__)


def _log_failure(identity, operation, error):
    # Log source locations, never exception messages, locals, source excerpts
    # or request bodies. Detached failures need the same diagnostic as awaited ones.
    frames = [{"file": Path(frame.f_code.co_filename).name, "line": line,
               "function": frame.f_code.co_name}
              for frame, line in traceback.walk_tb(error.__traceback__)][-16:]
    LOG.error("Guide operation failed: %s", json.dumps({
        "guide_id": identity, "request_id": operation["request_id"],
        "version": operation["version"], "error_type": type(error).__name__,
        "frames": frames}, ensure_ascii=True))


class GuideOperations:
    def __init__(self, wait_seconds=0.05):
        self.wait_seconds = wait_seconds
        self.tasks = {}
        self.lock = asyncio.Lock()

    async def read(self, service, identity):
        async with self.lock:
            # Completion is saved after the response. Read the receipt first so
            # a terminal receipt can never be paired with an older guide state.
            operation = await asyncio.to_thread(service.store.operation, identity)
            state = await service.get(identity)
            if operation and operation["status"] == "running" and identity not in self.tasks:
                # The task could have completed after the receipt read. Refresh
                # both before reconciling a receipt left by a restarted host.
                operation = await asyncio.to_thread(service.store.operation, identity)
                state = await service.get(identity)
                if operation["status"] == "running":
                    if state["version"] > operation["version"]:
                        operation.update(status="completed", detail="Your completed response was recovered.")
                    else:
                        operation.update(status="interrupted", detail="The setup server restarted before this response was saved. Your submitted answer is preserved. Retry it when you are ready.")
                    await asyncio.to_thread(service.store.save_operation, identity, operation)
            if operation:
                state["operation"] = operation
            return state

    async def submit(self, service, identity, value):
        body = value.model_dump(mode="json")
        request_id = fingerprint({"guide_id": identity, **body})
        async with self.lock:
            previous = await asyncio.to_thread(service.store.operation, identity)
            saved = await service.get(identity)
            active = self.tasks.get(identity)
            if not active and previous and previous["status"] == "running":
                previous = await asyncio.to_thread(service.store.operation, identity)
                saved = await service.get(identity)
            if active:
                if not previous or previous["request_id"] != request_id:
                    raise DraftConflict("Your previous answer is still being considered. Reconnect to its response before submitting another.")
                task = active
            elif previous and previous["request_id"] == request_id and previous["status"] == "completed":
                return saved, False
            else:
                if saved["version"] != value.version:
                    raise DraftConflict("This setup changed. Reload the saved conversation before submitting.")
                operation = {"request_id": request_id, "version": value.version, "status": "running",
                             "started_at": datetime.now(timezone.utc).isoformat(), "request": body,
                             "detail": "Your guide is considering the answer and checking the design. You can leave and return here."}
                await asyncio.to_thread(service.store.save_operation, identity, operation)
                task = asyncio.create_task(self._run(service, identity, value, operation))
                self.tasks[identity] = task
                # Consume detached task exceptions; the receipt carries a safe error.
                task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        try:
            result = await asyncio.wait_for(asyncio.shield(task), timeout=self.wait_seconds)
            return result, False
        except asyncio.TimeoutError:
            return await self.read(service, identity), True

    async def _run(self, service, identity, value, operation):
        try:
            result = await service.respond(identity, value)
            operation.update(status="completed", detail="Your response is saved.")
            await asyncio.to_thread(service.store.save_operation, identity, operation)
            return result
        except asyncio.CancelledError:
            operation.update(status="interrupted", detail="The server stopped before this response was saved. Your submitted answer is preserved for retry.")
            await asyncio.to_thread(service.store.save_operation, identity, operation)
            raise
        except GuidePublicationMissing:
            operation.update(status="failed",
                             failure={"phase": "publication", "category": "publication_missing"},
                             detail="The guide answered, but did not save its response in the setup. Your last saved setup and submitted request are preserved. Retry when ready.")
            await asyncio.to_thread(service.store.save_operation, identity, operation)
            raise
        except Exception as error:
            _log_failure(identity, operation, error)
            operation.update(status="failed", detail="The guide could not finish this response. Your last saved setup and submitted answer are preserved. Retry when ready.")
            await asyncio.to_thread(service.store.save_operation, identity, operation)
            raise
        finally:
            self.tasks.pop(identity, None)

    async def shutdown(self):
        pending = list(self.tasks.values())
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
