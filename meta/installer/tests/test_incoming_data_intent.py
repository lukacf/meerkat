import copy

import pytest
from pydantic import ValidationError

from launchkit.blueprint import starter_blueprint
from launchkit.guide_models import DesignBrief, default_question, review_readiness, validate_guide_publication


def brief():
    return {
        "facts": {"outcome": {"summary": "Notice relevant project changes and help decide what needs attention.",
            "status": "confirmed", "source_message_ids": ["request"]}},
        "requirements": [{"id": "changes", "outcome": "Bring relevant project changes to the right agent.",
            "disposition": "open", "extension": "Resolve source capabilities and configure native delivery.",
            "source_message_ids": ["request"]}],
        "incoming_data": [{"id": "project_changes", "requirement_id": "changes", "name": "Project updates",
            "scope": "The team's selected project sources", "purpose": "Notice decisions that need follow-up.",
            "audience": "The project team", "history": "new_only", "routes": [
                {"recipient": "lead:main", "delivery": "direct"},
                {"recipient": "review:main", "delivery": "triage"}], "source_message_ids": ["request"]}],
    }


def publish(value, plan=None):
    return validate_guide_publication({"message": "Choose which changes matter most.", "brief": value,
        "question": default_question("connections"), "design_changed": True},
        {"brief": {}, "evidence_messages": [{"id": "request", "role": "user", "text": "Help with project changes"}],
         "current_user_message": {"id": "request", "role": "user"}},
        plan or starter_blueprint().model_dump(mode="json"))


def test_old_brief_serialization_and_fingerprint_inputs_stay_unchanged():
    old = {"facts": {}, "requirements": []}
    assert DesignBrief.model_validate(old).model_dump(mode="json") == old
    assert DesignBrief.model_validate({**old, "incoming_data": []}).model_dump(mode="json") == old


def test_guided_publication_retains_mixed_routes_as_intent_and_open_implementation():
    value = brief()
    before = copy.deepcopy(value)
    result = publish(value)
    assert result["brief"]["incoming_data"][0]["routes"] == value["incoming_data"][0]["routes"]
    assert result["brief"]["requirements"][0]["disposition"] == "open"
    assert any("Bring relevant" in item for item in review_readiness(result["brief"])["implementation_blockers"])
    assert value == before


@pytest.mark.parametrize("class_id", ["review", "q", "qa"])
def test_class_route_can_start_empty_and_survives_reopen_without_placeholder_members(tmp_path, class_id):
    from launchkit.guide_store import GuideStore

    plan = starter_blueprint().model_dump(mode="json")
    recipient_class = next(item for item in plan["agent_classes"] if item["id"] == "review")
    recipient_class.update(id=class_id, instances=0)
    plan["flows"] = []
    value = brief()
    source = value["incoming_data"][0]
    source["routes"] = [{"recipient": class_id, "recipient_kind": "class", "delivery": "direct"}]
    before = copy.deepcopy(plan)
    result = publish(value, plan)
    saved = GuideStore(tmp_path).create({"brief": result["brief"], "proposal": plan})
    reopened = GuideStore(tmp_path).get(saved["id"])
    assert reopened["brief"]["incoming_data"][0]["routes"] == source["routes"]
    assert next(item for item in reopened["proposal"]["agent_classes"] if item["id"] == class_id)["instances"] == 0
    assert reopened["brief"]["requirements"][0]["disposition"] == "open"
    assert plan == before


@pytest.mark.parametrize("recipient", ["Q", "qa team", "a" * 41, "lead:main"])
def test_class_recipients_follow_blueprint_id_rules(recipient):
    from launchkit.guide_models import IncomingDataRoute

    with pytest.raises(ValueError, match="class ID"):
        IncomingDataRoute(recipient=recipient, recipient_kind="class", delivery="direct")


@pytest.mark.parametrize("recipient", ["q", "qa"])
def test_short_class_ids_do_not_relax_identity_recipient_validation(recipient):
    from launchkit.guide_models import IncomingDataRoute

    with pytest.raises(ValueError):
        IncomingDataRoute(recipient=recipient, delivery="direct")


