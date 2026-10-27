use serde_json::{Value, json};
use toolkit_package::*;

const FIXTURE: &str = r###"{"configuration":"{\"purpose\":\"synthetic-local-example\"}\n","files":{"db-password":"synthetic-example-only\n"},"manifest_json":"{\"components\":[{\"activation_scope\":{\"kind\":\"independent\"},\"dependencies\":[],\"host_services\":[],\"id\":\"database\",\"lifetime\":\"deployment\",\"placement\":\"local-compose\",\"readiness\":{\"checks\":[\"running\"],\"owner\":\"host\"},\"required_contracts\":[],\"rollback\":{\"kind\":\"owner_reconciliation\",\"operation\":\"reconcile\",\"owner\":\"host\"},\"runtime\":\"container\",\"shutdown\":{\"maximum_wait_ms\":30000,\"operation\":\"stop\",\"owner\":\"host\",\"timeout\":\"preserve_uncertainty\"},\"source\":{\"content_digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"dependencies\":{\"kind\":\"self_contained\"},\"entrypoint\":{\"arguments\":[],\"executable\":\"usr/local/bin/database\"},\"kind\":\"artifact\",\"location\":\"https://registry.example/v2/example/server/manifests/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"state\":[{\"format\":\"1\",\"migrations\":[],\"namespace\":\"data\",\"owner\":\"database-owner\"},{\"format\":\"1\",\"migrations\":[],\"namespace\":\"socket\",\"owner\":\"database-owner\"}],\"version\":\"1\"},{\"activation_scope\":{\"kind\":\"independent\"},\"dependencies\":[{\"component\":\"database\",\"selected_version\":\"1\"}],\"host_services\":[],\"id\":\"api\",\"lifetime\":\"deployment\",\"placement\":\"local-compose\",\"readiness\":{\"checks\":[\"running\"],\"owner\":\"host\"},\"required_contracts\":[],\"rollback\":{\"kind\":\"owner_reconciliation\",\"operation\":\"reconcile\",\"owner\":\"host\"},\"runtime\":\"container\",\"shutdown\":{\"maximum_wait_ms\":30000,\"operation\":\"stop\",\"owner\":\"host\",\"timeout\":\"preserve_uncertainty\"},\"source\":{\"content_digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"dependencies\":{\"kind\":\"self_contained\"},\"entrypoint\":{\"arguments\":[],\"executable\":\"usr/local/bin/api\"},\"kind\":\"artifact\",\"location\":\"https://registry.example/v2/example/server/manifests/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"state\":[],\"version\":\"1\"},{\"activation_scope\":{\"kind\":\"independent\"},\"dependencies\":[],\"host_services\":[],\"id\":\"independent\",\"lifetime\":\"deployment\",\"placement\":\"local-compose\",\"readiness\":{\"checks\":[\"running\"],\"owner\":\"host\"},\"required_contracts\":[],\"rollback\":{\"kind\":\"owner_reconciliation\",\"operation\":\"reconcile\",\"owner\":\"host\"},\"runtime\":\"container\",\"shutdown\":{\"maximum_wait_ms\":30000,\"operation\":\"stop\",\"owner\":\"host\",\"timeout\":\"preserve_uncertainty\"},\"source\":{\"content_digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"dependencies\":{\"kind\":\"self_contained\"},\"entrypoint\":{\"arguments\":[],\"executable\":\"usr/local/bin/independent\"},\"kind\":\"artifact\",\"location\":\"https://registry.example/v2/example/server/manifests/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"state\":[],\"version\":\"1\"},{\"activation_scope\":{\"kind\":\"independent\"},\"dependencies\":[{\"component\":\"database\",\"selected_version\":\"1\"}],\"host_services\":[],\"id\":\"mcp\",\"lifetime\":\"request\",\"placement\":\"local-compose\",\"readiness\":{\"checks\":[\"running\"],\"owner\":\"host\"},\"required_contracts\":[],\"rollback\":{\"kind\":\"owner_reconciliation\",\"operation\":\"reconcile\",\"owner\":\"host\"},\"runtime\":\"container\",\"shutdown\":{\"maximum_wait_ms\":30000,\"operation\":\"stop\",\"owner\":\"host\",\"timeout\":\"preserve_uncertainty\"},\"source\":{\"content_digest\":\"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\",\"dependencies\":{\"kind\":\"self_contained\"},\"entrypoint\":{\"arguments\":[],\"executable\":\"usr/local/bin/mcp\"},\"kind\":\"artifact\",\"location\":\"docker-image:///sha256/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\"},\"state\":[],\"version\":\"1\"}],\"configuration\":{\"content_digest\":\"cdb3b10ee0093d9e2a1feb6f8e6f044b3a958fe3dbba9b742236a605d08f8259\",\"owner\":\"host\",\"reference\":\"config\",\"revision\":\"1\"},\"exports\":[{\"authentication\":{\"kind\":\"none\"},\"cleanup\":\"release\",\"components\":[\"api\",\"independent\",\"mcp\"],\"diagnostics\":\"inspect\",\"enforcement_contract\":{\"name\":\"host/enforcement\",\"version\":{\"major\":1,\"minimum_minor\":0}},\"events\":[],\"id\":\"service\",\"operations\":[{\"declared_effect\":\"read\",\"id\":\"read\",\"required_authority\":[],\"resources\":{\"kind\":\"connection_wide\"},\"result_limits\":{\"maximum_bytes\":1024,\"maximum_items\":10},\"schema_digest\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"}],\"role\":\"service_connector\"}],\"host_services\":[],\"package\":{\"name\":\"compose-example\",\"publisher\":\"test\",\"version\":\"1.0.0\"},\"package_content_digest\":\"1f3178638b88246d45c9aad39819fc3eae2a12cc8102e0f821a814dfee0086a7\",\"policy\":{\"content_digest\":\"f0938ed52d4fab2561a16341a1fc379b6e2ba58d35289dabbe7bd30d0d913b41\",\"owner\":\"host\",\"revision\":\"1\"},\"portable_mcp_entries\":[],\"required_contracts\":[],\"schema_version\":1,\"strategy\":\"container\"}","package_json":"{\"assessments\":[],\"description\":\"Illustrative local Compose graph; all tool and image pins must be replaced with host-observed identities.\",\"display_name\":\"Compose graph example\",\"identity\":{\"name\":\"compose-example\",\"publisher\":\"test\",\"version\":\"1.0.0\"},\"known_exports\":[],\"provenance\":{\"license\":\"MIT\",\"local_patches\":[],\"notices\":[],\"source\":\"https://example.org/source\",\"source_revision\":\"commit1\"},\"schema_version\":1,\"strategies\":[{\"id\":\"container\",\"instructions\":\"Use the pinned host profile.\",\"kind\":\"native_adapter\",\"known_requirements\":[],\"sources\":[],\"unresolved_details\":[]}]}","policy":"{\"admission\":\"host-owned\"}\n","profile_json":"{\"components\":{\"api\":{\"environment\":{},\"healthcheck\":null,\"image_id\":\"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\",\"mounts\":[],\"platform\":\"linux/arm64\",\"resources\":{\"cpu_millis\":500,\"memory_bytes\":134217728,\"pids_limit\":128},\"service\":\"api\",\"user\":\"0:0\"},\"database\":{\"environment\":{\"POSTGRES_PASSWORD_FILE\":\"/run/config/db-password\"},\"healthcheck\":null,\"image_id\":\"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\",\"mounts\":[{\"kind\":\"file\",\"source\":\"files/db-password\",\"target\":\"/run/config/db-password\"},{\"kind\":\"volume\",\"read_only\":false,\"source\":\"data\",\"target\":\"/var/lib/postgresql/data\"},{\"kind\":\"volume\",\"read_only\":false,\"source\":\"socket\",\"target\":\"/var/run/postgresql\"}],\"platform\":\"linux/arm64\",\"resources\":{\"cpu_millis\":500,\"memory_bytes\":134217728,\"pids_limit\":128},\"service\":\"database\",\"user\":\"0:0\"},\"independent\":{\"environment\":{},\"healthcheck\":null,\"image_id\":\"sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\",\"mounts\":[],\"platform\":\"linux/arm64\",\"resources\":{\"cpu_millis\":500,\"memory_bytes\":134217728,\"pids_limit\":128},\"service\":\"independent\",\"user\":\"0:0\"},\"mcp\":{\"environment\":{},\"healthcheck\":null,\"image_id\":\"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\",\"mounts\":[{\"kind\":\"file\",\"source\":\"configuration\",\"target\":\"/run/config/configuration\"},{\"kind\":\"file\",\"source\":\"policy\",\"target\":\"/run/config/policy\"},{\"kind\":\"volume\",\"read_only\":true,\"source\":\"socket\",\"target\":\"/var/run/postgresql\"}],\"platform\":\"linux/arm64\",\"resources\":{\"cpu_millis\":500,\"memory_bytes\":134217728,\"pids_limit\":128},\"service\":\"mcp\",\"user\":\"0:0\"}},\"engine\":{\"compose\":{\"path\":\"/usr/local/lib/docker/cli-plugins/docker-compose\",\"sha256\":\"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\"},\"docker\":{\"path\":\"/usr/local/bin/docker\",\"sha256\":\"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\"},\"endpoint\":\"unix:///tmp/example-docker.sock\",\"id\":\"replace-with-observed-engine-id\"},\"files\":{\"db-password\":\"fd3ee8f991a84bf711d76fd7b341132b61432df9b76a3b854bf3ab3048e1b9c8\"},\"host\":{\"contracts\":{\"host/enforcement\":{\"major\":1,\"minor\":0}},\"placements\":[\"local-compose\"],\"runtimes\":[\"container\"],\"services\":{}},\"owner\":\"host\",\"placement\":\"local-compose\",\"project\":\"example-compose\",\"schema_version\":1,\"volumes\":{\"data\":{\"name\":\"example-compose-data\"},\"socket\":{\"name\":\"example-compose-socket\"}}}","profile_sha256":"614bdd5bc10bd79a21320d2a70ecea96ee522ba9f3ad3aaf47270d839d88c3cf","schema_version":1}"###;

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}
fn inner(request: &Value, field: &str) -> Value {
    serde_json::from_str(request[field].as_str().unwrap()).unwrap()
}
fn set_inner(request: &mut Value, field: &str, value: &Value) {
    let bytes = canonical_json(value).unwrap();
    if field == "profile_json" {
        request["profile_sha256"] = json!(ContentDigest::of_bytes(&bytes));
    }
    request[field] = json!(String::from_utf8(bytes).unwrap());
}
fn prepare(request: &Value) -> Result<PreparedCompose, ComposePreparationError> {
    prepare_compose_json(&canonical_json(request).unwrap())
}
fn key(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn successor(
    origin: &Value,
    old: &Value,
    new: &Value,
) -> Result<PreparedComposeSuccessor, ComposePreparationError> {
    let request = json!({"schema_version": 1,
        "origin_request": String::from_utf8(canonical_json(origin).unwrap()).unwrap(),
        "predecessor_request": String::from_utf8(canonical_json(old).unwrap()).unwrap(),
        "successor_request": String::from_utf8(canonical_json(new).unwrap()).unwrap()});
    prepare_compose_successor_json(&canonical_json(&request).unwrap())
}

fn new_version(request: &Value, version: &str) -> Value {
    let mut next = request.clone();
    let mut package = inner(request, "package_json");
    package["identity"]["version"] = json!(version);
    let mut manifest = inner(request, "manifest_json");
    manifest["package"] = package["identity"].clone();
    manifest["package_content_digest"] =
        json!(ContentDigest::of_bytes(&canonical_json(&package).unwrap()));
    set_inner(&mut next, "package_json", &package);
    set_inner(&mut next, "manifest_json", &manifest);
    next
}

fn migration_to(request: &Value, from: &str, to: &str, version: &str) -> Value {
    let mut next = new_version(request, version);
    let mut manifest = inner(&next, "manifest_json");
    let state = &mut component(&mut manifest, "database")["state"][0];
    state["format"] = json!(to);
    state["migrations"] = json!([{"from":from,"operation":"convert-data",
        "artifact_digest":"a".repeat(64),"reversible":false}]);
    let mut profile = inner(&next, "profile_json");
    profile["state_migration"] = json!({"protocol_version":1,"namespace":"data",
        "operation":"convert-data","apply":{"executable":"usr/local/bin/migrate","arguments":["apply"]},
        "readback":{"executable":"usr/local/bin/migrate","arguments":["readback"]},"timeout_ms":30000});
    set_inner(&mut next, "manifest_json", &manifest);
    set_inner(&mut next, "profile_json", &profile);
    next
}

fn history(requests: &[&Value]) -> Result<PreparedComposeSuccessor, ComposePreparationError> {
    let requests: Vec<_> = requests
        .iter()
        .map(|request| String::from_utf8(canonical_json(request).unwrap()).unwrap())
        .collect();
    prepare_compose_successor_json(
        &canonical_json(&json!({"schema_version":2,"requests":requests})).unwrap(),
    )
}

fn without_database(request: &Value) -> Value {
    let mut next = request.clone();
    let mut manifest = inner(request, "manifest_json");
    manifest["components"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["id"] == "independent");
    for export in manifest["exports"].as_array_mut().unwrap() {
        export["components"] = json!(["independent"]);
    }
    let mut profile = inner(request, "profile_json");
    profile["components"] = json!({"independent": profile["components"]["independent"]});
    profile["volumes"] = json!({});
    profile["files"] = json!({});
    profile.as_object_mut().unwrap().remove("state_migration");
    next["files"] = json!({});
    set_inner(&mut next, "manifest_json", &manifest);
    set_inner(&mut next, "profile_json", &profile);
    next
}

#[test]
fn migration_is_pinned_metadata_and_successor_protocol_fences_old_writers() {
    let a = fixture();
    let b = migration_to(&a, "1", "2", "2.0.0");
    let prepared = prepare(&b).unwrap();
    // Fresh preparation still describes ordinary B services. Only the owner
    // successor protocol can admit execution of this separate capability.
    assert_eq!(prepared.schema_version, 1);
    assert_eq!(
        prepared.plan.components.len(),
        prepare(&a).unwrap().plan.components.len()
    );
    assert_eq!(
        prepared.plan.components[&key("database")].entrypoint,
        ["usr/local/bin/database"]
    );
    let metadata = serde_json::to_value(prepared.plan.state_migration.as_ref().unwrap()).unwrap();
    let mut expected = inner(&b, "profile_json")["state_migration"].clone();
    expected["component"] = json!("database");
    expected["owner"] = json!("database-owner");
    expected["from_format"] = json!("1");
    expected["to_format"] = json!("2");
    expected["artifact_digest"] = json!("a".repeat(64));
    assert_eq!(metadata, expected);
    let first = successor(&a, &a, &b).unwrap();
    assert_eq!(first.schema_version, 2);
    assert_eq!(
        first.predecessor.plan_sha256,
        prepare(&a).unwrap().plan_sha256
    );
    let c = new_version(&b, "3.0.0");
    let next = successor(&a, &b, &c).unwrap();
    assert_eq!(next.schema_version, 2); // Historical migration also fences old writers.
    assert_eq!(next.predecessor.plan_sha256, first.successor.plan_sha256);
    assert_eq!(
        history(&[&a, &b, &c]).unwrap().successor.plan_sha256,
        next.successor.plan_sha256
    );
    assert!(successor(&a, &b, &a).is_err()); // No implicit rollback to original bytes.
}

#[test]
fn legacy_preparation_and_v1_successor_bytes_match_the_retained_prior_planner() {
    // Captured once from canonical prior planner f9ebcbc1 before its replacement.
    let a = fixture();
    let b = new_version(&a, "2.0.0");
    let c = new_version(&b, "3.0.0");
    for (bytes, expected) in [
        (
            canonical_json(&prepare(&a).unwrap()).unwrap(),
            "b56a861ccae3b9a6ea77bc5ecf8afa134b99f47c7350fb16c443e61f9add6f58",
        ),
        (
            canonical_json(&successor(&a, &a, &b).unwrap()).unwrap(),
            "541d8f26551ce84e4abbfe7882d69876556bac031c8a22ac59118879b8281444",
        ),
        (
            canonical_json(&successor(&a, &b, &c).unwrap()).unwrap(),
            "1fe075199ec3a9db052a3112ff9bd4b15bbd1d5e8d7d9e76e1f53a69f0959a4e",
        ),
    ] {
        assert_eq!(ContentDigest::of_bytes(&bytes).as_str(), expected);
    }
}

#[test]
fn migration_profile_refuses_unbound_code_and_unsupported_state_changes() {
    let a = fixture();
    let b = migration_to(&a, "1", "2", "2.0.0");
    for (path, value) in [
        ("/state_migration/protocol_version", json!(2)),
        ("/state_migration/namespace", json!("socket")),
        ("/state_migration/operation", json!("other")),
        ("/state_migration/timeout_ms", json!(999)),
        ("/state_migration/timeout_ms", json!(120001)),
        ("/state_migration/apply/executable", json!("../bin/migrate")),
        (
            "/state_migration/readback/executable",
            json!("usr/local/bin"),
        ),
        ("/state_migration/apply/arguments", json!(["${AMBIENT}"])),
    ] {
        let mut changed = b.clone();
        let mut profile = inner(&changed, "profile_json");
        *profile.pointer_mut(path).unwrap() = value;
        // This path shadows a pinned file mount and is invalid for either worker.
        if path == "/state_migration/readback/executable" {
            profile["state_migration"]["readback"]["executable"] =
                json!("run/config/db-password/readback");
        }
        set_inner(&mut changed, "profile_json", &profile);
        assert!(prepare(&changed).is_err(), "{path}");
    }
    for worker in ["apply", "readback"] {
        let mut changed = b.clone();
        let mut profile = inner(&changed, "profile_json");
        profile["state_migration"][worker]["python_console_script"] = json!("serve-it");
        set_inner(&mut changed, "profile_json", &profile);
        assert!(prepare(&changed).is_err());
    }
    for (field, value) in [
        ("artifact_digest", json!("b".repeat(64))),
        ("operation", json!("other")),
        ("from", json!("0")),
    ] {
        let mut changed = b.clone();
        let mut manifest = inner(&changed, "manifest_json");
        component(&mut manifest, "database")["state"][0]["migrations"][0][field] = value;
        set_inner(&mut changed, "manifest_json", &manifest);
        assert!(successor(&a, &a, &changed).is_err(), "{field}");
    }
    let mut changed = b.clone();
    let mut manifest = inner(&changed, "manifest_json");
    component(&mut manifest, "database")["state"][1]["format"] = json!("2");
    component(&mut manifest, "database")["state"][1]["migrations"] =
        component(&mut manifest, "database")["state"][0]["migrations"].clone();
    set_inner(&mut changed, "manifest_json", &manifest);
    assert!(successor(&a, &a, &changed).is_err()); // Only one migrated namespace.
    for field in ["owner", "namespace"] {
        let mut changed = b.clone();
        let mut manifest = inner(&changed, "manifest_json");
        component(&mut manifest, "database")["state"][0][field] = json!("other");
        set_inner(&mut changed, "manifest_json", &manifest);
        assert!(successor(&a, &a, &changed).is_err(), "{field}");
    }
    let mut changed = b;
    let mut profile = inner(&changed, "profile_json");
    profile["volumes"]["data"]["name"] = json!("example-compose-other");
    set_inner(&mut changed, "profile_json", &profile);
    assert!(prepare(&changed).is_ok());
    assert!(successor(&a, &a, &changed).is_err());
}

#[test]
fn history_keeps_migrated_dormant_format_and_original_physical_provenance() {
    let a = fixture();
    let b = migration_to(&a, "1", "2", "2.0.0");
    let c = without_database(&new_version(&b, "3.0.0"));
    let d = new_version(&c, "4.0.0");
    let ab = history(&[&a, &b]).unwrap();
    let abc = history(&[&a, &b, &c]).unwrap();
    let abcd = history(&[&a, &b, &c, &d]).unwrap();
    assert_eq!(abc.predecessor.plan_sha256, ab.successor.plan_sha256);
    assert_eq!(abcd.predecessor.plan_sha256, abc.successor.plan_sha256);
    let original = prepare(&a).unwrap();
    for prepared in [&abc.successor, &abcd.successor] {
        assert!(prepared.plan.state_migration.is_none());
        assert_eq!(prepared.plan.volumes[&key("data")].format.as_str(), "2");
        assert_eq!(prepared.plan.volumes[&key("socket")].format.as_str(), "1");
        assert_eq!(
            prepared.plan.retained_volume_labels,
            ab.successor.plan.retained_volume_labels
        );
        let document: Value = serde_json::from_str(&prepared.plan.compose_json).unwrap();
        for (namespace, prior) in &original.plan.volumes {
            let retained = &prepared.plan.volumes[namespace];
            assert_eq!(
                (&retained.name, &retained.owner, &retained.component),
                (&prior.name, &prior.owner, &prior.component)
            );
            assert_eq!(
                document["volumes"][&prior.name],
                json!({"name":prior.name,"external":true})
            );
        }
    }
    assert!(history(&[&a, &b, &c, &new_version(&b, "5.0.0")]).is_err());
    assert!(history(&[&a, &b, &c, &without_state(&new_version(&a, "5.0.0"))]).is_err());
}

#[test]
fn history_validates_immediate_formats_and_rejects_skipped_or_reordered_edges() {
    let a = fixture();
    let b = migration_to(&a, "1", "2", "2.0.0");
    let c = migration_to(&b, "2", "3", "3.0.0");
    let next = history(&[&a, &b, &c]).unwrap();
    assert_eq!(
        next.predecessor.plan.volumes[&key("data")].format.as_str(),
        "2"
    );
    assert_eq!(
        next.successor.plan.volumes[&key("data")].format.as_str(),
        "3"
    );
    assert!(history(&[&a, &c]).is_err());
    assert!(history(&[&a, &c, &b]).is_err());
    let mut stale = c.clone();
    let mut manifest = inner(&stale, "manifest_json");
    component(&mut manifest, "database")["state"][0]["migrations"][0]["from"] = json!("1");
    set_inner(&mut stale, "manifest_json", &manifest);
    assert!(history(&[&a, &b, &stale]).is_err());
}

#[test]
fn history_envelope_is_closed_bounded_and_never_accepts_repeated_selections() {
    let a = fixture();
    let b = new_version(&a, "2.0.0");
    let raw = |value: &Value| String::from_utf8(canonical_json(value).unwrap()).unwrap();
    for value in [
        json!({"schema_version":2,"requests":[]}),
        json!({"schema_version":2,"requests":[raw(&a)]}),
        json!({"schema_version":2,"requests":[raw(&a),raw(&a)]}),
        json!({"schema_version":2,"requests":[raw(&a),raw(&b)],"receipt":"untrusted"}),
        json!({"schema_version":1,"requests":[raw(&a),raw(&b)]}),
        json!({"schema_version":3,"requests":[raw(&a),raw(&b)]}),
        json!({"schema_version":2,"requests":[raw(&a),{}]}),
    ] {
        assert_eq!(
            prepare_compose_successor_json(&canonical_json(&value).unwrap()).unwrap_err(),
            ComposePreparationError::InvalidInput
        );
    }
    let requests: Vec<_> = (0..33)
        .map(|index| raw(&new_version(&a, &format!("{index}.0.0"))))
        .collect();
    assert_eq!(
        prepare_compose_successor_json(
            &canonical_json(&json!({"schema_version":2,"requests":requests})).unwrap()
        )
        .unwrap_err(),
        ComposePreparationError::InputLimit
    );
    assert!(
        prepare_compose_successor_json(br#"{"schema_version":2,"schema_version":2,"requests":[]}"#)
            .is_err()
    );
}

fn without_state(request: &Value) -> Value {
    let mut next = request.clone();
    let mut manifest = inner(request, "manifest_json");
    component(&mut manifest, "database")["state"] = json!([]);
    let mut profile = inner(request, "profile_json");
    profile["volumes"] = json!({});
    for selected in profile["components"].as_object_mut().unwrap().values_mut() {
        selected["mounts"]
            .as_array_mut()
            .unwrap()
            .retain(|mount| mount["kind"] != "volume");
    }
    set_inner(&mut next, "manifest_json", &manifest);
    set_inner(&mut next, "profile_json", &profile);
    next
}

#[test]
fn successor_retains_removed_component_state_through_later_selections() {
    let a = fixture();
    let b = without_database(&new_version(&a, "2.0.0"));
    let c = new_version(&b, "3.0.0");
    assert!(prepare(&b).unwrap().plan.volumes.is_empty());
    let original = prepare(&a).unwrap();
    let first = successor(&a, &a, &b).unwrap();
    let next = successor(&a, &b, &c).unwrap();
    assert_eq!(next.predecessor.plan_sha256, first.successor.plan_sha256);
    let creation: Value = serde_json::from_str(&original.plan.compose_json).unwrap();
    for prepared in [&first.successor, &next.predecessor, &next.successor] {
        assert_eq!(prepared.plan.volumes, original.plan.volumes);
        assert_eq!(
            prepared.plan.components.keys().collect::<Vec<_>>(),
            [&key("independent")]
        );
        assert_eq!(prepared.plan.deployment_order, [key("independent")]);
        assert_eq!(prepared.plan.shutdown_order, [key("independent")]);
        let document: Value = serde_json::from_str(&prepared.plan.compose_json).unwrap();
        assert_eq!(document["services"].as_object().unwrap().len(), 1);
        for (namespace, volume) in &original.plan.volumes {
            assert_eq!(
                document["volumes"][&volume.name],
                json!({"name":volume.name,"external":true})
            );
            assert_eq!(
                json!(prepared.plan.retained_volume_labels[namespace]),
                creation["volumes"][&volume.name]["labels"]
            );
            assert!(
                document["services"]["independent"]["volumes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|mount| mount["type"] != "volume")
            );
        }
    }
}

#[test]
fn successor_refuses_state_shedding_and_removed_namespace_reintroduction() {
    let a = fixture();
    let b = without_database(&new_version(&a, "2.0.0"));
    let shed = without_state(&new_version(&a, "2.0.0"));
    assert!(prepare(&shed).is_ok());
    assert_eq!(
        successor(&a, &a, &shed).unwrap_err(),
        ComposePreparationError::UnsupportedSelection
    );
    let reintroduced = new_version(&a, "3.0.0");
    assert!(prepare(&reintroduced).is_ok());
    assert_eq!(
        successor(&a, &b, &reintroduced).unwrap_err(),
        ComposePreparationError::UnsupportedSelection
    );
    // Raw B->C has no state change here, but the original owner component
    // cannot become active again while its namespace remains dormant.
    let reused_component = without_state(&reintroduced);
    assert!(prepare(&reused_component).is_ok());
    assert_eq!(
        successor(&a, &b, &reused_component).unwrap_err(),
        ComposePreparationError::UnsupportedSelection
    );
}

#[test]
fn successor_keeps_origin_provenance_and_allows_package_version_change() {
    let a = fixture();
    let b = new_version(&a, "2.0.0");
    let c = new_version(&b, "3.0.0");
    let first = successor(&a, &a, &b).unwrap();
    assert_eq!(
        first.predecessor.plan_sha256,
        prepare(&a).unwrap().plan_sha256
    );
    assert_ne!(first.successor.plan_sha256, first.predecessor.plan_sha256);
    let next = successor(&a, &b, &c).unwrap();
    assert_eq!(next.predecessor.plan_sha256, first.successor.plan_sha256);
    assert_eq!(
        next.successor.plan.retained_volume_labels,
        first.successor.plan.retained_volume_labels
    );
    let document: Value = serde_json::from_str(&next.successor.plan.compose_json).unwrap();
    assert_eq!(
        document["volumes"]["example-compose-data"],
        json!({"name":"example-compose-data", "external":true})
    );
    let labels = &next.successor.plan.retained_volume_labels[&key("data")];
    assert_eq!(
        labels["toolkit.rkat.ai/request-sha256"],
        prepare(&a).unwrap().plan.request_sha256.as_str()
    );
    assert_eq!(
        inner(&a, "manifest_json")["exports"],
        inner(&b, "manifest_json")["exports"]
    );
}

#[test]
fn successor_refuses_state_or_custody_contract_changes() {
    let a = fixture();
    let b = new_version(&a, "2.0.0");
    for field in ["owner", "format", "namespace"] {
        let mut changed = b.clone();
        let mut manifest = inner(&changed, "manifest_json");
        component(&mut manifest, "database")["state"][0][field] = json!("changed");
        set_inner(&mut changed, "manifest_json", &manifest);
        assert!(successor(&a, &a, &changed).is_err(), "{field}");
    }
    for field in ["project", "owner", "placement"] {
        let mut changed = b.clone();
        let mut profile = inner(&changed, "profile_json");
        profile[field] = json!("other");
        set_inner(&mut changed, "profile_json", &profile);
        assert!(successor(&a, &a, &changed).is_err(), "{field}");
    }
    let mut changed = b.clone();
    let mut profile = inner(&changed, "profile_json");
    profile["engine"]["id"] = json!("other-engine");
    set_inner(&mut changed, "profile_json", &profile);
    assert!(successor(&a, &a, &changed).is_err());
    let mut package = inner(&b, "package_json");
    package["identity"]["publisher"] = json!("other");
    let mut manifest = inner(&b, "manifest_json");
    manifest["package"] = package["identity"].clone();
    manifest["package_content_digest"] =
        json!(ContentDigest::of_bytes(&canonical_json(&package).unwrap()));
    set_inner(&mut changed, "package_json", &package);
    set_inner(&mut changed, "manifest_json", &manifest);
    assert!(successor(&a, &a, &changed).is_err());
}

#[test]
fn normal_planner_refuses_the_custody_marker_and_successor_envelope() {
    assert!(
        prepare_compose_json(br#"{"schema_version":2,"kind":"compose_successor_custody"}"#)
            .is_err()
    );
    let envelope = json!({"schema_version":1, "origin_request":FIXTURE,
        "predecessor_request":FIXTURE, "successor_request":FIXTURE});
    assert!(prepare_compose_json(&canonical_json(&envelope).unwrap()).is_err());
    assert!(prepare_compose_successor_json(&canonical_json(&envelope).unwrap()).is_err());
}
fn component<'a>(manifest: &'a mut Value, id: &str) -> &'a mut Value {
    manifest["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["id"] == id)
        .unwrap()
}
fn canonical_valid(request: &Value) -> bool {
    let package: IntegrationPackage =
        serde_json::from_value(inner(request, "package_json")).unwrap();
    let manifest: ActivationManifest =
        serde_json::from_value(inner(request, "manifest_json")).unwrap();
    manifest.validate(&package).is_ok()
}

#[test]
fn real_graph_has_canonical_orders_exact_files_and_separate_request_lifetime() {
    let request = fixture();
    let result = prepare(&request).unwrap();
    let plan = &result.plan;
    assert_eq!(result.status, "prepared");
    assert_eq!(
        plan.initialization_order,
        ["database", "api", "independent", "mcp"].map(key)
    );
    assert_eq!(
        plan.shutdown_order,
        ["mcp", "independent", "api", "database"].map(key)
    );
    assert_eq!(
        plan.deployment_order,
        ["database", "api", "independent"].map(key)
    );
    assert_eq!(plan.request_components, [key("mcp")]);
    assert_eq!(
        plan.components[&key("api")].deployment_dependencies,
        [key("database")]
    );
    assert!(
        plan.components[&key("independent")]
            .deployment_dependencies
            .is_empty()
    );
    assert_eq!(
        plan.components[&key("mcp")].lifetime,
        ComponentLifetime::Request
    );
    assert_eq!(
        plan.components[&key("mcp")].image,
        format!("sha256:{}", "e".repeat(64))
    );
    assert_eq!(
        plan.request_sha256,
        ContentDigest::of_bytes(&canonical_json(&request).unwrap())
    );
    assert_eq!(
        result.plan_sha256,
        ContentDigest::of_bytes(&canonical_json(plan).unwrap())
    );
    assert_eq!(result.plan_sha256, prepare(&request).unwrap().plan_sha256);
    assert_eq!(plan.files.len(), 4);
    for (name, file) in &plan.files {
        assert_eq!(
            file.sha256,
            ContentDigest::of_bytes(file.contents.as_bytes())
        );
        assert_eq!(
            file.mode,
            if name == "compose.json" { 0o600 } else { 0o444 }
        );
    }
    assert_eq!(
        plan.files["inputs/configuration"].contents,
        request["configuration"]
    );
    assert_eq!(plan.files["inputs/policy"].contents, request["policy"]);
    assert_eq!(
        plan.files["inputs/files/db-password"].contents,
        request["files"]["db-password"]
    );
    assert_eq!(plan.files["compose.json"].contents, plan.compose_json);
    let doc: Value = serde_json::from_str(&plan.compose_json).unwrap();
    assert_eq!(doc["networks"]["default"]["internal"], true);
    assert_eq!(doc["networks"]["default"]["name"], plan.network);
    assert_eq!(doc["services"]["mcp"]["profiles"], json!(["request"]));
    assert!(doc["services"]["database"].get("profiles").is_none());
    assert_eq!(
        doc["services"]["api"]["depends_on"]["database"]["condition"],
        "service_started"
    );
    assert_eq!(
        doc["services"]["database"]["environment"]["POSTGRES_PASSWORD_FILE"],
        "/run/config/db-password"
    );
    assert_eq!(
        doc["services"]["database"]["volumes"][0]["bind"]["create_host_path"],
        false
    );
    assert_eq!(doc["services"]["mcp"]["volumes"][2]["read_only"], true);
    assert_eq!(
        doc["services"]["database"]["healthcheck"],
        json!({"disable":true})
    );
    assert_eq!(doc["volumes"]["example-compose-data"]["driver"], "local");
    for service in doc["services"].as_object().unwrap().values() {
        assert_eq!(service["pull_policy"], "never");
        assert!(service.get("ports").is_none());
        assert_eq!(service["working_dir"], "/");
    }
}

#[test]
fn dependency_closure_comes_from_selected_edges_not_input_order_or_names() {
    let mut request = fixture();
    let mut manifest = inner(&request, "manifest_json");
    component(&mut manifest, "mcp")["dependencies"] =
        json!([{"component":"api","selected_version":"1"}]);
    manifest["components"].as_array_mut().unwrap().reverse();
    set_inner(&mut request, "manifest_json", &manifest);
    let result = prepare(&request).unwrap();
    assert_eq!(
        result.plan.components[&key("mcp")].deployment_dependencies,
        [key("database"), key("api")]
    );
    assert_eq!(
        result.plan.components[&key("mcp")].dependencies,
        [key("api")]
    );
    let mut profile = inner(&request, "profile_json");
    profile["components"]["database"]["service"] = json!("renamed-database");
    set_inner(&mut request, "profile_json", &profile);
    let changed = prepare(&request).unwrap();
    let doc: Value = serde_json::from_str(&changed.plan.compose_json).unwrap();
    assert_eq!(
        doc["services"]["api"]["depends_on"]["renamed-database"]["condition"],
        "service_started"
    );
    assert_eq!(
        changed.plan.initialization_order,
        result.plan.initialization_order
    );
}

#[test]
fn exact_source_bytes_and_profile_authority_remain_bound() {
    let request = fixture();
    let original = prepare(&request).unwrap();
    for field in ["configuration", "policy", "profile_json"] {
        let mut changed = request.clone();
        changed[field] = json!(format!("{} ", request[field].as_str().unwrap()));
        assert_eq!(
            prepare(&changed).unwrap_err(),
            ComposePreparationError::BindingMismatch
        );
    }
    let mut changed = request.clone();
    changed["files"]["db-password"] = json!("altered\n");
    assert_eq!(
        prepare(&changed).unwrap_err(),
        ComposePreparationError::BindingMismatch
    );
    let mut changed = request.clone();
    let mut profile = inner(&changed, "profile_json");
    profile["engine"]["id"] = json!("different-owner-engine");
    set_inner(&mut changed, "profile_json", &profile);
    assert_ne!(prepare(&changed).unwrap().plan_sha256, original.plan_sha256);
    let spaced = format!(
        " {} ",
        String::from_utf8(canonical_json(&request).unwrap()).unwrap()
    );
    let same_data = prepare_compose_json(spaced.as_bytes()).unwrap();
    assert_ne!(same_data.plan.request_sha256, original.plan.request_sha256);
    assert_eq!(
        same_data.plan.manifest_sha256,
        original.plan.manifest_sha256
    );
}

#[test]
fn absent_bind_source_prefix_preserves_existing_plan_bytes() {
    let request = fixture();
    let original = prepare(&request).unwrap();
    // Captured from the compiled planner before the optional field was added.
    assert_eq!(
        original.plan_sha256,
        ContentDigest::parse("7302533b0f89e46adc1392d31926d1b95ca18be0892f798be490744453e98af4")
            .unwrap()
    );
    let engine = serde_json::to_value(&original.plan.engine).unwrap();
    assert!(engine.get("bind_source_prefix").is_none());

    let mut with_null = request.clone();
    let mut profile = inner(&with_null, "profile_json");
    profile["engine"]["bind_source_prefix"] = Value::Null;
    set_inner(&mut with_null, "profile_json", &profile);
    let changed = prepare(&with_null).unwrap();
    assert_eq!(serde_json::to_value(changed.plan.engine).unwrap(), engine);
    // The optional field changes no engine semantics, but raw input stays bound.
    assert_ne!(changed.plan_sha256, original.plan_sha256);
}

#[test]
fn bind_source_prefix_is_explicit_preserved_and_digest_bound() {
    let mut request = fixture();
    let mut profile = inner(&request, "profile_json");
    let prefix = json!({"local_root":"/private/tmp","engine_root":"/host_mnt/private/tmp"});
    profile["engine"]["bind_source_prefix"] = prefix.clone();
    set_inner(&mut request, "profile_json", &profile);
    let mapped = prepare(&request).unwrap();
    assert_eq!(
        serde_json::to_value(&mapped.plan.engine).unwrap()["bind_source_prefix"],
        prefix
    );
    let compose: Value = serde_json::from_str(&mapped.plan.compose_json).unwrap();
    assert_eq!(
        compose["services"]["database"]["volumes"][0]["source"],
        "./inputs/files/db-password"
    );
    profile["engine"]["bind_source_prefix"]["engine_root"] = json!("/other_engine/private/tmp");
    // Without updating the exact profile digest, changed mapping is refused.
    request["profile_json"] = json!(String::from_utf8(canonical_json(&profile).unwrap()).unwrap());
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::BindingMismatch
    );
    set_inner(&mut request, "profile_json", &profile);
    let changed = prepare(&request).unwrap();
    assert_ne!(changed.plan_sha256, mapped.plan_sha256);
    assert_ne!(changed.plan.profile_sha256, mapped.plan.profile_sha256);

    // A lexical prefix without a path boundary is not an ancestor.
    profile["engine"]["bind_source_prefix"] =
        json!({"local_root":"/private/tmp","engine_root":"/private/tmp-other"});
    set_inner(&mut request, "profile_json", &profile);
    assert!(prepare(&request).is_ok());
}

#[test]
fn bind_source_prefix_refuses_ambiguous_roots_and_unknown_fields() {
    for root in [
        "",
        "/",
        "relative",
        "/private/tmp/",
        "/private//tmp",
        "/private/./tmp",
        "/private/../tmp",
        "/private\\tmp",
        "/private/$TMP",
        "/private/tmp\n",
        "/private/space here",
        "/private/\u{00e9}",
    ] {
        for field in ["local_root", "engine_root"] {
            let mut request = fixture();
            let mut profile = inner(&request, "profile_json");
            profile["engine"]["bind_source_prefix"] =
                json!({"local_root":"/private/tmp","engine_root":"/host_mnt/private/tmp"});
            profile["engine"]["bind_source_prefix"][field] = json!(root);
            set_inner(&mut request, "profile_json", &profile);
            assert_eq!(
                prepare(&request).unwrap_err(),
                ComposePreparationError::InvalidProfile,
                "{field}: {root:?}"
            );
        }
    }
    for (local, engine) in [
        ("/private/tmp", "/private/tmp"),
        ("/private", "/private/tmp"),
        ("/private/tmp", "/private"),
    ] {
        let mut request = fixture();
        let mut profile = inner(&request, "profile_json");
        profile["engine"]["bind_source_prefix"] = json!({"local_root":local,"engine_root":engine});
        set_inner(&mut request, "profile_json", &profile);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::InvalidProfile
        );
    }
    let mut request = fixture();
    let mut profile = inner(&request, "profile_json");
    profile["engine"]["bind_source_prefix"] =
        json!({"local_root":format!("/{}", "x".repeat(257)),"engine_root":"/host_mnt"});
    set_inner(&mut request, "profile_json", &profile);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::InvalidProfile
    );
    for prefix in [
        json!({"local_root":"/private/tmp","engine_root":"/host_mnt/private/tmp","guess":true}),
        json!({"local_root":"/private/tmp"}),
        json!({"local_root":false,"engine_root":"/host_mnt/private/tmp"}),
        json!([]),
    ] {
        let mut request = fixture();
        let mut profile = inner(&request, "profile_json");
        profile["engine"]["bind_source_prefix"] = prefix;
        set_inner(&mut request, "profile_json", &profile);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::InvalidInput
        );
    }
}

