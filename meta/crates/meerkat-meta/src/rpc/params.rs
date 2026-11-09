//! Shared parameter extraction helpers for RPC handlers.
//!
//! Every extractor returns a concrete error string on failure so callers can
//! wrap it in a `-32602 Invalid params` JSON-RPC error without duplicating
//! boilerplate.

use serde_json::Value;

/// Extract a required array-of-strings field.  Rejects missing fields,
/// non-array values, and non-string entries (no silent filtering).
pub fn required_string_array(params: &Value, field: &str) -> Result<Vec<String>, String> {
    let arr = params
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{field} must be an array of strings"))?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, entry) in arr.iter().enumerate() {
        let s = entry.as_str().ok_or_else(|| {
            format!(
                "{field}[{i}] must be a string, got {}",
                entry_type_name(entry)
            )
        })?;
        out.push(s.to_string());
    }
    Ok(out)
}

fn entry_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
