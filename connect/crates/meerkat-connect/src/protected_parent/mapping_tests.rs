//! Pure syntax/descriptor controls. These do not authenticate a parent or prove
//! namespace isolation, catalog compatibility, provider access or native work.
use super::*;
use serde_json::json;

fn qualified(kind: &str, id: &str, domain: &str) -> Value {
    json!({"kind":kind,"id":id,"qualification":{"kind":"qualified","trust_domain_id":domain}})
}
fn fixture() -> Value {
    json!({
        "schema":"meerkat.connect.protected-service.v1", "protocol_version":7,
        "application":{"application_id":"forge","origin":"https://forge.invalid"},
        "source_selection":"workspace",
        "principal_mappings":[{
            "issuer":"https://accounts.google.com", "subject":"subject", "workspace":"a".repeat(64),
            "origin":"https://forge.invalid", "caller":qualified("human","caller","login-domain")
        }],
        "authentication_evidence_domain":{
            "authority":qualified("runtime_host","broker","root-owner"), "namespace":"auth"
        },
        "realm":{"realm_id":"private","state_root":"/srv/state","context_root":"/srv/context","user_config_root":"/srv/config"},
        "ingress":qualified("runtime_host","ingress","installed-ingress"),
        "installation":{"executable":"/opt/forge/bin/service","resource_root":"/opt/forge/resources","resource_manifest_sha256":"a".repeat(64)},
        "execution":{"uid":1001,"gid":1001,"supplementary_gids":[1001,1002]},
        "provider_store":{"kind":"file","root":"/srv/provider"}, "catalog_id":"forge-v1",
        "collections":[{
            "source_id":"repository","family":"official-github","profile":"enterprise-selected-reads","agent":"agent",
            "requirements":[{"id":"repo","program_id":"repo_read","resource":{"repository":"o/r"},"raw_operation_ceiling":["read"]}],
            "limits":{"calls":1,"argument_bytes":1024,"result_bytes":1024}
        }]
    })
}
fn native_default(xdg: Value) -> Value {
    json!({"kind":"native_default","home":"/srv/home","xdg_config_home":xdg,"root":"/srv/provider"})
}
fn parse(value: &Value) -> Result<ParsedMapping, MappingRefused> {
    ParsedMapping::parse(&serde_json::to_vec(value).expect("fixture JSON"))
}
fn set(value: &mut Value, pointer: &str, replacement: Value) {
    *value.pointer_mut(pointer).expect("fixture pointer") = replacement;
}
fn refuses_at(pointer: &str, value: Value) {
    let mut input = fixture();
    set(&mut input, pointer, value);
    assert!(parse(&input).is_err(), "accepted invalid {pointer}");
}
fn collection(index: usize, requirements: usize) -> Value {
    let mut result = fixture()["collections"][0].clone();
    result["source_id"] = json!(format!("source-{index}"));
    let requirement = result["requirements"][0].clone();
    result["requirements"] = Value::Array(
        (0..requirements)
            .map(|id| {
                let mut row = requirement.clone();
                row["id"] = json!(format!("requirement-{id}"));
                row
            })
            .collect(),
    );
    result
}

