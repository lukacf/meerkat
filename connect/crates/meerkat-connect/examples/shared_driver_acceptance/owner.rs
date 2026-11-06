//! Fixed two-child application launch owner for the real Driver acceptance.
//! Child linkage is retained by the production canonical source launcher.
use async_trait::async_trait;
use meerkat_connect::{
    application_acquisition::workflow::{
        ApplicationWorkflowInvocation, ApplicationWorkflowLaunchOwner, ApplicationWorkflowRuntime,
        PrivateApplicationWorkflowLauncher,
    },
    application_driver::ApplicationPreparationPlan,
};
use meerkat_toolkit::host_lifetime::control::{HostLifetimeError, SelectedHostArtifact};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use toolkit_mobkit::{
    ToolAdmissionError, installed_host::acquisition::InstalledOriginalInvocation,
};
use toolkit_package::Id;
#[path = "launch.rs"]
mod launch;

pub(super) const P_TEXT: &str = "Driver host planning parent, without either waiter request";
pub(super) const G_TEXT: &str =
    "Driver host planning lead, awaiting independently authorized context delivery";
pub(super) fn request(tag: &str) -> String {
    format!("driver-shared-{tag}: read one public Hacker News story using the packaged integration")
}
pub(super) fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(super) fn id(s: &str) -> Id {
    Id::new(s).expect("fixed acceptance identifier")
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) struct LaunchOwner {
    plan: ApplicationPreparationPlan,
    directory: PathBuf,
    runtime: Mutex<Option<Arc<ApplicationWorkflowRuntime>>>,
    private_launcher: PrivateApplicationWorkflowLauncher,
    stopped: AtomicBool,
}
impl LaunchOwner {
    pub(super) fn load(reference: &SelectedHostArtifact) -> Result<Arc<Self>, String> {
        let bytes = std::fs::read(&reference.path).map_err(error)?;
        if bytes.len() > 8 * 1024 * 1024 || digest(&bytes) != reference.sha256 {
            return Err("acceptance host must use the exact selected Driver plan".into());
        }
        let plan: ApplicationPreparationPlan = serde_json::from_slice(&bytes).map_err(error)?;
        if plan.application.roster.len() != 4
            || plan.application.member_classes.len() != 1
            || plan
                .application
                .roster
                .iter()
                .any(|row| row.identity.as_str() == "planning-lead")
        {
            return Err("acceptance requires four real initial members and zero initial G".into());
        }
        Ok(Arc::new(Self {
            plan,
            directory: reference.path.parent().ok_or("plan parent absent")?.into(),
            runtime: Mutex::new(None),
            private_launcher: PrivateApplicationWorkflowLauncher::default(),
            stopped: AtomicBool::new(false),
        }))
    }
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    fn runtime(&self) -> Result<Arc<ApplicationWorkflowRuntime>, HostLifetimeError> {
        self.runtime
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .clone()
            .ok_or(HostLifetimeError::Unavailable)
    }
    pub(super) fn release_runtime(&self) -> Result<(), String> {
        self.runtime
            .lock()
            .map_err(|_| "launch owner poisoned")?
            .take();
        Ok(())
    }
    pub(super) async fn exercise(self: Arc<Self>) -> Result<(), String> {
        // The parent test requests work only after the normal SDK Ready. This
        // file conveys no membership or authority and never substitutes for it.
        while self.runtime().is_err() || !self.directory.join("begin-workflow").exists() {
            if self.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let result = self.launch().await;
        let record = match &result {
            Ok(value) => json!({"status":"submitted","launch":value}),
            Err(failure) => json!({"status":"failed","error":failure}),
        };
        std::fs::write(
            self.directory.join("driver-launch-result.json"),
            serde_json::to_vec_pretty(&record).map_err(error)?,
        )
        .map_err(error)?;
        result.map(|_| ())
    }
}
#[async_trait]
impl ApplicationWorkflowLaunchOwner for LaunchOwner {
    fn bind_runtime(&self, runtime: ApplicationWorkflowRuntime) -> Result<(), HostLifetimeError> {
        let mut selected = self
            .runtime
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if selected.is_some() || self.stopped.load(Ordering::Acquire) {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.private_launcher.bind(&runtime)?;
        *selected = Some(Arc::new(runtime));
        Ok(())
    }
    async fn invocation(
        &self,
        invocation: &InstalledOriginalInvocation<'_>,
    ) -> Result<Option<ApplicationWorkflowInvocation>, ToolAdmissionError> {
        self.private_launcher.invocation(invocation).await
    }
}
