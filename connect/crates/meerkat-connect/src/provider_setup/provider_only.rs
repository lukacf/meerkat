//! Provider-only protected presentation. No session or service continuation.
//! The trusted parent selects roots and viewer; native owners retain meaning.
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
mod auth;
mod target;
#[allow(
    clippy::duplicate_mod,
    dead_code,
    reason = "stdio_io.rs is intentionally loaded twice with different MAX_FRAME limits; deduplication is a 0.9.10 follow-up"
)]
#[path = "../stdio_io.rs"]
mod transport;
mod wire;
use wire::{Action, Answer, BrowserTransport, Initialize, Request};

/// Trusted process composition, never a browser answer. NativeDefault is an
/// explicit choice of the host's fixed OS-user/HOME scope, not per-request
/// isolation. File is opt-in and must also reach the later guide factory.
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialScope {
    NativeDefault,
    File { root: std::path::PathBuf },
}
impl CredentialScope {
    fn valid(&self) -> bool {
        match self {
            Self::NativeDefault => true,
            Self::File { root } => {
                root.is_absolute() && root.to_str().is_some_and(|s| !s.contains('\0'))
            }
        }
    }
}

fn invalid(message: &str) -> ConnectError {
    ConnectError::InvalidInput(message.to_owned())
}
fn checked_presentation_expiry(
    now: tokio::time::Instant,
    remaining: Duration,
) -> Result<tokio::time::Instant, ConnectError> {
    now.checked_add(remaining).ok_or_else(|| {
        invalid("Provider initialization deadline cannot be represented on this host")
    })
}
struct Prompt {
    value: Value,
    reply: oneshot::Sender<Answer>,
}
enum Message {
    Prompt(Prompt),
    Notice(Value),
}
#[derive(Clone)]
struct Bridge {
    messages: mpsc::Sender<Message>,
    retired: watch::Receiver<bool>,
    deadline: u64,
    expires: tokio::time::Instant,
}
impl Bridge {
    fn retired(&self) -> bool {
        *self.retired.borrow()
            || self.retired.has_changed().is_err()
            || wire::now() >= self.deadline
            || tokio::time::Instant::now() >= self.expires
    }
    async fn closed(&self) {
        if self.retired() {
            return;
        }
        let mut retired = self.retired.clone();
        tokio::select! { biased;
            _=tokio::time::sleep_until(self.expires)=>{},
            _=async { while !*retired.borrow_and_update() { if retired.changed().await.is_err() { break; } } }=>{},
        }
    }
    async fn ask(&self, value: Value) -> Result<Answer, ConnectError> {
        if self.retired() {
            return Ok(Answer::Cancel);
        }
        let (reply, receive) = oneshot::channel();
        tokio::select! { biased; _=self.closed()=>return Ok(Answer::Cancel),
        sent=self.messages.send(Message::Prompt(Prompt { value, reply }))=>sent.map_err(|_| invalid("Provider presenter ended"))? }

        tokio::select! { biased; _=self.closed()=>Ok(Answer::Cancel),
        answer=receive=>answer.map_err(|_|invalid("Provider prompt ended")) }
    }
    async fn notice(&self, value: Value) -> Result<(), ConnectError> {
        tokio::select! { biased; _=self.closed()=>Ok(()),
        sent=self.messages.send(Message::Notice(value))=>sent.map_err(|_|invalid("Provider presenter ended")) }
    }
}

async fn current(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    request: &CreateSessionModelResolutionRequest,
) -> Result<(), ConnectError> {
    startup.config = crate::realm::read_config(&startup.storage).await?;
    let actual = resolve_explicit_auth_binding_target(&startup.config, &target.auth_binding)
        .map_err(|e| ConnectError::native("Revalidate provider target", e))?;
    if actual != *target {
        return Err(invalid("Provider target changed; restart selection"));
    }
    let selected = resolve_create_session_model(&startup.config, request.clone())
        .map_err(|e| ConnectError::native("Revalidate provider model", e))?;
    if request.model.as_ref() != Some(&selected.model)
        || request.provider != Some(selected.provider)
        || selected.auth_binding.as_ref() != Some(&target.auth_binding)
    {
        return Err(invalid("Provider model selection changed"));
    }
    Ok(())
}

