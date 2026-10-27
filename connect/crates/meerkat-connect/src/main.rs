mod main_connector;
use std::{
    future::Future,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    pin::Pin,
};

use clap::{Parser, Subcommand};
use futures_util::FutureExt;
use meerkat_connect::{
    ConnectError, ConnectSession, RealmOptions, SessionOptions, SessionStartup,
    approval::{ApprovalPresenter, LocalApprovals, present_approval},
    discover_realms,
    forge::{ForgePresenter, present_forge_selection},
    host_selection::{HostSelectionPresenter, present_host_selection},
    paired_host_device::OsDeviceStore,
    paired_host_tool::PairedHostFrontend,
    pairing::{pair_paired_host, pair_paired_host_with_store},
    progress::TurnProgress,
    provider_setup::present_provider_setup,
    resources::InstalledResources,
    secret_presenter::SecretEntryPresenter,
    service_preparation::{NativeSetup, ServicePreparationPresenter, present_service_preparation},
    terminal::{
        TerminalInput, committed_text, display_text, failed_turn_allows_new_input, input_lines,
        select_realm,
    },
    terminal_connector::{
        FinalizationNotice, FinalizedAction, FinalizedInstallation, TerminalConnectors,
    },
};
#[cfg(test)]
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{
    AcceptOutcome,
    completion::{CompletionHandle, CompletionObservation, CompletionWaitError},
    input_state::InputTerminalOutcome,
};

#[derive(Parser)]
#[command(
    version,
    about = "Interactive Meerkat Connect setup conversation in a selected realm"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Run the private, host-bound Connect JSONL adapter without a terminal.
    #[arg(long, conflicts_with_all = ["realm", "state_root", "context_root", "user_config_root", "model", "choose_model", "paired_host", "paired_host_memory_only"])]
    stdio: bool,
    /// Select a native realm. Omit to choose a stored realm or name a new one.
    #[arg(long)]
    realm: Option<String>,
    /// Native state root containing realm directories.
    #[arg(long)]
    state_root: Option<PathBuf>,
    #[arg(long)]
    context_root: Option<PathBuf>,
    #[arg(long)]
    user_config_root: Option<PathBuf>,
    /// Override the selected realm's native model choice.
    #[arg(long)]
    model: Option<String>,
    /// Choose an existing native model target and optionally sign in before setup.
    #[arg(long)]
    choose_model: bool,
    /// Review frontend changes or new package proposals for this paired host.
    #[arg(long)]
    paired_host: Option<String>,
    /// Pair for this process without reading or writing the OS credential store.
    #[arg(long, requires = "paired_host")]
    paired_host_memory_only: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect, set or remove credentials for an existing service without setup or a model.
    Credentials {
        #[command(subcommand)]
        action: CredentialAction,
    },
}

#[derive(Subcommand)]
enum CredentialAction {
    /// Show one page of owned service names, declared slots and presence only.
    Status {
        #[arg(long)]
        after: Option<String>,
    },
    /// Review exact declared slots, then enter replacement values privately.
    Set {
        service: String,
        #[arg(long = "slot", required = true)]
        slots: Vec<String>,
    },
    /// Review removal of exact declared slots; configuration and grants stay unchanged.
    Remove {
        service: String,
        #[arg(long = "slot", required = true)]
        slots: Vec<String>,
    },
}

impl Cli {
    fn check_command_options(&self) -> Result<(), ConnectError> {
        if self.command.is_some()
            && (self.stdio
                || self.model.is_some()
                || self.choose_model
                || self.paired_host.is_some()
                || self.paired_host_memory_only)
        {
            return Err(ConnectError::InvalidInput(
                "Credential commands use the selected local realm and cannot select setup, a model or PairedHost.".into(),
            ));
        }
        Ok(())
    }
}

fn prompt(text: &str) -> Result<(), ConnectError> {
    let mut output = io::stdout();
    write!(output, "{text}")
        .and_then(|()| output.flush())
        .map_err(|e| ConnectError::native("write terminal prompt", e))
}

fn interaction_error<T>(
    primary: ConnectError,
    cancellation: Result<T, ConnectError>,
) -> ConnectError {
    match cancellation {
        Ok(_) => primary,
        Err(cleanup) => ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

fn main() -> std::process::ExitCode {
    // Use Core's measured worker and main-future budget, as ordinary rkat does.
    match meerkat_runtime::host_stack::run_host("meerkat-connect", cli_main) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{}", display_text(&error.to_string()));
            std::process::ExitCode::FAILURE
        }
    }
}

async fn cli_main() -> std::process::ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", display_text(&error.to_string()));
            std::process::ExitCode::FAILURE
        }
    }
}

async fn next_line(
    lines: &mut TerminalInput,
    mut interrupt: Pin<&mut impl Future<Output = io::Result<()>>>,
) -> Result<Option<String>, ConnectError> {
    let outcome = tokio::select! {
        biased;
        result = interrupt.as_mut() => result
            .map(|()| None)
            .map_err(|e| ConnectError::native("listen for cancellation", e)),
        result = lines.recv() => result.transpose()
            .map_err(|e| ConnectError::native("read terminal input", e)),
    };
    lines
        .check_cleanup()
        .map_err(|e| ConnectError::native("restore terminal input", e))?;
    outcome
}

