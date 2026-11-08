//! The component lifetime owner renders for the driver composition.

use toolkit_catalog::{component_lifetime_model_artifacts, component_lifetime_schema};

#[test]
fn component_lifetime_owner_renders_every_transition_with_empty_strings() {
    let artifacts = component_lifetime_model_artifacts().expect("the lifetime owner renders");
    for transition in component_lifetime_schema().transitions {
        assert!(
            artifacts
                .owner_model
                .contains(&format!("{}(", transition.name.as_str())),
            "{} is rendered",
            transition.name.as_str()
        );
    }
    for config in [&artifacts.ci_config, &artifacts.deep_config] {
        let strings = config
            .lines()
            .find(|line| line.starts_with("  StringValues = {"))
            .expect("one string domain");
        assert!(
            strings.contains("\"\""),
            "close and component identities need the empty string"
        );
        assert!(
            !config.contains("CHECK_DEADLOCK"),
            "generic deadlock checking stays on"
        );
    }
}
