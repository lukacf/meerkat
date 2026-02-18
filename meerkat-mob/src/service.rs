use crate::error::MobResult;
use crate::model::{
    MeerkatInstance, MobActivationRequest, MobActivationResponse, MobEvent, MobReconcileRequest,
    MobReconcileResult, MobRun, MobRunFilter, MobSpec, PollEventsResponse,
};
use crate::spec::ApplySpecRequest;
use async_trait::async_trait;

#[async_trait]
pub trait MobService: Send + Sync {
    async fn apply_spec(&self, request: ApplySpecRequest) -> MobResult<MobSpec>;
    async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpec>>;
    async fn list_specs(&self) -> MobResult<Vec<MobSpec>>;
    async fn delete_spec(&self, mob_id: &str) -> MobResult<()>;

    async fn activate(&self, request: MobActivationRequest) -> MobResult<MobActivationResponse>;
    async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>>;
    async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>>;
    async fn cancel_run(&self, run_id: &str) -> MobResult<()>;

    async fn list_meerkats(&self, mob_id: &str) -> MobResult<Vec<MeerkatInstance>>;
    async fn reconcile(&self, request: MobReconcileRequest) -> MobResult<MobReconcileResult>;

    async fn poll_events(&self, cursor: Option<u64>, limit: Option<usize>)
        -> MobResult<PollEventsResponse>;

    async fn capabilities(&self) -> MobResult<serde_json::Value>;

    async fn emit_event(&self, event: MobEvent) -> MobResult<()>;
}
