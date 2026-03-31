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
    PendingAttach,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimGrant {
    pub lease_id: String,
    pub target_id: String,
    pub target_name: String,
    pub target_pubkey: String,
    pub target_direct_addr: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseView {
    pub lease_id: String,
    pub target_id: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KennelPayload {
    TargetRegister {
        target_id: String,
        name: String,
        pubkey: String,
        direct_addr: String,
        #[serde(default)]
        labels: BTreeMap<String, String>,
        #[serde(default)]
        capabilities: BTreeMap<String, bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attached_tux_id: Option<String>,
    },
    TargetRegistered,
    TargetHeartbeat,
    TuxRegister {
        tux_id: String,
        pubkey: String,
        direct_addr: String,
        #[serde(default)]
        attached_target_ids: Vec<String>,
    },
    TuxRegistered,
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
    Adopted {
        lease_id: String,
        target_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
        expires_at_ms: i64,
    },
    AttachConfirmed {
        lease_id: String,
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
        lease_id: String,
        reason: String,
    },
    ClaimReleased {
        lease_id: String,
        target_id: String,
        reason: String,
    },
    TargetLost {
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease_id: Option<String>,
    },
    TuxHeartbeat,
    RebindTargets {
        target_ids: Vec<String>,
    },
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
    Error {
        message: String,
    },
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
    signer_id: &str,
    payload: KennelPayload,
) -> anyhow::Result<SignedKennelEnvelope> {
    let message_id = Uuid::new_v4().to_string();
    let issued_at_ms = chrono::Utc::now().timestamp_millis();
    let signable = signable_bytes(1, &message_id, issued_at_ms, signer_id, &payload)?;
    let signature = keypair.sign(&signable);
    Ok(SignedKennelEnvelope {
        version: 1,
        message_id,
        issued_at_ms,
        signer_id: signer_id.to_string(),
        payload,
        signature: BASE64.encode(signature.as_bytes()),
    })
}

pub fn verify_envelope(envelope: &SignedKennelEnvelope) -> anyhow::Result<PubKey> {
    if envelope.version != 1 {
        bail!("unsupported kennel envelope version {}", envelope.version);
    }
    let signer = PubKey::from_peer_id(&envelope.signer_id)
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
