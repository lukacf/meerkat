"""Installation on this computer: install an accepted agent team, then operate it.

Every installation action carries a fresh action_request_id, printed before it
is sent; after a lost response, `installation request` reads that request's
own record, and a repeated ID never repeats the action.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import re
import shlex
import uuid

from .. import setup_version
from ..client import UUID, short
from ..errors import CliError, step
from ..render import ACTIVE, SETTLED, receipt_lines, review_lines
from ..review import approve, install_document, review_digest, start_document
from .setup import classified, guide_path, read_state, require_idle, require_version, write_new

ACTIONS = {"start": "restart", "stop": "stop", "restart": "restart", "retry": "retry", "cleanup": "stop"}
STOPPED = {"stopped", "failed", "interrupted"}
SETTING = re.compile(r"[A-Z][A-Z0-9_]{0,119}")
HEX = re.compile(r"[0-9a-f]{64}")
NO_INSTALLATION = "No local deployment has been started for this setup."


def console_port(value):
    if not re.fullmatch(r"[1-9][0-9]{3,4}", value) or not 1024 <= int(value) <= 65535:
        raise argparse.ArgumentTypeError("use a port number from 1024 to 65535")
    return int(value)


def register(commands, command_parser):
    install = command_parser(commands, "install",
                             "Install the setup's accepted agent team on this computer.\n\n"
                             "A setting the package declares secret (a provider key, a Console password) is\n"
                             "never given here: --reuse-forge-keys lets Forge copy its own saved provider keys,\n"
                             "and other secrets are entered by a person in Forge's protected browser form.")
    install.add_argument("setup", metavar="SETUP")
    install.add_argument("--team", action="store_true", required=True,
                         help="install the agent team (applications are installed from Forge in the browser)")
    install.add_argument("--folder", required=True, metavar="DIR", help="a new or empty folder you own")
    install.add_argument("--port", type=console_port, metavar="N", help="Console port (default: the package's)")
    install.add_argument("--mode", choices=("interactive", "background"), default="interactive",
                         help="interactive: runs while Forge runs; background: an operating-system user service")
    install.add_argument("--reuse-forge-keys", action="store_true",
                         help="let Forge copy its own saved provider keys into the installation's private settings")
    install.add_argument("--set", action="append", default=[], metavar="NAME=VALUE",
                         help="a non-secret setting the package declares (repeatable)")
    install.add_argument("--dry-run", action="store_true", help="print the review and its digest; change nothing")
    install.add_argument("--version", type=setup_version, metavar="N", help="the setup version you reviewed")
    install.set_defaults(func=install_command, name="install")

    group = command_parser(commands, "installation", "The installation of a setup on this computer.")
    actions = group.add_subparsers(dest="action", metavar="ACTION", required=True)
    parser = command_parser(actions, "status", "Status, phase, health, checks and links.")
    parser.add_argument("setup", metavar="SETUP")
    parser.set_defaults(func=installation_status, name="installation status")
    for verb, text in (("start", "Start a stopped installation."), ("stop", "Stop the installation."),
                       ("restart", "Restart the installation."), ("retry", "Retry a failed installation."),
                       ("cleanup", "Stop and verify cleanup after a failed or interrupted start.")):
        parser = command_parser(actions, verb, text)
        parser.add_argument("setup", metavar="SETUP")
        parser.set_defaults(func=installation_action, name="installation " + verb, verb=verb)
    parser = command_parser(actions, "request", "Read an action request's record, for example after a lost response.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("request_id", metavar="ID", help="the action request ID the action printed")
    parser.add_argument("--deployment", metavar="ID", help="the installation (default: the setup's current one)")
    parser.set_defaults(func=installation_request, name="installation request")
    parser = command_parser(actions, "log", "The installation's bounded, redacted log lines.")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--follow", action="store_true", help="keep printing new lines until interrupted")
    parser.set_defaults(func=installation_log, name="installation log")
    parser = command_parser(actions, "open", "Print the Console and declared application links.")
    parser.add_argument("setup", metavar="SETUP")
    parser.set_defaults(func=installation_open, name="installation open")
    settings = command_parser(actions, "settings", "Private settings: field names and presence only.")
    settings_actions = settings.add_subparsers(dest="settings_action", metavar="ACTION", required=True)
    parser = command_parser(settings_actions, "show", "Field names, required or optional, and presence (never values).")
    parser.add_argument("setup", metavar="SETUP")
    parser.set_defaults(func=installation_settings, name="installation settings show")
    parser = command_parser(actions, "export", "Download the installed application package (no private settings).")
    parser.add_argument("setup", metavar="SETUP")
    parser.add_argument("--out", required=True, metavar="FILE", help="write the package to this new file")
    parser.set_defaults(func=installation_export, name="installation export")


def status_step(identity, *, wait=False):
    return step(f"meta-forge installation status {identity}" + (" --wait" if wait else ""),
                "Read the installation's current status.")


async def read_receipt(client, identity):
    try:
        return await client.get(guide_path(identity, "/deployment"))
    except CliError as error:
        if error.code == "not_found" and error.detail == NO_INSTALLATION:
            error.next = [step(f"meta-forge install {identity} --team --folder DIR",
                               "Install the setup's accepted agent team.")]
        raise


def receipt_events(context, receipt, seen):
    """Emit what changed in the receipt since the last read."""
    command = context.command
    key = (receipt.get("status"), receipt.get("phase"))
    if key != seen.get("phase"):
        seen["phase"] = key
        context.output.event(command, "phase", status=receipt.get("status"), phase=receipt.get("phase"),
                             message=f"{receipt.get('status')} ({receipt.get('phase')})")
    for entry in receipt.get("progress") or []:
        mark = ("progress", entry.get("at"), entry.get("message"))
        if mark not in seen["marks"]:
            seen["marks"].add(mark)
            context.output.event(command, "progress", at=entry.get("at"), message=entry.get("message"))
    for check in receipt.get("checks") or []:
        mark = ("check", check.get("name"), check.get("status"))
        if mark not in seen["marks"]:
            seen["marks"].add(mark)
            context.output.event(command, "check", name=check.get("name"), status=check.get("status"),
                                 detail=check.get("detail"), message=f"check {check.get('name')}: {check.get('status')}")
    for entry in receipt.get("logs") or []:
        mark = ("log", entry.get("at"), entry.get("phase"), entry.get("text"))
        if mark not in seen["marks"]:
            seen["marks"].add(mark)
            context.output.event(command, "log", at=entry.get("at"), phase=entry.get("phase"),
                                 message=f"[{entry.get('phase')}] {entry.get('text')}")


async def wait_installation(context, client, identity, deployment_id, *, settled=SETTLED, receipt=None):
    """Poll the installation as the browser does (every 1.5 s) until it settles."""
    seen = {"phase": None, "marks": set()}
    context.resume = [status_step(identity, wait=True)]
    while True:
        if receipt is None:
            receipt = await read_receipt(client, identity)
        receipt_events(context, receipt, seen)
        if receipt.get("deployment_id") != deployment_id:
            raise CliError("conflict", "Another installation of this setup was selected while waiting.",
                           result=receipt, next=[status_step(identity)])
        if receipt.get("status") in settled:
            return receipt
        receipt = None
        await context.sleep(1.5)


def outcome(receipt, identity):
    """A settled installation's exit: running or stopped succeed; failed or interrupted do not."""
    status = receipt.get("status")
    if status == "failed":
        raise CliError("operation_failed", receipt.get("detail") or "The installation failed.", result=receipt,
                       next=[step(f"meta-forge installation log {identity}", "Read the redacted log lines."),
                             step(f"meta-forge installation cleanup {identity}",
                                  "Stop and verify cleanup before retrying.")])
    if status == "interrupted":
        raise CliError("operation_interrupted", receipt.get("detail") or "The installation was interrupted.",
                       result=receipt, next=[step(f"meta-forge installation cleanup {identity}",
                                                  "Stop and verify cleanup, then start it again.")])


