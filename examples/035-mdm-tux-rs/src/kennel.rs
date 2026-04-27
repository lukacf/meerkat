use anyhow::{Context as _, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use meerkat_comms::identity::{Keypair, PubKey, Signature};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListScope {
    Available,
    Mine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KennelTargetState {
    Available,
    Claimed,
    RecoveringClaim,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetListEntry {
    pub target_id: String,
    pub name: String,
    pub state: KennelTargetState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    /// TCP address for JSON-RPC connections (when available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimGrant {
    pub lease_id: String,
    pub target_id: String,
    pub target_name: String,
    pub target_pubkey: String,
    pub target_direct_addr: String,
    /// TCP address for JSON-RPC connections (when available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_addr: Option<String>,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseView {
    pub lease_id: String,
    pub target_id: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LeaseRef {
    Known { lease_id: String },
    PendingRebind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRegistrationRejectReason {
    DuplicateName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseTerminationReason {
    ReleasedByTux,
    ClaimAckTimeout,
    RecoveryExpired,
    TuxDisconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KennelPayload {
    TargetRegister {
        target_id: String,
        name: String,
        pubkey: String,
        direct_addr: String,
        /// TCP address for JSON-RPC connections (e.g. "tcp://192.168.0.180:4800").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rpc_addr: Option<String>,
        #[serde(default)]
        labels: BTreeMap<String, String>,
        #[serde(default)]
        capabilities: BTreeMap<String, bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attached_tux_id: Option<String>,
    },
    TargetRegistered {
        /// Hive agent's comms pubkey — the target should add this as a trusted
        /// peer so the hive can send comms messages to it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hive_pubkey: Option<String>,
        /// Hive agent's comms address (tcp://host:port).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hive_comms_addr: Option<String>,
    },
    TargetRegistrationRejected {
        reason: TargetRegistrationRejectReason,
        message: String,
    },
    TargetHeartbeat,
    TuxRegister {
        tux_id: String,
        pubkey: String,
        #[serde(default)]
        attached_target_ids: Vec<String>,
    },
    TuxRegistered {
        /// RPC address for the kennel-resident hive agent (if available).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hive_rpc_addr: Option<String>,
        /// Session ID for the hive agent (pre-created by kennel).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hive_session_id: Option<String>,
    },
    ListTargets {
        scope: ListScope,
    },
    TargetList {
        scope: ListScope,
        targets: Vec<TargetListEntry>,
    },
    ClaimTargets {
        target_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease_ttl_sec: Option<u64>,
    },
    ClaimGranted {
        claims: Vec<ClaimGrant>,
    },
    ClaimAck {
        lease_ids: Vec<String>,
    },
    /// Kennel tells target about its new TUX owner (legacy: kept for target compat).
    Adopted {
        lease_id: String,
        target_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
        expires_at_ms: i64,
    },
    /// Kennel tells target about TUX reconnection with new address (recovery).
    LeaseRebound {
        lease_id: String,
        target_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
        target_pubkey: String,
        target_direct_addr: String,
        expires_at_ms: i64,
    },
    RenewLeases {
        lease_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease_ttl_sec: Option<u64>,
    },
    LeasesRenewed {
        leases: Vec<LeaseView>,
    },
    ReleaseTargets {
        lease_ids: Vec<String>,
    },
    Released {
        lease_ref: LeaseRef,
        reason: LeaseTerminationReason,
    },
    ClaimReleased {
        lease_ref: LeaseRef,
        target_id: String,
        reason: LeaseTerminationReason,
    },
    TuxHeartbeat,
    RebindTargets {
        target_ids: Vec<String>,
    },
    HivePrompt {
        prompt: String,
    },
    HiveStreamEvent {
        event: serde_json::Value,
    },
    HiveComplete {
        text: String,
    },
    HiveError {
        message: String,
    },
    /// Kennel tells a target to add another target as a trusted comms peer.
    /// Sent to both sides when the kennel wires two targets together.
    PeerWire {
        /// Human-readable name for the peer (the other target's name).
        peer_name: String,
        /// Comms public key of the peer (`ed25519:<base64>`).
        peer_id: String,
        /// Comms transport address of the peer (tcp://host:port).
        peer_addr: String,
    },
    /// Kennel tells a target to remove another target from its trusted peers.
    PeerUnwire {
        /// Comms public key of the peer to remove (`ed25519:<base64>`).
        peer_id: String,
    },
    Error {
        message: String,
    },
}

impl std::fmt::Display for LeaseTerminationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            LeaseTerminationReason::ReleasedByTux => "released_by_tux",
            LeaseTerminationReason::ClaimAckTimeout => "claim_ack_timeout",
            LeaseTerminationReason::RecoveryExpired => "recovery_expired",
            LeaseTerminationReason::TuxDisconnected => "tux_disconnected",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedKennelEnvelope {
    pub version: u32,
    pub message_id: String,
    pub issued_at_ms: i64,
    pub signer_id: String,
    pub payload: KennelPayload,
    pub signature: String,
}

#[derive(Debug, Serialize)]
struct UnsignedKennelEnvelope<'a> {
    version: u32,
    message_id: &'a str,
    issued_at_ms: i64,
    signer_id: &'a str,
    payload: &'a KennelPayload,
}

fn signable_bytes(
    version: u32,
    message_id: &str,
    issued_at_ms: i64,
    signer_id: &str,
    payload: &KennelPayload,
) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec(&UnsignedKennelEnvelope {
        version,
        message_id,
        issued_at_ms,
        signer_id,
        payload,
    })
    .context("serialize kennel envelope")
}

pub fn build_signed_envelope(
    keypair: &Keypair,
    _signer_id: &str,
    payload: KennelPayload,
) -> anyhow::Result<SignedKennelEnvelope> {
    let signer_id = keypair.public_key().to_pubkey_string();
    let message_id = Uuid::new_v4().to_string();
    let issued_at_ms = chrono::Utc::now().timestamp_millis();
    let signable = signable_bytes(1, &message_id, issued_at_ms, &signer_id, &payload)?;
    let signature = keypair.sign(&signable);
    Ok(SignedKennelEnvelope {
        version: 1,
        message_id,
        issued_at_ms,
        signer_id,
        payload,
        signature: BASE64.encode(signature.as_bytes()),
    })
}

pub fn verify_envelope(envelope: &SignedKennelEnvelope) -> anyhow::Result<PubKey> {
    if envelope.version != 1 {
        bail!("unsupported kennel envelope version {}", envelope.version);
    }
    let signer = PubKey::from_pubkey_string(&envelope.signer_id)
        .with_context(|| format!("bad signer id {}", envelope.signer_id))?;
    let signable = signable_bytes(
        envelope.version,
        &envelope.message_id,
        envelope.issued_at_ms,
        &envelope.signer_id,
        &envelope.payload,
    )?;
    let sig_bytes = BASE64
        .decode(envelope.signature.as_bytes())
        .context("decode kennel signature")?;
    if sig_bytes.len() != 64 {
        bail!("bad kennel signature length {}", sig_bytes.len());
    }
    let mut arr = [0u8; 64];
    arr.copy_from_slice(&sig_bytes);
    let signature = Signature::new(arr);
    if !signer.verify(&signable, &signature) {
        bail!("invalid kennel signature");
    }
    Ok(signer)
}

pub async fn write_envelope<W: AsyncWrite + Unpin>(
    writer: &mut W,
    envelope: &SignedKennelEnvelope,
) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(envelope).context("serialize kennel line")?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_envelope<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> anyhow::Result<Option<SignedKennelEnvelope>> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    let envelope = serde_json::from_str::<SignedKennelEnvelope>(line.trim())
        .context("parse kennel envelope")?;
    Ok(Some(envelope))
}
