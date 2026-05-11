#[test]
fn rpc_crate_does_not_depend_directly_on_webrtc_media_stack() {
    let cargo_toml = include_str!("../Cargo.toml");
    for forbidden in ["webrtc", "webrtc-media", "opus", "rubato"] {
        let direct_dep_prefix = format!("{forbidden} = ");
        let direct_dep = cargo_toml
            .lines()
            .map(str::trim_start)
            .find(|line| line.starts_with(&direct_dep_prefix));
        assert!(
            direct_dep.is_none(),
            "meerkat-rpc must not directly depend on {forbidden}; \
             route media through meerkat-live/webrtc instead: {direct_dep:?}"
        );
    }
    assert!(
        cargo_toml.contains("live-webrtc = [\"meerkat-live/webrtc\"]"),
        "meerkat-rpc should expose WebRTC only by forwarding the meerkat-live feature"
    );
}