def parse_settings(values):
    """--set NAME=VALUE pairs. Errors never repeat the argument: it may be a secret given by mistake."""
    settings = {}
    for value in values:
        name, separator, setting = value.partition("=")
        if not separator or not SETTING.fullmatch(name):
            raise CliError("usage", "--set takes NAME=VALUE, where NAME is a setting the package declares, "
                           "in upper case (the argument is not shown).",
                           next=[step("meta-forge install --help", "See the install options.")])
        if name == "MOBKIT_PORT":
            raise CliError("usage", "Choose the Console port with --port.",
                           next=[step("meta-forge install --help", "See the install options.")])
        if not setting or "\x00" in setting or len(setting) > 32768:
            raise CliError("usage", f"--set {name} needs a value of at most 32768 characters.",
                           next=[step("meta-forge install --help", "See the install options.")])
        if name in settings:
            raise CliError("usage", f"--set {name} is given twice.",
                           next=[step("meta-forge install --help", "See the install options.")])
        settings[name] = setting
    return settings


def secret_needs(base_url, identity, fields, *, deployment="new", reuse_hint):
    needs = []
    for field in fields:
        needs.append({"kind": "needs_secret", "handle": f"setting:{identity}:{deployment}:{field['key']}",
                      "why": field.get("label") or field["key"],
                      "instruction": (f"{field['key']} is a provider key Forge has saved: add --reuse-forge-keys, "
                                      "and Forge copies it into the installation without this command seeing it."
                                      if field["key"] in reuse_hint else
                                      "A person enters it in Forge's protected installation form: open Forge in "
                                      "a browser on this computer (over SSH, forward the port first), open this "
                                      "setup and install it from its installation panel. This command never "
                                      "takes it."),
                      "person_opens": base_url + "/"})
    return needs


