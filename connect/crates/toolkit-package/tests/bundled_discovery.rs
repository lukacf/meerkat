use toolkit_package::{
    CatalogOrigin, ExportRole, Id, IntegrationPackage, SupportStatus, bundled_discovery_catalog,
};

#[derive(serde::Deserialize)]
struct SourceObservation {
    url: String,
    final_url: Option<String>,
    sha256: String,
}

#[test]
fn every_bundled_strategy_source_has_an_exact_assessment_observation() {
    let observations: std::collections::BTreeMap<String, SourceObservation> =
        serde_json::from_str(include_str!("../catalog/source-observations.json")).unwrap();
    let catalog = bundled_discovery_catalog().unwrap();
    for entry in catalog.entries() {
        let evidence: Vec<_> = entry
            .package
            .assessments
            .iter()
            .flat_map(|assessment| &assessment.evidence)
            .map(|reference| {
                assert_eq!(reference.owner.as_str(), "toolkit-source-observations");
                let observed = observations.get(reference.record.as_str()).unwrap();
                assert_eq!(
                    reference.revision.as_str(),
                    format!("sha256:{}", observed.sha256)
                );
                observed
            })
            .collect();
        for strategy in &entry.package.strategies {
            for source in &strategy.sources {
                assert!(
                    evidence.iter().any(|observed| {
                        observed.url == source.as_str()
                            || observed.final_url.as_deref() == Some(source.as_str())
                    }),
                    "unassessed strategy source: {} / {} / {}",
                    entry.package.identity.name.as_str(),
                    strategy.id.as_str(),
                    source
                );
            }
        }
    }
}

#[test]
fn bundled_discovery_is_inert_and_never_claims_tested_support() {
    let catalog = bundled_discovery_catalog().expect("valid bundled inventory");
    let entries: Vec<_> = catalog.entries().collect();
    assert_eq!(entries.len(), 9);
    for entry in &entries {
        assert_eq!(
            entry.package.identity.publisher,
            Id::new("meerkat-toolkit").unwrap()
        );
        assert!(matches!(entry.origin, CatalogOrigin::Curated { .. }));
        assert_eq!(entry.digest, entry.package.content_digest().unwrap());
        assert!(!entry.package.assessments.is_empty());
        assert!(entry.package.assessments.iter().all(|assessment| {
            assessment.status == SupportStatus::Experimental && !assessment.evidence.is_empty()
        }));
        assert!(entry.package.strategies.iter().all(|strategy| {
            !strategy.sources.is_empty() && !strategy.unresolved_details.is_empty()
        }));
        let encoded = serde_json::to_vec(&entry.package).unwrap();
        let decoded: IntegrationPackage = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(entry.package, decoded);
    }

    for dual_role in ["slack"] {
        let package = &entries
            .iter()
            .find(|entry| entry.package.identity.name.as_str() == dual_role)
            .expect("dual-role recipe")
            .package;
        assert_eq!(package.known_exports.len(), 2);
        assert!(
            package
                .known_exports
                .iter()
                .any(|export| export.role == ExportRole::ServiceConnector)
        );
        assert!(
            package
                .known_exports
                .iter()
                .any(|export| export.role == ExportRole::AgentFrontend)
        );
    }
}
