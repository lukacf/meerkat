//! The Console's reads of a held change of a mob beside the launch mob, as
//! a caller who addresses that mob by an access rule is answered: the
//! caller reads the change only while the mob it is held for stands under
//! its id. A caller who may address every mob reads it whatever stands
//! there, and the launch mob's held changes are every caller's.

use meerkat_core::approval::decision::DecisionPhase;

use super::*;

/// What the Console's three reads answer one person about one decision.
struct Reads {
    get: serde_json::Value,
    rendering: serde_json::Value,
    listed: Vec<String>,
}

/// The get, the rendering and the list the Console answers `email`, the
/// first two about the decision `decision_id`.
async fn reads_of(app: &axum::Router, email: &str, decision_id: &str) -> Reads {
    let named = || serde_json::json!({"decision_id": decision_id});
    Reads {
        get: console_rpc(app, email, "meta/decisions/get", named()).await,
        rendering: console_rpc(app, email, "meta/decisions/rendering", named()).await,
        listed: decisions_listed_to(app, email).await,
    }
}

impl Reads {
    /// Whether every read serves the decision `decision_id`: the get
    /// answers it whole, the rendering answers its text, and the list
    /// names it.
    fn serve(&self, decision_id: &str) -> bool {
        self.get["result"]["decision"]["decision_id"] == serde_json::json!(decision_id)
            && self.rendering["result"]["rendering"].is_string()
            && self.listed.iter().any(|listed| listed == decision_id)
    }

    /// Whether every read answers about `decision_id` as `absent` answers
    /// about `absent_id`, an id no decision has: the get and the rendering
    /// refuse with the same error apart from the id, and the list does not
    /// name it.
    fn answer_as_for_no_decision(
        &self,
        decision_id: &str,
        absent: &Reads,
        absent_id: &str,
    ) -> bool {
        let refused_alike = |answer: &serde_json::Value, about_none: &serde_json::Value| {
            answer["result"].is_null()
                && !answer["error"].is_null()
                && answer["error"].to_string().replace(decision_id, "ID")
                    == about_none["error"].to_string().replace(absent_id, "ID")
        };
        refused_alike(&self.get, &absent.get)
            && refused_alike(&self.rendering, &absent.rendering)
            && !self.listed.iter().any(|listed| listed == decision_id)
    }

    /// The answers, for a failure text.
    fn shown(&self) -> String {
        format!(
            "get {} rendering {} listed {:?}",
            self.get, self.rendering, self.listed
        )
    }
}

/// An id no decision of the rows' hosts has.
fn an_id_no_decision_has() -> String {
    removal_filed_under("no-such-mob/no-such-operation".to_string())
}

/// The lists the Console answers `email` when it asks one decision to a
/// page and follows each cursor: the ids of each page, and the cursors the
/// pages gave. It follows eight cursors at most.
async fn paged_to(app: &axum::Router, email: &str) -> (Vec<Vec<String>>, Vec<String>) {
    let mut pages = Vec::new();
    let mut cursors: Vec<String> = Vec::new();
    for _ in 0..9 {
        let mut params = serde_json::json!({"limit": 1});
        if let Some(cursor) = cursors.last() {
            params["after"] = serde_json::json!(cursor);
        }
        let answer = console_rpc(app, email, "meta/decisions/list", params).await;
        let mut page = Vec::new();
        for member in ["decisions", "statuses"] {
            for row in answer["result"][member].as_array().into_iter().flatten() {
                if let Some(decision_id) = row["decision_id"].as_str() {
                    page.push(decision_id.to_string());
                }
            }
        }
        pages.push(page);
        match answer["result"]["next_after"].as_str() {
            Some(cursor) => cursors.push(cursor.to_string()),
            None => break,
        }
    }
    (pages, cursors)
}

/// `ids`, sorted.
fn sorted(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort();
    ids
}

