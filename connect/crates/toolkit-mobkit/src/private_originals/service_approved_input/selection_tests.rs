//! Pure persisted-wire checks, not approval, issuance or native admission.
//! LEGACY_WIRE freezes the singleton field order from public 4064f553's
//! ApprovedTaskLink and the selected Core's passive retained-work codec. The
//! coordinates are deliberately historical and carry no usable authority.
use super::selection::{ApprovedTaskBatch, BatchVersion};
use super::*;
use serde_json::{Value, json};

const LEGACY_WIRE: &str = concat!(
    r#"{"parent":{"route":"console-route","original":"original-a","event_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    r#","acquisition":{"realm":"realm-a","owner":"toolkit/acquisition/application-a","id":"setup-a"}"#,
    r#","original_task":{"origin_session_id":"00000000-0000-4000-8000-000000000001","retained_work":{"runtime_id":"rt:session:00000000-0000-4000-8000-000000000001","run_id":"00000000-0000-4000-8000-000000000002","contributors":[{"input_id":"00000000-0000-4000-8000-000000000003","association_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","submission_digest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}],"selected_input_bindings":{"00000000-0000-4000-8000-000000000003":{"binding":"exact-native-binding","batch_key":"native-batch"}},"controller":null},"member":{"mob_id":"paired_host","role":"research","member":"native-member-a"}}"#,
    r#","grant":{"route":"console-route","decision":"unissued-service-a","decision_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}"#,
    r#","before_ms":1800000000000}"#,
);

const LEGACY_CANONICAL_SHA256: &str = "ed77be33a1978f4a96cd4fd6b66c5b883de92cc7070646cf8a990c49782d7e46";

fn legacy() -> ApprovedTaskMember {
    serde_json::from_str(LEGACY_WIRE).expect("frozen legacy wire must decode")
}

fn batch(count: usize) -> ApprovedTaskLink {
    let old = legacy();
    ApprovedTaskLink::Batch(ApprovedTaskBatch {
        kind: BatchVersion::PublicServicesV1, setup: None,
        header: old.header,
        set_sha256: "e".repeat(64),
        members: (0..count)
            .map(|index| ApprovedServiceMember {
                connection: Id::new(format!("service-{index:02}")).unwrap(),
                grant: PrivateServiceGrantRef {
                    route: old.grant.route.clone(),
                    decision: Id::new(format!("unissued-service-{index:02}")).unwrap(),
                    decision_sha256: format!("{index:064x}"),
                },
            })
            .collect(),
        catalog: PrivateServiceGrantRef {
            route: old.grant.route,
            decision: Id::new("unissued-catalog").unwrap(),
            decision_sha256: "f".repeat(64),
        },
    })
}

fn batch_value(count: usize) -> Value {
    serde_json::to_value(batch(count)).unwrap()
}

fn assert_refused(label: &str, value: Value) {
    assert!(
        serde_json::from_value::<ApprovedTaskLink>(value).is_err(),
        "malformed approved source was accepted: {label}"
    );
}

fn assert_duplicate_refused(label: &str, wire: &str, key: &str) {
    let value: Value = serde_json::from_str(wire).unwrap();
    // Parse actual duplicate JSON fields. A Value round-trip would erase this
    // condition before the strict wire codec could observe it.
    let duplicated = format!(
        "{{{}:{},{}",
        serde_json::to_string(key).unwrap(),
        serde_json::to_string(&value[key]).unwrap(),
        &wire[1..]
    );
    assert!(
        serde_json::from_str::<ApprovedTaskLink>(&duplicated).is_err(),
        "duplicate approved-source field was accepted: {label}/{key}"
    );
}

#[test]
fn singleton_preserves_frozen_legacy_bytes_and_canonical_digest() {
    let decoded: ApprovedTaskLink = serde_json::from_str(LEGACY_WIRE).unwrap();
    assert!(matches!(decoded, ApprovedTaskLink::Single(_)));
    assert_eq!(serde_json::to_string(&decoded).unwrap(), LEGACY_WIRE);
    let canonical = canonical_json(&decoded).unwrap();
    let frozen: Value = serde_json::from_str(LEGACY_WIRE).unwrap();
    assert_eq!(canonical, canonical_json(&frozen).unwrap());
    assert_eq!(
        toolkit_package::ContentDigest::of_bytes(&canonical).to_string(),
        LEGACY_CANONICAL_SHA256
    );
    let member: ApprovedTaskMember = serde_json::from_str(LEGACY_WIRE).unwrap();
    assert_eq!(serde_json::to_string(&member).unwrap(), LEGACY_WIRE);
    assert!(decoded.singleton_grant().unwrap() == &member.grant);
    assert!(decoded.attachment_member(&member.grant).unwrap() == member);
}

#[test]
fn singleton_rejects_missing_null_unknown_and_duplicate_legacy_fields() {
    let original: Value = serde_json::from_str(LEGACY_WIRE).unwrap();
    for key in ["parent", "acquisition", "original_task", "grant", "before_ms"] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert_refused(&format!("missing {key}"), missing);
        let mut null = original.clone();
        null[key] = Value::Null;
        assert_refused(&format!("null {key}"), null);
        assert_duplicate_refused("singleton", LEGACY_WIRE, key);
    }
    let mut unknown = original.clone();
    unknown["unexpected"] = json!(true);
    assert_refused("unknown singleton field", unknown);
    let mut mixed = original;
    mixed["kind"] = json!("public_services_v1");
    assert_refused("legacy object with batch discriminator", mixed);
}