@pytest.mark.parametrize("delivery", ["triage", "conversation"])
def test_class_cannot_be_an_ambiguous_triage_or_conversation_owner(delivery):
    value = brief()
    value["incoming_data"][0]["routes"] = [{"recipient": "review", "recipient_kind": "class", "delivery": delivery}]
    with pytest.raises(ValueError, match="one identity"):
        publish(value)


@pytest.mark.parametrize("class_first", [True, False])
def test_class_and_known_member_cannot_receive_overlapping_routes(class_first):
    value = brief()
    routes = [{"recipient": "lead", "recipient_kind": "class", "delivery": "direct"},
              {"recipient": "lead:main", "delivery": "conversation"}]
    value["incoming_data"][0]["routes"] = routes if class_first else routes[::-1]
    with pytest.raises(ValueError, match="class and identity routes must not overlap"):
        publish(value)


def test_removed_class_cannot_remain_a_recipient():
    value = brief()
    value["incoming_data"][0]["routes"] = [{"recipient": "removed", "recipient_kind": "class", "delivery": "direct"}]
    with pytest.raises(ValueError, match="existing class"):
        publish(value)


@pytest.mark.parametrize("first_unassigned", [False, True])
def test_sources_may_independently_route_to_the_same_named_members_and_empty_class(first_unassigned):
    plan = starter_blueprint().model_dump(mode="json")
    lead = next(item for item in plan["agent_classes"] if item["id"] == "lead")
    lead.update(instances=2, members=[
        {"key": key, "name": key.title(), "instructions": "Review relevant project changes."}
        for key in ("primary", "backup")])
    next(item for item in plan["agent_classes"] if item["id"] == "review")["instances"] = 0
    plan["topology"] = {"mode": "hub", "entrypoint": "lead:primary", "edges": []}
    plan["flows"] = []
    value = brief()
    template = value["incoming_data"][0]
    value["incoming_data"] = []
    for index in range(32):
        source = copy.deepcopy(template)
        source["id"] = f"source_{index}"
        source["routes"] = [
            ({"recipient": "lead", "recipient_kind": "class", "delivery": "direct"}
             if index % 2 else {"recipient": "lead:primary", "delivery": "conversation"}),
            {"recipient": "review", "recipient_kind": "class", "delivery": "direct"}]
        value["incoming_data"].append(source)
    if first_unassigned:
        value["incoming_data"][0]["routes"] = []
    before = copy.deepcopy((value, plan))
    result = publish(value, plan)
    assert result["brief"] == DesignBrief.model_validate(value).model_dump(mode="json")
    assert (value, plan) == before


@pytest.mark.parametrize("class_first", [False, True])
def test_later_source_still_rejects_class_member_overlap_and_allows_correction(class_first):
    value = brief()
    template = value["incoming_data"][0]
    value["incoming_data"] = [{**copy.deepcopy(template), "id": f"source_{index}"} for index in range(32)]
    routes = [{"recipient": "lead", "recipient_kind": "class", "delivery": "direct"},
              {"recipient": "lead:main", "delivery": "conversation"}]
    value["incoming_data"][-1]["routes"] = routes if class_first else routes[::-1]
    before = copy.deepcopy(value)
    with pytest.raises(ValueError, match="class and identity routes must not overlap"):
        publish(value)
    assert value == before
    value["incoming_data"][-1]["routes"].pop()
    assert publish(value)["brief"] == DesignBrief.model_validate(value).model_dump(mode="json")


@pytest.mark.parametrize("change,error", [
    (lambda s: s["routes"].append({"recipient": "lead:main", "delivery": "conversation"}), "one incoming route"),
    (lambda s: s["routes"].append({"recipient": "lead:main", "delivery": "triage"}), "one incoming route"),
    (lambda s: s.update(history="bounded_history"), "history window"),
    (lambda s: s.update(history_window="Last month"), "bounded-history"),
    (lambda s: s.update(requirement_id="unknown"), "existing requested feature"),
    (lambda s: s.update(source_message_ids=["assistant_message"]), "non-user evidence"),
    (lambda s: s.update(routes=[{"recipient": "missing:main", "delivery": "direct"}]), "existing identity"),
])
def test_invalid_or_unsupported_desired_routes_do_not_publish(change, error):
    value = brief()
    change(value["incoming_data"][0])
    with pytest.raises((ValueError, ValidationError), match=error):
        publish(value)