#[test]
fn health_is_explicit_and_backend_specific_not_implicit_image_work() {
    let mut request = fixture();
    let mut profile = inner(&request, "profile_json");
    profile["components"]["database"]["healthcheck"] = json!({"argv":["usr/bin/pg_isready","-q"],"interval_ms":1000,"timeout_ms":500,"retries":5,"start_period_ms":0});
    set_inner(&mut request, "profile_json", &profile);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::UnsupportedSelection
    );
    let mut manifest = inner(&request, "manifest_json");
    component(&mut manifest, "database")["readiness"]["checks"] = json!(["healthy"]);
    set_inner(&mut request, "manifest_json", &manifest);
    let result = prepare(&request).unwrap();
    let doc: Value = serde_json::from_str(&result.plan.compose_json).unwrap();
    assert_eq!(
        doc["services"]["database"]["healthcheck"]["test"],
        json!(["CMD", "usr/bin/pg_isready", "-q"])
    );
    assert_eq!(
        doc["services"]["api"]["depends_on"]["database"]["condition"],
        "service_healthy"
    );
}

#[test]
fn namespace_sharing_requires_owner_dependency_readonly_and_no_aliasing() {
    let original = fixture();
    for field_value in [
        json!({"source":"data","read_only":false}),
        json!({"source":"foreign","read_only":true}),
    ] {
        let mut request = original.clone();
        let mut profile = inner(&request, "profile_json");
        let mount = &mut profile["components"]["mcp"]["mounts"][2];
        mount["source"] = field_value["source"].clone();
        mount["read_only"] = field_value["read_only"].clone();
        set_inner(&mut request, "profile_json", &profile);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::InvalidProfile
        );
    }
    let mut request = original.clone();
    let mut manifest = inner(&request, "manifest_json");
    component(&mut manifest, "mcp")["dependencies"] = json!([]);
    set_inner(&mut request, "manifest_json", &manifest);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::InvalidProfile
    );
    for name in ["foreign-data", "example-compose-socket"] {
        let mut request = original.clone();
        let mut profile = inner(&request, "profile_json");
        profile["volumes"]["data"]["name"] = json!(name);
        set_inner(&mut request, "profile_json", &profile);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::InvalidProfile
        );
    }
    let mut request = original;
    let mut profile = inner(&request, "profile_json");
    profile["components"]["database"]["mounts"][1]["read_only"] = json!(true);
    set_inner(&mut request, "profile_json", &profile);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::InvalidProfile
    );
}

