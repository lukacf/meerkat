"""Output modes and human renderings of Forge's own server data.

text: readable results on stdout; progress, prompts and diagnostics on stderr.
json: exactly one JSON document on stdout, also on failure.
jsonl: one event per line on stdout, ending with exactly one result or error.

Everything rendered here comes from the Forge server's presence-only reads or
from the command's own non-secret arguments. Server text may carry characters a
terminal acts on (escape sequences, carriage returns) or hides (format
characters such as a right-to-left override): every text write shows them
escaped, and JSON output carries them as \\u escapes.
"""
from __future__ import annotations

import json
import re
import shlex

FORMAT = "meta-forge-cli/1"
REVIEW_FORMAT = "meta-forge-review/1"
ACTIVE = {"queued", "installing", "checking", "starting", "stopping"}
SETTLED = {"running", "stopped", "failed", "interrupted"}
COLORS = {"ok": "32", "warn": "33", "fail": "31", "bold": "1", "dim": "2"}
NAMED_ESCAPES = {"\n": "\\n", "\t": "\\t", "\r": "\\r"}
NUMBER = re.compile(r"-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][-+]?[0-9]+)?")


def escaped(character):
    """One character a terminal would act on or hide, as text: \\n, \\x1b, \\u202e."""
    code = ord(character)
    if character in NAMED_ESCAPES:
        return NAMED_ESCAPES[character]
    if code < 0x100:
        return f"\\x{code:02x}"
    return f"\\u{code:04x}" if code < 0x10000 else f"\\U{code:08x}"


def visible(text):
    """Text for a terminal: line breaks and tabs stay; every other character that is not printable (control,
    format and separator characters, such as ESC, a carriage return or a right-to-left override) is escaped."""
    text = str(text).replace("\r\n", "\n")
    if text.replace("\n", "").replace("\t", "").isprintable():
        return text
    return "".join(character if character in "\n\t" or character.isprintable() else escaped(character)
                   for character in text)


def json_escaped(character):
    code = ord(character)
    if code < 0x10000:
        return f"\\u{code:04x}"
    code -= 0x10000
    return f"\\u{0xd800 + (code >> 10):04x}\\u{0xdc00 + (code & 0x3ff):04x}"


def json_text(value, **options):
    """JSON text of a value, every character printable: JSON escapes control characters itself; the other
    characters a terminal would act on or hide become \\u escapes. Parsing it gives the same value."""
    text = json.dumps(value, ensure_ascii=False, **options)
    if text.replace("\n", "").isprintable():
        return text
    return "".join(character if character == "\n" or character.isprintable() else json_escaped(character)
                   for character in text)


