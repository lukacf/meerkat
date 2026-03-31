use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use mdm_tux::machines::kennel_lease::{self, Effect, Event, State};
use mdm_tux::{
    ClaimGrant, KennelPayload, KennelTargetState, LeaseView, ListScope, SignedKennelEnvelope,
    TargetListEntry, build_signed_envelope, load_or_generate_keypair, read_envelope,
    verify_envelope, write_envelope,
};
use parking_lot::Mutex;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const DEFAULT_LEASE_TTL_SECS: u64 = 45;
const ACK_WINDOW_MS: i64 = 5_000;
const ATTACH_WINDOW_MS: i64 = 15_000;
const RECOVERY_WINDOW_MS: i64 = 60_000;

// ── Records (connection metadata — NOT lease state) ──────────────────────────

#[derive(Clone)]
struct TargetRecord {
    target_id: String,
    name: String,
    pubkey: String,
    direct_addr: String,
    #[allow(dead_code)]
    labels: BTreeMap<String, String>,
    #[allow(dead_code)]
    capabilities: BTreeMap<String, bool>,
    tx: mpsc::UnboundedSender<SignedKennelEnvelope>,
    /// Machine-owned lease lifecycle state.
    lease_state: State,
}

#[derive(Clone)]
struct TuxRecord {
    #[allow(dead_code)]
    tux_id: String,
    pubkey: String,
    direct_addr: String,
    tx: mpsc::UnboundedSender<SignedKennelEnvelope>,
}

#[derive(Default)]
struct KennelState {
    targets: HashMap<String, TargetRecord>,
    tuxes: HashMap<String, TuxRecord>,
    /// Lease ID → target ID index for fast lease lookups.
    lease_index: HashMap<String, String>,
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: mcm-kennel --listen HOST:PORT [--data-dir PATH]");
        std::process::exit(1);
    }
    let listen = find_flag(&args, "--listen")
        .or_else(|| args.first().cloned())
        .context("--listen HOST:PORT is required")?;
    let data_dir = find_flag(&args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rkat/mdm/kennel")
        });
    let keypair = Arc::new(load_or_generate_keypair(&data_dir.join("identity")).await?);
    let kennel_id = keypair.public_key().to_peer_id();

    let listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind kennel listener at {listen}"))?;
    let state = Arc::new(Mutex::new(KennelState::default()));

    println!("=== MCM Kennel ===");
    println!("listen    : {listen}");
    println!("kennel_id : {kennel_id}");

    tokio::spawn(run_janitor(
        state.clone(),
        keypair.clone(),
        kennel_id.clone(),
    ));

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let keypair = keypair.clone();
        let kennel_id = kennel_id.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state, keypair, kennel_id).await {
                eprintln!("[kennel] connection error: {e}");
            }
        });
    }
}

// ── Connection handler ───────────────────────────────────────────────────────

enum SessionKind {
    Target(String),
    Tux(String),
}

