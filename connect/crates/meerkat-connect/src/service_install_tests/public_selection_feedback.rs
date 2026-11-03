//! Actual checked-owner and software-review calls with synthetic package replies.
//! Exercise the shared public singleton/batch selection entry, not native ACCESS
//! admission, package execution, service attachment or an ordinary agent run.

use super::*;
use crate::installed_service_review::account_request::public::select_public_job;
use meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST;

const SERVER: &str = "toolkit-services-aaaaaaaaaaaa";
const RAW_OPERATION: &str = "fixture_public_read";

fn help() -> Value {
    // The existing fixture's family/profile are labels for an inert delegate.
    // This is an explicit synthetic public declaration, not a Slack auth claim.
    json!({"family":"services","profile":"slack", "authentication":{"kind":"none"},
        "fields":[], "operations":[{"id":"auth_test","tool":RAW_OPERATION}]})
}

async fn observe(fixture: &Fixture, id: &str, name: &str, args: Value, reply: Value) {
    *fixture.package.readback_reply.lock().unwrap() = Some(reply.clone());
    let result = fixture.start(id, name, args, true).await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.result.text_content()).unwrap(),
        reply
    );
}

fn refusal(fixture: &Fixture, realm: &RealmLocator, session: &SessionId, job: &str) -> String {
    let calls = fixture.package.calls.lock().unwrap().len();
    let reviews = serde_json::to_value(
        fixture
            .approvals
            .shared
            .service
            .list(Default::default())
            .unwrap(),
    )
    .unwrap();
    let error = select_public_job(&fixture.owner, realm, session, job)
        .err()
        .expect("selection must refuse before access review");
    let ToolError::InvalidArguments { name, reason } = error else {
        panic!("selection refusal must give a model-visible correction: {error:?}");
    };
    assert_eq!(name, SETUP_PUBLIC_SERVICE_REQUEST);
    assert!(reason.contains(&json!(job).to_string()));
    assert!(reason.contains("This request has not entered access review."));
    assert_eq!(fixture.package.calls.lock().unwrap().len(), calls);
    assert_eq!(fixture.count(), 1);
    assert_eq!(
        serde_json::to_value(
            fixture
                .approvals
                .shared
                .service
                .list(Default::default())
                .unwrap(),
        )
        .unwrap(),
        reviews
    );
    reason
}

async fn installed_after_early_help() -> Fixture {
    let mut fixture = Fixture::local(false, 0);
    fixture.package.reply.lock().unwrap()["credential_environment_references"] = json!([]);
    observe(
        &fixture,
        "early-help",
        "setup_help",
        json!({"family":"services","profile":"slack"}),
        help(),
    )
    .await;
    fixture.check().await;
    let install = fixture.install();
    fixture
        .prompt()
        .await
        .decide(ApprovalDecision::Approve)
        .unwrap();
    let result = install.await.unwrap().unwrap();
    let reply: Value = serde_json::from_str(&result.result.text_content()).unwrap();
    assert_eq!(reply["job"]["id"], "3".repeat(32));
    assert_eq!(fixture.count(), 1);
    assert_eq!(
        fixture
            .approvals
            .shared
            .service
            .list(Default::default())
            .unwrap()
            .len(),
        1
    );
    let mut completed = job_readback(false);
    completed["job"]["result"]["mcp_configuration"]["mcpServers"][SERVER]["env"] = json!({});
    observe(
        &fixture,
        "completed-job",
        "setup_job",
        json!({"job_id":"3".repeat(32)}),
        completed,
    )
    .await;
    fixture
}

async fn reconcile(fixture: &Fixture) {
    let mut matched = reconciled(false, 1);
    matched["observation"]["result"]["mcp_configuration"]["mcpServers"][SERVER]["env"] = json!({});
    observe(
        fixture,
        "reconcile",
        "setup_reconcile",
        json!({"job_id":"3".repeat(32)}),
        matched,
    )
    .await;
}