async fn present(
    binding: Initialize,
    bridge: Bridge,
    interaction: String,
) -> Result<Option<Value>, ConnectError> {
    let storage = crate::RealmOptions {
        context_root: binding.context_root,
        state_root: Some(binding.state_root),
        user_config_root: Some(binding.user_config_root),
    }
    .resolve(&binding.realm_id)?;
    let mut startup = match binding.credential_scope {
        CredentialScope::NativeDefault => SessionStartup::open(storage).await?,
        CredentialScope::File { root } => {
            SessionStartup::open_with_provider_store(storage, root).await?
        }
    };
    // Explicit choices only, even if an environment/default could open a model.
    loop {
        if bridge.retired() {
            return Ok(None);
        }
        let choices = target::catalog(&mut startup).await?;
        let answer = bridge.ask(json!({"kind":"choices","choices":choices.iter().map(target::Choice::show).collect::<Vec<_>>(),
            "restart":"new_interaction_native_status_reobserved"})).await?;
        let id = match answer {
            Answer::Choice { choice_id } => choice_id,
            Answer::Cancel => return Ok(None),
            _ => return Err(invalid("Unexpected provider selection")),
        };
        let choice = choices
            .iter()
            .find(|c| c.id == id && c.enabled)
            .ok_or_else(|| invalid("Unknown provider choice"))?;
        let (target, request) = if let Some(target) = choice.target.as_ref() {
            (target.clone(), choice.request(target.auth_binding.clone()))
        } else {
            let prepared = target::prepare(&mut startup, choice).await?;
            match bridge.ask(prepared.review.clone()).await? {
                Answer::Confirm => {}
                Answer::Cancel => return Ok(None),
                _ => return Err(invalid("Unexpected target confirmation")),
            }
            prepared.commit(&mut startup, &bridge).await?
        };
        loop {
            if bridge.retired() {
                return Ok(None);
            }
            current(&mut startup, &target, &request).await?;
            let observed = observe(&mut startup, &target).await?;
            let mut actions = vec![Action::Back];
            if observed.admits_open() {
                actions.push(Action::Use);
            }
            if managed_api_key(&target) {
                actions.push(Action::EnterKey);
            }
            if oauth_identity(&target).is_some() {
                actions.push(Action::SignIn);
            }
            let action = bridge.ask(json!({"kind":"action","selection":choice.show(),
                "auth_binding":target.auth_binding,"credential":observed.label(),"actions":actions})).await?;
            current(&mut startup, &target, &request).await?;
            if bridge.retired() {
                return Ok(None);
            }
            match action {
                Answer::Cancel => return Ok(None),
                Answer::Action {
                    action: Action::Back,
                } => break,
                Answer::Action {
                    action: Action::Use,
                } => {
                    let observed = observe(&mut startup, &target).await?;
                    if !observed.admits_open() {
                        return Err(invalid("Native credentials do not admit selection"));
                    }
                    if bridge.retired() {
                        return Ok(None);
                    }
                    return Ok(Some(
                        json!({"interaction_id":interaction,"model":choice.model,"provider":choice.provider,
                        "auth_binding":target.auth_binding,"credential":observed.label(),"live_entitlement":"unobserved"}),
                    ));
                }
                Answer::Action {
                    action: Action::EnterKey,
                } if managed_api_key(&target) => {
                    let key = match bridge
                        .ask(json!({"kind":"key","auth_binding":target.auth_binding}))
                        .await?
                    {
                        Answer::Key { key } => key,
                        Answer::Cancel => return Ok(None),
                        _ => return Err(invalid("Unexpected key response")),
                    };
                    current(&mut startup, &target, &request).await?;
                    if bridge.retired() {
                        return Ok(None);
                    }
                    let persistence = startup
                        .factory
                        .resolution_provider_auth_persistence()
                        .map_err(|e| ConnectError::provider_setup("Open native credentials", e))?
                        .ok_or_else(|| invalid("Native credential storage unavailable"))?;
                    let interrupt = async {
                        bridge.closed().await;
                        Ok(())
                    };
                    tokio::pin!(interrupt);
                    let saved = save_api_key(
                        &mut startup,
                        &target,
                        persistence,
                        meerkat_core::auth::token_store::PersistedTokens::api_key(key),
                        &mut io::sink(),
                        interrupt.as_mut(),
                    )
                    .await?;
                    if !saved {
                        return Ok(None);
                    }
                }
                Answer::Action {
                    action: Action::SignIn,
                } => {
                    let identity = oauth_identity(&target)
                        .ok_or_else(|| invalid("No native interactive flow"))?;
                    let completed = if identity == OAuthProviderIdentity::GitHubCopilot {
                        auth::device(&mut startup, &target, &bridge).await?
                    } else {
                        auth::browser(
                            &mut startup,
                            &target,
                            &request,
                            identity,
                            binding.browser,
                            &bridge,
                        )
                        .await?
                    };
                    if !completed {
                        return Ok(None);
                    }
                }
                _ => return Err(invalid("Unsupported provider action")),
            }
            // Native completion is followed by a new exact configuration/status
            // observation. It is never itself a grant to start an agent.
        }
    }
}