#[test]
fn mount_paths_never_shadow_executable_reserved_paths_or_other_inputs() {
    for target in [
        "/proc/x",
        "/sys",
        "/dev",
        "/usr/local/bin",
        "/run/config",
        "/run/../foreign",
        "/run//foreign",
        "/",
    ] {
        let mut request = fixture();
        let mut profile = inner(&request, "profile_json");
        profile["components"]["mcp"]["mounts"][0]["target"] = json!(target);
        set_inner(&mut request, "profile_json", &profile);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::InvalidProfile,
            "{target}"
        );
    }
    for source in [
        "../policy",
        "/etc/passwd",
        "files/../db-password",
        "missing",
    ] {
        let mut request = fixture();
        let mut profile = inner(&request, "profile_json");
        profile["components"]["mcp"]["mounts"][0]["source"] = json!(source);
        set_inner(&mut request, "profile_json", &profile);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::InvalidProfile
        );
    }
}

#[test]
fn refuses_unselected_extra_parts_interpolation_and_implicit_engine_inputs() {
    let original = fixture();
    for (path, value) in [
        ("/components/unused", json!({})),
        ("/components/api/service", json!("database")),
        ("/components/api/environment/SECRET", json!("${AMBIENT}")),
        ("/components/api/resources/cpu_millis", json!(0)),
        ("/components/api/platform", json!("linux/unknown")),
        ("/components/api/user", json!("postgres")),
        ("/engine/endpoint", json!("tcp://remote:2375")),
        ("/engine/docker/path", json!("docker")),
    ] {
        let mut request = original.clone();
        let mut profile = inner(&request, "profile_json");
        if path == "/components/unused" {
            profile["components"]["unused"] = value;
        } else if path.ends_with("SECRET") {
            profile["components"]["api"]["environment"]["SECRET"] = value;
        } else {
            *profile.pointer_mut(path).unwrap() = value;
        }
        set_inner(&mut request, "profile_json", &profile);
        assert!(prepare(&request).is_err(), "{path}");
    }
}

