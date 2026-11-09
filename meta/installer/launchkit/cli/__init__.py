"""meta-forge: Meta Forge's command-line front end.

It runs on the computer that runs Forge and drives the running Forge server
over the same local HTTP API the browser uses, so every step keeps Forge's
records, version checks, idempotency keys and approval gates. It never runs
Forge's services in its own process, keeps no records of its own, and never
reads, receives, forwards, stores or prints a secret value.
"""
from __future__ import annotations

import argparse
import asyncio
import contextlib
import os
from pathlib import Path
import re
import signal
import sys
import threading
import time

from .errors import CliError, EXIT_CODES, step
from .render import Output

FORGE_ROOT = Path(__file__).resolve().parents[2]
GLOBAL_OPTIONS = ("server", "home", "output", "wait", "timeout", "no_input", "approve", "verbose", "quiet")
DURATION = re.compile(r"([0-9]+(?:\.[0-9]+)?)(ms|s|m|h)?")
UNRECOGNIZED = "unrecognized arguments: "
OPTION_NAME = re.compile(r"--?[A-Za-z][A-Za-z0-9-]{0,38}")
ARGUMENT = re.compile(r"argument (?P<name>[^\s:]+): (?P<reason>.+)", re.S)
AMBIGUOUS = re.compile(r"ambiguous option: (?P<option>.*) could match "
                       r"(?P<matches>--?[A-Za-z0-9][-A-Za-z0-9]*(?:, --?[A-Za-z0-9][-A-Za-z0-9]*)*)", re.S)
SAFE_REASON = re.compile(r"expected (?:one|at most one|at least one|[0-9]+) arguments?|not allowed with argument [^\s:]+")
SAFE_MESSAGE = re.compile(r"the following arguments are required: [-A-Za-z0-9_/, ]+|"
                          r"one of the arguments [-A-Za-z0-9_/ ]+ is required")
# The refusals of this command's own argument types: fixed texts that name no value.
TYPE_REFUSALS = set()

DESCRIPTION = """\
Meta Forge from a terminal. Run it on the computer that runs Forge: it drives
the running Forge server (start one with "meta-forge serve") through the same
local API as the browser, with the same records, version checks and approvals.

No command takes a secret value. Provider keys, Console passwords and other
private settings are entered by a person in Forge's protected browser forms;
this command only reports whether they are present.
"""

EPILOG = """\
commands:
  serve                       run Forge's launcher (start.sh) in this terminal
  doctor                      check this computer for Forge
  version                     this command's Forge release and the server's
  provider status             model providers configured for Forge's own agents
  guide status|start|stop     Forge's setup team
  setup list|show|new|send|revisit|retry|timeline|organization
                              saved setups and the guided conversation
  chat [SETUP|--new]          talk to the guide in this terminal
  plan show|accept|export|validate|package
                              the setup's plan (blueprint)
  install SETUP --team --folder DIR
                              install the accepted agent team on this computer
  installation status|start|stop|restart|retry|cleanup|request|log|open|settings show|export
                              the installation of a setup

SETUP is a full setup ID, a unique ID prefix of at least 8 hex digits, or
"last" (the most recently updated setup).

Approvals: accepting a plan, adopting workplace guidance, installing and
starting show a review first. At a terminal you confirm it; otherwise add
--approve sha256:<digest> with the digest of the exact review (a changed
review has another digest).

Exit codes: 0 success, 1 the operation failed, 2 usage or invalid request,
80 no compatible Forge server, 81 not found, 82 version changed or busy,
83 outcome unknown, 84 approval required, 85 a person must answer or enter a
secret, 86 a prerequisite is unavailable, 87 --timeout elapsed (the work
continues), 130 interrupted (the work continues). Read error.code in JSON
output rather than only the number.
"""


class Parser(argparse.ArgumentParser):
    """Usage errors become typed outcomes instead of exiting the process.

    They name arguments, options and choices, never a value typed on the
    command line: a value may be a secret typed in the wrong place.
    """

    def error(self, message):
        raise CliError("usage", f"{self.prog}: {usage_message(self, message)}",
                       next=[step(f"{self.prog} --help", "See this command's usage and options.")])


def refusal(text):
    """An argument type's refusal: a fixed text, shown as it is."""
    TYPE_REFUSALS.add(text)
    return argparse.ArgumentTypeError(text)


def option_name(token):
    """The option part of a typed token (--name of --name=value, -k of -kvalue), or None."""
    name = token.split("=", 1)[0] if token.startswith("--") else token[:2] if token.startswith("-") else ""
    return name if OPTION_NAME.fullmatch(name) else None


