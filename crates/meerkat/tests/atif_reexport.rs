//! Compile-proof for the gated `meerkat::atif` re-export.
//!
//! The facade's ATIF dependency is optional (feature `atif`); this battery is
//! the feature-on half of that contract. With the feature off the file compiles
//! to nothing, which is the other half.

#![cfg(feature = "atif")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
fn facade_atif_feature_exposes_the_export_vocabulary() {
    assert_eq!(meerkat::atif::SCHEMA_VERSION, "ATIF-v1.7");
    let trajectory = meerkat::atif::TrajectoryBuilder::new()
        .with_session_id("facade-session")
        .finish(meerkat::atif::Agent {
            name: "meerkat".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            model_name: None,
            tool_definitions: None,
            extra: None,
        });
    assert_eq!(trajectory.session_id.as_deref(), Some("facade-session"));
    assert!(trajectory.steps.is_empty());
}
