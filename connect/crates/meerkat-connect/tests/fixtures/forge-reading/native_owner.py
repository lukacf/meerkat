#!/usr/bin/env python3
"""Opt-in Reading application owner with actual native input and loopback reply.

Builds nothing. The separate client uses the protected configuration HTTP
contract, not the Connect CLI. Telegram and model endpoints are controlled
loopback services; no real account, vendor delivery or deployment is qualified.
The interpreter must include the selected Toolkit telegram extra
(python-telegram-bot==22.7), plus the existing Console HTTP dependencies.
"""
import argparse
import asyncio
import copy
import dataclasses
import hashlib
import importlib
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import threading
import time
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
NATIVE_FACADE_SHA256 = "15c66c4097aa0e1ffcf5569cf46d40295e80c49ed3741ad2dffef3ee0786e317"
USER = 123456789
BOT = 987654321
IDENTITY = "lead:main"
ROUTE = "reading-telegram-dm"
CHANNEL = "reading-telegram"
TOKEN = "987654321:reading-loopback-private-token"
PASSWORD = "reading-loopback-private-password"
INPUT = "Reply with the exact Reading native frontend fixture acknowledgement."
ANSWER = "Reading native frontend fixture acknowledgement."
CONFIGURATION = {"component_id": CHANNEL, "profile": "private-text-dm",
    "participant_id": "reading-owner", "route_id": "reading-lead-dm",
    "telegram_user_id": USER, "telegram_dm_chat_id": USER, "native_identity": IDENTITY,
    "bot_token": {"private_field": "READING_TELEGRAM_BOT_TOKEN"},
    "exclusive_polling_acknowledged": True}

# Exact application policy reviewed in the protected proposal, not native truth.
PENDING_POLICY = {"paused_input": "retain", "paused_observation": "continue",
                  "accepted_work": "drain", "entered_admission": "join-with-uncertainty", "paused_replies": "drain",
                  "disabled_pending": "stop", "entered_provider": "join-with-uncertainty"}


class FixtureFailure(AssertionError):
    """Only fixed assertion labels authored in this harness may be reported."""
    def __init__(self, label):
        super().__init__(label)
        self.label = label


def require(value, label):
    if not value:
        raise FixtureFailure(label)


def failure(error, phase):
    observation = {"phase": phase, "error_type": type(error).__name__}
    if isinstance(error, FixtureFailure):
        observation["label"] = error.label
    return observation


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()


def setup_client(origin, action=None):
    """One protected application request session; no alternate management route."""
    import http.client
    from http.cookies import SimpleCookie
    import re
    from urllib.parse import urlencode, urlsplit
    target = urlsplit(origin)
    require(target.scheme == "http" and target.hostname == "127.0.0.1" and target.port,
            "setup client requires the selected loopback owner")
    cookies = SimpleCookie()

    def request(method, path, body=None, content_type="application/json"):
        connection = http.client.HTTPConnection(target.hostname, target.port, timeout=30)
        headers = {"Origin": origin}
        if cookies:
            headers["Cookie"] = "; ".join(key + "=" + value.value for key, value in cookies.items())
        if body is not None:
            headers["Content-Type"] = content_type
        try:
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            raw = response.read(256 * 1024 + 1)
            require(len(raw) <= 256 * 1024, "setup response exceeded bound")
            for key, value in response.getheaders():
                if key.lower() == "set-cookie":
                    cookies.load(value)
            return response.status, raw
        finally:
            connection.close()

    endpoint = "/console/forge/frontends"
    require(request("GET", endpoint)[0] == 401, "frontend inventory allowed anonymous access")
    status, body = request("GET", "/login")
    matches = re.findall(rb'name="nonce" value="([^"]+)"', body)
    require(status == 200 and len(matches) == 1, "missing actual login nonce")
    require(request("POST", "/login", urlencode({"nonce": matches[0].decode(), "password": PASSWORD}).encode(),
                    "application/x-www-form-urlencoded")[0] == 303, "protected login failed")
    status, body = request("GET", endpoint)
    inventory = json.loads(body)
    require(status == 200, "protected inventory failed")
    if action in {"inspect", "observe-settled"}:
        deadline = time.monotonic() + 25
        while action == "observe-settled" and (inventory["observation"]["native_close"] or {}).get("state") != "settled":
            require(time.monotonic() < deadline, "settled native close was not observed through the protected endpoint")
            time.sleep(.02)
            status, body = request("GET", endpoint)
            require(status == 200, "protected inventory failed during management")
            inventory = json.loads(body)
        require(request("POST", "/logout", b"")[0] == 303, "protected logout failed")
        print(json.dumps({"status": "observed", "observation": inventory["observation"]}), flush=True)
        return
    expected = inventory["observation"]["revision"]
    require(type(expected) is int and expected >= 0, "invalid observed configuration revision")
    configuration = CONFIGURATION if action in {None, "enable"} else {"component_id": CHANNEL, "action": action}
    if action in {None, "enable"}:
        require(expected == 0, "unexpected initial frontend state")
    else:
        require(action in {"pause", "resume", "disable", "remove"}, "unsupported controlled management action")
        require(inventory["observation"]["configuration"] == CONFIGURATION
                and inventory["observation"]["binding_matches_current_source"] is True,
                "management lost the exact configured application owner")
    prepare = {"configuration": configuration, "expected_revision": expected}
    if action not in {None, "enable"}:
        actions = inventory["observation"]["management_actions"]
        offered = [row for row in actions if row["action"] == action]
        require(len(offered) == 1 and offered[0]["available"] is True
                and offered[0]["prepare"] == prepare and offered[0]["pending_work_policy"] == PENDING_POLICY,
                "requested management action was not advertised by this application owner")
        prepare = offered[0]["prepare"]
    status, body = request("POST", endpoint + "/prepare", canonical(prepare))
    prepared = json.loads(body)
    require(status == 200 and prepared["proposal"]["configuration"] == configuration
            and prepared["proposal"]["expected_revision"] == expected
            and prepared["proposal"]["host"] == inventory["host"], "actual proposal changed the selected configuration")
    if action is not None:
        require(prepared["proposal"]["pending_work_policy"] == PENDING_POLICY,
                "protected proposal changed the requested pending-work policy")
    payload = {"proposal": prepared["proposal"], "approval": {**prepared["approval"], "approved": True}}
    wrong = copy.deepcopy(payload)
    if action in {None, "enable"}:
        wrong["proposal"]["configuration"]["telegram_dm_chat_id"] += 1
    else:
        wrong["proposal"]["expected_revision"] += 1
    require(request("POST", endpoint + "/submit", canonical(wrong))[0] == 403,
            "protected proof accepted a different reviewed request")
    status, body = request("POST", endpoint + "/submit", canonical(payload))
    result = json.loads(body)
    require(status == 200 and result["observation"]["status"] == "committed"
            and result["observation"]["revision"] == expected + 1, "actual application owner did not commit configuration")
    require(request("POST", "/logout", b"")[0] == 303, "protected logout failed")
    observation = {"status": "committed", "revision": expected + 1,
                   "change_id": prepared["proposal"]["change_id"]}
    if action is not None:
        observation.update(action=action, observation=result["observation"])
    print(json.dumps(observation), flush=True)


