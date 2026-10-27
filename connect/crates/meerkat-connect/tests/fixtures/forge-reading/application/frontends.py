"""Reading-specific durable configuration and current app-owned runtime observation.

This uses the existing collection Store and application lifetime. It is not an
agent tool, a second controller, a native input adapter or a delivery receipt.
"""
from contextlib import closing, contextmanager
from dataclasses import dataclass
import json
import os
import re
import sqlite3
import time
import uuid

COMPONENT = "reading-telegram"
PARTICIPANT = "reading-owner"
ROUTE = "reading-lead-dm"
PRIVATE_FIELD = "READING_TELEGRAM_BOT_TOKEN"
MAX_INTEGER = 2**53 - 1
# Actual Toolkit Telegram provider identity/presenter contract, not JS Integer.
MAX_TELEGRAM_ID = 2**52 - 1
CHANGE_ID = re.compile(r"[A-Za-z0-9._:-]{1,128}")
# Reading application policy, retained in each exact protected proposal.
# Native turn completion and provider settlement remain their owners' facts.
PENDING_POLICY = {"paused_input": "retain", "paused_observation": "continue",
                  "entered_admission": "join-with-uncertainty",
                  "accepted_work": "drain", "paused_replies": "drain",
                  "disabled_pending": "stop", "entered_provider": "join-with-uncertainty"}


@dataclass(frozen=True)
class _Submission:
    # Application-only transaction knowledge, not a wire or native receipt.
    observation: dict
    known: bool


def encoded(value):
    data = json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False,
                      separators=(",", ":"))
    if len(data.encode("utf-8")) > 256 * 1024:
        raise ValueError("The Reading frontend command is too large.")
    return data


def configuration(value):
    if isinstance(value, dict) and set(value) == {"component_id", "action"}:
        if value["component_id"] != COMPONENT or value["action"] not in {"pause", "resume", "disable", "remove"}:
            raise ValueError("Use a supported Reading frontend action.")
        return json.loads(encoded(value))
    keys = {"component_id", "profile", "participant_id", "route_id", "telegram_user_id", "telegram_dm_chat_id",
            "native_identity", "bot_token", "exclusive_polling_acknowledged"}
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError("Use the exact Reading private text DM configuration.")
    user = value["telegram_user_id"]
    if (value["component_id"] != COMPONENT or value["profile"] != "private-text-dm"
            or value["participant_id"] != PARTICIPANT or value["route_id"] != ROUTE
            or type(user) is not int or not 1 <= user <= MAX_TELEGRAM_ID
            or type(value["telegram_dm_chat_id"]) is not int or value["telegram_dm_chat_id"] != user
            or value["native_identity"] != "lead:main"
            or value["bot_token"] != {"private_field": PRIVATE_FIELD}
            or value["exclusive_polling_acknowledged"] is not True):
        raise ValueError("Reading supports one explicitly owned human text DM and lead:main.")
    # This is an explicit ownership acknowledgement, not a provider observation
    # or proof that another poller cannot exist. No secret bytes are resolved.
    return json.loads(encoded(value))


