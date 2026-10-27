//! Toolkit-authored discovery recipes. Loading this inventory performs no I/O.

use crate::{
    CatalogEntry, CatalogError, CatalogOrigin, ContentDigest, ExportRole, Id, IntegrationPackage,
    PackageCatalog, Revision,
};

const BUNDLED_DISCOVERY: &str = include_str!("../catalog/discovery.json");
const BUNDLED_ADOPTED: &str = include_str!("../catalog/adopted.json");

/// Complete frontend observations must fit in one bounded tool response.
pub const FRONTEND_CATALOG_MAX_BYTES: usize = 64 * 1024;

/// Whole existing catalog records, ordered by the catalog's package identity.
/// This is an inert observation, not an installation or trust decision.
#[derive(serde::Serialize)]
pub struct FrontendCatalogObservation<'a> {
    pub entries: Vec<&'a CatalogEntry>,
}

/// Project the declared frontend role, retaining service roles in mixed packages.
/// Names and descriptions never participate in selection. Records are borrowed
/// unchanged, including their original digest, origin and evidence references.
pub fn frontend_catalog_observation(catalog: &PackageCatalog) -> FrontendCatalogObservation<'_> {
    FrontendCatalogObservation {
        entries: catalog
            .entries()
            .filter(|entry| {
                entry
                    .package
                    .known_exports
                    .iter()
                    .any(|export| export.role == ExportRole::AgentFrontend)
            })
            .collect(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BundledDiscoveryError {
    #[error("bundled discovery metadata is not valid JSON")]
    InvalidEncoding,
    #[error("bundled discovery origin is invalid")]
    InvalidOrigin,
    #[error(transparent)]
    InvalidPackage(#[from] CatalogError),
    #[error("bundled recipe catalogs contain a repeated package identity")]
    RepeatedIdentity,
}

/// Read the bundled source-reviewed candidates using the ordinary catalog.
/// Curated origin describes recipe ownership, never runtime support or trust.
/// Agents may inspect other catalogs or investigate outside this inventory.
pub fn bundled_discovery_catalog() -> Result<PackageCatalog, BundledDiscoveryError> {
    let packages: Vec<IntegrationPackage> = serde_json::from_str(BUNDLED_DISCOVERY)
        .map_err(|_| BundledDiscoveryError::InvalidEncoding)?;
    let origin = CatalogOrigin::Curated {
        catalog: Id::new("meerkat-toolkit/discovery")
            .map_err(|_| BundledDiscoveryError::InvalidOrigin)?,
        revision: Revision::new("discovery-2026-10-07.1")
            .map_err(|_| BundledDiscoveryError::InvalidOrigin)?,
    };
    let mut catalog = PackageCatalog::default();
    for package in packages {
        catalog.insert(package, origin.clone())?;
    }
    Ok(catalog)
}

/// Read the adopted library's exact, scoped local MCP compatibility descriptions.
/// Generated from the canonical adoption inventory, this catalog adds no runtime
/// binding or activation authority. Evidence paths are public repository-relative
/// references, not instructions to load files or execute package code.
pub fn bundled_adopted_catalog() -> Result<PackageCatalog, BundledDiscoveryError> {
    let packages: Vec<IntegrationPackage> = serde_json::from_str(BUNDLED_ADOPTED)
        .map_err(|_| BundledDiscoveryError::InvalidEncoding)?;
    let origin = CatalogOrigin::Curated {
        catalog: Id::new("meerkat-toolkit/adopted")
            .map_err(|_| BundledDiscoveryError::InvalidOrigin)?,
        revision: Revision::new(format!(
            "sha256:{}",
            ContentDigest::of_bytes(BUNDLED_ADOPTED.as_bytes()).as_str()
        ))
        .map_err(|_| BundledDiscoveryError::InvalidOrigin)?,
    };
    let mut catalog = PackageCatalog::default();
    for package in packages {
        catalog.insert(package, origin.clone())?;
    }
    Ok(catalog)
}

/// The two canonical public inventories, preserving their distinct provenance.
/// These are recipe observations, not the selected runtime bundle's catalog.
pub fn bundled_recipe_catalog() -> Result<PackageCatalog, BundledDiscoveryError> {
    let mut catalog = bundled_adopted_catalog()?;
    for entry in bundled_discovery_catalog()?.entries() {
        // PackageCatalog permits an identical package to be inserted again, but
        // that must not silently discard either source's catalog origin here.
        if catalog.get(&entry.package.identity).is_some() {
            return Err(BundledDiscoveryError::RepeatedIdentity);
        }
        catalog.insert(entry.package.clone(), entry.origin.clone())?;
    }
    Ok(catalog)
}