def action_label(action):
    """An argument's name as argparse's messages give it."""
    if action.option_strings:
        return "/".join(action.option_strings)
    if action.metavar not in (None, argparse.SUPPRESS):
        return str(action.metavar)
    return action.dest


def usage_message(parser, message):
    """argparse's message, keeping argument names, options and choices only."""
    if message.startswith(UNRECOGNIZED):
        names = [name for name in map(option_name, message[len(UNRECOGNIZED):].split()) if name]
        return "unrecognized arguments" + (f" {' '.join(names)}" if names else "") + " (values not shown)"
    match = AMBIGUOUS.fullmatch(message)
    if match:
        name = option_name(match.group("option"))
        return f"ambiguous option{' ' + name if name else ''} could match {match.group('matches')}"
    match = ARGUMENT.fullmatch(message)
    if match:
        name, reason = match.group("name"), match.group("reason")
        if SAFE_REASON.fullmatch(reason) or reason in TYPE_REFUSALS:
            return message
        if reason.startswith("invalid choice"):
            action = next((item for item in parser._actions if action_label(item) == name), None)
            choices = ", ".join(map(str, action.choices)) if action is not None and action.choices else ""
            return f"argument {name}: invalid choice" + (f" (choose from {choices})" if choices else "")
        if reason.startswith("ignored explicit argument"):
            return f"argument {name}: takes no value"
        return f"argument {name}: invalid value (not shown)"
    if SAFE_MESSAGE.fullmatch(message):
        return message
    return "invalid arguments (values not shown)"


def vocabulary(parser):
    """Every word the command line defines: program names, options, commands, choices and metavars."""
    words = set(parser.prog.split())
    for action in parser._actions:
        words.update(action.option_strings)
        words.update(value for value in (action.metavar, action.dest) if isinstance(value, str))
        if action.choices:
            words.update(str(choice) for choice in action.choices)
        if isinstance(action, argparse._SubParsersAction):
            for child in action.choices.values():
                words |= vocabulary(child)
    return words


def without_values(detail, argv, parser):
    """A parse error's text if it repeats nothing typed on the command line apart from the command
    line's own words; otherwise one that names nothing typed. It holds whatever wording another Python
    release gives argparse's messages."""
    words = vocabulary(parser)
    for token in argv:
        if option_name(token) == token:
            continue  # an option's name alone, which messages may name
        parts = [token, *([token.split("=", 1)[1]] if token.startswith("-") and "=" in token else []),
                 *([token[2:]] if token.startswith("-") and not token.startswith("--") else [])]
        if any(len(part) >= 4 and part not in words and part in detail for part in parts):
            prefix = detail.split(": ", 1)[0]
            command = prefix if prefix and all(word in words for word in prefix.split()) else "meta-forge"
            return f"{command}: invalid arguments (values not shown)"
    return detail


def duration(value):
    match = DURATION.fullmatch(value.strip())
    if not match:
        raise refusal("use a duration such as 90s, 15m or 1h")
    number = float(match.group(1))
    seconds = number * {"ms": 0.001, "s": 1, "m": 60, "h": 3600, None: 1}[match.group(2)]
    if seconds <= 0:
        raise refusal("use a positive duration")
    return seconds


def port_number(value):
    if not re.fullmatch(r"[1-9][0-9]{0,4}", value) or int(value) > 65535:
        raise refusal("use a port number from 1 to 65535")
    return int(value)


def setup_version(value):
    if not re.fullmatch(r"[1-9][0-9]{0,15}", value):
        raise refusal("use the setup's version number, as setup show reports it")
    return int(value)


def add_global_options(parser, prefix):
    """The global options, accepted before or after the command name."""
    hidden = argparse.SUPPRESS
    group = parser.add_argument_group("global options")
    group.add_argument("--server", dest=prefix + "server", metavar="URL", default=hidden,
                       help="Forge server to use (default http://127.0.0.1:${META_FORGE_PORT:-8787})")
    group.add_argument("--home", dest=prefix + "home", metavar="DIR", default=hidden,
                       help="Forge state directory, for serve and doctor (default: META_FORGE_HOME, else .studio)")
    group.add_argument("-o", "--output", dest=prefix + "output", choices=("text", "json", "jsonl"),
                       default=hidden, help="output mode (default text)")
    group.add_argument("--json", dest=prefix + "output", action="store_const", const="json", default=hidden,
                       help="the same as --output json")
    group.add_argument("--wait", dest=prefix + "wait", action="store_true", default=hidden,
                       help="wait for a long step to finish (default when output is text on a terminal)")
    group.add_argument("--no-wait", dest=prefix + "wait", action="store_false", default=hidden,
                       help="return as soon as the step is admitted")
    group.add_argument("--timeout", dest=prefix + "timeout", metavar="DURATION", type=duration, default=hidden,
                       help="upper bound on waiting, such as 15m; the work continues (exit 87)")
    group.add_argument("--no-input", dest=prefix + "no_input", action="store_true", default=hidden,
                       help="never prompt (implied when stdin is not a terminal)")
    group.add_argument("--approve", dest=prefix + "approve", metavar="DIGEST", action="append", default=hidden,
                       help="approve the review with this digest (repeatable)")
    group.add_argument("-v", "--verbose", dest=prefix + "verbose", action="count", default=hidden,
                       help="more progress detail")
    group.add_argument("-q", "--quiet", dest=prefix + "quiet", action="store_true", default=hidden,
                       help="less progress detail")