async fn fresh_help(fixture: &Fixture) {
    observe(
        fixture,
        "fresh-help",
        "setup_help",
        json!({"family":"services","profile":"slack"}),
        help(),
    )
    .await;
}

#[tokio::test]
async fn public_selection_feedback_repairs_receipt_then_post_check_metadata_without_reinstall() {
    let fixture = installed_after_early_help().await;
    let job = "3".repeat(32);
    let unconfirmed = refusal(&fixture, &fixture.realm, &fixture.session, &job);
    assert!(unconfirmed.contains("setup_job"));
    assert!(unconfirmed.contains("setup_reconcile when reconciliation is available"));
    assert!(unconfirmed.contains("same job_id"));
    assert!(unconfirmed.contains("Do not reinstall"));
    assert!(!unconfirmed.contains("setup_help"));
    assert!(!unconfirmed.contains("slack"));

    reconcile(&fixture).await;
    let metadata = refusal(&fixture, &fixture.realm, &fixture.session, &job);
    assert!(metadata.contains("setup_help with {\"family\":\"services\",\"profile\":\"slack\"}"));
    assert!(metadata.contains("after the current setup_check"));
    assert!(metadata.contains("help read before that check is not retained"));
    assert!(metadata.contains(
        "same job, exact raw operation names, intended consumer agent and requested lifetime"
    ));
    assert!(metadata.contains("Do not reinstall"));
    assert!(!metadata.contains("setup_reconcile"));

    fresh_help(&fixture).await;
    let selected = select_public_job(&fixture.owner, &fixture.realm, &fixture.session, &job)
        .unwrap_or_else(|error| panic!("exact reconciled public selection: {error:?}"));
    assert_eq!(selected.job_id(), job);
    assert_eq!(selected.session(), &fixture.session);
    let operations = [RAW_OPERATION.to_owned()].into_iter().collect();
    assert_eq!(selected.public_operations().unwrap(), operations);
    let config = selected.public_configuration(&operations).unwrap();
    assert_eq!(config.name, SERVER);
    assert!(
        selected
            .public_configuration(&["auth_test".to_owned()].into_iter().collect())
            .is_err()
    );
    assert_eq!(fixture.count(), 1);
    assert_eq!(
        fixture
            .approvals
            .shared
            .service
            .list(Default::default())
            .unwrap()
            .len(),
        1
    );
    let calls: Vec<_> = fixture
        .package
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| call.0.clone())
        .collect();
    assert_eq!(
        calls,
        [
            "setup_help",
            "setup_check",
            "setup_install",
            "setup_job",
            "setup_reconcile",
            "setup_help"
        ]
    );
}

#[tokio::test]
async fn public_selection_feedback_keeps_foreign_unknown_and_stale_jobs_refused() {
    let fixture = installed_after_early_help().await;
    reconcile(&fixture).await;
    // Even before metadata is available, another session/realm/job cannot
    // learn the retained publisher family/profile through this feedback.
    let mut foreign_realm = fixture.realm.clone();
    foreign_realm.state_root = foreign_realm.state_root.join("foreign");
    for (realm, session, job) in [
        (fixture.realm.clone(), SessionId::new(), "3".repeat(32)),
        (foreign_realm, fixture.session.clone(), "3".repeat(32)),
        (
            fixture.realm.clone(),
            fixture.session.clone(),
            "4".repeat(32),
        ),
    ] {
        let reason = refusal(&fixture, &realm, &session, &job);
        assert!(reason.contains("Unknown, foreign or stale jobs remain refused"));
        assert!(!reason.contains("setup_help"));
        assert!(!reason.contains("slack"));
    }
    fresh_help(&fixture).await;
    let job = "3".repeat(32);
    assert!(select_public_job(&fixture.owner, &fixture.realm, &fixture.session, &job).is_ok());
    fixture.package.epoch.store(8, Ordering::SeqCst);
    let reason = refusal(&fixture, &fixture.realm, &fixture.session, &job);
    assert!(reason.contains("Unknown, foreign or stale jobs remain refused"));
    assert!(!reason.contains("setup_help"));
    assert!(!reason.contains("slack"));
}
