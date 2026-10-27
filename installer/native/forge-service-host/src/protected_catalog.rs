use crate::{GheProgram, HistoricalStore, ProgramLimits, ProgramSpec};
use meerkat_connect::{
    ConnectError,
    protected_parent::{
        CatalogRequirement, CatalogSelection, CatalogSource, ProtectedCollectionCatalog,
    },
    service_collection::RequiredCollectionOperation,
};
use std::{collections::BTreeSet, sync::Arc};

// These select compiled application behavior. They are never authentication,
// account evidence, an installed receipt or a replacement for the root mapping.
pub const APPLICATION_ID: &str = "meerkat-forge";
pub const CATALOG_ID: &str = "forge.ghe-collections.v1";
pub const SOURCE_SELECTION: &str = "forge.ghe-collections.v1";
pub const SOURCE_FAMILY: &str = "official-github";
pub const SOURCE_PROFILE: &str = "enterprise-selected-reads";
pub const SOURCE_AGENT: &str = "forge-ingestion";

pub struct ForgeGheCatalog {
    history: Arc<HistoricalStore>,
}
impl ForgeGheCatalog {
    pub fn new(history: Arc<HistoricalStore>) -> Self {
        Self { history }
    }
}

fn selection_matches(application: &str, source: &str, catalog: &str) -> bool {
    application == APPLICATION_ID && source == SOURCE_SELECTION && catalog == CATALOG_ID
}

fn source_matches(family: &str, profile: &str, agent: &str) -> bool {
    family == SOURCE_FAMILY && profile == SOURCE_PROFILE && agent == SOURCE_AGENT
}

fn limits_fit(limits: ProgramLimits, calls: usize, result_bytes: usize) -> bool {
    usize::from(limits.calls) <= calls && limits.result_bytes <= result_bytes
}

fn operations_match(spec: &ProgramSpec, raw: &[String]) -> bool {
    let supplied: BTreeSet<_> = raw.iter().cloned().collect();
    supplied.len() == raw.len() && supplied == spec.operations().into_iter().collect()
}

fn refused() -> ConnectError {
    // No reflected identity, path, selector, account or private payload.
    ConnectError::InvalidInput("The selected Forge collection is unsupported".into())
}

impl ProtectedCollectionCatalog for ForgeGheCatalog {
    fn accepts(&self, selection: &CatalogSelection<'_>) -> bool {
        selection_matches(
            selection.application_id(),
            selection.source_selection(),
            selection.catalog_id(),
        )
    }

    fn requirement(
        &self,
        selection: &CatalogSelection<'_>,
        source: &CatalogSource<'_>,
        requirement: &CatalogRequirement<'_>,
    ) -> Result<RequiredCollectionOperation, ConnectError> {
        if !self.accepts(selection)
            || !source_matches(source.family(), source.profile(), source.agent())
        {
            return Err(refused());
        }
        let (repository, spec, limits) =
            ProgramSpec::from_resource(requirement.program_id(), requirement.resource())
                .map_err(|_| refused())?;
        if !limits_fit(limits, source.calls(), source.result_bytes())
            || !operations_match(&spec, requirement.raw_operation_ceiling())
        {
            return Err(refused());
        }
        // Native source limits remain shared across all requirements. A program
        // fitting this ceiling does not reserve calls/bytes or prove completion.
        // No callback I/O, helper launch, installed config or Ready constructor.
        Arc::new(
            GheProgram::new(
                source.source_id().into(),
                requirement.id().into(),
                repository,
                spec,
                limits,
                self.history.clone(),
            )
            .map_err(|_| refused())?,
        )
        .requirement()
        .map_err(|_| refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_installed_profile_names_are_exact_not_mapping_self_assertions() {
        assert!(selection_matches(
            APPLICATION_ID,
            SOURCE_SELECTION,
            CATALOG_ID
        ));
        for (application, source, catalog) in [
            ("other", SOURCE_SELECTION, CATALOG_ID),
            (APPLICATION_ID, "other", CATALOG_ID),
            (APPLICATION_ID, SOURCE_SELECTION, "other"),
        ] {
            assert!(!selection_matches(application, source, catalog));
        }
        assert!(source_matches(SOURCE_FAMILY, SOURCE_PROFILE, SOURCE_AGENT));
        for (family, profile, agent) in [
            ("github", SOURCE_PROFILE, SOURCE_AGENT),
            (SOURCE_FAMILY, "selected-reads", SOURCE_AGENT),
            (SOURCE_FAMILY, SOURCE_PROFILE, "other"),
        ] {
            assert!(!source_matches(family, profile, agent));
        }
    }

    #[test]
    fn ceilings_refuse_extra_operations_duplicates_and_larger_program_budgets() {
        let spec = ProgramSpec::IssueHierarchy { number: 17 };
        assert!(operations_match(&spec, &["issue_read".into()]));
        for raw in [
            vec![],
            vec!["issue_read".into(), "issue_write".into()],
            vec!["issue_read".into(), "issue_read".into()],
        ] {
            assert!(!operations_match(&spec, &raw));
        }
        let limits = ProgramLimits {
            calls: 8,
            result_bytes: 4096,
        };
        assert!(limits_fit(limits, 8, 4096));
        assert!(!limits_fit(limits, 7, 4096));
        assert!(!limits_fit(limits, 8, 4095));
    }
}
