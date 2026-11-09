"""Stateless authoring shorthand that expands into the ordinary Blueprint.

Sequences and selectors are tool inputs only. They grant no authority, infer no
relationships and are never stored as an alternate roster or graph.
"""
from __future__ import annotations

import copy
import re
from string import Formatter
from typing import Annotated

from pydantic import BaseModel, ConfigDict, Field, model_validator

from .blueprint import AgentMember, Edge, ID, MAX_IDENTITY_LENGTH
from .planning_limits import MAX_CLASSES, MAX_IDENTITIES, MAX_INSTANCES_PER_CLASS, MAX_PEER_EDGES


class _Input(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class MemberSequence(_Input):
    id: str = Field(pattern=ID)
    class_id: str = Field(pattern=ID)
    start: int = Field(ge=0, le=10**19 - 1)
    count: int = Field(ge=1, le=MAX_INSTANCES_PER_CLASS)
    key_template: str = Field(min_length=1, max_length=120)
    name_template: str = Field(min_length=1, max_length=240)
    instructions_template: str = Field(min_length=1, max_length=4000)


class IdentitySelector(_Input):
    identities: list[Annotated[str, Field(min_length=1, max_length=MAX_IDENTITY_LENGTH)]] | None = Field(
        default=None, min_length=1, max_length=MAX_IDENTITIES)
    sequence: str | None = Field(default=None, pattern=ID)
    start: int | None = Field(default=None, ge=0, le=10**19 - 1)
    stop: int | None = Field(default=None, ge=0, le=10**19 + MAX_INSTANCES_PER_CLASS)

    @model_validator(mode="after")
    def one_source(self):
        if (self.identities is None) == (self.sequence is None):
            raise ValueError("Choose exactly one explicit identity list or member sequence.")
        if self.identities is not None:
            if self.start is not None or self.stop is not None:
                raise ValueError("Number bounds apply only to a member sequence.")
            if len(set(self.identities)) != len(self.identities):
                raise ValueError("A selector cannot repeat an identity.")
        return self


class PeerEdgeGroup(_Input):
    a: IdentitySelector
    b: IdentitySelector


class BlueprintAuthoring(_Input):
    member_sequences: list[MemberSequence] = Field(default_factory=list, max_length=MAX_IDENTITIES)
    peer_edge_groups: list[PeerEdgeGroup] = Field(default_factory=list, max_length=MAX_PEER_EDGES)


def authoring_schema():
    return BlueprintAuthoring.model_json_schema()


def blueprint_tool_schema(*, summary=False):
    """Keep nested schema references rooted at the actual registered tool."""
    authoring = authoring_schema()
    definitions = authoring.pop("$defs", {})
    properties = {"blueprint": {"type": "object"}, "authoring": authoring}
    required = ["blueprint"]
    if summary:
        properties["summary"] = {"type": "string"}
        required.append("summary")
    return {"type": "object", "properties": properties, "required": required,
            "additionalProperties": False, "$defs": definitions}


def _template(value, *, names=False):
    """Accept literal text and bounded placeholders, never attribute/expression access."""
    try:
        value.encode("utf-8")
        parts = list(Formatter().parse(value))
    except (ValueError, UnicodeError) as error:
        raise ValueError("Use a valid UTF-8 literal member template.") from error
    has_number = False
    for _, field, spec, conversion in parts:
        if field is None:
            continue
        if conversion or field not in ({"n", "name"} if names else {"n"}):
            raise ValueError("Member templates allow only {n} and {name} in instructions.")
        if field == "name" and spec:
            raise ValueError("The {name} placeholder has no format specifier.")
        if field == "n":
            has_number = True
            if spec not in ("", "d") and not re.fullmatch(r"0(?:[1-9]|1[0-2])d", spec):
                raise ValueError("Use {n} or a decimal zero-padding width from 1 to 12.")
    if not names and not has_number:
        raise ValueError("Sequence key and name templates must explicitly include {n}.")
    return parts


def _render(parts, number, name=None):
    pieces = []
    for literal, field, spec, _ in parts:
        pieces.append(literal)
        if field == "n":
            pieces.append(str(number).zfill(int(spec[1:-1])) if spec not in ("", "d") else str(number))
        elif field == "name":
            pieces.append(name)
    return "".join(pieces)


def _select(selector, sequences, identities):
    if selector.identities is not None:
        selected = selector.identities
    else:
        if selector.sequence not in sequences:
            raise ValueError("A peer selector names a missing member sequence.")
        sequence, members = sequences[selector.sequence]
        first = sequence.start if selector.start is None else selector.start
        last = sequence.start + sequence.count - 1 if selector.stop is None else selector.stop
        if first > last or first < sequence.start or last >= sequence.start + sequence.count:
            raise ValueError("A peer selector must stay inside its explicit member sequence.")
        selected = [members[number] for number in range(first, last + 1)]
    if not set(selected) <= identities:
        raise ValueError("A peer selector names an identity absent from the expanded Blueprint.")
    return selected


def materialize_blueprint(blueprint, authoring=None):
    """Expand explicit tool input without changing declared counts, grants or modes."""
    candidate = copy.deepcopy(blueprint)
    if authoring is None:
        return candidate
    directive = BlueprintAuthoring.model_validate(authoring)
    if not isinstance(candidate, dict) or not isinstance(candidate.get("agent_classes"), list):
        raise ValueError("Authoring needs an ordinary complete Blueprint with existing classes.")
    classes = candidate["agent_classes"]
    if not 1 <= len(classes) <= MAX_CLASSES or any(not isinstance(row, dict) for row in classes):
        raise ValueError("Authoring must remain within the Blueprint class limit.")
    by_class = {}
    total = 0
    for row in classes:
        identity, count = row.get("id"), row.get("instances", 1)
        if not isinstance(identity, str) or identity in by_class:
            raise ValueError("Authoring requires unique existing class IDs.")
        if type(count) is not int or not 0 <= count <= MAX_INSTANCES_PER_CLASS:
            raise ValueError("Declared instance counts must be bounded integers.")
        if not isinstance(row.get("members", []), list):
            raise ValueError("Class members must be ordinary Blueprint member lists.")
        total += count
        by_class[identity] = row
    if total > MAX_IDENTITIES:
        raise ValueError("Authoring exceeds the Blueprint total identity limit.")
    sequences = {}
    for sequence in directive.member_sequences:
        if sequence.id in sequences:
            raise ValueError("Member sequence IDs must be unique.")
        if sequence.class_id not in by_class:
            raise ValueError("A member sequence names a missing Blueprint class.")
        row = by_class[sequence.class_id]
        members = row.setdefault("members", [])
        if len(members) + sequence.count > row.get("instances", 1):
            raise ValueError("Member sequences must match the declared instance count.")
        keys, names, instructions = (_template(sequence.key_template),
                                    _template(sequence.name_template),
                                    _template(sequence.instructions_template, names=True))
        number_ids = {}
        for number in range(sequence.start, sequence.start + sequence.count):
            name = _render(names, number)
            member = AgentMember.model_validate({"key": _render(keys, number), "name": name,
                "instructions": _render(instructions, number, name)}).model_dump(mode="json")
            members.append(member)
            number_ids[number] = f"{sequence.class_id}:{member['key']}"
        sequences[sequence.id] = (sequence, number_ids)
    identities = set()
    for row in classes:
        count, members = row.get("instances", 1), row.get("members", [])
        if members:
            if len(members) != count:
                raise ValueError("Named members must match the declared instance count.")
            member_ids = [f"{row['id']}:{AgentMember.model_validate(member).key}" for member in members]
        else:
            member_ids = [f"{row['id']}:main"] if count == 1 else [f"{row['id']}:{n}" for n in range(1, count + 1)]
        if len(set(member_ids)) != len(member_ids) or identities.intersection(member_ids):
            raise ValueError("Expanded member keys must be unique.")
        identities.update(member_ids)
    topology = candidate.get("topology")
    if not isinstance(topology, dict):
        raise ValueError("Authoring needs the ordinary Blueprint topology.")
    if directive.peer_edge_groups and topology.get("mode", "hub") != "manual":
        raise ValueError("Explicit peer edge groups require an explicitly manual topology.")
    edges = topology.get("edges", [])
    if not isinstance(edges, list):
        raise ValueError("Topology edges must be an ordinary Blueprint edge list.")
    expanded_count = len(edges)
    if expanded_count > MAX_PEER_EDGES:
        raise ValueError("Expanded relationships exceed the Blueprint peer-edge limit.")
    pairs = []
    for group in directive.peer_edge_groups:
        selected_a = _select(group.a, sequences, identities)
        selected_b = _select(group.b, sequences, identities)
        expanded_count += len(selected_a) * len(selected_b)
        if expanded_count > MAX_PEER_EDGES:
            raise ValueError("Expanded relationships exceed the Blueprint peer-edge limit.")
        pairs.append((selected_a, selected_b))
    seen = set()
    for edge in edges:
        value = Edge.model_validate(edge)
        pair = tuple(sorted((value.a, value.b)))
        if value.a == value.b or pair in seen or not {value.a, value.b} <= identities:
            raise ValueError("Peer edges need distinct existing endpoints and unique unordered pairs.")
        seen.add(pair)
    for selected_a, selected_b in pairs:
        for a in selected_a:
            for b in selected_b:
                pair = tuple(sorted((a, b)))
                if a == b or pair in seen:
                    raise ValueError("Peer edges need distinct existing endpoints and unique unordered pairs.")
                seen.add(pair)
                edges.append({"a": a, "b": b})
    if directive.peer_edge_groups:
        topology["edges"] = edges
    return candidate