async def install_command(context):
    args = context.args
    provided = parse_settings(args.set)
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    state = await read_state(client, identity)
    require_version(state, args.version)
    require_idle(state)
    if state.get("status") != "ready" or state.get("proposal") or not state.get("blueprint"):
        raise CliError("invalid_request", "This setup has no accepted plan to install. Accept a plan first.",
                       next=[step(f"meta-forge plan show {identity}", "Read the plan."),
                             step(f"meta-forge plan accept {identity}", "Review and accept it.")])
    folder = os.path.abspath(os.path.expanduser(args.folder))
    base = f"meta-forge install {identity} --team --folder {shlex.quote(folder)}"
    params = {"source_kind": "native-team", "version": state["version"]}
    if args.port is not None:
        params["console_port"] = args.port
    options = await client.get(guide_path(identity, "/deployment/options"), params=params, read_timeout=120.0)
    if (options.get("source_kind") != "native-team" or options.get("current") is not True
            or not HEX.fullmatch(str(options.get("source_id"))) or not HEX.fullmatch(str(options.get("source_digest")))):
        raise CliError("conflict", "The agent team changed while preparing its installation. Read it again.",
                       next=[step(f"meta-forge setup show {identity}", "Read the current plan.")])
    if options.get("supported") is not True:
        raise CliError("invalid_request", f"This plan targets {options.get('target')}, but this computer is "
                       f"{options.get('host_target')}. Ask the guide to change the destination, or take the "
                       "agent team package to that computer.",
                       next=[step(f"meta-forge plan package {identity} --out team.zip", "Download the package."),
                             step(f"meta-forge chat {identity}", "Ask the guide to change the destination.")])
    fresh = options.get("fresh_install") or {}
    if fresh.get("available") is not True:
        raise CliError("prerequisite_unavailable", fresh.get("reason") or "A fresh installation is unavailable here.",
                       next=[step("meta-forge doctor", "Check this computer for Forge.")])
    modes = {item.get("id"): item for item in options.get("execution_modes") or [] if isinstance(item, dict)}
    if (modes.get(args.mode) or {}).get("available") is not True:
        reason = (modes.get(args.mode) or {}).get("reason") or "it is unavailable on this computer"
        raise CliError("prerequisite_unavailable", f"The {args.mode} mode cannot be used: {reason}",
                       next=[step(f"{base} --mode interactive",
                                  "Install it to run while Forge runs.")])
    port = args.port if args.port is not None else options.get("console_port")
    observation = options.get("console_port_observation") or {}
    port_status = observation.get("status") if observation.get("port") == port else "unknown"
    if port_status == "occupied":
        alternate = observation.get("alternate_port")
        raise CliError("invalid_request", f"Console port {port} is in use on this computer.",
                       next=[step(f"{base} --port {alternate}",
                                  "Use the free port Forge suggests.")] if alternate else
                       [step(f"{base} --port N", "Choose a free port.")])
    if port_status != "available":
        raise CliError("prerequisite_unavailable", f"Console port {port} could not be checked. Check it again.",
                       next=[step(f"{base} --dry-run",
                                  "Check the port again.")])
    fields = [field for field in options.get("fields") or [] if isinstance(field, dict) and not field.get("automatic")]
    declared = {field.get("key"): field for field in fields}
    for name in provided:
        if name not in declared:
            plain = sorted(key for key, field in declared.items() if not field.get("secret"))
            raise CliError("invalid_request", f"This package does not request {name}. Its non-secret settings: "
                           + (", ".join(plain) or "none") + ".",
                           next=[step(f"{base} --dry-run",
                                      "See every setting the package requests.")])
        if declared[name].get("secret"):
            raise CliError("invalid_request", f"{name} is a secret setting. It is never given to this command; "
                           "a person enters it in Forge's protected installation form.",
                           needs=secret_needs(client.base_url, identity, [declared[name]], reuse_hint=set()),
                           next=[step(f"{base} --dry-run",
                                      "Review the installation without that setting.")])
    host_keys = set(options.get("host_provider_keys") or []) if options.get("can_use_host_provider_credentials") else set()
    if args.reuse_forge_keys and not host_keys:
        raise CliError("prerequisite_unavailable", "Forge has no saved provider key that this package uses.",
                       next=[step("meta-forge provider status", "Check Forge's saved providers.")])
    reused = sorted(key for key in host_keys if key in declared) if args.reuse_forge_keys else []
    missing_secret, missing_plain, settings = [], [], []
    for key, field in declared.items():
        secret, required = bool(field.get("secret")), bool(field.get("required"))
        if key in provided:
            source, value = "this command (--set)", provided[key]
        elif key in reused:
            source, value = "Forge's saved provider key", None
        elif not secret and field.get("default"):
            source, value = "package default", field.get("default")
        else:
            source, value = "not set", None
            if required:
                (missing_secret if secret else missing_plain).append(field)
        entry = {"name": key, "secret": secret, "required": required, "provided": source}
        if value is not None and not secret:
            entry["value"] = value
        settings.append(entry)
    if missing_plain:
        raise CliError("invalid_request", "This package needs values for: "
                       + ", ".join(field["key"] for field in missing_plain) + ". Give each with --set NAME=VALUE.",
                       next=[step(f"{base} --dry-run",
                                  "See every setting the package requests.")])
    if missing_secret:
        raise CliError("input_required", "This package needs secret settings that a person enters outside this "
                       "command: " + ", ".join(field["key"] for field in missing_secret) + ".",
                       needs=secret_needs(client.base_url, identity, missing_secret, reuse_hint=host_keys),
                       next=[step(f"{base} --reuse-forge-keys",
                                  "Let Forge copy its saved provider keys.")]
                       if any(field["key"] in host_keys for field in missing_secret) else
                       [step("meta-forge provider status", "Check which provider keys Forge has saved.")])
    document = install_document(state, options, client.capabilities, folder=folder, port=port, port_status=port_status,
                                mode=args.mode, settings=settings, reused_keys=reused)
    # The exact command for this review; --set values are non-secret by the package's own declaration.
    again = " ".join([base, *(["--port", str(args.port)] if args.port is not None else []),
                      *(["--mode", "background"] if args.mode == "background" else []),
                      *(["--reuse-forge-keys"] if args.reuse_forge_keys else []),
                      *[part for name, value in provided.items() for part in ("--set", shlex.quote(f"{name}={value}"))]])
    if args.dry_run:
        digest = review_digest(document)
        context.output.success("install", {"review": document, "digest": digest}, setup=identity,
                               lines=review_lines(document, digest),
                               next=[step(f"{again} --approve {digest}", "Install exactly this review.")])
        return 0
    digest = await approve(context, document, question="Install and start this agent team?", approve_command=again)
    body = {"version": state["version"], "source_kind": "native-team", "source_id": options.get("source_id"),
            "source_digest": options.get("source_digest"), "destination": folder,
            "settings": {**provided, "MOBKIT_PORT": str(port)},
            "use_host_provider_credentials": bool(reused), "execution_mode": args.mode}
    context.resume = [status_step(identity, wait=True)]
    try:
        _, receipt = await context.bounded(client.post(guide_path(identity, "/deployment"), json=body),
                                           "the installation was admitted")
    except CliError as error:
        if error.code == "conflict":
            raise await classified(client, identity, state["version"], error)
        if error.code != "outcome_unknown":
            raise
        # The browser never resubmits an install after a lost response: read the installation instead.
        try:
            receipt = await read_receipt(client, identity)
        except CliError:
            raise error from None
        if receipt.get("destination") != folder or receipt.get("source_id") != options.get("source_id"):
            error.next = [status_step(identity)]
            raise error from None
    context.output.event("install", "admitted", deployment_id=receipt.get("deployment_id"),
                         message=f"installation {short(receipt.get('deployment_id'))} admitted")
    if context.wait:
        receipt = await wait_installation(context, client, identity, receipt.get("deployment_id"), receipt=receipt)
        outcome(receipt, identity)
    context.output.success("install", receipt, lines=receipt_lines(receipt), setup=identity,
                           extra={"review_digest": digest},
                           next=[step(f"meta-forge installation open {identity}", "Print the Console link."),
                                 status_step(identity, wait=receipt.get("status") in ACTIVE)])
    return 0