#[test]
fn protected_mapping_retains_exact_typed_descriptors_without_rewriting_input() {
    let mut input = fixture();
    input["principal_mappings"][0]["caller"] =
        qualified("human", " Caller ", "Case.Sensitive-Domain");
    input["collections"][0]["requirements"][0]["raw_operation_ceiling"] =
        json!(["z-read", "a-read"]);
    let mut bytes = b" \n".to_vec();
    bytes.extend(serde_json::to_vec_pretty(&input).unwrap());
    bytes.extend(b"\n ");
    let original = bytes.clone();
    let parsed = ParsedMapping::parse(&bytes).unwrap();
    assert_eq!(bytes, original);
    assert_eq!(parsed.application.application_id, "forge");
    assert_eq!(parsed.application.origin, "https://forge.invalid");
    assert_eq!(parsed.source_selection, "workspace");
    assert_eq!(parsed.catalog_id, "forge-v1");
    let row = &parsed.principal_mappings[0];
    assert_eq!(row.issuer, "https://accounts.google.com");
    assert_eq!(row.subject, "subject");
    assert_eq!(row.workspace, "a".repeat(64));
    assert_eq!(row.origin, parsed.application.origin);
    assert_eq!(
        row.caller,
        PrincipalRef::in_domain(
            PrincipalKind::Human,
            " Caller ",
            TrustDomainId::new("Case.Sensitive-Domain").unwrap()
        )
        .unwrap()
    );
    assert_ne!(row.caller, parsed.ingress);
    assert_eq!(parsed.authentication_evidence_domain.namespace, "auth");
    assert_eq!(
        parsed.authentication_evidence_domain.authority,
        PrincipalRef::in_domain(
            PrincipalKind::RuntimeHost,
            "broker",
            TrustDomainId::new("root-owner").unwrap()
        )
        .unwrap()
    );
    assert_eq!(parsed.realm.realm_id, RealmId::parse("private").unwrap());
    assert_eq!(parsed.realm.state_root, PathBuf::from("/srv/state"));
    assert_eq!(parsed.realm.context_root, PathBuf::from("/srv/context"));
    assert_eq!(parsed.realm.user_config_root, PathBuf::from("/srv/config"));
    assert_eq!(
        parsed.installation.executable,
        PathBuf::from("/opt/forge/bin/service")
    );
    assert_eq!(
        parsed.installation.resource_root,
        PathBuf::from("/opt/forge/resources")
    );
    assert_eq!(parsed.installation.resource_manifest_sha256, [0xaa; 32]);
    assert_eq!((parsed.execution.uid, parsed.execution.gid), (1001, 1001));
    assert_eq!(parsed.execution.supplementary_gids, vec![1001, 1002]);
    assert!(
        matches!(parsed.provider_store, ProviderStore::File { ref root } if root == &PathBuf::from("/srv/provider"))
    );
    let descriptor = &parsed.collections[0];
    assert_eq!(
        (
            &descriptor.source_id,
            &descriptor.family,
            &descriptor.profile,
            &descriptor.agent
        ),
        (
            &"repository".to_owned(),
            &"official-github".to_owned(),
            &"enterprise-selected-reads".to_owned(),
            &"agent".to_owned()
        )
    );
    let requirement = &descriptor.requirements[0];
    assert_eq!(requirement.id, "repo");
    assert_eq!(requirement.program_id, "repo_read");
    assert_eq!(requirement.resource, json!({"repository":"o/r"}));
    assert_eq!(requirement.raw_operation_ceiling, vec!["z-read", "a-read"]);
    assert_eq!(
        (
            descriptor.limits.calls,
            descriptor.limits.argument_bytes,
            descriptor.limits.result_bytes
        ),
        (1, 1024, 1024)
    );
}

#[test]
fn protected_mapping_closes_every_descriptor_and_requires_every_field() {
    let paths = [
        "",
        "/application",
        "/principal_mappings/0",
        "/principal_mappings/0/caller",
        "/principal_mappings/0/caller/qualification",
        "/authentication_evidence_domain",
        "/authentication_evidence_domain/authority",
        "/authentication_evidence_domain/authority/qualification",
        "/realm",
        "/ingress",
        "/ingress/qualification",
        "/installation",
        "/execution",
        "/provider_store",
        "/collections/0",
        "/collections/0/requirements/0",
        "/collections/0/limits",
    ];
    for pointer in paths {
        let baseline = fixture();
        let keys: Vec<String> = baseline
            .pointer(pointer)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut extra = baseline.clone();
        extra
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("future_field".into(), Value::Null);
        assert!(parse(&extra).is_err(), "unknown key at {pointer}");
        for key in keys {
            let mut missing = baseline.clone();
            missing
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(&key);
            assert!(parse(&missing).is_err(), "missing {pointer}/{key}");
        }
        let mut wrong_type = baseline;
        set(&mut wrong_type, pointer, Value::Array(vec![]));
        assert!(parse(&wrong_type).is_err(), "object type at {pointer}");
    }
    refuses_at("/schema", json!("meerkat.connect.protected-service.v2"));
    for version in [
        json!(6),
        json!(8),
        json!(7.0),
        json!("7"),
        json!(true),
        Value::Null,
    ] {
        refuses_at("/protocol_version", version);
    }
}