async fn handle_connection(
    stream: TcpStream,
    state: Arc<Mutex<KennelState>>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    kennel_id: String,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let (tx, mut rx) = mpsc::unbounded_channel::<SignedKennelEnvelope>();

    let writer_task = tokio::spawn(async move {
        while let Some(env) = rx.recv().await {
            if let Err(e) = write_envelope(&mut writer, &env).await {
                eprintln!("[kennel] write error: {e}");
                break;
            }
        }
    });

    let mut session_kind: Option<SessionKind> = None;

    loop {
        let Some(env) = read_envelope(&mut reader).await? else {
            break;
        };
        let signer = verify_envelope(&env)?;
        let signer_id = signer.to_peer_id();

        match &env.payload {
            KennelPayload::TargetRegister {
                target_id,
                name,
                pubkey,
                direct_addr,
                labels,
                capabilities,
                attached_tux_id,
            } => {
                anyhow::ensure!(target_id == &signer_id, "target signer_id mismatch");
                anyhow::ensure!(pubkey == &signer_id, "target pubkey mismatch");
                let now_ms = chrono::Utc::now().timestamp_millis();
                register_target(
                    &state,
                    RegisterTargetArgs {
                        target_id: target_id.clone(),
                        name: name.clone(),
                        pubkey: pubkey.clone(),
                        direct_addr: direct_addr.clone(),
                        labels: labels.clone(),
                        capabilities: capabilities.clone(),
                        attached_tux_id: attached_tux_id.clone(),
                        now_ms,
                        tx: tx.clone(),
                    },
                    &keypair,
                    &kennel_id,
                )?;
                let reply =
                    build_signed_envelope(&keypair, &kennel_id, KennelPayload::TargetRegistered)?;
                let _ = tx.send(reply);
                session_kind = Some(SessionKind::Target(target_id.clone()));
            }

            KennelPayload::TuxRegister {
                tux_id,
                pubkey,
                direct_addr,
                ..
            } => {
                anyhow::ensure!(tux_id == &signer_id, "tux signer_id mismatch");
                anyhow::ensure!(pubkey == &signer_id, "tux pubkey mismatch");
                {
                    let mut guard = state.lock();
                    guard.tuxes.insert(
                        tux_id.clone(),
                        TuxRecord {
                            tux_id: tux_id.clone(),
                            pubkey: pubkey.clone(),
                            direct_addr: direct_addr.clone(),
                            tx: tx.clone(),
                        },
                    );
                }
                let reply =
                    build_signed_envelope(&keypair, &kennel_id, KennelPayload::TuxRegistered)?;
                let _ = tx.send(reply);
                session_kind = Some(SessionKind::Tux(tux_id.clone()));
            }

            KennelPayload::ListTargets { scope } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let targets = {
                    let guard = state.lock();
                    list_targets(&guard, &tux_id, *scope)
                };
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::TargetList {
                        scope: *scope,
                        targets,
                    },
                )?;
                let _ = tx.send(reply);
            }

            KennelPayload::ClaimTargets {
                target_ids,
                lease_ttl_sec,
            } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let claims = {
                    let mut guard = state.lock();
                    handle_claim_targets(
                        &mut guard,
                        &tux_id,
                        target_ids,
                        lease_ttl_sec.unwrap_or(DEFAULT_LEASE_TTL_SECS),
                    )
                };
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::ClaimGranted { claims },
                )?;
                let _ = tx.send(reply);
            }

            KennelPayload::ClaimAck { lease_ids } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let mut guard = state.lock();
                handle_claim_ack(&mut guard, &keypair, &kennel_id, &tux_id, lease_ids);
            }

            KennelPayload::AttachConfirmed { lease_id } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let mut guard = state.lock();
                handle_attach_confirmed(&mut guard, &tux_id, lease_id);
            }

            KennelPayload::RenewLeases {
                lease_ids,
                lease_ttl_sec,
            } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let leases = {
                    let mut guard = state.lock();
                    handle_renew_leases(
                        &mut guard,
                        &tux_id,
                        lease_ids,
                        lease_ttl_sec.unwrap_or(DEFAULT_LEASE_TTL_SECS),
                    )
                };
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::LeasesRenewed { leases },
                )?;
                let _ = tx.send(reply);
            }

            KennelPayload::ReleaseTargets { lease_ids } => {
                if !matches!(&session_kind, Some(SessionKind::Tux(_))) {
                    continue;
                }
                let mut guard = state.lock();
                for lease_id in lease_ids {
                    apply_lease_event(
                        &mut guard,
                        lease_id,
                        Event::Released {
                            reason: "released_by_tux".into(),
                        },
                        &keypair,
                        &kennel_id,
                    );
                }
                drop(guard);
            }

            KennelPayload::RebindTargets { target_ids } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let mut guard = state.lock();
                handle_rebind_targets(&mut guard, &keypair, &kennel_id, &tux_id, target_ids);
            }

            KennelPayload::TuxHeartbeat | KennelPayload::TargetHeartbeat => {}
            _ => {}
        }
    }

    writer_task.abort();

    if let Some(kind) = session_kind {
        let mut guard = state.lock();
        match kind {
            SessionKind::Target(target_id) => {
                handle_target_disconnect(&mut guard, &keypair, &kennel_id, &target_id);
            }
            SessionKind::Tux(tux_id) => {
                handle_tux_disconnect(&mut guard, &keypair, &kennel_id, &tux_id);
            }
        }
    }

    Ok(())
}

