//! The one original parent channel and the one actual native invocation.
use super::{
    ProtectedCollectionCatalog, ProtectedServiceInputs, binding, refused,
    transport::ParentChannel,
    wire::{self, Control, Disposition, Envelope, Nonce, ParentRecord},
};
use crate::{
    ConnectError,
    service_probe::{ServiceProbeControl, now},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};

const HANDSHAKE: Duration = Duration::from_secs(20);
const DISPOSAL_OBSERVATION: Duration = Duration::from_secs(30);

struct RetireOnLoss(ServiceProbeControl);
impl Drop for RetireOnLoss {
    fn drop(&mut self) {
        self.0.retire();
    }
}

async fn until<T>(
    deadline: Instant,
    operation: impl Future<Output = Result<T, ConnectError>>,
) -> Result<T, ConnectError> {
    if Instant::now() >= deadline {
        return Err(refused());
    }
    tokio::time::timeout_at(deadline, operation)
        .await
        .map_err(|_| refused())?
}
async fn send(
    channel: &mut ParentChannel,
    bytes: Vec<u8>,
    deadline: Instant,
) -> Result<(), ConnectError> {
    until(deadline, async {
        channel.send(&bytes).await.map_err(|_| refused())
    })
    .await
}
async fn receive(
    channel: &mut ParentChannel,
    deadline: Instant,
) -> Result<super::transport::VerifiedParentPacket, ConnectError> {
    until(deadline, async {
        channel.receive().await.map_err(|_| refused())
    })
    .await
}

fn checked_control(
    control: &Control,
    owner: &Envelope,
    allocation: &Nonce,
    previous_requests: &[&Nonce],
) -> Result<(), ConnectError> {
    if !control.envelope.same_owner(owner)
        || control.control_id != *allocation
        || previous_requests
            .iter()
            .any(|id| **id == control.envelope.request_id)
    {
        return Err(refused());
    }
    Ok(())
}

fn authentication_deadline(expires_at_ms: u64) -> Result<Instant, ConnectError> {
    let remaining = expires_at_ms
        .checked_sub(now())
        .filter(|value| *value > 0)
        .ok_or_else(refused)?;
    Instant::now()
        .checked_add(Duration::from_millis(remaining))
        .ok_or_else(refused)
}

pub(super) async fn run(
    inputs: ProtectedServiceInputs,
    catalog: Arc<dyn ProtectedCollectionCatalog>,
) -> Result<(), ConnectError> {
    let ProtectedServiceInputs {
        parent,
        input,
        output,
        sealed,
        mapping,
        peer,
        provider_store_root,
        _resources,
        selected_input,
    } = inputs;
    let mut channel = ParentChannel::adopt(parent).map_err(|_| refused())?;
    let pipes = crate::stdio::ProtectedCollectionPipes::adopt(input, output)?;
    let digest: String = sealed
        .sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let digest = Nonce::try_from(digest).map_err(|_| refused())?;
    let invocation = Nonce::fresh();
    let challenge = Nonce::fresh();
    let handshake = Instant::now() + HANDSHAKE;
    send(
        &mut channel,
        wire::hello(&invocation, &challenge, &digest).map_err(|_| refused())?,
        handshake,
    )
    .await?;
    let observation = receive(&mut channel, handshake).await?;
    if observation.peer() != peer {
        return Err(refused());
    }
    let ParentRecord::Attest(attestation) =
        wire::decode(observation.bytes()).map_err(|_| refused())?
    else {
        return Err(refused());
    };
    if attestation.challenge != challenge
        || attestation.envelope.invocation != invocation
        || attestation.envelope.mapping_sha256 != digest
        || attestation.expires_at_ms <= now()
    {
        return Err(refused());
    }
    authentication_deadline(attestation.expires_at_ms)?;
    let mut selected = binding::select(&mapping, &observation, &attestation, catalog.as_ref())?;
    selected.selected_input = selected_input.clone();
    let control = selected.control();
    let _retire_on_loss = RetireOnLoss(control.clone());
    let allocation = Nonce::fresh();
    // Keep the original kernel observation and sealed bytes for this entire
    // owner. The native evidence reference addresses this retained observation.
    let _retained = (sealed, observation, mapping, _resources, selected_input);
    if attestation.expires_at_ms <= now() {
        return Err(refused());
    }
    send(
        &mut channel,
        attestation
            .envelope
            .bound(&allocation)
            .map_err(|_| refused())?,
        handshake,
    )
    .await?;
    let packet = receive(&mut channel, handshake).await?;
    let activation = match wire::decode(packet.bytes()).map_err(|_| refused())? {
        ParentRecord::Activate(activation) => {
            checked_control(
                &activation,
                &attestation.envelope,
                &allocation,
                &[&attestation.envelope.request_id],
            )?;
            if attestation.expires_at_ms <= now() {
                return Err(refused());
            }
            activation
        }
        ParentRecord::Fence(fence) => {
            checked_control(
                &fence,
                &attestation.envelope,
                &allocation,
                &[&attestation.envelope.request_id],
            )?;
            let receipt = control.fence()?;
            if !receipt.belongs_to(&control) {
                return Err(refused());
            }
            send(
                &mut channel,
                fence
                    .envelope
                    .fence_ack(&allocation)
                    .map_err(|_| refused())?,
                Instant::now() + HANDSHAKE,
            )
            .await?;
            drop(pipes);
            drop(selected);
            // Never entered is deliberately unconfirmed under protected parent protocol 1.
            send(
                &mut channel,
                fence
                    .envelope
                    .disposed(&allocation, Disposition::Unconfirmed)
                    .map_err(|_| refused())?,
                Instant::now() + HANDSHAKE,
            )
            .await?;
            return Err(refused());
        }
        ParentRecord::Attest(_) => return Err(refused()),
    };
    run_activated(
        &mut channel,
        pipes,
        selected,
        provider_store_root,
        ActiveBinding {
            owner: attestation.envelope,
            activation,
            allocation,
            expires_at_ms: attestation.expires_at_ms,
        },
    )
    .await
}

