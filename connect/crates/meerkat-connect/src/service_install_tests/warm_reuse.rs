//! Actual CheckedInstalls calls retain two independent jobs from one warm
//! session. Package responses are mechanical; no installer or service runs.
use super::*;

fn checked_public(second: bool) -> Value {
    let mut value = proposal(true);
    value["declared_service"] = json!({
        "authentication": {"kind": "none"}, "operation_tools": {"read": "actual_read"}
    });
    if second {
        value["validation_id"] = "9".repeat(32).into();
        value["destination"] = format!("/fixture/installations/{}", "9".repeat(32)).into();
        value["mcp_name"] = "second-fixture".into();
    }
    value
}

fn exact_readback(second: bool) -> Value {
    let proposal = checked_public(second);
    let mut value = reconciled(true, 1);
    value["observation"]["job_id"] = if second {
        "4".repeat(32)
    } else {
        "3".repeat(32)
    }
    .into();
    let result = &mut value["observation"]["result"];
    result["declared_service"] = proposal["declared_service"].clone();
    let destination = proposal["destination"].as_str().unwrap();
    result["mcp_path"] = format!("{destination}/ready/mcp.json").into();
    result["receipt_path"] = format!("{destination}/ready/installed.json").into();
    let server = proposal["mcp_name"].as_str().unwrap();
    result["mcp_configuration"] = json!({"mcpServers": {server: {
        "command": "/fixture/python", "args": ["-m", "fixture"], "env": {}
    }}});
    value
}

#[tokio::test]
async fn same_warm_checked_jobs_reselect_x_with_y_without_reinstall_or_authority_reuse() {
    let mut f = Fixture::new(true, 7, DECISION_LIFETIME);
    for second in [false, true] {
        *f.package.reply.lock().unwrap() = checked_public(second);
        f.check().await;
        let mut args = install(true);
        if second {
            args["validation_id"] = "9".repeat(32).into();
        }
        let run = f.start(
            if second { "install-y" } else { "install-x" },
            f.install_name(),
            args,
            true,
        );
        f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
        assert!(!run.await.unwrap().unwrap().result.is_error);
        *f.package.readback_reply.lock().unwrap() = Some(exact_readback(second));
        let job = if second {
            "4".repeat(32)
        } else {
            "3".repeat(32)
        };
        assert!(
            !f.start(
                if second { "read-y" } else { "read-x" },
                "setup_reconcile",
                json!({"job_id": job}),
                true
            )
            .await
            .unwrap()
            .unwrap()
            .result
            .is_error
        );
    }
    let x = f
        .owner
        .connector_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap_or_else(|_| panic!("older X job remains selected after Y"));
    let y = f
        .owner
        .connector_selection(&f.realm, &f.session, &"4".repeat(32))
        .unwrap_or_else(|_| panic!("Y has its own checked job"));
    assert_eq!(x.installed.validation_id, "b".repeat(32));
    assert_eq!(y.installed.validation_id, "9".repeat(32));
    assert_ne!(x.installed.job_id, y.installed.job_id);
    let operations = std::collections::BTreeSet::from(["actual_read".to_owned()]);
    let first_x = x.public_configuration(&operations).unwrap();
    assert_eq!(first_x.name, "selected-fixture");
    assert_eq!(
        y.public_configuration(&operations).unwrap().name,
        "second-fixture"
    );
    let count = f.count();
    let calls = f.package.calls.lock().unwrap().len();
    let selected_again = f
        .owner
        .connector_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap_or_else(|_| panic!("fresh source may reuse exact warm X artifact evidence"));
    assert!(selected_again.check_current().is_ok());
    assert_eq!(
        serde_json::to_value(selected_again.public_configuration(&operations).unwrap()).unwrap(),
        serde_json::to_value(first_x).unwrap()
    );
    assert_eq!(f.count(), count);
    assert_eq!(count, 2, "only the original X and Y installations entered");
    assert_eq!(
        f.package.calls.lock().unwrap().len(),
        calls,
        "selection itself does not dispatch or reinstall"
    );
    assert!(
        f.owner
            .connector_selection(&f.realm, &SessionId::new(), &"3".repeat(32))
            .is_err()
    );
    let mut foreign = f.realm.clone();
    foreign.state_root = foreign.state_root.join("foreign");
    assert!(
        f.owner
            .connector_selection(&foreign, &f.session, &"3".repeat(32))
            .is_err()
    );
    assert!(
        f.owner
            .connector_selection(&f.realm, &f.session, &"5".repeat(32))
            .is_err()
    );
    f.package.epoch.store(8, Ordering::SeqCst);
    assert!(selected_again.check_current().is_err());
    assert!(x.check_current().is_err() && y.check_current().is_err());
    assert!(
        f.owner
            .connector_selection(&f.realm, &f.session, &"3".repeat(32))
            .is_err()
    );
    assert_eq!(f.count(), 2);
    f.approvals.retire();
    f.owner.retire();
    // This owning-crate control selects checked artifacts only. Fresh C
    // approval and grant identities are asserted by the real native fixture.
}
