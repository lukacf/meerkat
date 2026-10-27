"""Versioned deployment blueprints: desired product policy, not runtime state."""
from __future__ import annotations

import re
from typing import Literal
from urllib.parse import parse_qsl, urlsplit

from pydantic import Field, field_validator, model_serializer, model_validator

from .schema import Advanced, Cloud, DEFAULT_MODELS, DeploymentSpec, Project, Provider, StrictModel, check_model_provider, default_model_values
from .planning_limits import MAX_CLASSES, MAX_FLOW_STEPS, MAX_IDENTITIES, MAX_INSTANCES_PER_CLASS, MAX_PEER_EDGES

ID = r"^[a-z][a-z0-9_-]{0,39}$"
ENV = r"^[A-Z][A-Z0-9_]{0,79}$"
MODEL = r"^[a-zA-Z0-9][a-zA-Z0-9._:/-]*$"
PLACEHOLDER = re.compile(r"\$\{([A-Z][A-Z0-9_]{0,79})\}")
CREDENTIAL_NAME = re.compile(r"(?:^|[-_])(?:token|password|passwd|secret|api[-_]?key|authorization|credential)$", re.I)
HEADER_REFERENCE = re.compile(r"(?:(?i:Bearer|Basic|Token|ApiKey) )?\$\{[A-Z][A-Z0-9_]{0,79}\}")


def validate_credential_value(value):
    if not HEADER_REFERENCE.fullmatch(value):
        raise ValueError("Keep connection credentials in local ${ENV_NAME} references.")


def validate_argument_credentials(args):
    """Reject recognizable credential literals at every blueprint ingress."""
    for index, value in enumerate(args):
        flag, separator, assigned = value.partition("=")
        if value.startswith("-") and CREDENTIAL_NAME.search(flag):
            credential = assigned if separator else args[index + 1] if index + 1 < len(args) else None
            if credential is not None:
                validate_credential_value(credential)
        # Docker -e NAME=value, HTTP header arguments and bare NAME=value
        # forms carry the same credentials even without a leading flag.
        if separator and CREDENTIAL_NAME.search(flag) and re.fullmatch(r"[-A-Za-z0-9_]+", flag):
            validate_credential_value(assigned)
        header = re.fullmatch(r"(?:Authorization|Proxy-Authorization|X-Api-Key)\s*:\s*(.*)", value, re.I)
        if header:
            validate_credential_value(header.group(1))
        elif re.match(r"(?:Bearer|Basic)\s", value, re.I):
            validate_credential_value(value)
        url = assigned if separator and value.startswith("-") else value
        if re.match(r"[A-Za-z][A-Za-z0-9+.-]*://", url):
            parsed = urlsplit(url)
            if parsed.username or parsed.password:
                raise ValueError("Use a local environment reference for credential-bearing connection URLs.")
            validate_query_credentials(parsed.query)


def validate_query_credentials(query):
    for key, value in parse_qsl(query, keep_blank_values=True):
        if CREDENTIAL_NAME.search(key) or key.lower() == "key":
            validate_credential_value(value)


class MCPGrant(StrictModel):
    connection_id: str = Field(pattern=ID)
    tool_names: list[str] = Field(default_factory=list, max_length=200)

    @field_validator("tool_names")
    @classmethod
    def tool_names_valid(cls, value):
        if len(set(value)) != len(value) or any(not re.fullmatch(r"[a-zA-Z0-9_.:/-]{1,128}", item) for item in value):
            raise ValueError("Tool names must be unique MCP tool names.")
        return value


class ToolGrants(StrictModel):
    read_only: bool = Field(default=False, strict=True,
                            description="Native execution policy: only tools declared read-only may execute. Category grants remain explicit; this is not an all-egress privacy guarantee.")
    workspace: bool = False
    notes: Literal["none", "read", "write"] = "none"
    builtins: bool = False
    shell: bool = False
    memory: bool = True
    workgraph: bool = False
    schedule: bool = False
    knowledge: bool = False
    mcp: list[MCPGrant] = Field(default_factory=list, max_length=32)

    @model_serializer(mode="wrap")
    def serialize_native_policy(self, handler):
        data = handler(self)
        # Preserve historical blueprint and acceptance bindings for the
        # unchanged native default; only the restricted policy adds a key.
        if self.read_only is False:
            data.pop("read_only", None)
        return data


