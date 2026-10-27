"""Validated conversation questions and evidence-linked design understanding."""
from __future__ import annotations

import copy
import re
from datetime import datetime, timezone
from typing import Annotated, Literal
from uuid import uuid4

from pydantic import Field, StringConstraints, field_validator, model_serializer, model_validator

from .blueprint import MAX_IDENTITY_LENGTH
from .schema import StrictModel

STAGES = ("purpose", "connections", "knowledge", "autonomy", "destination", "review")
QuestionStage = Literal["purpose", "connections", "knowledge", "autonomy", "destination"]
FactTopic = Literal["outcome", "audience", "interfaces", "ownership", "inputs", "workflow", "knowledge", "authority", "destination", "first_version"]
TOPIC_GROUPS = {
    "purpose": ("outcome", "audience", "interfaces", "ownership", "workflow"),
    "connections": ("inputs",), "knowledge": ("knowledge",),
    "autonomy": ("authority",), "destination": ("destination",),
}
REVIEW_TOPICS = tuple(topic for topics in TOPIC_GROUPS.values() for topic in topics)


class GuideOption(StrictModel):
    id: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,59}$")
    label: str = Field(min_length=1, max_length=100)
    description: str = Field(min_length=1, max_length=600)


class GuideQuestion(StrictModel):
    stage: QuestionStage
    id: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,59}$")
    title: str = Field(min_length=1, max_length=240)
    description: str = Field(min_length=1, max_length=2000)
    multiple: bool = Field(strict=True)
    options: list[GuideOption] = Field(default_factory=list, max_length=6)
    allow_text: Literal[True] = True

    @model_validator(mode="after")
    def usable_options(self):
        if len(self.options) == 1:
            raise ValueError("Offer at least two alternatives, or use no options for a text question.")
        if not self.options and self.multiple:
            raise ValueError("A text question cannot request multiple selections.")
        if len({choice.id for choice in self.options}) != len(self.options):
            raise ValueError("Give every answer option its own ID.")
        return self


class BriefFact(StrictModel):
    summary: str = Field(min_length=1, max_length=1600)
    status: Literal["confirmed", "proposed", "unresolved"]
    source_message_ids: list[str] = Field(default_factory=list, max_length=40)

    @model_validator(mode="after")
    def grounded_confirmation(self):
        if self.status == "confirmed" and not self.source_message_ids:
            raise ValueError("A confirmed fact needs supporting user message IDs.")
        if len(self.source_message_ids) != len(set(self.source_message_ids)):
            raise ValueError("Fact source IDs must be distinct.")
        return self


class RequestedFeature(StrictModel):
    id: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,79}$")
    outcome: str = Field(min_length=1, max_length=1200)
    supported_subset: str = Field(default="", max_length=1600)
    extension: str = Field(default="", max_length=1600)
    disposition: Literal["open", "supported", "prerequisite", "deferred", "cancelled", "handoff"]
    source_message_ids: list[str] = Field(min_length=1, max_length=40)
    decision_message_ids: list[str] = Field(default_factory=list, max_length=40)

    @model_validator(mode="after")
    def honest_disposition(self):
        if self.disposition in {"supported", "prerequisite"} and self.extension:
            raise ValueError("Missing application work is an open or deferred extension, not an installation prerequisite.")
        if self.disposition in {"deferred", "cancelled", "handoff"} and not self.decision_message_ids:
            raise ValueError("Deferring, cancelling or handing off a requested feature needs the user's later decision message ID.")
        return self


class IncomingDataRoute(StrictModel):
    """Desired recipient, not a runtime subscription or an access grant."""

    recipient: str = Field(min_length=1, max_length=MAX_IDENTITY_LENGTH)
    delivery: Literal["direct", "triage", "conversation"]
    recipient_kind: Literal["identity", "class"] = Field(default="identity",
        description="A proposed identity, or a whole proposed class including future members. A class is desired scope, not a native role selector or access grant.")

    @model_serializer(mode="wrap")
    def omit_default_recipient_kind(self, handler):
        data = handler(self)
        if self.recipient_kind == "identity":
            data.pop("recipient_kind", None)
        return data

    @model_validator(mode="after")
    def single_conversation_or_triage_owner(self):
        if self.recipient_kind == "class":
            from .blueprint import ID
            if not re.fullmatch(ID, self.recipient):
                raise ValueError("Choose a valid Blueprint class ID for a class recipient.")
        elif len(self.recipient) < 3:
            raise ValueError("An identity recipient must have at least 3 characters.")
        if self.recipient_kind == "class" and self.delivery != "direct":
            raise ValueError("Choose one identity for triage or conversation ownership; class routes describe direct awareness.")
        return self