class Output:
    def __init__(self, mode, stdout, stderr, *, verbose=0, quiet=False, environ=None):
        self.mode = mode
        self.stdout = stdout
        self.stderr = stderr
        self.verbose = verbose
        self.quiet = quiet
        environ = environ or {}
        self.color = (mode == "text" and "NO_COLOR" not in environ
                      and getattr(stdout, "isatty", lambda: False)())
        self.finished = False

    def style(self, text, kind):
        if not self.color or kind not in COLORS:
            return text
        return f"\x1b[{COLORS[kind]}m{text}\x1b[0m"

    def line(self, text=""):
        """A line of the human result on stdout (text mode only)."""
        if self.mode == "text":
            self.stdout.write(visible(text) + "\n")
            self.stdout.flush()

    def status_line(self, status, text):
        """A result line led by its ok, warn or fail status, in color on a terminal (text mode only)."""
        if self.mode == "text":
            self.stdout.write(self.style(status, status) + visible(text) + "\n")
            self.stdout.flush()

    def note(self, text):
        """A progress or diagnostic line on stderr (text mode, unless quiet)."""
        if self.mode == "text" and not self.quiet:
            self.say(text)

    def say(self, text):
        """A line on stderr, whatever the mode."""
        self.stderr.write(visible(text) + "\n")
        self.stderr.flush()

    def trace(self, text):
        """Verbose diagnostics on stderr in every mode; never request or response bodies."""
        if self.verbose:
            self.say(text)

    def prompt(self, text):
        self.stderr.write(visible(text))
        self.stderr.flush()

    def event(self, command, kind, **fields):
        """A jsonl progress event; in text mode its message goes to stderr."""
        if self.mode == "jsonl":
            self._write({"format": FORMAT, "event": kind, "command": command, **fields})
        elif self.mode == "text" and fields.get("message") and (kind != "log" or self.verbose):
            self.note("  " + fields["message"])

    def _write(self, value):
        self.stdout.write(json_text(value) + "\n")
        self.stdout.flush()

    def envelope(self, command, *, ok, result=None, error=None, setup=None, request=None, needs=(), next=(),
                 extra=None):
        value = {"format": FORMAT, "ok": ok, "command": command}
        if setup is not None:
            value["setup"] = setup
        if ok:
            value["result"] = result
        else:
            value["error"] = error
            if result is not None:
                value["result"] = result
        if request is not None:
            value["request"] = request
        value.update(extra or {})
        value["needs"] = list(needs)
        value["next"] = list(next)
        return value

    def success(self, command, result, *, lines=(), setup=None, request=None, needs=(), next=(), extra=None):
        self.finished = True
        if self.mode == "text":
            for text in lines:
                self.line(text)
            render_needs(self, needs)
            render_next(self, next)
            return
        value = self.envelope(command, ok=True, result=result, setup=setup, request=request, needs=needs,
                              next=next, extra=extra)
        if self.mode == "jsonl":
            value = {"format": FORMAT, "event": "result", **{k: v for k, v in value.items() if k != "format"}}
        self._write(value)

    def failure(self, command, error, *, setup=None):
        self.finished = True
        extra = {}
        if error.review is not None:
            extra["review"] = error.review
        if error.digest is not None:
            extra["digest"] = error.digest
        if self.mode == "text":
            if error.review is not None:
                for text in review_lines(error.review, error.digest):
                    self.say(text)
            self.stderr.write(self.style("error", "fail")
                              + visible(f": {error.detail} ({error.code}, exit {error.exit_code})") + "\n")
            if error.request is not None:
                self.say(f"  request: {error.request}")
            self.stderr.flush()
            render_needs(self, error.needs, force=True)
            render_next(self, error.next, force=True)
            return
        value = self.envelope(command, ok=False, error=error.error_object(), result=error.result, setup=setup,
                              request=error.request, needs=error.needs, next=error.next, extra=extra)
        if self.mode == "jsonl":
            value = {"format": FORMAT, "event": "error", **{k: v for k, v in value.items() if k != "format"}}
        self._write(value)


def render_needs(output, needs, *, force=False):
    if not needs or (output.quiet and not force):
        return
    for need in needs:
        kind = need.get("kind")
        if kind == "question":
            output.say("needs: the guide asks a question a person answers"
                       + (f": {need['person_runs']}" if need.get("person_runs") else ""))
        elif kind == "needs_secret":
            handles = need.get("one_of") or [need.get("handle")]
            output.say("needs: a person enters a secret outside this command: " + ", ".join(
                str(handle) for handle in handles))
            if need.get("why"):
                output.say(f"  why: {need['why']}")
            if need.get("instruction"):
                output.say(f"  how: {need['instruction']}")
            if need.get("person_opens"):
                output.say(f"  open: {need['person_opens']}")
            if need.get("person_runs"):
                output.say(f"  run: {need['person_runs']}")
        else:
            output.say(f"needs: {need}")


def render_next(output, steps, *, force=False):
    if not steps or (output.quiet and not force):
        return
    for item in steps:
        output.say(f"next: {item['command']}    # {item['reason']}")


# Plan (blueprint) renderings. A blueprint is the server's saved plan data.

def identities(item):
    """Durable identities of one agent class, as the blueprint derives them."""
    members = item.get("members") or []
    if members:
        return [f"{item.get('id')}:{member.get('key')}" for member in members if isinstance(member, dict)]
    count = item.get("instances", 1)
    if not isinstance(count, int) or count < 0:
        return []
    if count == 1:
        return [f"{item.get('id')}:main"]
    return [f"{item.get('id')}:{index}" for index in range(1, count + 1)]


