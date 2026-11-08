//! Wire-shape controls only. None authenticates a parent or mints a caller.
use super::*;
use serde_json::json;

fn nonce(value: char) -> Nonce {
    value.to_string().repeat(64).try_into().unwrap()
}
fn envelope() -> Envelope {
    Envelope {
        channel: nonce('a'),
        invocation: nonce('b'),
        reservation: nonce('c'),
        mapping_sha256: nonce('d'),
        request_id: nonce('e'),
    }
}
fn attest() -> Value {
    let mut value = envelope().fields("PARENT_ATTEST");
    let fields = value.as_object_mut().unwrap();
    fields.extend(
        json!({"broker_instance":"f".repeat(64),"challenge":"0".repeat(64),
        "issuer":"https://issuer.invalid","subject":"subject","scope":"s".repeat(43),
        "workspace":"3".repeat(64),"origin":"https://app.invalid","evidence_id":"1".repeat(64),
        "evidence_revision":1,"expires_at_ms":123456789})
        .as_object()
        .unwrap()
        .clone(),
    );
    value
}
fn accepts(value: &Value) -> bool {
    decode(&serde_json::to_vec(value).unwrap()).is_ok()
}

#[test]
fn committed_protected_parent_protocol_shapes_preserve_exact_attestation_and_control_fields() {
    let ParentRecord::Attest(a) = decode(&serde_json::to_vec(&attest()).unwrap()).unwrap() else {
        panic!("shape");
    };
    assert!(a.envelope == envelope());
    assert!(
        a.broker_instance == nonce('f') && a.challenge == nonce('0') && a.evidence_id == nonce('1')
    );
    assert_eq!(
        (&*a.issuer, &*a.subject, &*a.origin),
        ("https://issuer.invalid", "subject", "https://app.invalid")
    );
    assert_eq!(a.scope, "s".repeat(43));
    assert!(a.workspace == nonce('3'));
    assert_eq!(a.expires_at_ms, 123456789);
    for kind in ["ACTIVATE", "FENCE"] {
        let mut value = envelope().fields(kind);
        value["control_id"] = json!(nonce('2'));
        let record = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        let c = match record {
            ParentRecord::Activate(c) if kind == "ACTIVATE" => c,
            ParentRecord::Fence(c) if kind == "FENCE" => c,
            _ => panic!("direction"),
        };
        assert!(c.envelope == envelope() && c.control_id == nonce('2'));
    }
}

#[test]
fn duplicate_escaped_fields_unknown_fields_and_wrong_direction_are_refused() {
    let raw = serde_json::to_string(&attest()).unwrap();
    for duplicate in ["\"version\":1", "\"version\":2", "\"ver\\u0073ion\":1"] {
        let bytes = format!("{{{duplicate},{}", &raw[1..]);
        assert!(decode(bytes.as_bytes()).is_err());
    }
    let mut extra = attest();
    extra["claimed_authenticated"] = json!("yes");
    assert!(!accepts(&extra));
    let mut nested = attest();
    nested["scope"] = json!({"scope":"original-scope"});
    assert!(!accepts(&nested));
    for kind in [
        "HELLO",
        "BOUND",
        "FENCE_ACK",
        "DISPOSED_ACK",
        "parent_attest",
        "",
    ] {
        let mut value = attest();
        value["kind"] = json!(kind);
        assert!(!accepts(&value));
    }
    let mut control = envelope().fields("FENCE");
    control["control_id"] = json!(nonce('2'));
    control["challenge"] = json!(nonce('0'));
    assert!(!accepts(&control));
}

#[test]
fn missing_fields_wrong_types_and_noncanonical_protocol_identifiers_are_refused() {
    let original = attest();
    for key in original.as_object().unwrap().keys() {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(!accepts(&missing), "{key}");
        let mut null = original.clone();
        null[key] = Value::Null;
        assert!(!accepts(&null), "{key}");
    }
    for bad in [
        json!(true),
        json!("1"),
        json!(1.0),
        json!(-1),
        json!(2),
        json!(256),
    ] {
        let mut value = original.clone();
        value["version"] = bad;
        assert!(!accepts(&value));
    }
    for field in [
        "channel",
        "invocation",
        "reservation",
        "mapping_sha256",
        "request_id",
        "broker_instance",
        "challenge",
        "evidence_id",
        "workspace",
    ] {
        for bad in [
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            "é".repeat(32),
        ] {
            let mut value = original.clone();
            value[field] = json!(bad);
            assert!(!accepts(&value), "{field}");
        }
    }
    for bad in [json!(0), json!(2), json!(1.0), json!(true), json!("1")] {
        let mut value = original.clone();
        value["evidence_revision"] = bad;
        assert!(!accepts(&value));
    }
}