/// A caller who addresses a mob beside the launch mob by an access rule
/// reads a held change of that mob only while the mob it is held for
/// stands under its id. A removal of a second mob's agent is held, a
/// person approves it and it is carried out; the person a rule lets
/// address the mob reads the decision on a get, a rendering and a list.
/// The mob is destroyed: with no mob under the id, the three reads answer
/// that person as for an id no decision has. A mob is created under the
/// id: they still do, an administrator reads the decision on all three,
/// and a removal held for the mob that stands is read by that person on
/// all three.
#[tokio::test]
async fn a_caller_who_addresses_a_mob_by_a_rule_reads_a_held_change_only_while_its_mob_stands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let approval = match &held {
        Ok(decision_id) => approved(&host, decision_id).await,
        Err(error) => Err(error.clone()),
    };
    let carried_out = match &held {
        Ok(decision_id) => phase_once(&host, decision_id, &[DecisionPhase::Executed]).await,
        Err(error) => Err(error.clone()),
    };
    let app = console_of_approvers(&mut host, &second_id);
    let no_decision = an_id_no_decision_has();
    let decided = held.clone().unwrap_or_else(|_| no_decision.clone());
    let about_no_decision = reads_of(&app, OF_THE_SECOND_MOB, &no_decision).await;
    let while_its_mob_stands = reads_of(&app, OF_THE_SECOND_MOB, &decided).await;
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let with_no_mob_under_the_id = reads_of(&app, OF_THE_SECOND_MOB, &decided).await;
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let with_another_mob_under_the_id = reads_of(&app, OF_THE_SECOND_MOB, &decided).await;
    let to_an_administrator = reads_of(&app, ADMIN, &decided).await;
    let in_the_standing_mob = scout_created_in(&host, Some(&second_id)).await;
    let held_for_the_standing_mob =
        held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let of_the_standing_mob = held_for_the_standing_mob
        .clone()
        .unwrap_or_else(|_| no_decision.clone());
    let about_the_standing_mobs = reads_of(&app, OF_THE_SECOND_MOB, &of_the_standing_mob).await;
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        held.is_ok(),
        "the removal is held: {:?}",
        held.as_ref().err()
    );
    assert_eq!(
        (approval, carried_out),
        (Ok(()), Ok(DecisionPhase::Executed)),
        "a person approves the removal and it is carried out"
    );
    assert!(
        while_its_mob_stands.serve(&decided),
        "while its mob stands, the person a rule lets address the mob reads the decision: {}",
        while_its_mob_stands.shown()
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert!(
        with_another_mob_under_the_id.answer_as_for_no_decision(
            &decided,
            &about_no_decision,
            &no_decision
        ),
        "with another mob under the id, that person is answered as for an id no decision has: \
         {} against {}",
        with_another_mob_under_the_id.shown(),
        about_no_decision.shown()
    );
    assert!(
        with_no_mob_under_the_id.answer_as_for_no_decision(
            &decided,
            &about_no_decision,
            &no_decision
        ),
        "with no mob under the id, that person is answered as for an id no decision has: \
         {} against {}",
        with_no_mob_under_the_id.shown(),
        about_no_decision.shown()
    );
    assert!(
        to_an_administrator.serve(&decided),
        "an administrator reads the decision whatever stands under the id: {}",
        to_an_administrator.shown()
    );
    assert!(
        in_the_standing_mob.is_ok(),
        "an agent of the same name is created in the mob that stands: {in_the_standing_mob:?}"
    );
    assert!(
        held_for_the_standing_mob.is_ok() && held_for_the_standing_mob != held,
        "its removal is held under a decision of its own: {held_for_the_standing_mob:?}"
    );
    assert!(
        about_the_standing_mobs.serve(&of_the_standing_mob),
        "the person a rule lets address the mob reads the change held for the mob that stands: \
         {}",
        about_the_standing_mobs.shown()
    );
}