def tool_labels(tools):
    """A class's tool grants in words: categories, then each MCP connection's exact tools."""
    tools = tools if isinstance(tools, dict) else {}
    labels = []
    if tools.get("read_only"):
        labels.append("read-only tools only")
    if tools.get("workspace"):
        labels.append("workspace")
    notes = tools.get("notes", "none")
    if notes and notes != "none":
        labels.append(f"notes({notes})")
    for key, label in (("builtins", "builtins"), ("shell", "shell"), ("memory", "memory"),
                       ("workgraph", "workgraph"), ("schedule", "scheduling"), ("knowledge", "knowledge")):
        if tools.get(key, key == "memory"):
            labels.append(label)
    for grant in tools.get("mcp") or []:
        if isinstance(grant, dict):
            names = grant.get("tool_names") or []
            labels.append(f"mcp:{grant.get('connection_id')}(" + (", ".join(names) if names else "all tools") + ")")
    for bundle in tools.get("rust_bundles") or []:
        labels.append(f"native:{bundle}")
    return labels


def connection_summary(item):
    if not isinstance(item, dict):
        return {}
    value = {"id": item.get("id"), "name": item.get("name"), "transport": item.get("transport")}
    if item.get("transport") == "http":
        value["url"] = item.get("url")
    else:
        value["command"] = [item.get("command"), *(item.get("args") or [])]
    return value


def plan_summary(blueprint):
    """The plan facts a review shows, in a stable order."""
    blueprint = blueprint if isinstance(blueprint, dict) else {}
    topology = blueprint.get("topology") or {}
    classes = []
    for item in blueprint.get("agent_classes") or []:
        if not isinstance(item, dict):
            continue
        entry = {"id": item.get("id"), "name": item.get("name"), "identities": identities(item),
                 "model": f"{item.get('provider')}/{item.get('model')}", "tools": tool_labels(item.get("tools"))}
        if item.get("model_auth") and item.get("model_auth") != "api_key":
            entry["model_auth"] = item.get("model_auth")
        classes.append(entry)
    flows = [{"id": flow.get("id"), "name": flow.get("name"),
              "steps": [{"id": part.get("id"), "agent": part.get("agent")} for part in flow.get("steps") or []
                        if isinstance(part, dict)]}
             for flow in blueprint.get("flows") or [] if isinstance(flow, dict)]
    knowledge = blueprint.get("knowledge") or {}
    project = blueprint.get("project") or {}
    return {
        "project": {"name": project.get("name"), "slug": project.get("slug")},
        "destination": blueprint.get("target"),
        "entrypoint": topology.get("entrypoint"),
        "topology": topology.get("mode"),
        "classes": classes,
        "connections": [connection_summary(item) for item in blueprint.get("connections") or []],
        "flows": flows,
        "knowledge": knowledge.get("mode", "none") if isinstance(knowledge, dict) else "none",
    }


def plan_lines(blueprint, *, label, version=None, readiness=None, availability=None):
    """A plan as a person reads it: names and list items quoted where a bare reading could mislead, commands
    as a shell would read them."""
    summary = plan_summary(blueprint)
    agents = sum(len(item["identities"]) for item in summary["classes"])
    header = f"Plan ({label}" + (f", setup version {version}" if version is not None else "") + "): "
    header += f"{quoted(str(summary['project']['name']))} ({scalar(summary['project']['slug'])})"
    lines = [header,
             f"  Destination: {scalar(summary['destination'])}",
             f"  Agents: {len(summary['classes'])} classes, {agents} identities; entrypoint "
             f"{scalar(summary['entrypoint'])}; topology {scalar(summary['topology'])}"]
    for item in summary["classes"]:
        names = ", ".join(map(scalar, item["identities"])) or "(no initial identities)"
        lines.append(f"    {names}: {scalar(item['name'])} [{scalar(item['model'])}]")
        lines.append("      tools: " + (", ".join(map(scalar, item["tools"])) or "none"))
    if summary["connections"]:
        lines.append("  Connections:")
        for item in summary["connections"]:
            where = scalar(item.get("url")) if item.get("transport") == "http" else shlex.join(
                str(part) for part in item.get("command") or [] if part is not None)
            lines.append(f"    {scalar(item['id'])} ({scalar(item['transport'])}): {where}")
    else:
        lines.append("  Connections: none")
    if summary["flows"]:
        lines.append("  Flows:")
        for flow in summary["flows"]:
            lines.append(f"    {scalar(flow['id'])}: {scalar(flow['name'])}, {len(flow['steps'])} steps")
    else:
        lines.append("  Flows: none")
    lines.append(f"  Knowledge: {scalar(summary['knowledge'])}")
    if isinstance(readiness, dict):
        decisions = readiness.get("decision_blockers") or []
        work = readiness.get("implementation_blockers") or []
        lines.append("  Open decisions: " + ("; ".join(decisions) if decisions else "none"))
        lines.append("  Open implementation items: " + ("; ".join(work) if work else "none"))
    if isinstance(availability, dict):
        if availability.get("available") is False:
            lines.append(f"  Package: unavailable ({availability.get('status')}): {availability.get('reason')}")
        else:
            lines.append("  Package: available")
    return lines