class IncomingDataIntent(StrictModel):
    """Human-readable source intent, before resolving a native source binding."""

    id: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,79}$")
    requirement_id: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,79}$")
    name: str = Field(min_length=1, max_length=100)
    scope: str = Field(min_length=1, max_length=1000)
    purpose: str = Field(min_length=1, max_length=1000)
    audience: str = Field(default="", max_length=1000)
    history: Literal["undecided", "new_only", "bounded_history"] = "undecided"
    history_window: str = Field(default="", max_length=500)
    routes: list[IncomingDataRoute] = Field(default_factory=list, max_length=64)
    source_message_ids: list[str] = Field(min_length=1, max_length=40)

    @model_validator(mode="after")
    def coherent_intent(self):
        recipients = [(route.recipient_kind, route.recipient) for route in self.routes]
        if len(recipients) != len(set(recipients)):
            raise ValueError("Choose one incoming route per source and recipient; conversation and awareness routes must not overlap.")
        if self.history == "bounded_history" and not self.history_window:
            raise ValueError("Describe the requested history window before proposing a backfill.")
        if self.history != "bounded_history" and self.history_window:
            raise ValueError("A history window belongs only to a bounded-history choice.")
        if len(self.source_message_ids) != len(set(self.source_message_ids)):
            raise ValueError("Incoming data evidence IDs must be distinct.")
        return self


class DesignBrief(StrictModel):
    facts: dict[FactTopic, BriefFact] = Field(default_factory=dict)
    requirements: list[RequestedFeature] = Field(default_factory=list, max_length=32)
    incoming_data: list[IncomingDataIntent] = Field(default_factory=list, max_length=32,
        description="Optional proposed incoming sources and routing choices. This is user intent, not native configuration, provider capability evidence or permission to collect. Track implementation and access in the linked requirement.")

    @model_serializer(mode="wrap")
    def omit_unused_incoming_data(self, handler):
        data = handler(self)
        if not self.incoming_data:
            data.pop("incoming_data", None)
        return data

    @model_validator(mode="after")
    def unique_requirements(self):
        if len({item.id for item in self.requirements}) != len(self.requirements):
            raise ValueError("Give each requested feature its own ID.")
        if len({item.id for item in self.incoming_data}) != len(self.incoming_data):
            raise ValueError("Give each incoming source its own ID.")
        requirements = {item.id for item in self.requirements}
        if any(item.requirement_id not in requirements for item in self.incoming_data):
            raise ValueError("Link incoming data to an existing requested feature so its implementation remains visible.")
        return self


class GuidePublication(StrictModel):
    publication_mode: Literal["advance", "refinement"] = "advance"
    question: GuideQuestion | None
    message: str = Field(min_length=1, max_length=6000)
    brief: DesignBrief
    design_changed: bool = Field(default=False, strict=True)
    summary: str = Field(default="", max_length=6000,
        description="Short user-facing summary of the proposal or what changed. Do not include directions to rendering or implementation workers.")
    visualization_feedback: Annotated[str, StringConstraints(strip_whitespace=False)] | None = Field(default=None, strict=True, max_length=1000,
        description="Freeform preferences for Forge's proposal animation, outside application scope. Omitted or null preserves existing preferences; an empty string clears them.")
    warnings: list[str] = Field(default_factory=list, max_length=12)

    @field_validator("visualization_feedback")
    @classmethod
    def readable_visualization_feedback(cls, value):
        return _visualization_feedback_text(value)

    @model_validator(mode="after")
    def short_warnings(self):
        if self.publication_mode == "refinement" and self.question is not None:
            raise ValueError("A refinement update saves the proposal without a next question. Use question:null.")
        if any(len(item) > 1000 for item in self.warnings):
            raise ValueError("Keep each warning under 1000 characters.")
        return self