async fn run(cli: Cli) -> Result<(), ConnectError> {
    cli.check_command_options()?;
    if cli.stdio {
        return meerkat_connect::stdio::run().await;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(ConnectError::InvalidInput(
            "meerkat-connect requires an interactive terminal. Use the native SDK for programmatic conversations.".into(),
        ));
    }
    // Register once and retain this listener across every terminal handoff,
    // native startup, conversation shutdown and finalizer-entry check.
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal.map_err(|error| ConnectError::native("listen for cancellation", error))?;
        return Ok(());
    }
    let options = RealmOptions {
        context_root: cli.context_root.unwrap_or(
            std::env::current_dir()
                .map_err(|e| ConnectError::native("read invocation directory", e))?,
        ),
        state_root: cli.state_root,
        user_config_root: cli.user_config_root,
    };
    let mut lines = input_lines();
    let storage = if let Some(realm) = cli.realm {
        meerkat_core::runtime_bootstrap::validate_explicit_realm_id(&realm)
            .map_err(|e| ConnectError::native("validate explicit realm", e))?;
        options.resolve(&realm)?
    } else {
        let realms = discover_realms(&options).await?;
        let Some(storage) = select_realm(
            &options,
            &realms,
            &mut lines,
            &mut io::stdout(),
            interrupt.as_mut(),
        )
        .await?
        else {
            return Ok(());
        };
        storage
    };
    println!(
        "Realm: {} ({})",
        display_text(storage.locator.realm.as_str()),
        display_text(&storage.locator.state_root.display().to_string())
    );
    if let Some(Command::Credentials { action }) = cli.command {
        use meerkat_connect::terminal_connector::{
            ServiceCredentialCommand, run_service_credential_command,
        };
        let command = match action {
            CredentialAction::Status { after } => ServiceCredentialCommand::Status { after },
            CredentialAction::Set { service, slots } => {
                ServiceCredentialCommand::Set { service, slots }
            }
            CredentialAction::Remove { service, slots } => {
                ServiceCredentialCommand::Remove { service, slots }
            }
        };
        return run_service_credential_command(
            storage,
            command,
            &mut lines,
            &mut io::stdout(),
            interrupt.as_mut(),
        )
        .await;
    }
    println!(
        "Preparing the supplied setup helper. You will review its access before the setup conversation opens; the agent will then help choose your service or frontend."
    );
    let (prepared, mut round_owner) = InstalledResources::locate()?
        .prepare_with_local_round_owner(&storage, interrupt.as_mut())
        .await?;
    let artifacts = prepared.artifact_directory.clone();
    let native_session = meerkat::Session::new();
    let session_id = native_session.id().clone();
    let (approvals, mut approval_presenter) = LocalApprovals::create_paired_host(&artifacts)?;
    let mut approvals = std::sync::Arc::new(approvals);
    approvals.bind_session(&session_id)?;
    let Some(mut startup) = SessionStartup::prepare_standalone(
        storage.clone(),
        native_session,
        prepared,
        &approvals,
        &mut approval_presenter,
        &mut lines,
        &mut io::stdout(),
        interrupt.as_mut(),
    )
    .await?
    else {
        approvals.retire();
        return Ok(());
    };
    let configured = async {
    let Some(model) = present_provider_setup(
        &mut startup, cli.model, cli.choose_model, &mut lines, &mut io::stdout(),
        interrupt.as_mut(),
    ).await? else { return Ok(None); };
    let mut host_presenter = None;
    let paired_host = if let Some(origin) = cli.paired_host {
        println!(
            "This local realm owns the setup conversation. The selected paired hosts the extension and owns its configuration. For agent frontends, it also owns routing and participants."
        );
        let paired = if cli.paired_host_memory_only {
            println!(
                "Pairing is kept only for this process. The OS credential store will not be read or written. PairedHost enrollment is not revoked when this process ends."
            );
            pair_paired_host(
                &origin,
                &mut lines,
                &mut io::stdout(),
                interrupt.as_mut(),
            )
            .await?
        } else {
            pair_paired_host_with_store(
                &origin,
                std::sync::Arc::new(OsDeviceStore),
                &mut lines,
                &mut io::stdout(),
                interrupt.as_mut(),
            )
            .await?
        };
        let Some(client) = paired else {
            return Ok(None);
        };
        println!(
            "The setup agent can check current extension choices or propose a new package from a pinned source, preserving its selected service and frontend roles. For channel frontends, PairedHost routes each message to its sender's own agent; no destination-agent choice is needed. The terminal asks for approval before submitting the exact proposal for host operator review, creating a secret-entry intent or cancelling one. Secret values are entered only in PairedHost through a private terminal link, with independent browser pairing and host owner confirmation. A new package request queues PairedHost's build and review workflow. Its receipt confirms the request was recorded; installation, activation and readiness require later host evidence."
        );
        PairedHostFrontend::with_research(client)
    } else {
        let (paired_host, presenter) = PairedHostFrontend::deferred_with_research();
        host_presenter = Some(presenter);
        paired_host
    };
    if paired_host.bind_recovery_artifacts(&storage.locator, &artifacts).is_err() {
        println!("PairedHost setup history could not be opened. PairedHost preparation is unavailable in this invocation; other setup remains available.");
    }
    let secret_presenter = Some(paired_host.take_secret_entry_presenter()?);
    let session_options = SessionOptions {
        model,
        ..Default::default()
    };
    let connectors = TerminalConnectors::open(&mut startup).await?;
    println!(
        "The agent can prepare and inspect an exact service package, then request approval for its checked installation. The terminal handles connector sign-in and the verified account access review. After setup closes, native finalization rechecks the approved connector and reports what was saved; login or an approval alone is not service readiness. PairedHost and Forge keep their existing protected setup presenters. Never enter credential values into the conversation."
    );
    let (forge, forge_presenter) = main_connector::standalone_forge(None)?;
    let setup = NativeSetup::with_forge(session_id.clone(), storage.locator.clone(), paired_host.clone(),
        &connectors, approvals.clone(), (forge.clone(), forge_presenter))?;
    Ok::<_, ConnectError>(Some((session_options, setup, connectors, host_presenter, secret_presenter, paired_host, forge)))
    }.await;
    let (
        session_options,
        mut setup,
        mut connectors,
        mut host_presenter,
        mut secret_presenter,
        mut paired_host,
        mut forge_frontend,
    ) = match configured {
        Ok(Some(value)) => value,
        other => {
            approvals.retire();
            let cleanup = startup.abort_standalone(&mut io::stdout()).await;
            return match (other, cleanup) {
                (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                }),
                (Err(error), _) | (_, Err(error)) => Err(error),
                _ => Ok(()),
            };
        }
    };
    let continuation = startup.standalone_continuation()?;
    let model_selection = session_options.model.clone();
    let opened = Box::pin(startup.open_native_setup(
        session_options,
        setup.clone(),
        &approvals,
        &mut approval_presenter,
        &mut lines,
        &mut io::stdout(),
        interrupt.as_mut(),
    ))
    .await;
    let mut session = match opened {
        Ok(Some(session)) => session,
        other => {
            approvals.retire();
            let cleanup = connectors.retire();
            return match (other, cleanup) {
                (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                }),
                (Err(error), _) | (_, Err(error)) => Err(error),
                _ => Ok(()),
            };
        }
    };
    let mut finalizations: Vec<std::sync::Arc<FinalizedInstallation>> = Vec::new();
    let mut completed_actions: Vec<FinalizedAction> = Vec::new();
    let mut notice = None;
    // Every exit after finalization passes through the retained typed outcomes,
    // including a stop during a later round's startup or native close.
    let result = async {
    loop {
    let composition = setup.take_composition().and_then(|(lazy, presenter)| {
        let forge = lazy.take_forge_presenter()?;
        Ok((lazy, presenter, forge))
    });
    let (lazy_setup, presenter, forge) = match composition {
        Ok(value) => value,
        Err(primary) => {
            approvals.retire();
            let retired = connectors.retire();
            let closed = session.close().await;
            return Err(interaction_error(interaction_error(primary, retired), closed));
        }
    };
    if lazy_setup.set_forge_artifacts(&artifacts).is_err() {
        println!("Forge recovery artifacts could not be verified. Forge setup is unavailable in this invocation; other setup remains available.");
    }
    let mut forge_presenter = Some(forge);
    let mut service_presenter = Some(presenter);
    if completed_actions.is_empty() {
        println!("Describe the service or frontend you want. Enter /quit to end. Never enter passwords or tokens here.");
        println!("Setup session: {}", session.id());
    }
    // Keep the startup listener through shutdown too. A signal between native
    // startup and conversation entry must not require a second signal.
    let mut close_pending = false;
    // Keep exact unconfirmed native results/errors with the same live owner.
    // They are not cleanup receipts or authority to discard the reviewed work.
    let mut close_observations = Vec::new();
    let mut retry_close = false;
    let (result, cleanup) = loop {
        let result = if retry_close {
            retry_close = false;
            Ok(ConversationExit::CloseRequested)
        } else { converse(
            &session, &mut lines, &approvals, &mut approval_presenter,
            &mut secret_presenter, &mut host_presenter, &mut forge_presenter,
            &mut service_presenter, &mut connectors, &storage,
            interrupt.as_mut(), notice.take(), close_pending,
        ).await };
        if !matches!(result, Ok(ConversationExit::CloseRequested)) {
            // Ordinary explicit exit, cancellation or fatal interaction cleanup
            // keeps its existing contract. It never finalizes a retained review.
            if close_pending {
                let reason = if result.is_err() { "Setup cannot continue after an interaction failure." }
                    else { "Setup was stopped." };
                let _ = writeln!(io::stdout(), "{reason} The retained review will not be finalized. This shutdown may cancel accepted work. Existing installation records are not undone; a later setup needs a fresh access review.");
            }
            approvals.retire();
            let connector_cleanup = connectors.retire();
            break (result.map(|_| None), combine_cleanup(connector_cleanup, session.close().await));
        }
        // This is entered only after the actual reviewed turn settled, or an
        // explicit retry of that retained close. Do not race this owned close
        // with Ctrl-C and discard its outcome.
        match session.try_close_for_finalization().await {
            meerkat_connect::session::FinalizationClose::Closed(mut closed) => {
                approvals.retire();
                let connector_cleanup = connectors.retire();
                let mut failed_cleanups = Vec::new();
                let host_cleanup = loop {
                    match closed.finish_host_cleanup().await {
                        Ok(()) => break Ok(()),
                        Err((retained, error)) => {
                            closed = retained;
                            failed_cleanups.push(error);
                            let _ = writeln!(io::stdout(), "Native setup has closed, but host cleanup failed. The review and cleanup owner are retained. /retry-close retries host cleanup only; /quit stops without finalizing and reports the cleanup failure.");
                            let action = explicit_close_retry(&mut lines, interrupt.as_mut()).await;
                            if matches!(action, Ok(true)) { continue; }
                            // The native registration is already consumed.
                            // Never use ConnectSession::close on this owner.
                            let cleanup = failed_cleanups.into_iter().fold(Ok(()), |prior, error| combine_cleanup(prior, Err(error)));
                            break match action {
                                Ok(_) => cleanup,
                                Err(primary) => Err(interaction_error(primary, cleanup)),
                            };
                        }
                    }
                };
                let cleanup = combine_cleanup(connector_cleanup, host_cleanup);
                let reviewed = if cleanup.is_ok() { connectors.take_reviewed_action() } else { None };
                break (Ok(reviewed), cleanup);
            }
            meerkat_connect::session::FinalizationClose::Busy(retained) => {
                session = retained;
                close_pending = true;
                let _ = writeln!(io::stdout(), "Setup still has accepted work. The access review is retained. Continue the conversation, or enter /retry-close after that work finishes. Nothing was finalized.");
            }
            meerkat_connect::session::FinalizationClose::Unconfirmed { session: retained, observation } => {
                session = retained;
                close_pending = true;
                let message = match &observation {
                    Ok(meerkat_runtime::RuntimeSessionQuiescentClose::AlreadyClosing) => "Another native close is in progress; this call did not join it.",
                    Ok(meerkat_runtime::RuntimeSessionQuiescentClose::NotCurrent) => "The native registration is no longer current; cleanup was not confirmed.",
                    Err(_) => "The native close returned an error; cleanup was not confirmed.",
                    _ => "Native close did not return its expected retained-owner observation.",
                };
                close_observations.push(observation);
                let _ = writeln!(io::stdout(), "{message} The review and owner are retained. Enter /retry-close to ask native custody again, or /quit to stop without finalizing.");
                // Do not submit a fabricated turn or treat AlreadyClosing as a
                // join. An unconfirmed registration requires explicit action.
                let action = explicit_close_retry(&mut lines, interrupt.as_mut()).await;
                if matches!(action, Ok(true)) {
                    retry_close = true;
                } else {
                    // Explicit quit, EOF or the retained interrupt ends this
                    // invocation without converting the observation to Closed.
                    approvals.retire();
                    let connector_cleanup = connectors.retire();
                    let cleanup = combine_cleanup(connector_cleanup, session.close().await);
                    return match action {
                        Ok(_) => cleanup,
                        Err(primary) => Err(interaction_error(primary, cleanup)),
                    };
                }
                // Retry the retained close directly, without a new agent input.
                continue;
            }
        }
    };
    // Only Closed plus successful host cleanup can lead to finalization.
    // Exit or fatal interaction failure uses its separate shutdown behavior.
    drop(connectors);
    let result = if approvals.has_uncertain_dispatch() {
        let uncertain = ConnectError::InvalidInput(
            "A consented dispatch has an uncertain outcome. Its approval is spent; no automatic retry or restoration is available.".into(),
        );
        Err(match result {
            Err(primary) => interaction_error(primary, Err::<(), _>(uncertain)),
            _ => uncertain,
        })
    } else {
        result
    };
    // Drop the setup dispatchers and approval owner too: their references must
    // not overlap a fresh native administrative opener after session close.
    drop(setup);
    drop(lazy_setup);
    drop(service_presenter);
    drop(forge_presenter);
    drop(secret_presenter.take());
    drop(host_presenter.take());
    drop(approval_presenter);
    drop(approvals);
    let reviewed = match (result, cleanup) {
        (Err(primary), Err(cleanup)) => return Err(ConnectError::Cleanup {
            primary:Box::new(primary), cleanup:Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => return Err(error),
        (Ok(Some(reviewed)), Ok(())) => reviewed,
        (Ok(None), Ok(())) => return Ok(()),
    };
    let Some(mut finalized) = reviewed.finalize_outcome_unless_cancelled(interrupt.as_mut()).await? else {
        prompt("Connect stopped before native finalization. No finalization was started.\n")?;
        return Ok(());
    };
    // Hold the actual typed outcome before any artifact or terminal operation
    // can fail. This is evidence, never replacement native current state.
    let next_notice = finalized.take_continuation_notice().ok_or_else(|| ConnectError::InvalidInput(
        "The finalizer has no unused continuation occurrence. No new input was created.".into()))?;
    if let Some(installation) = finalized.installation() { finalizations.push(installation); }
    completed_actions.push(finalized);
    let finalized = completed_actions.last().expect("the settled action was just retained");
    let retained = finalized.retain(&artifacts, completed_actions.len());
    let rendered = finalized.display_for_continuation(&mut io::stdout());
    match (retained, rendered) {
        (Err(primary), Err(cleanup)) => return Err(ConnectError::Cleanup { primary:Box::new(primary), cleanup:Box::new(cleanup) }),
        (Err(error), _) | (_, Err(error)) => return Err(error),
        _ => {},
    }
    if finalized.stopped_during_finalization() { return Ok(()); }
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal.map_err(|error| ConnectError::native("Listen after native finalization", error))?;
        return Ok(());
    }
    // The old session, dispatchers, presenters and auth facade have all been
    // settled and dropped. Move only the original paired connection, never
    // its retired selection, consent store, research capture or runtime.
    let (fresh_forge, fresh_forge_presenter) = main_connector::standalone_forge(Some(forge_frontend))?;
    forge_frontend = fresh_forge;
    let (fresh_paired_host, fresh_host_presenter) = paired_host.continue_after_close()?;
    paired_host = fresh_paired_host;
    host_presenter = fresh_host_presenter;
    if paired_host.bind_recovery_artifacts(&storage.locator, &artifacts).is_err() {
        println!("PairedHost setup history could not be opened. PairedHost preparation is unavailable in this invocation; other setup remains available.");
    }
    secret_presenter = Some(paired_host.take_secret_entry_presenter()?);
    let mut resumed = continuation.reopen().await?;
    let (fresh_approvals, fresh_presenter) = LocalApprovals::create_paired_host(&artifacts)?;
    approvals = std::sync::Arc::new(fresh_approvals);
    approvals.bind_session(&session_id)?;
    approval_presenter = fresh_presenter;
    connectors = TerminalConnectors::open_with_finalizations(resumed.startup(), finalizations.clone().into()).await?;
    setup = NativeSetup::with_forge(session_id.clone(), storage.locator.clone(), paired_host.clone(),
        &connectors, approvals.clone(), (forge_frontend.clone(), fresh_forge_presenter))?;
    let opened = async {
        // This move-only owner came from the one original preparation. It
        // authorizes one fresh helper while native eagerly opens that exact
        // descriptor. Ordinary relaunch without this live handoff stays read-only.
        let guard = round_owner.begin(interrupt.as_mut()).await?;
        let opened = resumed.open_native_setup(SessionOptions { model:model_selection.clone(), ..Default::default() },
            setup.clone(), &mut io::stdout(), interrupt.as_mut()).await;
        match opened {
            Ok(Some(session)) => match guard.settle().await {
                Ok(()) => Ok(Some(session)),
                Err(primary) => {
                    // An actor may exist even if its helper handoff did not
                    // confirm. Close that exact new owner before reporting it.
                    Err(interaction_error(primary, session.close().await))
                }
            },
            Ok(None) => guard.cancel().await.map(|()| None),
            Err(primary) => Err(interaction_error(primary, guard.cancel().await)),
        }
    }.await;
    session = match opened {
        Ok(Some(session)) => session,
        other => {
            approvals.retire();
            let cleanup = connectors.retire();
            return match (other, cleanup) {
                (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup { primary:Box::new(primary), cleanup:Box::new(cleanup) }),
                (Err(error), _) | (_, Err(error)) => Err(error),
                _ => Ok(()),
            };
        }
    };
    notice = Some(next_notice);
    }
    }.await;
    FinalizedAction::finish_invocation(&completed_actions, result)
}

