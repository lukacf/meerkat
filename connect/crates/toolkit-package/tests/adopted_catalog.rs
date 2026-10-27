use toolkit_package::{
    CatalogError, CatalogOrigin, ExportRole, SupportStatus, bundled_adopted_catalog,
    bundled_discovery_catalog,
};

#[test]
fn adopted_catalog_preserves_exact_scoped_records() {
    let catalog = bundled_adopted_catalog().expect("valid generated catalog");
    let source: Vec<toolkit_package::IntegrationPackage> =
        serde_json::from_str(include_str!("../catalog/adopted.json")).unwrap();
    assert_eq!(catalog.entries().count(), source.len());
    assert!(!source.is_empty());
    for package in source {
        let entry = catalog.get(&package.identity).unwrap();
        assert_eq!(entry.package, package);
        assert_eq!(entry.digest, package.content_digest().unwrap());
        assert!(
            package
                .identity
                .version
                .as_str()
                .starts_with("metadata-sha256:")
        );
        assert!(matches!(entry.origin, CatalogOrigin::Curated { .. }));
        assert!(
            package
                .known_exports
                .iter()
                .all(|export| export.role == ExportRole::ServiceConnector)
        );
        assert!(package.assessments.iter().all(|assessment| {
            assessment.status == SupportStatus::Compatible && assessment.evidence.len() >= 4
        }));
        assert!(
            package
                .strategies
                .iter()
                .all(|strategy| !strategy.unresolved_details.is_empty())
        );
        // This stays well below DiscoveryTools' per-entry metadata bound.
        assert!(serde_json::to_vec(entry).unwrap().len() < 32 * 1024);
    }
}

#[test]
fn changed_metadata_cannot_overwrite_same_version() {
    let mut catalog = bundled_adopted_catalog().unwrap();
    let entry = catalog.entries().next().unwrap().clone();
    let mut changed = entry.package;
    changed.description.push_str(" Changed support claim.");
    assert_eq!(
        catalog.insert(changed, entry.origin),
        Err(CatalogError::ImmutableConflict)
    );
}

#[test]
fn adopted_catalog_does_not_relabel_legacy_research() {
    let legacy = bundled_discovery_catalog().unwrap();
    assert_eq!(legacy.entries().count(), 10);
    assert!(legacy.entries().all(|entry| {
        entry
            .package
            .assessments
            .iter()
            .all(|assessment| assessment.status == SupportStatus::Experimental)
    }));
    let adopted = bundled_adopted_catalog().unwrap();
    assert!(
        adopted
            .entries()
            .all(|entry| legacy.get(&entry.package.identity).is_none())
    );
}

#[test]
fn service_alternatives_preserve_primary_adoption_and_scoped_effects() {
    let catalog = bundled_adopted_catalog().unwrap();
    assert_eq!(catalog.entries().count(), 101);
    let alternatives: Vec<_> = catalog
        .entries()
        .filter(|entry| entry.package.strategies.len() > 1)
        .collect();
    assert_eq!(
        alternatives
            .iter()
            .map(|entry| entry.package.identity.name.as_str())
            .collect::<Vec<_>>(),
        ["gmail", "google-calendar", "google-drive", "slack"]
    );
    for entry in alternatives {
        let package = &entry.package;
        assert_eq!(package.strategies[0].id.as_str(), "selected-local-mcp");
        assert_eq!(
            package.strategies[1].id.as_str(),
            "first-party-services-mcp"
        );
        assert_eq!(package.provenance.source.host_str(), Some("github.com"));
        assert_eq!(
            package.provenance.source.path(),
            "/activepieces/activepieces"
        );
        assert_eq!(package.assessments.len(), 2);
        assert!(
            package.assessments[0]
                .scope
                .starts_with("Strategy selected-local-mcp only.")
        );
        assert!(
            package.assessments[1]
                .scope
                .starts_with("Strategy first-party-services-mcp only.")
        );
        assert!(
            package.assessments[1]
                .source_revision
                .as_str()
                .starts_with("sha256:")
        );
        for export in &package.known_exports[1..] {
            let effect = if matches!(
                export.id.as_str(),
                "gmail_send_raw"
                    | "gmail_create_draft_raw"
                    | "gmail_send_message"
                    | "gmail_create_draft_message"
                    | "google-calendar_insert_event"
                    | "google-calendar_patch_event"
                    | "google-calendar_delete_event"
            ) {
                "write"
            } else {
                "read"
            };
            assert!(
                export.description.starts_with(&format!(
                    "Strategy first-party-services-mcp only; effect {effect}."
                )),
                "{} export {} should have effect {effect}: {}",
                package.identity.name.as_str(),
                export.id.as_str(),
                export.description
            );
        }
    }
}
