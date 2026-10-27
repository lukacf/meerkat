//! Version-specific shape checks at the existing source writer. Canonical
//! selection equality and native lineage validation remain with their owners.
use super::OriginalWriterError;
use meerkat_authorization_contracts::work_association::{GrantTarget, MAX_ASSOCIATION_REFERENCES};

pub(crate) enum ServiceSetBindingTargets<'a> {
    PublicServicesV1,
    WithConfiguredSetupV2(&'a [GrantTarget]),
}

impl ServiceSetBindingTargets<'_> {
    pub(super) fn check(&self, targets: &[&GrantTarget]) -> Result<(), OriginalWriterError> {
        let refuse = OriginalWriterError::UnsupportedMandate;
        if targets
            .iter()
            .enumerate()
            .any(|(index, target)| targets[..index].contains(target))
        {
            return Err(refuse);
        }
        let max_services = crate::installed_host::operator_setup::MAX_PUBLIC_SERVICE_BATCH;
        match self {
            Self::PublicServicesV1 => {
                // Keep the legacy shape unchanged. A find target cannot opt a
                // v1 record into the configured v2 authority composition.
                if !(2..=max_services).contains(&targets.len())
                    || targets
                        .iter()
                        .any(|target| !matches!(target, GrantTarget::McpServer { .. }))
                {
                    return Err(refuse);
                }
            }
            Self::WithConfiguredSetupV2(setup) => {
                let find = crate::installed_host::operator_setup::setup_targets::find_target();
                if targets.len() > MAX_ASSOCIATION_REFERENCES
                    || !setup.contains(&find)
                    || setup
                        .iter()
                        .enumerate()
                        .any(|(index, target)| setup[..index].contains(target))
                    || setup.iter().any(|target| !targets.contains(&target))
                {
                    return Err(refuse);
                }
                let business: Vec<_> = targets
                    .iter()
                    .filter(|target| !setup.contains(target))
                    .collect();
                if !(1..=max_services).contains(&business.len())
                    || business
                        .iter()
                        .any(|target| !matches!(target, GrantTarget::McpServer { .. }))
                {
                    return Err(refuse);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp(name: &str) -> GrantTarget {
        GrantTarget::McpServer {
            server: name.into(),
        }
    }
    fn tool(name: &str) -> GrantTarget {
        GrantTarget::RegisteredToolBundleOperation {
            bundle: "setup".into(),
            operation: name.into(),
        }
    }
    fn checked(shape: ServiceSetBindingTargets<'_>, targets: &[GrantTarget]) -> bool {
        shape.check(&targets.iter().collect::<Vec<_>>()).is_ok()
    }

    #[test]
    fn versioned_service_set_keeps_legacy_shape_and_exact_setup_members() {
        let find = crate::installed_host::operator_setup::setup_targets::find_target();
        let setup = vec![find.clone(), tool("report"), mcp("installer")];
        assert!(checked(
            ServiceSetBindingTargets::PublicServicesV1,
            &[mcp("x"), mcp("y")]
        ));
        assert!(!checked(
            ServiceSetBindingTargets::PublicServicesV1,
            &[mcp("x")]
        ));
        let mut combined = setup.clone();
        combined.push(mcp("x"));
        assert!(!checked(
            ServiceSetBindingTargets::PublicServicesV1,
            &combined
        ));
        assert!(checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &combined
        ));
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &setup
        ));
        let mut missing = combined.clone();
        missing.remove(1);
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &missing
        ));
        let mut extra = combined.clone();
        extra.push(tool("unselected"));
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &extra
        ));
        let mut duplicate = combined.clone();
        duplicate.push(mcp("x"));
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &duplicate
        ));
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup[1..]),
            &combined
        ));
        let mut duplicated_setup = setup.clone();
        duplicated_setup.push(find);
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&duplicated_setup),
            &combined
        ));
    }

    #[test]
    fn configured_service_set_counts_business_members_separately_under_native_bound() {
        let mut setup = vec![crate::installed_host::operator_setup::setup_targets::find_target()];
        setup.extend((1..56).map(|index| tool(&format!("helper-{index}"))));
        let mut combined = setup.clone();
        combined.extend((0..8).map(|index| mcp(&format!("service-{index}"))));
        assert_eq!(combined.len(), MAX_ASSOCIATION_REFERENCES);
        assert!(checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &combined
        ));
        combined.push(mcp("ninth"));
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(&setup),
            &combined
        ));
        let only_find = &setup[..1];
        let mut nine = only_find.to_vec();
        nine.extend((0..9).map(|index| mcp(&format!("service-{index}"))));
        assert!(!checked(
            ServiceSetBindingTargets::WithConfiguredSetupV2(only_find),
            &nine
        ));
    }
}
