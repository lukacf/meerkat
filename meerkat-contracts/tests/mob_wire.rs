use meerkat_contracts::wire::WireMobToolConfig;

#[test]
fn wire_mob_tool_config_image_generation_roundtrips_and_defaults() -> serde_json::Result<()> {
    let tools = WireMobToolConfig {
        image_generation: true,
        ..WireMobToolConfig::default()
    };
    let encoded = serde_json::to_string(&tools)?;
    let decoded: WireMobToolConfig = serde_json::from_str(&encoded)?;
    assert!(decoded.image_generation);

    let decoded_legacy: WireMobToolConfig = serde_json::from_str("{}")?;
    assert!(!decoded_legacy.image_generation);
    Ok(())
}
