"""Reading's one-human private text DM, on its existing SQLite/lifecycle owner.

Provider facts are not admission, native previews are not replies, and neither
configuration commitment nor an empty webhook establishes active ownership.
"""
import asyncio
from contextlib import closing
from dataclasses import asdict
import fcntl
import hashlib
import json
import os
import uuid

from .frontends import COMPONENT, PRIVATE_FIELD, encoded


class BindingRefused(Exception):
    def __init__(self, reason):
        self.reason = reason


async def join_task(task):
    """Keep custody through caller cancellation, then propagate it after join."""
    joined = asyncio.gather(task, return_exceptions=True)
    cancelled = False
    while not joined.done():
        try:
            await asyncio.shield(joined)
        except asyncio.CancelledError:
            cancelled = True
    joined.result()
    if cancelled:
        raise asyncio.CancelledError


class ReadingReplySender:
    """Borrowed routing only; the existing transport owns entered send work."""
    def __init__(self, actor, selection, sender_id, transport, presenter):
        self.actor, self.selection, self.sender_id = actor, selection, sender_id
        self.transport, self.presenter = transport, presenter

    async def activate(self):
        if not self.actor.current(self.selection):
            raise BindingRefused("configuration-source-or-owner-changed")

    async def deactivate(self):
        await self.transport.close()

    async def __call__(self, *, sender_id, recipient, part_index, part):
        if (sender_id != self.sender_id or part_index != 0
                or recipient != str(self.selection["configuration"]["telegram_dm_chat_id"])
                or type(part) is not dict or set(part) != {"text"}):
            raise BindingRefused("reply-selection-changed")
        # No suspension precedes the real transport's synchronous operation
        # retention. The presenter owns neither permission nor a background task.
        return await self.presenter.send_text(self.transport, recipient=recipient, text=part["text"])