#[test]
fn packet_text_and_number_bounds_are_closed_without_leaking_bytes() {
    for (field, limit) in [("issuer", 1024), ("subject", 256), ("origin", 2048)] {
        for bad in [
            String::new(),
            "x".repeat(limit + 1),
            "text\ncontrol".into(),
            "text\u{7f}".into(),
        ] {
            let mut value = attest();
            value[field] = json!(bad);
            assert!(!accepts(&value), "{field}");
        }
    }
    for bad in [
        String::new(),
        "s".repeat(42),
        "s".repeat(44),
        "+".repeat(43),
        "/".repeat(43),
        "=".repeat(43),
        " ".repeat(43),
        "\n".repeat(43),
        format!("{}é", "s".repeat(41)),
    ] {
        let mut value = attest();
        value["scope"] = json!(bad);
        assert!(!accepts(&value));
    }
    for valid in [
        "A".repeat(43),
        "0".repeat(43),
        "_".repeat(43),
        "-".repeat(43),
    ] {
        let mut value = attest();
        value["scope"] = json!(valid);
        assert!(accepts(&value));
    }
    for bad in [json!(0), json!(-1), json!(1.0), json!(true)] {
        let mut value = attest();
        value["expires_at_ms"] = bad;
        assert!(!accepts(&value));
    }
    let raw = serde_json::to_vec(&attest()).unwrap();
    let mut exact = raw.clone();
    exact.resize(MAX_PACKET, b' ');
    assert!(decode(&exact).is_ok());
    exact.push(b' ');
    assert!(decode(&exact).is_err());
    for malformed in [
        b"".as_slice(),
        b"[]",
        b"null",
        b"{} {}",
        b"\xff",
        b"{\"version\":NaN}",
        b"{\"version\":1e9999}",
    ] {
        assert!(decode(malformed).is_err());
    }
    assert_eq!(format!("{:?}", Refused), "Refused");
    // Time alone is not interpreted here. The private verified binding must
    // enforce actual currentness; a successfully parsed stale record is no GO.
    let mut stale = attest();
    stale["expires_at_ms"] = json!(1);
    assert!(accepts(&stale));
}

#[test]
fn native_output_records_match_protected_parent_exact_fields_and_correlation() {
    let e = envelope();
    let control = nonce('2');
    let h: Value =
        serde_json::from_slice(&hello(&e.invocation, &nonce('0'), &e.mapping_sha256).unwrap())
            .unwrap();
    assert_eq!(
        h,
        json!({"version":1,"kind":"HELLO","invocation":e.invocation,
        "challenge":nonce('0'),"mapping_sha256":e.mapping_sha256})
    );
    for (kind, bytes, disposition) in [
        ("BOUND", e.bound(&control).unwrap(), None),
        ("FENCE_ACK", e.fence_ack(&control).unwrap(), None),
        (
            "DISPOSED_ACK",
            e.disposed(&control, Disposition::Joined).unwrap(),
            Some("joined"),
        ),
        (
            "DISPOSED_ACK",
            e.disposed(&control, Disposition::Unconfirmed).unwrap(),
            Some("unconfirmed"),
        ),
    ] {
        let mut expected = e.fields(kind);
        expected["control_id"] = json!(control);
        if let Some(disposition) = disposition {
            expected["disposition"] = json!(disposition);
        }
        let actual: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(actual, expected);
        assert!(bytes.len() <= MAX_PACKET);
        assert!(
            decode(&bytes).is_err(),
            "native output is never a parent command"
        );
    }
}

#[test]
fn owner_correlation_preserves_request_identity_and_all_other_fields() {
    let e = envelope();
    let mut next = e.clone();
    next.request_id = nonce('f');
    assert!(e.same_owner(&next) && e != next);
    let mut changes = Vec::new();
    let mut c = e.clone();
    c.channel = nonce('f');
    changes.push(c);
    let mut c = e.clone();
    c.invocation = nonce('f');
    changes.push(c);
    let mut c = e.clone();
    c.reservation = nonce('f');
    changes.push(c);
    let mut c = e.clone();
    c.mapping_sha256 = nonce('f');
    changes.push(c);
    for c in changes {
        assert!(!e.same_owner(&c));
    }
    let fresh = Nonce::fresh();
    assert_eq!(fresh.as_str().len(), 64);
    assert!(Nonce::try_from(fresh.as_str().to_owned()).is_ok());
}
