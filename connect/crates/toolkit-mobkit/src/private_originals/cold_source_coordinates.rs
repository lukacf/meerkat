//! Inert coordinates for the first authenticated ordinary input after Restore.
//! They never satisfy live route, grant, custody, or native admission APIs.
use super::*;
use crate::installed_host::dynamic_members::MemberClassAdmission;

pub(super) struct ColdOrdinarySourceCoordinates {
    admission: MemberClassAdmission,
    route: PrivateOriginalRoute,
}
impl ColdOrdinarySourceCoordinates {
    pub(super) fn new(admission: MemberClassAdmission, route: PrivateOriginalRoute) -> Self {
        Self { admission, route }
    }
}

impl PrivateOriginals {
    fn check_source_coordinates(
        &self,
        coordinates: &ColdOrdinarySourceCoordinates,
    ) -> Result<(), PrivateOriginalError> {
        // Observe close independently; never hold its lock across policy or IO.
        self.with_new_work_admission(|| ())?;
        let current = self
            .cold_ordinary_source_coordinates(&coordinates.route.id)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if current.admission != coordinates.admission || current.route != coordinates.route {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }

    pub(super) async fn retain_event_for_source_coordinates(
        &self,
        event: PrivateAuthenticatedEvent,
        coordinates: &ColdOrdinarySourceCoordinates,
    ) -> Result<PrivateOriginalRef, PrivateOriginalError> {
        self.check_source_coordinates(coordinates)?;
        // Existing immutable source/domain/key/content and admission-conflict
        // validation owns the row. This does not stage a Candidate or a grant.
        let reference = self
            .retain_event_for_route(event, &coordinates.route)
            .await?;
        self.check_source_coordinates(coordinates)?;
        Ok(reference)
    }

    async fn record_for_source_coordinates(
        &self,
        reference: &PrivateOriginalRef,
        coordinates: &ColdOrdinarySourceCoordinates,
    ) -> Result<SourceRecord, PrivateOriginalError> {
        self.check_source_coordinates(coordinates)?;
        let key = self.key_for_route(reference, &coordinates.route)?;
        let stored = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        // Refresh exact canonical authorization after IO, without a lock over it.
        self.check_source_coordinates(coordinates)?;
        self.decode_record_for_route(&stored, reference, &coordinates.route)
    }

    pub(super) async fn event_for_source_coordinates(
        &self,
        reference: &PrivateOriginalRef,
        coordinates: &ColdOrdinarySourceCoordinates,
    ) -> Result<PrivateAuthenticatedEvent, PrivateOriginalError> {
        let record = self
            .record_for_source_coordinates(reference, coordinates)
            .await?;
        Self::check_unbound_ordinary_source(&record)?;
        let mut event = record.event;
        event.route = reference.route.clone();
        Ok(event)
    }

    pub(super) fn target_for_source_coordinates<'a>(
        &self,
        coordinates: &'a ColdOrdinarySourceCoordinates,
    ) -> &'a str {
        coordinates.route.grant_target.as_str()
    }

    pub(super) fn authentication_domain_for_source_coordinates<'a>(
        &self,
        coordinates: &'a ColdOrdinarySourceCoordinates,
    ) -> &'a ResourceDomain {
        &coordinates.route.authentication_domain
    }

    pub(super) async fn event_key_for_source_coordinates(
        &self,
        reference: &PrivateOriginalRef,
        coordinates: &ColdOrdinarySourceCoordinates,
    ) -> Result<meerkat_runtime::identifiers::IdempotencyKey, PrivateOriginalError> {
        let record = self
            .record_for_source_coordinates(reference, coordinates)
            .await?;
        Self::check_unbound_ordinary_source(&record)?;
        cold_service_admission::source_event_key(&record)
    }

    // Cold coordinates select no service/task/setup/workflow/peer/native-target
    // path and never replay an already bound input. Adding a source-record field
    // requires an explicit classification here, not an implicit `..` allowance.
    fn check_unbound_ordinary_source(record: &SourceRecord) -> Result<(), PrivateOriginalError> {
        match record {
            SourceRecord {
                source: _,
                event: _,
                operation_cancelled: false,
                console_child_input: None,
                peer_ingress: None,
                native_operations: None,
                late_workflow_helper: None,
                workflow_child: None,
                class_input: None,
                custody_attempt: None,
                custody_input: None,
                shared_preparations,
                setup_request: None,
                configured_setup: None,
                service_setup_children,
                service_attachment_failure: None,
                service_attachment_observation: None,
                service_attachment_member: None,
                approved_task: None,
                service_admission: None,
                planning_source_evidence: None,
                bound_service_input: None,
                final_association: None,
            } if shared_preparations.is_empty() && service_setup_children.is_empty() => Ok(()),
            _ => Err(PrivateOriginalError::Denied),
        }
    }
}