// ── Machine-backed operations ────────────────────────────────────────────────

struct RegisterTargetArgs {
    target_id: String,
    name: String,
    pubkey: String,
    direct_addr: String,
    labels: BTreeMap<String, String>,
    capabilities: BTreeMap<String, bool>,
    attached_tux_id: Option<String>,
    now_ms: i64,
    tx: mpsc::UnboundedSender<SignedKennelEnvelope>,
}

fn register_target(
    state: &Arc<Mutex<KennelState>>,
    args: RegisterTargetArgs,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) -> anyhow::Result<()> {
    let RegisterTargetArgs {
        target_id,
        name,
        pubkey,
        direct_addr,
        labels,
        capabilities,
        attached_tux_id,
        now_ms,
        tx,
    } = args;
    let mut guard = state.lock();
    if let Some(existing) = guard
        .targets
        .values()
        .find(|t| t.name == name && t.target_id != target_id)
    {
        bail!(
            "target name '{}' already registered by {}",
            existing.name,
            existing.target_id
        );
    }

    // Use the machine for state initialization — never construct states directly.
    let new_initial_state = match &attached_tux_id {
        Some(tux_id) => {
            let (state, _effects) = kennel_lease::transition(
                State::Available,
                Event::TargetReregisteredWithAttachment {
                    target_id: target_id.clone(),
                    tux_id: tux_id.clone(),
                    now_ms,
                    recovery_window_ms: RECOVERY_WINDOW_MS,
                },
            )
            .unwrap_or((State::Available, vec![]));
            state
        }
        None => State::Available,
    };

    // Extract cleanup data from the old record before mutating guard.
    let old_record_info = guard.targets.get(&target_id).map(|existing| {
        let old_lease_id = match &existing.lease_state {
            State::AwaitingAck { lease_id, .. }
            | State::AwaitingAttach { lease_id, .. }
            | State::Claimed { lease_id, .. } => Some(lease_id.clone()),
            _ => None,
        };
        let addr_changed = existing.direct_addr != direct_addr;
        let old_state = existing.lease_state.clone();
        (old_lease_id, addr_changed, old_state)
    });
    if let Some((old_lease_id, addr_changed, old_state)) = old_record_info {
        // Clean up orphaned lease_index entries from the old state.
        if let Some(lid) = &old_lease_id {
            guard.lease_index.remove(lid);
        }
        // If address changed while in an active claim (not RecoveringClaim),
        // notify TUX via TargetAddressChanged so it can re-claim with the
        // new address. RecoveringClaim address changes are expected (targets
        // bind port 0 on restart) and should be silently accepted — the
        // rebind path picks up the new address from the TargetRecord.
        if addr_changed
            && !matches!(old_state, State::Available | State::RecoveringClaim { .. })
            && let Ok((_new_state, effects)) =
                kennel_lease::transition(old_state, Event::TargetAddressChanged)
        {
            dispatch_effects(&effects, &mut guard, &target_id, keypair, kennel_id);
        }
    }

    guard.targets.insert(
        target_id.clone(),
        TargetRecord {
            target_id,
            name,
            pubkey,
            direct_addr,
            labels,
            capabilities,
            tx,
            lease_state: new_initial_state,
        },
    );
    Ok(())
}