/// An unconfirmed close retains its native owner while awaiting an explicit
/// operator action. A broken input stream cannot authorize automatic teardown.
async fn explicit_close_retry(
    lines: &mut TerminalInput,
    mut interrupt: Pin<&mut impl Future<Output = io::Result<()>>>,
) -> Result<bool, ConnectError> {
    loop {
        let line = tokio::select! { biased;
            signal = interrupt.as_mut() => {
                let signal = signal.map_err(|error| ConnectError::native("Listen while native close is unconfirmed", error));
                let cleanup = lines.check_cleanup().map_err(|error| ConnectError::native("Restore retained terminal input", error));
                return combine_cleanup(signal, cleanup).map(|()| false);
            },
            line = lines.recv() => line.transpose().map_err(|error| ConnectError::native("Read retained terminal input", error)),
        };
        let line = match lines.check_cleanup() {
            Ok(()) => line,
            Err(error) => {
                let cleanup = ConnectError::native("Restore retained terminal input", error);
                Err(match line {
                    Err(primary) => interaction_error(primary, Err::<(), _>(cleanup)),
                    _ => cleanup,
                })
            }
        };
        match line {
            Ok(Some(line)) if line == "/retry-close" => return Ok(true),
            Ok(None) => return Ok(false),
            Ok(Some(line)) if line == "/quit" => return Ok(false),
            Ok(Some(_)) => {
                let _ = writeln!(
                    io::stdout(),
                    "Enter /retry-close or /quit. No service finalization has started."
                );
            }
            Err(primary) => {
                let _ = writeln!(
                    io::stderr(),
                    "Input is unavailable. Ctrl-C stops setup without finalizing."
                );
                // This branch came from input, so the retained interrupt has
                // not been consumed. Join it once before releasing the owner.
                let signal = interrupt.as_mut().await.map_err(|error| {
                    ConnectError::native("Listen while terminal input is unavailable", error)
                });
                return Err(interaction_error(primary, signal));
            }
        }
    }
}

