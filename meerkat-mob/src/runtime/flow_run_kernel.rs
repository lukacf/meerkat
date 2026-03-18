use super::terminalization::{
    FlowTerminalizationAuthority, TerminalizationOutcome, TerminalizationTarget,
};
use crate::error::MobError;
use crate::ids::{FlowId, MobId, RunId};
use crate::run::MobRun;
use crate::store::{MobEventStore, MobRunStore};
use std::sync::Arc;

#[derive(Clone)]
pub struct FlowRunKernel {
    mob_id: MobId,
    run_store: Arc<dyn MobRunStore>,
    terminalization: FlowTerminalizationAuthority,
}

impl FlowRunKernel {
    pub fn new(
        mob_id: MobId,
        run_store: Arc<dyn MobRunStore>,
        events: Arc<dyn MobEventStore>,
    ) -> Self {
        let terminalization =
            FlowTerminalizationAuthority::new(run_store.clone(), events, mob_id.clone());
        Self {
            mob_id,
            run_store,
            terminalization,
        }
    }

    pub async fn create_pending_run(
        &self,
        flow_id: FlowId,
        activation_params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        let run = MobRun::pending(self.mob_id.clone(), flow_id, activation_params);
        let run_id = run.run_id.clone();
        self.run_store.create_run(run).await?;
        Ok(run_id)
    }

    pub async fn terminalize_completed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalization
            .terminalize(run_id, flow_id, TerminalizationTarget::Completed)
            .await
    }

    pub async fn terminalize_failed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalization
            .terminalize(run_id, flow_id, TerminalizationTarget::Failed { reason })
            .await
    }

    pub async fn terminalize_canceled(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalization
            .terminalize(run_id, flow_id, TerminalizationTarget::Canceled)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MobEventKind;
    use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, MobEventStore, MobRunStore};

    #[tokio::test]
    async fn flow_run_kernel_creates_pending_runs_from_durable_truth() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-kernel"), run_store.clone(), events);

        let run_id = kernel
            .create_pending_run(FlowId::from("demo"), serde_json::json!({"mode":"test"}))
            .await
            .expect("create pending run");
        let run = run_store
            .get_run(&run_id)
            .await
            .expect("load run")
            .expect("pending run should persist");
        assert_eq!(run.flow_id, FlowId::from("demo"));
        assert_eq!(run.status, crate::run::MobRunStatus::Pending);
    }

    #[tokio::test]
    async fn flow_run_kernel_terminalizes_without_actor_owned_fallback_state() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(
            MobId::from("mob-terminal"),
            run_store.clone(),
            events.clone(),
        );
        let run_id = kernel
            .create_pending_run(FlowId::from("demo"), serde_json::json!({}))
            .await
            .expect("create pending run");

        kernel
            .terminalize_canceled(run_id.clone(), FlowId::from("demo"))
            .await
            .expect("terminalize canceled");

        let run = run_store
            .get_run(&run_id)
            .await
            .expect("load run")
            .expect("run exists");
        assert_eq!(run.status, crate::run::MobRunStatus::Canceled);

        let replay = events.replay_all().await.expect("replay events");
        assert!(replay.iter().any(|event| matches!(
            &event.kind,
            MobEventKind::FlowCanceled { run_id: id, .. } if id == &run_id
        )));
    }
}