def test_unknown_recipients_and_explicit_history_remain_valid_design_work():
    value = brief()
    source = value["incoming_data"][0]
    source.update(routes=[], history="bounded_history", history_window="The current project's last two weeks")
    result = publish(value)
    assert result["brief"]["incoming_data"][0]["routes"] == []
    assert result["brief"]["incoming_data"][0]["history_window"] == source["history_window"]


def test_restructured_team_cannot_leave_an_incoming_route_to_a_removed_member():
    plan = starter_blueprint().model_dump(mode="json")
    plan["agent_classes"] = plan["agent_classes"][:1]
    plan["flows"] = []
    with pytest.raises(ValueError, match="existing identity"):
        publish(brief(), plan)


def test_unchanged_team_can_continue_the_interview_with_saved_incoming_routes():
    value = brief()
    result = validate_guide_publication({"message": "Choose a history window.", "brief": value,
        "question": default_question("connections"), "design_changed": False},
        {"brief": value, "evidence_messages": [{"id": "request", "role": "user"}]},
        None, current_blueprint=starter_blueprint().model_dump(mode="json"))
    assert result["brief"]["incoming_data"][0]["routes"] == value["incoming_data"][0]["routes"]


def test_incoming_choices_survive_saved_setup_reopen_and_remain_in_agent_context(tmp_path):
    from launchkit.guide import compact_guidance_context
    from launchkit.guide_store import GuideStore
    from launchkit.guided_edits import guided_document

    value = publish(brief())["brief"]
    plan = starter_blueprint().model_dump(mode="json")
    saved = GuideStore(tmp_path).create({"brief": value, "proposal": plan})
    reopened = GuideStore(tmp_path).get(saved["id"])
    assert reopened["brief"] == value
    assert compact_guidance_context({"brief": reopened["brief"]})["brief"] == value
    assert guided_document(plan, reopened["brief"])["brief"] == value


@pytest.mark.asyncio
@pytest.mark.parametrize("invalid_change,error", [
    (lambda source: source.update(requirement_id="missing"), "existing requested feature"),
    (lambda source: source.update(source_message_ids=["missing"]), "non-user evidence"),
    (lambda source: source.update(history="bounded_history", history_window=" \t "), "history window"),
    (lambda source: source.update(history="all_history"), "history"),
    (lambda source: source.update(provider_scope={"verified": True}), "Extra inputs"),
    (lambda source: source.update(collector_binding="invented-runner"), "Extra inputs"),
    (lambda source: source.update(routes=[
        {"recipient": "lead", "recipient_kind": "class", "delivery": "triage"}]), "one identity"),
])
async def test_invalid_source_revision_preserves_saved_plan_and_allows_corrected_retry(
        tmp_path, monkeypatch, invalid_change, error):
    from launchkit.guide import GuideService
    from launchkit.guide_store import GuideStore
    from launchkit.server import GuideResponse

    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True})

    class Author:
        reject = False

        async def message(self, prompt, blueprint, *, guide_context):
            if guide_context["saved_plan_available"]:
                value = copy.deepcopy(guide_context["brief"])
                source = value["incoming_data"][0]
                source.update(history="bounded_history", history_window="The last two weeks")
                source["source_message_ids"].append(guide_context["current_user_message"]["id"])
                if self.reject:
                    invalid_change(source)
            else:
                value = brief()
                for item in [*value["facts"].values(), *value["requirements"], *value["incoming_data"]]:
                    item["source_message_ids"] = [guide_context["current_user_message"]["id"]]
            return {"guide": {"message": "Which sources should the team use?", "brief": value,
                "question": default_question("connections"), "design_changed": True},
                "proposal": starter_blueprint().model_dump(mode="json")}

    store, author = GuideStore(tmp_path), Author()
    service = GuideService(store, author)
    empty = await service.create()
    saved = await service.respond(empty["id"], GuideResponse(
        version=empty["version"], action="chat", text="Notice relevant project changes."))
    before = copy.deepcopy(store.get(saved["id"]))
    request = GuideResponse(version=saved["version"], action="chat", text="Include the last two weeks.")
    author.reject = True
    with pytest.raises(ValueError, match=error):
        await service.respond(saved["id"], request)
    # Reopen the durable store, rather than inspecting the response or an
    # in-memory candidate that could hide a partial save.
    assert GuideStore(tmp_path).get(saved["id"]) == before

    author.reject = False
    corrected = await service.respond(saved["id"], request)
    reopened = GuideStore(tmp_path).get(saved["id"])
    original_source = before["brief"]["incoming_data"][0]
    corrected_source = reopened["brief"]["incoming_data"][0]
    assert corrected["version"] == reopened["version"] == before["version"] + 1
    assert reopened["proposal"] == before["proposal"]
    assert reopened["messages"][:-2] == before["messages"]
    assert corrected_source == {**original_source,
        "history": "bounded_history", "history_window": "The last two weeks",
        "source_message_ids": [*original_source["source_message_ids"], reopened["messages"][-2]["id"]]}