fn combine_cleanup(
    first: Result<(), ConnectError>,
    second: Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    match (first, second) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

/// Call only after the existing setup owners have joined. `interrupt` is the
/// same listener retained by the conversation, so shutdown cannot lose a signal.
/// Once the finalizer is entered, retain it through its actual settlement.
#[cfg(test)]
async fn finalize_unless_cancelled<I, F, R>(
    mut interrupt: Pin<&mut I>,
    finalize: F,
) -> Result<bool, ConnectError>
where
    I: Future<Output = io::Result<()>>,
    F: FnOnce() -> R,
    R: Future<Output = Result<(), ConnectError>>,
{
    Ok(
        enter_finalization_unless_cancelled(interrupt.as_mut(), finalize)
            .await?
            .transpose()?
            .is_some(),
    )
}

#[cfg(test)]
async fn enter_finalization_unless_cancelled<I, F, R, T>(
    mut interrupt: Pin<&mut I>,
    finalize: F,
) -> Result<Option<T>, ConnectError>
where
    I: Future<Output = io::Result<()>>,
    F: FnOnce() -> R,
    R: Future<Output = T>,
{
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal
            .map_err(|error| ConnectError::native("listen for finalization cancellation", error))?;
        return Ok(None);
    }
    // Retain this exact entered future through settlement. Do not race a
    // cancellation against administration and lose a potentially partial result.
    Ok(Some(finalize().await))
}

/// The CLI retains the exact native input until admission and completion settle.
#[derive(Debug)]
enum TurnAdmission {
    Running {
        admission: AcceptOutcome,
        completion: CompletionHandle,
    },
    Interrupted(Result<CompletionObservation, CompletionWaitError>),
    NotSubmitted,
}

