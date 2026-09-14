use std::collections::{BTreeMap, BTreeSet};

use meerkat_runtime::live_ledger::completion::{LiveCompletionEvent, LiveCompletionRecord};
use meerkat_runtime::live_ledger::completion_budget::{
    maximal_completion_events, maximal_completion_record, minimal_completion_record,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn aggregate_encoding_bound_requires_actual_maximum_and_all_nested_branches() -> TestResult {
    let schema = json!({
        "type": "object", "x-max-encoded-bytes": 29,
        "properties": {
            "kind": { "enum": ["a", "b"] },
            "data": { "type": "string" }
        },
        "required": ["kind", "data"], "additionalProperties": false
    });
    let full = json!({"kind":"a","data":"1234567"});
    assert_eq!(encoded_width(&full)?, 29);
    assert!(
        verify_schema_coverage(
            schema.clone(),
            &[json!({"kind":"a","data":""}), json!({"kind":"b","data":""})]
        )
        .is_err()
    );
    assert!(verify_schema_coverage(schema.clone(), std::slice::from_ref(&full)).is_err());
    assert!(
        verify_schema_coverage(
            schema.clone(),
            &[full.clone(), json!({"kind":"b","data":"12345678"})]
        )
        .is_err()
    );
    verify_schema_coverage(schema, &[full, json!({"kind":"b","data":""})])?;
    Ok(())
}

#[test]
fn provider_diagnostic_aggregate_bound_is_enforced_by_the_real_codec() -> TestResult {
    use meerkat_core::live_execution::backend::{
        LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES, LiveProviderDiagnostic,
    };
    let diagnostic = maximal_completion_events()?
        .into_iter()
        .find_map(|event| match event {
            LiveCompletionEvent::ChannelProviderDiagnostic { diagnostic }
                if serde_json::to_vec(&diagnostic).ok()?.len()
                    == LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES =>
            {
                Some(diagnostic)
            }
            _ => None,
        })
        .ok_or("no actual maximum diagnostic")?;
    let bytes = serde_json::to_vec(&diagnostic)?;
    assert_eq!(
        serde_json::from_slice::<LiveProviderDiagnostic>(&bytes)?,
        diagnostic
    );
    let mut oversized = serde_json::to_value(diagnostic)?;
    let delegation = oversized
        .pointer_mut("/attribution/response/delegation")
        .ok_or("delegation")?;
    *delegation = json!(format!("{}a", delegation.as_str().ok_or("reference")?));
    assert_eq!(
        encoded_width(&oversized)?,
        LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES + 1
    );
    assert!(serde_json::from_value::<LiveProviderDiagnostic>(oversized).is_err());
    Ok(())
}

#[test]
fn multibyte_or_escaped_characters_do_not_prove_minimum_encoded_string_charge() -> TestResult {
    let schema = json!({
        "type": "string", "minLength": 1, "maxLength": 128, "x-max-utf8-bytes": 128
    });
    for minimum in ["\u{00e9}", "\0"] {
        assert!(
            verify_schema_coverage(schema.clone(), &[json!(minimum), json!("\0".repeat(128))])
                .is_err()
        );
    }
    verify_schema_coverage(schema, &[json!("a"), json!("\0".repeat(128))])?;
    Ok(())
}

#[test]
fn bounded_string_requires_utf8_and_worst_escaping_not_just_a_tag() -> TestResult {
    let schema = json!({"type":"string", "maxLength":400, "x-max-utf8-bytes":400});
    assert!(verify_schema_coverage(schema.clone(), &[json!("")]).is_err());
    assert!(verify_schema_coverage(schema.clone(), &[json!(""), json!("a".repeat(400))]).is_err());
    verify_schema_coverage(schema, &[json!(""), json!("\0".repeat(400))])?;
    Ok(())
}

#[test]
fn shrinking_actual_field_occurrences_does_not_hide_behind_other_maxima() -> TestResult {
    for (kind, field) in [
        ("context_chunk", "content"),
        ("effect_terminal", "diagnostic"),
    ] {
        let mut fixtures = encoded_fixtures(true)?;
        for fixture in &mut fixtures {
            if fixture["event"]["kind"] == kind {
                fixture["event"][field] = json!("");
            }
        }
        assert!(
            verify_type_coverage::<LiveCompletionRecord>(&fixtures).is_err(),
            "shrinking {kind}.{field} retained tags but must lose its own extrema"
        );
    }
    let mut fixtures = encoded_fixtures(true)?;
    for fixture in &mut fixtures {
        fixture["sequence"] = json!(1);
    }
    assert!(verify_type_coverage::<LiveCompletionRecord>(&fixtures).is_err());
    Ok(())
}

#[test]
fn numeric_maximum_value_is_not_a_substitute_for_maximum_encoded_width() -> TestResult {
    let schema = json!({"type":"number", "format":"double", "minimum":0.0, "maximum":1.0e20});
    assert!(encoded_width(&json!(f64::MIN_POSITIVE))? > encoded_width(&json!(1.0e20))?);
    assert!(verify_schema_coverage(schema.clone(), &[json!(0.0), json!(1.0e20)]).is_err());
    verify_schema_coverage(
        schema,
        &[json!(0.0), json!(1.0e20), json!(f64::MIN_POSITIVE)],
    )?;
    Ok(())
}

#[test]
fn bounded_arrays_require_length_and_extrema_at_every_element_occurrence() -> TestResult {
    let schema = json!({
        "type":"array", "minItems":0, "maxItems":3,
        "items":{"type":"integer", "minimum":0, "maximum":255}
    });
    assert!(verify_schema_coverage(schema.clone(), &[json!([])]).is_err());
    assert!(
        verify_schema_coverage(
            schema.clone(),
            &[json!([]), json!([0, 0, 0]), json!([0, 255, 255])]
        )
        .is_err()
    );
    verify_schema_coverage(
        schema,
        &[json!([]), json!([0, 0, 0]), json!([255, 255, 255])],
    )?;
    let mut fixtures = encoded_fixtures(true)?;
    for fixture in &mut fixtures {
        if let Some(digest) = fixture.pointer_mut("/event/batch_digest") {
            *digest = json!(vec![0; 32]);
        }
    }
    assert!(verify_type_coverage::<LiveCompletionRecord>(&fixtures).is_err());
    Ok(())
}

fn encoded_fixtures(include_control: bool) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut fixtures = Vec::new();
    for event in maximal_completion_events()? {
        let minimum = minimal_completion_record(event.clone())?;
        let record = maximal_completion_record(event)?;
        if !matches!(record.event, LiveCompletionEvent::ChannelControl { .. }) || include_control {
            fixtures.push(serde_json::from_slice(record.encode()?.bytes())?);
            fixtures.push(serde_json::from_slice(minimum.encode()?.bytes())?);
        }
    }
    Ok(fixtures)
}