@pytest.mark.parametrize("replacement", ["removed_class", "expanded_class_overlap"])
def test_source_routes_are_checked_against_replacement_team_not_saved_team(replacement):
    previous = starter_blueprint().model_dump(mode="json")
    value = brief()
    if replacement == "expanded_class_overlap":
        next(item for item in previous["agent_classes"] if item["id"] == "review")["instances"] = 0
        previous["flows"] = []
        value["incoming_data"][0]["routes"] = [
            {"recipient": "review", "recipient_kind": "class", "delivery": "direct"}]
    old_brief = publish(value, previous)["brief"]
    changed = copy.deepcopy(previous)
    if replacement == "removed_class":
        changed["agent_classes"] = changed["agent_classes"][:1]
        changed["flows"] = []
        error = "existing identity"
    else:
        next(item for item in changed["agent_classes"] if item["id"] == "review")["instances"] = 1
        value["incoming_data"][0]["routes"].append({"recipient": "review:main", "delivery": "conversation"})
        error = "class and identity routes must not overlap"
    context = {"brief": old_brief, "saved_plan_available": True,
        "evidence_messages": [{"id": "request", "role": "user"}]}
    before = copy.deepcopy((value, changed, previous, context))
    with pytest.raises(ValueError, match=error):
        validate_guide_publication({"message": "Review the revised team.", "brief": value,
            "question": default_question("connections"), "design_changed": True},
            context, changed, current_blueprint=previous)
    assert (value, changed, previous, context) == before
    value["incoming_data"][0]["routes"].pop()
    corrected = validate_guide_publication({"message": "Review the corrected routes.", "brief": value,
        "question": default_question("connections"), "design_changed": True},
        context, changed, current_blueprint=previous)
    assert corrected["brief"]["incoming_data"][0]["routes"] == value["incoming_data"][0]["routes"]


@pytest.mark.parametrize("recipient_kind,recipient", [("identity", "lead:main"), ("class", "lead")])
@pytest.mark.asyncio
async def test_starter_scaffolding_cannot_supply_recipients_at_publication_tool(tmp_path, recipient_kind, recipient):
    from launchkit.architect import ArchitectService

    author = ArchitectService(tmp_path)
    author.context = starter_blueprint().model_dump(mode="json")
    author.guide_context = {"saved_plan_available": False, "brief": {},
        "evidence_messages": [{"id": "request", "role": "user"}]}
    value = brief()
    value["facts"]["outcome"]["status"] = "proposed"
    value["incoming_data"][0]["routes"] = [
        {"recipient": recipient, "recipient_kind": recipient_kind, "delivery": "direct"}]
    result = await author.publish_guide_step({"message": "Which changes should matter?", "brief": value,
        "question": default_question("purpose"), "design_changed": False})
    assert result["valid"] is False
    assert "Propose the team" in result["issue"]
    assert author.guide_result is None and author.proposal is None