async fn retire_before_output<W: AsyncWrite + Unpin>(
    retire: &watch::Sender<bool>,
    output: &mut W,
    value: Value,
) -> Result<(), ConnectError> {
    // Admission retirement is synchronous. A backpressured output observer
    // cannot defer it or permit another provider request.
    retire.send_replace(true);
    emit(output, value).await
}

async fn emit<W: AsyncWrite + Unpin>(output: &mut W, value: Value) -> Result<(), ConnectError> {
    let mut bytes = serde_json::to_vec(&value)
        .map_err(|e| ConnectError::native("Encode provider response", e))?;
    if bytes.len() > wire::MAX_FRAME {
        return Err(invalid("Provider response exceeds frame bound"));
    }
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(30), async {
        output.write_all(&bytes).await?;
        output.flush().await
    })
    .await
    .map_err(|_| invalid("Provider output timed out"))?
    .map_err(|e| ConnectError::native("Write provider response", e))
}

// Some macOS runtimes/frameworks initialize this text-encoding metadata after
// exec. It is neither a provider credential nor a configuration/root selector.
// SDK launch arguments still exclude it; do not admit an arbitrary prefix or
// silently strip unknown environment entries at this native boundary.
fn runtime_environment_metadata(key: &str, value: &std::ffi::OsStr) -> bool {
    #[cfg(target_os = "macos")]
    if key == "__CF_USER_TEXT_ENCODING" {
        let Some(value) = value.to_str() else {
            return false;
        };
        if value.len() > 32 {
            return false;
        }
        let fields: Vec<_> = value.split(':').collect();
        if fields.len() != 3 {
            return false;
        }
        let parsed: Option<Vec<u32>> = fields
            .into_iter()
            .map(|field| {
                let hex = field.strip_prefix("0x")?;
                if hex.is_empty()
                    || hex.len() > 8
                    || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return None;
                }
                u32::from_str_radix(hex, 16).ok()
            })
            .collect();
        return parsed.is_some_and(|fields| {
            fields[0] == nix::unistd::geteuid().as_raw()
                && nix::unistd::getuid() == nix::unistd::geteuid()
        });
    }
    let _ = (key, value);
    false
}
fn provider_environment_entry(
    native_default: bool,
    key: &std::ffi::OsStr,
    value: &std::ffi::OsStr,
) -> bool {
    key.to_str().is_some_and(|key| {
        wire::ENVIRONMENT.contains(&key)
            || (native_default && wire::NATIVE_ENVIRONMENT.contains(&key))
            || runtime_environment_metadata(key, value)
    })
}