#[test]
fn protected_mapping_rejects_duplicate_keys_including_escaped_and_resource_keys() {
    let input = serde_json::to_string(&fixture()).unwrap();
    for (before, after) in [
        (
            "\"protocol_version\":7",
            "\"protocol_version\":7,\"protocol_version\":7",
        ),
        (
            "\"application_id\":\"forge\"",
            "\"application_id\":\"forge\",\"application_id\":\"other\"",
        ),
        (
            "\"repository\":\"o/r\"",
            "\"repository\":\"o/r\",\"repository\":\"other\"",
        ),
        (
            "\"repository\":\"o/r\"",
            "\"repository\":\"o/r\",\"\\u0072epository\":\"other\"",
        ),
        ("\"repository\":\"o/r\"", "\"nested\":[{\"x\":1,\"x\":2}]"),
    ] {
        assert_eq!(input.matches(before).count(), 1);
        let duplicate = input.replacen(before, after, 1);
        assert!(ParsedMapping::parse(duplicate.as_bytes()).is_err());
    }
}

#[test]
fn protected_mapping_requires_explicit_qualified_native_principals() {
    for pointer in [
        "/ingress",
        "/principal_mappings/0/caller",
        "/authentication_evidence_domain/authority",
    ] {
        for bad in [
            json!({"kind":"human","id":"x"}),
            json!({"kind":"human","id":"x","qualification":{"kind":"unqualified"}}),
            json!({"kind":"human","id":"x","qualification":{"kind":"qualified"}}),
            json!({"kind":"human","id":"x","qualification":{"kind":"qualified","trust_domain_id":" ","realm":"private"}}),
            json!({"kind":{"human":null},"id":"x","qualification":{"kind":"qualified","trust_domain_id":"domain"}}),
            qualified("unknown", "x", "domain"),
            qualified("human", " ", "domain"),
            qualified("human", "x", " "),
            qualified("human", "x\n", "domain"),
            qualified("human", "x", "domain\u{85}"),
            qualified("human", &"x".repeat(257), "domain"),
        ] {
            refuses_at(pointer, bad);
        }
    }
    for kind in [
        "human",
        "personal_agent",
        "shared_agent",
        "runtime_host",
        "service_account",
    ] {
        let mut input = fixture();
        input["principal_mappings"][0]["caller"] = qualified(kind, "x", "explicit-domain");
        assert!(parse(&input).is_ok());
    }
    let mut input = fixture();
    input["principal_mappings"][0]["caller"] = input["ingress"].clone();
    assert!(parse(&input).is_err());
    input = fixture();
    let mut conflicting = input["principal_mappings"][0].clone();
    conflicting["caller"] = qualified("human", "different", "domain");
    input["principal_mappings"]
        .as_array_mut()
        .unwrap()
        .push(conflicting);
    assert!(parse(&input).is_err());
}