async def installation_status(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    receipt = await read_receipt(client, identity)
    if context.wait and receipt.get("status") not in SETTLED:
        receipt = await wait_installation(context, client, identity, receipt.get("deployment_id"), receipt=receipt)
        outcome(receipt, identity)
    context.output.success("installation status", receipt, lines=receipt_lines(receipt), setup=identity)
    return 0


async def installation_action(context):
    verb = context.args.verb
    action = ACTIONS[verb]
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    receipt = await read_receipt(client, identity)
    deployment_id = receipt.get("deployment_id")
    interactive = receipt.get("execution_mode", "interactive") == "interactive" and not receipt.get("service")
    if verb == "start" and receipt.get("status") != "stopped":
        raise CliError("conflict", f"start applies to a stopped installation; this one is {receipt.get('status')}.",
                       next=[status_step(identity)] + ([step(f"meta-forge installation cleanup {identity}",
                                                            "Stop and verify cleanup first.")]
                                                       if receipt.get("status") in ("failed", "interrupted") else []))
    if verb == "cleanup" and not (interactive and receipt.get("status") in ("failed", "interrupted")):
        raise CliError("conflict", "cleanup applies after a failed or interrupted start of an installation that "
                       f"runs while Forge runs; this one is {receipt.get('status')}.",
                       next=[step(f"meta-forge installation stop {identity}", "Stop it instead.")])
    digest = None
    if action in ("restart", "retry"):
        state = await read_state(client, identity)
        document = start_document(state, receipt, action)
        digest = await approve(context, document, question=f"{verb.capitalize()} this installation?",
                               approve_command=f"meta-forge installation {verb} {identity}")
    request_id = str(uuid.uuid4())
    observe = step(f"meta-forge installation request {identity} {request_id} --wait",
                   "Read this action's own record; it is never repeated.")
    context.output.note(f"Action request: {request_id}")
    context.output.event(context.command, "admitted", request=request_id, action=action, deployment_id=deployment_id)
    context.resume = [observe, status_step(identity)]
    body = {"deployment_id": deployment_id, "action": action, "action_request_id": request_id}
    try:
        _, result = await context.bounded(client.post(guide_path(identity, "/deployment/action"), json=body),
                                          f"the {verb} action answered")
    except CliError as error:
        error.request = request_id
        if error.code in ("outcome_unknown", "wait_timeout"):
            error.next = [observe]
        raise
    if context.wait:
        result = await wait_installation(context, client, identity, deployment_id,
                                         settled=STOPPED if action == "stop" else SETTLED)
        outcome(result, identity)
    context.output.success(context.command, result, lines=receipt_lines(result), setup=identity, request=request_id,
                           extra={"review_digest": digest} if digest else None)
    return 0


async def installation_request(context):
    args = context.args
    if not UUID.fullmatch(args.request_id):
        raise CliError("usage", "The action request ID is the UUID the action printed.",
                       next=[step("meta-forge installation request --help", "See this command's usage.")])
    client = await context.client()
    identity = await client.resolve_setup(args.setup)
    deployment_id = args.deployment or (await read_receipt(client, identity)).get("deployment_id")
    path = guide_path(identity, f"/deployment/action/{deployment_id}/{args.request_id}")
    context.resume = [step(f"meta-forge installation request {identity} {args.request_id} --wait",
                           "Read the request's record again.")]
    while True:
        value = await client.get(path)
        record = value.get("action_request") or {}
        if not context.wait or record.get("phase") in ("settled", "unknown"):
            break
        await context.sleep(1.5)
    phase, result = record.get("phase"), record.get("outcome")
    lines = [f"Action request {args.request_id}: {record.get('action')} on installation {short(deployment_id)}: "
             f"{phase}" + (f" ({result})" if result else "")]
    if phase == "settled" and result == "error":
        raise CliError("operation_failed", "The action ran and reported an error; some steps may have finished.",
                       result=value, request=args.request_id, next=[status_step(identity)])
    if phase != "settled":
        raise CliError("outcome_unknown", "The action's outcome is not settled yet." if phase == "pending" else
                       "The action's effects are uncertain; check the installation before another action.",
                       result=value, request=args.request_id,
                       next=[context.resume[0], status_step(identity)])
    context.output.success("installation request", value, lines=lines, setup=identity, request=args.request_id,
                           next=[status_step(identity)])
    return 0


async def installation_log(context):
    if context.args.follow and context.output.mode == "json":
        raise CliError("usage", "--follow streams lines; use --output jsonl or text.",
                       next=[step("meta-forge installation log --help", "See this command's usage.")])
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    receipt = await read_receipt(client, identity)
    if not context.args.follow:
        logs = receipt.get("logs") or []
        lines = [f"{entry.get('at')} [{entry.get('phase')}] {entry.get('text')}" for entry in logs] or ["No log lines yet."]
        context.output.success("installation log", {"deployment_id": receipt.get("deployment_id"), "logs": logs},
                               lines=lines, setup=identity)
        return 0
    context.resume = [step(f"meta-forge installation log {identity}", "Print the log lines again.")]
    seen = set()
    while True:
        for entry in receipt.get("logs") or []:
            mark = (entry.get("at"), entry.get("phase"), entry.get("text"))
            if mark not in seen:
                seen.add(mark)
                if context.output.mode == "text":
                    context.output.line(f"{entry.get('at')} [{entry.get('phase')}] {entry.get('text')}")
                else:
                    context.output.event("installation log", "log", at=entry.get("at"), phase=entry.get("phase"),
                                         message=entry.get("text"))
        await context.sleep(1.5)
        receipt = await read_receipt(client, identity)


async def installation_open(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    receipt = await read_receipt(client, identity)
    experiences = await client.get(guide_path(identity, "/deployment/experiences"))
    lines = [f"Console: {receipt['console_url']}" if receipt.get("console_url") else
             f"Console: not available while the installation is {receipt.get('status')}"]
    for entry in experiences.get("entries") or []:
        if isinstance(entry, dict) and entry.get("url"):
            lines.append(f"{entry.get('label') or entry.get('id')}: {entry.get('url')}")
    if experiences.get("error"):
        lines.append(f"Application links: {experiences['error']}")
    lines.append("Open these in a browser on this computer (over SSH, forward their ports first).")
    context.output.success("installation open", {"console_url": receipt.get("console_url"),
                                                 "status": receipt.get("status"), "experiences": experiences},
                           lines=lines, setup=identity)
    return 0


async def installation_settings(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    receipt = await read_receipt(client, identity)
    value = await client.get(guide_path(identity, "/deployment/settings"),
                             params={"deployment_id": receipt.get("deployment_id")}, read_timeout=120.0)
    lines = [f"Private settings of installation {short(receipt.get('deployment_id'))} (presence only):"]
    for field in value.get("fields") or []:
        if not isinstance(field, dict):
            continue
        lines.append(f"  {field.get('key'):<28} {'secret' if field.get('secret') else 'plain':<7}"
                     f"{'required' if field.get('required') else 'optional':<10}"
                     f"{'present' if field.get('configured') else 'missing'}")
    context.output.success("installation settings show", value, lines=lines, setup=identity)
    return 0


async def installation_export(context):
    client = await context.client()
    identity = await client.resolve_setup(context.args.setup)
    receipt = await read_receipt(client, identity)
    if receipt.get("source_kind") == "native-team":
        raise CliError("invalid_request", "An agent team is exported as its package, not as an installed application.",
                       next=[step(f"meta-forge plan package {identity} --out team.zip", "Download the agent team package.")])
    source_digest = receipt.get("source_digest")
    if not (isinstance(source_digest, str) and re.fullmatch(r"[0-9a-f]{64}", source_digest)):
        build = await client.get(guide_path(identity, "/implementation"))
        source_digest = build.get("source_digest") if build.get("build_id") == receipt.get("build_id") else None
    if not (isinstance(source_digest, str) and re.fullmatch(r"[0-9a-f]{64}", source_digest)):
        raise CliError("prerequisite_unavailable", "Forge does not report this installation's verified source "
                       "digest, so its package cannot be exported.", next=[status_step(identity)])
    params = {"deployment_id": receipt.get("deployment_id"), "build_id": receipt.get("build_id"),
              "source_digest": source_digest, "package_sha256": receipt.get("package_sha256")}
    _, payload = await client.request("GET", guide_path(identity, "/deployment/installed-package"), params=params,
                                      read_timeout=120.0, expect="bytes")
    digest = hashlib.sha256(payload).hexdigest()
    if digest != receipt.get("package_sha256"):
        raise CliError("conflict", "The downloaded package differs from this installation. Nothing was written.",
                       next=[status_step(identity)])
    written = write_new(context.args.out, payload, "installation export")
    result = {"setup": identity, "deployment_id": receipt.get("deployment_id"), "file": written,
              "bytes": len(payload), "sha256": digest}
    context.output.success("installation export", result, setup=identity,
                           lines=[f"Wrote the installed package ({len(payload)} bytes, sha256 {digest}) to {written}."])
    return 0