class GuidePublicationMissing(ValueError):
    """A native turn ended without a validated, durable guide response."""


def _visualization_feedback_text(value):
    if value is not None and (not isinstance(value, str) or len(value) > 1000
            or value != "" and not value.strip()
            or re.search(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f\ud800-\udfff]", value)):
        raise ValueError("Visualization feedback must be at most 1000 readable characters.")
    return value


def publication_visualization_feedback(publication, context):
    """Project animation preferences using the actual host-owned user message."""
    text = _visualization_feedback_text(publication.get("visualization_feedback"))
    if text is None:
        return copy.deepcopy(context.get("visualization_feedback"))
    if text == "":
        return None
    source = (context.get("current_user_message") or {}).get("id")
    if (not isinstance(source, str) or not source.strip() or len(source) > 256
            or re.search(r"[\x00-\x1f\x7f\ud800-\udfff]", source)):
        raise ValueError("Visualization feedback needs the current host-owned user message ID.")
    return {"text": text, "source_message_id": source}


def status_only_publication(value, context, proposal):
    """Recognize operational guidance without granting final design readiness.

    The saved-plan flag is host context, never a model publication field. Full
    canonical brief equality is required even when the brief still has gaps.
    """
    if not isinstance(value, GuidePublication):
        value = GuidePublication.model_validate(value)
    return bool(value.publication_mode == "advance" and value.question is None and value.design_changed is False
        and proposal is None and context.get("saved_plan_available") is True
        and context.get("action") == "chat"
        and context.get("manual_plan_submitted") is not True
        and value.brief.model_dump(mode="json") == DesignBrief.model_validate(
            context.get("brief") or {}).model_dump(mode="json"))


def review_readiness(brief: dict):
    """Readiness is grounded understanding, not a fixed number of answers."""
    value = DesignBrief.model_validate(brief)
    blockers = []
    decision_blockers, implementation_blockers = [], []

    def decision(issue):
        blockers.append(issue)
        decision_blockers.append(issue)

    def implementation(issue):
        blockers.append(issue)
        implementation_blockers.append(issue)

    outcome = value.facts.get("outcome")
    if not outcome or outcome.status != "confirmed":
        decision("Understand a concrete first job and useful output before designing the final team.")
    for topic in REVIEW_TOPICS:
        fact = value.facts.get(topic)
        if topic != "outcome" and (not fact or fact.status == "unresolved"):
            decision(f"Resolve {topic.replace('_', ' ')} or show an explicit proposed default.")
    context_ready = not blockers
    for requirement in value.requirements:
        if requirement.disposition == "open":
            if requirement.extension:
                implementation(f"Implementation still required: {requirement.outcome}")
            else:
                decision(f"Agree how to handle: {requirement.outcome}")
        elif requirement.disposition == "handoff":
            implementation(f"Implementation still required: {requirement.outcome}")
    if any(item.disposition == "deferred" for item in value.requirements):
        first_version = value.facts.get("first_version")
        if not first_version or first_version.status != "confirmed":
            decision("Confirm the useful first version when deferring a requested feature.")
    return {"ready": not blockers, "context_ready": context_ready, "blockers": blockers,
            "decision_blockers": decision_blockers, "implementation_blockers": implementation_blockers,
            "confirmed_topics": [topic for topic, fact in value.facts.items() if fact.status == "confirmed"],
            "proposed_topics": [topic for topic, fact in value.facts.items() if fact.status == "proposed"]}


def validate_guide_publication(args: dict, context: dict, proposal: dict | None, *, current_blueprint=None):
    """Validate model output against actual user evidence and design readiness."""
    value = GuidePublication.model_validate(args)
    messages = context.get("evidence_messages", [])
    evidence = {message["id"]: message for message in messages if message.get("role") == "user"}
    positions = {message["id"]: index for index, message in enumerate(messages)}
    old = DesignBrief.model_validate(context.get("brief") or {})
    missing = set(old.facts) - set(value.brief.facts)
    if missing:
        raise ValueError(f"Preserve independent brief facts or explicitly revise them: {', '.join(sorted(missing))}.")
    old_requirements = {item.id for item in old.requirements}
    if old_requirements - {item.id for item in value.brief.requirements}:
        raise ValueError("Do not drop a requested feature. Update its disposition and supporting evidence.")
    old_sources = {item.id for item in old.incoming_data}
    if old_sources - {item.id for item in value.brief.incoming_data}:
        raise ValueError("Do not drop an incoming source. Revise its choices and record scope decisions in its linked feature.")
    for topic, fact in value.brief.facts.items():
        if any(identity not in evidence for identity in fact.source_message_ids):
            raise ValueError(f"The {topic} fact cites a missing or non-user message. Use evidence_messages IDs.")
        if topic == "outcome" and fact.status == "confirmed":
            if not any(not evidence[identity].get("intent_only") for identity in fact.source_message_ids):
                raise ValueError("A broad category is not a concrete outcome. Ask about one useful first job without proposing a team.")
    for requirement in value.brief.requirements:
        ids = requirement.source_message_ids + requirement.decision_message_ids
        if any(identity not in evidence for identity in ids):
            raise ValueError(f"The {requirement.id} feature cites missing or non-user evidence.")
        if requirement.disposition in {"deferred", "cancelled", "handoff"}:
            requested_at = min(positions[identity] for identity in requirement.source_message_ids)
            if not any(positions[identity] > requested_at for identity in requirement.decision_message_ids):
                raise ValueError("The feature request itself cannot authorize its deferral, cancellation or handoff. Ask for the user's choice.")
    selected = proposal if proposal is not None else current_blueprint
    # The initial turn carries compiler scaffolding as current_blueprint. Only
    # an actual proposal or an existing saved design can supply recipients.
    if proposal is None and context.get("saved_plan_available") is False:
        selected = None
    class_recipients = None
    identities = set()
    for source in value.brief.incoming_data:
        if any(identity not in evidence for identity in source.source_message_ids):
            raise ValueError("Incoming data cites missing or non-user evidence.")
        # Validate against the same desired plan being published, never a native
        # roster or a guessed provider scope. Runtime admission is separate.
        if source.routes:
            from .blueprint import Blueprint
            if not selected:
                raise ValueError("Propose the team before assigning incoming data recipients.")
            if class_recipients is None:
                route_plan = Blueprint.model_validate(selected)
                # One immutable membership snapshot for this publication. The
                # overlap accumulator below still belongs to each source.
                class_recipients = {item.id: frozenset(item.identities()) for item in route_plan.agent_classes}
                identities = {identity for members in class_recipients.values() for identity in members}
            selected_identities = set()
            for route in source.routes:
                if route.recipient_kind == "class":
                    if route.recipient not in class_recipients:
                        raise ValueError("Incoming data class routes must name an existing class in the proposed team.")
                    recipients = class_recipients[route.recipient]
                else:
                    if route.recipient not in identities:
                        raise ValueError("Incoming data routes must name an existing identity in the proposed team.")
                    recipients = {route.recipient}
                if selected_identities & recipients:
                    raise ValueError("Choose one incoming route per source and recipient; class and identity routes must not overlap.")
                selected_identities.update(recipients)
    outcome = value.brief.facts.get("outcome")
    if (context.get("current_user_message", {}).get("intent_only")
            and (not outcome or outcome.status != "confirmed")
            and (value.question is None or value.question.stage != "purpose")):
        raise ValueError("Keep the purpose open after a broad category. Ask for a concrete first job before other setup details.")
    if proposal is not None and (not outcome or outcome.status != "confirmed"):
        raise ValueError("Understand a concrete outcome before publishing a personalized team. A category needs a useful follow-up.")
    if (value.design_changed or context.get("action") == "manual") and proposal is None:
        raise ValueError("Validate and propose the changed plan before publishing its update.")
    if value.publication_mode == "refinement":
        if proposal is None:
            raise ValueError("Validate and propose the current blueprint before saving a refinement update.")
    elif value.question is None and not status_only_publication(value, context, proposal):
        readiness = review_readiness(value.brief.model_dump(mode="json"))
        if not readiness["ready"]:
            raise ValueError("The brief is not ready for review: " + " ".join(readiness["blockers"]))
        if proposal is None:
            raise ValueError("Validate and propose the complete blueprint before final review.")
    if (context.get("saved_plan_available") is False and proposal is None
            and outcome is not None and outcome.status == "confirmed"):
        raise ValueError("Publish a purpose-specific first draft with propose_blueprint before the next guided step. The draft can remain provisional while you ask about missing audience, operating context or configuration.")
    return value.model_dump(mode="json")


def conversation_message(role: str, text: str, *, stage: str, action: str, **details):
    return {"id": str(uuid4()), "role": role, "text": text,
            "created_at": datetime.now(timezone.utc).isoformat(),
            "stage": stage, "action": action, **details}


def default_question(stage: str, answers: list[dict] | None = None):
    """Immediate opening and neutral revisit prompts; the agent owns follow-ups."""
    del answers
    if stage == "review":
        return None
    questions = {
        "purpose": {
            "title": "What do you want to build?",
            "description": "Describe the system you have in mind: who it helps, what it should notice, and what it should do. I will turn it into a first proposal we can explore together.",
            "options": [],
        },
        "connections": {
            "title": "Which information sources should we use?",
            "description": "Select what matters for this job, or name another service. I will check the available connections.",
            "options": [
                ("files", "Local text files", "Read reference text placed in the system's workspace folder."),
                ("github", "GitHub", "Use your repositories, issues, and project information."),
                ("slack", "Slack", "Find relevant information in team conversations."),
                ("google_drive", "Google Drive", "Use the documents your team works with."),
                ("none", "No connected sources yet", "Begin with information you provide in conversation."),
            ],
        },
        "knowledge": {
            "title": "What should your team remember together?",
            "description": "Conversation memory, working notes, and a knowledge service serve different purposes.",
            "options": [
                ("simple", "Memory and shared notes", "Keep useful context and local work without another service."),
                ("existing", "An existing knowledge service", "Connect a knowledge service you already run."),
                ("new", "A new shared knowledge base", "Prepare a knowledge service for lasting, connected facts with their sources."),
            ],
        },
        "autonomy": {
            "title": "Which outside actions may agents take?",
            "description": "Your agents can keep internal notes and task status with any choice. This decision concerns connected services.",
            "options": [
                ("drafts", "Read information and prepare drafts", "I carry out changes in outside services myself."),
                ("review", "Prepare changes for review", "Use a tool's real approval step where one exists; otherwise leave outside changes to me."),
                ("routine", "Perform specific routine actions", "Choose the exact tasks and tool permissions together."),
            ],
        },
        "destination": {
            "title": "Where should this system run?",
            "description": "I will prepare the package and installation steps for the computer or cloud account you choose.",
            "options": [
                ("mac", "My Mac", "Run locally and manage it on your computer."),
                ("linux", "A Linux computer", "Use your own Linux machine or server."),
                ("gcp", "Google Cloud", "Use your cloud project; project ID and location are needed."),
            ],
        },
    }
    if stage not in questions:
        raise ValueError("Unknown guided setup step.")
    item = questions[stage]
    return GuideQuestion(stage=stage, id=stage, title=item["title"], description=item["description"],
                         multiple=stage == "connections", allow_text=True,
                         options=[{"id": identity, "label": label, "description": detail}
                                  for identity, label, detail in item["options"]]).model_dump(mode="json")


def starter_state(capabilities: dict):
    question = default_question("purpose")
    return {
        "id": None, "version": 0, "stage": "purpose", "question": question,
        "answers": [], "messages": [conversation_message(
            "assistant", "Let's start with something useful you want to get done.",
            stage="purpose", action="start", question=question,
        )], "blueprint": None, "proposal": None, "manual_choices": None, "manual_edits": [],
        "brief": DesignBrief().model_dump(mode="json"), "reopened_topics": [],
        "source": "starter", "status": "question", "completed_stages": [],
        "capabilities": dict(capabilities), "summary": "", "warnings": [],
        "changes": [], "console_url": None, "readiness": review_readiness({}),
    }
