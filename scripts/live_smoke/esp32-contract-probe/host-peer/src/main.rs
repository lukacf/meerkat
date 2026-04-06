use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use meerkat_comms::agent::{CommsMessage, spawn_tcp_listener};
use meerkat_comms::agent::types::CommsContent;
use meerkat_comms::{
    CommsConfig, Inbox, Keypair, PubKey, Router, TrustedPeer, TrustedPeers,
};
use parking_lot::RwLock;
use serde_json::json;
use tokio::time::timeout;

const PHASE0_HOST_SECRET: [u8; 32] = [7; 32];

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    let peer_pubkey = PubKey::from_peer_id(&args.peer_id).context("invalid ESP peer id")?;

    let trusted = Arc::new(RwLock::new(TrustedPeers {
        peers: vec![TrustedPeer {
            name: args.peer_name.clone(),
            pubkey: peer_pubkey,
            addr: args.peer_addr.clone(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }));
    let (mut inbox, inbox_sender) = Inbox::new();
    let router = Router::with_shared_peers(
        Keypair::from_secret(PHASE0_HOST_SECRET),
        trusted.clone(),
        CommsConfig::default(),
        inbox_sender.clone(),
        true,
    );
    let host_keypair = router.keypair_arc();

    let listener = spawn_tcp_listener(
        &args.listen_addr,
        host_keypair.clone(),
        trusted.clone(),
        inbox_sender,
    )
    .await
    .with_context(|| format!("failed to bind host listener on {}", args.listen_addr))?;

    println!(
        "MKT:HOST_COMMS:LISTENING peer_id={} addr={}",
        json_quote(&host_keypair.public_key().to_peer_id()),
        json_quote(&args.listen_addr)
    );

    let request_id = router
        .send_request(
            &args.peer_name,
            "phase0_probe".to_string(),
            json!({ "message": args.message }),
        )
        .await
        .context("failed to send comms request to ESP32 peer")?;
    println!(
        "MKT:HOST_COMMS:REQUEST_SENT request_id={} peer={}",
        json_quote(&request_id.to_string()),
        json_quote(&args.peer_name)
    );

    let response = timeout(Duration::from_secs(args.timeout_secs), async {
        loop {
            let item = inbox
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("host inbox closed"))?;
            let trusted_snapshot = trusted.read().clone();
            let Some(message) = CommsMessage::from_inbox_item(&item, &trusted_snapshot, true) else {
                continue;
            };
            if let CommsContent::Response { in_reply_to, result, .. } = &message.content {
                if *in_reply_to == request_id {
                    return Ok::<(String, serde_json::Value), anyhow::Error>((
                        message.from_peer.clone(),
                        result.clone(),
                    ));
                }
            }
        }
    })
    .await
    .context("timed out waiting for ESP32 response")??;

    println!(
        "MKT:HOST_COMMS:RESPONSE_RECEIVED peer={} result={}",
        json_quote(&response.0),
        json_quote(&response.1.to_string())
    );
    println!(
        "MKT:HOST_COMMS:PASS request_id={} peer={}",
        json_quote(&request_id.to_string()),
        json_quote(&response.0)
    );

    listener.abort();
    Ok(())
}

struct Args {
    peer_name: String,
    peer_id: String,
    peer_addr: String,
    listen_addr: String,
    message: String,
    timeout_secs: u64,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut peer_name = None;
        let mut peer_id = None;
        let mut peer_addr = None;
        let mut listen_addr = None;
        let mut message = None;
        let mut timeout_secs = 30_u64;

        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--peer-name" => peer_name = iter.next(),
                "--peer-id" => peer_id = iter.next(),
                "--peer-addr" => peer_addr = iter.next(),
                "--listen-addr" => listen_addr = iter.next(),
                "--message" => message = iter.next(),
                "--timeout-secs" => {
                    let value = iter.next().context("missing value for --timeout-secs")?;
                    timeout_secs = value.parse().context("invalid --timeout-secs value")?;
                }
                other => bail!("unknown argument: {other}"),
            }
        }

        Ok(Self {
            peer_name: peer_name.context("missing --peer-name")?,
            peer_id: peer_id.context("missing --peer-id")?,
            peer_addr: peer_addr.context("missing --peer-addr")?,
            listen_addr: listen_addr.context("missing --listen-addr")?,
            message: message.context("missing --message")?,
            timeout_secs,
        })
    }
}

fn json_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<encoding-error>\"".to_string())
}