#[test]
fn batch_roundtrips_two_and_eight_members_with_exact_attachment_projection() {
    for count in [2, 8] {
        let expected = batch(count);
        expected.validate().unwrap();
        let wire = serde_json::to_vec(&expected).unwrap();
        let decoded: ApprovedTaskLink = serde_json::from_slice(&wire).unwrap();
        assert!(decoded == expected);
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), wire);
        assert_eq!(canonical_json(&decoded).unwrap(), canonical_json(&expected).unwrap());
        assert!(decoded.singleton_grant().is_err());
        let ApprovedTaskLink::Batch(selected) = &decoded else {
            panic!("versioned set decoded as legacy singleton");
        };
        let attachments = decoded.attachments();
        assert_eq!(attachments.len(), count);
        assert_eq!(decoded.grants().len(), count + 1);
        assert!(decoded.contains(&selected.catalog));
        assert!(decoded.attachment_member(&selected.catalog).is_err());
        assert!(decoded.attachment_grant("not-selected").is_err());
        for (member, projected) in selected.members.iter().zip(attachments) {
            assert!(projected.header == selected.header);
            assert!(projected.grant == member.grant);
            assert!(decoded.attachment_grant(member.connection.as_str()).unwrap() == &member.grant);
            assert!(decoded.attachment_member(&member.grant).unwrap() == projected);
        }
    }
}

#[test]
fn batch_requires_exact_version_and_complete_strict_wire_fields() {
    let original = batch_value(2);
    let wire = serde_json::to_string(&original).unwrap();
    for key in [
        "kind", "parent", "acquisition", "original_task", "before_ms",
        "set_sha256", "members", "catalog",
    ] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert_refused(&format!("batch missing {key}"), missing);
        let mut null = original.clone();
        null[key] = Value::Null;
        assert_refused(&format!("batch null {key}"), null);
        assert_duplicate_refused("batch", &wire, key);
    }
    for kind in [json!("public_services_v2"), json!("Single"), json!(0), json!({})] {
        let mut malformed = original.clone();
        malformed["kind"] = kind;
        assert_refused("unknown or malformed batch discriminator", malformed);
    }
    let mut unknown = original.clone();
    unknown["unexpected"] = json!(true);
    assert_refused("unknown batch field", unknown);
    let mut mixed = original.clone();
    mixed["grant"] = json!(legacy().grant);
    assert_refused("batch object with singleton grant", mixed);
    let mut unknown_member = original.clone();
    unknown_member["members"][0]["unexpected"] = json!(true);
    assert_refused("unknown member field", unknown_member);
    for key in ["connection", "grant"] {
        let mut missing = original.clone();
        missing["members"][0].as_object_mut().unwrap().remove(key);
        assert_refused(&format!("member missing {key}"), missing);
        let mut null = original.clone();
        null["members"][0][key] = Value::Null;
        assert_refused(&format!("member null {key}"), null);
    }
}

#[test]
fn batch_rejects_invalid_cardinality_order_digest_and_routes() {
    for count in [0, 1, 9] {
        assert_refused(&format!("member count {count}"), batch_value(count));
    }
    let mut unsorted = batch_value(2);
    unsorted["members"].as_array_mut().unwrap().reverse();
    assert_refused("unsorted targets", unsorted);
    let mut repeated_target = batch_value(2);
    repeated_target["members"][1]["connection"] = repeated_target["members"][0]["connection"].clone();
    assert_refused("repeated target with distinct grants", repeated_target);
    for digest in ["e".repeat(63), "g".repeat(64), "e".repeat(65)] {
        let mut invalid = batch_value(2);
        invalid["set_sha256"] = digest.into();
        assert_refused("malformed set digest", invalid);
    }
    let mut member_route = batch_value(2);
    member_route["members"][0]["grant"]["route"] = "other-route".into();
    assert_refused("member grant from another route", member_route);
    let mut catalog_route = batch_value(2);
    catalog_route["catalog"]["route"] = "other-route".into();
    assert_refused("catalog grant from another route", catalog_route);
}

#[test]
fn batch_rejects_reused_grant_coordinates_even_with_changed_evidence() {
    for contradictory_digest in [false, true] {
        let mut duplicate = batch_value(2);
        duplicate["members"][1]["grant"] = duplicate["members"][0]["grant"].clone();
        if contradictory_digest {
            duplicate["members"][1]["grant"]["decision_sha256"] = "9".repeat(64).into();
        }
        assert_refused("same member grant coordinate", duplicate);
        let mut catalog_reuse = batch_value(2);
        catalog_reuse["catalog"] = catalog_reuse["members"][0]["grant"].clone();
        if contradictory_digest {
            catalog_reuse["catalog"]["decision_sha256"] = "9".repeat(64).into();
        }
        assert_refused("catalog reuses member grant coordinate", catalog_reuse);
    }
}
