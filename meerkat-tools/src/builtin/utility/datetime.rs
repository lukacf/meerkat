//! DateTime tool for getting current date and time

use crate::builtin::{BuiltinTool, BuiltinToolError};
use crate::schema::empty_object_schema;
use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde_json::{Value, json};

/// Tool for getting the current date and time
///
/// Returns the current date and time in ISO 8601 format along with
/// Unix timestamp for programmatic use.
#[derive(Debug, Clone)]
pub struct DateTimeTool;

impl DateTimeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DateTimeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for DateTimeTool {
    fn name(&self) -> &'static str {
        "datetime"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "datetime".into(),
            description: "Get the current date and time. Returns ISO 8601 formatted datetime and Unix timestamp.".into(),
            input_schema: empty_object_schema(),
        }
    }

    fn default_enabled(&self) -> bool {
        true // Utility tools enabled by default
    }

    async fn call(&self, _args: Value) -> Result<Value, BuiltinToolError> {
        use meerkat_core::time_compat::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now();
        let unix_timestamp = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Format as ISO 8601
        let datetime = chrono::Utc::now();
        let iso8601 = datetime.to_rfc3339();
        let date = datetime.format("%Y-%m-%d").to_string();
        let time = datetime.format("%H:%M:%S").to_string();
        let timezone = "UTC";

        Ok(json!({
            "iso8601": iso8601,
            "date": date,
            "time": time,
            "timezone": timezone,
            "unix_timestamp": unix_timestamp,
            "year": datetime.format("%Y").to_string(),
            "month": datetime.format("%m").to_string(),
            "day": datetime.format("%d").to_string(),
            "weekday": datetime.format("%A").to_string()
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_datetime_tool_name() {
        let tool = DateTimeTool::new();
        assert_eq!(tool.name(), "datetime");
    }

    #[test]
    fn test_datetime_tool_default_enabled() {
        let tool = DateTimeTool::new();
        assert!(tool.default_enabled());
    }

    #[test]
    fn test_datetime_tool_def() {
        let tool = DateTimeTool::new();
        let def = tool.def();
        assert_eq!(def.name, "datetime");
        assert!(def.description.contains("current date and time"));
    }

    #[tokio::test]
    async fn test_datetime_tool_returns_valid_data() {
        let tool = DateTimeTool::new();
        let result = tool.call(json!({})).await.unwrap();

        // Check all expected fields are present
        assert!(result.get("iso8601").is_some());
        assert!(result.get("date").is_some());
        assert!(result.get("time").is_some());
        assert!(result.get("timezone").is_some());
        assert!(result.get("unix_timestamp").is_some());
        assert!(result.get("year").is_some());
        assert!(result.get("month").is_some());
        assert!(result.get("day").is_some());
        assert!(result.get("weekday").is_some());
    }

    #[tokio::test]
    async fn test_datetime_tool_iso8601_format() {
        let tool = DateTimeTool::new();
        let result = tool.call(json!({})).await.unwrap();

        let iso8601 = result["iso8601"].as_str().unwrap();
        // Basic ISO 8601 format check: contains T and ends with Z or timezone
        assert!(iso8601.contains('T'));
    }

    #[tokio::test]
    async fn test_datetime_tool_unix_timestamp_reasonable() {
        let tool = DateTimeTool::new();
        let result = tool.call(json!({})).await.unwrap();

        let timestamp = result["unix_timestamp"].as_u64().unwrap();
        // Should be after 2020 and before 2100
        assert!(timestamp > 1577836800); // 2020-01-01
        assert!(timestamp < 4102444800); // 2100-01-01
    }
}
