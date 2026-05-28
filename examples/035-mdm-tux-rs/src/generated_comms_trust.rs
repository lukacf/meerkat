use std::sync::Arc;

use anyhow::Context as _;
use meerkat_comms::{CommsRuntime, TrustedPeer};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{PeerId, TrustedPeerDescriptor};
use meerkat_core::types::SessionId;
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::meerkat_machine::dsl::PeerEndpoint;
use tokio::sync::Mutex;

pub struct ExampleGeneratedCommsTrustRouter {
    runtime_adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    comms_runtime: Arc<CommsRuntime>,
    operations: Mutex<()>,
}

impl ExampleGeneratedCommsTrustRouter {
    pub fn new(
        runtime_adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        comms_runtime: Arc<CommsRuntime>,
    ) -> Self {
        Self {
            runtime_adapter,
            session_id,
            comms_runtime,
            operations: Mutex::new(()),
        }
    }

    pub async fn add_trusted_peer(&self, peer: TrustedPeer) -> anyhow::Result<()> {
        let endpoint = endpoint_from_trusted_peer(&peer)?;
        let remove_peer_id = endpoint.peer_id.0.clone();
        let repair_peer_id = remove_peer_id.clone();
        let _guard = self.operations.lock().await;
        self.remove_matching_locked(
            move |existing| existing.peer_id.0 == remove_peer_id,
            Some(repair_peer_id.as_str()),
        )
        .await?;
        self.stage_add_locked(endpoint).await
    }

    pub async fn replace_named_trusted_peer(&self, peer: TrustedPeer) -> anyhow::Result<()> {
        let endpoint = endpoint_from_trusted_peer(&peer)?;
        let peer_name = endpoint.name.0.clone();
        let peer_id = endpoint.peer_id.0.clone();
        let remove_peer_name = peer_name.clone();
        let remove_peer_id = peer_id.clone();
        let _guard = self.operations.lock().await;
        self.remove_matching_locked(
            move |existing| {
                existing.name.0 == remove_peer_name || existing.peer_id.0 == remove_peer_id
            },
            Some(peer_id.as_str()),
        )
        .await?;
        self.stage_add_locked(endpoint).await
    }

    pub async fn remove_trusted_peer(&self, peer_id: &PeerId) -> anyhow::Result<()> {
        let peer_id = peer_id.to_string();
        let remove_peer_id = peer_id.clone();
        let _guard = self.operations.lock().await;
        self.remove_matching_locked(
            move |existing| existing.peer_id.0 == remove_peer_id,
            Some(peer_id.as_str()),
        )
        .await
    }

    async fn remove_matching_locked(
        &self,
        mut should_remove: impl FnMut(&PeerEndpoint) -> bool,
        repair_peer_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut removed = false;
        for endpoint in self.current_generated_endpoints().await? {
            if should_remove(&endpoint) {
                self.stage_remove_locked(endpoint).await?;
                removed = true;
            }
        }
        if !removed
            && let Some(peer_id) = repair_peer_id
        {
            self.stage_remove_peer_id_repair_locked(peer_id).await?;
        }
        Ok(())
    }

    async fn current_generated_endpoints(&self) -> anyhow::Result<Vec<PeerEndpoint>> {
        let endpoints = self
            .runtime_adapter
            .direct_peer_endpoints(&self.session_id)
            .await
            .context("read generated MeerkatMachine direct peer endpoints")?;
        Ok(endpoints.into_iter().collect())
    }

    async fn stage_add_locked(&self, endpoint: PeerEndpoint) -> anyhow::Result<()> {
        self.runtime_adapter
            .stage_add_direct_peer_endpoint(
                &self.session_id,
                endpoint,
                Arc::clone(&self.comms_runtime) as Arc<dyn CoreCommsRuntime>,
            )
            .await
            .context("generated MeerkatMachine add peer endpoint")
    }

    async fn stage_remove_locked(&self, endpoint: PeerEndpoint) -> anyhow::Result<()> {
        self.runtime_adapter
            .stage_remove_direct_peer_endpoint(
                &self.session_id,
                endpoint,
                Arc::clone(&self.comms_runtime) as Arc<dyn CoreCommsRuntime>,
            )
            .await
            .context("generated MeerkatMachine remove peer endpoint")
    }

    async fn stage_remove_peer_id_repair_locked(&self, peer_id: &str) -> anyhow::Result<()> {
        self.runtime_adapter
            .stage_repair_remove_direct_peer_id(
                &self.session_id,
                peer_id.to_string(),
                Arc::clone(&self.comms_runtime) as Arc<dyn CoreCommsRuntime>,
            )
            .await
            .context("generated MeerkatMachine repair remove peer endpoint")
    }
}

fn endpoint_from_trusted_peer(peer: &TrustedPeer) -> anyhow::Result<PeerEndpoint> {
    let descriptor = TrustedPeerDescriptor::unsigned_with_pubkey(
        peer.name.as_str(),
        peer.pubkey.to_peer_id().to_string(),
        *peer.pubkey.as_bytes(),
        peer.addr.as_str(),
    )
    .map_err(|detail| anyhow::anyhow!("invalid trusted peer {}: {detail}", peer.name))?;
    Ok(PeerEndpoint::from(&descriptor))
}
