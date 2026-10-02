use meerkat_contracts::wire::{MobToolConfigInput, WireMobToolConfig};

#[test]
fn wire_mob_tool_config_image_generation_roundtrips_and_defaults() -> serde_json::Result<()> {
    let tools = WireMobToolConfig {
        workgraph: true,
        image_generation: true,
        ..WireMobToolConfig::default()
    };
    let encoded = serde_json::to_string(&tools)?;
    let decoded: WireMobToolConfig = serde_json::from_str(&encoded)?;
    assert!(decoded.workgraph);
    assert!(decoded.image_generation);

    let decoded_legacy: WireMobToolConfig = serde_json::from_str("{}")?;
    assert!(!decoded_legacy.workgraph);
    assert!(!decoded_legacy.image_generation);
    Ok(())
}

#[test]
fn wire_mob_tool_config_deny_roundtrips_and_is_omitted_when_empty() -> serde_json::Result<()> {
    let tools = WireMobToolConfig {
        mob: true,
        deny: vec!["mob_wire".to_string(), "mob_unwire".to_string()],
        ..WireMobToolConfig::default()
    };
    let decoded: WireMobToolConfig = serde_json::from_str(&serde_json::to_string(&tools)?)?;
    assert_eq!(decoded.deny, tools.deny);

    let open = serde_json::to_value(WireMobToolConfig::default())?;
    assert!(
        open.get("deny").is_none(),
        "empty deny must be omitted: {open}"
    );
    let decoded_legacy: WireMobToolConfig = serde_json::from_str("{}")?;
    assert!(decoded_legacy.deny.is_empty());
    Ok(())
}

#[test]
fn mob_tool_config_input_rust_bundles_roundtrip_and_are_omitted_when_empty()
-> serde_json::Result<()> {
    let tools = MobToolConfigInput {
        comms: true,
        rust_bundles: vec!["child-probe".to_string()],
        ..MobToolConfigInput::default()
    };
    let decoded: MobToolConfigInput = serde_json::from_str(&serde_json::to_string(&tools)?)?;
    assert_eq!(decoded.rust_bundles, tools.rust_bundles);

    // Payloads written before the field existed decode unchanged, and an
    // empty list is not serialized, so existing bytes stay identical.
    let legacy: MobToolConfigInput = serde_json::from_str(r#"{"comms":true}"#)?;
    assert!(legacy.rust_bundles.is_empty());
    let encoded = serde_json::to_value(&legacy)?;
    assert!(
        encoded.get("rust_bundles").is_none(),
        "empty rust_bundles must be omitted: {encoded}"
    );
    Ok(())
}