#[test]
fn actual_encoded_maximum_fixtures_cover_the_recursive_type_schema() -> TestResult {
    verify_type_coverage::<LiveCompletionRecord>(&encoded_fixtures(true)?)?;
    Ok(())
}

#[test]
fn unexecuted_control_constructor_cannot_count_as_measured_coverage() -> TestResult {
    let error = verify_type_coverage::<LiveCompletionRecord>(&encoded_fixtures(false)?)
        .err()
        .ok_or("missing channel controls unexpectedly passed coverage")?;
    assert!(error.to_string().contains("uncovered"));
    Ok(())
}

#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Nested {
    Known {},
    NewCase {},
}

#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FutureEnvelope {
    Payload { nested: Nested },
}

#[test]
fn newly_nested_enum_case_is_discovered_without_a_handwritten_type_list() -> TestResult {
    let mut fixtures = vec![serde_json::to_value(FutureEnvelope::Payload {
        nested: Nested::Known {},
    })?];
    assert!(verify_type_coverage::<FutureEnvelope>(&fixtures).is_err());
    fixtures.push(serde_json::to_value(FutureEnvelope::Payload {
        nested: Nested::NewCase {},
    })?);
    verify_type_coverage::<FutureEnvelope>(&fixtures)?;
    Ok(())
}

#[test]
fn missing_external_or_unsupported_schema_branches_fail_closed() {
    for schema in [
        json!({"$ref": "#/$defs/missing"}),
        json!({"$ref": "https://not-a-local-schema.invalid/schema"}),
        json!({"oneOf": []}),
        json!({"oneOf": [true, {"type": "null"}]}),
        json!({"type": "object", "if": {"required": ["hidden"]}, "then": {"type": "null"}}),
    ] {
        assert!(verify_schema_coverage(schema, &[json!(null)]).is_err());
    }
}

fn verify_type_coverage<T: JsonSchema>(fixtures: &[Value]) -> TestResult {
    verify_schema_coverage(serde_json::to_value(schemars::schema_for!(T))?, fixtures)
}

