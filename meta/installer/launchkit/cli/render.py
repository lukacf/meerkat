"""Output modes and human renderings of Forge's own server data.

text: readable results on stdout; progress, prompts and diagnostics on stderr.
json: exactly one JSON document on stdout, also on failure.
jsonl: one event per line on stdout, ending with exactly one result or error.

Everything rendered here comes from the Forge server's presence-only reads or
from the command's own non-secret arguments.
"""
from __future__ import annotations

import json

FORMAT = "meta-forge-cli/1"
ACTIVE = {"queued", "installing", "checking", "starting", "stopping"}
SETTLED = {"running", "stopped", "failed", "interrupted"}
COLORS = {"ok": "32", "warn": "33", "fail": "31", "bold": "1", "dim": "2"}


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
            self.stdout.write(text + "\n")
            self.stdout.flush()

    def note(self, text):
        """A progress or diagnostic line on stderr (text mode, unless quiet)."""
        if self.mode == "text" and not self.quiet:
            self.stderr.write(text + "\n")
            self.stderr.flush()

    def trace(self, text):
        """Verbose diagnostics on stderr in every mode; never request or response bodies."""
        if self.verbose:
            self.stderr.write(text + "\n")
            self.stderr.flush()

    def prompt(self, text):
        self.stderr.write(text)
        self.stderr.flush()

    def event(self, command, kind, **fields):
        """A jsonl progress event; in text mode its message goes to stderr."""
        if self.mode == "jsonl":
            self._write({"format": FORMAT, "event": kind, "command": command, **fields})
        elif self.mode == "text" and fields.get("message") and (kind != "log" or self.verbose):
            self.note("  " + fields["message"])

    def _write(self, value):
        self.stdout.write(json.dumps(value, ensure_ascii=False, sort_keys=False) + "\n")
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
                    self.stderr.write(text + "\n")
            self.stderr.write(self.style("error", "fail") + f": {error.detail} ({error.code}, exit {error.exit_code})\n")
            if error.request is not None:
                self.stderr.write(f"  request: {error.request}\n")
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
            output.stderr.write("needs: the guide asks a question a person answers"
                                + (f": {need['person_runs']}" if need.get("person_runs") else "") + "\n")
        elif kind == "needs_secret":
            handles = need.get("one_of") or [need.get("handle")]
            output.stderr.write("needs: a person enters a secret outside this command: " + ", ".join(
                str(handle) for handle in handles) + "\n")
            if need.get("why"):
                output.stderr.write(f"  why: {need['why']}\n")
            if need.get("instruction"):
                output.stderr.write(f"  how: {need['instruction']}\n")
            if need.get("person_opens"):
                output.stderr.write(f"  open: {need['person_opens']}\n")
            if need.get("person_runs"):
                output.stderr.write(f"  run: {need['person_runs']}\n")
        else:
            output.stderr.write(f"needs: {need}\n")
    output.stderr.flush()


def render_next(output, steps, *, force=False):
    if not steps or (output.quiet and not force):
        return
    for item in steps:
        output.stderr.write(f"next: {item['command']}    # {item['reason']}\n")
    output.stderr.flush()


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
    summary = plan_summary(blueprint)
    agents = sum(len(item["identities"]) for item in summary["classes"])
    header = f"Plan ({label}" + (f", setup version {version}" if version is not None else "") + "): "
    header += f"\"{summary['project']['name']}\" ({summary['project']['slug']})"
    lines = [header,
             f"  Destination: {summary['destination']}",
             f"  Agents: {len(summary['classes'])} classes, {agents} identities; entrypoint "
             f"{summary['entrypoint']}; topology {summary['topology']}"]
    for item in summary["classes"]:
        names = ", ".join(item["identities"]) or "(no initial identities)"
        lines.append(f"    {names}: {item['name']} [{item['model']}]")
        lines.append("      tools: " + (", ".join(item["tools"]) or "none"))
    if summary["connections"]:
        lines.append("  Connections:")
        for item in summary["connections"]:
            where = item.get("url") if item.get("transport") == "http" else " ".join(
                str(part) for part in item.get("command") or [] if part)
            lines.append(f"    {item['id']} ({item['transport']}): {where}")
    else:
        lines.append("  Connections: none")
    if summary["flows"]:
        lines.append("  Flows:")
        for flow in summary["flows"]:
            lines.append(f"    {flow['id']}: {flow['name']}, {len(flow['steps'])} steps")
    else:
        lines.append("  Flows: none")
    lines.append(f"  Knowledge: {summary['knowledge']}")
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


def document_lines(value, indent="  "):
    """A review document as plain indented lines, in its own key order."""
    lines = []
    if isinstance(value, dict):
        for key, item in value.items():
            label = str(key).replace("_", " ")
            if isinstance(item, dict):
                lines.append(f"{indent}{label}:")
                lines.extend(document_lines(item, indent + "  "))
            elif isinstance(item, list):
                if not item:
                    lines.append(f"{indent}{label}: none")
                elif all(not isinstance(part, (dict, list)) for part in item):
                    lines.append(f"{indent}{label}: " + ", ".join(str(part) for part in item))
                else:
                    lines.append(f"{indent}{label}:")
                    for part in item:
                        nested = document_lines(part, indent + "    ")
                        if nested:
                            lines.append(indent + "  - " + nested[0].strip())
                            lines.extend(nested[1:])
            else:
                lines.append(f"{indent}{label}: {item}")
    elif isinstance(value, list):
        for part in value:
            lines.append(f"{indent}- {part}")
    else:
        lines.append(f"{indent}{value}")
    return lines


def review_lines(document, digest):
    title = document.get("title") if isinstance(document, dict) else None
    lines = [f"Review: {title}" if title else "Review:"]
    body = {key: item for key, item in document.items() if key not in ("format", "title")} if isinstance(
        document, dict) else document
    lines.extend(document_lines(body))
    if digest:
        lines.append(f"Digest: {digest}")
    return lines
