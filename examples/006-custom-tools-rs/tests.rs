#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

async fn weather(unit: Option<&str>) -> Result<ToolDispatchOutcome, ToolError> {
    let mut args = json!({"city": "Tokyo"});
    if let Some(unit) = unit {
        args["unit"] = json!(unit);
    }
    let args = serde_json::value::to_raw_value(&args).unwrap();
    WeatherAndConvertDispatcher
        .dispatch(ToolCallView {
            id: "weather-test",
            name: "get_weather",
            args: &args,
        })
        .await
}

#[tokio::test]
async fn weather_units_default_convert_and_reject_unknown_values() {
    for (unit, temperature, label) in [
        (None, 28.0, "celsius"),
        (Some("celsius"), 28.0, "celsius"),
        (Some("fahrenheit"), 82.4, "fahrenheit"),
    ] {
        let outcome = weather(unit).await.unwrap();
        assert!(!outcome.result.is_error);
        let value: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(value["unit"], label);
        assert!((value["temperature"].as_f64().unwrap() - temperature).abs() < 1e-8);
    }
    for unit in ["kelvin", "celcius", "Celsius", ""] {
        assert!(matches!(
            weather(Some(unit)).await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }
}

#[test]
fn weather_schema_restricts_unit_wire_values() {
    let tools = WeatherAndConvertDispatcher.tools();
    let schema = &tools
        .iter()
        .find(|tool| tool.name == "get_weather")
        .unwrap()
        .input_schema;
    let unit = &schema["properties"]["unit"];
    let definition = unit.get("$ref").map_or(unit, |reference| {
        schema
            .pointer(reference.as_str().unwrap().strip_prefix('#').unwrap())
            .unwrap()
    });
    assert_eq!(definition["enum"], json!(["celsius", "fahrenheit"]));
    assert!(
        !schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("unit"))
    );
}
