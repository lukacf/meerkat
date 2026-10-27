//! One human-selected host, captured inside the original setup conversation.
//! The model requests this interaction but supplies no origin or credential.

use std::{
    future::Future,
    io::{self, Write},
    sync::{Arc, Mutex, OnceLock},
};

use meerkat_core::{ToolDef, ToolError, ToolResult, ops::ToolDispatchOutcome, types::ToolCallView};
use tokio::sync::{mpsc, oneshot};

use crate::{
    ConnectError,
    paired_host::PairedHostClient,
    paired_host_device::OsDeviceStore,
    pairing::{pair_paired_host, pair_paired_host_with_store},
    terminal::TerminalInput,
};

pub const PAIRED_HOST_SELECT_HOST: &str = "paired_host_select_host";

pub(crate) struct CapturedHost {
    pub(crate) origin: String,
    pub(crate) client: tokio::sync::Mutex<PairedHostClient>,
}

#[derive(Default)]
struct SelectionState {
    active: Option<Arc<()>>,
    retired: bool,
}

pub(crate) struct HostSelection {
    captured: OnceLock<CapturedHost>,
    state: Mutex<SelectionState>,
    prompts: Option<mpsc::Sender<HostSelectionPrompt>>,
}

impl HostSelection {
    pub(crate) fn prebound(client: PairedHostClient) -> Arc<Self> {
        Arc::new(Self {
            captured: OnceLock::from(CapturedHost {
                origin: client.selected_origin().into(),
                client: tokio::sync::Mutex::new(client),
            }),
            state: Mutex::new(SelectionState::default()),
            prompts: None,
        })
    }

    pub(crate) fn deferred() -> (Arc<Self>, HostSelectionPresenter) {
        let (send, receive) = mpsc::channel(1);
        let binding = Arc::new(Self {
            captured: OnceLock::new(),
            state: Mutex::new(SelectionState::default()),
            prompts: Some(send),
        });
        (binding, HostSelectionPresenter { prompts: receive })
    }

    pub(crate) fn captured(&self) -> Option<&CapturedHost> {
        self.captured.get()
    }

    pub(crate) fn offers_selection(&self) -> bool {
        self.prompts.is_some()
    }

    pub(crate) fn retire(&self) -> Result<(), ConnectError> {
        let mut state = self.state.lock().map_err(|_| ended())?;
        state.retired = true;
        state.active = None;
        Ok(())
    }

    /// Move the same private connection out only after every old selection
    /// owner is gone. Consuming the OnceLock cannot revive a retired lease.
    pub(crate) fn into_continuation_client(self) -> Result<Option<PairedHostClient>, ConnectError> {
        let state = self.state.into_inner().map_err(|_| ended())?;
        if !state.retired || state.active.is_some() {
            return Err(ConnectError::InvalidInput(
            "The previous PairedHost selection has not settled; no continuation connection was created.".into()));
        }
        Ok(self
            .captured
            .into_inner()
            .map(|host| host.client.into_inner()))
    }