fn announce_cancellation<W: Write>(
    signal: io::Result<()>,
    approvals: &LocalApprovals,
    output: &mut W,
) -> Result<(), ConnectError> {
    approvals.retire();
    let printed = writeln!(
        output,
        "\nCancellation requested. Waiting for native setup work to settle."
    )
    .and_then(|()| output.flush())
    .map_err(|error| ConnectError::native("write cancellation progress", error));
    let signal = signal.map_err(|error| ConnectError::native("listen for cancellation", error));
    match (signal, printed) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

async fn admit_turn<S, C, W>(
    session: &ConnectSession,
    submit: S,
    mut interrupt: Pin<&mut C>,
    approvals: &LocalApprovals,
    output: &mut W,
) -> Result<TurnAdmission, ConnectError>
where
    S: Future<Output = Result<(AcceptOutcome, CompletionHandle), ConnectError>>,
    C: Future<Output = io::Result<()>>,
    W: Write,
{
    // Register before native admission. A cancellation already observed here
    // must not start a new input merely to obtain something to cancel.
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        approvals.retire();
        signal.map_err(|error| ConnectError::native("listen for cancellation", error))?;
        return Ok(TurnAdmission::NotSubmitted);
    }
    tokio::pin!(submit);
    // After its first poll, admission may have durable effects even before the
    // receipt reaches this caller. Keep and join this exact future on every path.
    let (submitted, interrupted) = if let Some(result) = submit.as_mut().now_or_never() {
        (result, None)
    } else {
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => {
                let report = announce_cancellation(signal, approvals, output);
                (submit.await, Some(report))
            }
            result = submit.as_mut() => (result, None),
        }
    };
    let interrupted = match interrupted {
        Some(report) => Some(report),
        None => interrupt
            .as_mut()
            .now_or_never()
            .map(|signal| announce_cancellation(signal, approvals, output)),
    };
    let (admission, completion) = match submitted {
        Ok(receipt) => receipt,
        Err(primary) => {
            return Err(match interrupted {
                Some(Err(cleanup)) => ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                },
                _ => primary,
            });
        }
    };
    if let Some(report) = interrupted {
        let cancelled = session.cancel(&admission).await;
        let completed = completion.try_wait_with_terminal_outcome().await;
        if let Err(primary) = report {
            return Err(interaction_error(primary, cancelled));
        }
        cancelled?;
        return Ok(TurnAdmission::Interrupted(completed));
    }
    Ok(TurnAdmission::Running {
        admission,
        completion,
    })
}

/// A local conversation boundary, never a native close or finalization receipt.
enum ConversationExit {
    CloseRequested,
    Stopped,
}

