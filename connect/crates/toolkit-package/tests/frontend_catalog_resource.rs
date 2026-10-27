use toolkit_package::{
    CatalogEntry, ContentDigest, FRONTEND_CATALOG_MAX_BYTES, bundled_discovery_catalog,
    frontend_catalog_observation,
};

#[test]
fn packaged_python_resource_is_the_exact_canonical_projection_and_pin() {
    let catalog = bundled_discovery_catalog().unwrap();
    let observation = frontend_catalog_observation(&catalog);
    let canonical = serde_json::to_vec(&observation).unwrap();
    assert!(!observation.entries.is_empty());
    assert!(canonical.len() <= FRONTEND_CATALOG_MAX_BYTES);
    let resource =
        include_bytes!("../../../sdk/python/src/meerkat_toolkit/data/frontend_catalog.json");
    let pin = include_str!("../../../sdk/python/src/meerkat_toolkit/data/frontend_catalog.sha256");
    assert_eq!(resource.as_slice(), canonical);
    assert_eq!(
        pin,
        format!("{}\n", ContentDigest::of_bytes(resource).as_str())
    );

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Observation {
        entries: Vec<CatalogEntry>,
    }
    let roundtrip: Observation = serde_json::from_slice(resource).unwrap();
    assert_eq!(
        roundtrip.entries,
        observation.entries.into_iter().cloned().collect::<Vec<_>>()
    );
    for entry in &roundtrip.entries {
        assert_eq!(entry.digest, entry.package.content_digest().unwrap());
    }
}