class AgentMember(StrictModel):
    key: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,19}$")
    name: str = Field(min_length=1, max_length=80)
    instructions: str = Field(min_length=10, max_length=4000)


class AgentClass(StrictModel):
    id: str = Field(pattern=ID)
    name: str = Field(min_length=1, max_length=80)
    description: str = Field(min_length=1, max_length=1000)
    instructions: str = Field(min_length=10, max_length=12000)
    provider: Provider = "openai"
    model: str = Field(default_factory=lambda values: DEFAULT_MODELS[values.get("provider", "openai")],
                       max_length=100, pattern=MODEL, json_schema_extra={"default": DEFAULT_MODELS["openai"]})
    instances: int = Field(default=1, ge=0, le=MAX_INSTANCES_PER_CLASS, strict=True,
                           description="Initial durable identity count. Zero retains a reusable class profile with no initial identities; it is not a worker or placeholder agent.")
    members: list[AgentMember] = Field(default_factory=list, max_length=MAX_INSTANCES_PER_CLASS)
    tools: ToolGrants = Field(default_factory=ToolGrants)

    @model_validator(mode="after")
    def check_provider(self):
        check_model_provider(self.provider, self.model)
        if self.members and (len(self.members) != self.instances or len({member.key for member in self.members}) != len(self.members)):
            raise ValueError("Named members must have unique stable keys and match the instance count.")
        grants = [grant.connection_id for grant in self.tools.mcp]
        if len(set(grants)) != len(grants):
            raise ValueError("Assign each MCP connection only once per class.")
        return self

    def identities(self):
        if self.members:
            return [f"{self.id}:{member.key}" for member in self.members]
        return [f"{self.id}:main"] if self.instances == 1 else [f"{self.id}:{i}" for i in range(1, self.instances + 1)]


class ConnectionSource(StrictModel):
    registry_id: str = Field(default="", max_length=300)
    repository: str = Field(default="", max_length=1000)
    version: str = Field(default="", max_length=100)
    url: str = Field(default="", max_length=2000)


class SetupInput(StrictModel):
    name: str = Field(pattern=ENV)
    description: str = Field(default="", max_length=1000)
    secret: bool = True
    required: bool = True
    default: str = Field(default="", max_length=2000)

    @model_validator(mode="after")
    def secret_defaults(self):
        if self.secret and self.default:
            raise ValueError("Secret configuration must not contain a default value.")
        return self


class ToolEvidence(StrictModel):
    source_url: str = Field(max_length=2000)
    sha256: str = Field(pattern=r"^[a-f0-9]{64}$")
    retrieved_at: str = Field(max_length=100)
    tool_names: list[str] = Field(max_length=500)


class Connection(StrictModel):
    id: str = Field(pattern=ID)
    name: str = Field(min_length=1, max_length=100)
    transport: Literal["stdio", "http"]
    command: str = Field(default="", max_length=500)
    args: list[str] = Field(default_factory=list, max_length=80)
    url: str = Field(default="", max_length=2000)
    env: dict[str, str] = Field(default_factory=dict, max_length=50)
    headers: dict[str, str] = Field(default_factory=dict, max_length=30)
    source: ConnectionSource = Field(default_factory=ConnectionSource)
    description: str = Field(default="", max_length=2000)
    required_environment: list[str] = Field(default_factory=list, max_length=50)
    setup_inputs: list[SetupInput] = Field(default_factory=list, max_length=50)
    optional_args: list[int] = Field(default_factory=list, max_length=80)
    tool_evidence: ToolEvidence | None = None

    @model_validator(mode="after")
    def connection_valid(self):
        if self.id == "elephant":
            raise ValueError("The elephant connection ID is reserved for the knowledge configuration.")
        if self.transport == "stdio":
            if not self.command or "\x00" in self.command or self.url or self.headers:
                raise ValueError("Stdio connections need a command, arguments and optional environment only.")
        elif not self.url or self.command or self.args or self.env:
            raise ValueError("HTTP connections need a URL and optional headers only.")
        if any("\x00" in arg or len(arg) > 4000 for arg in self.args):
            raise ValueError("An MCP argument is invalid or too long.")
        validate_argument_credentials(self.args)
        if self.url:
            validate_url(self.url)
        for key, value in self.env.items():
            if not re.fullmatch(ENV, key) or not PLACEHOLDER.fullmatch(value):
                raise ValueError("Connection environment values must reference local ${ENV_NAME} values.")
        for key, value in self.headers.items():
            if not re.fullmatch(r"[A-Za-z0-9-]{1,80}", key) or "\r" in value or "\n" in value:
                raise ValueError("Invalid MCP HTTP header.")
            if not HEADER_REFERENCE.fullmatch(value):
                raise ValueError("HTTP header values must be a local ${ENV_NAME} reference, optionally prefixed with Bearer, Basic, Token or ApiKey.")
        if any(not re.fullmatch(ENV, key) for key in self.required_environment):
            raise ValueError("Invalid required environment name.")
        if len({item.name for item in self.setup_inputs}) != len(self.setup_inputs):
            raise ValueError("Connection setup input names must be unique.")
        if len(set(self.optional_args)) != len(self.optional_args) or any(type(index) is not int or index < 0 or index >= len(self.args) for index in self.optional_args):
            raise ValueError("Optional argument indexes must refer to unique existing argument positions.")
        return self