#[test]
fn protected_mapping_binds_exact_login_origin_workspace_and_wire_bounds() {
    for (pointer, max) in [
        ("/principal_mappings/0/issuer", 1024),
        ("/principal_mappings/0/subject", 256),
        ("/source_selection", 256),
        ("/catalog_id", 256),
        ("/authentication_evidence_domain/namespace", 256),
    ] {
        let mut input = fixture();
        set(&mut input, pointer, json!("x".repeat(max)));
        assert!(parse(&input).is_ok(), "valid boundary {pointer}");
        refuses_at(pointer, json!("x".repeat(max + 1)));
        refuses_at(pointer, json!(""));
        refuses_at(pointer, json!("control\u{7f}"));
    }
    let mut input = fixture();
    input["application"]["origin"] = json!("x".repeat(2048));
    input["principal_mappings"][0]["origin"] = input["application"]["origin"].clone();
    assert!(parse(&input).is_ok());
    input["application"]["origin"] = json!("x".repeat(2049));
    input["principal_mappings"][0]["origin"] = input["application"]["origin"].clone();
    assert!(parse(&input).is_err());
    refuses_at(
        "/principal_mappings/0/origin",
        json!("https://other.invalid"),
    );
    for workspace in [
        "A".repeat(64),
        "g".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
        "*".to_owned(),
    ] {
        refuses_at("/principal_mappings/0/workspace", json!(workspace));
    }
    refuses_at(
        "/installation/resource_manifest_sha256",
        json!("A".repeat(64)),
    );
    refuses_at("/realm/realm_id", json!("realm/child"));
    // Compact valid rows reach the independent 256-row bound under 64 KiB.
    let mut compact = fixture();
    compact["application"]["origin"] = json!("a");
    compact["principal_mappings"] = Value::Array((0..256).map(|id| json!({
        "issuer":"i", "subject":id.to_string(), "workspace":"a".repeat(64), "origin":"a",
        "caller":qualified("human","c","d")
    })).collect());
    assert!(parse(&compact).is_ok());
    let mut extra = compact["principal_mappings"][0].clone();
    extra["subject"] = json!("256");
    compact["principal_mappings"]
        .as_array_mut()
        .unwrap()
        .push(extra);
    assert!(parse(&compact).is_err());
    refuses_at("/principal_mappings", json!([]));
}