#[test]
fn refuses_unsupported_lifecycle_graph_state_and_host_contracts() {
    let original = fixture();
    for (path, value) in [
        ("/lifetime", json!("externally_managed")),
        (
            "/activation_scope",
            json!({"kind":"toolkit_required","activation_contract":{"name":"host/enforcement","version":{"major":1,"minimum_minor":0}}}),
        ),
        ("/shutdown/timeout", json!("owner_controlled_fencing")),
        ("/shutdown/owner", json!("other")),
        ("/shutdown/operation", json!("erase")),
        ("/shutdown/maximum_wait_ms", json!(500)),
        ("/shutdown/maximum_wait_ms", json!(1500)),
        (
            "/rollback",
            json!({"kind":"reversible","owner":"host","operation":"undo"}),
        ),
        ("/readiness/checks", json!(["domain-ready"])),
    ] {
        let mut request = original.clone();
        let mut manifest = inner(&request, "manifest_json");
        *component(&mut manifest, "api").pointer_mut(path).unwrap() = value;
        set_inner(&mut request, "manifest_json", &manifest);
        assert!(prepare(&request).is_err(), "{path}");
    }
    let mut request = original.clone();
    let mut manifest = inner(&request, "manifest_json");
    component(&mut manifest, "database")["state"][0]["migrations"] = json!([{"from":"0","operation":"migrate","artifact_digest":"a".repeat(64),"reversible":false}]);
    set_inner(&mut request, "manifest_json", &manifest);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::UnsupportedSelection
    );
    let mut request = original;
    let mut profile = inner(&request, "profile_json");
    profile["host"]["contracts"] = json!({});
    set_inner(&mut request, "profile_json", &profile);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::IncompatibleHost
    );
}