fn verify_schema_coverage(schema: Value, fixtures: &[Value]) -> TestResult {
    let mut coverage = Coverage {
        schema: &schema,
        expected: BTreeSet::new(),
        observed: BTreeSet::new(),
        numeric: BTreeMap::new(),
        aggregate_bound: None,
    };
    coverage.walk(&schema, "#", "#", None, &mut BTreeSet::new())?;
    let validator = jsonschema::validator_for(&schema)?;
    for fixture in fixtures {
        if !validator.is_valid(fixture) {
            return Err("an encoded fixture does not satisfy its actual type schema".into());
        }
        coverage.walk(&schema, "#", "#", Some(fixture), &mut BTreeSet::new())?;
    }
    let missing: Vec<_> = coverage.expected.difference(&coverage.observed).collect();
    if !missing.is_empty() {
        return Err(format!("uncovered serialized type branches: {missing:?}").into());
    }
    Ok(())
}

struct Coverage<'a> {
    schema: &'a Value,
    expected: BTreeSet<String>,
    observed: BTreeSet<String>,
    numeric: BTreeMap<String, NumericExtrema>,
    aggregate_bound: Option<u64>,
}

impl Coverage<'_> {
    fn record(&mut self, key: String, observation: Option<bool>) {
        match observation {
            None => {
                self.expected.insert(key);
            }
            Some(true) => {
                self.observed.insert(key);
            }
            Some(false) => {}
        }
    }

    fn walk(
        &mut self,
        schema: &Value,
        path: &str,
        occurrence: &str,
        value: Option<&Value>,
        references: &mut BTreeSet<String>,
    ) -> TestResult {
        let object = schema
            .as_object()
            .ok_or("unsupported unconstrained or boolean schema")?;
        for key in object.keys() {
            if ![
                "$schema",
                "$defs",
                "$ref",
                "title",
                "description",
                "default",
                "examples",
                "deprecated",
                "readOnly",
                "writeOnly",
                "format",
                "type",
                "properties",
                "required",
                "additionalProperties",
                "items",
                "prefixItems",
                "minItems",
                "maxItems",
                "uniqueItems",
                "minLength",
                "maxLength",
                "pattern",
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "multipleOf",
                "enum",
                "const",
                "oneOf",
                "anyOf",
                "allOf",
                "x-max-utf8-bytes",
                "x-max-encoded-bytes",
            ]
            .contains(&key.as_str())
            {
                return Err(format!("unsupported schema keyword {key} at {path}").into());
            }
        }
        let previous_bound = self.aggregate_bound;
        if let Some(maximum) = object.get("x-max-encoded-bytes") {
            let maximum = maximum
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or("invalid aggregate encoded bound")?;
            if let Some(value) = value
                && encoded_width(value)? > maximum as usize
            {
                return Err("aggregate encoded bound exceeded".into());
            }
            self.record(
                format!("{occurrence}/encoded-maximum"),
                value
                    .map(|value| encoded_width(value).map(|bytes| bytes == maximum as usize))
                    .transpose()?,
            );
            self.aggregate_bound = Some(maximum);
        }
        if let Some(reference) = object.get("$ref") {
            let reference = reference.as_str().ok_or("non-string schema reference")?;
            let pointer = reference
                .strip_prefix('#')
                .filter(|pointer| pointer.starts_with('/'))
                .ok_or("external or unsupported schema reference")?;
            let target = self
                .schema
                .pointer(pointer)
                .ok_or("missing local schema reference")?;
            if !references.insert(reference.to_owned()) {
                return Err("recursive schema needs an explicit finite coverage proof".into());
            }
            self.walk(target, reference, occurrence, value, references)?;
            references.remove(reference);
        }
        for keyword in ["oneOf", "anyOf", "allOf"] {
            if let Some(branches) = object.get(keyword) {
                let branches = branches
                    .as_array()
                    .filter(|branches| !branches.is_empty())
                    .ok_or("empty or malformed schema branches")?;
                let mut matched = 0;
                for (index, branch) in branches.iter().enumerate() {
                    let branch_path = format!("{path}/{keyword}/{index}");
                    let branch_occurrence = format!("{occurrence}/{keyword}/{index}");
                    if value.is_none() {
                        if keyword != "allOf" {
                            self.record(branch_path.clone(), None);
                        }
                        self.walk(branch, &branch_path, &branch_occurrence, None, references)?;
                    } else if let Some(value) = value
                        && self.branch_matches(branch, value)?
                    {
                        matched += 1;
                        if keyword != "allOf" {
                            self.record(branch_path.clone(), Some(true));
                        }
                        self.walk(
                            branch,
                            &branch_path,
                            &branch_occurrence,
                            Some(value),
                            references,
                        )?;
                    }
                }
                if value.is_some()
                    && ((keyword == "allOf" && matched != branches.len())
                        || (keyword != "allOf" && matched != 1))
                {
                    return Err(
                        format!("unsupported ambiguous or unmatched branch at {path}").into(),
                    );
                }
            }
        }
        // A coupled encoded ceiling bounds the whole subtree, rather than
        // requiring mutually impossible independent field maxima. Its actual
        // maximum and every nested discriminant must still be exercised.
        if self.aggregate_bound.is_none() {
            self.observe_extrema(schema, path, occurrence, value)?;
        }
        if let Some(variants) = object.get("enum") {
            for (index, variant) in variants
                .as_array()
                .ok_or("malformed enum schema")?
                .iter()
                .enumerate()
            {
                self.record(
                    format!("{path}/enum/{index}"),
                    value.map(|value| value == variant),
                );
            }
        }
        if let Some(constant) = object.get("const") {
            self.record(
                format!("{path}/const"),
                value.map(|value| value == constant),
            );
        }
        if let Some(Value::Array(types)) = object.get("type") {
            for (index, kind) in types.iter().enumerate() {
                let kind = kind.as_str().ok_or("malformed type union")?;
                let matches = value.map(|value| type_matches(kind, value)).transpose()?;
                self.record(format!("{path}/type/{index}"), matches);
            }
        }
        if let Some(properties) = object.get("properties") {
            for (name, property) in properties
                .as_object()
                .ok_or("malformed properties schema")?
            {
                let child_path = format!(
                    "{path}/properties/{}",
                    name.replace('~', "~0").replace('/', "~1")
                );
                let child_occurrence = format!(
                    "{occurrence}/{}",
                    name.replace('~', "~0").replace('/', "~1")
                );
                match value {
                    None => {
                        self.walk(property, &child_path, &child_occurrence, None, references)?;
                    }
                    Some(value) => {
                        if let Some(child) = value.get(name) {
                            self.walk(
                                property,
                                &child_path,
                                &child_occurrence,
                                Some(child),
                                references,
                            )?;
                        }
                    }
                }
            }
        }
        if object
            .get("additionalProperties")
            .is_some_and(|value| value != false)
        {
            return Err(
                "dynamic additional properties lack a finite typed coverage inventory".into(),
            );
        }
        if let Some(items) = object.get("items") {
            if self.aggregate_bound.is_some() {
                match value {
                    None => self.walk(
                        items,
                        &format!("{path}/items"),
                        &format!("{occurrence}/items"),
                        None,
                        references,
                    )?,
                    Some(Value::Array(values)) => {
                        for value in values {
                            self.walk(
                                items,
                                &format!("{path}/items"),
                                &format!("{occurrence}/items"),
                                Some(value),
                                references,
                            )?;
                        }
                    }
                    Some(_) => {}
                }
            } else {
                let maximum = object
                    .get("maxItems")
                    .and_then(Value::as_u64)
                    .ok_or("array items require a finite maximum for extrema coverage")?;
                if maximum > 4096 {
                    return Err("array coverage exceeds its explicit test bound".into());
                }
                match value {
                    None => {
                        for index in 0..maximum {
                            self.walk(
                                items,
                                &format!("{path}/items"),
                                &format!("{occurrence}/items/{index}"),
                                None,
                                references,
                            )?;
                        }
                    }
                    Some(Value::Array(values)) => {
                        for (index, value) in values.iter().enumerate() {
                            self.walk(
                                items,
                                &format!("{path}/items"),
                                &format!("{occurrence}/items/{index}"),
                                Some(value),
                                references,
                            )?;
                        }
                    }
                    Some(_) => {}
                }
            }
        }
        if let Some(items) = object.get("prefixItems") {
            for (index, item) in items
                .as_array()
                .ok_or("malformed tuple schema")?
                .iter()
                .enumerate()
            {
                match value {
                    None => self.walk(
                        item,
                        &format!("{path}/prefixItems/{index}"),
                        &format!("{occurrence}/prefixItems/{index}"),
                        None,
                        references,
                    )?,
                    Some(value) => {
                        if let Some(child) = value.get(index) {
                            self.walk(
                                item,
                                &format!("{path}/prefixItems/{index}"),
                                &format!("{occurrence}/prefixItems/{index}"),
                                Some(child),
                                references,
                            )?;
                        }
                    }
                }
            }
        }
        self.aggregate_bound = previous_bound;
        Ok(())
    }

    fn observe_extrema(
        &mut self,
        schema: &Value,
        path: &str,
        occurrence: &str,
        value: Option<&Value>,
    ) -> TestResult {
        if let Some(maximum) = schema
            .get("x-max-utf8-bytes")
            .or_else(|| schema.get("maxLength"))
        {
            let maximum = maximum.as_u64().ok_or("noninteger string bound")?;
            let minimum = minimum_string_length(schema, self.schema)?;
            let width = maximum
                .checked_mul(6)
                .and_then(|n| n.checked_add(2))
                .ok_or("string width overflow")?;
            self.record(
                format!("{occurrence}/extrema/string-min={minimum}"),
                value.map(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.chars().count() as u64 == minimum)
                }),
            );
            self.record(
                format!("{occurrence}/extrema/string-min-utf8={minimum}"),
                value.map(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.len() as u64 == minimum)
                }),
            );
            let minimum_length = usize::try_from(minimum)?;
            if minimum_length > 64 * 1024 {
                return Err("minimum string witness exceeds the bounded proof envelope".into());
            }
            let minimum_width = encoded_width(&json!("a".repeat(minimum_length)))?;
            self.record(
                format!("{occurrence}/extrema/string-min-encoded={minimum_width}"),
                value
                    .map(|value| {
                        encoded_width(value)
                            .map(|width| value.is_string() && width == minimum_width)
                    })
                    .transpose()?,
            );
            self.record(
                format!("{occurrence}/extrema/string-max"),
                value.map(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.len() as u64 == maximum)
                }),
            );
            let encoded_max = value
                .map(|value| encoded_width(value).map(|bytes| bytes as u64 == width))
                .transpose()?;
            self.record(format!("{occurrence}/extrema/string-escaping"), encoded_max);
        }
        if schema_type_contains(schema, "integer") || schema_type_contains(schema, "number") {
            if !self.numeric.contains_key(path) {
                self.numeric
                    .insert(path.to_owned(), numeric_extrema(schema)?);
            }
            let extrema = self
                .numeric
                .get(path)
                .ok_or("numeric extrema missing")?
                .clone();
            let min_value =
                value.map(|value| numeric_equal(value, &extrema.minimum, extrema.float));
            let max_value =
                value.map(|value| numeric_equal(value, &extrema.maximum, extrema.float));
            let min_width = value
                .map(|value| {
                    encoded_width(value)
                        .map(|width| value.is_number() && width == extrema.minimum_width)
                })
                .transpose()?;
            let max_width = value
                .map(|value| {
                    encoded_width(value)
                        .map(|width| value.is_number() && width == extrema.maximum_width)
                })
                .transpose()?;
            self.record(format!("{occurrence}/extrema/numeric-min"), min_value);
            self.record(format!("{occurrence}/extrema/numeric-max"), max_value);
            self.record(format!("{occurrence}/extrema/numeric-min-width"), min_width);
            self.record(format!("{occurrence}/extrema/numeric-max-width"), max_width);
        }
        if let Some(maximum) = schema.get("maxItems") {
            let maximum = maximum.as_u64().ok_or("noninteger array bound")?;
            let minimum = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            for (name, length) in [("array-min", minimum), ("array-max", maximum)] {
                self.record(
                    format!("{occurrence}/extrema/{name}"),
                    value.map(|value| {
                        value
                            .as_array()
                            .is_some_and(|values| values.len() as u64 == length)
                    }),
                );
            }
        }
        Ok(())
    }

    fn branch_matches(
        &self,
        branch: &Value,
        value: &Value,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mut branch = branch
            .as_object()
            .ok_or("unsupported boolean branch")?
            .clone();
        if let Some(definitions) = self.schema.get("$defs") {
            branch.insert("$defs".into(), definitions.clone());
        }
        if let Some(draft) = self.schema.get("$schema") {
            branch.insert("$schema".into(), draft.clone());
        }
        Ok(jsonschema::validator_for(&Value::Object(branch))?.is_valid(value))
    }
}

