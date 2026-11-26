//! Rows for `meta/agent_memory/purge` on the Console.

use super::memory_fixture::{ConsoleMemoryFixture, MOB, REALM, RowResult, mob_scope};
use super::*;
use crate::identity_first::agent_memory::AgentMemoryProvider;
use crate::memory::records::{
    MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord, RecordStatus,
};

const IDENTITY: &str = "lead:main";

fn identity_scope() -> MemoryScope {
    MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: IDENTITY.to_string(),
    }
}

fn identity_params(memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": IDENTITY, "memory_id": memory_id })
}

/// The purge rides `agent.memory.admin`: on the identity the request names
/// for an identity's record, unscoped for a shared record, and unscoped for
/// a request that names neither, so a caller without it learns nothing
/// from the parse.
#[test]
fn purge_requires_agent_memory_admin_on_the_identity_or_unscoped() {
    assert_eq!(
        super::super::console_rpc_access_requirements(
            "meta/agent_memory/purge",
            &identity_params("mem-own")
        ),
        Some(vec![(
            crate::access::ACTION_AGENT_MEMORY_ADMIN,
            Some(IDENTITY.to_string())
        )])
    );
    for params in [
        json!({ "realm": REALM, "scope": { "kind": "mob", "key": MOB }, "memory_id": "mem-shared" }),
        json!({ "realm": REALM, "scope": { "kind": "realm" }, "memory_id": "mem-realm" }),
        json!({ "realm": REALM, "memory_id": "mem-own" }),
    ] {
        assert_eq!(
            super::super::console_rpc_access_requirements("meta/agent_memory/purge", &params),
            Some(vec![(crate::access::ACTION_AGENT_MEMORY_ADMIN, None)]),
            "{params}"
        );
    }
    assert!(super::super::is_console_mutating_rpc_method(
        "meta/agent_memory/purge"
    ));
}

/// On the Console the purge is the operator's, and a caller who may change
/// nothing there purges nothing.
#[tokio::test]
async fn console_purge_blanks_a_retired_record_as_the_operator() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge").await?;
    fixture
        .seed(
            "mem-own",
            identity_scope(),
            "Home address",
            "12 Example Lane",
        )
        .await;
    fixture
        .store
        .forget_authored(
            &identity_scope(),
            "mem-own",
            MemoryAuthor::Operator { principal: None },
        )
        .await?;

    let read_only = fixture
        .rpc("meta/agent_memory/purge", identity_params("mem-own"), false)
        .await;
    assert!(!read_only["error"].is_null(), "{read_only:#?}");
    assert_eq!(fixture.record("mem-own").await.title, "Home address");

    let response = fixture
        .rpc("meta/agent_memory/purge", identity_params("mem-own"), true)
        .await;
    assert_eq!(
        response["result"],
        json!({ "memory_id": "mem-own", "purged": true, "purged_ids": ["mem-own"] }),
        "{response:#?}"
    );
    let record = fixture.record("mem-own").await;
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert_eq!((record.title.as_str(), record.body.as_str()), ("", ""));
    assert_eq!(
        fixture.audit_authors("mem-own", "purge"),
        vec![json!({ "author": "operator" })]
    );
    Ok(())
}

/// A shared record is named by its scope.
#[tokio::test]
async fn console_purge_blanks_a_retired_mob_record() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge-mob").await?;
    fixture
        .seed(
            "mem-shared",
            mob_scope(),
            "Customer note",
            "a customer's detail",
        )
        .await;
    fixture
        .store
        .forget_authored(
            &mob_scope(),
            "mem-shared",
            MemoryAuthor::Operator { principal: None },
        )
        .await?;

    let response = fixture
        .rpc(
            "meta/agent_memory/purge",
            json!({ "realm": REALM, "scope": { "kind": "mob", "key": MOB }, "memory_id": "mem-shared" }),
            true,
        )
        .await;

    assert_eq!(
        response["result"]["purged_ids"],
        json!(["mem-shared"]),
        "{response:#?}"
    );
    assert_eq!(fixture.record("mem-shared").await.body, "");
    Ok(())
}