#[test]
fn canonical_graph_rejects_wrong_versions_cycles_and_request_dependencies() {
    for dependencies in [
        json!([{"component":"database","selected_version":"2"}]),
        json!([{"component":"absent","selected_version":"1"}]),
        json!([{"component":"mcp","selected_version":"1"}]),
        json!([{"component":"api","selected_version":"1"}]),
    ] {
        let mut request = fixture();
        let mut manifest = inner(&request, "manifest_json");
        component(&mut manifest, "api")["dependencies"] = dependencies;
        set_inner(&mut request, "manifest_json", &manifest);
        assert_eq!(
            prepare(&request).unwrap_err(),
            ComposePreparationError::UnsupportedSelection
        );
    }
}

#[test]
fn local_image_scheme_is_exact_and_container_artifact_only() {
    let original = fixture();
    assert!(canonical_valid(&original));
    for runtime in ["native", "python", "node", "remote", "host_provided"] {
        let mut request = original.clone();
        let mut manifest = inner(&request, "manifest_json");
        component(&mut manifest, "mcp")["runtime"] = json!(runtime);
        set_inner(&mut request, "manifest_json", &manifest);
        assert!(!canonical_valid(&request), "{runtime}");
    }
    for location in [
        format!("docker-image://host/sha256/{}", "e".repeat(64)),
        format!("docker-image:///sha256/{}?x=y", "e".repeat(64)),
        format!("docker-image:///sha256/{}#fragment", "e".repeat(64)),
        format!("docker-image:///sha256/{}", "f".repeat(64)),
        format!("docker-image:///sha256/{}", "E".repeat(64)),
        format!("docker-image://user@host/sha256/{}", "e".repeat(64)),
    ] {
        let mut request = original.clone();
        let mut manifest = inner(&request, "manifest_json");
        component(&mut manifest, "mcp")["source"]["location"] = json!(location);
        set_inner(&mut request, "manifest_json", &manifest);
        assert!(!canonical_valid(&request));
    }
    let mut request = original.clone();
    let mut profile = inner(&request, "profile_json");
    profile["components"]["mcp"]["image_id"] = json!(format!("sha256:{}", "f".repeat(64)));
    set_inner(&mut request, "profile_json", &profile);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::BindingMismatch
    );
    let mut request = original;
    let mut manifest = inner(&request, "manifest_json");
    let c = component(&mut manifest, "mcp");
    c["runtime"] = json!("remote");
    c["lifetime"] = json!("externally_managed");
    c["source"] = json!({"kind":"remote_service","endpoint":format!("docker-image:///sha256/{}", "e".repeat(64)),"publisher":"test","configuration_revision":"1","observed_contract":"e".repeat(64),"transport":"mcp_streamable_http"});
    set_inner(&mut request, "manifest_json", &manifest);
    assert!(!canonical_valid(&request));
}

