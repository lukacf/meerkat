//! Isolated public-only consumer construction, without workspace dev-feature
//! unification or an injected provider factory.

use std::path::PathBuf;

pub fn ordinary_factory(storage_root: PathBuf) -> meerkat::AgentFactory {
    meerkat::AgentFactory::new(storage_root.clone())
        .runtime_root(storage_root)
        .builtins(true)
        .shell(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_profile_configuration_carries_no_default_activation() {
        let config = meerkat::Config::default();
        assert!(config.live.is_empty());
        let _: fn(std::path::PathBuf) -> meerkat::AgentFactory = super::ordinary_factory;
    }
}
