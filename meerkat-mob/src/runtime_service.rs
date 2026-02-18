use crate::error::{MobError, MobResult};
use crate::model::{
    MeerkatInstance, MobActivationRequest, MobActivationResponse, MobReconcileRequest,
    MobReconcileResult, MobRun, MobRunFilter, MobSpec, NewMobEvent, PollEventsResponse,
};
use crate::runtime::MobRuntime;
use crate::service::MobService;
use crate::spec::ApplySpecRequest;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

enum MobCommand {
    ApplySpec {
        request: ApplySpecRequest,
        tx: oneshot::Sender<MobResult<MobSpec>>,
    },
    GetSpec {
        mob_id: String,
        tx: oneshot::Sender<MobResult<Option<MobSpec>>>,
    },
    ListSpecs {
        tx: oneshot::Sender<MobResult<Vec<MobSpec>>>,
    },
    DeleteSpec {
        mob_id: String,
        tx: oneshot::Sender<MobResult<()>>,
    },
    Activate {
        request: MobActivationRequest,
        tx: oneshot::Sender<MobResult<MobActivationResponse>>,
    },
    GetRun {
        run_id: String,
        tx: oneshot::Sender<MobResult<Option<MobRun>>>,
    },
    ListRuns {
        filter: MobRunFilter,
        tx: oneshot::Sender<MobResult<Vec<MobRun>>>,
    },
    CancelRun {
        run_id: String,
        tx: oneshot::Sender<MobResult<()>>,
    },
    ListMeerkats {
        mob_id: String,
        tx: oneshot::Sender<MobResult<Vec<MeerkatInstance>>>,
    },
    Reconcile {
        request: MobReconcileRequest,
        tx: oneshot::Sender<MobResult<MobReconcileResult>>,
    },
    PollEvents {
        cursor: Option<u64>,
        limit: Option<usize>,
        tx: oneshot::Sender<MobResult<PollEventsResponse>>,
    },
    Capabilities {
        tx: oneshot::Sender<MobResult<serde_json::Value>>,
    },
    EmitEvent {
        event: NewMobEvent,
        tx: oneshot::Sender<MobResult<()>>,
    },
}

pub struct MobRuntimeService {
    tx: mpsc::Sender<MobCommand>,
}

impl MobRuntimeService {
    pub fn new(runtime: MobRuntime) -> Self {
        let runtime = Arc::new(runtime);
        let (tx, mut rx) = mpsc::channel(128);
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    MobCommand::ApplySpec { request, tx } => {
                        let _ = tx.send(MobService::apply_spec(&*runtime, request).await);
                    }
                    MobCommand::GetSpec { mob_id, tx } => {
                        let _ = tx.send(MobService::get_spec(&*runtime, &mob_id).await);
                    }
                    MobCommand::ListSpecs { tx } => {
                        let _ = tx.send(MobService::list_specs(&*runtime).await);
                    }
                    MobCommand::DeleteSpec { mob_id, tx } => {
                        let _ = tx.send(MobService::delete_spec(&*runtime, &mob_id).await);
                    }
                    MobCommand::Activate { request, tx } => {
                        let _ = tx.send(MobService::activate(&*runtime, request).await);
                    }
                    MobCommand::GetRun { run_id, tx } => {
                        let _ = tx.send(MobService::get_run(&*runtime, &run_id).await);
                    }
                    MobCommand::ListRuns { filter, tx } => {
                        let _ = tx.send(MobService::list_runs(&*runtime, filter).await);
                    }
                    MobCommand::CancelRun { run_id, tx } => {
                        let _ = tx.send(MobService::cancel_run(&*runtime, &run_id).await);
                    }
                    MobCommand::ListMeerkats { mob_id, tx } => {
                        let _ = tx.send(MobService::list_meerkats(&*runtime, &mob_id).await);
                    }
                    MobCommand::Reconcile { request, tx } => {
                        let _ = tx.send(MobService::reconcile(&*runtime, request).await);
                    }
                    MobCommand::PollEvents { cursor, limit, tx } => {
                        let _ = tx.send(MobService::poll_events(&*runtime, cursor, limit).await);
                    }
                    MobCommand::Capabilities { tx } => {
                        let _ = tx.send(MobService::capabilities(&*runtime).await);
                    }
                    MobCommand::EmitEvent { event, tx } => {
                        let _ = tx.send(MobService::emit_event(&*runtime, event).await);
                    }
                }
            }
        });
        Self { tx }
    }

    async fn call<R>(&self, cmd: MobCommand, rx: oneshot::Receiver<MobResult<R>>) -> MobResult<R> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| MobError::Internal("mob command channel closed".to_string()))?;
        rx.await
            .map_err(|_| MobError::Internal("mob command response dropped".to_string()))?
    }
}

