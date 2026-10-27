//! Private presentation of the existing paired host-selection owner.
//! A browser login never imports the terminal's OS-user device credential.
use crate::{
    ConnectError,
    paired_host::{PairedHostClient, PairedHostError, PairingCode, TerminalPairingCode},
    host_selection::HostSelectionPrompt,
    stdio::{PairingMethod, PromptAnswer, PromptBridge},
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::future::Future;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub target_id: String,
    pub origin: String,
}

fn ended() -> ConnectError {
    ConnectError::InvalidInput("The protected PairedHost interaction ended. No pairing retry was started; PairedHost retains any enrollment it accepted.".into())
}

pub(crate) async fn entered<T>(
    prompt: &mut HostSelectionPrompt,
    bridge: &PromptBridge,
    operation: impl Future<Output = Result<T, PairedHostError>>,
) -> Result<T, ConnectError> {
    if bridge.retired() || prompt.is_closed() {
        return Err(ended());
    }
    tokio::pin!(operation);
    let result = if let Some(result) = operation.as_mut().now_or_never() {
        result
    } else {
        // Retain the exact first-polled HTTP operation. Cancelling local
        // presentation cannot undo an enrollment accepted by PairedHost.
        tokio::select! {
            biased;
            _ = bridge.closed() => operation.await,
            _ = prompt.closed() => operation.await,
            result = &mut operation => result,
        }
    };
    result.map_err(|error| ConnectError::native("pair selected PairedHost", error))
}

pub(crate) async fn present(
    mut prompt: HostSelectionPrompt,
    bridge: &PromptBridge,
    targets: &[Target],
) -> Result<(), ConnectError> {
    if bridge.retired() || prompt.is_closed() {
        return Err(ended());
    }
    let answer = tokio::select! {
        biased;
        _ = prompt.closed() => return Err(ended()),
        answer = bridge.ask(json!({"kind":"paired_host_target","choices":targets,"custody":"process_only"})) => answer?,
    };
    let PromptAnswer::PairedHostTarget { target_id } = answer else {
        return Err(ended());
    };
    let origin = &targets
        .iter()
        .find(|target| target.target_id == target_id)
        .ok_or_else(ended)?
        .origin;
    let mut client = PairedHostClient::new(origin).map_err(|_| ended())?;
    let answer = tokio::select! {
        biased;
        _ = prompt.closed() => return Err(ended()),
        answer = bridge.ask(json!({"kind":"paired_host_pairing_method","origin":origin,"methods":["request","terminal"]})) => answer?,
    };
    let PromptAnswer::PairedHostPairingMethod { method } = answer else {
        return Err(ended());
    };
    if method == PairingMethod::Request {
        entered(
            &mut prompt,
            bridge,
            client.request_pairing("Meerkat Connect Console"),
        )
        .await?;
    }
    if bridge.retired() || prompt.is_closed() {
        return Err(ended());
    }
    let answer = tokio::select! {
        biased;
        _ = prompt.closed() => return Err(ended()),
        answer = bridge.ask(json!({"kind":"paired_host_pairing_code","origin":origin,"method":method})) => answer?,
    };
    let PromptAnswer::PairedHostPairingCode { code } = answer else {
        return Err(ended());
    };
    match method {
        PairingMethod::Request => {
            let code = PairingCode::new(code).map_err(|_| ended())?;
            entered(&mut prompt, bridge, client.confirm_pairing(code)).await?;
        }
        PairingMethod::Terminal => {
            let code = TerminalPairingCode::new(code).map_err(|_| ended())?;
            entered(&mut prompt, bridge, client.confirm_terminal_pairing(code)).await?;
        }
    }
    if bridge.retired() || prompt.is_closed() {
        return Err(ended());
    }
    tokio::select! {
        biased;
        _ = bridge.closed() => Err(ended()),
        result = prompt.handoff(client) => result,
    }
}