#[test]
fn protected_mapping_requires_explicit_provider_environment_and_store_variant() {
    for xdg in [Value::Null, json!("/srv/xdg")] {
        let mut input = fixture();
        input["provider_store"] = native_default(xdg.clone());
        let parsed = parse(&input).unwrap();
        match parsed.provider_store {
            ProviderStore::NativeDefault {
                home,
                xdg_config_home,
                root,
            } => {
                assert_eq!(home, PathBuf::from("/srv/home"));
                assert_eq!(xdg_config_home, xdg.as_str().map(PathBuf::from));
                assert_eq!(root, PathBuf::from("/srv/provider"));
            }
            ProviderStore::File { .. } => panic!("wrong explicit variant"),
        }
        for key in ["home", "xdg_config_home", "root"] {
            let mut missing = input.clone();
            missing["provider_store"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(parse(&missing).is_err());
        }
        input["provider_store"]["future"] = Value::Null;
        assert!(parse(&input).is_err());
    }
    for bad in [
        Value::Null,
        json!({"kind":"native_default"}),
        json!({"kind":"file"}),
        json!({"kind":"file","root":null}),
        json!({"kind":"future","root":"/srv/provider"}),
        json!({"kind":"file","root":"/srv/provider","home":"/srv/home"}),
        json!({"kind":"native_default","home":null,"xdg_config_home":null,"root":"/srv/provider"}),
    ] {
        refuses_at("/provider_store", bad);
    }
}

#[test]
fn protected_mapping_paths_are_lexical_absolute_and_never_resolved() {
    let paths = [
        "/realm/state_root",
        "/realm/context_root",
        "/realm/user_config_root",
        "/installation/executable",
        "/installation/resource_root",
        "/provider_store/root",
    ];
    for pointer in paths {
        for invalid in [
            "",
            "relative",
            "//srv",
            "/srv/",
            "/srv//child",
            "/srv/./child",
            "/srv/../child",
            "/srv/\0child",
        ] {
            refuses_at(pointer, json!(invalid));
        }
        refuses_at(pointer, json!(format!("/{}", "x".repeat(4096))));
        for valid in [
            "/".to_owned(),
            format!("/{}", "x".repeat(4095)),
            "/does-not-need-to-exist/selected".to_owned(),
        ] {
            let mut input = fixture();
            set(&mut input, pointer, json!(valid));
            assert!(parse(&input).is_ok(), "lexical path {pointer}");
        }
    }
    for key in ["home", "xdg_config_home", "root"] {
        let mut input = fixture();
        input["provider_store"] = native_default(json!("/srv/xdg"));
        input["provider_store"][key] = json!("../ambient");
        assert!(parse(&input).is_err());
    }
}

#[test]
fn protected_mapping_execution_is_explicit_nonroot_and_sorted_bounded_groups() {
    for bad in [
        json!(0),
        json!(-1),
        json!(1.0),
        json!(4294967296u64),
        json!("1001"),
        json!(true),
        Value::Null,
    ] {
        refuses_at("/execution/uid", bad);
    }
    for bad in [
        json!(0),
        json!(-1),
        json!(1.0),
        json!(4294967296u64),
        json!("1001"),
        Value::Null,
    ] {
        refuses_at("/execution/gid", bad);
    }
    for groups in [
        json!([0]),
        json!([2, 1]),
        json!([1, 1]),
        json!([-1]),
        json!([1.0]),
        json!([4294967296u64]),
        Value::Null,
        json!((1..=65).collect::<Vec<_>>()),
    ] {
        refuses_at("/execution/supplementary_gids", groups);
    }
    for groups in [json!([]), json!((1..=64).collect::<Vec<_>>())] {
        let mut input = fixture();
        input["execution"]["supplementary_gids"] = groups;
        assert!(parse(&input).is_ok());
    }
}

#[test]
fn protected_mapping_preserves_native_collection_counts_and_uniform_tuple() {
    let mut input = fixture();
    input["collections"] = Value::Array((0..16).map(|id| collection(id, 1)).collect());
    assert!(parse(&input).is_ok());
    input["collections"]
        .as_array_mut()
        .unwrap()
        .push(collection(16, 1));
    assert!(parse(&input).is_err());
    refuses_at("/collections", json!([]));
    for field in ["source_id", "family", "profile", "agent"] {
        let mut input = fixture();
        input["collections"] = json!([collection(0, 1), collection(1, 1)]);
        input["collections"][1][field] = if field == "source_id" {
            json!("source-0")
        } else {
            json!("different")
        };
        assert!(parse(&input).is_err(), "source uniqueness/tuple {field}");
    }
    input = fixture();
    input["collections"] = json!([collection(0, 64)]);
    assert!(parse(&input).is_ok());
    input["collections"] = json!([collection(0, 65)]);
    assert!(parse(&input).is_err());
    input["collections"] = Value::Array((0..4).map(|id| collection(id, 64)).collect());
    assert!(parse(&input).is_ok()); // Requirement IDs may repeat in different sources.
    input["collections"]
        .as_array_mut()
        .unwrap()
        .push(collection(4, 1));
    assert!(parse(&input).is_err());
    input["collections"] = json!([collection(0, 2)]);
    input["collections"][0]["requirements"][1]["id"] = json!("requirement-0");
    assert!(parse(&input).is_err());
    refuses_at("/collections/0/requirements", json!([]));
}

#[test]
fn protected_mapping_preserves_operation_union_order_and_resource_budgets() {
    let raw = "/collections/0/requirements/0/raw_operation_ceiling";
    let mut input = fixture();
    set(
        &mut input,
        raw,
        json!((0..32).map(|i| format!("op-{i}")).collect::<Vec<_>>()),
    );
    assert!(parse(&input).is_ok());
    refuses_at(
        raw,
        json!((0..33).map(|i| format!("op-{i}")).collect::<Vec<_>>()),
    );
    for invalid in [
        json!([]),
        json!(["same", "same"]),
        json!([""]),
        json!(["x\n"]),
        json!(["x".repeat(257)]),
    ] {
        refuses_at(raw, invalid);
    }
    input["collections"] = json!([collection(0, 8)]);
    for i in 0..8 {
        input["collections"][0]["requirements"][i]["raw_operation_ceiling"] = json!(
            (0..32)
                .map(|j| format!("op-{}", i * 32 + j))
                .collect::<Vec<_>>()
        );
    }
    assert!(parse(&input).is_ok());
    let mut extra = input["collections"][0]["requirements"][0].clone();
    extra["id"] = json!("extra");
    extra["raw_operation_ceiling"] = json!(["op-256"]);
    input["collections"][0]["requirements"]
        .as_array_mut()
        .unwrap()
        .push(extra);
    assert!(parse(&input).is_err());
    input["collections"][0]["requirements"][8]["raw_operation_ceiling"] = json!(["op-0"]);
    assert!(parse(&input).is_ok()); // Shared raw operation does not enlarge the union.
    let resource = "/collections/0/requirements/0/resource";
    for value in [
        json!(true),
        json!(false),
        json!(0),
        json!([]),
        json!({}),
        json!("x".repeat(4094)),
    ] {
        let mut input = fixture();
        set(&mut input, resource, value);
        assert!(parse(&input).is_ok());
    }
    refuses_at(resource, Value::Null);
    refuses_at(resource, json!("x".repeat(4095)));
    for pointer in [
        "/collections/0/source_id",
        "/collections/0/requirements/0/id",
        "/collections/0/requirements/0/program_id",
    ] {
        refuses_at(pointer, json!(""));
        refuses_at(pointer, json!("x".repeat(257)));
    }
}

#[test]
fn protected_mapping_uses_exact_native_positive_limit_bounds() {
    for (key, max) in [
        ("calls", 256u64),
        ("argument_bytes", 8 * 1024 * 1024),
        ("result_bytes", 32 * 1024 * 1024),
    ] {
        let pointer = format!("/collections/0/limits/{key}");
        for value in [1, max] {
            let mut input = fixture();
            set(&mut input, &pointer, json!(value));
            assert!(parse(&input).is_ok());
        }
        for value in [
            json!(0),
            json!(max + 1),
            json!(-1),
            json!(1.0),
            json!("1"),
            Value::Null,
        ] {
            refuses_at(&pointer, value);
        }
    }
}

#[test]
fn protected_mapping_limits_bytes_depth_nodes_before_typed_conversion() {
    let mut bytes = serde_json::to_vec(&fixture()).unwrap();
    bytes.resize(MAX_BYTES, b' ');
    assert!(ParsedMapping::parse(&bytes).is_ok());
    bytes.push(b' ');
    assert!(ParsedMapping::parse(&bytes).is_err());
    let nested = |depth: usize| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
    assert!(bounded_value(nested(MAX_DEPTH).as_bytes()).is_ok());
    assert!(bounded_value(nested(MAX_DEPTH + 1).as_bytes()).is_err());
    let flat = |count: usize| format!("[{}]", vec!["0"; count].join(","));
    assert!(bounded_value(flat(MAX_NODES - 1).as_bytes()).is_ok());
    assert!(bounded_value(flat(MAX_NODES).as_bytes()).is_err());
    let mut input = fixture();
    input["collections"] = json!([collection(0, 9)]);
    for requirement in input["collections"][0]["requirements"]
        .as_array_mut()
        .unwrap()
    {
        requirement["resource"] = json!(vec![0; 2000]); // Each resource is within 4096 bytes.
    }
    assert!(serde_json::to_vec(&input).unwrap().len() < MAX_BYTES);
    assert!(parse(&input).is_err()); // Combined tree exceeds the independent node bound.
}

#[test]
fn protected_mapping_rejects_invalid_json_and_preserves_native_resource_number_semantics() {
    for invalid in [
        b"".as_slice(),
        b"{} {}",
        b"{\"x\":NaN}",
        b"{\"x\":Infinity}",
        b"{\"x\":\"\\uD800\"}",
        b"\xff",
    ] {
        assert!(ParsedMapping::parse(invalid).is_err());
    }
    let input = serde_json::to_string(&fixture()).unwrap();
    for number in ["18446744073709551616", "-9223372036854775809", "1.25e12"] {
        let bytes = input.replacen("{\"repository\":\"o/r\"}", number, 1);
        assert_ne!(bytes, input);
        let parsed = ParsedMapping::parse(bytes.as_bytes()).unwrap();
        assert_eq!(
            parsed.collections[0].requirements[0].resource,
            serde_json::from_str::<Value>(number).unwrap()
        );
    }
}

fn multiple_fixture() -> Value {
    let mut value = fixture();
    let first = value
        .as_object_mut()
        .unwrap()
        .remove("collections")
        .unwrap();
    let mut second = first.clone();
    second[0]["source_id"] = json!("selected-python-source");
    second[0]["family"] = json!("selected-python");
    second[0]["profile"] = json!("python");
    value["schema"] = json!("meerkat.connect.protected-service.v2");
    value["services"] = json!([{"service_id":"official","collections":first},{"service_id":"python","collections":second}]);
    value
}
#[test]
fn explicit_v2_mapping_preserves_distinct_services_and_v1_stays_single_service() {
    let value = multiple_fixture();
    let parsed = parse(&value).unwrap();
    assert_eq!(
        parsed.service_groups,
        vec![("official".into(), 0..1), ("python".into(), 1..2)]
    );
    assert_eq!(parsed.collections[0].service_id, "official");
    assert_eq!(parsed.collections[1].service_id, "python");
    assert_eq!(parsed.collections[1].family, "selected-python");
    let single = parse(&fixture()).unwrap();
    assert_eq!(single.service_groups, vec![("default".into(), 0..1)]);
    let mut old = value.clone();
    old["schema"] = json!("meerkat.connect.protected-service.v1");
    assert!(parse(&old).is_err());
    let mut both = value.clone();
    both["collections"] = fixture()["collections"].clone();
    assert!(parse(&both).is_err());
}
#[test]
fn multiple_mapping_refuses_cross_service_duplicates_and_limits() {
    for (pointer, value) in [
        ("/services/1/service_id", json!("official")),
        ("/services/1/collections/0/source_id", json!("repository")),
        ("/services/1/collections", json!([])),
    ] {
        let mut bad = multiple_fixture();
        *bad.pointer_mut(pointer).unwrap() = value;
        assert!(parse(&bad).is_err());
    }
    let mut extra = multiple_fixture();
    extra["services"][1]["caller"] = json!({});
    assert!(parse(&extra).is_err());
    let mut mixed = multiple_fixture();
    let row = mixed["services"][0]["collections"][0].clone();
    mixed["services"][1]["collections"]
        .as_array_mut()
        .unwrap()
        .push(row);
    assert!(parse(&mixed).is_err());
    let mut large = multiple_fixture();
    large["services"][0]["collections"] = Value::Array((0..9).map(|i| collection(i, 1)).collect());
    large["services"][1]["collections"] = Value::Array((9..17).map(|i| collection(i, 1)).collect());
    assert!(parse(&large).is_err());
}

#[test]
fn mapping_v3_binds_one_selected_input_and_preserves_legacy_absence() {
    let mut value = fixture();
    assert!(parse(&value).unwrap().selected_input.is_none());
    let collections = value
        .as_object_mut()
        .unwrap()
        .remove("collections")
        .unwrap();
    value["schema"] = json!("meerkat.connect.protected-service.v2");
    value["services"] = json!([{"service_id":"repository","collections":collections}]);
    assert!(parse(&value).unwrap().selected_input.is_none());
    value["selected_input"] = json!({"service_id":"repository","bundle_sha256":"a".repeat(64)});
    assert!(parse(&value).is_err());
    value["schema"] = json!("meerkat.connect.protected-service.v3");
    let parsed = parse(&value).unwrap();
    let catalog = super::super::CatalogSelection(&parsed);
    let input = catalog.selected_input().unwrap();
    assert_eq!(input.service_id(), "repository");
    assert_eq!(input.bundle_sha256(), "a".repeat(64));
    for replacement in [
        json!(null),
        json!({"service_id":"other","bundle_sha256":"a".repeat(64)}),
        json!({"service_id":"repository","bundle_sha256":"A".repeat(64)}),
        json!({"service_id":"repository","bundle_sha256":"a".repeat(64),"path":"/tmp/input"}),
    ] {
        let mut bad = value.clone();
        bad["selected_input"] = replacement;
        assert!(parse(&bad).is_err());
    }
    value.as_object_mut().unwrap().remove("selected_input");
    assert!(parse(&value).is_err());
}
