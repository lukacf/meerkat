use meerkat_contracts::wire::WireMobToolConfig;

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

/// MG P1/P4t: a wire profile's tool config carries its tool grant gate, and
/// omits the empty gate, so an ungated profile keeps the bytes an older
/// client parses; a gated profile sent to an older server is refused there,
/// never read ungated.
#[test]
fn wire_mob_tool_configs_carry_their_grant_gate_and_omit_the_empty_gate() -> serde_json::Result<()>
{
    let gated = serde_json::json!({
        "comms": true,
        "grant_gated": {"families": ["shell"], "tools": ["deploy"]},
    });
    let tools: WireMobToolConfig = serde_json::from_value(gated.clone())?;
    let encoded = serde_json::to_value(&tools)?;
    assert_eq!(encoded["grant_gated"], gated["grant_gated"]);
    let open = serde_json::to_value(WireMobToolConfig::default())?;
    assert!(
        open.get("grant_gated").is_none(),
        "an empty gate is omitted: {open}"
    );

    let input: meerkat_contracts::wire::MobToolConfigInput = serde_json::from_value(gated.clone())?;
    let encoded = serde_json::to_value(&input)?;
    assert_eq!(encoded["grant_gated"], gated["grant_gated"]);
    let open = serde_json::to_value(meerkat_contracts::wire::MobToolConfigInput::default())?;
    assert!(
        open.get("grant_gated").is_none(),
        "an empty gate is omitted: {open}"
    );
    Ok(())
}
