//! Shared JSON schema helpers for tool definitions.

use schemars::JsonSchema;
use serde_json::{Map, Value};

pub fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    // `schemars::Schema` is `#[repr(transparent)] struct Schema(Value)`; `to_value`
    // hands back the schema's own inner JSON value directly. Use it instead of a
    // fallible `serde_json::to_value(..).unwrap_or(Value::Null)` round-trip: there
    // is no serialization step to fail and therefore no fail-open path that could
    // launder a failure into a `null` schema. A tool can never advertise
    // `input_schema: null` from this helper.
    let mut value = schema.to_value();

    // Some generators omit empty `properties`/`required` for `{}`.
    // Our tool schema contract expects explicit presence of both keys.
    if let Value::Object(ref mut obj) = value
        && obj.get("type").and_then(Value::as_str) == Some("object")
    {
        obj.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        obj.entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    value
}

#[derive(Debug, Clone, Copy, JsonSchema)]
struct EmptyObject {}

pub fn empty_object_schema() -> Value {
    schema_for::<EmptyObject>()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct PopulatedArgs {
        prompt: String,
        count: u32,
    }

    /// `schema_for` must never produce a `null` schema: the helper has no
    /// fail-open serialization path, so a tool can never advertise
    /// `input_schema: null`. Covers both a populated struct and the
    /// empty-object schema.
    #[test]
    fn schema_for_is_never_null() {
        let populated = schema_for::<PopulatedArgs>();
        assert_ne!(populated, Value::Null);
        assert!(
            populated.is_object(),
            "populated schema must be a JSON object, got {populated}"
        );

        let empty = empty_object_schema();
        assert_ne!(empty, Value::Null);
        assert!(
            empty.is_object(),
            "empty-object schema must be a JSON object, got {empty}"
        );
    }

    /// Object schemas always carry explicit `properties` and `required` keys,
    /// per the tool schema contract.
    #[test]
    fn object_schema_has_explicit_properties_and_required() {
        let empty = empty_object_schema();
        let obj = empty
            .as_object()
            .expect("empty-object schema must be a JSON object");
        assert_eq!(obj.get("type").and_then(Value::as_str), Some("object"));
        assert!(obj.contains_key("properties"));
        assert!(obj.contains_key("required"));
    }
}