class LoopbackTelegram:
    """HTTP provider fixture used by the real Toolkit transport/presenter."""
    def __init__(self, send_limit=1):
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        owner = self
        self.lock = threading.Lock()
        self.send_limit = send_limit
        self.updates, self.calls, self.sends, self.errors = [], [], [], []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    require(self.path.startswith("/bot" + TOKEN + "/"), "unexpected provider path")
                    method = self.path.removeprefix("/bot" + TOKEN + "/")
                    size = int(self.headers.get("Content-Length", "0"))
                    require(0 < size < 64 * 1024, "provider body bound")
                    body = json.loads(self.rfile.read(size))
                    with owner.lock:
                        owner.calls.append({"method": method, "body": body})
                        if method == "getMe":
                            result = {"id": BOT, "is_bot": True, "first_name": "Reading fixture"}
                        elif method == "getWebhookInfo":
                            result = {"url": "", "has_custom_certificate": False, "pending_update_count": 0}
                        elif method == "getUpdates":
                            offset = body.get("offset", 0)
                            result = [row for row in owner.updates if row["update_id"] >= offset]
                        elif method == "sendMessage":
                            require(str(body["chat_id"]) == str(USER) and body["text"] == ANSWER,
                                    "reply differs from exact destination or native completion")
                            require(len(owner.sends) < owner.send_limit, "provider send was replayed or crossed disabled entry")
                            owner.sends.append(body)
                            result = {"message_id": 900 + len(owner.sends), "date": int(time.time()),
                                      "chat": {"id": USER, "type": "private"}, "text": body["text"]}
                        else:
                            raise FixtureFailure("unexpected provider method")
                    raw = canonical({"ok": True, "result": result})
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                except BaseException as error:
                    owner.errors.append(failure(error, "loopback_provider_request"))
                    self.send_error(500, "controlled provider refused")

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = False
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=False)
        self.thread.start()
        self.api = "http://127.0.0.1:" + str(self.server.server_address[1]) + "/bot" + TOKEN + "/"

    def release(self, *update_ids):
        with self.lock:
            for update_id in update_ids or (101,):
                require(type(update_id) is int and 101 <= update_id <= 105
                        and not any(row["update_id"] == update_id for row in self.updates),
                        "provider occurrence was repeated or undeclared")
                self.updates.append({"update_id": update_id, "message": {"message_id": update_id + 400,
                    "date": int(time.time()), "from": {"id": USER, "is_bot": False, "first_name": "Fixture human"},
                    "chat": {"id": USER, "type": "private"}, "text": INPUT}})

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(5)
        require(not self.thread.is_alive(), "loopback provider did not join")


async def frontend_plan(v, args, roster, environment, events, report, observe):
    # Reuse existing application/helper preparation, adding only this original
    # route before the selected replacement pins. The base pins are not selected.
    directory = args.output
    plan, plan_pin, source_pin = v.installed_plan(directory, roster)
    finalization = directory / "finalization"
    finalization.mkdir(mode=0o700)
    finalizer = v.InstalledFixtureFinalizer(args.toolkit_gateway, finalization, plan,
        plan_pin, source_pin, environment, events, observe)
    report["finalization"] = finalizer.evidence
    inventory_input = v.new_artifact(directory / "reading-inventory-input.json", canonical({
        "schema": 1, "mob_id": "fixture", "roster": roster, "helper_identity": "helper"}))
    report["inventory_input"] = inventory_input
    # The existing pure native command owns public-identity/member lowering.
    # Its declared roster inventory is not an admission, grant or live session.
    inventory = await finalizer.command("reading-inventory",
        ["--lower-application-inventory", "--input", inventory_input["path"]])
    require(isinstance(inventory, dict)
            and set(inventory) == {"schema", "mob_id", "realm_id", "helper_identity", "members"}
            and inventory["schema"] == 1 and inventory["mob_id"] == "fixture"
            and inventory["realm_id"] == plan["application"]["realm"]["realm_id"]
            and inventory["helper_identity"] == "helper"
            and isinstance(inventory["members"], list) and len(inventory["members"]) == len(roster),
            "native inventory differs from the declared Reading roster")
    bindings = {}
    for row, declared in zip(inventory["members"], roster):
        require(isinstance(row, dict) and set(row) == {"identity", "member"}
                and row["identity"] == declared["identity"] and row["identity"] not in bindings
                and isinstance(row["member"], dict) and set(row["member"]) == {"mob_id", "role", "member"}
                and row["member"]["mob_id"] == inventory["mob_id"]
                and row["member"]["role"] == declared["profile"]
                and isinstance(row["member"]["member"], str) and row["member"]["member"],
                "native inventory member differs from its exact public identity")
        bindings[row["identity"]] = row["member"]
    require(set(bindings) == {"helper", IDENTITY}
            and bindings["helper"] == plan["application"]["helper"]["member"]
            and len({row["member"] for row in bindings.values()}) == len(bindings),
            "native inventory changed the selected helper or repeated a member")
    member = copy.deepcopy(bindings[IDENTITY])
    source = json.loads(Path(source_pin["path"]).read_bytes())
    principal = lambda name: {"kind": "service_account", "id": name,
        "qualification": {"kind": "qualified", "trust_domain_id": "reading-loopback-fixture"}}
    requester = {"kind": "human", "id": str(USER),
        "qualification": {"kind": "qualified", "trust_domain_id": "reading-loopback-bot:" + str(BOT)}}
    native = plan["owner"]["native_host"]
    source_owner = principal("reading-provider-source")
    selected = {"member": member, "logical_owner": principal("reading-lead"),
        "context_generation": 1, "audience": {"audience": "principal", "principal": requester}}
    native["members"].append(selected)
    plan["application"]["members"] = native["members"]
    plan["owner"]["components"] = [CHANNEL]
    plan["owner"]["channels"] = [{"component": CHANNEL, "channel_id": CHANNEL,
        "service_subject": CHANNEL, "provider_account": str(BOT), "sender_id": str(USER),
        "requester": requester, "ingress_actor": source_owner, "target_identity": IDENTITY,
        "member": member, "realm": "mob.fixture", "required_surfaces": ["channel:" + CHANNEL]}]
    native["routes"] = [{"id": ROUTE, "origin": {"kind": "provider_event", "channel_id": CHANNEL,
        "sender_id": str(USER), "target_identity": IDENTITY}, "application_parent": None,
        "controller_parent": None, "logical_executor": selected["logical_owner"], "represented_subject": None,
        "source_authority": source_owner, "authentication_domain": {"authority": source_owner, "namespace": "reading-provider-auth"},
        "event_domain": {"authority": source_owner, "namespace": "reading-provider-events"}}]
    plan["authorities"]["controllers"].append({"target": {"kind": "original_route", "route": ROUTE},
        "issuance_id": "reading-controller-parent"})
    plan["application"]["definition"]["text"] += '''
[profiles.lead]
external_addressable = true
model = "fixture-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"
[profiles.lead.tools]
comms = true
'''
    source.update(purpose="Reading loopback frontend original, no acquisition or live provider",
        definition=plan["application"]["definition"], frontend_configuration=CONFIGURATION,
        source_route=native["routes"][0], source_channel=plan["owner"]["channels"][0], target=selected)
    source_pin = v.new_artifact(directory / "reading-policy.json", canonical(source))
    policy = plan["application"]["tool_policy"]
    policy["source"]["source_digest"] = v.installed_fixture_policy_digest(canonical(source))
    policy["members"].append({"member_identity": member["member"], "grants": []})
    plan_pin = v.new_artifact(directory / "reading-plan.json", canonical(plan))
    finalizer.plan_pin, finalizer.source_pin = plan_pin, source_pin
    return plan, plan_pin, source_pin, finalizer


