//! Shared JSON schema helpers for tool definitions.
//!
//! The emission itself is owned by the core schema module
//! ([`meerkat_core::schema::tool_input_schema_for`]) — the single infallible
//! `schema_for!` → `Value` seam in the workspace. This module re-exposes it
//! under the historical `schema_for` name for tool crates.

use schemars::JsonSchema;
use serde_json::Value;

pub use meerkat_core::schema::tool_input_schema_for;

/// Infallible tool-input schema for `T`. See
/// [`meerkat_core::schema::tool_input_schema_for`].
pub fn schema_for<T: JsonSchema>() -> Value {
    tool_input_schema_for::<T>()
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