    fn retire_request(&self, token: &Arc<()>) {
        if let Ok(mut state) = self.state.lock()
            && state
                .active
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, token))
        {
            state.active = None;
            // A lost/failed human handoff is not an instruction to pair again.
            // Starting another CLI invocation is explicit new interaction.
            state.retired = self.captured.get().is_none();
        }
    }

    pub(crate) async fn request(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != PAIRED_HOST_SELECT_HOST || !self.offers_selection() {
            return Err(ToolError::not_found(call.name));
        }
        if call.args.get().len() > 1024
            || !call
                .parse_args::<serde_json::Map<String, serde_json::Value>>()
                .is_ok_and(|value| value.is_empty())
        {
            return Err(ToolError::execution_failed(
                "Host selection takes only an empty object; the person selects the host in the protected setup controls",
            ));
        }
        if self.captured.get().is_some() {
            return Ok(ToolResult::new(
                call.id.into(),
                r#"{"state":"selected","changed":false}"#.into(),
                false,
            )
            .into());
        }
        let token = Arc::new(());
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| ToolError::access_denied(call.name))?;
            if state.retired || state.active.is_some() {
                return Err(ToolError::access_denied(call.name));
            }
            state.active = Some(token.clone());
        }
        let _lease = SelectionLease {
            binding: self.clone(),
            token: token.clone(),
        };
        let (response, receive) = oneshot::channel();
        self.prompts
            .as_ref()
            .ok_or_else(|| ToolError::access_denied(call.name))?
            .try_send(HostSelectionPrompt {
                binding: self.clone(),
                token,
                response: Some(response),
            })
            .map_err(|_| {
                ToolError::execution_failed(
                    "The protected host interaction is unavailable; stop without retrying pairing",
                )
            })?;
        let (client, acknowledged) = receive.await.map_err(|_| ToolError::execution_failed("The protected host interaction ended without a confirmed handoff; stop without retrying pairing"))?;
        {
            let state = self
                .state
                .lock()
                .map_err(|_| ToolError::access_denied(call.name))?;
            if state.retired
                || !state
                    .active
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, &_lease.token))
            {
                return Err(ToolError::access_denied(call.name));
            }
            // Publication runs inside the live native dispatch future with no
            // intervening await. A dropped receiver cannot publish a host.
            self.captured
                .set(CapturedHost {
                    origin: client.selected_origin().into(),
                    client: tokio::sync::Mutex::new(client),
                })
                .map_err(|_| ToolError::access_denied(call.name))?;
        }
        let _ = acknowledged.send(());
        Ok(ToolResult::new(
            call.id.into(),
            r#"{"state":"selected","changed":true}"#.into(),
            false,
        )
        .into())
    }
}

struct SelectionLease {
    binding: Arc<HostSelection>,
    token: Arc<()>,
}
impl Drop for SelectionLease {
    fn drop(&mut self) {
        self.binding.retire_request(&self.token);
    }
}

pub(crate) fn definition() -> Arc<ToolDef> {
    Arc::new(ToolDef::new(
        PAIRED_HOST_SELECT_HOST,
        "Ask the protected setup controls to select and pair one PairedHost extension host in this same conversation. Supply only an empty object. Never ask for or send its origin, pairing code, password, token or cookie in chat. The person chooses among the host and credential-custody options offered privately. Console pairing is process-only; the standalone CLI may offer remembered pairing. This can change pairing/device custody and is not a read-only call. It authorizes no extension change, secret intent or installation; those keep their own exact approvals. Once captured the host cannot be replaced in this conversation. If this interaction ends or times out, stop; do not retry pairing or infer that host enrollment was undone.",
        serde_json::json!({"type":"object","additionalProperties":false,"properties":{}}),
    ))
}

/// Sole protected host-selection receiver. It has no model-visible serialization or Debug view.
pub struct HostSelectionPresenter {
    prompts: mpsc::Receiver<HostSelectionPrompt>,
}
impl HostSelectionPresenter {
    pub async fn next(&mut self) -> Option<HostSelectionPrompt> {
        self.prompts.recv().await
    }
}

/// A live native tool invocation's private handoff, not a reusable host grant.
pub struct HostSelectionPrompt {
    binding: Arc<HostSelection>,
    token: Arc<()>,
    response: Option<oneshot::Sender<(PairedHostClient, oneshot::Sender<()>)>>,
}

impl HostSelectionPrompt {
    pub(crate) fn is_closed(&self) -> bool {
        self.response
            .as_ref()
            .is_none_or(oneshot::Sender::is_closed)
    }

    pub(crate) async fn closed(&mut self) {
        if let Some(response) = self.response.as_mut() {
            response.closed().await;
        }
        self.binding.retire_request(&self.token);
    }

    pub(crate) async fn handoff(&mut self, client: PairedHostClient) -> Result<(), ConnectError> {
        let (acknowledge, published) = oneshot::channel();
        self.response
            .take()
            .ok_or_else(ended)?
            .send((client, acknowledge))
            .map_err(|_| ended())?;
        published.await.map_err(|_| ended())
    }
}