async def run(args, v):
    from meerkat_meta.builder import MetaBuilderConfig
    from meerkat_meta.identity_first_models import DurableAgentSpec
    from meerkat_toolkit.application_runtime import ToolkitApplicationRuntime
    from meerkat_meta.errors import GatewayStartupCleanupError
    from meerkat_toolkit.frontend.telegram import TelegramBotApiTransport
    from application import build
    from application.telegram_runtime import join_task
    import native_console_auth
    from native_console_auth import (ConsoleFront, FrontendManagementRequest,
        FrontendSubmissionDisposition, FrontendSubmissionResult, _frontend_json)
    require(Path(native_console_auth.__file__).resolve() == HERE / "native_console_auth.py",
            "native Reading facade import escaped its pinned module")
    SDK = v.SDK
    directory = args.output
    events = v.EventLog(directory)
    report = {"scope": "persistent Reading owner; protected HTTP setup client; controlled loopback provider and model; no Connect CLI or real Telegram claim",
              "status": "failed", "phase": "preparation", "imports": [], "setup_client": None,
              "native_facade": {"path": str(HERE / "native_console_auth.py"),
                                "sha256": NATIVE_FACADE_SHA256, "bytes": 83475}}
    if args.cli_owner:
        report["scope"] = "persistent Reading owner driven by actual meerkat-connect CLI; controlled loopback provider/model; no real Telegram or rendered Console UI claim"
    runtime = integration = front = telegram = provider = transport = None
    child = public_reader = private_reader = None
    management_children = []
    control_transport = None
    management_calls = []
    reply_waiting, release_reply = asyncio.Event(), asyncio.Event()
    outgoing, responses, settlements = [], [], []
    original_directory = Path.cwd()
    previous = {name: os.environ.get(name) for name in ("CONSOLE_USER_ID", "READING_TELEGRAM_BOT_TOKEN")}
    candidate, package, _, _ = v.installed_candidate(args)
    report["candidate"] = candidate

    class Provider(v.InstalledAcquisitionProvider):
        def error_observation(self, error):
            return failure(error, "controlled_model_request")

        def check_model_request(self, body):
            require(TOKEN not in json.dumps(body) and PASSWORD not in json.dumps(body), "private input reached model")

        def response(self, body):
            require(body["model"] == "fixture-model" and 1 <= len(self.requests) <= (3 if args.management else 1), "unexpected native model work")
            require(INPUT in json.dumps(body["messages"]), "native model lacks exact admitted input")
            return {"delta": {"content": ANSWER}, "finish_reason": "stop"}

    try:
        # The selected SDK keeps its Telegram parser optional. This application
        # uses that declared extra and checks it before providers or native work.
        report["phase"] = "telegram_parser_preflight"
        try:
            parser_version = importlib.metadata.version("python-telegram-bot")
        except importlib.metadata.PackageNotFoundError:
            parser_version = None
        require(parser_version == "22.7",
                "Reading requires the Toolkit telegram extra: python-telegram-bot==22.7")
        try:
            from telegram import Update
        except Exception:
            raise FixtureFailure("the declared Telegram parser could not be loaded") from None
        require(callable(getattr(Update, "de_json", None)), "the declared Telegram parser API is unavailable")
        parser_path = Path(sys.modules["telegram"].__file__).resolve()
        report["telegram_parser"] = {"distribution": "python-telegram-bot", "version": parser_version,
                                    "path": str(parser_path), "sha256": v.digest(parser_path)}
        report["phase"] = "preparation"
        provider = Provider(directory, events)
        telegram = LoopbackTelegram(send_limit=2 if args.management else 1)
        environment = v.process_env(directory)
        environment["META_GATEWAY_STDERR_FILE"] = str(directory / "gateway.stderr")
        config_path = directory / "meerkat.toml"
        v.new_artifact(config_path, v.config_text(provider.port).encode())
        specs = [DurableAgentSpec(identity="helper", profile="research", runtime_mode_override="turn_driven"),
                 DurableAgentSpec(identity=IDENTITY, profile="lead", runtime_mode_override="turn_driven")]
        plan, plan_pin, source_pin, finalizer = await frontend_plan(v, args,
            [spec.to_dict() for spec in specs], environment, events, report, lambda: require(
                transport is not None and transport._process.poll() is None, "native child left during finalization"))
        report.update(plan=plan_pin, policy=source_pin)

        class Providers:
            async def roster(self, context):
                require(context.mob_definition is not None, "native roster has no definition")
                return specs

            async def compute_edges(self, target_identities, context):
                return []

        class Application(ToolkitApplicationRuntime):
            def _make_gateway_transport(self):
                nonlocal transport
                transport = super()._make_gateway_transport()
                transport._env = dict(environment)
                write, send, deliver = transport._write_line, transport._send_sync_running, transport._deliver_init_event

                def observed_write(value, **kwargs):
                    outgoing.append(value)
                    return write(value, **kwargs)

                def observed_send(value, *, timeout=None, **kwargs):
                    result = send(value, timeout=timeout, **kwargs)
                    responses.append(result)
                    return result

                def observed_deliver(method, params, **kwargs):
                    settlements.append({"method": method, "params": params})
                    return deliver(method, params, **kwargs)

                transport._write_line, transport._send_sync_running = observed_write, observed_send
                transport._deliver_init_event = observed_deliver
                return transport

        providers = Providers()
        config = MetaBuilderConfig(gateway_bin=str(args.toolkit_gateway), init_deadline=v.INSTALLED_LIMIT,
            gateway_init_timeout=v.INSTALLED_LIMIT, roster_provider=providers, topology_provider=providers,
            persistent_state=plan["application"]["realm"]["state_root"], meerkat_config_path=str(config_path),
            http_listen="127.0.0.1:0", console_read_only=True, console_require_app_auth=False)
        runtime = Application(config, SDK.ApplicationPreparation(SDK.SelectedHostArtifact(**plan_pin), timeout=v.INSTALLED_LIMIT), finalizer)
        os.chdir(directory)
        report["phase"] = "native_connect"
        await runtime.connect()
        attempt = runtime.preparation_attempt
        require(runtime.is_running and transport._process.poll() is None and attempt is not None
                and attempt.settlement is not None and not isinstance(attempt.settlement.native, BaseException)
                and not isinstance(attempt.settlement.finalization, BaseException)
                and dataclasses.asdict(attempt.selection) == finalizer.selection, "actual preparation did not settle")
        native_pid = transport._process.pid
        public_reader, private_reader = transport._reader_thread, runtime.host_control._reader
        init = [row for row in outgoing if row.get("method") == "meta/init"]
        require(len(init) == 1, "native init entered more than once")
        ready = [row for row in settlements if row["method"] == "meta/init_settled"
                 and row["params"].get("init_id") == init[0]["params"]["init_id"]
                 and row["params"].get("outcome") == "ready"]
        require(len(ready) == 1, "missing correlated native Ready")
        report.update(native_pid=native_pid, native_ready=ready[0], finalization=finalizer.evidence)
        source_digest = hashlib.sha256(canonical({name: v.digest(HERE / "application" / name)
            for name in ("__init__.py", "frontends.py", "collection.py", "telegram_runtime.py")})).hexdigest()
        host = {"protocol": 1, "host_id": "forge:" + hashlib.sha256(str(directory).encode()).hexdigest(),
                "application_id": "reading-fixture", "source_digest": source_digest}
        os.environ["CONSOLE_USER_ID"] = "fixture-human"
        os.environ["READING_TELEGRAM_BOT_TOKEN"] = TOKEN
        admissions = []
        report["admissions"] = admissions

        async def entered_call(function, *values, **keywords):
            task = asyncio.create_task(asyncio.to_thread(function, *values, **keywords))
            try:
                return await asyncio.shield(task)
            except asyncio.CancelledError:
                await join_task(task)
                raise

        async def submit(bot_id, raw, selection):
            report["frontend_phase"] = "frontend_route_validation"
            try:
                require(report["setup_client"] is not None and report["setup_client"]["exit_code"] == 0,
                        "frontend input preceded setup client exit")
                require(bot_id == str(BOT) and selection["configuration"] == CONFIGURATION
                        and integration.telegram.current(selection), "frontend owner selection changed")
                expected_updates = (101, 102, 103) if args.management else (101,)
                require(len(admissions) < len(expected_updates)
                        and raw["update_id"] == expected_updates[len(admissions)]
                        and raw in telegram.updates and raw["message"]["from"]["id"] == USER
                        and raw["message"]["chat"] == {"id": USER, "type": "private"}
                        and raw["message"]["text"] == INPUT, "wrong provider occurrence or replay")
                # A real response from the loopback provider supplies this occurrence.
                # The trusted application validates bot/account, sender, DM and content
                # before crossing its inherited host pipe. No real Telegram login claim.
                occurrence = "reading:" + bot_id + ":" + str(raw["update_id"])
                evidence = SDK.PrivateSourceEvidence(occurrence, "reading-loopback-auth-v1",
                    hashlib.sha256(canonical({"bot": bot_id, "sender": USER, "dm": USER, "event": raw})).hexdigest())
                event = SDK.PrivateAuthenticatedEvent(ROUTE, occurrence, occurrence, evidence,
                    SDK.PrivateSourceEvidence(occurrence, "reading-loopback-event-v1", hashlib.sha256(canonical(raw)).hexdigest()),
                    (), hashlib.sha256(INPUT.encode()).hexdigest())
                report["frontend_phase"] = "retain_authenticated_event"
                retained = await entered_call(runtime.host_control.retain_authenticated_event, event, timeout=60.0)
                provider.enabled = True
                report["frontend_phase"] = "submit_authenticated_original"
                actual = await entered_call(runtime.host_control.submit_authenticated_original,
                                            retained.original, INPUT, evidence, timeout=60.0)
                admissions.append({"provider_update_id": raw["update_id"], "event": dataclasses.asdict(event), "original": dataclasses.asdict(retained.original), "outcome": actual})
                require(actual["state"] == "admitted" and actual["source_settlement"] == "fresh_acceptance_promoted"
                        and actual["deduplicated"] is False and actual["turn_ticket"], "native input was not admitted and tracked")
                report["frontend_phase"] = "native_session_validation"
                identity_status = await runtime.status(IDENTITY)
                # Bounded native fields only; the predicate below uses the full values.
                report["native_identity_status"] = {
                    "identity": identity_status.identity[:256],
                    "identity_truncated": len(identity_status.identity) > 256,
                    "state": identity_status.state[:64],
                    "state_truncated": len(identity_status.state) > 64,
                    "session_id": identity_status.session_id[:128] if identity_status.session_id is not None else None,
                    "session_id_truncated": identity_status.session_id is not None and len(identity_status.session_id) > 128}
                require(identity_status.identity == IDENTITY and identity_status.session_id == actual["session_id"],
                        "native identity status does not match the admitted session")
                report["frontend_phase"] = "admitted"
                return actual
            except BaseException as error:
                report["frontend_failure"] = failure(error, report["frontend_phase"])
                raise

        def transport_factory(token):
            require(token == TOKEN, "unexpected declared private token")
            selected = TelegramBotApiTransport(token)
            # Fixture-only endpoint substitution; all HTTP parsing, custody,
            # send classification and destination checks remain the real code.
            selected._api_base = telegram.api
            selected._file_base = telegram.api + "unsupported-file/"
            return selected

        async def before_reply_entry(bot_id, update_id):
            if args.management and update_id == 103:
                require(bot_id == str(BOT) and not reply_waiting.is_set(), "unexpected pending-reply barrier entry")
                # The real actor has already checked the real completed ticket.
                # This hook cannot grant delivery or supply a native outcome.
                reply_waiting.set()
                await release_reply.wait()

        def frontend_submission_result(command, observation, *, known):
            # Use the exact types loaded by this native-only producer facade.
            # The application declares command disposition; this host binds it
            # to the original approved command and current full source owner.
            require(type(command) is FrontendManagementRequest and command.operation == "submit"
                    and command.approved is True and type(command.payload) is dict
                    and type(command.payload.get("proposal")) is dict
                    and type(observation) is dict and type(known) is bool,
                    "explicit frontend disposition lost its approved command")
            current_host = context.frontend_host()
            change_id = command.payload["proposal"].get("change_id")
            require(type(current_host) is dict and type(command.host) is dict
                    and _frontend_json(command.host) == _frontend_json(current_host)
                    and type(change_id) is str and re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", change_id),
                    "explicit frontend disposition lost its current source or change")
            return FrontendSubmissionResult(copy.deepcopy(observation), copy.deepcopy(current_host), change_id,
                FrontendSubmissionDisposition.KNOWN if known else FrontendSubmissionDisposition.UNKNOWN)

        context = SimpleNamespace(root=directory / "reading", deployment={"slug": "reading-fixture"},
            runtime=runtime, frontend_host=lambda: copy.deepcopy(host), submit_frontend_input=submit,
            telegram_transport_factory=transport_factory, before_frontend_reply_entry=before_reply_entry,
            frontend_submission_result=frontend_submission_result)
        integration = build(context)
        await integration.start()
        if args.management:
            # This is the same application's ordinary service export and Store,
            # not a second integration or a native model-tool dispatch. Seed only
            # this isolated fixture's local collection through its existing owner.
            service_instance, service_store = integration, integration.store
            service_tools = [tool for tool in integration.tools_for_agent({"identity": IDENTITY})
                             if tool["name"] == "collection_snapshot"]
            require(len(service_tools) == 1 and callable(service_tools[0]["handler"]),
                    "Reading service export is missing or ambiguous")
            service_tool = service_tools[0]
            service_handler, service_arguments = service_tool["handler"], {}
            service_selection = {key: copy.deepcopy(service_tool[key])
                                 for key in ("name", "description", "schema")}
            seed = service_store.apply({"operation": "add", "request_id": "reading-role-isolation-seed",
                "title": "Persistent Reading fixture", "author": "Synthetic fixture author",
                "evidence": "Isolated service-role fixture data, not a user action or approval"})
            require(seed.get("status") == "saved" and seed.get("revision") == 1,
                    "Reading service fixture seed did not commit")
            service_path = service_store.path.resolve()
            service_file = service_path.stat()
            before_service = service_handler(service_arguments)
            require(isinstance(before_service, dict) and len(before_service.get("books", [])) == 1,
                    "Reading service did not return its nonempty stored collection")
            book = before_service["books"][0]
            require(book.get("id") == seed["book_id"] and book.get("revision") == 1
                    and book.get("title") == "Persistent Reading fixture"
                    and book.get("author") == "Synthetic fixture author"
                    and book.get("status") == "wishlist"
                    and len(book.get("events", [])) == 1
                    and book["events"][0]["id"] == seed["event_id"],
                    "Reading service result did not retain the actual seeded item and event")
            report["service_role"] = {
                "scope": "same application service export and Store; not native tool authorization or model dispatch",
                "selection": service_selection, "arguments": copy.deepcopy(service_arguments),
                "seed_receipt": seed, "before_frontend_setup": copy.deepcopy(before_service)}
        front = ConsoleFront(context.root, v.vacant_port(), "fixture-human", PASSWORD, runtime_namespace="meta")
        front.frontend_host = context.frontend_host
        front.frontend_private_fields = ("READING_TELEGRAM_BOT_TOKEN",)
        async def observed_management(request):
            # Observe the real protected route. The fixture pipe below never
            # supplies configuration, approval, or a management command.
            row = {"operation": request.operation, "approved": request.approved}
            if request.operation == "prepare":
                row["configuration"] = copy.deepcopy(request.payload["configuration"])
            elif request.operation == "submit":
                row["configuration"] = copy.deepcopy(request.payload["proposal"]["configuration"])
            management_calls.append(row)
            result = await integration.frontend_management(request)
            if type(result) is FrontendSubmissionResult:
                row["result"] = copy.deepcopy(result.observation)
                row["submission_outcome"] = {"change_id": result.change_id, "disposition": result.disposition.value}
                row["submission_host"] = copy.deepcopy(result.host)
            else:
                row["result"] = copy.deepcopy(result)
            return result

        front.frontend_management = observed_management
        await front.start(runtime.rust_http_base_url)
        if args.cli_owner:
            reader = asyncio.StreamReader(limit=256 * 1024)
            protocol = asyncio.StreamReaderProtocol(reader)
            control_transport, _ = await asyncio.get_running_loop().connect_read_pipe(lambda: protocol, sys.stdin.buffer)
            report["management_route_observations"] = management_calls

        async def external_phase(label, observation):
            print(json.dumps({"phase": label, "observation": observation}), flush=True)
            line = await asyncio.wait_for(reader.readline(), 360)
            require(0 < len(line) <= 256 * 1024, "CLI fixture release was missing or exceeded bound")
            value = json.loads(line)
            require(isinstance(value, dict) and set(value) <= {"release", "client_exit"}
                    and value.get("release") == label, "CLI fixture released another observation phase")
            return value
        report["phase"] = "protected_setup_client"
        if args.cli_owner:
            released = await external_phase("setup-client-exited", {"origin": front.origin, "native_pid": native_pid})
            client_exit = released.get("client_exit")
            require(isinstance(client_exit, dict) and set(client_exit) == {"pid", "exit_code"}
                    and type(client_exit["pid"]) is int and client_exit["pid"] > 0 and client_exit["exit_code"] == 0,
                    "actual CLI setup exit was not observed by its harness")
            setup_submits = [row for row in management_calls if row["operation"] == "submit"]
            require(len(setup_submits) == 1 and setup_submits[0]["approved"] is True
                    and setup_submits[0]["configuration"] == CONFIGURATION
                    and setup_submits[0]["result"]["status"] == "committed"
                    and setup_submits[0]["result"]["revision"] == 1
                    and setup_submits[0]["submission_outcome"] == {
                        "change_id": setup_submits[0]["result"]["change_id"], "disposition": "known"}
                    and setup_submits[0]["submission_host"] == host,
                    "actual CLI did not commit the selected application configuration")
            report["setup_client"] = {**client_exit, "observer": "actual_cli_harness",
                                      "result": setup_submits[0]["result"]}
        else:
            child = await asyncio.create_subprocess_exec(sys.executable, "-I", "-B", str(Path(__file__).resolve()),
                "--setup-client", front.origin, stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=environment, cwd=directory)
            report["setup_client"] = {"pid": child.pid, "exit_code": None, "result": None}
            stdout, stderr = await asyncio.wait_for(child.communicate(), 60.0)
            report["setup_client"].update(exit_code=child.returncode, result=json.loads(stdout))
            require(child.returncode == 0 and not stderr, "protected setup client did not exit normally")
        require(not provider.requests and not admissions and not telegram.sends,
                "native input or reply occurred during setup")
        require(transport._process.pid == native_pid and transport._process.poll() is None
                and integration.telegram.task is not None and not integration.telegram.task.done(),
                "application owner did not outlive setup client")
        report["phase"] = "native_frontend_component_open"
        # The committed application owns this selected component. Native Ready
        # opens the activation only; provider input also requires its component.
        opened = await entered_call(runtime.host_control.open_component, CHANNEL)
        report["native_component"] = {"incarnation": str(opened.incarnation),
            "component": opened.component, "revision": opened.revision, "state": opened.state.value}
        require(opened.component == CHANNEL and opened.state.value == "open",
                "native frontend component did not open")
        report["phase"] = "frontend_input_and_reply"
        telegram.release()
        deadline = time.monotonic() + 60
        while integration.telegram.reply is None:
            require(time.monotonic() < deadline and not provider.errors and not telegram.errors,
                    "bounded native input/reply did not settle")
            await asyncio.sleep(.05)
        reply = integration.telegram.reply
        require(len(admissions) == 1 and len(provider.requests) == 1 and len(telegram.sends) == 1,
                "native admission, model work or provider send count differs")
        report["phase"] = "native_completion_and_receipt"
        actual = admissions[0]["outcome"]
        completion = (await runtime.turn_result(IDENTITY, actual["turn_ticket"])).to_dict()
        require(completion["identity"] == IDENTITY and completion["ticket"] == actual["turn_ticket"]
                and completion["state"] == "completed" and completion["output_status"] == "text"
                and completion["output"] == ANSWER and completion["output_truncated"] is False,
                "actual native completed output differs")
        require(reply["recipient"] == str(USER) and reply["turn_ticket"] == actual["turn_ticket"]
                and reply["receipt"]["outcome"] == "provider_accepted"
                and reply["receipt"]["provider_message_id"] == "901", "destination-bound reply not accepted")
        report.update(completion=completion, reply=reply, owner_survived_setup_exit=True)
        if args.management:
            report["phase"] = "protected_native_management"
            unrelated_before = await runtime.status("helper")
            require(unrelated_before.identity == "helper" and unrelated_before.session_id is not None,
                    "unrelated configured native identity has no canonical session")
            unrelated_selection = {"identity": unrelated_before.identity, "state": unrelated_before.state,
                                   "session_id": unrelated_before.session_id}
            require(os.environ.get("READING_TELEGRAM_BOT_TOKEN") == TOKEN,
                    "declared shared private field was lost before management")
            report["management"] = {"scope": "actual CLI through protected application route" if args.cli_owner else "protected HTTP application route with real native actor; not Connect CLI or rendered Console UI",
                                    "clients": [], "snapshots": []}

            async def launch_client(action):
                process = await asyncio.create_subprocess_exec(sys.executable, "-I", "-B", str(Path(__file__).resolve()),
                    "--management-client", front.origin, action, stdin=asyncio.subprocess.DEVNULL,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=environment, cwd=directory)
                task = asyncio.create_task(process.communicate())
                record = {"action": action, "pid": process.pid, "exit_code": None}
                report["management"]["clients"].append(record)
                management_children.append((process, task, record))
                return process, task, record

            async def finish_client(pending):
                process, task, record = pending
                stdout, stderr = await asyncio.wait_for(asyncio.shield(task), 60)
                record.update(exit_code=process.returncode)
                require(len(stdout) <= 256 * 1024, "management child output exceeded bound")
                record["result"] = json.loads(stdout)
                require(process.returncode == 0 and not stderr, "protected management client failed")
                require(record["result"]["status"] in {"observed", "committed"}, "management result was not observed")
                return record["result"]

            async def command(action):
                if not args.cli_owner:
                    return await finish_client(await launch_client(action))
                start = len(management_calls)
                await external_phase(action, {"native_pid": native_pid})
                rows = management_calls[start:]
                if action in {"inspect", "observe-settled"}:
                    observations = [row["result"] for row in rows if row["operation"] == "inventory" and "result" in row]
                    require(observations and not any(row["operation"] in {"prepare", "submit"} for row in rows),
                            "CLI status phase repeated a management mutation")
                    return {"status": "observed", "observation": observations[-1]}
                submissions = [row for row in rows if row["operation"] == "submit"]
                require(len(submissions) == 1 and submissions[0]["approved"] is True
                        and submissions[0]["configuration"] == {"component_id": CHANNEL, "action": action}
                        and submissions[0]["result"]["status"] == "committed"
                        and submissions[0]["submission_outcome"] == {
                            "change_id": submissions[0]["result"]["change_id"], "disposition": "known"}
                        and submissions[0]["submission_host"] == host,
                        "CLI did not submit exactly the selected approved action")
                actual = submissions[0]["result"]
                return {"status": "committed", "revision": actual["revision"], "observation": actual}

            def inbox():
                # Observe actual application-owned durable rows, without writes.
                with integration.frontends.connection(readonly=True) as db:
                    return [dict(row) for row in db.execute(
                        "SELECT bot_id,update_id,status,admission,completion FROM reading_telegram_inbox ORDER BY sequence")]

            def snapshot(label):
                rows = inbox()
                report["management"]["snapshots"].append({"label": label, "rows": rows,
                    "admissions": len(admissions), "model_requests": len(provider.requests), "sends": len(telegram.sends)})
                return rows

            async def observed(predicate, label):
                deadline = time.monotonic() + 60
                while not predicate():
                    require(time.monotonic() < deadline and not provider.errors and not telegram.errors, label)
                    await asyncio.sleep(.02)

            def committed(result, revision, state):
                value = result["observation"]
                require(result["status"] == "committed" and result["revision"] == revision
                        and value["revision"] == revision and value["management_state"] == state
                        and value["pending_work_policy"] == PENDING_POLICY
                        and value["management_observation"]["policy"] == {"state": state, "pending_work_policy": PENDING_POLICY},
                        "committed management projection differs")
                return value["management_observation"]

            committed(await command("pause"), 2, "paused")
            telegram.release(102)
            await observed(lambda: any(row["update_id"] == 102 and row["status"] == "queued" for row in inbox()),
                           "paused input was not retained from the real provider poll")
            require(len(admissions) == len(provider.requests) == len(telegram.sends) == 1,
                    "paused queued input entered native work or provider delivery")
            snapshot("paused_input_retained")
            committed(await command("resume"), 3, "enabled")
            await observed(lambda: integration.telegram.reply is not None and integration.telegram.reply["update_id"] == 102,
                           "resumed queued input did not receive its native reply")
            require(len(admissions) == len(provider.requests) == len(telegram.sends) == 2,
                    "resume replayed or lost the retained input")
            resumed = integration.telegram.reply
            second = admissions[1]["outcome"]
            second_completion = (await runtime.turn_result(IDENTITY, second["turn_ticket"])).to_dict()
            require(second_completion["identity"] == IDENTITY and second_completion["ticket"] == second["turn_ticket"]
                    and second_completion["state"] == "completed" and second_completion["output_status"] == "text"
                    and second_completion["output"] == ANSWER and second_completion["output_truncated"] is False
                    and resumed["recipient"] == str(USER) and resumed["turn_ticket"] == second["turn_ticket"]
                    and resumed["receipt"]["outcome"] == "provider_accepted"
                    and resumed["receipt"]["provider_message_id"] == "902", "resumed native reply differs")
            report["management"].update(resumed_completion=second_completion, resumed_reply=resumed)
            snapshot("resumed_input_delivered_once")

            telegram.release(103, 104)
            await observed(reply_waiting.is_set, "real native completion did not reach the pending-reply barrier")
            rows = snapshot("completed_reply_and_queued_input_before_disable")
            require([(row["update_id"], row["status"]) for row in rows][-2:] == [(103, "native-completed"), (104, "queued")]
                    and len(admissions) == len(provider.requests) == 3 and len(telegram.sends) == 2,
                    "disable did not begin with actual completed and queued work")
            pending = admissions[2]["outcome"]
            pending_completion = (await runtime.turn_result(IDENTITY, pending["turn_ticket"])).to_dict()
            require(pending_completion["identity"] == IDENTITY and pending_completion["ticket"] == pending["turn_ticket"]
                    and pending_completion["state"] == "completed" and pending_completion["output_status"] == "text"
                    and pending_completion["output"] == ANSWER and pending_completion["output_truncated"] is False,
                    "pending reply did not have a real completed native ticket")
            disabled = committed(await command("disable"), 4, "disabled")
            require(not release_reply.is_set() and len(telegram.sends) == 2
                    and (disabled["native_close"] or {}).get("state") != "settled",
                    "disable inferred settlement while an actual reply was held")
            release_reply.set()
            disabled_inventory = (await command("observe-settled"))["observation"]
            require(disabled_inventory["revision"] == 4 and disabled_inventory["management"] == {
                    "state": "disabled", "pending_work_policy": PENDING_POLICY}, "disabled owner projection changed")
            closed = disabled_inventory["native_close"]
            require(closed["state"] == "settled" and closed["observation"]["state"] == "settled"
                    and closed["observation"]["failure"] is None
                    and closed["observation"]["handle"]["incarnation"] == report["native_component"]["incarnation"]
                    and closed["observation"]["handle"]["scope"] == {"kind": "component", "component": CHANNEL}
                    and closed["application_drain"] == {"accepted_native_unsettled": 0, "provider_outcome_unknown": 0,
                        "pending_input_stopped": 1, "pending_reply_stopped": 1}, "disable did not join actual component and application work")
            rows = snapshot("disabled_work_stopped")
            require([(row["update_id"], row["status"]) for row in rows][-2:] == [(103, "disabled-reply-stopped"), (104, "disabled-pending-stopped")]
                    and len(admissions) == len(provider.requests) == 3 and len(telegram.sends) == 2,
                    "disabled work was admitted or delivered")
            removed = committed(await command("remove"), 5, "removed")
            require(removed["native_close"] == closed, "removal replaced the actual close outcome")
            final_inventory = (await command("inspect"))["observation"]
            require(final_inventory["revision"] == 5 and final_inventory["configuration"] is None
                    and final_inventory["management"]["state"] == "removed"
                    and final_inventory["native_close"] == closed, "removal did not clear only the Reading configuration")
            telegram.release(105)
            await command("inspect")
            require(not any(row["update_id"] == 105 for row in inbox()) and len(admissions) == len(provider.requests) == 3
                    and len(telegram.sends) == 2 and (integration.telegram.session is None or integration.telegram.session.done())
                    and transport._process.pid == native_pid
                    and transport._process.poll() is None, "removed frontend resumed work or lost its application owner")
            if args.cli_owner:
                released = await external_phase("management-client-exited", {"native_pid": native_pid})
                client_exit = released.get("client_exit")
                require(isinstance(client_exit, dict) and set(client_exit) == {"pid", "exit_code"}
                        and type(client_exit["pid"]) is int and client_exit["pid"] > 0 and client_exit["exit_code"] == 0
                        and client_exit["pid"] != report["setup_client"]["pid"],
                        "fresh actual CLI management process did not exit normally")
                report["management"]["cli_exit_observation"] = client_exit
                require(transport._process.pid == native_pid and transport._process.poll() is None,
                        "application owner did not outlive the actual CLI management process")
            unrelated_after = await runtime.status("helper")
            unrelated_observed = {"identity": unrelated_after.identity, "state": unrelated_after.state,
                                  "session_id": unrelated_after.session_id}
            require(unrelated_observed == unrelated_selection,
                    "Reading management changed another configured native identity")
            require(os.environ.get("READING_TELEGRAM_BOT_TOKEN") == TOKEN,
                    "Reading removal changed the shared declared private field")
            # Invoke the retained service callable only after the real frontend
            # removal and (in CLI mode) the management process have settled.
            require(integration is service_instance and integration.store is service_store
                    and service_store.path.resolve() == service_path,
                    "Reading frontend removal replaced the service instance or Store")
            current_file = service_path.stat()
            require((current_file.st_dev, current_file.st_ino) == (service_file.st_dev, service_file.st_ino),
                    "Reading frontend removal replaced the service database")
            current_tools = [tool for tool in integration.tools_for_agent({"identity": IDENTITY})
                             if tool["name"] == service_selection["name"]]
            require(len(current_tools) == 1 and current_tools[0]["handler"] == service_handler
                    and {key: current_tools[0][key] for key in service_selection} == service_selection,
                    "Reading frontend removal changed the independently selected service export")
            after_service = service_handler(service_arguments)
            require(service_arguments == {} and after_service == before_service,
                    "Reading service call after frontend removal lost its stored item or history")
            require(final_inventory["configuration"] is None
                    and final_inventory["management"]["state"] == "removed"
                    and len(admissions) == len(provider.requests) == 3 and len(telegram.sends) == 2,
                    "Reading service call restarted frontend work or changed native delivery")
            report["service_role"].update(after_frontend_removal=after_service,
                same_instance=True, same_store=True, same_database=True, same_export=True,
                frontend_state="removed", application_service_calls=2)
            report["management"].update(pending_completion=pending_completion, native_close=closed,
                final_inventory=final_inventory, owner_survived_management_clients=True,
                unrelated_identity_before=unrelated_selection, unrelated_identity_after=unrelated_observed,
                shared_private_field_preserved=True)
        report.update(status="passed", phase="completed", activation=integration.telegram.observation())
    except BaseException as error:
        report.update(status="failed", failure=failure(error, report["phase"]))
        if report["phase"] == "native_connect":
            # Preserve typed startup causes without formatting errors or payloads.
            if isinstance(error, GatewayStartupCleanupError):
                report["failure"]["startup_error_type"] = type(error.startup_error).__name__[:128]
                report["failure"]["cleanup_error_type"] = type(error.cleanup_error).__name__[:128]
            attempt = runtime.preparation_attempt if runtime is not None else None
            if attempt is not None and attempt.settlement is not None:
                native_result = attempt.settlement.native
                if isinstance(native_result, BaseException):
                    report["failure"]["preparation_native_error_type"] = type(native_result).__name__[:128]
    finally:
        # A failed assertion must not strand the actor behind the fixture barrier.
        release_reply.set()
        if control_transport is not None:
            control_transport.close()
        for process, task, record in management_children:
            if process.returncode is None:
                process.kill()
                report.update(status="failed", forced_management_client_cleanup=True)
            await task
            record["exit_code"] = process.returncode
        if integration is not None:
            try:
                report["activation"] = integration.telegram.observation()
            except BaseException as error:
                report.update(status="failed", observation_failure=failure(error, "activation_observation"))
        if child is not None and child.returncode is None:
            child.kill()
            await child.wait()
            report.update(status="failed", forced_setup_client_cleanup=True)
        for name, operation in (("application", integration.stop if integration else None),
                                ("front", front.close if front else None),
                                ("native", runtime.shutdown if runtime else None)):
            if operation is not None:
                try:
                    await operation()
                    report[name + "_joined"] = True
                except BaseException as error:
                    report.update(status="failed", **{name + "_cleanup_failure": failure(error, name + "_cleanup")})
        if runtime is not None:
            shutdown = [row for row in outgoing if row.get("method") == "meta/shutdown"]
            replies = [row for row in responses if shutdown and str(row.get("id")) == str(shutdown[0]["id"])]
            report["shutdown_responses"] = replies
            if (len(shutdown) != 1 or len(replies) != 1 or "error" in replies[0]
                    or not all(replies[0].get("result", {}).get(key) is True for key in
                        ("shutdown", "runtime_cleanup_completed", "host_cleanup_completed"))):
                report.update(status="failed", shutdown_failure="native cleanup acknowledgement missing")
            report["sdk_readers_joined"] = all(reader is not None and not reader.is_alive()
                for reader in (public_reader, private_reader))
            if not report["sdk_readers_joined"] or transport is None or transport._process is not None:
                report.update(status="failed", reader_failure="native transport or SDK reader did not join")
            termination = runtime.last_gateway_termination
            report["termination"] = dataclasses.asdict(termination) if termination is not None else None
            if termination is None or termination.state != "exited" or termination.exit_code != 0 or termination.signal is not None or termination.escalation != "none":
                report.update(status="failed", termination_failure="native termination was not a normal joined exit")
        for name, owner in (("model", provider), ("telegram", telegram)):
            if owner is not None:
                try:
                    owner.close(drain=True) if name == "model" else owner.close()
                    report[name + "_provider_joined"] = True
                except BaseException as error:
                    report.update(status="failed", **{name + "_provider_cleanup_failure": failure(error, name + "_provider_cleanup")})
        if telegram is not None:
            report["provider_calls"] = telegram.calls
            report["provider_errors"] = telegram.errors
            if telegram.errors:
                report["status"] = "failed"
        if provider is not None:
            report["model_errors"] = provider.errors
            if provider.errors:
                report["status"] = "failed"
        os.chdir(original_directory)
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        events.flush()
        require(TOKEN not in json.dumps(report) and PASSWORD not in json.dumps(report), "private literal entered report")
        v.save_json(directory / "result.json", report)
    return report


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--setup-client":
        try:
            setup_client(sys.argv[2])
            return 0
        except BaseException as error:
            print(json.dumps({"status": "failed", "failure": failure(error, "protected_setup_client")}), flush=True)
            return 1
    if len(sys.argv) == 4 and sys.argv[1] == "--management-client":
        try:
            setup_client(sys.argv[2], sys.argv[3])
            return 0
        except BaseException as error:
            print(json.dumps({"status": "failed", "failure": failure(error, "protected_management_client")}), flush=True)
            return 1
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--management", action="store_true", help="exercise protected pause/resume/disable/remove on the same native owner")
    parser.add_argument("--cli-owner", action="store_true", help="hold this same owner for the actual CLI harness; pipe controls observations only")
    for name in ("toolkit-gateway", "toolkit-python", "meta-python", "installed-staging", "sdk-selection", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--installed-staging-sha256", required=True)
    parser.add_argument("--sdk-selection-sha256", required=True)
    args = parser.parse_args()
    if args.cli_owner:
        args.management = True
    require(all(getattr(args, name).is_absolute() for name in ("toolkit_gateway", "toolkit_python", "meta_python", "installed_staging", "sdk_selection", "output")), "absolute selected paths required")
    require(not args.output.exists(), "evidence output must be fresh")
    root = HERE.parents[4]
    spec = importlib.util.spec_from_file_location("installed_verifier", root / "scripts/verify_native_gateway_subprocess.py")
    v = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(v)
    facade_bytes = v.exact_bytes(HERE / "native_console_auth.py", NATIVE_FACADE_SHA256)
    require(len(facade_bytes) == 83475, "native Reading facade byte length differs")
    sdk = json.loads(v.exact_bytes(args.sdk_selection, args.sdk_selection_sha256))
    require(sdk["toolkit_python"] == str(args.toolkit_python) and sdk["meta_python"] == str(args.meta_python), "selected SDK roots differ")
    for row in sdk["files"]:
        sdk_bytes = v.exact_bytes(Path(row["path"]), row["sha256"], limit=16 * 1024 * 1024, minimum=0)
        require(len(sdk_bytes) == row["bytes"], "selected SDK byte length differs")
    sys.path[0:0] = [str(args.toolkit_python), str(args.meta_python), str(HERE), str(HERE.parent / "forge-auth")]
    v.SDK = importlib.import_module("meerkat_toolkit.host_control")
    v.APP_RUNTIME = importlib.import_module("meerkat_toolkit.application_runtime")
    v.META_BUILDER = importlib.import_module("meerkat_meta.builder")
    v.META_IDENTITY = importlib.import_module("meerkat_meta.identity_first_models")
    args.mobkit_python = args.meta_python
    v.installed_candidate(args)
    args.output.mkdir(mode=0o700)
    report = asyncio.run(run(args, v))
    try:
        require(v.exact_bytes(HERE / "native_console_auth.py", NATIVE_FACADE_SHA256) == facade_bytes,
                "native Reading facade changed")
        require(v.installed_candidate(args)[0] == report["candidate"], "selected installed candidate changed")
        selected_files = {Path(row["path"]).resolve(): row for row in sdk["files"]}
        for row in sdk["files"]:
            sdk_bytes = v.exact_bytes(Path(row["path"]), row["sha256"], limit=16 * 1024 * 1024, minimum=0)
            require(len(sdk_bytes) == row["bytes"], "selected SDK byte length differs")
        for name, module in sorted(sys.modules.items()):
            if name.split(".")[0] not in ("meerkat_toolkit", "meerkat_meta") or not getattr(module, "__file__", None):
                continue
            path = Path(module.__file__).resolve()
            require(path in selected_files, "native SDK import escaped exact selected files")
            report["imports"].append({"module": name, **selected_files[path]})
    except BaseException as error:
        report.update(status="failed", post_input_failure=failure(error, "post_input_verification"))
    v.save_json(args.output / "result.json", report)
    if args.cli_owner:
        print(json.dumps({"phase": "finished", "status": report["status"], "result_path": str(args.output / "result.json")}), flush=True)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
