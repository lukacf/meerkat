//! Bind only a verified original-parent observation to native caller types.
use super::{
    CatalogRequirement, CatalogSelection, CatalogSource, ProtectedCollectionCatalog,
    mapping::ParsedMapping, refused, transport::VerifiedParentPacket, wire,
};
use crate::{
    ConnectError, RealmOptions,
    service_collection::{CollectionLimits, RequiredServiceCollection, ServiceCollectionSelection},
    service_probe::{ProbeHostBinding, now},
};
use meerkat::governed_realm::stock::caller::StockAuthenticatedCaller;
use meerkat_authorization_contracts::{
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    resource::ResourceRef,
};

pub(super) fn select(
    mapping: &ParsedMapping,
    observation: &VerifiedParentPacket,
    attestation: &wire::Attestation,
    catalog: &dyn ProtectedCollectionCatalog,
) -> Result<ServiceCollectionSelection, ConnectError> {
    if attestation.expires_at_ms <= now() || attestation.origin != mapping.application.origin {
        return Err(refused());
    }
    let rows: Vec<_> = mapping
        .principal_mappings
        .iter()
        .filter(|row| {
            row.issuer == attestation.issuer
                && row.subject == attestation.subject
                && row.workspace == attestation.workspace.as_str()
                && row.origin == attestation.origin
        })
        .collect();
    if rows.len() != 1 {
        return Err(refused());
    }
    // Length-delimited bytes include the actual received record and kernel
    // sender, not a reconstruction of user labels. The sealed mapping and this
    // original packet remain retained by the calling owner through real close.
    let mut evidence = b"meerkat.connect.root-parent-observation.v1\0".to_vec();
    evidence.extend_from_slice(&observation.peer().pid().to_be_bytes());
    evidence.extend_from_slice(&observation.peer().uid().to_be_bytes());
    evidence.extend_from_slice(&(observation.bytes().len() as u64).to_be_bytes());
    evidence.extend_from_slice(observation.bytes());
    let caller = StockAuthenticatedCaller::new(
        rows[0].caller.clone(),
        HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: mapping.authentication_evidence_domain.clone(),
                resource_id: format!(
                    "{}/{}/{}",
                    attestation.broker_instance.as_str(),
                    attestation.evidence_id.as_str(),
                    attestation.envelope.invocation.as_str()
                ),
            },
            revision: EvidenceId::new("1").map_err(|_| refused())?,
            digest: EvidenceDigest::of_bytes(&evidence),
        },
    )
    .map_err(|_| refused())?;
    let storage = RealmOptions {
        context_root: mapping.realm.context_root.clone(),
        state_root: Some(mapping.realm.state_root.clone()),
        user_config_root: Some(mapping.realm.user_config_root.clone()),
    }
    .resolve(mapping.realm.realm_id.as_str())?;
    if storage.locator.realm != mapping.realm.realm_id
        || storage.locator.state_root != mapping.realm.state_root
    {
        return Err(refused());
    }
    let selected = CatalogSelection(mapping);
    let collections = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !catalog.accepts(&selected) {
            return Err(refused());
        }
        let mut services = Vec::new();
        for (service_id, range) in &mapping.service_groups {
            let mut collections = Vec::new();
            for source in &mapping.collections[range.clone()] {
                let mut requirements = Vec::new();
                for requirement in &source.requirements {
                    let resolved = catalog.requirement(
                        &selected,
                        &CatalogSource(source),
                        &CatalogRequirement(requirement),
                    )?;
                    if resolved.id != requirement.id
                        || resolved.resource != requirement.resource
                        || resolved.operations
                            != requirement.raw_operation_ceiling.iter().cloned().collect()
                    {
                        return Err(refused());
                    }
                    requirements.push(resolved);
                }
                collections.push(RequiredServiceCollection::new(
                    source.source_id.clone(),
                    source.family.clone(),
                    source.profile.clone(),
                    source.agent.clone(),
                    requirements,
                    CollectionLimits::new(
                        source.limits.calls,
                        source.limits.argument_bytes,
                        source.limits.result_bytes,
                    )?,
                )?);
            }
            services.push((service_id.clone(), collections));
        }
        Ok(services)
    }))
    .map_err(|_| refused())??;
    ServiceCollectionSelection::for_services(
        ProbeHostBinding {
            application_id: mapping.application.application_id.clone(),
            instance_id: attestation.envelope.invocation.as_str().into(),
            origin: attestation.origin.clone(),
            viewer_id: attestation.subject.clone(),
            login_scope_id: attestation.scope.clone(),
            expires_at_ms: attestation.expires_at_ms,
            realm: storage.locator,
            context_root: mapping.realm.context_root.clone(),
            user_config_root: mapping.realm.user_config_root.clone(),
            caller,
            ingress: mapping.ingress.clone(),
        },
        collections,
    )
}