class ReadingFrontend:
    def __init__(self, context, store):
        self.context, self.store = context, store
        self.principal = os.environ.get("CONSOLE_USER_ID")
        self.clock = time.time
        self.started = False
        self.activation_observer = None

    def start(self):
        # Only the application start owner prepares schema. Management GETs and
        # command recovery never call Store.connect(), which also creates it.
        with closing(self.store.connect()) as db:
            db.executescript('''
            BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS reading_frontend_state(
              component_id TEXT PRIMARY KEY, revision INTEGER NOT NULL,
              configuration TEXT, binding TEXT);
            CREATE TABLE IF NOT EXISTS reading_frontend_changes(
              change_id TEXT PRIMARY KEY, proposal TEXT NOT NULL, result TEXT);
            INSERT OR IGNORE INTO reading_frontend_state(component_id,revision,configuration,binding) VALUES('reading-telegram',0,NULL,NULL);
            COMMIT;
            ''')
            # Extend the existing application row. Pause revisions do not replace
            # the configuration binding retained by already queued input.
            with db:
                columns = {row[1] for row in db.execute("PRAGMA table_info(reading_frontend_state)")}
                for name, declaration in (("binding_revision", "INTEGER NOT NULL DEFAULT 0"),
                                          ("management", "TEXT"), ("native_close", "TEXT")):
                    if name not in columns:
                        db.execute("ALTER TABLE reading_frontend_state ADD COLUMN " + name + " " + declaration)
                db.execute("UPDATE reading_frontend_state SET binding_revision=revision WHERE binding_revision=0 AND configuration IS NOT NULL")
                row = self.state(db)
                if row["management"] is None:
                    db.execute("UPDATE reading_frontend_state SET management=? WHERE component_id=?",
                        (encoded({"state": "enabled" if row["configuration"] else "unconfigured",
                                  "pending_work_policy": PENDING_POLICY}), COMPONENT))
        self.started = True

    def stop(self):
        # Integration joins its app-owned transport before stopping storage and
        # before the host shuts down the existing native runtime.
        self.started = False

    def activation(self):
        if self.activation_observer is not None:
            return self.activation_observer()
        return {"state": "unbound" if self.started else "not-started",
                "binding": "not-implemented", "application_started": self.started,
                "provider_observation": None, "native_admission": None, "reply_receipt": None}

    @contextmanager
    def connection(self, *, readonly):
        mode = "ro" if readonly else "rw"
        with closing(sqlite3.connect(self.store.path.resolve().as_uri() + "?mode=" + mode,
                                     uri=True, timeout=10)) as db:
            db.row_factory = sqlite3.Row
            if readonly:
                db.execute("PRAGMA query_only=ON")
            yield db

    def state(self, db):
        tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if not {"reading_frontend_state", "reading_frontend_changes"} <= tables:
            return None
        return db.execute("SELECT * FROM reading_frontend_state WHERE component_id=?", (COMPONENT,)).fetchone()

    def authorize(self, request):
        if not self.principal or request.principal != self.principal:
            raise PermissionError("Only the configured Reading owner may manage this frontend.")
        host = request.host
        if (not isinstance(host, dict) or set(host) != {"protocol", "host_id", "application_id", "source_digest"}
                or type(host["protocol"]) is not int or host["protocol"] != 1
                or host["application_id"] != self.context.deployment["slug"]
                or not isinstance(host["host_id"], str) or not re.fullmatch(r"forge:[a-f0-9]{64}", host["host_id"])
                or not isinstance(host["source_digest"], str) or not re.fullmatch(r"[a-f0-9]{64}", host["source_digest"])):
            raise PermissionError("The selected Reading application host is required.")

    def binding_matches_owner(self, binding, request):
        return (binding["principal"] == request.principal
                and binding["host"]["host_id"] == request.host["host_id"]
                and binding["host"]["application_id"] == request.host["application_id"])

    def inventory(self, request):
        storage, row = "not-initialized", None
        if self.store.path.exists():
            try:
                with self.connection(readonly=True) as db:
                    row = self.state(db)
                storage = "available" if row is not None else "not-initialized"
            except sqlite3.DatabaseError:
                storage = "unavailable"
        value = json.loads(row["configuration"]) if row is not None and row["configuration"] else None
        binding = json.loads(row["binding"]) if row is not None and row["binding"] else None
        return {"storage": storage, "revision": row["revision"] if row is not None else None,
                "configuration_status": "committed" if value is not None else "not-configured" if row is not None else "unknown",
                "configuration": value, "binding": binding,
                "management": json.loads(row["management"]) if row is not None and row["management"] else None,
                "native_close": json.loads(row["native_close"]) if row is not None and row["native_close"] else None,
                "binding_matches_current_source": binding == {"host": request.host, "principal": request.principal} if binding else None,
                "owner_principal": self.principal,
                "participants": [{"id": PARTICIPANT, "label": "Reading owner", "principal": self.principal,
                                  "telegram_user_id": value["telegram_user_id"] if value else None,
                                  "allowed_profile": "private-text-dm"}],
                "routes": [{"id": ROUTE, "label": "Reading guide in your private DM", "participant_id": PARTICIPANT,
                            "native_identity": "lead:main", "destination_rule": "same-authorized-human-private-dm",
                            "telegram_dm_chat_id": value["telegram_dm_chat_id"] if value else None}],
                "component": {"id": COMPONENT, "profile": "private-text-dm", "support": "private-text-dm-source-candidate",
                              "native_identity": "lead:main", "private_reference": {"private_field": PRIVATE_FIELD},
                              "participant": "One explicit Telegram numeric human ID; DM destination must match.",
                              "outside_first_profile": ["groups", "topics", "media", "multiple-participants"]},
                "management_actions": self.actions(row),
                "activation": self.activation()}

    def actions(self, row):
        mode = json.loads(row["management"])["state"] if row is not None and row["management"] else "unconfigured"
        close = json.loads(row["native_close"]) if row is not None and row["native_close"] else None
        allowed = {"pause": mode == "enabled", "resume": mode == "paused",
                   "disable": mode in {"enabled", "paused"},
                   "remove": mode == "disabled" and close is not None and close["state"] == "settled"}
        return [{"action": action, "available": allowed[action],
                 "prepare": {"configuration": {"component_id": COMPONENT, "action": action},
                             "expected_revision": row["revision"] if row is not None else None},
                 "pending_work_policy": PENDING_POLICY}
                for action in ("pause", "resume", "disable", "remove")]

    def status(self, request):
        change_id = request.payload.get("change_id")
        if not isinstance(change_id, str) or not CHANGE_ID.fullmatch(change_id):
            raise ValueError("Use the exact Reading change ID.")
        unknown = {"change_id": change_id, "status": "unknown", "activation": self.activation()}
        if not self.store.path.exists():
            return {**unknown, "storage": "not-initialized"}
        try:
            with self.connection(readonly=True) as db:
                if self.state(db) is None:
                    return {**unknown, "storage": "not-initialized"}
                row = db.execute("SELECT * FROM reading_frontend_changes WHERE change_id=?", (change_id,)).fetchone()
            if row is None:
                return {**unknown, "storage": "available"}
            proposal = json.loads(row["proposal"])
            if not self.binding_matches_owner(proposal, request):
                return {**unknown, "storage": "available"}
            result = json.loads(row["result"]) if row["result"] else None
            return {"change_id": change_id, "status": result["status"] if result else "prepared",
                    "proposal": proposal, "result": result, "storage": "available",
                    "binding_matches_current_source": proposal["host"] == request.host,
                    "management_observation": self.management_observation(),
                    "activation": self.activation()}
        except sqlite3.DatabaseError:
            return {**unknown, "storage": "unavailable"}

    def management_observation(self):
        with self.connection(readonly=True) as db:
            row = self.state(db)
        return {"revision": row["revision"], "policy": json.loads(row["management"]),
                "native_close": json.loads(row["native_close"]) if row["native_close"] else None}

    def next_state(self, row, value):
        state = json.loads(row["management"])["state"]
        action = value.get("action")
        if action is None:
            if state in {"disabled", "removed"}:
                raise ValueError("A disabled Reading component needs a separately selected new activation.")
            return "enabled"
        allowed = {"pause": {"enabled"}, "resume": {"paused"},
                   "disable": {"enabled", "paused"}, "remove": {"disabled"}}
        if state not in allowed[action] or not row["configuration"]:
            raise ValueError("Reading frontend state changed; read inventory again.")
        if action == "remove":
            close = json.loads(row["native_close"]) if row["native_close"] else None
            if close is None or close["state"] != "settled":
                raise ValueError("Removal requires the actual disabled component settlement.")
        return {"pause": "paused", "resume": "enabled", "disable": "disabled", "remove": "removed"}[action]

    def claim_component_close(self, revision):
        with self.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            row = self.state(db)
            if row["revision"] != revision or json.loads(row["management"])["state"] != "disabled" or (row["native_close"] and json.loads(row["native_close"])["state"] != "not_entered"):
                return False
            db.execute("UPDATE reading_frontend_state SET native_close=? WHERE component_id=?",
                       (encoded({"state": "entered", "observation": None}), COMPONENT))
            return True

    def record_component_close(self, revision, observation):
        with self.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            row = self.state(db)
            if row["revision"] != revision or json.loads(row["management"])["state"] != "disabled":
                raise ValueError("Reading close observation belongs to another policy revision.")
            db.execute("UPDATE reading_frontend_state SET native_close=? WHERE component_id=?",
                       (encoded(observation), COMPONENT))

    def prepare(self, request):
        if set(request.payload) != {"configuration", "expected_revision"}:
            raise ValueError("Prepare needs exact Reading configuration and revision.")
        value = configuration(request.payload["configuration"])
        revision = request.payload["expected_revision"]
        if type(revision) is not int or not 0 <= revision < MAX_INTEGER:
            raise ValueError("Use the exact Reading frontend revision.")
        with self.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            row = self.state(db)
            if row is None:
                raise ValueError("The application start owner must prepare Reading storage.")
            if revision != row["revision"]:
                raise ValueError("The Reading frontend revision changed. Read inventory again.")
            if row["binding"] and not self.binding_matches_owner(json.loads(row["binding"]), request):
                raise PermissionError("Existing configuration belongs to another host or owner.")
            self.next_state(row, value)
            proposal = {"change_id": COMPONENT + ":" + uuid.uuid4().hex,
                        "configuration": value, "expected_revision": revision,
                        "pending_work_policy": PENDING_POLICY,
                        "host": request.host, "principal": request.principal,
                        "expires_at": int(self.clock()) + 300}
            db.execute("INSERT INTO reading_frontend_changes VALUES(?,?,NULL)",
                       (proposal["change_id"], encoded(proposal)))
        return {"proposal": proposal}

    def submit(self, request):
        if set(request.payload) != {"proposal", "approval"}:
            raise ValueError("Submit the exact prepared Reading proposal.")
        approval = request.payload["approval"]
        if (request.approved is not True or not isinstance(approval, dict)
                or set(approval) != {"principal", "expires_at"}
                or approval["principal"] != request.principal
                or type(approval["expires_at"]) is not int or approval["expires_at"] <= self.clock()):
            raise PermissionError("Protected explicit human approval is required.")
        proposal = request.payload["proposal"]
        if not isinstance(proposal, dict):
            raise ValueError("Submit the exact prepared Reading proposal.")
        change_id = proposal.get("change_id")
        if not isinstance(change_id, str) or not CHANGE_ID.fullmatch(change_id):
            raise ValueError("Use the exact Reading change ID.")
        exact = encoded(proposal)
        with self.connection(readonly=False) as db, db:
            db.execute("BEGIN IMMEDIATE")
            current = self.state(db)
            if current is None:
                raise ValueError("The application start owner must prepare Reading storage.")
            row = db.execute("SELECT * FROM reading_frontend_changes WHERE change_id=?", (change_id,)).fetchone()
            if row is None:
                return _Submission({"change_id": change_id, "status": "unknown", "activation": self.activation()}, False)
            if row["proposal"] != exact:
                return _Submission({"change_id": change_id, "status": "change-conflict", "activation": self.activation()}, True)
            if proposal["host"] != request.host or proposal["principal"] != request.principal:
                return _Submission({"change_id": change_id, "status": "source-conflict", "activation": self.activation()}, True)
            if row["result"]:
                return _Submission(json.loads(row["result"]), True)
            if proposal["expires_at"] <= self.clock():
                result = {"change_id": change_id, "status": "expired", "activation_at_submission": self.activation()}
            elif proposal["expected_revision"] != current["revision"]:
                result = {"change_id": change_id, "status": "revision-conflict", "revision": current["revision"],
                          "activation_at_submission": self.activation()}
            elif current["binding"] and not self.binding_matches_owner(json.loads(current["binding"]), request):
                result = {"change_id": change_id, "status": "owner-conflict", "activation_at_submission": self.activation()}
            else:
                revision = current["revision"] + 1
                value = proposal["configuration"]
                mode = self.next_state(current, value)
                action = value.get("action")
                if action is None:
                    config, binding_revision = encoded(value), revision
                else:
                    config = None if action == "remove" else current["configuration"]
                    binding_revision = current["binding_revision"]
                db.execute("UPDATE reading_frontend_state SET revision=?,configuration=?,binding=?,binding_revision=?,management=? WHERE component_id=?",
                           (revision, config, encoded({"host": request.host, "principal": request.principal}),
                            binding_revision, encoded({"state": mode, "pending_work_policy": PENDING_POLICY}), COMPONENT))
                result = {"change_id": change_id, "status": "committed", "revision": revision,
                          "configuration_status": "removed" if action == "remove" else "committed",
                          "management_state": mode, "pending_work_policy": PENDING_POLICY,
                          "activation_at_submission": self.activation()}
            result["approval"] = {"approved": True, "principal": request.principal, "expires_at": approval["expires_at"]}
            # Configuration and exact outcome commit in the same transaction.
            # Any storage uncertainty propagates to the host's unknown response;
            # GET never retries it or invents a negative terminal receipt.
            db.execute("UPDATE reading_frontend_changes SET result=? WHERE change_id=?", (encoded(result), change_id))
            # The context manager must commit before this value can escape.
            # A failed or lost commit raises instead of declaring known.
            return _Submission(result, True)

    def manage(self, request, *, explicit_submission=False):
        self.authorize(request)
        if not isinstance(request.payload, dict):
            raise ValueError("Reading frontend commands are objects.")
        if request.operation == "inventory" and not request.payload:
            return self.inventory(request)
        if request.operation == "status" and set(request.payload) == {"change_id"}:
            return self.status(request)
        if request.operation == "prepare":
            return self.prepare(request)
        if request.operation == "submit":
            result = self.submit(request)
            return result if explicit_submission else result.observation
        raise ValueError("Unknown Reading frontend command.")
