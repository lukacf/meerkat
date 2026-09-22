//! The managed host uses the same durable owner for RPC, comms and schedules.

use anyhow::Context as _;
use meerkat::{AgentBuildConfig, AgentFactory};
use meerkat_core::{Config, ConfigStore, types::SessionId};
use meerkat_rpc::{router::NotificationSink, session_runtime::SessionRuntime};
use std::{path::Path, sync::Arc};

pub struct ManagedRpcHost {
    pub runtime: Arc<SessionRuntime>,
    pub config_store: Arc<dyn ConfigStore>,
    pub comms: Arc<meerkat_comms::CommsRuntime>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ManagedIdentity {
    mdm_host: String,
}

impl ManagedRpcHost {
    pub async fn open(
        root: &Path,
        config: Config,
        comms_config: meerkat_comms::ResolvedCommsConfig,
    ) -> anyhow::Result<Self> {
        let (manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            "mdm",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await?;
        let mut comms = meerkat_comms::CommsRuntime::new(comms_config)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        comms.set_blob_store(persistence.blob_store());
        let comms = Arc::new(comms);
        let factory = AgentFactory::new(persistence.store_path().context("durable store path")?)
            .shell(true)
            .builtins(true)
            .comms(true)
            .schedule(true)
            .mob(true)
            .with_comms_runtime(comms.clone());
        let config_store: Arc<dyn ConfigStore> = Arc::new(meerkat_core::MemoryConfigStore::new(
            config.clone(),
            meerkat_models::canonical(),
        ));
        let runtime =
            SessionRuntime::new(factory, config, 1024, persistence, NotificationSink::noop());
        runtime.set_realm_context(
            Some(manifest.realm),
            None,
            Some(manifest.backend.as_str().into()),
        );
        let mobs = Arc::new(meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            runtime.session_service(),
            Some(runtime.runtime_adapter()),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        runtime.set_mob_tools(Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
            mobs.clone(),
        )));
        runtime.set_mob_state(mobs);
        runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            config_store.clone(),
            root.join("config_state.json"),
        )));
        Ok(Self {
            runtime: Arc::new(runtime),
            config_store,
            comms,
        })
    }

    pub async fn managed_session(
        &self,
        name: &str,
        mut build: AgentBuildConfig,
        mcp_servers: Vec<meerkat_core::mcp_config::McpServerConfig>,
    ) -> anyhow::Result<SessionId> {
        // Static host identity lives in the canonical Session's app context,
        // not a sidecar session-id file or a JSONL projection.
        let sessions = self
            .runtime
            .list_sessions(Default::default())
            .await
            .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        let mut existing = Vec::new();
        for info in sessions {
            let session = self
                .runtime
                .load_persisted_session(&info.session_id)
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.message))?
                .with_context(|| format!("listed session disappeared: {}", info.session_id))?;
            if let Some(context) = session
                .try_build_state()?
                .and_then(|state| state.app_context)
                && let Ok(identity) = serde_json::from_value::<ManagedIdentity>(context)
                && identity.mdm_host == name
            {
                existing.push(info.session_id);
            }
        }
        anyhow::ensure!(existing.len() <= 1, "multiple managed sessions for {name}");
        let fresh = existing.is_empty();
        build.mcp_servers = mcp_servers;
        build.app_context = Some(serde_json::to_value(ManagedIdentity {
            mdm_host: name.into(),
        })?);
        build.wait_for_mcp = !build.mcp_servers.is_empty();
        build.override_builtins = meerkat_core::ToolCategoryOverride::Enable;
        build.override_shell = meerkat_core::ToolCategoryOverride::Enable;
        build.override_mob = meerkat_core::ToolCategoryOverride::Enable;
        let session_id = self
            .runtime
            .create_or_resume_session_without_turn(
                build,
                existing.pop(),
                Some([("mdm_host".into(), name.into())].into_iter().collect()),
                if fresh {
                    Default::default()
                } else {
                    meerkat_core::SurfaceSessionRecoveryOverrides {
                        override_builtins: Some(true),
                        override_shell: Some(true),
                        override_mob: Some(true),
                        ..Default::default()
                    }
                },
            )
            .await
            .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        if fresh {
            self.runtime.append_system_context(&session_id,
                meerkat_core::AppendSystemContextRequest::from_text(format!(
                    "Your current session_id is '{session_id}'. Use this exact session_id for scheduled follow-up work for this session."
                ))).await.map_err(|error| anyhow::anyhow!("{}", error.message))?;
        }
        self.runtime.ensure_schedule_host_started().await?;
        self.runtime
            .enable_autonomous_comms_drain(
                &session_id,
                self.comms.clone() as Arc<dyn meerkat_core::agent::CommsRuntime>,
            )
            .await
            .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        Ok(session_id)
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.runtime.try_shutdown().await?;
        self.comms.retire_inproc_route();
        Ok(())
    }
}