@pytest.mark.parametrize("plan_source", ["new_proposal", "saved_proposal", "manual_proposal"])
def test_real_team_may_keep_starter_identity_names(plan_source):
    plan = starter_blueprint().model_dump(mode="json")
    value = brief()
    proposed = plan_source != "saved_proposal"
    result = validate_guide_publication({"message": "Which project sources should the team use?", "brief": value,
        "question": default_question("connections"), "design_changed": proposed},
        {"brief": {}, "saved_plan_available": not proposed,
         "action": "manual" if plan_source == "manual_proposal" else "chat",
         "manual_plan_submitted": plan_source == "manual_proposal",
         "evidence_messages": [{"id": "request", "role": "user"}]},
        plan if proposed else None, current_blueprint=plan)
    assert result["brief"] == DesignBrief.model_validate(value).model_dump(mode="json")


@pytest.mark.asyncio
@pytest.mark.parametrize("routes", [[], [{"recipient": "lead:main", "delivery": "direct"}],
    [{"recipient": "lead", "recipient_kind": "class", "delivery": "direct"}]])
async def test_first_guide_turn_saves_only_unassigned_source_intent(tmp_path, monkeypatch, routes):
    from launchkit.guide import GuideService
    from launchkit.guide_store import GuideStore
    from launchkit.server import GuideResponse

    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True})

    class Author:
        async def message(self, prompt, blueprint, *, guide_context):
            assert guide_context["saved_plan_available"] is False
            assert guide_context["manual_plan_submitted"] is False
            assert blueprint["agent_classes"][0]["id"] == "lead"
            value = brief()
            value["facts"]["outcome"]["status"] = "proposed"
            value["incoming_data"][0]["routes"] = routes
            for item in [*value["facts"].values(), *value["requirements"], *value["incoming_data"]]:
                item["source_message_ids"] = [guide_context["current_user_message"]["id"]]
            return {"guide": {"message": "Which changes should matter?", "brief": value,
                "question": default_question("purpose"), "design_changed": False}, "proposal": None}

    store = GuideStore(tmp_path)
    service = GuideService(store, Author())
    original = await service.create()
    before = copy.deepcopy(store.get(original["id"]))
    request = GuideResponse(version=original["version"], action="chat", text="Help me notice project changes.")
    if routes:
        with pytest.raises(ValueError, match="Propose the team"):
            await service.respond(original["id"], request)
        assert store.get(original["id"]) == before
    else:
        result = await service.respond(original["id"], request)
        reopened = GuideStore(tmp_path).get(original["id"])
        assert result["version"] == original["version"] + 1
        assert reopened["brief"]["incoming_data"][0]["routes"] == []
        assert reopened["proposal"] is None and reopened["blueprint"] is None