/// A held change a caller is not served takes no place in a page of a
/// list and is no cursor. Removals are held for the launch mob's agent and
/// for a second mob's; the second mob is destroyed, a mob is created under
/// its id and a removal is held for its agent. The person a rule lets
/// address that id asks one decision to a page: the pages name the launch
/// mob's removal and the standing mob's, one to a page and each once, and
/// no cursor names another decision. An administrator's pages name all
/// three, and the pages of a person whose rule names no mob name the
/// launch mob's alone.
#[tokio::test]
async fn a_held_change_a_caller_is_not_served_takes_no_place_in_a_page_and_is_no_cursor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = (
        scout_created_in(&host, None).await,
        scout_created_in(&host, Some(&second_id)).await,
    );
    let of_the_launch_mob = held_by(&scout_changed_in(&host, None, "remove").await);
    let of_the_destroyed_mob = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let in_the_standing_mob = scout_created_in(&host, Some(&second_id)).await;
    let of_the_standing_mob = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let app = console_of_approvers(&mut host, &second_id);
    let (pages, cursors) = paged_to(&app, OF_THE_SECOND_MOB).await;
    let (to_an_administrator, _) = paged_to(&app, ADMIN).await;
    let (to_the_launch_mobs, _) = paged_to(&app, OF_THE_LAUNCH_MOB).await;
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.0.is_ok() && created.1.is_ok() && in_the_standing_mob.is_ok(),
        "an agent of one name is created in each mob: {created:?} {in_the_standing_mob:?}"
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert!(
        of_the_launch_mob.is_ok() && of_the_destroyed_mob.is_ok() && of_the_standing_mob.is_ok(),
        "the three removals are held: {of_the_launch_mob:?} {of_the_destroyed_mob:?} \
         {of_the_standing_mob:?}"
    );
    let [of_the_launch_mob, of_the_destroyed_mob, of_the_standing_mob] =
        [of_the_launch_mob, of_the_destroyed_mob, of_the_standing_mob]
            .map(Result::unwrap_or_default);
    let served = sorted([of_the_launch_mob.clone(), of_the_standing_mob.clone()]);
    assert_eq!(
        sorted(pages.iter().flatten().cloned()),
        served,
        "the pages name the launch mob's removal and the standing mob's, each once: {pages:?}"
    );
    assert!(
        pages.iter().all(|page| page.len() == 1),
        "each page names one decision: {pages:?}"
    );
    assert!(
        cursors.iter().all(|cursor| served.contains(cursor)),
        "each cursor names a decision a page named: {cursors:?} of {served:?}"
    );
    assert_eq!(
        sorted(to_an_administrator.iter().flatten().cloned()),
        sorted([
            of_the_launch_mob.clone(),
            of_the_destroyed_mob,
            of_the_standing_mob
        ]),
        "an administrator's pages name all three: {to_an_administrator:?}"
    );
    assert_eq!(
        to_the_launch_mobs,
        vec![vec![of_the_launch_mob]],
        "the pages of a person whose rule names no mob name the launch mob's removal alone"
    );
}

/// A held change of the launch mob is read whatever stands beside the
/// launch mob. A removal of the launch mob's agent is held; a second mob
/// is destroyed and a mob is created under its id. The person whose rule
/// names no mob and the person a rule lets address the second mob's id
/// each read the launch mob's removal on a get, a rendering and a list.
#[tokio::test]
async fn a_held_change_of_the_launch_mob_is_read_whatever_stands_beside_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, None).await;
    let held = held_by(&scout_changed_in(&host, None, "remove").await);
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let app = console_of_approvers(&mut host, &second_id);
    let decision_id = held.clone().unwrap_or_else(|_| an_id_no_decision_has());
    let to_the_launch_mobs = reads_of(&app, OF_THE_LAUNCH_MOB, &decision_id).await;
    let to_the_second_mobs = reads_of(&app, OF_THE_SECOND_MOB, &decision_id).await;
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        held.is_ok(),
        "the removal is held: {:?}",
        held.as_ref().err()
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the second mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert!(
        to_the_launch_mobs.serve(&decision_id),
        "the person whose rule names no mob reads the launch mob's removal: {}",
        to_the_launch_mobs.shown()
    );
    assert!(
        to_the_second_mobs.serve(&decision_id),
        "the person a rule lets address the second mob's id reads it too: {}",
        to_the_second_mobs.shown()
    );
}

