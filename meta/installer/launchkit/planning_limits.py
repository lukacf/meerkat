"""Shared blueprint editing bounds, not measured runtime capacity guarantees."""
import json
from pathlib import Path

PLANNING_LIMITS = json.loads(Path(__file__).with_name("blueprint-limits.json").read_text())
MAX_IDENTITIES = PLANNING_LIMITS["maxIdentities"]
MAX_INSTANCES_PER_CLASS = PLANNING_LIMITS["maxInstancesPerClass"]
MAX_CLASSES = PLANNING_LIMITS["maxClasses"]
MAX_FLOW_STEPS = PLANNING_LIMITS["maxFlowSteps"]
MAX_PEER_EDGES = PLANNING_LIMITS["maxPeerEdges"]