#[test]
fn registry_digest_grammar_remains_strict_and_independent_of_local_form() {
    let original = fixture();
    assert!(prepare(&original).is_ok());
    for location in [
        format!(
            "https://registry.example/v2/a..b/manifests/sha256:{}",
            "a".repeat(64)
        ),
        "https://registry.example/v2/server/tags/latest".to_owned(),
        format!(
            "http://registry.example/v2/server/manifests/sha256:{}",
            "a".repeat(64)
        ),
        format!(
            "https://registry.example/v2/server/manifests/sha256:{}",
            "f".repeat(64)
        ),
    ] {
        let mut request = original.clone();
        let mut manifest = inner(&request, "manifest_json");
        component(&mut manifest, "database")["source"]["location"] = json!(location);
        set_inner(&mut request, "manifest_json", &manifest);
        assert!(prepare(&request).is_err());
    }
}

#[test]
fn parser_and_limits_fail_before_unbounded_graph_processing() {
    assert_eq!(
        prepare_compose_json(br#"{"schema_version":1,"schema_version":1}"#).unwrap_err(),
        ComposePreparationError::InvalidInput
    );
    assert_eq!(
        prepare_compose_json(&vec![b' '; COMPOSE_MAX_INPUT + 1]).unwrap_err(),
        ComposePreparationError::InputLimit
    );
    let mut request = fixture();
    request["unrecognized"] = json!(true);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::InvalidInput
    );
    let mut request = fixture();
    let mut manifest = inner(&request, "manifest_json");
    let c = manifest["components"][0].clone();
    manifest["components"] = json!(vec![c; 17]);
    set_inner(&mut request, "manifest_json", &manifest);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::InputLimit
    );
    let mut request = fixture();
    request["configuration"] = json!("x".repeat(65537));
    let mut manifest = inner(&request, "manifest_json");
    manifest["configuration"]["content_digest"] = json!(ContentDigest::of_bytes(
        request["configuration"].as_str().unwrap().as_bytes()
    ));
    set_inner(&mut request, "manifest_json", &manifest);
    assert_eq!(
        prepare(&request).unwrap_err(),
        ComposePreparationError::InputLimit
    );
}

#[test]
fn cli_reports_only_fixed_diagnostic_not_input_values() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-compose-plan"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"secret":"do-not-reflect-this"}"#)
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(result.stderr, b"invalid_input\n");
}