#[derive(Clone)]
struct NumericExtrema {
    minimum: Value,
    maximum: Value,
    minimum_width: usize,
    maximum_width: usize,
    float: bool,
}

fn schema_type_contains(schema: &Value, kind: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(value)) => value == kind,
        Some(Value::Array(values)) => values.iter().any(|value| value == kind),
        _ => false,
    }
}

fn minimum_string_length(schema: &Value, root: &Value) -> Result<u64, Box<dyn std::error::Error>> {
    let mut minimum = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0);
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or("external string schema reference")?;
        minimum = minimum.max(minimum_string_length(
            root.pointer(pointer)
                .ok_or("missing string schema reference")?,
            root,
        )?);
    }
    if let Some(parts) = schema.get("allOf") {
        for part in parts.as_array().ok_or("malformed string intersection")? {
            minimum = minimum.max(minimum_string_length(part, root)?);
        }
    }
    Ok(minimum)
}

fn encoded_width(value: &Value) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(value)?.len())
}

fn numeric_equal(value: &Value, expected: &Value, float: bool) -> bool {
    if float {
        value
            .as_f64()
            .zip(expected.as_f64())
            .is_some_and(|(a, b)| a == b)
    } else {
        value == expected
    }
}

fn numeric_extrema(schema: &Value) -> Result<NumericExtrema, Box<dyn std::error::Error>> {
    if ["exclusiveMinimum", "exclusiveMaximum", "multipleOf"]
        .iter()
        .any(|key| schema.get(key).is_some())
    {
        return Err("restricted numeric lattice needs explicit extrema support".into());
    }
    let format = schema.get("format").and_then(Value::as_str);
    if schema_type_contains(schema, "integer") {
        let (default_min, default_max) = match format {
            Some("uint8") => (json!(0), json!(u8::MAX)),
            Some("uint32") => (json!(0), json!(u32::MAX)),
            Some("uint64") => (json!(0), json!(u64::MAX)),
            Some("int64") => (json!(i64::MIN), json!(i64::MAX)),
            _ => (Value::Null, Value::Null),
        };
        let minimum = schema.get("minimum").unwrap_or(&default_min).clone();
        let maximum = schema.get("maximum").unwrap_or(&default_max).clone();
        if !(minimum.is_i64() || minimum.is_u64()) || !(maximum.is_i64() || maximum.is_u64()) {
            return Err("integer extrema need exact finite integer bounds".into());
        }
        let min_width = encoded_width(&minimum)?;
        let max_width = encoded_width(&maximum)?;
        let includes_zero =
            minimum.as_f64().ok_or("minimum")? <= 0.0 && maximum.as_f64().ok_or("maximum")? >= 0.0;
        return Ok(NumericExtrema {
            minimum,
            maximum,
            minimum_width: if includes_zero {
                1
            } else {
                min_width.min(max_width)
            },
            maximum_width: min_width.max(max_width),
            float: false,
        });
    }
    if format != Some("double") {
        return Err("floating extrema need the concrete f64 serialization contract".into());
    }
    let minimum = schema
        .get("minimum")
        .and_then(Value::as_f64)
        .ok_or("missing float minimum")?;
    let maximum = schema
        .get("maximum")
        .and_then(Value::as_f64)
        .ok_or("missing float maximum")?;
    if !minimum.is_finite() || !maximum.is_finite() || minimum > maximum {
        return Err("invalid finite floating bounds".into());
    }
    let mut minimum_width = usize::MAX;
    let mut maximum_width = 0;
    let mut measure = |value: f64| -> TestResult {
        if value.is_finite() && value >= minimum && value <= maximum {
            let width = encoded_width(&json!(value))?;
            minimum_width = minimum_width.min(width);
            maximum_width = maximum_width.max(width);
        }
        Ok(())
    };
    measure(minimum)?;
    measure(maximum)?;
    // Inspect shortest-formatter exponent and significand boundaries instead
    // of assuming that the numerically largest float has the widest spelling.
    for exponent in 0_u64..2047 {
        for mantissa in [0, 1, (1_u64 << 52) - 1, 0x9_e377_9b97_f4a7] {
            let value = f64::from_bits((exponent << 52) | mantissa);
            measure(value)?;
            measure(-value)?;
        }
    }
    Ok(NumericExtrema {
        minimum: json!(minimum),
        maximum: json!(maximum),
        minimum_width,
        maximum_width,
        float: true,
    })
}

fn type_matches(kind: &str, value: &Value) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => return Err(format!("unsupported schema type {kind}").into()),
    })
}
