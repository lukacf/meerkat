//! Helper-facing wire controls only. Checked installation identity, native
//! authorization and protected review effects are covered by their owners.
use super::*;

fn proposal() -> Value {
    json!({"services":[{"job_id":"one","operations":["read_one"]},
        {"job_id":"two","operations":["read_two"]}],"agent":"original", "expires_in_ms":null,
        "tool_policy":{"tools":[{"name":"read_one","consequence":"r0"},
            {"name":"read_two","consequence":"r0"}],"catalog":"r0"}})
}

#[test]
fn batch_arguments_require_one_finite_exact_set_and_explicit_policy() {
    let valid = proposal();
    assert!(parse_batch(&valid.to_string()).is_ok());
    let mut eight = valid.clone();
    eight["services"] = json!((0..8).map(|index| json!({"job_id":format!("job-{index}"),
        "operations":["read"]})).collect::<Vec<_>>());
    assert!(parse_batch(&eight.to_string()).is_ok());
    for n in [0, 1, 9] {
        let mut invalid = eight.clone(); invalid["services"] = json!((0..n).map(|index|
            json!({"job_id":format!("job-{index}"),"operations":["read"]})).collect::<Vec<_>>());
        assert!(parse_batch(&invalid.to_string()).is_err());
    }
    for field in ["services", "agent", "expires_in_ms", "tool_policy"] {
        let mut invalid = valid.clone(); invalid.as_object_mut().unwrap().remove(field);
        assert!(parse_batch(&invalid.to_string()).is_err(), "{field}");
    }
    for (field, value) in [("job_id", json!("singleton")), ("operations", json!(["read"])),
        ("command", json!("/bin/sh")), ("credentials", json!({})), ("authority", json!({}))] {
        let mut invalid = valid.clone(); invalid[field] = value;
        assert!(parse_batch(&invalid.to_string()).is_err(), "{field}");
    }
    let mut duplicate = valid.clone(); duplicate["services"][1] = duplicate["services"][0].clone();
    assert!(parse_batch(&duplicate.to_string()).is_err());
    let mut repeated_operation = valid.clone(); repeated_operation["services"][0]["operations"] = json!(["read", "read"]);
    assert!(parse_batch(&repeated_operation.to_string()).is_err());
    let mut no_catalog = valid.clone(); no_catalog["tool_policy"]["catalog"] = Value::Null;
    assert!(parse_batch(&no_catalog.to_string()).is_err());
    let duplicate_key = valid.to_string().replacen("\"agent\":", "\"agent\":\"other\",\"agent\":", 1);
    assert!(parse_batch(&duplicate_key).is_err());
}

#[test]
fn public_tool_schema_preserves_provider_object_root_and_both_argument_forms() {
    let definition = installed_public_service_request_definition();
    let schema = &definition.input_schema;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema.get("oneOf").is_none() && schema.get("anyOf").is_none());
    for field in ["job_id", "operations", "agent", "expires_in_ms", "tool_policy", "services"] {
        assert!(schema["properties"].get(field).is_some(), "{field}");
    }
    assert!(parse(&json!({"job_id":"old-job","operations":["read"],
        "agent":"original","expires_in_ms":null}).to_string()).is_ok());
    assert!(parse_batch(&proposal().to_string()).is_ok());
    assert!(parse(&proposal().to_string()).is_err());
}

fn assert_invalid_batch(raw: &str, case: &str) {
    assert!(
        matches!(parse_batch(raw), Err(ToolError::InvalidArguments { name, .. })
            if name == SETUP_PUBLIC_SERVICE_REQUEST),
        "{case}: malformed arguments must fail before access review"
    );
}

fn singleton() -> Value {
    json!({"job_id":"checked-singleton", "operations":["read"],
        "agent":"original", "expires_in_ms":null})
}