impl Drop for HostSelectionPrompt {
    fn drop(&mut self) {
        self.binding.retire_request(&self.token);
    }
}

fn ended() -> ConnectError {
    ConnectError::InvalidInput("The native host-selection request has ended. No late binding or automatic pairing retry is allowed; any completed host enrollment remains owned by PairedHost.".into())
}

/// Own the existing terminal exclusively until pairing settles or the exact
/// native waiter retires. The caller must cancel/join the native turn on false
/// or error, and must not resume this interaction or automatically request again.
pub async fn present_host_selection(
    mut request: HostSelectionPrompt,
    input: &mut TerminalInput,
    output: &mut impl Write,
    interrupted: impl Future<Output = io::Result<()>>,
) -> Result<bool, ConnectError> {
    input
        .require_terminal()
        .map_err(|error| ConnectError::native("check protected host selection", error))?;
    tokio::pin!(interrupted);
    writeln!(output, "Select the running PairedHost for this extension. This realm keeps the same setup conversation. Enter the origin here, never in chat. /quit ends setup.")
        .map_err(|error| ConnectError::native("write host selection", error))?;
    let origin = loop {
        write!(output, "PairedHost origin: ")
            .and_then(|_| output.flush())
            .map_err(|error| ConnectError::native("write host selection", error))?;
        let line = tokio::select! {
            biased;
            _ = request.closed() => Err(ended()),
            signal = &mut interrupted => signal.map(|_| None).map_err(|error| ConnectError::native("interrupt host selection", error)),
            line = input.recv() => line.transpose().map_err(|error| ConnectError::native("read selected host", error)),
        };
        input
            .check_cleanup()
            .map_err(|error| ConnectError::native("restore protected terminal", error))?;
        let Some(line) = line? else {
            return Ok(false);
        };
        if line == "/quit" {
            return Ok(false);
        }
        if PairedHostClient::new(&line).is_ok() {
            break line;
        }
        writeln!(
            output,
            "Enter a supported PairedHost origin without a path, query, fragment or credentials."
        )
        .map_err(|error| ConnectError::native("write host selection", error))?;
    };
    let remember = loop {
        write!(output, "Type 'remember' to use the OS credential store, 'memory' for this process only, or /quit: ")
            .and_then(|_| output.flush()).map_err(|error| ConnectError::native("write pairing custody choice", error))?;
        let line = tokio::select! {
            biased;
            _ = request.closed() => Err(ended()),
            signal = &mut interrupted => signal.map(|_| None).map_err(|error| ConnectError::native("interrupt host selection", error)),
            line = input.recv() => line.transpose().map_err(|error| ConnectError::native("read pairing custody choice", error)),
        };
        input
            .check_cleanup()
            .map_err(|error| ConnectError::native("restore protected terminal", error))?;
        match line?.as_deref() {
            Some("remember") => break true,
            Some("memory") => break false,
            None | Some("/quit") => return Ok(false),
            _ => {}
        }
    };
    let paired = {
        let cancelled = async {
            tokio::select! {
                biased;
                _ = request.closed() => Ok(()),
                signal = &mut interrupted => signal,
            }
        };
        if remember {
            pair_paired_host_with_store(&origin, Arc::new(OsDeviceStore), input, output, cancelled)
                .await?
        } else {
            writeln!(output, "Pairing is kept only for this process. The OS credential store will not be read or written. PairedHost enrollment is not revoked when this process ends.")
                .map_err(|error| ConnectError::native("write pairing custody", error))?;
            pair_paired_host(&origin, input, output, cancelled).await?
        }
    };
    let Some(client) = paired else {
        return if request
            .response
            .as_ref()
            .is_none_or(oneshot::Sender::is_closed)
        {
            Err(ended())
        } else {
            Ok(false)
        };
    };
    tokio::select! {
        biased;
        signal = &mut interrupted => {
            signal.map_err(|error| ConnectError::native("interrupt host selection", error))?;
            Ok(false)
        },
        outcome = request.handoff(client) => outcome.map(|()| true),
    }
}
