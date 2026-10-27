"""Inspection-only activation summaries, using trusted source data offline."""
from copy import deepcopy
import json

import pytest

from launchkit import visualizer as module
from launchkit.projects import fingerprint
from test_visualizer import inputs, inspected_job, large_inspection_inputs, service


def mixed_inputs():
    blueprint, model = large_inspection_inputs()
    model["edges"].extend({"id": f"other:{index}", "source": "human", "target": "lead:main", "kind": kind}
                          for index, kind in enumerate(("custom-handoff", None, 7, ["peer"])))
    model["phases"] = [
        {"id": "mixed", "name": "Keep the actual handoffs", "description": "Distinct context survives inspection.",
         "activeNodeIds": ["human", "research:member-000", "review:main"],
         "activeEdgeIds": ["human-contact", "peer:0", "native-dispatch", "tool:0:notes",
                           "native-dependency", "other:0", "returned-result", "peer:79"]},
        {"id": "permissions", "activeNodeIds": ["research:member-079"],
         "activeEdgeIds": ["peer:79", "tool:79:archive", "tool:79:notes"]},
        {"id": "boundary", "activeNodeIds": ["human", "lead:main"],
         "activeEdgeIds": ["human-contact", "native-dispatch", "native-dependency", "returned-result",
                           "other:0", "other:1", "other:2", "other:3"]},
        {"id": "empty", "activeNodeIds": [], "activeEdgeIds": []},
        {"id": "repeated-references", "activeNodeIds": ["lead:main"],
         "activeEdgeIds": ["peer:0", "human-contact", "peer:0", "human-contact"]},
    ]
    return module.checked_inputs(blueprint, model)


def test_phase_summary_removes_only_omitted_real_relationships_and_keeps_every_other_context():
    blueprint, model = mixed_inputs()
    before = deepcopy((blueprint, model))
    source_hash = fingerprint(model)
    projected = module.inspection_graph(model)
    originals = {phase["id"]: phase for phase in model["phases"]}
    assert projected["phases"][0] == {**originals["mixed"],
        "activeEdgeIds": ["human-contact", "native-dispatch", "native-dependency", "other:0", "returned-result"],
        "active_edges_complete": False, "active_edge_count": 8,
        "omitted_active_edge_counts": {"peer": 2, "tool": 1}}
    assert projected["phases"][1] == {**originals["permissions"],
        "activeEdgeIds": [], "active_edges_complete": False, "active_edge_count": 3,
        "omitted_active_edge_counts": {"peer": 1, "tool": 2}}
    assert projected["phases"][2:4] == model["phases"][2:4]
    assert projected["phases"][4] == {**originals["repeated-references"],
        "activeEdgeIds": ["human-contact", "human-contact"], "active_edges_complete": False,
        "active_edge_count": 2, "omitted_active_edge_counts": {"peer": 1}}
    inline = {edge["id"] for edge in projected["edges"]}
    omitted = {edge["id"]: edge for edge in model["edges"] if module.bulk_relationship(edge)}
    assert {edge["id"] for edge in model["edges"]} == inline | omitted.keys()
    for phase in projected["phases"]:
        original = originals[phase["id"]]
        assert set(phase["activeEdgeIds"]) <= inline
        if phase.get("active_edges_complete") is False:
            active = set(original["activeEdgeIds"])
            assert phase["active_edge_count"] == len(active)
            assert len(set(phase["activeEdgeIds"])) + sum(phase["omitted_active_edge_counts"].values()) == len(active)
            for kind, count in phase["omitted_active_edge_counts"].items():
                assert count == sum(omitted[identity]["kind"] == kind for identity in active & omitted.keys())
    for name in set(model) - {"edges", "phases"}:
        assert projected[name] == model[name]
    assert projected["bulk_relationship_counts"] == {"peer": 80, "tool": 240}
    assert sum(item["edge_count"] for item in projected["relationship_groups"]) == len(omitted)
    assert (blueprint, model) == before and fingerprint(model) == source_hash


@pytest.mark.parametrize("count", [0, 1, module.MAX_INLINE_RELATIONSHIPS])
def test_unsummarized_graphs_and_phases_remain_byte_compatible(count):
    _, model = inputs()
    original_edge = model["edges"][0]
    model["edges"] = [{**original_edge, "id": f"peer:{index}"} for index in range(count)]
    model["phases"][0]["activeEdgeIds"] = [edge["id"] for edge in model["edges"]]
    before = json.dumps(model).encode()
    result = module.inspection_graph(model)
    assert result is model and json.dumps(result).encode() == before
    assert "active_edges_complete" not in result["phases"][0]


@pytest.mark.asyncio
async def test_current_key_exact_phase_edges_and_full_queries_keep_all_original_relationships(service):
    job = inspected_job(service, mixed_inputs(), key="owned-current-source")
    before = deepcopy(job)
    overview = await service.inspect({"request_key": job["key"]})
    assert overview["model"]["phases"][0]["active_edges_complete"] is False
    assert "active_edge_count counts distinct original active edges" in overview["projection"]
    for phase in job["model"]["phases"]:
        detail = await service.inspect({"view": "phase", "id": phase["id"], "request_key": job["key"]})
        assert detail == {"available": True, "request_key": job["key"], "view": "phase", "phase": phase}
        rows, offset = [], 0
        while True:
            page = await service.inspect({"view": "edges", "phase_id": phase["id"],
                "request_key": job["key"], "offset": offset, "limit": 2})
            assert page["available"] is True
            rows.extend(page["edges"])
            if page["next_offset"] is None:
                break
            offset = page["next_offset"]
        assert rows == [edge for edge in job["model"]["edges"] if edge["id"] in phase["activeEdgeIds"]]
    full = await service.inspect({"view": "full", "request_key": job["key"]})
    assert full == {"available": True, "request_key": job["key"], "view": "full",
                    "blueprint": job["blueprint"], "model": job["model"]}
    for args in ({"view": "full"}, {"view": "phase", "id": "mixed"}, {"view": "edges", "phase_id": "mixed"}):
        rejected = await service.inspect({**args, "request_key": "obsolete-source"})
        assert rejected["available"] is False and "model" not in rejected and "phase" not in rejected and "edges" not in rejected
    service.pending = {**deepcopy(job), "key": "replacement-source"}
    assert (await service.inspect({"view": "phase", "id": "mixed", "request_key": job["key"]}))["available"] is False
    assert job == before and service.architect.starts == 0
