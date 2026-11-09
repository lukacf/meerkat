"""Review documents for the human approval gates, and their digests.

Each gated command builds its review from server data, prints it, and proceeds
only after a person confirms it at a terminal, or with --approve DIGEST naming
the exact review computed now. The digest is the SHA-256 of the canonical JSON
of the review document: keys sorted, no whitespace, ASCII escapes (the same
canonical form as Forge's own fingerprints). Any change since the review (a new
plan version, another folder, port or mode) changes the digest.
"""
from __future__ import annotations

import hashlib
import json
import re

from .errors import CliError, step
from .render import plan_summary, review_lines

REVIEW_FORMAT = "meta-forge-review/1"
DIGEST = re.compile(r"(?:sha256:)?([0-9a-f]{64})")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False).encode()


def sha256_of(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def review_digest(document):
    return "sha256:" + sha256_of(document)


def normalized_approval(value):
    match = DIGEST.fullmatch(value.strip().lower()) if isinstance(value, str) else None
    if match is None:
        raise CliError("usage", "--approve takes a review digest: sha256: followed by 64 hex digits.",
                       next=[step("meta-forge --help", "See how review digests work.")])
    return "sha256:" + match.group(1)


def plan_document(state):
    """Accept plan: what the setup's proposal becomes when accepted."""
    proposal = state.get("proposal") or {}
    readiness = state.get("readiness") or {}
    availability = state.get("package_availability") or {}
    summary = plan_summary(proposal)
    return {
        "format": REVIEW_FORMAT,
        "title": f"accept plan version {state.get('version')} of setup {str(state.get('id'))[:8]}",
        "gate": "accept-plan",
        "setup": state.get("id"),
        "version": state.get("version"),
        "effect": "Accepting saves this proposal as the setup's accepted plan. Nothing is installed or started.",
        **summary,
        "open_implementation_items": list(readiness.get("implementation_blockers") or []),
        "package_availability": {key: availability.get(key) for key in ("available", "status", "reason")
                                 if key in availability},
        "blueprint_sha256": sha256_of(proposal),
        "brief_sha256": sha256_of(state.get("brief") or {}),
    }


def organization_document(state):
    """Organization guidance: the text that joins this setup's guide context."""
    update = state.get("organization_update") or {}
    context = update.get("context") or {}
    return {
        "format": REVIEW_FORMAT,
        "title": f"adopt workplace guidance for setup {str(state.get('id'))[:8]}",
        "gate": "organization-guidance",
        "setup": state.get("id"),
        "version": state.get("version"),
        "effect": "The guidance becomes part of this setup's guide context and of its future builds. "
                  "Everyone who can read this setup can read it.",
        "organization": {key: context.get(key) for key in ("id", "version", "label", "description", "guide_context")},
        "fingerprint": update.get("fingerprint"),
    }


def start_document(state, receipt, action):
    """Start, restart or retry an installation: what starts and how it runs."""
    plan = None
    if receipt.get("current") is True and state.get("blueprint"):
        summary = plan_summary(state["blueprint"])
        plan = {"agents": [name for item in summary["classes"] for name in item["identities"]],
                "tools": {item["id"]: item["tools"] for item in summary["classes"]},
                "flows": [flow["id"] for flow in summary["flows"]],
                "connections": [item["id"] for item in summary["connections"]]}
    native = receipt.get("source_kind") == "native-team"
    return {
        "format": REVIEW_FORMAT,
        "title": f"{action} installation {str(receipt.get('deployment_id'))[:8]} of setup {str(state.get('id'))[:8]}",
        "gate": "start",
        "action": action,
        "setup": state.get("id"),
        "deployment": receipt.get("deployment_id"),
        "kind": "agent-team" if native else "application",
        "source": receipt.get("source_id") if native else receipt.get("build_id"),
        "package_sha256": receipt.get("package_sha256"),
        "folder": receipt.get("destination"),
        "mode": receipt.get("execution_mode", "interactive"),
        "current_status": receipt.get("status"),
        "starts": plan if plan is not None else "the installed package, from setup version "
                  f"{receipt.get('guide_version')} (the current plan differs or is not accepted)",
        "effect": ("Forge starts the installed agents and the tools they are granted, under this "
                   + ("operating-system user service." if receipt.get("execution_mode") == "background"
                      else "Forge server; stopping Forge stops them.")),
    }


def install_document(state, options, capabilities, *, folder, port, port_status, mode, settings, reused_keys):
    """Install: what is installed, where, how it runs and which settings it gets (never values of secrets)."""
    summary = plan_summary(state.get("blueprint") or {})
    modes = {item.get("id"): item for item in options.get("execution_modes") or [] if isinstance(item, dict)}
    chosen = modes.get(mode) or {}
    team = (capabilities or {}).get("agent_team_runtime") or {}
    runs = ("as an operating-system user service that keeps running without Forge" if mode == "background"
            else "while this Forge server runs; stopping Forge stops it")
    return {
        "format": REVIEW_FORMAT,
        "title": f"install the agent team of setup {str(state.get('id'))[:8]} (plan version {state.get('version')}) "
                 "on this computer",
        "gate": "install",
        "setup": state.get("id"),
        "version": state.get("version"),
        "what": {"kind": "agent-team", "project": summary["project"]["name"],
                 "agents": [name for item in summary["classes"] for name in item["identities"]],
                 "source_id": options.get("source_id"), "source_digest": options.get("source_digest")},
        "runtime": {"sdk": team.get("sdk"), "version": team.get("version"), "release": team.get("release")},
        "where": {"folder": folder, "computer": options.get("host_target"),
                  "rule": "an empty folder you own; Forge never overwrites a folder"},
        "port": {"number": port, "observed": port_status},
        "mode": {"id": mode, "label": chosen.get("label"), "description": chosen.get("description")},
        "settings": settings,
        "forge_provider_keys": ({"copied_by_forge": reused_keys,
                                 "note": "Forge copies its own saved keys server-side; they never go through "
                                         "this command."} if reused_keys else "not copied"),
        "connections_checked": [{key: value for key, value in item.items() if key != "name"}
                                for item in summary["connections"]],
        "effect": "Forge copies the agent team into the folder, installs its pinned runtime, writes its private "
                  f"settings file (mode 600), checks its connections and starts its agents, which run {runs}.",
    }


async def approve(ctx, document, *, question, approve_command):
    """Proceed only after a person's confirmation or a matching --approve digest.

    Returns the approved digest. Raises approval_required (exit 84) otherwise;
    nothing has been sent at that point.
    """
    digest = review_digest(document)
    approvals = {normalized_approval(value) for value in ctx.approvals}
    retry = step(f"{approve_command} --approve {digest}", "Approve exactly this review.")
    if approvals:
        if digest in approvals:
            return digest
        raise CliError("approval_required", "The review changed since it was approved, or the approval names "
                       "another review. Nothing was changed. Read the new review and approve its digest.",
                       review=document, digest=digest, next=[retry])
    if ctx.interactive:
        for text in review_lines(document, digest):
            ctx.output.prompt(text + "\n")
        answer = ctx.read_line(f"{question} [y/N] ")
        if answer is not None and answer.strip().lower() in ("y", "yes"):
            return digest
        raise CliError("approval_required", "Not approved. Nothing was changed.", review=document, digest=digest,
                       next=[retry])
    raise CliError("approval_required", "This step needs a person's approval. Read the review, then run the "
                   "command again with --approve and its digest.", review=document, digest=digest, next=[retry])
