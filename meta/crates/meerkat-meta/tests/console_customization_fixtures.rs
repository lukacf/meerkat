use meerkat_meta::console_config::load_console_ui_config_from_toml;

#[test]
fn configured_host_shell_fixture_preserves_stock_console_config_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let config = load_console_ui_config_from_toml(include_str!(
        "../../../console/fixtures/configured-host-shell/config/console.toml"
    ))?;

    assert_eq!(config.title.as_deref(), Some("Configured Host Console"));
    assert_eq!(config.brand.label.as_deref(), Some("Configured Host"));
    assert_eq!(config.layout.initial_control.as_deref(), Some("roster"));
    assert_eq!(
        config.layout.initial_agent.as_deref(),
        Some("identity:lead")
    );
    assert_eq!(config.sidebar.hidden_controls, vec!["gating"]);
    assert_eq!(config.sidebar.buttons.len(), 2);
    assert_eq!(config.sidebar.buttons[0].id, "host-dashboard");
    assert_eq!(
        config.sidebar.buttons[0].href.as_deref(),
        Some("/host/dashboard")
    );
    assert_eq!(config.sidebar.buttons[0].control, None);
    assert_eq!(config.sidebar.buttons[1].id, "routing-control");
    assert_eq!(
        config.sidebar.buttons[1].control.as_deref(),
        Some("routing")
    );
    assert_eq!(config.sidebar.buttons[1].href, None);
    assert_eq!(
        config.agent_list.group_by,
        vec!["labels.console_section", "group", "role"]
    );
    assert_eq!(config.agent_list.subgroup_by, vec!["labels.scope"]);
    assert_eq!(
        config.agent_list.section_order,
        vec!["Pinned", "Projects", "Workers", "Other"]
    );
    assert_eq!(
        config.agent_list.default_pinned_agent_ids,
        vec!["identity:lead", "member:worker-1"]
    );
    assert_eq!(config.agent_list.sections[1].name, "Workers");
    assert_eq!(config.agent_list.sections[1].collapsed, Some(true));
    assert_eq!(config.actions.show_respawn, Some(false));
    assert_eq!(config.actions.show_retire, Some(true));
    assert_eq!(config.actions.show_reset, Some(false));
    Ok(())
}
