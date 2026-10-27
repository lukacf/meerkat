//! Human-only PairedHost pairing presentation. This is not a model tool, native
//! approval or frontend activation. Device persistence delegates to the OS adapter.

use std::{
    future::Future,
    io::{self, Write},
    pin::Pin,
    sync::Arc,
};

use futures_util::FutureExt;

use crate::{
    ConnectError,
    paired_host::{
        FrontendInventoryRead, PairedHostClient, PairedHostError, PairedHostOperation, PairedHostRefusal,
        PairingCode, TerminalPairingCode,
    },
    paired_host_device::{self, DeviceStore, DeviceStoreError, LoadedDevice, StoreJob},
    terminal::{TerminalInput, display_text},
};

const DEVICE_LABEL: &str = "Meerkat Connect CLI";

enum EnteredCode {
    Telegram(PairingCode),
    Terminal(TerminalPairingCode),
}

/// Production presenter composition. Store access and code entry remain outside
/// the model. The supplied store is trusted OS custody, not caller grant data.
/// An interrupted/dropped store wait joins its blocking call before returning.
pub async fn pair_paired_host_with_store(
    human_selected_origin: &str,
    store: Arc<dyn DeviceStore>,
    input: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<Option<PairedHostClient>, ConnectError> {
    let mut client = PairedHostClient::new(human_selected_origin)
        .map_err(|error| ConnectError::native("select paired host", error))?;
    input
        .require_terminal()
        .map_err(|error| ConnectError::native("check protected code entry", error))?;
    tokio::pin!(interrupted);
    let Some(loaded) = await_store(
        || paired_host_device::load(store.clone(), &client),
        interrupted.as_mut(),
        output,
    )
    .await?
    else {
        return Ok(None);
    };
    let mut remember = true;
    match loaded {
        Ok(LoadedDevice::Ready(record)) => {
            let Some(choice) = device_choice(input, output, interrupted.as_mut(),
                "A saved pairing is available. Press Enter to use it, type 'forget' to forget it locally, or /quit.",
                &["", "forget"]).await? else { return Ok(None); };
            if choice == "forget" {
                forget_selected(store.clone(), &client, interrupted.as_mut(), output).await?;
                return Ok(None);
            }
            client
                .restore_device(record)
                .map_err(|error| ConnectError::native("restore saved PairedHost pairing", error))?;
            let admitted = tokio::select! {
                biased;
                signal = &mut interrupted => return Err(unconfirmed_interruption(signal, "checking saved pairing; the saved credential was retained")),
                result = client.frontend_inventory() => result,
            };
            match admitted {
                Ok(FrontendInventoryRead::Inventory(_)) => {
                    say(output, "PairedHost accepted the saved pairing. Current frontend choices will be read again when needed.")?;
                    return Ok(Some(client));
                }
                Err(PairedHostError::Refused { operation: PairedHostOperation::FrontendInventory,
                    reason: PairedHostRefusal::ConsoleOwnerRequired }) => {
                    say(output, "PairedHost did not accept the saved pairing. It was not deleted or treated as revoked. A new code requires your explicit request.")?;
                }
                Err(error) => return Err(ConnectError::native("check saved PairedHost pairing; the saved credential was retained", error)),
                Ok(FrontendInventoryRead::Rejected(_)) => return Err(ConnectError::InvalidInput(
                    "PairedHost could not provide its current inventory. The saved pairing was retained; no code request was sent.".into())),
            }
        }
        Ok(LoadedDevice::Missing) => say(
            output,
            "No saved pairing was found for this PairedHost origin.",
        )?,
        Ok(LoadedDevice::Expired) => {
            let Some(choice) = device_choice(input, output, interrupted.as_mut(),
                "The saved pairing has expired. Press Enter to choose a new pairing, type 'forget' to forget it locally, or /quit.",
                &["", "forget"]).await? else { return Ok(None); };
            if choice == "forget" {
                forget_selected(store.clone(), &client, interrupted.as_mut(), output).await?;
                return Ok(None);
            }
        }
        Err(DeviceStoreError::InvalidRecord) => {
            let Some(choice) = device_choice(input, output, interrupted.as_mut(),
                "The saved record is invalid. Type 'forget' to remove only this origin's local entry, 'memory' to pair for this process, or /quit.",
                &["forget", "memory"]).await? else { return Ok(None); };
            if choice == "forget" {
                forget_selected(store.clone(), &client, interrupted.as_mut(), output).await?;
                return Ok(None);
            }
            remember = false;
        }
        Err(error) => {
            say(
                output,
                &format!(
                    "Credentials cannot be remembered now: {error}. No plaintext fallback is used."
                ),
            )?;
            let Some(_) = device_choice(
                input,
                output,
                interrupted.as_mut(),
                "Type 'memory' to pair for this process only, or /quit.",
                &["memory"],
            )
            .await?
            else {
                return Ok(None);
            };
            remember = false;
        }
    }
    if remember {
        say(
            output,
            "A confirmed pairing will be saved in this OS user's credential store for this exact PairedHost origin.",
        )?;
    }
    // Start a fresh client: a refused saved cookie cannot participate in the new
    // ceremony. The existing single-reader, explicit request presenter owns it.
    let Some(mut client) =
        pair_paired_host(human_selected_origin, input, output, &mut interrupted).await?
    else {
        return Ok(None);
    };
    if remember {
        let Some(saved) = await_store(
            || paired_host_device::save(store.clone(), &mut client),
            interrupted.as_mut(),
            output,
        )
        .await?
        else {
            return Ok(None);
        };
        match saved {
            Ok(()) => say(
                output,
                "This pairing was remembered in the OS credential store until its original expiry. PairedHost still checks every request.",
            )?,
            Err(error) => {
                say(
                    output,
                    &format!(
                        "Remembering this pairing was not confirmed: {error}. PairedHost's enrollment remains; no save or pairing retry was started."
                    ),
                )?;
                let Some(_) = device_choice(
                    input,
                    output,
                    interrupted.as_mut(),
                    "Type 'continue' to use the confirmed pairing now, or /quit.",
                    &["continue"],
                )
                .await?
                else {
                    return Ok(None);
                };
            }
        }
    }
    Ok(Some(client))
}

fn say(output: &mut impl Write, message: &str) -> Result<(), ConnectError> {
    writeln!(output, "{message}")
        .and_then(|_| output.flush())
        .map_err(|error| ConnectError::native("write PairedHost device custody status", error))
}

async fn forget_selected<F: Future<Output = io::Result<()>>>(
    store: Arc<dyn DeviceStore>,
    client: &PairedHostClient,
    interrupted: Pin<&mut F>,
    output: &mut impl Write,
) -> Result<(), ConnectError> {
    if let Some(result) = await_store(
        || paired_host_device::forget(store, client),
        interrupted,
        output,
    )
    .await?
    {
        result.map_err(|error| {
            ConnectError::native(
                "forget local PairedHost device; host enrollment is unchanged",
                error,
            )
        })?;
        say(
            output,
            "The local device credential is absent. PairedHost's enrollment was not revoked.",
        )?;
    }
    Ok(())
}

async fn device_choice<F: Future<Output = io::Result<()>>>(
    input: &mut TerminalInput,
    output: &mut impl Write,
    mut interrupted: Pin<&mut F>,
    prompt: &str,
    choices: &[&str],
) -> Result<Option<String>, ConnectError> {
    loop {
        say(output, prompt)?;
        let choice = tokio::select! {
            biased;
            signal = &mut interrupted => signal.map(|()| None),
            line = input.recv() => line.transpose(),
        };
        input
            .check_cleanup()
            .map_err(|error| ConnectError::native("restore PairedHost device choice input", error))?;
        let Some(choice) =
            choice.map_err(|error| ConnectError::native("read PairedHost device choice", error))?
        else {
            return Ok(None);
        };
        if choice.trim() == "/quit" {
            return Ok(None);
        }
        if choices.contains(&choice.trim()) {
            return Ok(Some(choice.trim().to_owned()));
        }
    }
}

async fn await_store<T: Send + 'static, F: Future<Output = io::Result<()>>>(
    start: impl FnOnce() -> Result<StoreJob<T>, DeviceStoreError>,
    mut interrupted: Pin<&mut F>,
    output: &mut impl Write,
) -> Result<Option<Result<T, DeviceStoreError>>, ConnectError> {
    if let Some(signal) = interrupted.as_mut().now_or_never() {
        signal.map_err(|error| {
            ConnectError::native("listen for credential-store cancellation", error)
        })?;
        return Ok(None);
    }
    let mut job = match start() {
        Ok(job) => job,
        Err(error) => return Ok(Some(Err(error))),
    };
    tokio::select! {
        biased;
        signal = &mut interrupted => {
            say(output, "Stopping after the credential-store operation finishes. A started save or forget is not rolled back.")?;
            let result = job.join().await;
            if result.is_err() { say(output, "The credential-store result was not confirmed. No retry was started.")?; }
            signal.map_err(|error| ConnectError::native("listen for credential-store cancellation", error))?;
            Ok(None)
        }
        result = job.join() => Ok(Some(result)),
    }
}