#[test]
fn public_batch_and_singleton_forms_are_exclusive_even_with_null_extra_fields() {
    let single = singleton();
    assert!(parse(&single.to_string()).is_ok());
    assert_invalid_batch(&single.to_string(), "singleton is not a batch");

    for services in [Value::Null, json!([]), proposal()["services"].clone()] {
        let mut mixed = single.clone();
        mixed["services"] = services;
        assert!(parse(&mixed.to_string()).is_err());
        assert_invalid_batch(&mixed.to_string(), "singleton plus services");
    }
    for (field, value) in [
        ("job_id", Value::Null),
        ("job_id", json!("checked-singleton")),
        ("operations", Value::Null),
        ("operations", json!(["read"])),
    ] {
        let mut mixed = proposal();
        mixed[field] = value;
        assert!(parse(&mixed.to_string()).is_err());
        assert_invalid_batch(&mixed.to_string(), field);
    }

    for raw in ["", "null", "true", "3", "[]", "\"services\"", "{}"] {
        assert_invalid_batch(raw, "non-proposal root");
    }
    assert_invalid_batch(
        &format!("{} {{}}", proposal()),
        "a second JSON value is not trailing whitespace",
    );
    assert_invalid_batch(
        &format!("{}{}", proposal(), " ".repeat(64 * 1024)),
        "valid JSON still obeys the whole request byte budget",
    );
}

#[test]
fn batch_duplicate_keys_are_rejected_before_any_last_value_can_win() {
    let valid = proposal().to_string();
    // These cover every object level, including typed policy objects. An
    // earlier conflicting value cannot be erased by Value deserialization.
    for (field, first_value) in [
        ("services", "[]"),
        ("agent", "\"different-agent\""),
        ("expires_in_ms", "0"),
        ("tool_policy", "{}"),
        ("job_id", "\"different-job\""),
        ("operations", "[]"),
        ("tools", "[]"),
        ("catalog", "null"),
        ("name", "\"different-tool\""),
        ("consequence", "\"r3\""),
    ] {
        let key = format!("\"{field}\":");
        assert!(valid.contains(&key), "fixture must contain {field}");
        let raw = valid.replacen(&key, &format!("{key}{first_value},{key}"), 1);
        assert_invalid_batch(&raw, field);
    }

    let mut repeated_job = proposal();
    repeated_job["services"][1]["job_id"] = repeated_job["services"][0]["job_id"].clone();
    assert_ne!(
        repeated_job["services"][0]["operations"],
        repeated_job["services"][1]["operations"]
    );
    assert_invalid_batch(
        &repeated_job.to_string(),
        "same job with different operations",
    );

    // Distinct checked jobs may expose the same raw tool name. The checked
    // selection owner, not this parser, resolves their actual MCP targets.
    let mut shared_raw_name = proposal();
    shared_raw_name["services"][1]["operations"] =
        shared_raw_name["services"][0]["operations"].clone();
    assert!(parse_batch(&shared_raw_name.to_string()).is_ok());
}

#[test]
fn batch_lifetime_is_explicit_and_has_the_singleton_integer_contract() {
    for lifetime in [Value::Null, json!(1), json!(30_000), json!(u64::MAX)] {
        let mut valid = proposal();
        valid["expires_in_ms"] = lifetime.clone();
        let parsed = parse_batch(&valid.to_string()).ok().unwrap();
        assert_eq!(parsed.expires_in_ms, lifetime.as_u64());
    }
    for lifetime in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("30000"),
        json!(true),
        json!([]),
        json!({}),
    ] {
        let mut invalid = proposal();
        invalid["expires_in_ms"] = lifetime;
        assert_invalid_batch(&invalid.to_string(), "invalid explicit lifetime");
    }
    let mut missing = proposal();
    missing.as_object_mut().unwrap().remove("expires_in_ms");
    assert_invalid_batch(&missing.to_string(), "missing lifetime is not null");
    let overflow = proposal().to_string().replace(
        "\"expires_in_ms\":null",
        "\"expires_in_ms\":18446744073709551616",
    );
    assert_invalid_batch(&overflow, "lifetime exceeds u64");
}

