"""Saved setups, the guided conversation, an interactive chat, and the plan."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re
import time
import uuid

from .. import refusal, setup_version
from ..client import UUID, short
from ..errors import CliError, step
from ..render import (json_text, last_guide_message, plan_lines, plan_summary, question_lines, setup_lines,
                      setup_name)
from ..review import approve, organization_document, plan_document, sha256_of
from .host import state_directory

TOPICS = ("purpose", "connections", "knowledge", "autonomy", "destination")
FINAL = {"completed", "failed", "interrupted"}
ATTACHMENT_SUFFIXES = {".png", ".jpg", ".jpeg", ".webp", ".gif", ".txt", ".md", ".markdown", ".csv", ".json", ".pdf"}
MAX_ATTACHMENTS = 4
MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024
MAX_TURN_BYTES = 16 * 1024 * 1024
MAX_TEXT = 6000
MAX_BLUEPRINT_BYTES = 1_000_000
ANSWER = re.compile(r"\s*([0-9]+(?:\s*,\s*[0-9]+)*)(?:\s+(.*))?", re.S)
CHAT_HELP = """\
In this chat:
  a number answers the guide's question (1,3 when more than one fits); text
  after the number adds context. Plain text answers in your own words, or
  talks to the guide when no question is open.
  /plan            show the current plan
  /accept          review and accept the proposed plan
  /attach FILE     attach a file to your next message
  /revisit TOPIC   reopen a discussed topic (purpose, connections, knowledge, autonomy, destination)
  /retry           resend the saved answer of an interrupted or failed turn
  /status          show this setup's status
  /open            print browser links (Forge, the guide's console)
  /quit            leave the chat (the setup is saved)"""


def guide_path(identity, suffix=""):
    return f"/api/guide/{identity}{suffix}"


def timeline_limit(value):
    if not re.fullmatch(r"[1-9][0-9]{0,2}", value) or int(value) > 500:
        raise refusal("use a number from 1 to 500")
    return int(value)


def show_step(identity):
    return step(f"meta-forge setup show {identity}", "Read the setup's current state and version.")


async def read_state(client, identity):
    return await client.get(guide_path(identity))


def require_version(state, expected):
    if expected is not None and state.get("version") != expected:
        raise CliError("stale_version", f"Setup {short(state.get('id'))} is at version {state.get('version')}, "
                       f"not {expected}. Read it again before changing it.", next=[show_step(state.get("id"))])


def require_idle(state):
    if (state.get("operation") or {}).get("status") == "running":
        raise CliError("busy", "The guide is still answering this setup's last message.",
                       next=[step(f"meta-forge setup show {state.get('id')} --wait", "Wait for the guide's answer.")])


async def classified(client, identity, sent_version, error):
    """A refused version-checked write is stale_version when the setup moved on; otherwise it stays as it was."""
    if error.code != "conflict":
        return error
    try:
        current = await read_state(client, identity)
    except CliError:
        return error
    if current.get("version") != sent_version:
        error.code = "stale_version"
    if not error.next:
        error.next = [show_step(identity)]
    return error


def question_need(state):
    question = state.get("question")
    if not isinstance(question, dict):
        return []
    return [{"kind": "question", "question": question, "person_runs": f"meta-forge chat {state.get('id')}"}]


def frame_summary(frame):
    kind = frame.get("kind")
    payload = frame.get("payload") if isinstance(frame.get("payload"), dict) else {}
    for key in ("text", "content", "message", "name", "tool_name", "tool"):
        value = payload.get(key)
        if isinstance(value, str) and value.strip():
            return f"{kind}: {' '.join(value.split())[:200]}"
    return str(kind)


async def latest_cursor(client, identity):
    try:
        value = await client.get(guide_path(identity, "/timeline"), params={"limit": 1})
    except CliError:
        return None
    return value.get("latest_cursor") if isinstance(value, dict) else None


async def wait_turn(context, client, identity, admitted, *, cursor=None):
    """Poll the setup as the browser does until the admitted turn is final."""
    operation = admitted.get("operation") or {}
    request_id = operation.get("request_id")
    context.resume = [step(f"meta-forge setup show {identity} --wait", "Wait for the guide's answer again.")]
    follow = context.output.mode == "jsonl" or context.output.verbose > 0
    context.output.event(context.command, "progress", status="running",
                         message="the guide is working on its answer...")
    state, next_state, next_timeline = admitted, 0.0, 0.0
    while True:
        now = time.monotonic()
        if follow and now >= next_timeline:
            try:
                page = await client.get(guide_path(identity, "/timeline"),
                                        params={"limit": 500, **({"after": cursor} if cursor else {})})
                for frame in page.get("frames") or []:
                    context.output.event(context.command, "log", cursor=frame.get("cursor"), kind=frame.get("kind"),
                                         message=frame_summary(frame))
                cursor = page.get("next_cursor") or cursor
            except CliError:
                pass
            next_timeline = now + 1.0 * context.poll_scale
        if now >= next_state:
            state = await read_state(client, identity)
            current = state.get("operation") or {}
            if not current or current.get("status") in FINAL or (request_id and current.get("request_id") != request_id):
                return state
            next_state = now + 2.5 * context.poll_scale
        await context.sleep(1.0 if follow else 2.5)


def turn_outcome(state):
    operation = state.get("operation") or {}
    identity = state.get("id")
    if operation.get("status") == "failed":
        raise CliError("operation_failed", operation.get("detail") or "The guide could not finish this answer.",
                       result=state, next=[step(f"meta-forge setup retry {identity}", "Resend the saved answer."),
                                           show_step(identity)])
    if operation.get("status") == "interrupted":
        raise CliError("operation_interrupted", operation.get("detail") or "The guide's turn was interrupted.",
                       result=state, next=[step(f"meta-forge setup retry {identity}", "Resend the saved answer.")])


def same_request(saved, body):
    """Whether a saved operation request is this body, in the server's normalized form."""
    if not isinstance(saved, dict) or saved.get("version") != body.get("version"):
        return False
    defaults = {"action": "answer", "text": "", "selected": [], "stage": None, "combine_options": False,
                "organization_fingerprint": None}
    if any(saved.get(key, default) != body.get(key, default) for key, default in defaults.items()):
        return False
    return ([item.get("id") for item in saved.get("attachments") or []]
            == [item.get("id") for item in body.get("attachments") or []])


async def recovered_turn(client, identity, body):
    """After a lost answer, the setup's own operation record says what happened to this request."""
    try:
        state = await read_state(client, identity)
    except CliError:
        return None
    operation = state.get("operation") or {}
    if same_request(operation.get("request"), body) and operation.get("status") in (*FINAL, "running"):
        return state
    return None


async def send_turn(context, client, identity, body, *, wait):
    """POST one guide turn; wait for it when asked. Returns (state, pending)."""
    cursor = await latest_cursor(client, identity) if wait and (
        context.output.mode == "jsonl" or context.output.verbose) else None
    context.resume = [step(f"meta-forge setup show {identity} --wait",
                           "Read whether the guide received the message; it is never sent again for you.")]
    try:
        status, state = await client.post(guide_path(identity, "/respond"), json=body)
    except CliError as error:
        error = await classified(client, identity, body.get("version"), error)
        recovered = await recovered_turn(client, identity, body) if error.code == "outcome_unknown" else None
        if recovered is None:
            if error.code == "outcome_unknown" and not error.next:
                error.next = list(context.resume)
            raise error from None
        state = recovered
        status = 202 if (state.get("operation") or {}).get("status") == "running" else 200
        turn_outcome(state)
    if status == 202 and wait:
        state = await wait_turn(context, client, identity, state, cursor=cursor)
        turn_outcome(state)
        return state, False
    return state, status == 202


def option_ids(question, values):
    """--answer values: option IDs, or their 1-based numbers."""
    options = (question or {}).get("options") or []
    ids = [option.get("id") for option in options]
    selected = []
    for value in values:
        if value in ids:
            selected.append(value)
        elif value.isdigit() and 1 <= int(value) <= len(ids):
            selected.append(ids[int(value) - 1])
        else:
            raise CliError("usage", "An --answer value is not an option of the current question. Options: "
                           + (", ".join(f"{index}={option}" for index, option in enumerate(ids, start=1)) or "none")
                           + ".", next=[step("meta-forge setup show SETUP", "Read the current question.")])
    if len(set(selected)) != len(selected):
        raise CliError("usage", "Choose each option once.", next=[step("meta-forge setup show SETUP",
                                                                         "Read the current question.")])
    return selected


def private_roots(context):
    """Folders this command never reads from: Forge's state folder, and Meerkat's configuration folders,
    which hold its credential store (<config>/meerkat/credentials) and its user configuration (~/.rkat)."""
    home = Path(context.environ.get("HOME") or Path.home())
    roots = [state_directory(context), home / ".rkat", home / ".config" / "meerkat",
             home / "Library" / "Application Support" / "meerkat"]
    configured = context.environ.get("XDG_CONFIG_HOME")
    if configured and os.path.isabs(configured):
        roots.append(Path(configured) / "meerkat")
    return roots


def private_path(path, roots):
    """Forge's private files by name (saved provider keys, an installation's .env and .mobkit files), or a
    path inside one of the private folders. Names and folders compare without case."""
    name = path.name.casefold()
    if name == "provider-settings.json" or name.startswith(".env") or ".mobkit" in (
            part.casefold() for part in path.parts):
        return True
    text = str(path).casefold()
    return any(text == root or text.startswith(root.rstrip(os.sep) + os.sep) for root in roots)


def named_file(context, value, option):
    """The one guard of every option that reads a file a person names (--attach, --text-file, /attach,
    plan validate). It decides from the path alone, as given and with links resolved, before anything is
    opened: Forge's private files and Meerkat's credential store are never read. Returns (given, resolved).
    Errors name the option, never the value: a secret may have been typed in its place."""
    help_command = "meta-forge plan validate --help" if option == "plan validate" else "meta-forge setup send --help"
    given = Path(os.path.abspath(os.path.expanduser(value)))
    try:
        resolved = given.resolve()
    except (OSError, RuntimeError):
        raise CliError("usage", f"A file given to {option} cannot be read.",
                       next=[step(help_command, "See which files this command reads.")]) from None
    roots = set()
    for root in private_roots(context):
        absolute = Path(os.path.abspath(root))
        roots.add(str(absolute).casefold())
        try:
            roots.add(str(absolute.resolve()).casefold())
        except (OSError, RuntimeError):
            pass
    if private_path(given, roots) or private_path(resolved, roots):
        raise CliError("invalid_request", f"A file given to {option} is one of Forge's private files or in "
                       "Meerkat's credential store; this command never reads it.",
                       next=[step(help_command, "See which files this command reads.")])
    return given, resolved


async def upload(context, client, identity, values, option="--attach"):
    if len(values) > MAX_ATTACHMENTS:
        raise CliError("usage", f"Attach at most {MAX_ATTACHMENTS} files to one message.",
                       next=[step("meta-forge setup send --help", "See the attachment limits.")])
    files, total = [], 0
    for value in values:
        given, path = named_file(context, value, option)
        if given.suffix.lower() not in ATTACHMENT_SUFFIXES:
            raise CliError("invalid_request", f"Attach PNG, JPEG, WebP, GIF, text, Markdown, CSV, JSON or a text "
                           f"PDF with {option}.",
                           next=[step("meta-forge setup send --help", "See which files can be attached.")])
        try:
            info = path.stat()
        except OSError:
            raise CliError("usage", f"A file given to {option} cannot be read.",
                           next=[step("meta-forge setup send --help", "See how to attach files.")]) from None
        if not path.is_file() or not 1 <= info.st_size <= MAX_ATTACHMENT_BYTES:
            raise CliError("invalid_request", f"Each file given to {option} is a nonempty file of at most 8 MiB.",
                           next=[step("meta-forge setup send --help", "See the attachment limits.")])
        total += info.st_size
        if total > MAX_TURN_BYTES:
            raise CliError("invalid_request", "Attachments of one message together must be at most 16 MiB.",
                           next=[step("meta-forge setup send --help", "See the attachment limits.")])
        try:
            content = path.read_bytes()
        except OSError:
            raise CliError("usage", f"A file given to {option} cannot be read.",
                           next=[step("meta-forge setup send --help", "See how to attach files.")]) from None
        files.append(("files", (given.name, content, "application/octet-stream")))
    _, value = await client.post(guide_path(identity, "/attachments"), files=files)
    return list(value.get("attachments") or [])


def message_text(context, args):
    text = args.text or ""
    if args.text_file:
        _, path = named_file(context, args.text_file, "--text-file")
        try:
            with path.open("rb") as stream:
                raw = stream.read(MAX_TEXT * 4 + 1)
            text = raw.decode("utf-8")
        except (OSError, UnicodeError):
            raise CliError("usage", "The file given to --text-file cannot be read as UTF-8 text.",
                           next=[step("meta-forge setup send --help", "See the message options.")]) from None
    if len(text) > MAX_TEXT:
        raise CliError("usage", f"A message has at most {MAX_TEXT} characters.",
                       next=[step("meta-forge setup send --help", "See the message options.")])
    return text


def turn_body(state, *, selected, text, combine, attachments):
    """The respond body the browser sends for the same step."""
    question = state.get("question")
    files_only = bool(attachments) and not text.strip() and not selected
    action = "answer" if question and not files_only else "chat"
    if selected and not question:
        raise CliError("usage", "There is no open question to answer; send text instead.",
                       next=[show_step(state.get("id"))])
    if combine and (not question or question.get("multiple") or len(selected) < 2):
        raise CliError("usage", "--combine needs two or more answers to a single-choice question.",
                       next=[show_step(state.get("id"))])
    if action == "answer" and not selected and not text.strip():
        raise CliError("usage", "Choose an option or say what you have in mind.", next=[show_step(state.get("id"))])
    if action == "chat" and not text.strip() and not attachments:
        raise CliError("usage", "Say what you would like to discuss.", next=[show_step(state.get("id"))])
    if attachments and not text.strip() and not selected:
        text = "Consider the attached evidence."
    body = {"version": state["version"], "action": action, "selected": list(selected), "text": text}
    if combine:
        body["combine_options"] = True
    if attachments:
        body["attachments"] = attachments
    return body


def reply_lines(state, *, chat=False):
    lines = []
    message = last_guide_message(state)
    if message:
        lines.append(f"Guide: {message}")
    changes = state.get("changes") or []
    plan = state.get("proposal") or state.get("blueprint")
    if changes and plan:
        summary = plan_summary(plan)
        agents = sum(len(item["identities"]) for item in summary["classes"])
        lines.append(f"Proposal (version {state.get('version')}): {agents} agents, entrypoint {summary['entrypoint']}"
                     + (" (/plan shows it)" if chat else f" (meta-forge plan show {short(state.get('id'))})"))
        lines.append("  Changes: " + "; ".join(str(item) for item in changes))
    lines.extend(question_lines(state.get("question"), chat=chat))
    return lines


def register(commands, command_parser):
    setup = command_parser(commands, "setup", "Saved setups and the guided conversation.")
    actions = setup.add_subparsers(dest="action", metavar="ACTION", required=True)
    parser = command_parser(actions, "list", "Saved setups with their stage, status and installation.")
    parser.add_argument("--all", action="store_true", help="every setup, not only the newest 100")
    parser.set_defaults(func=setup_list, name="setup list")
    parser = command_parser(actions, "show", "A setup's status, current question, readiness and plan.")
    parser.add_argument("setup", metavar="SETUP")
    parser.set_defaults(func=setup_show, name="setup show")
    parser = command_parser(actions, "new", "Create a setup; repeating the same --request-id returns the same setup.")
    parser.add_argument("--request-id", metavar="UUID", help="creation request ID (a fresh one by default)")
    parser.set_defaults(func=setup_new, name="setup new")
    parser = command_parser(actions, "send", "Send one turn to the guide and, with --wait, show its answer.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--answer", action="append", default=[], metavar="ID",
                        help="an option of the current question, by ID or number (repeatable)")
    parser.add_argument("--combine", action="store_true",
                        help="more than one option of a single-choice question fits")
    text = parser.add_mutually_exclusive_group()
    text.add_argument("--text", metavar="TEXT", help="your answer or message")
    text.add_argument("--text-file", metavar="FILE", help="read the message from a UTF-8 file")
    parser.add_argument("--attach", action="append", default=[], metavar="FILE",
                        help="attach a file: PNG, JPEG, WebP, GIF, text, Markdown, CSV, JSON or text PDF (up to 4)")
    parser.add_argument("--version", type=setup_version, metavar="N", help="the setup version you read")
    parser.set_defaults(func=setup_send, name="setup send")
    parser = command_parser(actions, "revisit", "Reopen a discussed topic (no guide turn).")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("topic", choices=TOPICS, metavar="TOPIC", help="one of: " + ", ".join(TOPICS))
    parser.add_argument("--version", type=setup_version, metavar="N", help="the setup version you read")
    parser.set_defaults(func=setup_revisit, name="setup revisit")
    parser = command_parser(actions, "retry", "Deliberately resend the saved answer of an interrupted or failed turn.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--version", type=setup_version, metavar="N", help="the setup version you read")
    parser.set_defaults(func=setup_retry, name="setup retry")
    parser = command_parser(actions, "timeline", "The guide's conversation frames.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--after", metavar="CURSOR", help="only frames after this cursor (console:N)")
    parser.add_argument("--limit", type=timeline_limit, default=500, metavar="N", help="frames per page, 1 to 500")
    parser.set_defaults(func=setup_timeline, name="setup timeline")
    parser = command_parser(actions, "organization", "Adopt this Forge's updated workplace guidance for the setup.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--version", type=setup_version, metavar="N", help="the setup version you read")
    parser.set_defaults(func=setup_organization, name="setup organization")

    chat = command_parser(commands, "chat", "Talk to the guide in this terminal.")
    chat.add_argument("setup", nargs="?", metavar="SETUP", help="the setup to continue")
    chat.add_argument("--new", action="store_true", help="start a new setup")
    chat.set_defaults(func=chat_command, name="chat")

    plan = command_parser(commands, "plan", "The setup's plan (blueprint).")
    actions = plan.add_subparsers(dest="action", metavar="ACTION", required=True)
    parser = command_parser(actions, "show", "Classes, identities, tools, connections, flows and open items.")
    parser.add_argument("setup", metavar="SETUP")
    parser.set_defaults(func=plan_show, name="plan show")
    parser = command_parser(actions, "accept", "Review and accept the proposed plan (Use this plan).")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--version", type=setup_version, metavar="N", help="the setup version you reviewed")
    parser.set_defaults(func=plan_accept, name="plan accept")
    parser = command_parser(actions, "export", "The accepted (or proposed) blueprint as JSON.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--out", metavar="FILE", help="write it to this new file")
    parser.set_defaults(func=plan_export, name="plan export")
    parser = command_parser(actions, "validate", "Validate a blueprint file without a model.")
    parser.add_argument("file", metavar="FILE")
    parser.set_defaults(func=plan_validate, name="plan validate")
    parser = command_parser(actions, "package", "Download the accepted plan's agent team package.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--out", metavar="FILE", required=True, help="write the package to this new file")
    parser.add_argument("--version", type=setup_version, metavar="N", help="the setup version you read")
    parser.set_defaults(func=plan_package, name="plan package")


async def installation_summary(client, identity):
    try:
        receipt = await client.get(guide_path(identity, "/deployment"))
    except CliError as error:
        if error.code == "not_found" and error.detail == "No local deployment has been started for this setup.":
            return None
        if error.code in ("server_unavailable", "server_incompatible"):
            raise
        return {"unknown": True, "detail": error.detail}
    if not isinstance(receipt, dict):
        return {"unknown": True, "detail": "The installation record could not be read."}
    return {key: receipt.get(key) for key in ("deployment_id", "status", "health", "destination", "execution_mode",
                                              "source_kind")}


async def setup_list(context):
    client = await context.client()
    setups = await client.list_setups(include_all=context.args.all)
    limit = asyncio.Semaphore(3)

    async def summarized(item):
        async with limit:
            return {**item, "installation": await installation_summary(client, item["id"])}

    rows = await asyncio.gather(*(summarized(item) for item in setups if isinstance(item, dict)))
    lines = [f"{'SETUP':<9} {'STATUS':<9} {'STAGE':<12} {'UPDATED':<20} NAME / INSTALLATION"]
    for row in rows:
        installation = row["installation"]
        where = ("none" if installation is None else f"status unknown ({installation['detail']})"
                 if installation.get("unknown") else f"{installation.get('status')} at {installation.get('destination')}")
        lines.append(f"{short(row['id']):<9} {str(row.get('status')):<9} {str(row.get('stage')):<12} "
                     f"{str(row.get('updated_at'))[:19]:<20} {row.get('name')}")
        lines.append(f"{'':<52} installation: {where}")
    if not rows:
        lines = ["No saved setups yet."]
    context.output.success("setup list", {"setups": rows}, lines=lines,
                           next=[] if rows else [step("meta-forge chat --new", "Start a setup with the guide.")])
    return 0


async def setup_show(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    state = await read_state(client, identity)
    if context.wait and (state.get("operation") or {}).get("status") == "running":
        state = await wait_turn(context, client, identity, state)
        turn_outcome(state)
    context.output.success("setup show", state, lines=setup_lines(state), setup=identity, needs=question_need(state))
    return 0


async def create_setup(context, client, request=None):
    request = request or str(uuid.uuid4())
    if not UUID.fullmatch(request):
        raise CliError("usage", "--request-id is a lowercase UUID such as 67f2f55d-8fd4-4d92-b999-14fb240e5730.",
                       next=[step("meta-forge setup new", "Create a setup with a fresh request ID.")])
    again = step(f"meta-forge setup new --request-id {request}",
                 "Repeat the creation with the same request ID; it never creates a second setup.")
    context.output.note(f"Creation request: {request}")
    context.output.event(context.command, "admitted", request=request)
    context.resume = [again]
    context.request = request
    try:
        _, state = await client.post("/api/guide/start", json={"start_request_id": request})
    except CliError as error:
        error.request = request
        if error.code == "outcome_unknown":
            error.next = [again]
        raise
    return state, request


async def setup_new(context):
    client = await context.client()
    state, request = await create_setup(context, client, context.args.request_id)
    context.output.success("setup new", state, lines=setup_lines(state), setup=state.get("id"), request=request,
                           next=[step(f"meta-forge chat {state.get('id')}", "Talk to the guide about this setup."),
                                 step(f"meta-forge setup send {state.get('id')} --text TEXT --wait",
                                      "Send the first message without a chat.")])
    return 0


async def setup_send(context):
    args = context.args
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    state = await read_state(client, identity)
    require_version(state, args.version)
    require_idle(state)
    selected = option_ids(state.get("question"), args.answer)
    text = message_text(context, args)
    attachments = await upload(context, client, identity, args.attach) if args.attach else []
    body = turn_body(state, selected=selected, text=text, combine=args.combine, attachments=attachments)
    state, pending = await send_turn(context, client, identity, body, wait=context.wait)
    if pending:
        context.output.success("setup send", state, lines=["The guide is working on its answer."], setup=identity,
                               next=[step(f"meta-forge setup show {identity} --wait", "Wait for the answer.")])
        return 0
    context.output.success("setup send", state, lines=reply_lines(state), setup=identity, needs=question_need(state))
    return 0


async def setup_revisit(context):
    args = context.args
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    state = await read_state(client, identity)
    require_version(state, args.version)
    require_idle(state)
    body = {"version": state["version"], "action": "back", "selected": [], "text": "", "stage": args.topic}
    state, _ = await send_turn(context, client, identity, body, wait=True)
    context.output.success("setup revisit", state, lines=reply_lines(state), setup=identity,
                           needs=question_need(state))
    return 0


async def setup_retry(context):
    args = context.args
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    state = await read_state(client, identity)
    require_version(state, args.version)
    operation = state.get("operation") or {}
    if operation.get("status") not in ("failed", "interrupted") or not isinstance(operation.get("request"), dict):
        raise CliError("invalid_request", "There is no interrupted or failed turn to resend "
                       f"(the last turn is {operation.get('status') or 'absent'}).", next=[show_step(identity)])
    # The browser's Retry saved answer: the saved request at the current version.
    body = {**operation["request"], "version": state["version"]}
    state, pending = await send_turn(context, client, identity, body, wait=context.wait)
    if pending:
        context.output.success("setup retry", state, lines=["The guide is working on the resent answer."],
                               setup=identity,
                               next=[step(f"meta-forge setup show {identity} --wait", "Wait for the answer.")])
        return 0
    context.output.success("setup retry", state, lines=reply_lines(state), setup=identity, needs=question_need(state))
    return 0


async def setup_timeline(context):
    args = context.args
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    params = {"limit": args.limit, **({"after": args.after} if args.after else {})}
    value = await client.get(guide_path(identity, "/timeline"), params=params)
    lines = [f"{frame.get('cursor')}  {frame_summary(frame)}" for frame in value.get("frames") or []]
    if not value.get("available", True):
        lines = ["No conversation frames yet."]
    if value.get("next_cursor") and not value.get("exhausted", True):
        lines.append(f"(more: meta-forge setup timeline {short(identity)} --after {value['next_cursor']})")
    context.output.success("setup timeline", value, lines=lines, setup=identity)
    return 0


async def setup_organization(context):
    args = context.args
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    state = await read_state(client, identity)
    require_version(state, args.version)
    require_idle(state)
    update = state.get("organization_update")
    if not isinstance(update, dict) or not update.get("fingerprint"):
        if state.get("organization"):
            context.output.success("setup organization", state, setup=identity,
                                   lines=["This setup already uses this Forge's current workplace guidance."])
            return 0
        raise CliError("invalid_request", "This Forge offers no workplace guidance to adopt.",
                       next=[show_step(identity)])
    document = organization_document(state)
    digest = await approve(context, document, question="Use this guidance for this setup?",
                           approve_command=f"meta-forge setup organization {identity} --version {state['version']}")
    body = {"version": state["version"], "action": "organization", "selected": [], "text": "",
            "organization_fingerprint": update["fingerprint"]}
    state, _ = await send_turn(context, client, identity, body, wait=True)
    context.output.success("setup organization", state, lines=reply_lines(state), setup=identity,
                           extra={"review_digest": digest})
    return 0


def plan_of(state):
    if state.get("proposal"):
        return "proposed", state["proposal"]
    if state.get("blueprint"):
        return "accepted", state["blueprint"]
    return None, None


async def plan_show(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    state = await read_state(client, identity)
    label, plan = plan_of(state)
    if plan is None:
        raise CliError("not_found", "This setup has no plan yet. The guide proposes one as you describe the job.",
                       next=[step(f"meta-forge chat {identity}", "Describe the job to the guide.")])
    result = {"setup": identity, "version": state.get("version"), "status": state.get("status"), "plan": label,
              "blueprint": plan, "readiness": state.get("readiness"),
              "package_availability": state.get("package_availability")}
    lines = plan_lines(plan, label=label, version=state.get("version"), readiness=state.get("readiness"),
                       availability=state.get("package_availability"))
    next_steps = [step(f"meta-forge plan accept {identity}", "Review and accept this plan.")] if label == "proposed" else [
        step(f"meta-forge install {identity} --team --folder DIR", "Install the accepted agent team on this computer.")]
    context.output.success("plan show", result, lines=lines, setup=identity, next=next_steps)
    return 0


async def accept_plan(context, client, identity, expected_version=None):
    """The Use this plan gate: review, approval, then POST /apply with the reviewed version."""
    state = await read_state(client, identity)
    require_version(state, expected_version)
    require_idle(state)
    if state.get("status") == "ready" and not state.get("proposal") and state.get("blueprint"):
        return state, None, True
    if isinstance(state.get("question"), dict):
        raise CliError("input_required", "The guide is waiting for an answer before the plan can be accepted.",
                       needs=question_need(state), next=[step(f"meta-forge chat {identity}", "Answer the guide.")])
    if state.get("status") not in ("review", "refining") or not state.get("proposal"):
        raise CliError("invalid_request", "There is no proposal ready to accept yet.",
                       next=[step(f"meta-forge chat {identity}", "Continue the conversation with the guide.")])
    readiness = state.get("readiness") or {}
    if readiness.get("context_ready") is not True or readiness.get("decision_blockers"):
        raise CliError("invalid_request", "Resolve the open decisions first: "
                       + ("; ".join(readiness.get("decision_blockers") or []) or "the brief is not ready") + ".",
                       next=[step(f"meta-forge chat {identity}", "Resolve the open decisions with the guide.")])
    document = plan_document(state)
    digest = await approve(context, document, question="Accept this plan? Nothing is installed by accepting.",
                           approve_command=f"meta-forge plan accept {identity} --version {state['version']}")
    context.resume = [step(f"meta-forge setup show {identity}", "Read whether the plan is accepted now."),
                      step(f"meta-forge plan accept {identity}",
                           "Accept it if it is still only proposed; an accepted plan is not accepted twice.")]
    try:
        _, accepted = await client.post(guide_path(identity, "/apply"), json={"version": state["version"]})
    except CliError as error:
        error = await classified(client, identity, state["version"], error)
        if error.code == "outcome_unknown" and not error.next:
            error.next = list(context.resume)
        raise error from None
    return accepted, digest, False


async def plan_accept(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    state, digest, already = await accept_plan(context, client, identity, context.args.version)
    lines = ([f"This setup's plan is already accepted (version {state.get('version')})."] if already
             else [f"Plan accepted (version {state.get('version')}).", *reply_lines(state)])
    context.output.success("plan accept", state, lines=lines, setup=identity,
                           extra={"review_digest": digest} if digest else None,
                           next=[step(f"meta-forge install {identity} --team --folder DIR",
                                      "Install the accepted agent team on this computer.")])
    return 0


def write_new(path, content, what):
    """Write a new file for --out; its errors name the option, never the value given."""
    target = Path(path).expanduser()
    try:
        with target.open("xb") as stream:
            stream.write(content)
    except FileExistsError:
        raise CliError("usage", "The file given to --out already exists. Choose a new file name; nothing is "
                       "overwritten.", next=[step(f"meta-forge {what} --help", "See the output options.")]) from None
    except OSError as error:
        raise CliError("usage", f"The file given to --out cannot be written ({type(error).__name__}).",
                       next=[step(f"meta-forge {what} --help", "See the output options.")]) from None
    return str(target.resolve())


async def plan_export(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    state = await read_state(client, identity)
    label, plan = plan_of(state)
    if plan is None:
        raise CliError("not_found", "This setup has no plan yet.",
                       next=[step(f"meta-forge chat {identity}", "Describe the job to the guide.")])
    result = {"setup": identity, "version": state.get("version"), "plan": label, "blueprint_sha256": sha256_of(plan)}
    if context.args.out:
        content = (json.dumps(plan, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
        result["file"] = write_new(context.args.out, content, "plan export")
        lines = [f"Wrote the {label} blueprint of setup {short(identity)} to {result['file']}."]
    else:
        result["blueprint"] = plan
        # On a terminal, as JSON whose every character is printable: it parses to the same blueprint.
        lines = [json_text(plan, indent=2)]
    context.output.success("plan export", result, lines=lines, setup=identity)
    return 0


async def plan_validate(context):
    _, path = named_file(context, context.args.file, "plan validate")
    try:
        with path.open("rb") as stream:
            raw = stream.read(MAX_BLUEPRINT_BYTES + 1)
        value = json.loads(raw.decode("utf-8"))
    except (OSError, UnicodeError, ValueError):
        raise CliError("usage", "The file given to plan validate is not a readable JSON blueprint file.",
                       next=[step("meta-forge plan export SETUP --out FILE", "Export a blueprint to start from.")]) from None
    if len(raw) > MAX_BLUEPRINT_BYTES or not isinstance(value, dict):
        raise CliError("usage", "A blueprint is one JSON object of at most 1 MB.",
                       next=[step("meta-forge plan export SETUP --out FILE", "Export a blueprint to start from.")])
    client = await context.client()
    _, preview = await client.post("/api/blueprint/validate", json=value)
    summary = preview.get("summary") or {}
    lines = [f"Valid blueprint: {summary.get('classes')} classes, {summary.get('agents')} agents, "
             f"{summary.get('connections')} connections, {summary.get('flows')} flows."]
    for check in preview.get("checks") or []:
        lines.append(f"  {check.get('label')}: {check.get('status')} ({check.get('detail')})")
    availability = preview.get("package_availability") or {}
    if availability.get("available") is False:
        lines.append(f"  Package: unavailable: {availability.get('reason')}")
    context.output.success("plan validate", preview, lines=lines)
    return 0


async def plan_package(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    state = await read_state(client, identity)
    require_version(state, context.args.version)
    _, payload = await client.request("GET", guide_path(identity, "/deployment/package"),
                                      params={"version": state["version"], "source_kind": "native-team"},
                                      read_timeout=120.0, expect="bytes")
    import hashlib
    written = write_new(context.args.out, payload, "plan package")
    result = {"setup": identity, "version": state["version"], "file": written, "bytes": len(payload),
              "sha256": hashlib.sha256(payload).hexdigest()}
    context.output.success("plan package", result, setup=identity,
                           lines=[f"Wrote the agent team package ({len(payload)} bytes, sha256 {result['sha256']}) "
                                  f"to {written}."])
    return 0


async def chat_command(context):
    args = context.args
    if context.output.mode != "text":
        raise CliError("usage", "chat is for a person at a terminal. Scripts and agents use setup send.",
                       next=[step("meta-forge setup send --help", "Send one turn without a chat.")])
    if bool(args.setup) == bool(args.new):
        raise CliError("usage", "Choose a setup to continue or --new.",
                       next=[step("meta-forge chat --new", "Start a new setup."),
                             step("meta-forge chat last", "Continue the most recent setup.")])
    if not context.interactive:
        raise CliError("usage", "chat needs a terminal for its prompts. Use setup send instead.",
                       next=[step("meta-forge setup send --help", "Send one turn without a chat.")])
    client = await context.client()
    if args.new:
        state, _ = await create_setup(context, client)
        identity = state["id"]
    else:
        identity = await client.resolve_setup(args.setup)
        state = await read_state(client, identity)
    out = context.output
    context.resume = [step(f"meta-forge chat {identity}", "Continue this conversation.")]
    out.line(f"Setup {short(identity)}: {setup_name(state)} (version {state.get('version')}). /help lists commands.")
    if (state.get("operation") or {}).get("status") == "running":
        out.line("(the guide is still answering...)")
        state = await wait_turn(context, client, identity, state)
    for text in reply_lines(state, chat=True):
        out.line(text)
    pending = []
    while True:
        line = context.read_line("> ")
        if line is None:
            break
        line = line.strip()
        if not line:
            continue
        try:
            if line.startswith("/"):
                command, _, rest = line.partition(" ")
                rest = rest.strip()
                if command in ("/quit", "/exit"):
                    break
                if command == "/help":
                    out.line(CHAT_HELP)
                elif command == "/plan":
                    state = await read_state(client, identity)
                    label, plan = plan_of(state)
                    for text in (plan_lines(plan, label=label, version=state.get("version"),
                                            readiness=state.get("readiness"),
                                            availability=state.get("package_availability"))
                                 if plan else ["No plan yet."]):
                        out.line(text)
                elif command == "/status":
                    state = await read_state(client, identity)
                    for text in setup_lines(state):
                        out.line(text)
                elif command == "/open":
                    out.line(f"Forge: {client.base_url}/ (open this setup there to see its animated plan)")
                    status = await client.get("/api/architect/status")
                    if status.get("browser_console_url"):
                        out.line(f"Guide console: {client.base_url}{status['browser_console_url']}")
                elif command == "/attach":
                    if not rest:
                        out.line("Name a file: /attach FILE")
                        continue
                    added = await upload(context, client, identity, [rest], option="/attach")
                    pending.extend(added)
                    out.line(f"Attached {added[0].get('filename')}; it goes with your next message.")
                elif command == "/revisit":
                    if rest not in TOPICS:
                        out.line("Choose a topic: " + ", ".join(TOPICS))
                        continue
                    state = await read_state(client, identity)
                    body = {"version": state["version"], "action": "back", "selected": [], "text": "", "stage": rest}
                    state, _ = await send_turn(context, client, identity, body, wait=True)
                    for text in reply_lines(state, chat=True):
                        out.line(text)
                elif command == "/retry":
                    state = await read_state(client, identity)
                    operation = state.get("operation") or {}
                    if (operation.get("status") not in ("failed", "interrupted")
                            or not isinstance(operation.get("request"), dict)):
                        out.line("There is no interrupted or failed turn to resend.")
                        continue
                    out.line("(resending your saved answer...)")
                    body = {**operation["request"], "version": state["version"]}
                    state, _ = await send_turn(context, client, identity, body, wait=True)
                    for text in reply_lines(state, chat=True):
                        out.line(text)
                elif command == "/accept":
                    state, _, already = await accept_plan(context, client, identity)
                    out.line(f"Plan already accepted (version {state.get('version')})." if already
                             else f"Plan accepted (version {state.get('version')}).")
                else:
                    out.line("Unknown chat command. /help lists commands.")
                continue
            state = await read_state(client, identity)
            require_idle(state)
            selected, text, combine = chat_answer(state.get("question"), line)
            body = turn_body(state, selected=selected, text=text, combine=combine, attachments=pending)
            out.line("(working... the guide is answering)")
            state, _ = await send_turn(context, client, identity, body, wait=True)
            pending = []
            for text in reply_lines(state, chat=True):
                out.line(text)
        except CliError as error:
            if error.code in ("server_unavailable", "server_incompatible", "wait_timeout"):
                raise
            out.say(f"error: {error.detail} ({error.code})")
    out.line(f"Saved. Continue with: meta-forge chat {short(identity)}")
    out.finished = True
    return 0


def chat_answer(question, line):
    """Parse '2', '1,3' or '2 with more context' against the open question's options."""
    options = (question or {}).get("options") or []
    match = ANSWER.fullmatch(line) if options else None
    if not match:
        return [], line, False
    numbers = [int(part) for part in re.split(r"\s*,\s*", match.group(1))]
    if any(number < 1 or number > len(options) for number in numbers) or len(set(numbers)) != len(numbers):
        raise CliError("usage", f"Choose numbers from 1 to {len(options)}, each once.")
    selected = [options[number - 1]["id"] for number in numbers]
    combine = len(selected) > 1 and not question.get("multiple")
    return selected, (match.group(2) or "").strip(), combine
