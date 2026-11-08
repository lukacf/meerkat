//! Pure framing and lifetime controls. No root adoption or native claim.
use super::*;
use serde_json::{Value, json};

fn header(service: &str) -> Value {
    json!({"schema":"meerkat.connect.selected-input.v1", "service_id":service,
    "profile":"python", "component":"selected-component", "members":[
        {"name":"activation.json","size":2,"sha256":digest(b"{}")},
        {"name":"artifact.whl","size":5,"sha256":digest(b"wheel")},
        {"name":"package.json","size":2,"sha256":digest(b"{}")}
    ]})
}
fn frame(header: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        b"MCSPKG01".as_slice(),
        &(header.len() as u32).to_be_bytes(),
        header,
        payload,
    ]
    .concat()
}
fn parse(raw: Vec<u8>) -> Result<Bundle, ConnectError> {
    let identity = Identity {
        service_id: "repository".into(),
        bundle_sha256: digest(&raw),
    };
    Bundle::parse(raw, &identity)
}
pub(super) fn fixture(service: &str) -> (Vec<u8>, Identity) {
    let bytes = frame(&serde_json::to_vec(&header(service)).unwrap(), b"{}wheel{}");
    let identity = Identity {
        service_id: service.into(),
        bundle_sha256: digest(&bytes),
    };
    (bytes, identity)
}

#[test]
fn exact_frame_retains_bytes_and_projects_only_staged_identity() {
    let (bytes, identity) = fixture("repository");
    let bundle = Bundle::parse(bytes.clone(), &identity).unwrap();
    assert_eq!(bundle.bytes, bytes);
    assert_eq!(bundle.bytes(&bundle.members[1]), b"wheel");
    let value = serde_json::to_value(
        bundle
            .observation("input-0123456789abcdef0123456789abcdef".into())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value.as_object().unwrap().len(), 7);
    assert_eq!(value["service_id"], "repository");
    assert_eq!(value["selection_sha256"], digest(b"{}"));
    assert_eq!(value["package_sha256"], digest(b"{}"));
    assert!(!value.to_string().contains("/"));
}

#[test]
fn framing_rejects_truncation_trailing_bytes_wrong_digests_and_wrong_slot() {
    let (bytes, mut identity) = fixture("repository");
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(parse(bad).is_err());
    for size in [0, 7, 11, bytes.len() - 1] {
        assert!(parse(bytes[..size].to_vec()).is_err());
    }
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(parse(bad).is_err());
    let mut bad = bytes.clone();
    *bad.last_mut().unwrap() = b'!';
    assert!(parse(bad).is_err());
    identity.service_id = "other".into();
    assert!(Bundle::parse(bytes.clone(), &identity).is_err());
    identity.service_id = "repository".into();
    identity.bundle_sha256 = "f".repeat(64);
    assert!(Bundle::parse(bytes, &identity).is_err());
}

#[test]
fn closed_header_refuses_duplicates_unknowns_floats_and_noncanonical_members() {
    let baseline = header("repository");
    for pointer in ["", "/members/0"] {
        let mut bad = baseline.clone();
        bad.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(true));
        assert!(parse(frame(&serde_json::to_vec(&bad).unwrap(), b"{}wheel{}")).is_err());
    }
    let raw = serde_json::to_string(&baseline)
        .unwrap()
        .replace("\"size\":2", "\"size\":2,\"size\":2");
    assert!(parse(frame(raw.as_bytes(), b"{}wheel{}")).is_err());
    for replacement in [
        json!(2.0),
        json!(-1),
        json!("2"),
        json!(true),
        json!(u64::MAX),
    ] {
        let mut bad = baseline.clone();
        bad["members"][0]["size"] = replacement;
        assert!(parse(frame(&serde_json::to_vec(&bad).unwrap(), b"{}wheel{}")).is_err());
    }
    for name in [
        "../activation.json",
        "closure/../x",
        "closure/.hidden",
        "closure/a/b",
        "artifact.zip",
        "package.json",
    ] {
        let mut bad = baseline.clone();
        bad["members"][0]["name"] = json!(name);
        assert!(parse(frame(&serde_json::to_vec(&bad).unwrap(), b"{}wheel{}")).is_err());
    }
    let mut bad = baseline;
    bad["members"].as_array_mut().unwrap().swap(0, 1);
    assert!(parse(frame(&serde_json::to_vec(&bad).unwrap(), b"{}wheel{}")).is_err());
}

#[test]
fn bounds_refuse_header_payload_depth_nodes_and_invalid_utf8() {
    assert!(parse(frame(&vec![b' '; HEADER_MAX + 1], b"")).is_err());
    assert!(
        parse(frame(
            &serde_json::to_vec(&header("repository")).unwrap(),
            &vec![0; PAYLOAD_MAX + 1]
        ))
        .is_err()
    );
    assert!(parse(frame(&[0xff], b"")).is_err());
    assert!(mapping::bounded_json(b"[[[[[[[[[[0]]]]]]]]]]", HEADER_MAX, 8, 2048).is_err());
    assert!(mapping::bounded_json(b"[0,1,2]", HEADER_MAX, 8, 3).is_err());
    let mut many = header("repository");
    let member = many["members"][0].clone();
    many["members"] = json!(vec![member; 65]);
    assert!(parse(frame(&serde_json::to_vec(&many).unwrap(), b"")).is_err());
}

#[test]
fn selected_input_is_single_use_and_retirement_never_restores_staging() {
    let input = inert_test_input("repository");
    input.begin().unwrap();
    assert!(input.begin().is_err());
    input.finish(true).unwrap();
    assert!(input.begin().is_err());
    assert!(input.finish(true).is_err());
    let input = inert_test_input("repository");
    input.begin().unwrap();
    input.retire();
    assert!(input.finish(true).is_err());
    assert!(input.begin().is_err());
    let input = inert_test_input("repository");
    input.retire();
    assert!(input.begin().is_err());
}

#[test]
fn closure_members_are_flat_ordered_and_bound_to_their_own_hashes() {
    let mut value = header("repository");
    value["members"].as_array_mut().unwrap().insert(
        2,
        json!({"name":"closure/dependency.whl","size":3,"sha256":digest(b"dep")}),
    );
    let bundle = parse(frame(&serde_json::to_vec(&value).unwrap(), b"{}wheeldep{}")).unwrap();
    assert_eq!(bundle.bytes(&bundle.members[2]), b"dep");
    value["members"][2]["sha256"] = json!(digest(b"bad"));
    assert!(parse(frame(&serde_json::to_vec(&value).unwrap(), b"{}wheeldep{}")).is_err());
}