def validate_url(value):
    parsed = urlsplit(value)
    if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError("Use an HTTP(S) URL without embedded credentials.")
    if parsed.scheme == "http" and parsed.hostname not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("Remote connections require HTTPS; HTTP is only allowed on loopback.")
    if parsed.fragment or "\r" in value or "\n" in value:
        raise ValueError("Invalid connection URL.")
    validate_query_credentials(parsed.query)


class Edge(StrictModel):
    a: str = Field(max_length=60)
    b: str = Field(max_length=60)


class Topology(StrictModel):
    mode: Literal["hub", "mesh", "pipeline", "manual"] = "hub"
    entrypoint: str = Field(max_length=60)
    edges: list[Edge] = Field(default_factory=list, max_length=MAX_PEER_EDGES)


class FlowStep(StrictModel):
    id: str = Field(pattern=ID)
    agent: str = Field(max_length=60)
    prompt: str = Field(min_length=1, max_length=12000)
    depends_on: list[str] = Field(default_factory=list, max_length=MAX_FLOW_STEPS)
    timeout_ms: int = Field(default=120000, ge=1000, le=1800000, strict=True)


class Flow(StrictModel):
    id: str = Field(pattern=ID)
    name: str = Field(min_length=1, max_length=100)
    description: str = Field(default="", max_length=2000)
    steps: list[FlowStep] = Field(min_length=1, max_length=MAX_FLOW_STEPS)


class Knowledge(StrictModel):
    mode: Literal["none", "remote", "local"] = "none"
    url: str = Field(default="http://127.0.0.1:8091/mcp/agent", max_length=2000)
    token_env: str = Field(default="ELEPHANT_TOKEN", pattern=ENV)
    space_env: str = Field(default="ELEPHANT_SPACE_ID", pattern=ENV)
    source_repository: str = Field(default="", max_length=2000)
    source_revision: str = Field(default="", max_length=100)

    @model_validator(mode="after")
    def valid_endpoint(self):
        if self.mode != "none":
            from .elephant_setup import validate_environment_names
            validate_environment_names(self.token_env, self.space_env)
            validate_url(self.url)
            parsed = urlsplit(self.url)
            if parsed.path.rstrip("/") != "/mcp/agent" or parsed.query:
                raise ValueError("Knowledge grants use Elephant read-only /mcp/agent with no query parameters.")
            if self.mode == "local" and (parsed.hostname not in ("localhost", "127.0.0.1") or parsed.scheme != "http" or not parsed.port or not 1024 <= parsed.port <= 65535):
                raise ValueError("Local Elephant needs a loopback HTTP endpoint with a port from 1024 to 65535.")
        return self


class CloudOptions(Cloud):
    machine_type: Literal["e2-small", "e2-medium", "e2-standard-2", "e2-standard-4", "e2-standard-8", "e2-standard-16"] = "e2-medium"
    disk_gb: int = Field(default=30, ge=30, le=2000, strict=True)


