"""One trusted Forge login's native provider setup interaction.

The launcher selects matching Connect bytes, realm roots and credential scope.
This owner projects native prompts, never creates a provider catalog or changes
an active guide generation. Its final selection stays private to the launcher.
"""
from __future__ import annotations

import asyncio
from copy import deepcopy
import inspect
import time

from .connect_installer_caller import ExactFile
from .templates_v2.console_auth import ConsoleConnectLogin


class ProviderSetupUnavailable(RuntimeError):
    def __init__(self):
        super().__init__("Native provider setup is unavailable for this login.")


def _retain(task):
    task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
    return task


class ForgeProviderSetupSession:
    """Retain the same native child through observer cancellation and logout.

    Register revoke()/close() with the actual authenticated login lifecycle
    before begin(). Request fields must never construct this owner or binding.
    A selected result is usable only through selection(), after actual SDK join.
    The launcher still has to compose the identical realm/config/credential scope
    into a fresh guide generation. This owner does not activate that generation.
    """
    def __init__(self, executable, binding, login, *, environment=None):
        from meerkat_toolkit.provider_setup import ProviderSetupBinding

        if (type(executable) is not ExactFile or type(binding) is not ProviderSetupBinding
                or type(login) is not ConsoleConnectLogin or not callable(login.is_current)
                or binding.principal_id != login.principal
                or binding.login_scope_id != login.login_scope_id
                or binding.expires_at_ms != login.expires_at_ms):
            raise ProviderSetupUnavailable()
        binding.data()  # Native SDK validates trusted fields before any child.
        self._executable = executable
        self._binding = deepcopy(binding)
        self._login = login
        self._environment = deepcopy(environment)
        self._loop = None
        self._opening = self._reader = self._closing = self._joining = None
        self._client = None
        self._answers = set()
        self._closed = False
        self._joined = False
        self._cleanup_failed = False
        self._state = "not_started"
        self._interaction = self._prompt_id = self._prompt = self._selection = None
        self._notice = None

    def __repr__(self):
        return "ForgeProviderSetupSession(<host-private>)"

    @property
    def retired(self):
        return self._closed and (self._opening is None or self._opening.done()) and (
            self._client is None or self._joined) and (self._reader is None or self._reader.done()) and all(
                task.done() for task in self._answers)

    def _current(self, context):
        try:
            current = self._login.is_current()
            if inspect.isawaitable(current):
                if inspect.iscoroutine(current):
                    current.close()
                return False
            return (not self._closed and context is self._login and current is True
                    and time.time() * 1000 < self._login.expires_at_ms
                    and (self._loop is None or self._loop is asyncio.get_running_loop()))
        except Exception:
            return False

    def _admit(self, context):
        if not self._current(context):
            raise ProviderSetupUnavailable()

    async def begin(self, context):
        self._admit(context)
        if self._loop is None:
            self._loop = asyncio.get_running_loop()
        if self._opening is None:
            self._state = "starting"
            self._opening = _retain(asyncio.create_task(self._open()))
        try:
            await asyncio.shield(self._opening)
        except asyncio.CancelledError:
            raise
        except Exception:
            raise ProviderSetupUnavailable() from None
        self._admit(context)
        return self.snapshot(context)

    async def _open(self):
        from meerkat_toolkit.provider_setup import ProviderSetupClient

        try:
            await asyncio.to_thread(self._executable.verify, executable=True)
            self._admit(self._login)
            # This task is retained before launch. A cancelled HTTP observer
            # cannot discard a late returned Popen owner or spawn a replacement.
            self._client = await asyncio.to_thread(ProviderSetupClient,
                str(self._executable.path), self._binding, environment=self._environment)
            if self._current(self._login):
                self._state = "waiting"
                self._reader = _retain(asyncio.create_task(self._read()))
            else:
                self.revoke()
        except Exception:
            self._state = "failed"
            raise ProviderSetupUnavailable() from None

    async def _join_child(self):
        from meerkat_toolkit.provider_setup import ProviderSetupError

        if self._client is None:
            return
        while True:
            try:
                await asyncio.to_thread(self._client.close, 1.0)
                self._joined = True
                return
            except TimeoutError:
                # Same SDK reader and Popen. Timeout never means joined.
                continue
            except ProviderSetupError:
                # This SDK reports close failure only after direct-child reap.
                self._joined = True
                self._cleanup_failed = True
                return

    def _join(self):
        if self._joining is None:
            self._joining = _retain(asyncio.create_task(self._join_child()))
        return self._joining

    async def _read(self):
        from meerkat_toolkit.provider_setup import ProviderSelection

        try:
            while self._current(self._login):
                try:
                    event = await asyncio.to_thread(self._client.next_event, 1.0)
                except TimeoutError:
                    continue
                if not self._current(self._login):
                    break
                kind = event["kind"]
                if kind == "initialized":
                    self._interaction = event["interaction_id"]
                elif kind == "prompt":
                    self._prompt_id, self._prompt = event["prompt_id"], deepcopy(event["prompt"])
                    self._notice = None
                    self._state = "prompt"
                elif kind in ("answered", "prompt_closed"):
                    if event.get("prompt_id") == self._prompt_id:
                        self._prompt_id = self._prompt = None
                    self._state = "waiting"
                elif kind == "notice":
                    self._notice = deepcopy(event.get("notice"))
                elif kind == "cancelling":
                    self._state = "closing"
                elif kind in ("selected", "cancelled", "failed"):
                    selected = ProviderSelection.decode(event["selection"]) if kind == "selected" else None
                    self._prompt_id = self._prompt = self._notice = None
                    self._state = "joining"
                    await asyncio.shield(self._join())
                    if self._current(self._login):
                        self._state = "failed" if self._cleanup_failed else kind
                        self._selection = selected if self._state == "selected" else None
                    return
            self.revoke()
        except Exception:
            self._prompt_id = self._prompt = self._notice = None
            self._selection = None
            self._state = "failed"
            await asyncio.shield(self._join())

    def snapshot(self, context):
        self._admit(context)
        value = {"status": self._state, "joined": self._joined, "live_entitlement": "unobserved"}
        if self._prompt is not None:
            value.update(prompt_id=self._prompt_id, prompt=deepcopy(self._prompt))
        if self._notice is not None:
            value["notice"] = deepcopy(self._notice)
        if self._selection is not None:
            value["selection"] = {"provider": self._selection.provider, "model": self._selection.model,
                "credential": self._selection.credential, "live_entitlement": "unobserved"}
        return value

    async def answer(self, context, prompt_id, answer):
        self._admit(context)
        if (self._client is None or self._state != "prompt" or type(prompt_id) is not str
                or prompt_id != self._prompt_id or type(answer) is not dict):
            raise ProviderSetupUnavailable()
        # Preserve the offered identity, reserve once, and never retry a secret
        # or callback answer after an ambiguous transport observation.
        self._state = "answer_pending"
        self._prompt_id = self._prompt = self._notice = None
        task = _retain(asyncio.create_task(self._send_answer(prompt_id, deepcopy(answer))))
        self._answers.add(task)
        try:
            await asyncio.shield(task)
        except asyncio.CancelledError:
            raise
        except Exception:
            self.revoke()
            raise ProviderSetupUnavailable() from None
        finally:
            task.add_done_callback(self._answers.discard)
        self._admit(context)
        return self.snapshot(context)

    async def _send_answer(self, prompt_id, answer):
        try:
            self._admit(self._login)
            await asyncio.to_thread(self._client.answer, prompt_id, answer)
        except Exception:
            self.revoke()
            raise ProviderSetupUnavailable() from None

    def selection(self, context):
        """Trusted launcher only; no serialized browser result can enter here."""
        self._admit(context)
        if not self._joined or self._state != "selected" or self._selection is None:
            raise ProviderSetupUnavailable()
        return deepcopy(self._selection)

    def revoke(self):
        """Fence synchronously on the owning loop, then retain cleanup."""
        self._closed = True
        self._prompt_id = self._prompt = self._notice = self._selection = None
        if self._loop is not None and self._closing is None:
            self._closing = _retain(asyncio.create_task(self._close()))

    async def _close(self):
        if self._opening is not None:
            await asyncio.gather(asyncio.shield(self._opening), return_exceptions=True)
        if self._client is not None:
            await asyncio.shield(self._join())
        if self._reader is not None:
            await asyncio.gather(asyncio.shield(self._reader), return_exceptions=True)
        if self._answers:
            await asyncio.gather(*(asyncio.shield(task) for task in tuple(self._answers)), return_exceptions=True)

    async def close(self):
        self.revoke()
        if self._closing is not None:
            await asyncio.shield(self._closing)
        return self.retired