/// Dedicated parent-only JSONL endpoint. SDK launchers pass a minimal explicit
/// environment; no ambient credential/config environment or global HOME edits.
pub async fn run(scope: CredentialScope) -> Result<(), ConnectError> {
    let native_default = scope == CredentialScope::NativeDefault;
    if !scope.valid()
        || std::env::vars_os()
            .any(|(key, value)| !provider_environment_entry(native_default, &key, &value))
    {
        return Err(invalid(
            "Provider-only launch requires an isolated host-selected environment",
        ));
    }
    if native_default
        && !std::env::var_os("HOME").is_some_and(|home| std::path::Path::new(&home).is_absolute())
    {
        return Err(invalid(
            "Native-default scope requires the host's fixed HOME at launch",
        ));
    }
    let input = transport::Pipe::inherited(std::io::stdin())
        .map_err(|_| invalid("Provider input unavailable"))?;
    let output = transport::Pipe::inherited(std::io::stdout())
        .map_err(|_| invalid("Provider output unavailable"))?;
    run_io(input, output, scope)
        .await
        .map_err(|_| invalid("Provider setup failed; native effects require reobservation"))
}
async fn run_io<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    mut output: W,
    scope: CredentialScope,
) -> Result<(), ConnectError> {
    let mut frames = transport::Frames::new(input);
    let first = tokio::time::timeout(Duration::from_secs(60), frames.next())
        .await
        .map_err(|_| invalid("Provider initialization timed out"))?
        .map_err(|_| invalid("Invalid provider frame"))?
        .ok_or_else(|| invalid("Provider parent ended"))?;
    let Request::Initialize { id, binding } =
        serde_json::from_slice(&first).map_err(|_| invalid("Invalid provider initialization"))?
    else {
        return Err(invalid("Provider initialization required"));
    };
    if !wire::opaque(&id) || !binding.valid() || binding.credential_scope != scope {
        return Err(invalid("Invalid trusted provider binding"));
    }
    let interaction = uuid::Uuid::new_v4().to_string();
    let deadline = binding.expires_at_ms;
    let (sender, mut messages) = mpsc::channel(8);
    let (retire, retired) = watch::channel(false);
    let expires = checked_presentation_expiry(
        tokio::time::Instant::now(),
        Duration::from_millis(deadline.saturating_sub(wire::now())),
    )?;
    let bridge = Bridge {
        messages: sender,
        retired,
        deadline,
        expires,
    };
    let selected_id = interaction.clone();
    let mut task = tokio::spawn(async move { present(binding, bridge, selected_id).await });
    let mut pending: Option<(String, Prompt)> = None;
    let mut seen = BTreeSet::from([id.clone()]);
    let mut outcome = None;
    // Every fallible transport step is inside this observer body. On any exit
    // the retained operation is retired and joined below, never aborted.
    let transport_result: Result<(), ConnectError> = async {
        emit(&mut output,json!({"id":id,"kind":"initialized","protocol":wire::PROTOCOL,
            "interaction_id":interaction,"restart":"new_interaction_only"})).await?;
        loop {
            tokio::select! { biased;
                result=&mut task => { outcome=Some(result); break; },
                _=tokio::time::sleep_until(expires) => { break; },
                message=messages.recv()=>match message {
                    Some(Message::Notice(value))=>emit(&mut output,json!({"kind":"notice","interaction_id":interaction,"notice":value})).await?,
                    Some(Message::Prompt(prompt))=>{
                        if let Some((old,_))=pending.take() { emit(&mut output,json!({"kind":"prompt_closed","prompt_id":old})).await?; }
                        let prompt_id=uuid::Uuid::new_v4().to_string();
                        emit(&mut output,json!({"kind":"prompt","interaction_id":interaction,"prompt_id":prompt_id,"prompt":prompt.value})).await?;
                        pending=Some((prompt_id,prompt));
                    },
                    None=>{},
                },
                frame=frames.next()=>{
                    let Some(bytes)=frame.map_err(|_|invalid("Invalid provider frame"))? else { break; };
                    let request:Request=serde_json::from_slice(&bytes).map_err(|_|invalid("Invalid provider request"))?;
                    match request {
                        Request::Answer { id, prompt_id, answer }=>{
                            if !wire::opaque(&id) || !seen.insert(id.clone()) || seen.len()>1024 { return Err(invalid("Invalid provider request identity")); }
                            let Some((expected,prompt))=pending.take() else { return Err(invalid("No active provider prompt")); };
                            if expected!=prompt_id || !wire::valid_answer(&prompt.value,&answer) { return Err(invalid("Invalid provider answer")); }
                            if matches!(answer,Answer::Cancel) { retire.send_replace(true); }
                            emit(&mut output,json!({"id":id,"kind":"answered","prompt_id":expected})).await?;
                            let _=prompt.reply.send(answer);
                        },
                        Request::Cancel { id }=>{
                            if !wire::opaque(&id) || !seen.insert(id.clone()) { return Err(invalid("Invalid cancellation identity")); }
                            retire_before_output(&retire,&mut output,json!({"id":id,"kind":"cancelling","interaction_id":interaction})).await?;
                            break;
                        },
                        Request::Initialize { .. }=>return Err(invalid("Provider binding cannot be replaced")),
                    }
                }
            }
        }
        Ok(())
    }.await;
    retire.send_replace(true);
    if let Some((prompt_id, prompt)) = pending.take() {
        drop(prompt);
        // Closing notification is best effort after transport failure. The
        // native operation still joins before this owner returns.
        let _ = emit(
            &mut output,
            json!({"kind":"prompt_closed","prompt_id":prompt_id}),
        )
        .await;
    }
    let result = match outcome {
        Some(result) => result,
        None => task.await,
    }
    .map_err(|_| invalid("Provider owner failed"))?;
    if let Err(error) = transport_result {
        return Err(combined(error, result.map(|_| ())));
    }
    match result {
        Ok(Some(selection)) => {
            emit(
                &mut output,
                json!({"kind":"selected","selection":selection}),
            )
            .await
        }
        Ok(None) => {
            emit(
                &mut output,
                json!({"kind":"cancelled","interaction_id":interaction,
            "native_effects":"reobserve","joined":true,"join_scope":"provider_interaction"}),
            )
            .await
        }
        Err(error) => {
            // Native free-form messages can include secrets. Preserve failure
            // versus cancellation but never serialize the underlying error.
            let written=emit(&mut output,json!({"kind":"failed","interaction_id":interaction,
                "reason":"native_refusal_or_unavailable","native_effects":"reobserve","joined":true,"join_scope":"provider_interaction"})).await;
            Err(combined(error, written))
        }
    }
}

#[cfg(test)]
mod tests;