/// After a restart the read loads no mob. A removal is held for a second
/// mob's agent; the mob is destroyed, a mob is created under its id and a
/// removal is held for its agent; the host is shut down and launched again
/// over the same state, and the second mob is not loaded. The person a
/// rule lets address the mob reads the removal held for the mob that
/// stands, once the mob set's records keep that mob's incarnation (the row
/// waits for the record, ten seconds at most), and is answered about the
/// other removal as for an id no decision has; the mob is still not loaded
/// after the reads.
#[tokio::test]
async fn after_a_restart_the_read_of_a_held_change_loads_no_mob() {
    let stored = super::super::mob_entries::lazy_start_tests::StoredHost::new();
    let kept = tempfile::tempdir().expect("tempdir");
    let roster = Arc::new(LaunchRoster::default());
    let (first, _sessions) =
        stored_launch_whose_removals_a_person_decides(&stored, &roster, kept.path()).await;
    let of_the_second = definition("second");
    roster.configure(&of_the_second.id, &["desk", "back"]);
    let second_id = second_mob(&first, of_the_second.clone()).await;
    let created = scout_created_in(&first, Some(&second_id)).await;
    let of_the_destroyed_mob = held_by(&scout_changed_in(&first, Some(&second_id), "remove").await);
    let destroyed = Box::pin(set_of(&first).mob_destroy(&second_id)).await;
    let created_again = Box::pin(set_of(&first).mob_create_definition(of_the_second)).await;
    let in_the_standing_mob = scout_created_in(&first, Some(&second_id)).await;
    let of_the_standing_mob = held_by(&scout_changed_in(&first, Some(&second_id), "remove").await);
    shut_down(&first, &[]).await;
    drop(first);

    let (mut host, _sessions) =
        stored_launch_whose_removals_a_person_decides(&stored, &roster, kept.path()).await;
    let app = console_of_approvers(&mut host, &second_id);
    let not_loaded_by_the_launch = not_loaded(&host, &second_id).await;
    let no_decision = an_id_no_decision_has();
    let standing = of_the_standing_mob
        .clone()
        .unwrap_or_else(|_| no_decision.clone());
    let other = of_the_destroyed_mob
        .clone()
        .unwrap_or_else(|_| no_decision.clone());
    // The set reads the record of a mob that is not loaded in a pass of
    // its own: the row waits for the record, and reads once.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let on_record = loop {
        let on_record = set_of(&host)
            .mob_inventory()
            .await
            .is_ok_and(|inventory| inventory.dormant_incarnation.contains_key(&second_id));
        if on_record || tokio::time::Instant::now() >= deadline {
            break on_record;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let about_the_standing_mobs = reads_of(&app, OF_THE_SECOND_MOB, &standing).await;
    let about_the_other = reads_of(&app, OF_THE_SECOND_MOB, &other).await;
    let about_no_decision = reads_of(&app, OF_THE_SECOND_MOB, &no_decision).await;
    let not_loaded_by_the_reads = not_loaded(&host, &second_id).await;
    shut_down(&host, &[]).await;
    assert!(
        created.is_ok() && in_the_standing_mob.is_ok(),
        "an agent is created in the second mob and in the mob created under its id: \
         {created:?} {in_the_standing_mob:?}"
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert!(
        of_the_destroyed_mob.is_ok()
            && of_the_standing_mob.is_ok()
            && of_the_destroyed_mob != of_the_standing_mob,
        "a removal is held for each mob, under a decision of its own: \
         {of_the_destroyed_mob:?} {of_the_standing_mob:?}"
    );
    assert!(
        not_loaded_by_the_launch,
        "the launch over the stored state does not load the second mob"
    );
    assert!(
        on_record,
        "the mob set's records keep the incarnation of the mob that is not loaded"
    );
    assert!(
        about_the_standing_mobs.serve(&standing),
        "the person a rule lets address the mob reads the removal held for the mob that \
         stands: {}",
        about_the_standing_mobs.shown()
    );
    assert!(
        about_the_other.answer_as_for_no_decision(&other, &about_no_decision, &no_decision),
        "and is answered about the other removal as for an id no decision has: {} against {}",
        about_the_other.shown(),
        about_no_decision.shown()
    );
    assert!(
        not_loaded_by_the_reads,
        "the reads load no mob: the second mob is still not loaded"
    );
}

/// A held change whose body names no incarnation is read by no caller who
/// addresses its mob by an access rule. A removal of a second mob's agent
/// is held as the family holds it, and a record of the same request is
/// filed beside it under that mob's own key, with a body that names no
/// incarnation. The person a rule lets address the mob reads the family's
/// removal on a get, a rendering and a list, and is answered about the
/// record as for an id no decision has; an administrator reads the record
/// on all three.
#[tokio::test]
async fn a_held_change_whose_body_names_no_incarnation_is_read_by_no_caller_of_a_rule() {
    use meerkat_core::approval::decision::{
        DecisionHoldRequest, DecisionOperationId, DecisionRequestBody, DecisionSubjectKey,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, _definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let held = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let incarnation = host
        .resolve_mob(Some(&second_id))
        .await
        .map(|mob| mob.mob_runtime().handle().incarnation().to_string())
        .map_err(|error| format!("{error:?}"));
    let filed_beside = held
        .clone()
        .and_then(|decision_id| decision_row(&host, &decision_id))
        .and_then(|row| {
            let incarnation = incarnation.clone()?;
            let descriptor = row.descriptor;
            let operation = format!("{}-of-a-body-without-it", descriptor.operation_id.as_str());
            let mut body: serde_json::Value = serde_json::from_str(descriptor.request.get())
                .map_err(|error| format!("the held body is no JSON: {error}"))?;
            let named = body
                .as_object_mut()
                .and_then(|members| members.remove("mob_incarnation"));
            if named.as_ref().and_then(serde_json::Value::as_str) != Some(incarnation.as_str()) {
                return Err(format!(
                    "the family's own body names {named:?}, not its mob's incarnation"
                ));
            }
            let hold = DecisionHoldRequest::new(
                descriptor.kind,
                DecisionSubjectKey::new(format!("{second_id}@{incarnation}/{operation}"))
                    .map_err(|error| format!("no subject key: {error:?}"))?,
                DecisionOperationId::new(operation.as_str())
                    .map_err(|error| format!("no operation id: {error:?}"))?,
                descriptor.request_digest,
                DecisionRequestBody::from_value(&body)
                    .map_err(|error| format!("no request body: {error:?}"))?,
                descriptor.requester,
                descriptor.approver_id,
                descriptor.deadline_at_ms,
                descriptor.rendering,
                descriptor.renderer,
            );
            host.durable_decisions()
                .owner()
                .hold(hold, false)
                .map(|row| row.decision_id.to_string())
                .map_err(|error| format!("the record is not filed: {error}"))
        });
    let app = console_of_approvers(&mut host, &second_id);
    let no_decision = an_id_no_decision_has();
    let of_the_family = held.clone().unwrap_or_else(|_| no_decision.clone());
    let record = filed_beside.clone().unwrap_or_else(|_| no_decision.clone());
    let about_the_familys = reads_of(&app, OF_THE_SECOND_MOB, &of_the_family).await;
    let about_the_record = reads_of(&app, OF_THE_SECOND_MOB, &record).await;
    let about_no_decision = reads_of(&app, OF_THE_SECOND_MOB, &no_decision).await;
    let to_an_administrator = reads_of(&app, ADMIN, &record).await;
    shut_down(&host, &[&second_id]).await;
    assert!(created.is_ok(), "the agent is created: {created:?}");
    assert!(
        held.is_ok(),
        "the removal is held: {:?}",
        held.as_ref().err()
    );
    assert!(
        filed_beside.is_ok(),
        "a record with a body that names no incarnation is filed under the second mob's key: \
         {filed_beside:?}"
    );
    assert!(
        about_the_familys.serve(&of_the_family),
        "the person a rule lets address the mob reads the removal the family holds: {}",
        about_the_familys.shown()
    );
    assert!(
        about_the_record.answer_as_for_no_decision(&record, &about_no_decision, &no_decision),
        "and is answered about the record as for an id no decision has: {} against {}",
        about_the_record.shown(),
        about_no_decision.shown()
    );
    assert!(
        to_an_administrator.serve(&record),
        "an administrator reads the record: {}",
        to_an_administrator.shown()
    );
}

/// [`console_of_approvers`] whose person of the second mob also administers
/// decisions by a rule: the rule lets that person read every decision whole
/// and withdraw, and the person is no administrator of the access
/// configuration, so may address no mob beside the launch mob but the one
/// a rule names.
fn console_of_a_rules_decision_administrator(
    host: &mut UnifiedRuntime,
    second_id: &MobId,
) -> axum::Router {
    let mut access: crate::AccessControlConfig = toml::from_str(&format!(
        "enabled = true\nadmins = [\"{ADMIN}\"]\n\n[approvers.\"{APPROVERS}\"]\n\
         subjects = [\"{OF_THE_LAUNCH_MOB}\"]\n"
    ))
    .expect("the approver map");
    access.rules = vec![
        crate::access::AccessRule {
            id: "views-the-second-mob".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            mobs: vec![second_id.to_string()],
            ..crate::access::AccessRule::default()
        },
        crate::access::AccessRule {
            id: "administers-decisions".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string()],
            actions: vec![
                crate::access::ACTION_DECISIONS_VIEW.to_string(),
                crate::access::ACTION_DECISIONS_ADMIN.to_string(),
            ],
            ..crate::access::AccessRule::default()
        },
    ];
    host.set_access_controller(
        crate::access::AccessController::new(access).expect("access configuration"),
    );
    let decisions = crate::console_auth_config::parse_console_auth_config(&serde_json::json!({
        "shared_secret": SIGNING_SECRET,
        "accounts": "personal",
        "email_allowlist": [ADMIN, OF_THE_SECOND_MOB, OF_THE_LAUNCH_MOB],
    }))
    .expect("console sign-in configuration");
    host.build_reference_app_router(decisions)
}

/// The Console's withdraw is served for a decision its caller may see: a
/// caller who administers decisions by a rule, and addresses a mob beside
/// the launch mob by a rule, withdraws a held change of that mob only while
/// the mob it is held for stands under its id. A removal is held for a
/// second mob's agent; the mob is destroyed, a mob is created under its id
/// and a removal is held for its agent. That caller's withdraw of the
/// first removal answers as for an id no decision has, and the withdraw of
/// the standing mob's removal withdraws it. An administrator of the access
/// configuration is answered about the first removal by what the decision
/// holds.
#[tokio::test]
async fn a_rules_decision_administrator_withdraws_a_held_change_only_while_its_mob_stands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut host, _roster, definition, second_id) =
        two_mobs_whose_removals_a_person_decides(&temp).await;
    let created = scout_created_in(&host, Some(&second_id)).await;
    let of_the_destroyed_mob = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    let ended_by_the_destroy = match &of_the_destroyed_mob {
        Ok(decision_id) => phase_once(&host, decision_id, &[DecisionPhase::Withdrawn]).await,
        Err(error) => Err(error.clone()),
    };
    let created_again = Box::pin(set_of(&host).mob_create_definition(definition)).await;
    let in_the_standing_mob = scout_created_in(&host, Some(&second_id)).await;
    let of_the_standing_mob = held_by(&scout_changed_in(&host, Some(&second_id), "remove").await);
    let app = console_of_a_rules_decision_administrator(&mut host, &second_id);
    let no_decision = an_id_no_decision_has();
    let other = of_the_destroyed_mob
        .clone()
        .unwrap_or_else(|_| no_decision.clone());
    let standing = of_the_standing_mob
        .clone()
        .unwrap_or_else(|_| no_decision.clone());
    let withdraw = |email: &'static str, decision_id: &str| {
        console_rpc(
            &app,
            email,
            "meta/decisions/withdraw",
            serde_json::json!({"decision_id": decision_id}),
        )
    };
    let of_no_decision = withdraw(OF_THE_SECOND_MOB, &no_decision).await;
    let of_the_other = withdraw(OF_THE_SECOND_MOB, &other).await;
    let by_an_administrator = withdraw(ADMIN, &other).await;
    let of_the_standing = withdraw(OF_THE_SECOND_MOB, &standing).await;
    let the_standing_mobs_ended = phase_of(&host, &standing);
    shut_down(&host, &[&second_id]).await;
    assert!(
        created.is_ok() && in_the_standing_mob.is_ok(),
        "an agent is created in the second mob and in the mob created under its id: \
         {created:?} {in_the_standing_mob:?}"
    );
    assert!(
        destroyed.is_ok() && created_again.as_ref().ok() == Some(&second_id),
        "the mob is destroyed and a mob is created under its id: {:?} {:?}",
        destroyed.as_ref().err(),
        created_again.as_ref().err()
    );
    assert!(
        of_the_destroyed_mob.is_ok()
            && of_the_standing_mob.is_ok()
            && of_the_destroyed_mob != of_the_standing_mob,
        "a removal is held for each mob, under a decision of its own: \
         {of_the_destroyed_mob:?} {of_the_standing_mob:?}"
    );
    assert_eq!(
        ended_by_the_destroy,
        Ok(DecisionPhase::Withdrawn),
        "the destroy ended the removal held for the destroyed mob"
    );
    let apart_from_the_id =
        |answer: &serde_json::Value, id: &str| answer["error"].to_string().replace(id, "ID");
    assert!(
        of_the_other["result"].is_null()
            && !of_the_other["error"].is_null()
            && apart_from_the_id(&of_the_other, &other)
                == apart_from_the_id(&of_no_decision, &no_decision),
        "the withdraw by a rule's decision administrator answers as for an id no decision \
         has: {of_the_other} against {of_no_decision}"
    );
    assert!(
        !by_an_administrator["error"].is_null()
            && apart_from_the_id(&by_an_administrator, &other)
                != apart_from_the_id(&of_no_decision, &no_decision),
        "an administrator is answered by what the decision holds, not as for an id no \
         decision has: {by_an_administrator} against {of_no_decision}"
    );
    assert_eq!(
        (
            &of_the_standing["result"]["decision"]["decision_id"],
            the_standing_mobs_ended
        ),
        (&serde_json::json!(standing), Ok(DecisionPhase::Withdrawn)),
        "the same caller withdraws the removal held for the mob that stands: {of_the_standing}"
    );
}