fn list_targets(state: &KennelState, tux_id: &str, scope: ListScope) -> Vec<TargetListEntry> {
    let mut out = Vec::new();
    for target in state.targets.values() {
        let entry = match (&scope, &target.lease_state) {
            (ListScope::Available, State::Available) => Some(TargetListEntry {
                target_id: target.target_id.clone(),
                name: target.name.clone(),
                state: KennelTargetState::Available,
                lease_id: None,
            }),
            (
                ListScope::Mine,
                State::AwaitingAck {
                    tux_id: owner,
                    lease_id,
                    ..
                },
            )
            | (
                ListScope::Mine,
                State::AwaitingAttach {
                    tux_id: owner,
                    lease_id,
                    ..
                },
            ) if owner == tux_id => Some(TargetListEntry {
                target_id: target.target_id.clone(),
                name: target.name.clone(),
                state: KennelTargetState::PendingAttach,
                lease_id: Some(lease_id.clone()),
            }),
            (
                ListScope::Mine,
                State::Claimed {
                    tux_id: owner,
                    lease_id,
                    ..
                },
            ) if owner == tux_id => Some(TargetListEntry {
                target_id: target.target_id.clone(),
                name: target.name.clone(),
                state: KennelTargetState::Claimed,
                lease_id: Some(lease_id.clone()),
            }),
            (ListScope::Mine, State::RecoveringClaim { tux_id: owner, .. }) if owner == tux_id => {
                Some(TargetListEntry {
                    target_id: target.target_id.clone(),
                    name: target.name.clone(),
                    state: KennelTargetState::RecoveringClaim,
                    lease_id: None,
                })
            }
            _ => None,
        };
        if let Some(e) = entry {
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn handle_claim_targets(
    state: &mut KennelState,
    tux_id: &str,
    target_ids: &[String],
    ttl_secs: u64,
) -> Vec<ClaimGrant> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_at_ms = now_ms + (ttl_secs as i64 * 1000);
    let ack_deadline_ms = now_ms + ACK_WINDOW_MS;
    let mut claims = Vec::new();

    for target_id in target_ids {
        let Some(target) = state.targets.get_mut(target_id) else {
            continue;
        };
        let lease_id = uuid::Uuid::new_v4().to_string();
        let event = Event::ClaimRequested {
            target_id: target_id.clone(),
            lease_id: lease_id.clone(),
            tux_id: tux_id.to_string(),
            expires_at_ms,
            ack_deadline_ms,
        };
        let Ok((new_state, _effects)) = kennel_lease::transition(target.lease_state.clone(), event)
        else {
            continue; // target not Available
        };
        target.lease_state = new_state;
        state
            .lease_index
            .insert(lease_id.clone(), target_id.clone());
        claims.push(ClaimGrant {
            lease_id,
            target_id: target.target_id.clone(),
            target_name: target.name.clone(),
            target_pubkey: target.pubkey.clone(),
            target_direct_addr: target.direct_addr.clone(),
            expires_at_ms,
        });
    }
    claims
}

fn handle_claim_ack(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    tux_id: &str,
    lease_ids: &[String],
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let attach_deadline_ms = now_ms + ATTACH_WINDOW_MS;

    for lease_id in lease_ids {
        let Some(target_id) = state.lease_index.get(lease_id).cloned() else {
            continue;
        };
        let Some(target) = state.targets.get_mut(&target_id) else {
            continue;
        };
        let event = Event::ClaimAcked {
            lease_id: lease_id.clone(),
            tux_id: tux_id.to_string(),
            attach_deadline_ms,
        };
        let Ok((new_state, effects)) = kennel_lease::transition(target.lease_state.clone(), event)
        else {
            continue;
        };
        target.lease_state = new_state;
        dispatch_effects(&effects, state, &target_id, keypair, kennel_id);
    }
}

fn handle_attach_confirmed(state: &mut KennelState, tux_id: &str, lease_id: &str) {
    let Some(target_id) = state.lease_index.get(lease_id).cloned() else {
        return;
    };
    let Some(target) = state.targets.get_mut(&target_id) else {
        return;
    };
    let event = Event::AttachConfirmed {
        lease_id: lease_id.to_string(),
        tux_id: tux_id.to_string(),
    };
    if let Ok((new_state, _effects)) = kennel_lease::transition(target.lease_state.clone(), event) {
        target.lease_state = new_state;
    }
}

fn handle_renew_leases(
    state: &mut KennelState,
    tux_id: &str,
    lease_ids: &[String],
    ttl_secs: u64,
) -> Vec<LeaseView> {
    let new_expires_at_ms = chrono::Utc::now().timestamp_millis() + (ttl_secs as i64 * 1000);
    let mut leases = Vec::new();

    for lease_id in lease_ids {
        let Some(target_id) = state.lease_index.get(lease_id).cloned() else {
            continue;
        };
        let Some(target) = state.targets.get_mut(&target_id) else {
            continue;
        };
        let event = Event::LeaseRenewed {
            lease_id: lease_id.clone(),
            tux_id: tux_id.to_string(),
            new_expires_at_ms,
        };
        let Ok((new_state, _effects)) = kennel_lease::transition(target.lease_state.clone(), event)
        else {
            continue;
        };
        target.lease_state = new_state;
        leases.push(LeaseView {
            lease_id: lease_id.clone(),
            target_id,
            expires_at_ms: new_expires_at_ms,
        });
    }
    leases
}

fn handle_rebind_targets(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    tux_id: &str,
    target_ids: &[String],
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let new_expires_at_ms = now_ms + (DEFAULT_LEASE_TTL_SECS as i64 * 1000);

    for target_id in target_ids {
        let Some(target) = state.targets.get_mut(target_id) else {
            continue;
        };
        let new_lease_id = uuid::Uuid::new_v4().to_string();
        let event = Event::Rebound {
            new_lease_id: new_lease_id.clone(),
            tux_id: tux_id.to_string(),
            new_expires_at_ms,
        };
        let Ok((new_state, effects)) = kennel_lease::transition(target.lease_state.clone(), event)
        else {
            continue;
        };
        target.lease_state = new_state;
        state.lease_index.insert(new_lease_id, target_id.clone());
        dispatch_effects(&effects, state, target_id, keypair, kennel_id);
    }
}

fn handle_target_disconnect(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    target_id: &str,
) {
    apply_target_event(
        state,
        target_id,
        Event::TargetDisconnected {
            now_ms: chrono::Utc::now().timestamp_millis(),
            recovery_window_ms: RECOVERY_WINDOW_MS,
        },
        keypair,
        kennel_id,
    );
}

fn handle_tux_disconnect(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    tux_id: &str,
) {
    state.tuxes.remove(tux_id);
    let target_ids: Vec<String> = state.targets.keys().cloned().collect();
    for target_id in target_ids {
        let Some(target) = state.targets.get(&target_id) else {
            continue;
        };
        let owner_matches = match &target.lease_state {
            State::AwaitingAck { tux_id: owner, .. }
            | State::AwaitingAttach { tux_id: owner, .. }
            | State::Claimed { tux_id: owner, .. }
            | State::RecoveringClaim { tux_id: owner, .. } => owner == tux_id,
            State::Available => false,
        };
        if owner_matches {
            apply_target_event(
                state,
                &target_id,
                Event::TuxDisconnected {
                    tux_id: tux_id.to_string(),
                    now_ms: chrono::Utc::now().timestamp_millis(),
                    recovery_window_ms: RECOVERY_WINDOW_MS,
                },
                keypair,
                kennel_id,
            );
        }
    }
}

/// Apply an event to a target by lease_id lookup.
fn apply_lease_event(
    state: &mut KennelState,
    lease_id: &str,
    event: Event,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) {
    let Some(target_id) = state.lease_index.get(lease_id).cloned() else {
        return;
    };
    apply_target_event(state, &target_id, event, keypair, kennel_id);
}

/// Apply an event to a target by target_id.
fn apply_target_event(
    state: &mut KennelState,
    target_id: &str,
    event: Event,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) {
    let Some(target) = state.targets.get_mut(target_id) else {
        return;
    };
    let Ok((new_state, effects)) = kennel_lease::transition(target.lease_state.clone(), event)
    else {
        return;
    };
    target.lease_state = new_state;
    dispatch_effects(&effects, state, target_id, keypair, kennel_id);
}

// ── Effect dispatch ──────────────────────────────────────────────────────────

fn dispatch_effects(
    effects: &[Effect],
    state: &mut KennelState,
    target_id: &str,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) {
    for effect in effects {
        match effect {
            Effect::SendAdoptedToTarget {
                target_id: _,
                lease_id,
                tux_id,
                expires_at_ms,
            } => {
                let tux = state.tuxes.get(tux_id);
                if let Some(target) = state.targets.get(target_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::Adopted {
                            lease_id: lease_id.clone(),
                            target_id: target_id.to_string(),
                            tux_id: tux_id.clone(),
                            tux_pubkey: tux.map(|t| t.pubkey.clone()).unwrap_or_default(),
                            tux_direct_addr: tux.map(|t| t.direct_addr.clone()).unwrap_or_default(),
                            expires_at_ms: *expires_at_ms,
                        },
                    )
                {
                    let _ = target.tx.send(env);
                }
            }
            Effect::SendTargetReleased {
                target_id: _,
                lease_id,
                reason,
            } => {
                if let Some(target) = state.targets.get(target_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::Released {
                            lease_id: lease_id.clone().unwrap_or_default(),
                            reason: reason.clone(),
                        },
                    )
                {
                    let _ = target.tx.send(env);
                }
            }
            Effect::SendClaimReleasedToTux {
                target_id: _,
                lease_id,
                tux_id,
                reason,
            } => {
                if let Some(tux) = state.tuxes.get(tux_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::ClaimReleased {
                            lease_id: lease_id.clone().unwrap_or_default(),
                            target_id: target_id.to_string(),
                            reason: reason.clone(),
                        },
                    )
                {
                    let _ = tux.tx.send(env);
                }
            }
            Effect::SendTargetLostToTux {
                target_id: _,
                tux_id,
                lease_id,
            } => {
                if let Some(tux) = state.tuxes.get(tux_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::TargetLost {
                            target_id: target_id.to_string(),
                            lease_id: lease_id.clone(),
                        },
                    )
                {
                    let _ = tux.tx.send(env);
                }
            }
            Effect::SendLeaseRebound {
                target_id: _,
                lease_id,
                tux_id,
                expires_at_ms,
            } => {
                let tux_info = state.tuxes.get(tux_id);
                let tux_pubkey = tux_info.map(|t| t.pubkey.clone()).unwrap_or_default();
                let tux_direct_addr = tux_info.map(|t| t.direct_addr.clone()).unwrap_or_default();
                if let Some(target) = state.targets.get(target_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::LeaseRebound {
                            lease_id: lease_id.clone(),
                            target_id: target_id.to_string(),
                            tux_id: tux_id.clone(),
                            tux_pubkey,
                            tux_direct_addr,
                            target_pubkey: target.pubkey.clone(),
                            target_direct_addr: target.direct_addr.clone(),
                            expires_at_ms: *expires_at_ms,
                        },
                    )
                {
                    let _ = target.tx.send(env.clone());
                    if let Some(tux) = state.tuxes.get(tux_id) {
                        let _ = tux.tx.send(env);
                    }
                }
            }
            Effect::RemoveLease { lease_id } => {
                state.lease_index.remove(lease_id);
            }
            Effect::DropTargetRecord { target_id: effect_tid } => {
                // Use the effect's target_id if provided, otherwise fall
                // back to the caller's target_id (for Available state which
                // doesn't carry target_id).
                let tid = if effect_tid.is_empty() { target_id } else { effect_tid.as_str() };
                state.targets.remove(tid);
            }
        }
    }
}

// ── Janitor ──────────────────────────────────────────────────────────────────

async fn run_janitor(
    state: Arc<Mutex<KennelState>>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    kennel_id: String,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut guard = state.lock();

        // Collect target IDs that need tick processing (avoid borrow issues)
        let target_ids: Vec<String> = guard.targets.keys().cloned().collect();
        for target_id in target_ids {
            apply_target_event(
                &mut guard,
                &target_id,
                Event::Tick { now_ms },
                &keypair,
                &kennel_id,
            );
        }

        // Remove phantom targets: disconnected records that expired back to
        // Available. These have a closed tx (dead TCP connection) and must
        // not appear in listings or accept new claims.
        guard.targets.retain(|_, target| {
            if matches!(target.lease_state, State::Available) && target.tx.is_closed() {
                false // remove phantom
            } else {
                true
            }
        });
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}