/// The panel's record answer says a purged record is one, so the front end
/// shows a notice in place of an empty body; any other record carries no
/// such field.
#[tokio::test]
async fn panel_record_json_marks_a_purged_record() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge-panel").await?;
    fixture
        .seed(
            "mem-own",
            identity_scope(),
            "Home address",
            "12 Example Lane",
        )
        .await;
    let active = fixture.record("mem-own").await;
    assert!(
        super::super::memory_panel_record_json(&active, true)
            .get("purged")
            .is_none()
    );
    fixture
        .store
        .forget_authored(
            &identity_scope(),
            "mem-own",
            MemoryAuthor::Operator { principal: None },
        )
        .await?;
    let retired = fixture.record("mem-own").await;
    assert!(
        super::super::memory_panel_record_json(&retired, true)
            .get("purged")
            .is_none(),
        "a retired record that still has its text is not purged"
    );
    fixture
        .rpc("meta/agent_memory/purge", identity_params("mem-own"), true)
        .await;
    let purged = fixture.record("mem-own").await;
    for include_body in [true, false] {
        let value = super::super::memory_panel_record_json(&purged, include_body);
        assert_eq!(value["purged"], json!(true), "{value:#?}");
        assert_eq!(value["status"]["status"], json!("tombstoned"), "{value:#?}");
    }
    Ok(())
}

/// What the panel's record read answers after a purge. The forgotten
/// record, and the earlier version an update replaced, are each asked for
/// by their own id: each answers an empty title, an empty description and
/// an empty body, and says it was purged. No row of either record's chain
/// holds text.
#[tokio::test]
async fn panel_record_read_answers_empty_text_after_a_purge() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge-panel-read").await?;
    let wording = |body: &str| NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: "Home address".to_string(),
        description: "Where the family lives".to_string(),
        body: body.to_string(),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    };
    let operator = || MemoryAuthor::Operator { principal: None };
    let earlier = fixture
        .store
        .remember_authored(&identity_scope(), wording("12 Example Lane"), operator())
        .await?
        .memory_id;
    let latest = fixture
        .store
        .supersede_authored(
            &identity_scope(),
            &earlier,
            wording("14 Example Lane"),
            operator(),
        )
        .await?
        .memory_id;
    fixture
        .store
        .forget_authored(&identity_scope(), &latest, operator())
        .await?;
    let read = |id: &str| fixture.panel_rpc("meta/memory/panel/record", json!({ "memory_id": id }));

    // Before the purge the panel reads each wording.
    for (id, body) in [(&earlier, "12 Example Lane"), (&latest, "14 Example Lane")] {
        let answer = read(id).await;
        assert_eq!(
            answer["result"]["record"]["body"],
            json!(body),
            "{answer:#?}"
        );
        assert!(
            answer["result"]["record"].get("purged").is_none(),
            "{answer:#?}"
        );
    }

    let purged = fixture
        .rpc("meta/agent_memory/purge", identity_params(&latest), true)
        .await;
    assert_eq!(purged["result"]["purged"], json!(true), "{purged:#?}");

    for (id, status) in [(&latest, "tombstoned"), (&earlier, "superseded")] {
        let answer = read(id).await;
        assert!(answer["error"].is_null(), "{id}: {answer:#?}");
        let record = &answer["result"]["record"];
        assert_eq!(record["id"], json!(id), "{answer:#?}");
        assert_eq!(record["status"]["status"], json!(status), "{answer:#?}");
        assert_eq!(record["purged"], json!(true), "{id}: {answer:#?}");
        for field in ["title", "description", "body"] {
            assert_eq!(record[field], json!(""), "{id}, {field}: {answer:#?}");
        }
        let chain = answer["result"]["chain"]
            .as_array()
            .ok_or("the record read answers a chain")?;
        assert!(!chain.is_empty(), "{id}: {answer:#?}");
        for link in chain {
            assert_eq!(link["purged"], json!(true), "{id}: {answer:#?}");
            assert_eq!(link["body_bytes"], json!(0), "{id}: {answer:#?}");
            for field in ["title", "description"] {
                assert_eq!(link[field], json!(""), "{id}, {field}: {answer:#?}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn console_advertises_purge_only_where_it_may_change_something() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge-advertised").await?;
    let method = "meta/agent_memory/purge".to_string();
    assert!(fixture.advertised(true).await.contains(&method));
    assert!(!fixture.advertised(false).await.contains(&method));
    Ok(())
}