# Setup (saved guided conversation) renderings.

def setup_name(state):
    plan = state.get("proposal") or state.get("blueprint") or {}
    return (plan.get("project") or {}).get("name") or "New agent system"


def last_guide_message(state):
    for message in reversed(state.get("messages") or []):
        if isinstance(message, dict) and message.get("role") == "assistant":
            return message.get("text") or ""
    return ""


def question_lines(question, *, chat=False):
    if not isinstance(question, dict):
        return []
    lines = [f"Question: {question.get('title')}"]
    if question.get("description"):
        lines.append(f"  {question['description']}")
    options = question.get("options") or []
    for index, option in enumerate(options, start=1):
        lines.append(f"  {index}) {option.get('label')} [{option.get('id')}]: {option.get('description')}")
    if options:
        if chat:
            lines.append("  [number, or 1,3 when more than one fits]  [text after the number adds context]  [/help]"
                         if not question.get("multiple") else
                         "  [one or more numbers, such as 1,3]  [text after the numbers adds context]  [/help]")
        else:
            lines.append("  Answer with --answer ID (repeat it when more than one fits; add --combine for a "
                         "single-choice question) and optional --text." if not question.get("multiple") else
                         "  Answer with one or more --answer ID and optional --text.")
    else:
        lines.append("  Answer in your own words." if chat else "  Answer with --text.")
    return lines


def operation_text(operation):
    if not isinstance(operation, dict):
        return "none"
    status = operation.get("status")
    if status == "running":
        return f"running since {operation.get('started_at')}"
    if status in ("failed", "interrupted"):
        return f"{status}: {operation.get('detail')}"
    return str(status)


def setup_lines(state):
    plan_label = "proposed" if state.get("proposal") else "accepted" if state.get("blueprint") else None
    lines = [f"Setup {state.get('id', '')[:8]} \"{setup_name(state)}\" (version {state.get('version')})",
             f"  ID: {state.get('id')}",
             f"  Status: {state.get('status')}, stage {state.get('stage')}",
             f"  Turn: {operation_text(state.get('operation'))}"]
    if plan_label:
        plan = state.get("proposal") or state.get("blueprint")
        summary = plan_summary(plan)
        agents = sum(len(item["identities"]) for item in summary["classes"])
        lines.append(f"  Plan: {plan_label}, {len(summary['classes'])} classes, {agents} agents")
    else:
        lines.append("  Plan: none yet")
    readiness = state.get("readiness") or {}
    blockers = readiness.get("decision_blockers") or []
    lines.append("  Open decisions: " + ("; ".join(blockers) if blockers else "none"))
    if state.get("organization_update"):
        context = state["organization_update"].get("context") or {}
        lines.append(f"  Workplace guidance available: {context.get('label')} (meta-forge setup organization)")
    message = last_guide_message(state)
    if message:
        lines.append(f"Guide: {message}")
    lines.extend(question_lines(state.get("question")))
    return lines


# Installation (deployment receipt) renderings.

