#![allow(clippy::expect_used)]

//! `[storage] hosting` (#1813): the realm's hosting mode is a typed, layered
//! realm option. A child layer's explicit value, including the default
//! `single_process`, overrides its parent's; a layer that does not name it
//! inherits; absent everywhere it is `single_process`.

use meerkat_core::Config;
use meerkat_core::config::HostingMode;

#[test]
fn hosting_mode_is_layered_with_the_child_winning() {
    let mut config = Config::default();
    assert_eq!(config.storage.hosting_mode(), HostingMode::SingleProcess);
    config
        .merge_toml_str("[storage]\nhosting = \"multiprocess\"\n")
        .expect("parent layer");
    assert_eq!(config.storage.hosting_mode(), HostingMode::Multiprocess);
    config
        .merge_toml_str("[storage]\n")
        .expect("a layer that does not name the hosting mode");
    assert_eq!(
        config.storage.hosting_mode(),
        HostingMode::Multiprocess,
        "a child that says nothing inherits"
    );
    config
        .merge_toml_str("[storage]\nhosting = \"single_process\"\n")
        .expect("child layer");
    assert_eq!(
        config.storage.hosting_mode(),
        HostingMode::SingleProcess,
        "an explicit default in the child overrides the parent"
    );
}

#[test]
fn an_unknown_hosting_mode_is_refused() {
    let mut config = Config::default();
    assert!(
        config
            .merge_toml_str("[storage]\nhosting = \"several\"\n")
            .is_err(),
        "the mode is a closed, typed vocabulary"
    );
}
