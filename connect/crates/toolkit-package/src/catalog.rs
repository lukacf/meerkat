use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

impl IntegrationPackage {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != 1 {
            return Err(ValidationError::new(
                "schema_version",
                "unsupported package schema",
            ));
        }
        crate::manifest::nonempty(&self.display_name, "display_name", "display name is empty")?;
        crate::manifest::nonempty(&self.description, "description", "description is empty")?;
        crate::manifest::nonempty(
            &self.provenance.license,
            "provenance.license",
            "source license is required",
        )?;
        crate::manifest::validate_url(&self.provenance.source, "provenance.source", true)?;
        if self.strategies.is_empty() {
            return Err(ValidationError::new(
                "strategies",
                "at least one discovery or setup strategy is required",
            ));
        }
        let mut strategies = std::collections::BTreeSet::new();
        for strategy in &self.strategies {
            if !strategies.insert(&strategy.id) {
                return Err(ValidationError::new(
                    "strategies",
                    "duplicate strategy identity",
                ));
            }
            crate::manifest::nonempty(
                &strategy.instructions,
                "strategy.instructions",
                "agent guidance is required",
            )?;
            for source in &strategy.sources {
                crate::manifest::validate_url(source, "strategy.sources", true)?;
            }
            for requirement in &strategy.known_requirements {
                crate::manifest::validate_service(requirement, "strategy.known_requirements")?;
            }
            for unknown in &strategy.unresolved_details {
                crate::manifest::nonempty(
                    unknown,
                    "strategy.unresolved_details",
                    "unresolved details must be named",
                )?;
            }
        }
        let mut exports = std::collections::BTreeSet::new();
        for export in &self.known_exports {
            if !exports.insert(&export.id) {
                return Err(ValidationError::new(
                    "known_exports",
                    "duplicate export hint identity",
                ));
            }
            crate::manifest::nonempty(
                &export.description,
                "known_exports",
                "export hint description is empty",
            )?;
        }
        for assessment in &self.assessments {
            crate::manifest::nonempty(
                &assessment.scope,
                "assessments",
                "support assessment scope is required",
            )?;
            if matches!(
                assessment.status,
                SupportStatus::Supported | SupportStatus::Compatible
            ) && assessment.evidence.is_empty()
            {
                return Err(ValidationError::new(
                    "assessments",
                    "supported and compatible claims require validation evidence",
                ));
            }
        }
        Ok(())
    }

    pub fn content_digest(&self) -> Result<ContentDigest, ValidationError> {
        self.validate()?;
        canonical_json(self)
            .map(|bytes| ContentDigest::of_bytes(&bytes))
            .map_err(|_| ValidationError::new("package", "canonical serialization failed"))
    }
}

/// Immutable records only. Catalog lookup returns facts for agents to inspect;
/// there is deliberately no intent classifier, score, fuzzy resolver, or default
/// package choice. Registries can implement retrieval as an independent index.
#[derive(Debug, Clone, Default)]
pub struct PackageCatalog {
    entries: BTreeMap<PackageIdentity, CatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub package: IntegrationPackage,
    pub digest: ContentDigest,
    pub origin: CatalogOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogOrigin {
    Curated {
        catalog: Id,
        revision: Revision,
    },
    LocalInvestigation {
        decision: EvidenceReference,
    },
    Imported {
        format: Id,
        source_digest: ContentDigest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("package identity already exists with different immutable content")]
    ImmutableConflict,
}

impl PackageCatalog {
    /// Inserting local research never promotes it into a curated catalog. The
    /// catalog origin and assessment must be explicitly supplied by their owner.
    pub fn insert(
        &mut self,
        package: IntegrationPackage,
        origin: CatalogOrigin,
    ) -> Result<ContentDigest, CatalogError> {
        let digest = package.content_digest()?;
        if let Some(existing) = self.entries.get(&package.identity) {
            if existing.digest != digest {
                return Err(CatalogError::ImmutableConflict);
            }
            return Ok(existing.digest.clone());
        }
        self.entries.insert(
            package.identity.clone(),
            CatalogEntry {
                package,
                digest: digest.clone(),
                origin,
            },
        );
        Ok(digest)
    }

    pub fn get(&self, identity: &PackageIdentity) -> Option<&CatalogEntry> {
        self.entries.get(identity)
    }

    /// Deterministic inventory, not semantic ranking or automatic selection.
    pub fn entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }
}