#[test]
fn batch_members_keep_exact_raw_operation_and_bounded_identity_validation() {
    let mut valid = proposal();
    valid["services"][0]["operations"] = json!(["read.v1_with-dash", "x".repeat(128)]);
    assert!(parse_batch(&valid.to_string()).is_ok());
    valid["services"][0]["operations"] = json!((0..128)
        .map(|index| format!("read_{index}"))
        .collect::<Vec<_>>());
    assert!(parse_batch(&valid.to_string()).is_ok());

    for operations in [
        Value::Null,
        json!("read"),
        json!([]),
        json!([null]),
        json!([7]),
        json!([""]),
        json!(["read", "read"]),
        json!(["declaration/read"]),
        json!(["read:namespace"]),
        json!(["read tool"]),
        json!(["read\n"]),
        json!(["r\u{00e9}ad"]),
        json!(["x".repeat(129)]),
        json!((0..129)
            .map(|index| format!("read_{index}"))
            .collect::<Vec<_>>()),
    ] {
        let mut invalid = proposal();
        invalid["services"][0]["operations"] = operations;
        assert_invalid_batch(&invalid.to_string(), "invalid raw operations");
    }
    for field in ["job_id", "operations"] {
        let mut missing = proposal();
        missing["services"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_invalid_batch(&missing.to_string(), field);
    }
    for field in [
        "command",
        "credentials",
        "account",
        "authority",
        "agent",
        "expires_in_ms",
    ] {
        let mut extra = proposal();
        extra["services"][0][field] = Value::Null;
        assert_invalid_batch(&extra.to_string(), field);
    }
    for identity in [
        Value::Null,
        json!(7),
        json!(""),
        json!("bad\nidentity"),
        json!("x".repeat(2049)),
    ] {
        let mut job = proposal();
        job["services"][0]["job_id"] = identity.clone();
        assert_invalid_batch(&job.to_string(), "invalid job identity");
        let mut agent = proposal();
        agent["agent"] = identity;
        assert_invalid_batch(&agent.to_string(), "invalid consumer identity");
    }
}

#[test]
fn batch_policy_requires_typed_complete_fields_without_embedded_authority() {
    for policy in [
        Value::Null,
        json!({}),
        json!({"catalog":"r0"}),
        json!({"tools":[]}),
        json!({"tools":null,"catalog":"r0"}),
        json!({"tools":[],"catalog":null}),
        json!({"tools":[],"catalog":"r4"}),
        json!({"tools":[],"catalog":"r0","authority":{}}),
        json!({"tools":[{"name":"read"}],"catalog":"r0"}),
        json!({"tools":[{"name":"read","consequence":"r0","grant_id":"copied"}],"catalog":"r0"}),
    ] {
        let mut invalid = proposal();
        invalid["tool_policy"] = policy;
        assert_invalid_batch(&invalid.to_string(), "invalid policy shape");
    }
}

#[test]
fn public_batch_schema_exposes_bounded_member_and_lifetime_fields() {
    let definition = installed_public_service_request_definition();
    let schema = &definition.input_schema;
    assert_eq!(schema["required"], json!(["agent", "expires_in_ms"]));
    assert_eq!(
        schema["properties"]["expires_in_ms"]["type"],
        json!(["integer", "null"])
    );
    assert_eq!(schema["properties"]["expires_in_ms"]["minimum"], 1);

    let services = &schema["properties"]["services"];
    assert_eq!(services["type"], "array");
    assert_eq!(services["minItems"], 2);
    assert_eq!(services["maxItems"], MAX_PUBLIC_SERVICE_BATCH);
    assert_eq!(services["items"]["type"], "object");
    assert_eq!(services["items"]["additionalProperties"], false);
    assert_eq!(
        services["items"]["required"],
        json!(["job_id", "operations"])
    );
    let operations = &services["items"]["properties"]["operations"];
    assert_eq!(operations, &schema["properties"]["operations"]);
    assert_eq!(operations["type"], "array");
    assert_eq!(operations["minItems"], 1);
    assert_eq!(operations["maxItems"], 128);
    assert_eq!(operations["uniqueItems"], true);
    assert_eq!(operations["items"]["type"], "string");
    assert_eq!(operations["items"]["minLength"], 1);
    assert_eq!(operations["items"]["maxLength"], 128);
    assert_eq!(
        schema["properties"]["tool_policy"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["properties"]["tool_policy"]["required"],
        json!(["tools", "catalog"])
    );
}