async fn converse(
    session: &ConnectSession,
    lines: &mut TerminalInput,
    approvals: &LocalApprovals,
    approval_presenter: &mut ApprovalPresenter,
    secret_presenter: &mut Option<SecretEntryPresenter>,
    host_presenter: &mut Option<HostSelectionPresenter>,
    forge_presenter: &mut Option<ForgePresenter>,
    service_presenter: &mut Option<ServicePreparationPresenter>,
    connectors: &mut TerminalConnectors,
    storage: &meerkat_core::ResolvedStorage,
    mut interrupt: Pin<&mut (impl Future<Output = io::Result<()>> + Send)>,
    mut first_notice: Option<FinalizationNotice>,
    close_pending: bool,
) -> Result<ConversationExit, ConnectError> {
    // Register before waiting for terminal input, and retain this listener when
    // handing a reviewed connector back for shutdown and finalization.
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        approvals.retire();
        signal.map_err(|error| ConnectError::native("listen for cancellation", error))?;
        return Ok(ConversationExit::Stopped);
    }
    loop {
        let notice = first_notice.take();
        let message = if notice.is_some() {
            None
        } else {
            prompt("You: ")?;
            let Some(message) = next_line(lines, interrupt.as_mut()).await? else {
                return Ok(ConversationExit::Stopped);
            };
            if message == "/quit" {
                return Ok(ConversationExit::Stopped);
            }
            if close_pending && message == "/retry-close" {
                return Ok(ConversationExit::CloseRequested);
            }
            if message.trim().is_empty() {
                continue;
            }
            Some(message)
        };
        // Observe the retained interrupt before even the event subscription.
        if let Some(signal) = interrupt.as_mut().now_or_never() {
            approvals.retire();
            signal.map_err(|error| ConnectError::native("listen for cancellation", error))?;
            return Ok(ConversationExit::Stopped);
        }
        // Subscribe before admission so early native assistant deltas are not
        // lost. This CLI owns one input at a time in this dedicated session.
        let events = session.events();
        tokio::pin!(events);
        let events = tokio::select! {
            biased;
            signal = interrupt.as_mut() => {
                let report = announce_cancellation(signal, approvals, &mut io::stdout());
                let subscribed = events.await;
                if let Err(primary) = subscribed {
                    return Err(match report {
                        Err(cleanup) => ConnectError::Cleanup {
                            primary: Box::new(primary),
                            cleanup: Box::new(cleanup),
                        },
                        Ok(()) => primary,
                    });
                }
                report?;
                return Ok(ConversationExit::Stopped);
            }
            result = events.as_mut() => result?,
        };
        let mut progress = TurnProgress::new(session.id().clone(), events);
        progress
            .begin(&mut io::stdout())
            .map_err(|e| ConnectError::native("write setup progress", e))?;
        let mut exit_requested = false;
        let mut output = io::stdout();
        let outcome = match admit_turn(
            session,
            async {
                match (notice, message) {
                    (Some(notice), None) => session.submit_finalization(notice).await,
                    (None, Some(message)) => session.submit(message).await,
                    _ => Err(ConnectError::InvalidInput(
                        "Setup input ownership is inconsistent".into(),
                    )),
                }
            },
            interrupt.as_mut(),
            approvals,
            &mut output,
        )
        .await?
        {
            TurnAdmission::NotSubmitted => return Ok(ConversationExit::Stopped),
            TurnAdmission::Interrupted(outcome) => {
                exit_requested = true;
                outcome
            }
            TurnAdmission::Running {
                admission,
                completion,
            } => {
                let wait = completion.try_wait_with_terminal_outcome();
                tokio::pin!(wait);
                loop {
                    tokio::select! {
                        biased;
                        signal = interrupt.as_mut() => {
                            exit_requested = true;
                            let report = announce_cancellation(signal, approvals, &mut output);
                            let cancelled = session.cancel(&admission).await;
                            let completed = wait.await;
                            if let Err(primary) = report {
                                return Err(interaction_error(primary, cancelled));
                            }
                            cancelled?;
                            break completed;
                        }
                        result = &mut wait => break result,
                        selection = async {
                            match host_presenter.as_mut() {
                                Some(presenter) => presenter.next().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            // This branch exclusively owns terminal input. Native tool
                            // expiry closes its handoff and interrupts pairing too.
                            let presented = match selection {
                                Some(selection) => match progress.pause_for_prompt(&mut output) {
                                    Ok(()) => present_host_selection(selection, lines, &mut output, interrupt.as_mut()).await,
                                    Err(error) => Err(ConnectError::native("pause setup progress", error)),
                                },
                                None => Err(ConnectError::InvalidInput("The protected host presenter has ended".into())),
                            };
                            if !matches!(presented, Ok(true)) {
                                approvals.retire();
                                let cancelled = session.cancel(&admission).await;
                                let _ = wait.await;
                                return match presented {
                                    Err(primary) => Err(interaction_error(primary, cancelled)),
                                    Ok(_) => cancelled.map(|_| ConversationExit::Stopped),
                                };
                            }
                        }
                        selection = async {
                            match forge_presenter.as_mut() {
                                Some(presenter) => presenter.next().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            let presented = match selection {
                                Some(selection) => match progress.pause_for_prompt(&mut output) {
                                    Ok(()) => present_forge_selection(selection, lines, &mut output, interrupt.as_mut()).await,
                                    Err(error) => Err(ConnectError::native("pause setup progress", error)),
                                },
                                None => Err(ConnectError::InvalidInput("The protected Forge presenter has ended".into())),
                            };
                            if !main_connector::continue_after_presentation(
                                presented, session, &admission, approvals, wait.as_mut(),
                            ).await? { return Ok(ConversationExit::Stopped); }
                        }
                        request = async {
                            match service_presenter.as_mut() {
                                Some(presenter) => presenter.next().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            // Keep preparation outside the disposable native tool future.
                            // Its closure/Ctrl-C cancellation is driven to an awaited
                            // child exit by the existing installed-resource owner.
                            let presented = match request {
                                Some(request) => match progress.pause_for_prompt(&mut output) {
                                    Ok(()) => present_service_preparation(request, storage, &mut output, interrupt.as_mut()).await,
                                    Err(error) => Err(ConnectError::native("pause setup progress", error)),
                                },
                                None => Err(ConnectError::InvalidInput("The service preparation owner has ended".into())),
                            };
                            if !matches!(presented, Ok(true)) {
                                approvals.retire();
                                let cancelled = session.cancel(&admission).await;
                                let _ = wait.await;
                                return match presented {
                                    Err(primary) => Err(interaction_error(primary, cancelled)),
                                    Ok(_) => cancelled.map(|_| ConversationExit::Stopped),
                                };
                            }
                        }
                        request = connectors.next() => {
                            let presented = match request {
                                Some(request) => match progress.pause_for_prompt(&mut output) {
                                    Ok(()) => connectors.present(request, approvals, approval_presenter,
                                        lines, &mut output, interrupt.as_mut()).await,
                                    Err(error) => Err(ConnectError::native("pause setup progress", error)),
                                },
                                None => Err(ConnectError::InvalidInput("The protected connector presenter has ended".into())),
                            };
                            if !main_connector::continue_after_presentation(
                                presented, session, &admission, approvals, wait.as_mut(),
                            ).await? {
                                return Ok(ConversationExit::Stopped);
                            }
                        }
                        approval = approval_presenter.next() => {
                            // No event polling or background printing while this owns
                            // the terminal. It uses the same exclusive input reader.
                            let presented = match approval {
                                Some(approval) => match progress.pause_for_prompt(&mut output) {
                                    Ok(()) => present_approval(approval, lines, &mut output, interrupt.as_mut()).await,
                                    Err(error) => Err(ConnectError::native("pause setup progress", error)),
                                },
                                None => Err(ConnectError::InvalidInput("The local approval presenter has ended".into())),
                            };
                            if !matches!(presented, Ok(true)) {
                                exit_requested = true;
                                approvals.retire();
                                let cancelled = session.cancel(&admission).await;
                                let completed = wait.await;
                                if let Err(primary) = presented {
                                    return Err(interaction_error(primary, cancelled));
                                }
                                cancelled?;
                                break completed;
                            }
                        }
                        rendered = progress.next(&mut output) => {
                            if let Err(error) = rendered {
                                approvals.retire();
                                let cancelled = session.cancel(&admission).await;
                                let _ = wait.await;
                                return Err(interaction_error(ConnectError::native("write setup progress", error), cancelled));
                            }
                        }
                    }
                }
            }
        };
        // A completed input may win just before a recorded interrupt is
        // observed. Preserve its real outcome while retaining the user's exit.
        if !exit_requested && let Some(signal) = interrupt.as_mut().now_or_never() {
            exit_requested = true;
            approvals.retire();
            signal.map_err(|error| ConnectError::native("listen for cancellation", error))?;
        }
        // Render private links only after native completion is joined. A
        // terminal failure cannot interrupt a successful effect's settlement.
        // Drain before the completed answer, including cancellation/quit races.
        progress
            .pause_for_prompt(&mut output)
            .map_err(|e| ConnectError::native("finish setup progress line", e))?;
        if let Some(presenter) = secret_presenter.as_mut() {
            presenter.present_pending(&mut output)?;
        }
        if service_presenter
            .as_ref()
            .is_some_and(ServicePreparationPresenter::requires_shutdown)
        {
            approvals.retire();
            return Err(ConnectError::InvalidInput(
                "Service preparation ended without a confirmed handoff. This setup invocation will close without retrying; retained files are not claimed rolled back.".into(),
            ));
        }
        let observation = outcome.map_err(|_| {
            ConnectError::InvalidInput(
                "The native completion could not be confirmed. No automatic retry was started."
                    .into(),
            )
        })?;
        let (outcome, input_terminal) = observation.into_parts();
        let status_readback = approvals.permits_uncertain_status_readback();
        if approvals.has_uncertain_dispatch() && !status_readback {
            return Err(ConnectError::InvalidInput(
                "A consented dispatch did not settle with a known outcome. Stop and inspect the retained native decision; do not replay it.".into(),
            ));
        }
        // Completion can win the race with cancellation. The user's exit
        // still ends this conversation; the retired gate cannot be reopened.
        if exit_requested {
            return Ok(ConversationExit::Stopped);
        }
        let text = match committed_text(&outcome) {
            Ok(text) => Some(text),
            Err(message)
                if failed_turn_allows_new_input(
                    &outcome,
                    input_terminal.as_ref(),
                    exit_requested,
                ) =>
            {
                // Report the native failure as a failure. Its committed input
                // witness permits offering a new request, not resuming it or
                // deciding that the runtime will accept the next input.
                writeln!(output, "{message}")
                    .map_err(|e| ConnectError::native("write failed setup turn", e))?;
                None
            }
            Err(message) => return Err(ConnectError::InvalidInput(message.into())),
        };
        if status_readback {
            writeln!(output, "The effect response is uncertain. Only status checks for that exact request remain available; it will not be sent again.")
                .map_err(|e| ConnectError::native("write submission uncertainty", e))?;
        }
        if let Some(text) = text {
            progress
                .finish_committed(text, &mut output)
                .map_err(|e| ConnectError::native("write completed setup answer", e))?;
        }
        if connectors.has_reviewed() {
            if !matches!(
                input_terminal,
                Some(InputTerminalOutcome::Consumed | InputTerminalOutcome::Abandoned { .. })
            ) {
                return Err(ConnectError::InvalidInput("The reviewed installation has no committed terminal input witness. Native finalization was not started.".into()));
            }
            // The exact native turn has settled. Main closes setup before
            // consuming this historical installation/access decision handoff.
            return Ok(ConversationExit::CloseRequested);
        }
    }
}

#[cfg(test)]
mod main_admission_tests;

#[cfg(test)]
mod input_handoff_tests {
    use super::*;
    use std::{
        os::unix::net::UnixStream,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn registered_interrupt_survives_realm_to_conversation_handoff() {
        let root = tempfile::tempdir().unwrap();
        let options = RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("config")),
        };
        let (socket, mut writer) = UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        let (cancel, cancelled) = oneshot::channel();
        let completed = Arc::new(AtomicUsize::new(0));
        let observed = completed.clone();
        let interrupt = async move {
            cancelled.await.unwrap();
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(interrupt.as_mut().now_or_never().is_none());
        writer.write_all(b"chosen-realm\n").unwrap();
        let selected = select_realm(
            &options,
            &[],
            &mut input,
            &mut Vec::new(),
            interrupt.as_mut(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(selected.locator.realm.as_str(), "chosen-realm");
        assert_eq!(completed.load(Ordering::SeqCst), 0);
        // The signal arrives after the preceding stage returned. Ready input
        // must not win, and no newly registered signal listener is involved.
        cancel.send(()).unwrap();
        writer.write_all(b"must remain unread\n").unwrap();
        assert!(
            next_line(&mut input, interrupt.as_mut())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert_eq!(input.recv().await.unwrap().unwrap(), "must remain unread");
        input.check_cleanup().unwrap();
        // An async future panics if polled after completion. next_line returned
        // without a second poll, and the consumed listener is now dropped.
    }

    #[tokio::test]
    async fn registered_interrupt_cancels_a_pending_read_without_consuming_later_input() {
        let (socket, mut writer) = UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        let (cancel, cancelled) = oneshot::channel();
        let interrupt = async move {
            cancelled.await.unwrap();
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(interrupt.as_mut().now_or_never().is_none());
        {
            let reading = next_line(&mut input, interrupt.as_mut());
            tokio::pin!(reading);
            assert!(reading.as_mut().now_or_never().is_none());
            cancel.send(()).unwrap();
            assert!(reading.await.unwrap().is_none());
        }
        input.check_cleanup().unwrap();
        writer.write_all(b"later input\n").unwrap();
        assert_eq!(input.recv().await.unwrap().unwrap(), "later input");
    }

    #[tokio::test]
    async fn ordinary_input_and_eof_leave_the_same_interrupt_pending() {
        let (socket, mut writer) = UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        let (_cancel, cancelled) = oneshot::channel::<()>();
        let interrupt = async move {
            cancelled.await.unwrap();
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(interrupt.as_mut().now_or_never().is_none());
        writer.write_all(b"ordinary message\n").unwrap();
        assert_eq!(
            next_line(&mut input, interrupt.as_mut())
                .await
                .unwrap()
                .as_deref(),
            Some("ordinary message"),
        );
        drop(writer);
        assert!(
            next_line(&mut input, interrupt.as_mut())
                .await
                .unwrap()
                .is_none()
        );
        assert!(interrupt.as_mut().now_or_never().is_none());
        input.check_cleanup().unwrap();
    }

    #[tokio::test]
    async fn failed_interrupt_wins_over_ready_input() {
        let (socket, mut writer) = UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        writer.write_all(b"must remain unread\n").unwrap();
        let interrupt = async { Err(io::Error::other("signal listener unavailable")) };
        tokio::pin!(interrupt);
        assert!(next_line(&mut input, interrupt.as_mut()).await.is_err());
        assert_eq!(input.recv().await.unwrap().unwrap(), "must remain unread");
        input.check_cleanup().unwrap();
    }

    #[tokio::test]
    async fn retained_close_accepts_only_explicit_retry_or_exit_without_new_input() {
        for (text, retry) in [
            ("ordinary text\n/retry-close\n", true),
            ("/quit\n", false),
            ("", false),
        ] {
            let (socket, mut writer) = UnixStream::pair().unwrap();
            let mut input = TerminalInput::from_owned_fd(socket.into());
            writer.write_all(text.as_bytes()).unwrap();
            drop(writer);
            let interrupt = std::future::pending::<io::Result<()>>();
            tokio::pin!(interrupt);
            let observed = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                explicit_close_retry(&mut input, interrupt.as_mut()),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(observed, retry);
            assert!(interrupt.as_mut().now_or_never().is_none());
            input.check_cleanup().unwrap();
        }
    }

    #[tokio::test]
    async fn retained_close_joins_its_one_interrupt_before_releasing_failed_input() {
        let (socket, mut writer) = UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        // Invalid UTF-8 is a real failure of the existing ordinary input owner.
        writer.write_all(&[0xff, b'\n']).unwrap();
        let (cancel, cancelled) = oneshot::channel();
        let completed = Arc::new(AtomicUsize::new(0));
        let observed = completed.clone();
        let interrupt = async move {
            cancelled.await.unwrap();
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        tokio::pin!(interrupt);
        let reading = explicit_close_retry(&mut input, interrupt.as_mut());
        tokio::pin!(reading);
        // Consume the exact failed line through the real reader. Even after
        // that failure, only the explicit retained interrupt can release it.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), reading.as_mut())
                .await
                .is_err()
        );
        assert_eq!(completed.load(Ordering::SeqCst), 0);
        cancel.send(()).unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), reading)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert!(matches!(
            error,
            ConnectError::Native {
                operation: "Read retained terminal input",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn retained_close_interrupt_wins_over_retry_without_repoll_or_read() {
        let (socket, mut writer) = UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        writer.write_all(b"/retry-close\n").unwrap();
        let completed = Arc::new(AtomicUsize::new(0));
        let observed = completed.clone();
        let interrupt = async move {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(
            !explicit_close_retry(&mut input, interrupt.as_mut())
                .await
                .unwrap()
        );
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert_eq!(input.recv().await.unwrap().unwrap(), "/retry-close");
        input.check_cleanup().unwrap();
    }
}

#[cfg(test)]
mod finalization_handoff_tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn cancellation_during_shutdown_prevents_finalizer_entry() {
        let (cancel, cancelled) = oneshot::channel();
        let interrupt = async move {
            cancelled.await.expect("signal sender retained");
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(interrupt.as_mut().now_or_never().is_none());
        let (closing, close_started) = oneshot::channel();
        let (settled, close_settled) = oneshot::channel();
        let observer = tokio::spawn(async move {
            close_started.await.unwrap();
            cancel.send(()).unwrap();
            settled.send(()).unwrap();
        });
        closing.send(()).unwrap();
        // The existing close is joined even after cancellation. Its settlement
        // is distinct from permission to enter the next native operation.
        close_settled.await.unwrap();
        observer.await.unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let flag = entered.clone();
        let ran = finalize_unless_cancelled(interrupt.as_mut(), move || async move {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert!(!ran);
        assert!(!entered.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_after_finalizer_entry_does_not_drop_settlement() {
        let (cancel, cancelled) = oneshot::channel();
        let interrupt = async move {
            cancelled.await.expect("signal sender retained");
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(interrupt.as_mut().now_or_never().is_none());
        let (entered, started) = oneshot::channel();
        let (finish, finishing) = oneshot::channel();
        let settled = Arc::new(AtomicBool::new(false));
        let effect_settled = settled.clone();
        let operation = finalize_unless_cancelled(interrupt.as_mut(), move || async move {
            entered.send(()).unwrap();
            finishing.await.unwrap();
            effect_settled.store(true, Ordering::SeqCst);
            Ok(())
        });
        let observer = async {
            started.await.unwrap();
            cancel.send(()).unwrap();
            tokio::task::yield_now().await;
            assert!(!settled.load(Ordering::SeqCst));
            finish.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(operation, observer);
        assert!(result.unwrap());
        assert!(settled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn failed_cancellation_listener_prevents_finalizer_entry() {
        let interrupt = async { Err(io::Error::other("signal listener unavailable")) };
        tokio::pin!(interrupt);
        let result = finalize_unless_cancelled(interrupt.as_mut(), || async {
            panic!("must not enter finalizer after listener failure");
        })
        .await;
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{SessionId, TurnErrorMetadata, lifecycle::InputId};
    use meerkat_runtime::input_state::InputAbandonReason;

    #[test]
    fn credential_commands_are_available_without_setup_options() {
        for arguments in [
            vec![
                "meerkat-connect",
                "--realm",
                "meta",
                "credentials",
                "status",
            ],
            vec![
                "meerkat-connect",
                "--realm",
                "meta",
                "credentials",
                "set",
                "cards",
                "--slot",
                "TOKEN",
            ],
            vec![
                "meerkat-connect",
                "--realm",
                "meta",
                "credentials",
                "remove",
                "cards",
                "--slot",
                "TOKEN",
            ],
        ] {
            assert!(
                Cli::try_parse_from(arguments).is_ok(),
                "credential management must be reachable without starting setup"
            );
        }
    }

    #[test]
    fn credential_commands_keep_setup_options_out_of_the_direct_route() {
        let ordinary =
            Cli::try_parse_from(["meerkat-connect", "--realm", "meta", "--model", "fixture"])
                .unwrap();
        assert!(ordinary.command.is_none());
        ordinary.check_command_options().unwrap();
        for arguments in [
            vec![
                "meerkat-connect",
                "--model",
                "fixture",
                "credentials",
                "status",
            ],
            vec![
                "meerkat-connect",
                "--model",
                "fixture",
                "credentials",
                "set",
                "cards",
                "--slot",
                "KEY",
            ],
            vec!["meerkat-connect", "--choose-model", "credentials", "status"],
            vec![
                "meerkat-connect",
                "--paired-host",
                "https://example.invalid",
                "credentials",
                "status",
            ],
            vec!["meerkat-connect", "--stdio", "credentials", "status"],
        ] {
            assert!(
                Cli::try_parse_from(arguments)
                    .unwrap()
                    .check_command_options()
                    .is_err()
            );
        }
        assert!(
            Cli::try_parse_from(["meerkat-connect", "credentials", "remove", "cards"]).is_err()
        );
        assert!(
            Cli::try_parse_from([
                "meerkat-connect",
                "credentials",
                "remove",
                "cards",
                "--slot",
                "TOKEN",
                "--value",
                "not-accepted"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["meerkat-connect", "credentials", "set", "cards"]).is_err());
        assert!(
            Cli::try_parse_from([
                "meerkat-connect",
                "credentials",
                "set",
                "cards",
                "--slot",
                "TOKEN",
                "--value",
                "not-accepted"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "meerkat-connect",
                "credentials",
                "set",
                "cards",
                "--slot",
                "TOKEN",
                "not-accepted"
            ])
            .is_err()
        );
        let set = Cli::try_parse_from([
            "meerkat-connect",
            "--realm",
            "meta",
            "credentials",
            "set",
            "cards",
            "--slot",
            "KEY",
            "--slot",
            "TOKEN",
        ])
        .unwrap();
        assert!(
            matches!(set.command, Some(Command::Credentials { action: CredentialAction::Set { service, slots } })
            if service == "cards" && slots == ["KEY", "TOKEN"])
        );
        let selected = Cli::try_parse_from([
            "meerkat-connect",
            "--realm",
            "meta",
            "credentials",
            "status",
        ])
        .unwrap();
        assert!(matches!(
            selected.command,
            Some(Command::Credentials {
                action: CredentialAction::Status { .. }
            })
        ));
    }

    #[test]
    fn failed_turn_new_input_requires_exact_terminal_witness_and_live_invocation() {
        let error = TurnErrorMetadata::runtime_apply_failure("fixture failure");
        let failed = [
            CompletionOutcome::Abandoned {
                reason: "fixture".into(),
                error: error.clone(),
            },
            CompletionOutcome::AbandonedWithError {
                reason: "fixture".into(),
                error: error.clone(),
            },
        ];
        let terminal = [
            InputTerminalOutcome::Consumed,
            InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
            },
        ];
        for outcome in &failed {
            for witness in &terminal {
                assert!(failed_turn_allows_new_input(outcome, Some(witness), false));
                assert!(!failed_turn_allows_new_input(outcome, Some(witness), true));
                assert!(
                    committed_text(outcome).is_err(),
                    "failure never becomes committed assistant text"
                );
            }
            // The same public failed-attempt class can accompany a requeued
            // input. Missing terminal authority cannot offer another turn.
            assert!(!failed_turn_allows_new_input(outcome, None, false));
            for witness in [
                InputTerminalOutcome::Superseded {
                    superseded_by: InputId::new(),
                },
                InputTerminalOutcome::Coalesced {
                    aggregate_id: InputId::new(),
                },
            ] {
                assert!(!failed_turn_allows_new_input(
                    outcome,
                    Some(&witness),
                    false
                ));
            }
        }
        let completed = serde_json::from_value(serde_json::json!({
            "text":"fixture", "session_id":SessionId::new(), "usage":meerkat_core::Usage::default(),
            "turns":1, "tool_calls":0
        }))
        .unwrap();
        for outcome in [
            CompletionOutcome::Completed(Box::new(completed)),
            CompletionOutcome::CompletedWithoutResult,
            CompletionOutcome::CallbackPending {
                tool_use_id: "fixture".into(),
                tool_name: "fixture".into(),
                args: serde_json::json!({}),
                dedupe_key: None,
            },
            CompletionOutcome::CallbackBatchPending {
                pending_tool_calls: Vec::new(),
            },
            CompletionOutcome::Cancelled,
            CompletionOutcome::CompletedWithFinalizationFailure {
                error: error.clone(),
            },
            CompletionOutcome::RuntimeTerminated {
                reason: "fixture".into(),
                error,
            },
        ] {
            for witness in &terminal {
                assert!(!failed_turn_allows_new_input(
                    &outcome,
                    Some(witness),
                    false
                ));
            }
        }
    }
}
