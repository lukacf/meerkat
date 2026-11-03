"""Presentation-only projection of one checked, currently staged guided design."""
from __future__ import annotations

import copy
import re

from .guide_models import publication_visualization_feedback


def staged_guide_preview(state, *, context, proposal, publication, active):
    """Never replace saved brief, questions, readiness or lifecycle evidence.

    The atomic edit path stages both checked halves together. Full authoring can
    stage a validated topology first; its missing brief stays explicitly absent,
    never borrowed from saved or earlier context. A previous same-guide turn is
    not this operation. Snapshot without persisting any caller-owned state.
    """
    if (active is not True or not all(isinstance(value, dict) for value in (state, context, proposal))
            or publication is not None and not isinstance(publication, dict)):
        return None
    operation = state.get("operation")
    if not isinstance(operation, dict):
        return None
    version, identity, request_id = state.get("version"), state.get("id"), operation.get("request_id")
    if (operation.get("status") != "running" or type(version) is not int or version < 1
            or type(operation.get("version")) is not int or operation["version"] != version
            or type(context.get("guide_version")) is not int or context["guide_version"] != version
            or not isinstance(identity, str) or not identity or context.get("guide_id") != identity
            or not isinstance(request_id, str) or not re.fullmatch(r"[a-f0-9]{64}", request_id)
            or context.get("operation_request_id") != request_id
            or not isinstance(proposal.get("blueprint"), dict)
            or publication is not None and not isinstance(publication.get("brief"), dict)):
        return None
    result = {"guide_id": identity, "guide_version": version, "request_id": request_id,
              "blueprint": copy.deepcopy(proposal["blueprint"]),
              "brief": copy.deepcopy(publication["brief"]) if publication is not None else None}
    if publication is None:
        result["context_pending"] = True
    elif "visualization_feedback" in publication or "visualization_feedback" in context:
        try:
            result["visualization_feedback"] = publication_visualization_feedback(publication, context)
        except ValueError:
            return None
    return result