def global_value(namespace, name):
    top = getattr(namespace, "g_" + name, None)
    command = getattr(namespace, "s_" + name, None)
    if name == "approve":
        return [*(top or []), *(command or [])]
    if name == "verbose":
        return (top or 0) + (command or 0)
    return command if command is not None else top


def build_parser():
    from .commands import register
    parser = Parser(prog="meta-forge", description=DESCRIPTION, epilog=EPILOG,
                    formatter_class=argparse.RawDescriptionHelpFormatter)
    add_global_options(parser, "g_")
    commands = parser.add_subparsers(dest="command", metavar="COMMAND", parser_class=Parser)
    commands.required = True

    def command_parser(container, name, help_text, **options):
        sub = container.add_parser(name, help=help_text.split("\n\n", 1)[0], description=help_text,
                                   formatter_class=argparse.RawDescriptionHelpFormatter, **options)
        add_global_options(sub, "s_")
        return sub

    register(commands, command_parser)
    return parser


def requested_mode(argv):
    """The output mode named in argv, for usage errors found before parsing completes."""
    mode = "text"
    for index, value in enumerate(argv):
        if value == "--json":
            mode = "json"
        elif value in ("-o", "--output") and index + 1 < len(argv) and argv[index + 1] in ("text", "json", "jsonl"):
            mode = argv[index + 1]
        elif value.startswith("--output=") and value.split("=", 1)[1] in ("text", "json", "jsonl"):
            mode = value.split("=", 1)[1]
    return mode


class Context:
    """One command run: its options, output, streams and Forge server client."""

    def __init__(self, namespace, output, *, environ, stdin, transport, poll_scale, executor):
        self.args = namespace
        self.output = output
        self.environ = environ
        self.stdin = stdin
        self.transport = transport
        self.poll_scale = poll_scale
        self.executor = executor
        self.command = getattr(namespace, "name", "meta-forge")
        self.resume = []
        # The idempotency key this command sent or reads (a creation or action request ID), kept for the
        # outcome of an interruption or a timeout.
        self.request = None
        self._client = None
        timeout = self.option("timeout")
        self.deadline = time.monotonic() + timeout if timeout else None

    def option(self, name):
        return global_value(self.args, name)

    @property
    def approvals(self):
        return self.option("approve")

    @property
    def interactive(self):
        def terminal(stream):
            return bool(getattr(stream, "isatty", lambda: False)())
        return not self.option("no_input") and terminal(self.stdin) and terminal(self.output.stderr)

    @property
    def wait(self):
        value = self.option("wait")
        if value is not None:
            return value
        return self.output.mode == "text" and bool(getattr(self.output.stdout, "isatty", lambda: False)())

    def read_line(self, prompt):
        """One line from the person at the terminal; None at end of input."""
        self.output.prompt(prompt)
        if self.stdin is sys.stdin and threading.current_thread() is threading.main_thread():
            # The asyncio runner's first Ctrl-C only cancels at the next await;
            # while blocked here, Ctrl-C must interrupt the read itself.
            previous = signal.getsignal(signal.SIGINT)
            signal.signal(signal.SIGINT, signal.default_int_handler)
            try:
                return input()
            except EOFError:
                return None
            finally:
                signal.signal(signal.SIGINT, previous)
        line = self.stdin.readline()
        return None if line == "" else line.rstrip("\n")

    def remaining(self):
        if self.deadline is None:
            return None
        return self.deadline - time.monotonic()

    def next_steps(self):
        """What to run after an interruption or a timeout: the command's own resume step, else a read of the
        setup it was working on, else the setup list."""
        if self.resume:
            return list(self.resume)
        identity = getattr(self._client, "resolved", None)
        if identity:
            return [step(f"meta-forge setup show {identity}", "Read the setup's current state.")]
        return [step("meta-forge setup list", "Check the current state.")]

    def timed_out(self, what):
        return CliError("wait_timeout", f"--timeout elapsed while {what}. The work continues in Forge.",
                        next=self.next_steps(), request=self.request)

    async def sleep(self, seconds):
        delay = seconds * self.poll_scale
        left = self.remaining()
        if left is not None:
            if left <= 0:
                raise self.timed_out("waiting")
            delay = min(delay, left)
        await asyncio.sleep(max(delay, 0))

    async def client(self, *, compatible=True):
        if self._client is None:
            from importlib.util import find_spec
            if find_spec("httpx") is None:
                raise CliError("prerequisite_unavailable", "Forge's Python environment is incomplete.",
                               next=[step("bash start.sh --prepare-only", "Prepare Forge's environment in its folder.")])
            from .client import ForgeClient, server_url
            self._client = ForgeClient(server_url(self.environ, self.option("server")), transport=self.transport,
                                       trace=self.output.trace if self.output.verbose else None,
                                       remaining=self.remaining, timed_out=self.timed_out)
        if compatible and self._client.capabilities is None:
            await self._client.check_compatible(cli_version())
        return self._client

    async def close(self):
        if self._client is not None:
            await self._client.close()