class Blueprint(StrictModel):
    schema_version: Literal[2] = 2
    project: Project
    provider: Provider = "openai"
    model: str = Field(default=DEFAULT_MODELS["openai"], max_length=100, pattern=MODEL)
    target: Literal["mac", "linux", "gcp"] = "mac"
    storage: Literal["sqlite"] = "sqlite"
    gcp: CloudOptions = Field(default_factory=CloudOptions)
    advanced: Advanced = Field(default_factory=Advanced)
    agent_classes: list[AgentClass] = Field(min_length=1, max_length=MAX_CLASSES)
    connections: list[Connection] = Field(default_factory=list, max_length=32)
    topology: Topology
    flows: list[Flow] = Field(default_factory=list, max_length=32)
    knowledge: Knowledge = Field(default_factory=Knowledge)

    @model_validator(mode="before")
    @classmethod
    def inherit_class_models(cls, value):
        value = default_model_values(value)
        if not isinstance(value, dict) or not isinstance(value.get("agent_classes"), (list, tuple)):
            return value
        provider = value.get("provider", "openai")
        classes = []
        for item in value["agent_classes"]:
            raw = item
            if isinstance(item, AgentClass):
                raw = item.model_dump()
                if "model" not in item.model_fields_set:
                    raw.pop("model")
                    if "provider" not in item.model_fields_set:
                        raw.pop("provider")
            if not isinstance(raw, dict):
                classes.append(raw)
                continue
            raw = {"provider": provider, **raw}
            if "model" not in raw and raw["provider"] == provider and "model" in value:
                raw = {**raw, "model": value["model"]}
            classes.append(default_model_values(raw))
        return {**value, "agent_classes": classes}

    def identities(self):
        return [identity for item in self.agent_classes for identity in item.identities()]

    def resolved_edges(self):
        identities = self.identities()
        if self.topology.mode == "manual":
            return self.topology.edges
        if self.topology.mode == "hub":
            return [Edge(a=self.topology.entrypoint, b=identity) for identity in identities if identity != self.topology.entrypoint]
        if self.topology.mode == "pipeline":
            return [Edge(a=a, b=b) for a, b in zip(identities, identities[1:])]
        return [Edge(a=a, b=b) for index, a in enumerate(identities) for b in identities[index + 1:]]

    @model_validator(mode="after")
    def coherent(self):
        check_model_provider(self.provider, self.model)
        ids = self.identities()
        if len(ids) > MAX_IDENTITIES:
            raise ValueError(f"A blueprint can describe up to {MAX_IDENTITIES} durable identities.")
        if self.topology.mode == "mesh" and len(ids) * (len(ids) - 1) // 2 > MAX_PEER_EDGES:
            raise ValueError(f"This mesh exceeds the {MAX_PEER_EDGES} peer-edge planning limit. Use a hub, pipeline or explicit relationships for this population.")
        identity_set = set(ids)
        class_ids = [item.id for item in self.agent_classes]
        connection_ids = [item.id for item in self.connections]
        native_profiles = [f"{item.id}__{item.members[index].key}" if item.members else item.id if item.instances == 1 else f"{item.id}__{index + 1}" for item in self.agent_classes for index in range(item.instances)]
        if len(set(native_profiles)) != len(native_profiles):
            raise ValueError("Class IDs collide with derived instance profiles. Rename the conflicting class.")
        if len(set(class_ids)) != len(class_ids) or len(set(connection_ids)) != len(connection_ids):
            raise ValueError("Class IDs and connection IDs must be unique in their respective lists.")
        if self.topology.entrypoint not in identity_set:
            raise ValueError("Choose an existing identity as the entrypoint.")
        seen = set()
        for edge in self.topology.edges:
            if edge.a not in identity_set or edge.b not in identity_set or edge.a == edge.b:
                raise ValueError("Topology edges must connect two different existing identities.")
            key = tuple(sorted((edge.a, edge.b)))
            if key in seen:
                raise ValueError("Topology edges must be unique.")
            seen.add(key)
        for item in self.agent_classes:
            if any(grant.connection_id not in connection_ids for grant in item.tools.mcp):
                raise ValueError(f"Class {item.id} grants an unknown MCP connection.")
            if item.tools.knowledge and self.knowledge.mode == "none":
                raise ValueError("Configure Elephant before granting knowledge access.")
        if len({flow.id for flow in self.flows}) != len(self.flows):
            raise ValueError("Flow IDs must be unique.")
        for flow in self.flows:
            steps = {step.id: step for step in flow.steps}
            if len(steps) != len(flow.steps):
                raise ValueError(f"Flow {flow.id} has duplicate step IDs.")
            for step in flow.steps:
                if step.agent not in identity_set:
                    raise ValueError(f"Flow {flow.id} references an unknown identity.")
                if len(set(step.depends_on)) != len(step.depends_on) or any(dep not in steps for dep in step.depends_on):
                    raise ValueError(f"Flow {flow.id} has invalid dependencies.")
            pending = set(steps)
            complete = set()
            while pending:
                ready = {key for key in pending if set(steps[key].depends_on) <= complete}
                if not ready:
                    raise ValueError(f"Flow {flow.id} contains a dependency cycle.")
                pending -= ready
                complete |= ready
        if self.target == "gcp":
            if self.knowledge.mode == "local" and (self.gcp.machine_type not in ("e2-standard-8", "e2-standard-16") or self.gcp.disk_gb < 100):
                raise ValueError("Building local Elephant on GCP requires an e2-standard-8 or larger VM and at least 100 GB disk. Use remote knowledge for a smaller VM.")
            if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", self.gcp.project):
                raise ValueError("Enter a valid Google Cloud project ID.")
            if not self.gcp.zone.startswith(self.gcp.region + "-"):
                raise ValueError("Choose a zone inside your selected region.")
        return self