def receipt_lines(receipt):
    if not isinstance(receipt, dict):
        return []
    native = receipt.get("source_kind") == "native-team"
    kind = (f"agent team (source {str(receipt.get('source_digest'))[:12]})" if native
            else f"application build {receipt.get('build_id')}")
    mode = receipt.get("execution_mode", "interactive")
    lines = [f"Installation {str(receipt.get('deployment_id'))[:8]}: {receipt.get('status')} "
             f"(phase {receipt.get('phase')}, health {receipt.get('health', 'unknown')})",
             f"  Kind: {kind}",
             f"  Folder: {receipt.get('destination')}",
             "  Mode: " + ("background (an operating-system user service; it keeps running without Forge)"
                           if mode == "background" else "interactive (runs while this Forge server runs)")]
    if receipt.get("console_url"):
        lines.append(f"  Console: {receipt['console_url']}")
    checks = receipt.get("checks") or []
    if checks:
        lines.append("  Checks: " + ", ".join(f"{item.get('name')}: {item.get('status')}"
                                             for item in checks if isinstance(item, dict)))
    if receipt.get("detail"):
        lines.append(f"  Detail: {receipt['detail']}")
    cleanup = receipt.get("cleanup_recovery")
    if isinstance(cleanup, dict):
        lines.append("  Cleanup: " + ("verified cleanup is available (meta-forge installation cleanup)"
                                      if cleanup.get("available") else str(cleanup.get("reason"))))
    if receipt.get("unresolved"):
        lines.append("  Still to check yourself: " + "; ".join(str(item) for item in receipt["unresolved"]))
    return lines


def bare(text, *, key=False):
    """Whether a string reads as itself without quotes: printable, not padded, not empty, without quotes,
    backslashes, brackets or ", " (the separator of list items), and not readable as null, true, false or a
    number. A key also has no ": " and does not open like a list item."""
    return (text != "" and text.isprintable() and text == text.strip() and not any(c in text for c in '"\\[]{}')
            and ", " not in text and text not in ("null", "true", "false") and not NUMBER.fullmatch(text)
            and not (key and (": " in text or text.startswith("- "))))


def quoted(text):
    """A string in double quotes, with quotes, backslashes and every character that is not printable escaped."""
    return '"' + "".join("\\\\" if c == "\\" else '\\"' if c == '"' else c if c.isprintable() else escaped(c)
                         for c in text) + '"'


def scalar(value, *, key=False):
    """One value as a person reads it: strings bare where that cannot mislead, else quoted; null, true and false
    as in JSON."""
    if isinstance(value, str):
        return value if bare(value, key=key) else quoted(value)
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return json.dumps(value)
    return quoted(str(value))


def nested(value):
    """Whether a value takes lines of its own: a nonempty object, or a list holding objects or lists."""
    return (isinstance(value, dict) and bool(value)) or (
        isinstance(value, list) and any(isinstance(item, (dict, list)) for item in value))


def inline(value):
    if isinstance(value, dict):
        return "{}"
    if isinstance(value, list):
        return "[" + ", ".join(scalar(item) for item in value) + "]"
    return scalar(value)


def document_lines(value, indent="  "):
    """A review document as indented lines that stand for exactly that document: keys as they are, lists in
    brackets or as "- " items, and strings quoted with escapes wherever a bare reading could mislead."""
    if isinstance(value, dict) and value:
        lines = []
        for key, item in value.items():
            label = scalar(str(key), key=True)
            if nested(item):
                lines.append(f"{indent}{label}:")
                lines.extend(document_lines(item, indent + "  "))
            else:
                lines.append(f"{indent}{label}: {inline(item)}")
        return lines
    if isinstance(value, list) and nested(value):
        lines = []
        for item in value:
            if nested(item):
                inner = document_lines(item, indent + "  ")
                lines.append(f"{indent}- {inner[0].strip()}")
                lines.extend(inner[1:])
            else:
                lines.append(f"{indent}- {inline(item)}")
        return lines
    return [f"{indent}{inline(value)}"]


def review_lines(document, digest):
    """The review a person reads before approving: every key of the document (its title as the heading, its
    constant format left out), rendered by document_lines. The digest covers exactly this document."""
    if not isinstance(document, dict):
        return ["Review:", *document_lines(document)] + ([f"Digest: {digest}"] if digest else [])
    title = document.get("title")
    lines = [f"Review: {scalar(title)}" if "title" in document else "Review:"]
    body = {key: item for key, item in document.items()
            if key != "title" and not (key == "format" and item == REVIEW_FORMAT)}
    if body:
        lines.extend(document_lines(body))
    if digest:
        lines.append(f"Digest: {digest}")
    return lines
