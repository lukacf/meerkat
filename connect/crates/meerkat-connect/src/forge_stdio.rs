//! Typed private-pipe presentation of the existing Forge request objects.
use super::*;
use crate::stdio::{PromptAnswer, PromptBridge, RecoveryChoice};
use serde_json::json;

pub(crate) async fn present(
    prompt: ForgePrompt,
    bridge: &PromptBridge,
    application: &str,
    origin: &str,
) -> Result<(), ConnectError> {
    match prompt {
        ForgePrompt::Operation(operation) => {
            configuration::browser_operation(operation, bridge).await
        }
        ForgePrompt::Selection(selection) => select(selection, bridge, application, origin).await,
    }
}
async fn select(
    mut prompt: ForgeSelection,
    bridge: &PromptBridge,
    application: &str,
    origin: &str,
) -> Result<(), ConnectError> {
    if bridge.retired() {
        return Err(ended());
    }
    let prior = prompt.prior.clone();
    let mut selected = prior.as_ref().map(|p| p.client.literal_origin.clone());
    if selected.is_none() {
        loop {
            let page = configuration::browser_recovery_page(&prompt.binding, application, origin)?;
            if page.projection()["choices"]
                .as_array()
                .is_some_and(Vec::is_empty)
                && page.skipped == 0
                && !page.more
            {
                break;
            }
            let answer = tokio::select! {biased;_=prompt.closed()=>return Err(ended()),answer=bridge.ask(page.projection())=>answer?};
            match answer {
                PromptAnswer::ForgeRecovery {
                    choice: RecoveryChoice::New,
                } => break,
                PromptAnswer::ForgeRecovery {
                    choice: RecoveryChoice::More,
                } if page.more => continue,
                PromptAnswer::ForgeRecovery {
                    choice: RecoveryChoice::Existing { choice_id },
                } => {
                    selected = Some(page.choose(&prompt.binding, &choice_id)?);
                    break;
                }
                _ => return Err(ended()),
            }
        }
    }
    if selected.is_none() {
        match tokio::select! {biased;_=prompt.closed()=>return Err(ended()),answer=bridge.ask(json!({"kind":"forge_origin","permitted_origin":origin}))=>answer?}
        {
            PromptAnswer::ForgeOrigin { origin: answer } if answer == origin => {
                selected = Some(answer)
            }
            _ => return Err(ended()),
        }
    }
    if selected.as_deref() != Some(origin) {
        return Err(ended());
    }
    let mut client = ForgeClient::new(origin).map_err(protocol)?;
    let form = tokio::select! {biased;_=bridge.closed()=>return Err(ended()),_=prompt.closed()=>return Err(ended()),result=client.login_form()=>result.map_err(protocol)?};
    let answer = tokio::select! {biased;_=prompt.closed()=>return Err(ended()),answer=bridge.ask(json!({"kind":"forge_password","origin":origin,"mode":if prior.is_some(){"reauthenticate"}else{"initial"},"min_chars":12,"max_chars":4096}))=>answer?};
    let PromptAnswer::ForgePassword { password } = answer else {
        return Err(ended());
    };
    let password = ForgePassword::new(password).map_err(protocol)?;
    if bridge.retired()
        || prompt
            .response
            .as_ref()
            .is_none_or(oneshot::Sender::is_closed)
    {
        return Err(ended());
    }
    let result = {
        let login = client.login(form, password);
        tokio::pin!(login);
        if let Some(result) = login.as_mut().now_or_never() {
            result
        } else {
            tokio::select! {biased;_=bridge.closed()=>login.await,_=prompt.closed()=>login.await,result=&mut login=>result}
        }
    };
    result.map_err(protocol)?;
    if bridge.retired()
        || prompt
            .response
            .as_ref()
            .is_none_or(oneshot::Sender::is_closed)
    {
        return Err(ended());
    }
    let support = tokio::select! {biased;_=bridge.closed()=>return Err(ended()),_=prompt.closed()=>return Err(ended()),result=client.support()=>result.map_err(protocol)?};
    if let ForgeSupport::Declared { host } = &support
        && host.application_id != application
    {
        return Err(protocol(ForgeError::UnexpectedResponse));
    }
    configuration::selected(&prompt.binding, prior.as_ref(), &client, &support)
        .map_err(protocol)?;
    if bridge.retired() {
        return Err(ended());
    }
    prompt.handoff(Captured { client, support }).await
}