def cli_version():
    """This folder's Forge release: pyproject.toml's version, as the server reports its own."""
    try:
        import tomllib
        return tomllib.loads((FORGE_ROOT / "pyproject.toml").read_text())["project"]["version"]
    except (OSError, KeyError, ValueError):
        from importlib.metadata import PackageNotFoundError, version
        try:
            return version("meta-forge")
        except PackageNotFoundError:
            return "unknown"


async def run_async(argv, *, stdin=None, stdout=None, stderr=None, environ=None, transport=None,
                    poll_scale=1.0, executor=None, context_holder=None):
    """Run one command; return its exit code. Streams and transport are injectable for tests."""
    stdin = sys.stdin if stdin is None else stdin
    stdout = sys.stdout if stdout is None else stdout
    stderr = sys.stderr if stderr is None else stderr
    environ = dict(os.environ) if environ is None else environ
    argv = list(argv)
    parser = build_parser()
    try:
        with contextlib.redirect_stdout(stdout):
            namespace = parser.parse_args(argv)
    except SystemExit as exit_request:
        return exit_request.code if isinstance(exit_request.code, int) else 0
    except CliError as error:
        error.detail = without_values(error.detail, argv, parser)
        Output(requested_mode(argv), stdout, stderr, environ=environ).failure("meta-forge", error)
        return error.exit_code
    mode = global_value(namespace, "output") or "text"
    output = Output(mode, stdout, stderr, verbose=global_value(namespace, "verbose"),
                    quiet=bool(global_value(namespace, "quiet")), environ=environ)
    context = Context(namespace, output, environ=environ, stdin=stdin, transport=transport,
                      poll_scale=poll_scale, executor=executor or os.execve)
    if context_holder is not None:
        context_holder.append(context)
    try:
        if context.option("home") and getattr(namespace, "name", "") not in ("serve", "doctor"):
            raise CliError("usage", "--home applies to serve and doctor. To use another running Forge, "
                           "use --server http://127.0.0.1:PORT.",
                           next=[step("meta-forge --help", "See the global options.")])
        return await namespace.func(context)
    except CliError as error:
        output.failure(context.command, error)
        return error.exit_code
    except KeyboardInterrupt:
        error = interrupted(context)
        output.failure(context.command, error)
        return error.exit_code
    except Exception as defect:
        # A defect of this command: still one typed outcome, without the exception's text, which may hold
        # values from Forge or from the command line.
        error = CliError("internal_error", f"meta-forge stopped on an unexpected error ({type(defect).__name__}). "
                         "Work it already sent to Forge may continue.", next=context.next_steps(),
                         request=context.request)
        output.failure(context.command, error)
        return error.exit_code
    finally:
        await context.close()


def interrupted(context):
    return CliError("interrupted", "Interrupted. Only waiting stopped; work already sent to Forge continues.",
                    next=context.next_steps(), request=context.request)


def main(argv=None):
    """Console entry point: python -m launchkit.cli, or the meta-forge launcher."""
    holder = []
    try:
        return asyncio.run(run_async(sys.argv[1:] if argv is None else argv, context_holder=holder))
    except KeyboardInterrupt:
        if holder and not holder[0].output.finished:
            error = interrupted(holder[0])
            holder[0].output.failure(holder[0].command, error)
        return EXIT_CODES["interrupted"]
