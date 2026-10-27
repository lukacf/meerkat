//! Historical selection and exact child correspondence for administrative
//! recovery. No serialized value here is a grant or an issuance request.
use super::*;

pub(crate) struct RetainedServiceSelection {
    decision: AccessDecision,
    selection: Option<StoredServiceMcpSelection>,
    pub reference: PrivateServiceGrantRef,
}
pub(crate) struct ServiceRecoveryChild {
    pub stored: Option<StoredRecord>,
    pub native: Option<(PrincipalRef, GrantLineageRef)>,
    pub state: &'static str,
    pub owner: ServiceRecoveryChildOwner,
}

pub(crate) enum ServiceRecoveryChildOwner {
    ThisApproval(PrincipalRef),
    NotObserved,
    AnotherApproval,
}

impl PrivateOriginals {
    pub(crate) fn retained_service_selection(
        &self,
        decision: &PrivateServiceMcpAccessDecision,
        selection: &serde_json::Value,
        parent_restrictions: &ExecutionRestrictions,
    ) -> Result<RetainedServiceSelection, PrivateOriginalError> {
        let selected: StoredServiceMcpSelection = serde_json::from_value(selection.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        if canonical_json(&selected).map_err(|_| PrivateOriginalError::Conflict)?
            != canonical_json(selection).map_err(|_| PrivateOriginalError::Conflict)?
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let route = self.route(&decision.route)?;
        // Reproduce the historical bounds without treating expiry as revoke.
        // This API observes evidence only; no native entry can consume it.
        let mut expected = selected.restrictions(route, decision, 0)?;
        expected.delegation_depth = DelegationDepth::remaining(1);
        if &expected != parent_restrictions {
            return Err(PrivateOriginalError::Conflict);
        }
        let reference = PrivateServiceGrantRef {
            route: decision.route.clone(),
            decision: decision.decision.clone(),
            decision_sha256: decision_digest(
                &AccessDecision::Mcp(decision.clone()),
                Some(&selected),
            )?,
        };
        Ok(RetainedServiceSelection {
            decision: AccessDecision::Mcp(decision.clone()),
            selection: Some(selected),
            reference,
        })
    }

    pub(crate) fn retained_catalog_selection(&self, decision: &catalog::ServiceCatalogDecision,
        requested: &ExecutionRestrictions) -> Result<RetainedServiceSelection, PrivateOriginalError> {
        let mut expected = decision.restrictions(self.route(&decision.route)?, 0)?;
        expected.delegation_depth = DelegationDepth::remaining(1);
        if &expected != requested { return Err(PrivateOriginalError::Conflict); }
        Ok(RetainedServiceSelection { decision: AccessDecision::Catalog(decision.clone()), selection: None,
            reference: Self::service_catalog_reference(decision)? })
    }

    pub(crate) async fn service_recovery_child(
        &self,
        selection: &RetainedServiceSelection,
        parent: &GrantLineageRef,
    ) -> Result<ServiceRecoveryChild, PrivateOriginalError> {
        let key = self.access_key(&selection.reference)?;
        let stored = self.store.get(&key).await.map_err(store_error)?;
        let Some(saved) = &stored else {
            let id = EvidenceId::new(format!("access-{}", key.id))
                .map_err(|_| PrivateOriginalError::Conflict)?;
            if self
                .owner
                .grants()
                .issued_reference(&id)
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .is_some()
            {
                return Err(PrivateOriginalError::Unavailable);
            }
            return Ok(ServiceRecoveryChild {
                stored,
                native: None,
                state: "not_observed",
                owner: ServiceRecoveryChildOwner::NotObserved,
            });
        };
        // Decode the row's actual identity before deciding whether this review
        // owns it. Different reviews can name the same deterministic child ID.
        let raw: AccessRecord = serde_json::from_value(saved.data.body.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let actual = PrivateServiceGrantRef {
            route: raw.decision.route().clone(),
            decision: raw.decision.id().clone(),
            decision_sha256: decision_digest(&raw.decision, raw.mcp.as_ref())?,
        };
        let record = self.decode_access(saved, &actual)?;
        if record.parent.as_slice() != [parent.clone()] {
            return Ok(ServiceRecoveryChild {
                stored,
                native: None,
                state: "another_approval",
                owner: ServiceRecoveryChildOwner::AnotherApproval,
            });
        }
        if actual != selection.reference
            || record.decision != selection.decision
            || record.mcp != selection.selection
            || record.application_policy.is_some()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let native = self
            .service_compensation_child(&selection.reference, parent)
            .await?;
        let state = match self
            .access_observation(selection.reference.clone(), &record)?
            .state
        {
            PrivateServiceGrantState::GrantIssued => "issued",
            PrivateServiceGrantState::GrantUnavailable => "unavailable",
            PrivateServiceGrantState::PendingReconciliation => "pending_reconciliation",
            PrivateServiceGrantState::ApprovalDenied => "denied",
        };
        Ok(ServiceRecoveryChild {
            stored,
            native,
            state,
            owner: ServiceRecoveryChildOwner::ThisApproval(record.caller),
        })
    }

    pub(crate) fn service_recovery_child_reference(
        &self,
        selection: &RetainedServiceSelection,
    ) -> Result<Option<GrantLineageRef>, PrivateOriginalError> {
        let key = self.access_key(&selection.reference)?;
        let id = EvidenceId::new(format!("access-{}", key.id))
            .map_err(|_| PrivateOriginalError::Conflict)?;
        self.owner
            .grants()
            .issued_reference(&id)
            .map_err(|_| PrivateOriginalError::Unavailable)
    }
}

impl RetainedServiceSelection {
    pub(crate) fn projection(&self) -> serde_json::Value {
        match (&self.decision, &self.selection) {
            (AccessDecision::Mcp(decision), Some(selection)) => serde_json::json!({"service":selection.server,
                "selected_account":selection.connector.as_ref().map(|binding| binding.account.as_str()),
                "target":decision.permission.target, "operations":decision.permission.operations,
                "resource":decision.permission.resource, "lifetime":decision.permission.lifetime}),
            (AccessDecision::Catalog(decision), None) => serde_json::json!({"service":"public_service_set_catalog",
                "selected_account":null, "operations":["catalog"], "resource":decision.session.to_string(),
                "lifetime":decision.lifetime}),
            _ => serde_json::Value::Null,
        }
    }
}