/// Pair with one human-selected host before opening the agent session. The
/// caller supplies the same exclusive input owner used for realm/conversation
/// input. Code entry goes straight to the client, never through `recv` or a
/// model/session/tool argument. Cookies remain in this returned client only.
///
/// None means the presenter stopped without yielding a paired client. It does
/// not undo an acknowledged code request. Interruption of an in-flight HTTP
/// operation is an explicit unconfirmed error, not a rollback or retry. A host
/// which drops this entire future after starting HTTP must likewise retain
/// that uncertainty; only the supplied interruption future can print it here.
/// Confirmed host device enrollment is not undone when this client is dropped
/// or a later CLI step fails, even though the local cookie is memory-only.
pub async fn pair_paired_host(
    human_selected_origin: &str,
    input: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<Option<PairedHostClient>, ConnectError> {
    let mut client = PairedHostClient::new(human_selected_origin)
        .map_err(|error| ConnectError::native("select paired host", error))?;
    input
        .require_terminal()
        .map_err(|error| ConnectError::native("check protected code entry", error))?;
    writeln!(
        output,
        "paired host: {}",
        display_text(human_selected_origin)
    )
    .and_then(|_| {
        writeln!(
            output,
            "Type 'request' to ask this host to send its owner a pairing code, 'terminal' to use a code issued on the host terminal, or /quit."
        )
    })
    .and_then(|_| output.flush())
    .map_err(|error| ConnectError::native("write PairedHost pairing choice", error))?;

    tokio::pin!(interrupted);
    let terminal = loop {
        let line = tokio::select! {
            biased;
            signal = &mut interrupted => {
                input.check_cleanup()
                    .map_err(|error| ConnectError::native("restore PairedHost pairing choice input", error))?;
                signal.map_err(|error| ConnectError::native("listen for pairing cancellation", error))?;
                return Ok(None);
            }
            line = input.recv() => line.transpose()
                .map_err(|error| ConnectError::native("read PairedHost pairing choice", error))?,
        };
        let Some(line) = line else { return Ok(None) };
        match line.trim() {
            "/quit" => return Ok(None),
            "request" => break false,
            "terminal" => break true,
            _ => writeln!(
                output,
                "Type 'request' to send a code, 'terminal' to enter a host-terminal code, or /quit."
            )
            .and_then(|_| output.flush())
            .map_err(|error| ConnectError::native("explain PairedHost pairing choice", error))?,
        }
    };

    if terminal {
        say(
            output,
            "Use a code issued on the selected paired host with 'python -m paired_host.console_pairing pair --terminal'. The host must already enable terminal pairing. Connect does not change that setting. Enter the code only in the hidden prompt below.",
        )?;
    } else {
        tokio::select! {
            biased;
            signal = &mut interrupted => return Err(unconfirmed_interruption(signal, "requesting a code")),
            result = client.request_pairing(DEVICE_LABEL) => result
                .map_err(|error| ConnectError::native("request PairedHost pairing", error))?,
        }
        say(
            output,
            "A code was requested from the selected host. Enter it only in the hidden prompt below.",
        )?;
    }

    let code = tokio::select! {
        biased;
        signal = &mut interrupted => {
            signal.map_err(|error| ConnectError::native("listen for pairing cancellation", error))?;
            None
        }
        code = async {
            if terminal {
                input.read_terminal_pairing_code(output).await.map(|code| code.map(EnteredCode::Terminal))
            } else {
                input.read_pairing_code(output).await.map(|code| code.map(EnteredCode::Telegram))
            }
        } => code
            .map_err(|error| ConnectError::native("read protected pairing code", error))?,
    };
    input
        .check_cleanup()
        .map_err(|error| ConnectError::native("restore protected code entry", error))?;
    let Some(code) = code else {
        say(
            output,
            if terminal {
                "Pairing stopped before the terminal code was sent. The host's existing code was not cancelled."
            } else {
                "Pairing stopped. The host's code request was not undone; no automatic retry was started."
            },
        )?;
        return Ok(None);
    };

    tokio::select! {
        biased;
        signal = &mut interrupted => return Err(unconfirmed_interruption(signal, "confirming the code")),
        result = async {
            match code {
                EnteredCode::Telegram(code) => client.confirm_pairing(code).await,
                EnteredCode::Terminal(code) => client.confirm_terminal_pairing(code).await,
            }
        } => result
            .map_err(|error| ConnectError::native("confirm PairedHost pairing", error))?,
    }
    writeln!(output, "Paired for this Connect process. PairedHost keeps the paired device after Connect exits. Frontend activation remains a separate host action.")
        .and_then(|_| output.flush())
        .map_err(|error| ConnectError::native("write PairedHost pairing result after confirmed enrollment", error))?;
    Ok(Some(client))
}

fn unconfirmed_interruption(signal: io::Result<()>, operation: &'static str) -> ConnectError {
    let reason = if signal.is_ok() {
        "Pairing stopped"
    } else {
        "The cancellation listener failed"
    };
    ConnectError::InvalidInput(format!(
        "{reason} while {operation}. The PairedHost outcome is unconfirmed; nothing was rolled back and no automatic retry was started."
    ))
}