class ReadingTelegram:
    def __init__(self, context, store, frontends):
        # No token resolution, Toolkit import, database or I/O in build().
        self.context, self.store, self.frontends = context, store, frontends
        self.task = None
        self.session = None
        self.changed = None
        self.stopping = False
        self.active_revision = None
        self.state = "not-started"
        self.provider = None
        self.native = None
        self.reply = None
        self.sender_authority = None
        self.sender_activation = None
        self.poll_interval = .25

    def observation(self):
        history = None
        if self.store.path.exists():
            try:
                with self.frontends.connection(readonly=True) as db:
                    rows = db.execute("SELECT status,COUNT(*) FROM reading_telegram_inbox GROUP BY status").fetchall()
                    conflicts = db.execute("SELECT COUNT(*) FROM reading_telegram_conflicts").fetchone()[0]
                history = {"input_outcomes": {row[0]: row[1] for row in rows}, "conflicting_provider_replays": conflicts}
            except Exception:
                history = {"storage": "unavailable-or-not-initialized"}
        return {"state": self.state, "binding": "reading-private-text-dm",
                "application_started": self.task is not None and not self.task.done() and not self.stopping,
                "active_revision": self.active_revision,
                "provider_observation": self.provider, "native_admission": self.native,
                "reply_receipt": self.reply,
                "ownership_scope": "same-application-local-locks-and-explicit-human-acknowledgement",
                "retained_history": history,
                "management_scope": "reading-application-policy-on-protected-host-tools"}

    async def start(self):
        if self.task is not None:
            raise RuntimeError("Reading Telegram is already started.")
        # Called only by the existing application start/storage owner.
        with closing(self.store.connect()) as db:
            db.executescript('''
            BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS reading_telegram_cursors(bot_id TEXT PRIMARY KEY,cursor TEXT);
            CREATE TABLE IF NOT EXISTS reading_telegram_inbox(
              sequence INTEGER PRIMARY KEY AUTOINCREMENT,bot_id TEXT NOT NULL,update_id INTEGER NOT NULL,
              payload TEXT NOT NULL,binding TEXT NOT NULL,status TEXT NOT NULL,
              dispatch_input TEXT,admission TEXT,completion TEXT,UNIQUE(bot_id,update_id));
            CREATE TABLE IF NOT EXISTS reading_telegram_conflicts(
              bot_id TEXT NOT NULL,update_id INTEGER NOT NULL,payload TEXT NOT NULL,
              UNIQUE(bot_id,update_id,payload));
            CREATE TABLE IF NOT EXISTS reading_telegram_outbound(
              bot_id TEXT NOT NULL,update_id INTEGER NOT NULL,attempt_id TEXT NOT NULL,
              recipient TEXT NOT NULL,text TEXT NOT NULL,ticket TEXT NOT NULL,binding TEXT NOT NULL,
              state TEXT NOT NULL,receipt TEXT,PRIMARY KEY(bot_id,update_id));
            COMMIT;
            ''')
        self.stopping = False
        self.changed = asyncio.Event()
        self.changed.set()
        self.state = "awaiting-configuration"
        self.task = asyncio.create_task(self._supervise())

    async def stop(self):
        self.stopping = True
        task = self.task
        if task is not None:
            task.cancel()
            try:
                await join_task(task)
            finally:
                self.task = None
                self.active_revision = None
                self.state = "not-started"

    async def configuration_committed(self, revision):
        # Commitment wakes the existing application actor. It is not a native
        # close receipt, and caller cancellation does not own actor settlement.
        with self.frontends.connection(readonly=True) as db:
            row = self.frontends.state(db)
        if row is None or row["revision"] != revision or self.stopping:
            return
        mode = json.loads(row["management"])["state"]
        if mode == "removed":
            self.state = "removed"
        elif self.changed is not None and (mode == "disabled" or self.active_revision != row["binding_revision"]):
            self.changed.set()

    async def observe_disabled_inputs(self):
        # Observation of exact retained tickets only. No provider poll, dispatch,
        # alternate identity, generated ticket, or delivery is performed here.
        if getattr(self.context, "submit_frontend_input", None) is None:
            from meerkat_mobkit.errors import TurnUnknownError, TurnFailedError
        else:
            from meerkat_meta.errors import TurnUnknownError, TurnFailedError
        with self.frontends.connection(readonly=True) as db:
            rows = db.execute("SELECT * FROM reading_telegram_inbox WHERE admission IS NOT NULL AND completion IS NULL AND status IN ('native-pending','native-observation-unknown','native-unknown','native-untracked','old-configuration-blocked') ORDER BY sequence").fetchall()
        for row in rows:
            admission = json.loads(row["admission"])
            ticket = admission.get("turn_ticket")
            if admission.get("identity") != "lead:main" or not isinstance(ticket, str) or not ticket:
                continue
            completion = None
            try:
                result = await self.context.runtime.turn_result("lead:main", ticket)
                observed = result.to_dict()
                if observed.get("identity") != "lead:main" or observed.get("ticket") != ticket:
                    status = "native-observation-unknown"
                elif observed.get("state") == "completed":
                    token = os.environ.get(PRIVATE_FIELD)
                    if token and token in encoded(observed):
                        status = "native-private-literal-refused"
                    else:
                        status, completion = "disabled-reply-stopped", observed
                else:
                    status = "native-pending"
            except TurnFailedError:
                status = "native-failed"
            except TurnUnknownError:
                status = "native-unknown"
            except Exception:
                status = "native-observation-unknown"
            with self.frontends.connection(readonly=False) as db, db:
                # Another actual observation may have settled the same ticket.
                # A pending/failed read cannot overwrite its retained completion.
                db.execute("UPDATE reading_telegram_inbox SET status=?,completion=? WHERE bot_id=? AND update_id=? AND admission=? AND completion IS NULL",
                           (status, encoded(completion) if completion is not None else None,
                            row["bot_id"], row["update_id"], row["admission"]))

    def drain_observation(self):
        with self.frontends.connection(readonly=True) as db:
            rows = db.execute("SELECT status,COUNT(*) FROM reading_telegram_inbox GROUP BY status").fetchall()
            old_entered = db.execute("SELECT COUNT(*) FROM reading_telegram_inbox WHERE status='old-configuration-blocked' AND dispatch_input IS NOT NULL AND completion IS NULL").fetchone()[0]
        outcomes = {name: count for name, count in rows}
        unresolved = old_entered + sum(outcomes.get(name, 0) for name in
            ("native-dispatching", "native-pending", "native-observation-unknown", "native-untracked", "native-unknown"))
        return {"accepted_native_unsettled": unresolved,
                "provider_outcome_unknown": outcomes.get("outbound-unknown", 0),
                "pending_input_stopped": outcomes.get("disabled-pending-stopped", 0),
                "pending_reply_stopped": outcomes.get("disabled-reply-stopped", 0)}

    async def disable_owner(self, selection):
        # The existing supervisor owns this continuation, including after an app
        # restart. The approved desired state requires no second human decision.
        from meerkat_toolkit.host_control import HostCloseReason, HostCloseState, HostCloseHandle, HostCloseScope
        while not self.stopping:
            with self.frontends.connection(readonly=True) as db:
                row = self.frontends.state(db)
            if json.loads(row["management"])["state"] != "disabled":
                return
            revision = row["revision"]
            self.stop_queued(selection)
            await self.observe_disabled_inputs()
            drain = self.drain_observation()
            with self.frontends.connection(readonly=True) as db:
                row = self.frontends.state(db)
            close = json.loads(row["native_close"]) if row["native_close"] else None
            if close is None or close["state"] == "not_entered":
                if drain["accepted_native_unsettled"]:
                    self.state = "disabled-awaiting-native-outcome"
                    self.frontends.record_component_close(revision,
                        {"state": "not_entered", "reason": "accepted-native-outcome-unsettled",
                         "application_drain": drain, "observation": None})
                    await asyncio.sleep(self.poll_interval)
                    continue
                if not self.frontends.claim_component_close(revision):
                    continue
                close = {"state": "entered", "observation": None}
                try:
                    actual = await asyncio.to_thread(self.context.runtime.host_control.begin_close_component,
                                                     COMPONENT, HostCloseReason.OPERATOR_DISABLE)
                    close = {**self.close_observation(actual), "application_drain": drain}
                    self.frontends.record_component_close(revision, close)
                except Exception:
                    self.frontends.record_component_close(revision,
                        {"state": "uncertain", "reason": "native-close-outcome-unavailable",
                         "application_drain": drain, "observation": None})
                    self.state = "disabled-close-outcome-unknown"
                    return
            observation = close.get("observation")
            if observation is None:
                # Entry may have succeeded without returning its handle. A
                # restart or another submit must never issue begin-close again.
                self.state = "disabled-close-outcome-unknown"
                return
            try:
                value = observation["handle"]
                handle = HostCloseHandle(uuid.UUID(value["incarnation"]), uuid.UUID(value["close_id"]),
                                         HostCloseScope(**value["scope"]))
                actual = await asyncio.to_thread(self.context.runtime.host_control.observe_close, handle)
                if actual.state == HostCloseState.CLOSING:
                    actual = await asyncio.to_thread(self.context.runtime.host_control.join_close, handle)
                close = {**self.close_observation(actual), "application_drain": drain}
                self.frontends.record_component_close(revision, close)
                if actual.state in {HostCloseState.SETTLED, HostCloseState.FAILED}:
                    self.state = "disabled"
                    return
            except Exception:
                # Keep the exact known handle so subsequent observation can
                # discover settlement without another begin-close operation.
                self.frontends.record_component_close(revision,
                    {"state": "uncertain", "reason": "native-close-observation-unavailable",
                     "application_drain": drain, "observation": observation})
            self.state = "disabled-awaiting-close-observation"
            await asyncio.sleep(self.poll_interval)

    @staticmethod
    def close_observation(actual):
        return {"state": actual.state.value,
                "observation": {"handle": actual.handle.wire(), "state": actual.state.value,
                                "failure": actual.failure}}

    def selected(self, db=None):
        try:
            host = self.context.frontend_host()  # Existing loaded-source owner.
        except Exception:
            raise BindingRefused("configuration-source-or-owner-changed") from None
        if db is None:
            with self.frontends.connection(readonly=True) as connection:
                row = self.frontends.state(connection)
        else:
            row = self.frontends.state(db)
        if row is None or not row["configuration"]:
            return None
        binding = json.loads(row["binding"])
        if binding != {"host": host, "principal": self.frontends.principal}:
            raise BindingRefused("configuration-source-or-owner-changed")
        value = json.loads(row["configuration"])
        if not 0 < value["telegram_user_id"] < 2**52:
            raise BindingRefused("telegram-id-outside-provider-range")
        return {"revision": row["binding_revision"], "configuration": value, "binding": binding}

    def mode(self, selection, db=None):
        if self.stopping:
            return None
        if db is None:
            with self.frontends.connection(readonly=True) as connection:
                return self.mode(selection, connection)
        if self.selected(db) != selection:
            return None
        return json.loads(self.frontends.state(db)["management"])["state"]

    def current(self, selection):
        try:
            return self.mode(selection) in {"enabled", "paused"}
        except Exception:
            return False

    def stop_queued(self, selection):
        if selection is None:
            return
        with self.frontends.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            if self.mode(selection, db) == "disabled":
                db.execute("UPDATE reading_telegram_inbox SET status='disabled-pending-stopped' WHERE binding=? AND status='queued'",
                           (encoded(selection),))

    async def close_sender(self, activation=None):
        held = self.sender_activation
        if held is None or (activation is not None and activation is not held):
            return
        # Keep the actual handle through failure/cancellation. The supervisor
        # and session remain the task owners; this is not an independent drainer.
        await self.sender_authority.close_activation(held)
        if self.sender_activation is held:
            self.sender_activation = None

    async def _supervise(self):
        session = None
        wake = None
        try:
            while not self.stopping:
                await self.changed.wait()
                self.changed.clear()
                self.provider = self.native = self.reply = None
                try:
                    await self.close_sender()
                    selection = self.selected()
                    if selection is None:
                        self.state = "awaiting-configuration"
                        continue
                    if self.mode(selection) == "disabled":
                        # Same supervisor and child custody as polling. Stop
                        # joins any entered observation before native shutdown.
                        session = asyncio.create_task(self.disable_owner(selection))
                        self.session = session
                        await join_task(session)
                        session.result()
                        session = self.session = None
                        continue
                    self.active_revision = selection["revision"]
                    session = asyncio.create_task(self._session(selection))
                    self.session = session
                    wake = asyncio.create_task(self.changed.wait())
                    done, _ = await asyncio.wait((session, wake), return_when=asyncio.FIRST_COMPLETED)
                    if wake in done:
                        if self.mode(selection) != "disabled":
                            session.cancel()
                        # Detach before awaiting physical send settlement. A
                        # disabled route preserves its already-entered receipt.
                        await self.close_sender()
                    await join_task(session)
                    wake.cancel()
                    await join_task(wake)
                    session = wake = None
                    self.session = None
                    # Failure stays explicit until a new configuration signal or
                    # application restart. No automatic uncertain-effect retry.
                except BindingRefused as exc:
                    self.state = exc.reason
                except Exception:
                    self.state = "configuration-unavailable"
        finally:
            for child in (wake, session):
                if child is not None:
                    child.cancel()
            try:
                await self.close_sender()
            finally:
                try:
                    if wake is not None:
                        await join_task(wake)
                finally:
                    try:
                        if session is not None:
                            await join_task(session)
                    finally:
                        self.active_revision = None
                        self.session = None

    def lock(self, scope):
        # Local app-scoped ownership only. Never infer external poller absence.
        name = hashlib.sha256(scope.encode()).hexdigest()
        path = self.store.path.parent / (".reading-telegram-" + name + ".lock")
        fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BaseException:
            os.close(fd)
            raise BindingRefused("active-same-application-poller")
        return fd

    async def _session(self, selection):
        transport = None
        activation = None
        locks = []
        try:
            if not self.current(selection):
                raise BindingRefused("configuration-source-or-owner-changed")
            if self.context.runtime is None:
                raise BindingRefused("native-runtime-not-started")
            # Existing private setup has already loaded this declared field.
            # Neither a model, config literal nor a management payload supplies it.
            token = os.environ.get(PRIVATE_FIELD)
            if not token:
                raise BindingRefused("private-token-not-configured")
            from meerkat_toolkit.frontend.telegram import TelegramBotApiTransport
            from meerkat_toolkit.frontend.telegram_adapter import TelegramPresenter
            locks.append(self.lock("token:" + token))
            factory = getattr(self.context, "telegram_transport_factory", TelegramBotApiTransport)
            transport = factory(token)
            self.state = "inspecting-provider"
            bot = await transport.get_me()
            locks.append(self.lock("bot:" + bot.bot_id))
            webhook = await transport.get_webhook_info()
            self.provider = {"bot_id": bot.bot_id, "webhook_configured": webhook.configured,
                             "ownership_proof": False, "poll_page_committed": False}
            if webhook.configured:
                raise BindingRefused("existing-webhook-refused")
            if not self.current(selection):
                raise BindingRefused("configuration-source-or-owner-changed")
            self.recover(bot.bot_id)
            self.state = "polling"
            presenter = TelegramPresenter()
            from meerkat_toolkit.frontend.sender import SenderAuthority
            if self.sender_authority is None:
                # Reuse the existing application database; no second outbox.
                self.sender_authority = SenderAuthority(path=self.store.path)
            sender_id = "reading:telegram:" + bot.bot_id
            sender = ReadingReplySender(self, selection, sender_id, transport, presenter)
            owner_id = hashlib.sha256(encoded(selection).encode()).hexdigest()
            activation = self.sender_authority.stage_activation(
                sender_id=sender_id, to_owner_id=owner_id, sender=sender)
            self.sender_activation = activation
            await self.sender_authority.commit_activation(activation)
            while self.current(selection):
                self.state = "paused" if self.mode(selection) == "paused" else "polling"
                await self.process_pending(bot.bot_id, selection, transport, presenter, token)
                if not self.current(selection):
                    break
                from meerkat_toolkit.frontend.telegram_adapter import poll_cursor_offset, interpret_poll_page
                cursor = self.cursor(bot.bot_id)
                updates = await transport.get_updates(poll_cursor_offset(cursor))
                page = interpret_poll_page(updates, cursor)
                if not self.current(selection):
                    break
                self.stage(bot.bot_id, selection, cursor, updates, page, token)
                self.provider = {**self.provider, "poll_page_committed": True}
                await self.process_pending(bot.bot_id, selection, transport, presenter, token)
                await asyncio.sleep(self.poll_interval)
            if not self.stopping:
                self.state = "disabled" if self.mode(selection) == "disabled" else "configuration-source-or-owner-changed"
        except BindingRefused as exc:
            self.state = exc.reason
        except asyncio.CancelledError:
            raise
        except Exception:
            # Provider/native/storage exceptions can contain credentials/URLs.
            self.state = "runtime-outcome-unknown"
        finally:
            # Toolkit close joins entered blocking operations under cancellation.
            # No guessed timeout, background native shutdown or detached thread.
            try:
                if activation is not None:
                    await self.close_sender(activation)
            finally:
                try:
                    if transport is not None:
                        await transport.close()
                finally:
                    for fd in reversed(locks):
                        os.close(fd)

    def cursor(self, bot_id):
        with self.frontends.connection(readonly=True) as db:
            row = db.execute("SELECT cursor FROM reading_telegram_cursors WHERE bot_id=?", (bot_id,)).fetchone()
        return row[0] if row else None

    def recover(self, bot_id):
        # Persisted entry before a missing reply means unknown, never permission
        # to dispatch or send again. Known tickets remain observation-only.
        with self.frontends.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            db.execute("UPDATE reading_telegram_inbox SET status='native-unknown' WHERE bot_id=? AND status='native-dispatching'", (bot_id,))
            db.execute("UPDATE reading_telegram_inbox SET status='outbound-unknown' WHERE bot_id=? AND update_id IN (SELECT update_id FROM reading_telegram_outbound WHERE bot_id=? AND state='entered')", (bot_id, bot_id))
            db.execute("UPDATE reading_telegram_outbound SET state='unknown' WHERE bot_id=? AND state='entered'", (bot_id,))

    def outcome(self, raw, event, selection):
        from meerkat_toolkit.frontend.telegram_adapter import TelegramEvent
        if event is None:
            return "ignored-provider-update"
        if not isinstance(event, TelegramEvent):
            return "unsupported:" + event.reason
        message = raw.get("message")
        if not isinstance(message, dict):
            return "refused-private-text-dm"
        sender, chat = message.get("from"), message.get("chat")
        cfg, envelope = selection["configuration"], event.envelope
        if (not isinstance(sender, dict) or not isinstance(chat, dict)
                or sender.get("is_bot") is not False or chat.get("type") != "private"
                or type(sender.get("id")) is not int or sender["id"] != cfg["telegram_user_id"]
                or type(chat.get("id")) is not int or chat["id"] != cfg["telegram_dm_chat_id"]
                or event.media or message.get("message_thread_id") is not None
                or not isinstance(message.get("text"), str) or not message["text"].strip()
                or envelope.get("provider") != "telegram" or envelope.get("sender_id") != str(sender["id"])
                or envelope.get("recipient_or_chat_id") != str(chat["id"])
                or envelope.get("text") != message["text"]):
            return "refused-private-text-dm"
        return "queued"

    def stage(self, bot_id, selection, old_cursor, updates, page, token):
        # Interpret the whole provider page first; one malformed item means no
        # cursor advance. All outcomes, including ignored, retain provider order.
        # Toolkit outcomes correspond to message-bearing raw items in provider
        # order. Duplicate IDs must never reuse a later payload's outcome.
        events = iter(page.events)
        prepared = []
        for raw in updates:
            payload = encoded(raw)
            event = next(events, None) if "message" in raw else None
            if "message" in raw and (event is None or event.update_id != raw["update_id"]):
                raise BindingRefused("provider-outcome-order-mismatch")
            status = self.outcome(raw, event, selection)
            if token in payload:
                payload, status = encoded({"update_id": raw["update_id"], "private_literal_refused": True}), "refused-private-literal"
            prepared.append((raw["update_id"], payload, status))
        if next(events, None) is not None:
            raise BindingRefused("provider-outcome-order-mismatch")
        with self.frontends.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            if self.mode(selection, db) not in {"enabled", "paused"}:
                raise BindingRefused("provider-observation-policy-changed")
            current = db.execute("SELECT cursor FROM reading_telegram_cursors WHERE bot_id=?", (bot_id,)).fetchone()
            if (current[0] if current else None) != old_cursor:
                raise BindingRefused("cursor-owner-conflict")
            for update_id, payload, status in prepared:
                prior = db.execute("SELECT payload FROM reading_telegram_inbox WHERE bot_id=? AND update_id=?", (bot_id, update_id)).fetchone()
                if prior:
                    if prior[0] != payload:
                        db.execute("INSERT OR IGNORE INTO reading_telegram_conflicts VALUES(?,?,?)", (bot_id, update_id, payload))
                    continue
                # A provider replay below retained cursor is recorded but cannot
                # become a new native input even if no earlier row is retained.
                if old_cursor is not None and update_id < int(old_cursor):
                    status = "old-provider-update"
                db.execute("INSERT INTO reading_telegram_inbox(bot_id,update_id,payload,binding,status) VALUES(?,?,?,?,?)",
                           (bot_id, update_id, payload, encoded(selection), status))
            next_cursor = page.next_cursor
            if old_cursor is not None and (next_cursor is None or int(next_cursor) < int(old_cursor)):
                next_cursor = old_cursor
            db.execute("INSERT INTO reading_telegram_cursors VALUES(?,?) ON CONFLICT(bot_id) DO UPDATE SET cursor=excluded.cursor", (bot_id, next_cursor))

    def update(self, bot_id, update_id, status, **fields):
        allowed = {"dispatch_input", "admission", "completion"}
        if set(fields) - allowed:
            raise ValueError("Unknown Reading input field.")
        assignments = ["status=?"] + [key + "=?" for key in fields]
        with self.frontends.connection(readonly=False) as db, db:
            db.execute("UPDATE reading_telegram_inbox SET " + ",".join(assignments) + " WHERE bot_id=? AND update_id=?",
                       [status] + [encoded(value) for value in fields.values()] + [bot_id, update_id])

    async def process_pending(self, bot_id, selection, transport, presenter, token):
        with self.frontends.connection(readonly=True) as db:
            rows = db.execute("SELECT * FROM reading_telegram_inbox WHERE bot_id=? AND status IN ('queued','native-pending','native-observation-unknown','native-completed') ORDER BY sequence", (bot_id,)).fetchall()
        for row in rows:
            mode = self.mode(selection)
            if json.loads(row["binding"]) != selection or mode is None:
                self.update(bot_id, row["update_id"], "old-configuration-blocked")
                continue
            if row["status"] == "queued" and mode != "enabled":
                if mode == "disabled":
                    self.stop_queued(selection)
                continue
            await self.process_one(row, selection, transport, presenter, token)

    async def process_one(self, row, selection, transport, presenter, token):
        submit = getattr(self.context, "submit_frontend_input", None)
        if submit is None:
            from meerkat_mobkit.identity_first_models import DispatchInput
            from meerkat_mobkit.errors import TurnUnknownError, TurnFailedError
        else:
            from meerkat_meta.errors import TurnUnknownError, TurnFailedError
        bot_id, update_id = row["bot_id"], row["update_id"]
        admission = json.loads(row["admission"]) if row["admission"] else None
        completion = json.loads(row["completion"]) if row["completion"] else None
        if row["status"] == "queued":
            raw = json.loads(row["payload"])
            if submit is None:
                # Preserve the archived SDK path for its existing fixture.
                di = DispatchInput(content=raw["message"]["text"], origin="connector",
                                   correlation_id=str(uuid.uuid4()),
                                   idempotency_key="reading:telegram:" + bot_id + ":" + str(update_id))
                retained_input = di.to_dict()
            else:
                # The application owns provider authentication and the configured
                # private source route. This is source evidence, not admission.
                retained_input = {"kind": "authenticated_provider_event", "bot_id": bot_id,
                                  "update_id": update_id, "provider_event": raw}
            # This reservation and protected management commit serialize on the
            # same existing SQLite owner. No await separates check and entry.
            with self.frontends.connection(readonly=False) as db, db:
                db.execute("BEGIN IMMEDIATE")
                mode = self.mode(selection, db)
                if mode != "enabled":
                    if mode == "disabled":
                        db.execute("UPDATE reading_telegram_inbox SET status='disabled-pending-stopped' WHERE bot_id=? AND update_id=? AND status='queued'", (bot_id, update_id))
                    return
                entered = db.execute("UPDATE reading_telegram_inbox SET status='native-dispatching',dispatch_input=? WHERE bot_id=? AND update_id=? AND status='queued'",
                                     (encoded(retained_input), bot_id, update_id))
                if entered.rowcount != 1:
                    return
            try:
                if submit is None:
                    result = await self.context.runtime.dispatch("lead:main", di, track_turn=True)
                    admission = {"native_result": result.to_dict(), "identity": "lead:main",
                                 "turn_ticket": result.turn_ticket,
                                 "turn_unavailable": asdict(result.turn_unavailable) if result.turn_unavailable else None}
                    ticket = result.turn_ticket
                    self.native = {"update_id": update_id, "identity": "lead:main", "turn_ticket": ticket,
                                   "durable": result.durable, "turn_unavailable": admission.get("turn_unavailable")}
                else:
                    result = await submit(bot_id, raw, selection)
                    # Retain the actual private receipt. It has no fencing token
                    # or durable flag and must not be converted to DispatchResult.
                    ticket = result.get("turn_ticket") if result.get("state") == "admitted" else None
                    admission = {"native_result": result, "identity": "lead:main", "turn_ticket": ticket}
                    self.native = {"update_id": update_id, **admission}
                self.update(bot_id, update_id, "native-pending" if isinstance(ticket, str) and ticket else "native-untracked", admission=admission)
            except BaseException:
                self.update(bot_id, update_id, "native-unknown")
                raise
        ticket = admission.get("turn_ticket") if admission else None
        if not isinstance(ticket, str) or not ticket:
            return
        if completion is None:
            try:
                result = await self.context.runtime.turn_result("lead:main", ticket)
                completion = result.to_dict()
                if completion.get("state") == "pending":
                    result = await self.context.runtime.agent("lead:main").wait_for_turn(ticket)
                    completion = result.to_dict()
            except asyncio.CancelledError:
                # Exact retained ticket remains pending, observation-only on return.
                raise
            except TimeoutError:
                return
            except TurnUnknownError:
                self.update(bot_id, update_id, "native-unknown")
                return
            except TurnFailedError:
                self.update(bot_id, update_id, "native-failed")
                return
            except Exception:
                # The ticket is known; transport observation uncertainty is not
                # lost admission. Resume only this exact ticket read, never dispatch.
                self.update(bot_id, update_id, "native-observation-unknown")
                self.native = {"identity": "lead:main", "turn_ticket": ticket, "observation": "unknown"}
                return
            if token in encoded(completion):
                self.update(bot_id, update_id, "native-private-literal-refused")
                return
            self.update(bot_id, update_id, "native-completed" if completion.get("state") == "completed" else "native-" + completion.get("state", "unknown"), completion=completion)
        if (completion.get("identity") != "lead:main" or completion.get("ticket") != ticket
                or completion.get("state") != "completed" or completion.get("output_status") != "text"
                or not isinstance(completion.get("output"), str) or not completion["output"].strip()
                or completion.get("output_truncated") is not False):
            self.update(bot_id, update_id, "native-no-permitted-reply")
            return
        before_reply = getattr(self.context, "before_frontend_reply_entry", None)
        if before_reply is not None:
            # Fixture observation barrier only; native completion remains the
            # actual ticket result and this hook cannot grant reply permission.
            await before_reply(bot_id, update_id)
        recipient = str(selection["configuration"]["telegram_dm_chat_id"])
        sender_id = "reading:telegram:" + bot_id
        dispatch = None
        if self.sender_authority is not None:
            try:
                dispatch = await self.sender_authority.select_dispatch(sender_id=sender_id)
            except (ValueError, TypeError):
                # Still record the existing disabled/changed policy outcome.
                # A detached route cannot spend the one-attempt row below.
                pass
        with self.frontends.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            mode = self.mode(selection, db)
            if mode not in {"enabled", "paused"}:
                status = "disabled-reply-stopped" if mode == "disabled" else "old-configuration-blocked"
                db.execute("UPDATE reading_telegram_inbox SET status=? WHERE bot_id=? AND update_id=?", (status, bot_id, update_id))
                return
            if (dispatch is None or not self.sender_authority.is_current_dispatch(dispatch)
                    or type(dispatch.sender) is not ReadingReplySender
                    or dispatch.sender.actor is not self
                    or dispatch.sender.selection != selection
                    or dispatch.sender.transport is not transport
                    or dispatch.sender.presenter is not presenter):
                return
            prior = db.execute("SELECT state FROM reading_telegram_outbound WHERE bot_id=? AND update_id=?", (bot_id, update_id)).fetchone()
            if prior:
                # This input has already had its one permitted attempt.
                return
            db.execute("INSERT INTO reading_telegram_outbound VALUES(?,?,?,?,?,?,?,?,NULL)",
                       (bot_id, update_id, str(uuid.uuid4()), recipient, completion["output"], ticket, encoded(selection), "entered"))
        try:
            receipt = await dispatch.sender(sender_id=sender_id, recipient=recipient,
                                            part_index=0, part={"text": completion["output"]})
            saved = asdict(receipt)
            if isinstance(saved.get("error"), str) and token in saved["error"]:
                saved["error"] = "Private provider literal omitted."
            with self.frontends.connection(readonly=False) as db, db:
                db.execute("UPDATE reading_telegram_outbound SET state=?,receipt=? WHERE bot_id=? AND update_id=?", (receipt.outcome, encoded(saved), bot_id, update_id))
                db.execute("UPDATE reading_telegram_inbox SET status=? WHERE bot_id=? AND update_id=?", ("outbound-" + receipt.outcome, bot_id, update_id))
            self.reply = {"update_id": update_id, "recipient": recipient, "turn_ticket": ticket, "receipt": saved}
        except BaseException:
            # Entry is durable before send. A cancellation/lost receipt is unknown
            # even if the provider performed it. No retry or alternate destination.
            with self.frontends.connection(readonly=False) as db, db:
                db.execute("UPDATE reading_telegram_outbound SET state='unknown' WHERE bot_id=? AND update_id=?", (bot_id, update_id))
                db.execute("UPDATE reading_telegram_inbox SET status='outbound-unknown' WHERE bot_id=? AND update_id=?", (bot_id, update_id))
            raise
