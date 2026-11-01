"""Project desired service operations without inventing access or check results."""
from copy import deepcopy


def required_service_operations(state):
    brief = state.get("brief") or {}
    features = {item["id"]: item for item in brief.get("requirements", [])}

    def required(item):
        return features.get(item.get("requirement_id"), {}).get("disposition") not in {
            "deferred", "cancelled", "handoff"}

    declared = brief.get("service_requirements") or []
    operations = [deepcopy(item) for item in declared if required(item)]
    by_id = {item["id"]: item for item in operations}
    for item in brief.get("incoming_data", []):
        # A feature may need multiple independent sources. A declared operation
        # sharing that feature does not establish this source's native binding.
        if required(item):
            selected = item.get("service_requirement_ids") or []
            if not isinstance(selected, list) or not all(isinstance(identity, str) for identity in selected):
                selected = []
            planned = [by_id[identity] for identity in selected if identity in by_id
                       and by_id[identity].get("requirement_id") == item["requirement_id"]]
            associated = bool(selected) and len(planned) == len(selected) == len(set(selected))
            operations.append({"id": "incoming:" + item["id"],
                "requirement_id": item["requirement_id"], "service": item["name"],
                # IncomingDataIntent also represents polled collections. Its
                # presence alone cannot select a conversation-delivery operation.
                "capability": "unresolved", "operation": item["purpose"],
                "resource": item["scope"], "source_message_ids": item["source_message_ids"],
                "reason_code": "source_binding_not_verified" if associated else "source_operation_not_selected",
                "planned_operations": planned if associated else []})

    # Selected connections without operation rows still need refinement, even
    # when an empty inventory was proposed. A missing inventory is gated by the
    # caller separately; zero selected connections does not establish no services.
    # Names/tool catalogs are desired configuration, never successful checks.
    if not declared:
        plan = state.get("proposal") or state.get("blueprint") or {}
        for connection in plan.get("connections", []):
            if isinstance(connection, str):
                if connection == "workspace":
                    continue
                identity, name = connection, connection
            else:
                identity, name = connection["id"], connection["name"]
            operations.append({"id": "connection:" + identity, "service": name,
                "capability": "unresolved", "operation": "Identify and check the operations this application needs",
                "resource": "", "reason_code": "required_operations_not_selected"})
        knowledge = plan.get("knowledge") or {}
        if knowledge.get("mode") == "remote":
            operations.append({"id": "knowledge:remote", "service": "Knowledge service",
                "capability": "read", "operation": "Read the selected knowledge space",
                "resource": "", "reason_code": "required_operations_not_selected"})
    return deepcopy(operations)