#[async_trait]
impl MobService for MobRuntimeService {
    async fn apply_spec(&self, request: ApplySpecRequest) -> MobResult<MobSpec> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::ApplySpec { request, tx }, rx).await
    }

    async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpec>> {
        let (tx, rx) = oneshot::channel();
        self.call(
            MobCommand::GetSpec {
                mob_id: mob_id.to_string(),
                tx,
            },
            rx,
        )
        .await
    }

    async fn list_specs(&self) -> MobResult<Vec<MobSpec>> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::ListSpecs { tx }, rx).await
    }

    async fn delete_spec(&self, mob_id: &str) -> MobResult<()> {
        let (tx, rx) = oneshot::channel();
        self.call(
            MobCommand::DeleteSpec {
                mob_id: mob_id.to_string(),
                tx,
            },
            rx,
        )
        .await
    }

    async fn activate(&self, request: MobActivationRequest) -> MobResult<MobActivationResponse> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::Activate { request, tx }, rx).await
    }

    async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>> {
        let (tx, rx) = oneshot::channel();
        self.call(
            MobCommand::GetRun {
                run_id: run_id.to_string(),
                tx,
            },
            rx,
        )
        .await
    }

    async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::ListRuns { filter, tx }, rx).await
    }

    async fn cancel_run(&self, run_id: &str) -> MobResult<()> {
        let (tx, rx) = oneshot::channel();
        self.call(
            MobCommand::CancelRun {
                run_id: run_id.to_string(),
                tx,
            },
            rx,
        )
        .await
    }

    async fn list_meerkats(&self, mob_id: &str) -> MobResult<Vec<MeerkatInstance>> {
        let (tx, rx) = oneshot::channel();
        self.call(
            MobCommand::ListMeerkats {
                mob_id: mob_id.to_string(),
                tx,
            },
            rx,
        )
        .await
    }

    async fn reconcile(&self, request: MobReconcileRequest) -> MobResult<MobReconcileResult> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::Reconcile { request, tx }, rx).await
    }

    async fn poll_events(
        &self,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> MobResult<PollEventsResponse> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::PollEvents { cursor, limit, tx }, rx)
            .await
    }

    async fn capabilities(&self) -> MobResult<serde_json::Value> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::Capabilities { tx }, rx).await
    }

    async fn emit_event(&self, event: NewMobEvent) -> MobResult<()> {
        let (tx, rx) = oneshot::channel();
        self.call(MobCommand::EmitEvent { event, tx }, rx).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::runtime::MobRuntimeBuilder;
    use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
    use async_trait::async_trait;
    use meerkat::{CreateSessionRequest, Session, SessionId, SessionService};
    use std::path::PathBuf;

    struct MockSessionService;

    #[async_trait]
    impl SessionService for MockSessionService {
        async fn create_session(
            &self,
            _req: CreateSessionRequest,
        ) -> Result<meerkat::RunResult, meerkat::SessionError> {
            Ok(meerkat::RunResult {
                text: String::new(),
                session_id: Session::new().id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: meerkat::Usage::default(),
                structured_output: None,
                schema_warnings: None,
            })
        }
        async fn start_turn(
            &self,
            _id: &SessionId,
            _req: meerkat::StartTurnRequest,
        ) -> Result<meerkat::RunResult, meerkat::SessionError> {
            Ok(meerkat::RunResult {
                text: String::new(),
                session_id: Session::new().id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: meerkat::Usage::default(),
                structured_output: None,
                schema_warnings: None,
            })
        }
        async fn interrupt(&self, _id: &SessionId) -> Result<(), meerkat::SessionError> {
            Ok(())
        }
        async fn read(&self, _id: &SessionId) -> Result<meerkat::SessionView, meerkat::SessionError> {
            Err(meerkat::SessionError::NotFound {
                id: Session::new().id().clone(),
            })
        }
        async fn list(
            &self,
            _query: meerkat::SessionQuery,
        ) -> Result<Vec<meerkat::SessionSummary>, meerkat::SessionError> {
            Ok(Vec::new())
        }
        async fn archive(&self, _id: &SessionId) -> Result<(), meerkat::SessionError> {
            Ok(())
        }
        async fn comms_runtime(
            &self,
            _id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
            None
        }
    }

    #[tokio::test]
    async fn service_channel_handles_capabilities_call() {
        let runtime = MobRuntimeBuilder::new(
            "realm-a",
            Arc::new(MockSessionService),
            meerkat::AgentFactory::new("/tmp"),
            Arc::new(InMemoryMobSpecStore::default()),
            Arc::new(InMemoryMobRunStore::default()),
            Arc::new(InMemoryMobEventStore::default()),
        )
        .runtime_root(PathBuf::from("/tmp"))
        .build()
        .unwrap();
        let svc = MobRuntimeService::new(runtime);
        let caps = svc.capabilities().await.unwrap();
        assert!(caps.get("flow_model").is_some());
    }
}