// Only the checked handshake above supplies production activation. Keeping this
// exact continuation separate permits actual socket/stdio custody controls
// without pretending their same-process transport authenticated a root parent.
struct ActiveBinding {
    owner: Envelope,
    activation: Control,
    allocation: Nonce,
    expires_at_ms: u64,
}

async fn run_activated(
    channel: &mut ParentChannel,
    pipes: crate::stdio::ProtectedCollectionPipes,
    selected: crate::service_collection::ServiceCollectionSelection,
    provider_store_root: std::path::PathBuf,
    binding: ActiveBinding,
) -> Result<(), ConnectError> {
    let control = selected.control();
    let (done_tx, done_rx) = watch::channel(false);
    let native = async {
        let result = pipes.run(selected, Some(provider_store_root)).await;
        done_tx.send_replace(true);
        result
    };
    let parent = async {
        let result = active_parent(
            channel,
            &control,
            &binding.owner,
            &binding.activation,
            &binding.allocation,
            binding.expires_at_ms,
            done_rx,
        )
        .await;
        if result.is_err() {
            control.retire();
        }
        result
    };
    // Poll native first so ACTIVATE really registers the exact invocation before
    // a queued FENCE is read. An error/timeout of the parent observer never drops
    // this invocation: join continues driving its original close and workers.
    let (native, parent) = tokio::join!(biased; native, parent);
    match (native, parent) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(native), Ok(())) => Err(native),
        (Ok(()), Err(parent)) => Err(parent),
        (Err(native), Err(parent)) => Err(ConnectError::Cleanup {
            primary: Box::new(native),
            cleanup: Box::new(parent),
        }),
    }
}

async fn active_parent(
    channel: &mut ParentChannel,
    control: &ServiceProbeControl,
    owner: &Envelope,
    activation: &Control,
    allocation: &Nonce,
    expires_at_ms: u64,
    mut done: watch::Receiver<bool>,
) -> Result<(), ConnectError> {
    let mut deadline = authentication_deadline(expires_at_ms)?;
    let mut native_done = false;
    let packet = loop {
        tokio::select! { biased;
            changed = done.changed(), if !native_done => {
                if changed.is_err() || !*done.borrow() { return Err(refused()); }
                native_done = true;
                control.retire();
                // Ordinary EOF/close can finish before the parent asks to
                // fence. Allow its original bounded close exchange, not a new
                // reservation or an indefinitely retained idle process.
                deadline = deadline.min(Instant::now() + HANDSHAKE);
            },
            packet = receive(channel, deadline) => break packet?,
        }
    };
    let ParentRecord::Fence(fence) = wire::decode(packet.bytes()).map_err(|_| refused())? else {
        return Err(refused());
    };
    checked_control(
        &fence,
        owner,
        allocation,
        &[&owner.request_id, &activation.envelope.request_id],
    )?;
    let receipt = control.fence()?;
    if !receipt.belongs_to(control) {
        return Err(refused());
    }
    send(
        channel,
        fence
            .envelope
            .fence_ack(allocation)
            .map_err(|_| refused())?,
        Instant::now() + HANDSHAKE,
    )
    .await?;
    let joined = tokio::time::timeout(DISPOSAL_OBSERVATION, receipt.joined()).await;
    let confirmed = matches!(joined, Ok(Ok(ref closed)) if closed.belongs_to(control));
    send(
        channel,
        fence
            .envelope
            .disposed(
                allocation,
                if confirmed {
                    Disposition::Joined
                } else {
                    Disposition::Unconfirmed
                },
            )
            .map_err(|_| refused())?,
        Instant::now() + HANDSHAKE,
    )
    .await?;
    if confirmed { Ok(()) } else { Err(refused()) }
}

#[cfg(test)]
mod tests;