@pytest.mark.asyncio
@pytest.mark.parametrize("fresh_proposal", [False, True])
@pytest.mark.parametrize("omission", ["missing_field", "empty_list", "partial_list", "replaced_id"])
async def test_source_omission_cannot_erase_saved_choices(tmp_path, monkeypatch, fresh_proposal, omission):
    from launchkit.guide import GuideService
    from launchkit.guide_store import GuideStore
    from launchkit.server import GuideResponse

    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True})

    class Author:
        reject = False

        async def message(self, prompt, blueprint, *, guide_context):
            saved = guide_context["saved_plan_available"]
            if saved:
                value = copy.deepcopy(guide_context["brief"])
                source = value["incoming_data"][0]
                source.update(history="bounded_history", history_window="The last two weeks")
                source["source_message_ids"].append(guide_context["current_user_message"]["id"])
                if self.reject:
                    if omission == "missing_field":
                        value.pop("incoming_data")
                    elif omission == "empty_list":
                        value["incoming_data"] = []
                    elif omission == "partial_list":
                        value["incoming_data"].pop()
                    else:
                        value["incoming_data"][0]["id"] = "replacement"
            else:
                value = brief()
                second = copy.deepcopy(value["incoming_data"][0])
                second.update(id="release_changes", name="Release updates", scope="Selected release notes")
                value["incoming_data"].append(second)
                for item in [*value["facts"].values(), *value["requirements"], *value["incoming_data"]]:
                    item["source_message_ids"] = [guide_context["current_user_message"]["id"]]
            result = {"guide": {"message": "Which sources should the team use?", "brief": value,
                "question": default_question("connections"), "design_changed": not saved or fresh_proposal}}
            if not saved or fresh_proposal:
                result["proposal"] = starter_blueprint().model_dump(mode="json")
            return result

    store, author = GuideStore(tmp_path), Author()
    service = GuideService(store, author)
    empty = await service.create()
    saved = await service.respond(empty["id"], GuideResponse(
        version=empty["version"], action="chat", text="Notice project changes and release updates."))
    before = copy.deepcopy(store.get(saved["id"]))
    request = GuideResponse(version=saved["version"], action="chat", text="Include two weeks of project changes.")
    author.reject = True
    with pytest.raises(ValueError, match="Do not drop an incoming source"):
        await service.respond(saved["id"], request)
    assert GuideStore(tmp_path).get(saved["id"]) == before

    # The rejected turn must not consume its version or partially replace the brief.
    author.reject = False
    corrected = await service.respond(saved["id"], request)
    reopened = GuideStore(tmp_path).get(saved["id"])
    assert corrected["version"] == reopened["version"] == before["version"] + 1
    assert reopened["proposal"] == before["proposal"]
    assert reopened["messages"][:-2] == before["messages"]
    assert reopened["brief"]["incoming_data"][1] == before["brief"]["incoming_data"][1]
    assert reopened["brief"]["incoming_data"][0] == {
        **before["brief"]["incoming_data"][0], "history": "bounded_history",
        "history_window": "The last two weeks",
        "source_message_ids": [*before["brief"]["incoming_data"][0]["source_message_ids"], reopened["messages"][-2]["id"]]}


@pytest.mark.parametrize("disposition", ["deferred", "cancelled", "handoff"])
def test_source_retention_allows_explicit_scope_decisions_and_recipient_revision(disposition):
    previous = brief()
    revised = copy.deepcopy(previous)
    revised["requirements"][0].update(disposition=disposition, decision_message_ids=["decision"])
    revised["incoming_data"][0].update(routes=[], source_message_ids=["request", "decision"])
    result = validate_guide_publication({"message": "The source remains recorded outside this version.",
        "brief": revised, "question": default_question("connections"), "design_changed": True},
        {"brief": previous, "evidence_messages": [
            {"id": "request", "role": "user"}, {"id": "decision", "role": "user"}]},
        starter_blueprint().model_dump(mode="json"))
    assert result["brief"] == DesignBrief.model_validate(revised).model_dump(mode="json")
    assert previous == brief()


def test_source_retention_allows_new_sources_and_revised_choices():
    previous = brief()
    revised = copy.deepcopy(previous)
    source = revised["incoming_data"][0]
    source.update(scope="The selected project", history="bounded_history", history_window="The last two weeks",
        routes=[{"recipient": "lead:main", "delivery": "conversation"}], source_message_ids=["request", "revision"])
    additional = copy.deepcopy(source)
    additional.update(id="release_changes", name="Release updates", scope="Selected release notes")
    revised["incoming_data"].insert(0, additional)
    result = validate_guide_publication({"message": "Updated the sources and their choices.", "brief": revised,
        "question": default_question("connections"), "design_changed": True},
        {"brief": previous, "evidence_messages": [
            {"id": "request", "role": "user"}, {"id": "revision", "role": "user"}]},
        starter_blueprint().model_dump(mode="json"))
    assert result["brief"] == DesignBrief.model_validate(revised).model_dump(mode="json")
    assert previous == brief()