def from_v1(value: DeploymentSpec | dict) -> Blueprint:
    old = value if isinstance(value, DeploymentSpec) else DeploymentSpec.model_validate(value)
    # V1-only Telegram/GitHub integrations remain available through their v1
    # package endpoint. Import maps structural intent without inventing MCPs.
    return Blueprint(
        project=old.project, provider=old.provider, model=old.model, target=old.target,
        gcp=CloudOptions(**old.gcp.model_dump()), advanced=old.advanced,
        agent_classes=[AgentClass(id=a.id, name=a.name, description=a.role,
                                 instructions=a.instructions, provider=old.provider, model=old.model,
                                 tools=ToolGrants(workspace="workspace" in old.connections, notes="write"))
                       for a in old.agents],
        topology=Topology(entrypoint=f"{old.agents[0].id}:main"),
    )


def starter_blueprint() -> Blueprint:
    return Blueprint(
        project=Project(name="Atlas Operations", slug="atlas-operations", description="A capable operations team that researches, coordinates work, and keeps useful shared knowledge."),
        agent_classes=[
            AgentClass(id="lead", name="Operations lead", description="Coordinates the team and makes decisions", instructions="Understand the user's goal, coordinate specialists, and provide a clear decision with supporting evidence.", tools=ToolGrants(notes="write", workgraph=True)),
            AgentClass(id="research", name="Research specialist", description="Investigates sources and produces cited findings", instructions="Investigate the assigned question with available sources. Distinguish evidence, uncertainty, and recommendations.", instances=2, tools=ToolGrants(workspace=True, notes="read")),
            AgentClass(id="review", name="Quality reviewer", description="Checks conclusions and challenges weak assumptions", instructions="Review the team's work independently. Identify factual gaps, weak assumptions, and actionable improvements.", tools=ToolGrants(notes="read")),
        ],
        topology=Topology(entrypoint="lead:main"),
        flows=[Flow(id="briefing", name="Research briefing", description="Two parallel investigations followed by independent review and synthesis.", steps=[
            FlowStep(id="investigate", agent="research:1", prompt="Investigate the topic in the activation parameters. Provide findings and cite available sources."),
            FlowStep(id="challenge", agent="research:2", prompt="Independently investigate risks and counterarguments for the topic in the activation parameters."),
            FlowStep(id="review", agent="review:main", prompt="Assess the prior findings for accuracy and missing evidence.", depends_on=["investigate", "challenge"]),
            FlowStep(id="synthesize", agent="lead:main", prompt="Synthesize the original investigations and their independent review into a concise, actionable briefing with source evidence and uncertainty.", depends_on=["investigate", "challenge", "review"]),
        ])],
    )
