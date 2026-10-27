//! Protected startup presentation for native model targets.
//!
//! First-target setup adds reviewed native configuration through its own realm
//! owner. Native resolution, credential status, OAuth state, exchange and
//! persistence retain their existing owners.
//! The terminal and callback are used before any agent exists. The caller must
//! await this interaction, including cleanup, before consuming SessionStartup.

use futures_util::FutureExt;
use std::{
    future::Future,
    io::{self, Write},
    pin::Pin,
    time::Duration,
};

use meerkat::{
    CreateSessionModelResolutionError, CreateSessionModelResolutionRequest, HostAuthDevicePoll,
    HostAuthService, HostAuthTarget, resolve_create_session_model,
};
use meerkat_auth_core::{
    auth_oauth::{OAuthError, bind_loopback_callback_with_redirect},
    oauth_flow::validate_oauth_login_connection_target,
};
use meerkat_core::auth::token_store::PersistedAuthMode;
use meerkat_core::connection::RealmChain;
use meerkat_core::{
    AuthBindingRef, AuthStatusPhase, BindingId, BindingOrigin, CredentialSourceSpec,
    OAuthProviderIdentity, ResolvedConnectionTarget, resolve_auth_binding_candidates_for_provider,
    resolve_explicit_auth_binding_target,
};
use meerkat_llm_core::provider_runtime::{
    binding::NormalizedAuthMethod, registry::ResolverEnvironment, runtime::CredentialReadiness,
};

use crate::{
    ConnectError, SessionStartup,
    terminal::{TerminalInput, review_text},
};

const CALLBACK_WINDOW: Duration = Duration::from_secs(120);
const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEVICE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[path = "provider_stdio.rs"]
pub(crate) mod private;

mod first_target;

// Native operations create these values. Presenters must not recover them from
// terminal output or infer provider completion from a displayed URL.
enum SignInNotice {
    Browser {
        authorization_url: String,
    },
    Device {
        verification_uri: String,
        user_code: String,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SignInDisposition {
    Completed,
    Refused,
    Expired,
}
struct SignInPresentation<'a> {
    show: &'a mut (dyn FnMut(SignInNotice) -> io::Result<()> + Send),
    disposition: SignInDisposition,
}
impl<'a> SignInPresentation<'a> {
    fn new(show: &'a mut (dyn FnMut(SignInNotice) -> io::Result<()> + Send)) -> Self {
        Self {
            show,
            disposition: SignInDisposition::Refused,
        }
    }
    fn expired(&mut self) {
        self.disposition = SignInDisposition::Expired;
    }
    fn observe_device_poll(&mut self, outcome: &HostAuthDevicePoll) {
        if matches!(outcome, HostAuthDevicePoll::Expired) {
            self.expired();
        }
    }
    fn observe_callback_error(&mut self, error: &OAuthError) {
        if matches!(error, OAuthError::Timeout) {
            self.expired();
        }
    }
    fn disposition(&self, result: &Result<bool, ConnectError>) -> SignInDisposition {
        if matches!(result, Ok(true)) {
            SignInDisposition::Completed
        } else {
            self.disposition
        }
    }
}

fn device_window_elapsed(
    expires_at: std::time::Instant,
    presentation: &mut Option<&mut SignInPresentation<'_>>,
) -> bool {
    let elapsed = std::time::Instant::now() >= expires_at;
    if elapsed && let Some(present) = presentation.as_deref_mut() {
        present.expired();
    }
    elapsed
}

enum Observation {
    Credential(CredentialReadiness),
    OAuth(AuthStatusPhase),
}

impl Observation {
    // This selects UI, not credential authority. Expired OAuth credentials
    // still go through the native open's refresh policy; this UI never refreshes.
    fn admits_open(&self) -> bool {
        match self {
            Self::Credential(value) => value.admits_open(),
            Self::OAuth(value) => matches!(
                value,
                AuthStatusPhase::Valid | AuthStatusPhase::Expiring | AuthStatusPhase::Expired
            ),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Credential(CredentialReadiness::Ready) => "ready",
            Self::Credential(CredentialReadiness::Missing) => "credential not ready",
            Self::Credential(CredentialReadiness::NeedsReauth) => "sign-in required",
            Self::Credential(_) => "resolved during native open",
            Self::OAuth(value) => value.as_public_str(),
        }
    }
}

fn oauth_identity(target: &ResolvedConnectionTarget) -> Option<OAuthProviderIdentity> {
    if target.auth_binding.is_env_default() {
        return None;
    }
    let mode =
        NormalizedAuthMethod::from_auth_profile(&target.auth_profile)?.persisted_auth_mode()?;
    OAuthProviderIdentity::INTERACTIVE
        .iter()
        .copied()
        .find(|identity| identity.auth_mode() == mode)
}

fn host_target(
    target: &ResolvedConnectionTarget,
    provider: OAuthProviderIdentity,
) -> HostAuthTarget {
    HostAuthTarget {
        provider,
        realm_id: target.auth_binding.realm.clone(),
        binding_id: target.auth_binding.binding.clone(),
        profile_id: target.auth_binding.profile.clone(),
    }
}

async fn observe(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
) -> Result<Observation, ConnectError> {
    let persistence = startup
        .factory
        .resolution_provider_auth_persistence()
        .map_err(|e| ConnectError::provider_setup("Open native provider credentials", e))?;
    if let Some(identity) = oauth_identity(target) {
        validate_oauth_login_connection_target(target, identity)
            .map_err(|e| ConnectError::provider_setup("Validate configured sign-in target", e))?;
        let authority = startup.provider_auth_runtime_authority().await?;
        let service = HostAuthService::from_factory(&startup.factory, authority)
            .map_err(|e| ConnectError::provider_setup("Open native sign-in service", e))?;
        let status = service
            .status(&startup.config, &host_target(target, identity))
            .await
            .map_err(|e| ConnectError::provider_setup("Read native sign-in status", e))?;
        return Ok(Observation::OAuth(status.phase));
    }
    let mut environment = ResolverEnvironment::with_process_env();
    if let Some(persistence) = persistence {
        environment = environment.with_provider_auth_persistence(persistence);
    }
    // Readiness for a staged setup uses the same lease that will prepare its
    // controller. The ordinary env/static path still need not open a bundle.
    if startup.standalone.is_some() || startup.persistence.is_some() {
        environment = environment.with_auth_lease_handle(
            startup
                .provider_auth_runtime_authority()
                .await?
                .generated_auth_lease_handle(),
        );
    }
    let status = startup
        .factory
        .provider_runtime_registry()
        .observe_credential_readiness(&target.realm, &target.auth_binding, &environment)
        .await
        .map_err(|e| ConnectError::provider_setup("Observe native credential readiness", e))?;
    Ok(Observation::Credential(status))
}

fn configured_choices(startup: &SessionStartup) -> Result<Vec<AuthBindingRef>, ConnectError> {
    let chain = RealmChain::resolve(&startup.config, &startup.storage.locator.realm)
        .map_err(|e| ConnectError::native("Resolve model target inheritance", e))?;
    let mut choices = Vec::new();
    for realm in chain.realms() {
        if let Some(section) = startup.config.realm.get(realm.as_str()) {
            for binding in section.binding.keys() {
                let reference = AuthBindingRef {
                    realm: realm.clone(),
                    binding: BindingId::parse(binding.clone())
                        .map_err(|e| ConnectError::native("Read configured binding identity", e))?,
                    profile: None,
                    origin: BindingOrigin::Configured,
                };
                // Native owner stamping and profile validation remain decisive.
                let resolved = resolve_explicit_auth_binding_target(&startup.config, &reference)
                    .map_err(|e| ConnectError::native("Resolve configured model target", e))?;
                if !choices.contains(&resolved.auth_binding) {
                    choices.push(resolved.auth_binding);
                }
            }
        }
    }
    Ok(choices)
}

/// Noninteractive native readiness observation for a trusted private-pipe host.
/// This never starts a sign-in presenter or chooses a different binding.
pub(crate) async fn ready_for_open(
    startup: &mut SessionStartup,
    request: &CreateSessionModelResolutionRequest,
) -> Result<bool, ConnectError> {
    let resolution = resolve_create_session_model(&startup.config, request.clone())
        .map_err(|e| ConnectError::native("Resolve native model selection", e))?;
    let candidates = resolve_auth_binding_candidates_for_provider(
        &startup.config,
        resolution.provider,
        resolution.auth_binding.as_ref(),
        Some(&startup.storage.locator.realm),
        true,
    )
    .map_err(|e| ConnectError::native("Resolve native model candidates", e))?;
    let mut known_oauth_signin_required = false;
    for candidate in candidates {
        let observation = observe(startup, &candidate).await?;
        if matches!(
            observation,
            Observation::Credential(CredentialReadiness::MaterializedAtOpen)
        ) && known_oauth_signin_required
        {
            continue;
        }
        if observation.admits_open() {
            return Ok(true);
        }
        known_oauth_signin_required |= matches!(observation, Observation::OAuth(_));
    }
    Ok(false)
}

async fn line<F: Future<Output = io::Result<()>>, W: Write>(
    input: &mut TerminalInput,
    output: &mut W,
    text: &str,
    interrupt: Pin<&mut F>,
) -> Result<Option<String>, ConnectError> {
    write!(output, "{text}")
        .and_then(|()| output.flush())
        .map_err(|e| ConnectError::native("Write model setup prompt", e))?;
    let result = tokio::select! {
        biased;
        result = interrupt => result.map(|()| None)
            .map_err(|e| ConnectError::native("Listen for model setup cancellation", e)),
        result = input.recv() => result.transpose()
            .map_err(|e| ConnectError::native("Read model setup choice", e)),
    };
    input
        .check_cleanup()
        .map_err(|e| ConnectError::native("Restore model setup terminal", e))?;
    result.map(|value| value.filter(|value| value.trim() != "/quit"))
}

/// Select an existing target or create the first direct API-key target, then
/// authenticate through its native owner. Existing browser OAuth and Copilot
/// device sign-in retain their configured targets. Ready defaults remain
/// automatic unless choose_model is true. The original model override remains
/// authoritative across selection. A successful return retains the resolved
/// model/provider and native optional auth reference for later continuation;
/// it does not retain credentials or replace native currentness checks.
///
/// Output is a private terminal: never forward it to a model, history or log.
/// Open the printed URL only in a user-controlled browser unavailable to agents.
/// No session is created here. Standalone setup uses its existing stage's auth
/// pair for every method, including non-OAuth readiness. Ordinary startup can
/// open its one retained realm bundle for OAuth; env/static readiness does not
/// require that bundle.
///
/// Await this future through cancellation. Released native callback wait owns
/// its transport until its original 120-second deadline; cancellation expires
/// exact OAuth state then waits for that cleanup. An exchange already started
/// is also awaited and its actual result reported before returning.
pub async fn present_provider_setup<F, W>(
    startup: &mut SessionStartup,
    model: Option<String>,
    choose_model: bool,
    input: &mut TerminalInput,
    output: &mut W,
    interrupt: F,
) -> Result<Option<CreateSessionModelResolutionRequest>, ConnectError>
where
    F: Future<Output = io::Result<()>>,
    W: Write,
{
    let selected =
        present_provider_setup_inner(startup, model, choose_model, input, output, interrupt)
            .await?;
    selected
        .map(|request| retain_model_selection(&startup.config, request))
        .transpose()
}

// Keep the choice used by initial setup explicit when the same conversation
// reopens after finalization. Refreshing service configuration must not select
// a new default model/provider. An absent native auth reference stays absent;
// credential candidates, currentness and token contents remain native-owned.
fn retain_model_selection(
    config: &meerkat_core::Config,
    mut request: CreateSessionModelResolutionRequest,
) -> Result<CreateSessionModelResolutionRequest, ConnectError> {
    let selected = resolve_create_session_model(config, request.clone())
        .map_err(|error| ConnectError::native("Retain setup model selection", error))?;
    request.model = Some(selected.model);
    request.provider = Some(selected.provider);
    request.auth_binding = selected.auth_binding;
    Ok(request)
}

async fn present_provider_setup_inner<F, W>(
    startup: &mut SessionStartup,
    model: Option<String>,
    choose_model: bool,
    input: &mut TerminalInput,
    output: &mut W,
    interrupt: F,
) -> Result<Option<CreateSessionModelResolutionRequest>, ConnectError>
where
    F: Future<Output = io::Result<()>>,
    W: Write,
{
    tokio::pin!(interrupt);
    // A faulted backend must not be presented as a missing credential or replaced.
    startup
        .factory
        .resolution_provider_auth_persistence()
        .map_err(|e| ConnectError::provider_setup("Open native provider credentials", e))?;
    let original = CreateSessionModelResolutionRequest {
        model: model.clone(),
        ..Default::default()
    };
    let resolution = resolve_create_session_model(&startup.config, original.clone())
        .map_err(|e| ConnectError::native("Resolve native model selection", e))?;
    if !choose_model {
        let candidates = resolve_auth_binding_candidates_for_provider(
            &startup.config,
            resolution.provider,
            resolution.auth_binding.as_ref(),
            Some(&startup.storage.locator.realm),
            true,
        )
        .map_err(|e| ConnectError::native("Resolve native model candidates", e))?;
        let mut first_error = None;
        let mut not_ready = false;
        let mut known_oauth_signin_required = false;
        for candidate in candidates {
            match observe(startup, &candidate).await {
                Ok(Observation::Credential(CredentialReadiness::MaterializedAtOpen))
                    if known_oauth_signin_required =>
                {
                    // An opaque fallback does not erase an observed OAuth
                    // sign-in requirement. Actual readiness can still admit open.
                }
                Ok(observation) if observation.admits_open() => return Ok(Some(original)),
                Ok(observation) => {
                    not_ready = true;
                    known_oauth_signin_required |= matches!(observation, Observation::OAuth(_));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        // Preserve a real observation fault when no native not-ready result
        // exists. Candidate ordering and final client resolution stay native.
        if !not_ready && let Some(error) = first_error {
            return Err(error);
        }
        writeln!(
            output,
            "The selected native model has no ready credential observation."
        )
        .map_err(|e| ConnectError::native("Write model setup status", e))?;
    }
    let choices = configured_choices(startup)?;
    if choices.is_empty() {
        return first_target::present(startup, original, input, output, interrupt.as_mut()).await;
    }
    input
        .require_terminal()
        .map_err(|e| ConnectError::native("Require protected model setup terminal", e))?;
    writeln!(output, "Choose an existing model target. This changes only this setup conversation; native configuration is not rewritten.")
        .map_err(|e| ConnectError::native("Write model target choices", e))?;
    for (index, reference) in choices.iter().enumerate() {
        writeln!(
            output,
            "{}. Owner realm: {}; binding: {}",
            index + 1,
            review_text(reference.realm.as_str()),
            review_text(reference.binding.as_str())
        )
        .map_err(|e| ConnectError::native("Write model target choices", e))?;
    }
    loop {
        let Some(choice) = line(
            input,
            output,
            "Model target (number, or /quit): ",
            interrupt.as_mut(),
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(reference) = choice
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| choices.get(index))
        else {
            writeln!(output, "Enter one of the listed target numbers, or /quit.")
                .map_err(|e| ConnectError::native("Write model target guidance", e))?;
            continue;
        };
        let request = CreateSessionModelResolutionRequest {
            model: model.clone(),
            auth_binding: Some(reference.clone()),
            ..Default::default()
        };
        let selected = match resolve_create_session_model(&startup.config, request.clone()) {
            Ok(selected) => selected,
            Err(CreateSessionModelResolutionError::ProviderModelMismatch(_)) => {
                writeln!(output, "That target cannot use the requested model. Choose another listed target, or /quit. The requested model is unchanged.")
                    .map_err(|e| ConnectError::native("Write model target guidance", e))?;
                continue;
            }
            Err(error) => {
                return Err(ConnectError::native(
                    "Resolve chosen native model target",
                    error,
                ));
            }
        };
        let target = resolve_explicit_auth_binding_target(&startup.config, reference)
            .map_err(|e| ConnectError::native("Resolve chosen native credential owner", e))?;
        let observation = observe(startup, &target).await?;
        writeln!(
            output,
            "Model: {}; provider: {}; owner realm: {}; binding: {}; status: {}",
            review_text(&selected.model),
            selected.provider.as_str(),
            review_text(target.auth_binding.realm.as_str()),
            review_text(target.auth_binding.binding.as_str()),
            observation.label()
        )
        .map_err(|e| ConnectError::native("Write chosen model target", e))?;
        if managed_api_key(&target) {
            writeln!(output, "Store or replace API key for owner realm: {}; binding: {}; profile: {}. Other sessions using this credential owner will use the stored key.",
                review_text(target.auth_binding.realm.as_str()), review_text(target.auth_binding.binding.as_str()),
                review_text(target.auth_profile.id.as_str()))
                .map_err(|e| ConnectError::native("Write API-key owner review", e))?;
            if let Some(account) = target.credential_identity.account() {
                writeln!(
                    output,
                    "Shared credential account: {}",
                    review_text(account.as_str())
                )
                .map_err(|e| ConnectError::native("Write API-key account review", e))?;
            }

            loop {
                let prompt = if observation.admits_open() {
                    "Type 'enter-key' to store or replace this owner's API key, 'use' for native open, 'back', or /quit: "
                } else {
                    "Type 'enter-key' to store or replace this owner's API key, 'back', or /quit: "
                };
                let Some(action) = line(input, output, prompt, interrupt.as_mut()).await? else {
                    return Ok(None);
                };
                match action.trim() {
                    "back" => break,
                    "use" if observation.admits_open() => return Ok(Some(request)),
                    "enter-key" => {
                        return if api_key_login(startup, &target, input, output, interrupt.as_mut())
                            .await?
                        {
                            Ok(Some(request))
                        } else {
                            Ok(None)
                        };
                    }
                    _ => {}
                }
            }
            continue;
        }
        let Some(identity) = oauth_identity(&target) else {
            if observation.admits_open() {
                return Ok(Some(request));
            }
            writeln!(output, "This target's native credential source is not ready. Browser sign-in cannot supply an API key or replace that source. Configure its credential through the native owner, or choose another existing target.")
                .map_err(|e| ConnectError::native("Write credential source guidance", e))?;
            continue;
        };
        if !identity.supports_browser_flow() && identity != OAuthProviderIdentity::GitHubCopilot {
            if observation.admits_open() {
                return Ok(Some(request));
            }
            writeln!(output, "This target uses device sign-in, which Connect does not yet present. Use its native sign-in owner, or choose another target.")
                .map_err(|e| ConnectError::native("Write device sign-in guidance", e))?;
            continue;
        }
        let prompt = if observation.admits_open() {
            "Type 'signin' to re-authenticate this exact owner, 'use' for native open, 'back', or /quit: "
        } else {
            "Type 'signin' to authenticate this exact owner, 'back', or /quit: "
        };
        loop {
            let Some(action) = line(input, output, prompt, interrupt.as_mut()).await? else {
                return Ok(None);
            };
            match action.trim() {
                "back" => break,
                "use" if observation.admits_open() => return Ok(Some(request)),
                "signin" => {
                    let completed = if identity == OAuthProviderIdentity::GitHubCopilot {
                        device_login(startup, &target, output, interrupt.as_mut()).await?
                    } else {
                        browser_login(startup, &target, identity, output, interrupt.as_mut())
                            .await?
                    };
                    return if completed {
                        Ok(Some(request))
                    } else {
                        Ok(None)
                    };
                }
                _ => {}
            }
        }
    }
}

fn managed_api_key(target: &ResolvedConnectionTarget) -> bool {
    !target.auth_binding.is_env_default()
        && matches!(
            target.auth_profile.source,
            CredentialSourceSpec::ManagedStore
        )
        && NormalizedAuthMethod::from_auth_profile(&target.auth_profile)
            .and_then(NormalizedAuthMethod::persisted_auth_mode)
            == Some(PersistedAuthMode::ApiKey)
}

#[cfg(test)]
mod api_key_tests {
    #[test]
    fn credential_save_errors_preserve_native_refusal_and_redact_private_details() {
        use meerkat_core::auth::token_store::{
            CredentialMutationError as E, CredentialSlotRefusal as R,
        };
        for error in [
            E::StalePreparation,
            E::Cancelled,
            E::SlotRefused(R::Occupied),
            E::SlotRefused(R::ModeMismatch),
            E::SlotRefused(R::AccountMismatch),
            E::SlotRefused(R::ContextMismatch),
            E::SlotRefused(R::UnverifiedConnectorPublication),
        ] {
            let expected = error.clone();
            let mapped = credential_save_error(error);
            let ConnectError::ProviderSetup { operation, source } = &mapped else {
                panic!("native credential refusal lost its typed source");
            };
            assert_eq!(*operation, "Store native API-key credential");
            let actual = source.downcast_ref::<E>().unwrap();
            assert_eq!(
                std::mem::discriminant(actual),
                std::mem::discriminant(&expected)
            );
            assert_eq!(actual.to_string(), expected.to_string());
            if let E::SlotRefused(expected) = expected {
                assert!(matches!(actual, E::SlotRefused(actual) if *actual == expected));
            }
        }
        for error in [
            E::Operation("private-credential-canary".into()),
            E::TokenStore("private-credential-canary".into()),
            E::AuthLifecycle("private-credential-canary".into()),
            E::LockFailed("private-credential-canary".into()),
        ] {
            let expected = std::mem::discriminant(&error);
            let mapped = credential_save_error(error);
            assert!(!format!("{mapped:?}").contains("private-credential-canary"));
            let ConnectError::ProviderSetup { source, .. } = mapped else {
                panic!("native credential failure lost its typed source");
            };
            let actual = source.downcast_ref::<E>().unwrap();
            assert_eq!(std::mem::discriminant(actual), expected);
            assert!(actual.to_string().contains("details withheld"));
        }
        use meerkat_core::auth::token_store::TokenStoreRefusal as T;
        for refusal in [
            T::Denied,
            T::InteractionRequired,
            T::Pending,
            T::Unavailable,
            T::NotObserved,
            T::Unsupported,
        ] {
            let mapped = credential_save_error(E::CredentialAccess {
                refusal,
                detail: "private-credential-canary".into(),
            });
            assert!(!format!("{mapped:?}").contains("private-credential-canary"));
            let ConnectError::ProviderSetup { source, .. } = mapped else {
                panic!("native credential access refusal lost its typed source");
            };
            assert!(matches!(source.downcast_ref::<E>(),
                Some(E::CredentialAccess { refusal: actual, detail })
                    if *actual == refusal && detail == "details withheld"));
        }
    }

    #[test]
    fn private_native_expiry_keeps_error_and_cleanup_separate_from_display() {
        let mut show = |_| Ok(());
        for outcome in [
            HostAuthDevicePoll::Pending,
            HostAuthDevicePoll::AccessDenied,
            HostAuthDevicePoll::Expired,
        ] {
            let mut presentation = SignInPresentation::new(&mut show);
            presentation.observe_device_poll(&outcome);
            let result = Err(combined(
                ConnectError::InvalidInput("native refusal".into()),
                Err(ConnectError::InvalidInput("cleanup failure".into())),
            ));
            let original = result.as_ref().unwrap_err().to_string();
            assert_eq!(
                presentation.disposition(&result),
                if matches!(outcome, HostAuthDevicePoll::Expired) {
                    SignInDisposition::Expired
                } else {
                    SignInDisposition::Refused
                }
            );
            assert_eq!(result.as_ref().unwrap_err().to_string(), original);
            assert!(original.contains("cleanup failure"));
        }
        for error in [OAuthError::UserDenied, OAuthError::Timeout] {
            let expected = if matches!(error, OAuthError::Timeout) {
                SignInDisposition::Expired
            } else {
                SignInDisposition::Refused
            };
            let mut presentation = SignInPresentation::new(&mut show);
            presentation.observe_callback_error(&error);
            let result = Err(ConnectError::provider_setup(
                "Await native sign-in callback",
                error,
            ));
            assert_eq!(presentation.disposition(&result), expected);
            assert!(result.is_err());
        }
        let mut presentation = SignInPresentation::new(&mut show);
        assert!(!device_window_elapsed(
            std::time::Instant::now() + Duration::from_secs(60),
            &mut Some(&mut presentation)
        ));
        assert_eq!(
            presentation.disposition(&Ok(false)),
            SignInDisposition::Refused
        );
        assert_eq!(
            presentation.disposition(&Ok(true)),
            SignInDisposition::Completed
        );
        assert!(device_window_elapsed(
            std::time::Instant::now(),
            &mut Some(&mut presentation)
        ));
        assert_eq!(
            presentation.disposition(&Ok(false)),
            SignInDisposition::Expired
        );
        // Terminal calls have no private display, but use the same native deadline.
        assert!(device_window_elapsed(std::time::Instant::now(), &mut None));
    }

    use super::*;
    use meerkat_auth_core::auth_store::{
        FileLockCoordinator, FileTokenStore, PersistedTokens, ProviderAuthPersistence, TokenKey,
        TokenStore, TokenStoreError,
    };
    use nix::{
        pty::openpty,
        sys::termios::{LocalFlags, tcgetattr},
        unistd::write,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::{Semaphore, oneshot};

    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl Output {
        fn contains(&self, text: &str) -> bool {
            String::from_utf8_lossy(&self.0.lock().unwrap()).contains(text)
        }
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    async fn shown(output: &Output, text: &str) {
        while !output.contains(text) {
            tokio::task::yield_now().await;
        }
    }
    struct HeldStore {
        inner: Arc<FileTokenStore>,
        entered: Semaphore,
        release: Semaphore,
        saves: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl TokenStore for HeldStore {
        async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            self.inner.load(key).await
        }
        async fn save(
            &self,
            key: &TokenKey,
            value: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            self.inner.save(key, value).await
        }
        async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
            self.inner.clear(key).await
        }
        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            self.inner.list().await
        }
        fn backend_name(&self) -> &'static str {
            "held-native-file"
        }
    }

    fn config() -> meerkat_core::Config {
        meerkat_core::Config::from_persisted_toml(
            r#"
[agent]
model = "gpt-6-astra"
[realm.owner]
default_binding = "configured"
[realm.owner.backend.configured]
provider = "openai"
backend_kind = "openai_api"
[realm.owner.auth.configured]
provider = "openai"
auth_method = "api_key"
source = {kind = "managed_store"}
[realm.owner.binding.configured]
backend_profile = "configured"
auth_profile = "configured"
[realm.child]
parent = "owner"
"#,
        )
        .unwrap()
        .0
    }
    fn target(config: &meerkat_core::Config) -> ResolvedConnectionTarget {
        resolve_explicit_auth_binding_target(
            config,
            &AuthBindingRef {
                realm: meerkat_core::RealmId::parse("owner").unwrap(),
                binding: BindingId::parse("configured").unwrap(),
                profile: None,
                origin: BindingOrigin::Configured,
            },
        )
        .unwrap()
    }

    #[test]
    fn api_key_admission_refuses_inherited_write_and_changed_identity() {
        let config = config();
        let exact = target(&config);
        validate_api_key_target(&config, &exact).unwrap();
        let mut wrong = exact.clone();
        wrong.auth_binding.realm = meerkat_core::RealmId::parse("child").unwrap();
        assert!(validate_api_key_target(&config, &wrong).is_err());
        let mut wrong = exact.clone();
        wrong.credential_identity =
            meerkat_core::AuthCredentialIdentity::from_auth_binding(&AuthBindingRef {
                realm: meerkat_core::RealmId::parse("elsewhere").unwrap(),
                ..exact.auth_binding.clone()
            });
        assert!(validate_api_key_target(&config, &wrong).is_err());
        for source in [
            CredentialSourceSpec::Env {
                env: "UNREAD".into(),
                fallback: vec![],
            },
            CredentialSourceSpec::PlatformDefault,
        ] {
            let mut wrong = exact.clone();
            wrong.auth_profile.source = source;
            assert!(validate_api_key_target(&config, &wrong).is_err());
        }
        let mut wrong = exact;
        wrong.auth_profile.auth_method = "chatgpt_oauth".into();
        assert!(validate_api_key_target(&config, &wrong).is_err());
    }

    #[tokio::test]
    async fn entered_native_api_key_save_is_joined_after_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let config = config();
        let target = target(&config);
        let key = TokenKey::from_credential_identity(&target.credential_identity);
        let inner = Arc::new(FileTokenStore::new(root.join("credentials")));
        let held = Arc::new(HeldStore {
            inner: inner.clone(),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            saves: AtomicUsize::new(0),
        });
        let persistence = ProviderAuthPersistence::new(
            held.clone(),
            Arc::new(FileLockCoordinator::new(root.join("locks"))),
        );
        let storage = crate::RealmOptions {
            context_root: root.into(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("owner")
        .unwrap();
        let factory = meerkat::AgentFactory::new(root.join("state/owner"))
            .with_provider_auth_persistence(persistence);
        let mut startup = SessionStartup {
            storage,
            config,
            factory,
            persistence: None,
            connector_auth: None,
            standalone: None,
        };
        let pair = openpty(None, None).unwrap();
        let observer = pair.slave.try_clone().unwrap();
        let original = tcgetattr(&observer).unwrap();
        let mut input = TerminalInput::from_owned_fd(pair.slave);
        let mut output = Output::default();
        let observed = output.clone();
        let (send, receive) = oneshot::channel();
        let interrupt = async {
            receive
                .await
                .map_err(|_| io::Error::other("test signal closed"))
        };
        tokio::pin!(interrupt);
        let human = async {
            shown(&observed, "Enter the API key (hidden)").await;
            assert!(
                !tcgetattr(&observer)
                    .unwrap()
                    .local_flags
                    .contains(LocalFlags::ECHO)
            );
            write(&pair.master, b"synthetic-held-native-key\n").unwrap();
            held.entered.acquire().await.unwrap().forget();
            send.send(()).unwrap();
            shown(&observed, "Waiting for native credential storage to settle").await;
            assert!(inner.load(&key).await.unwrap().is_none());
            assert_eq!(held.saves.load(Ordering::SeqCst), 1);
            held.release.add_permits(1);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                api_key_login(
                    &mut startup,
                    &target,
                    &mut input,
                    &mut output,
                    interrupt.as_mut()
                ),
                human
            )
        })
        .await
        .expect("held native save must settle under fixture bound");
        assert!(
            !result.unwrap(),
            "cancelled save must not continue to session creation"
        );
        assert_eq!(held.saves.load(Ordering::SeqCst), 1);
        let stored = inner.load(&key).await.unwrap().unwrap();
        assert!(stored.primary_secret.as_deref() == Some("synthetic-held-native-key"));
        assert!(observed.contains("API key stored by the native credential owner."));
        assert!(!observed.contains("synthetic-held-native-key"));
        assert_eq!(tcgetattr(&observer).unwrap(), original);
        input.check_cleanup().unwrap();
    }

    #[tokio::test]
    async fn private_api_key_save_preserves_exact_value_and_joined_cancellation() {
        for before_entry in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let config = config();
            let target = target(&config);
            let key = TokenKey::from_credential_identity(&target.credential_identity);
            let inner = Arc::new(FileTokenStore::new(root.join("credentials")));
            let held = Arc::new(HeldStore {
                inner: inner.clone(),
                entered: Semaphore::new(0),
                release: Semaphore::new(0),
                saves: AtomicUsize::new(0),
            });
            let persistence = ProviderAuthPersistence::new(
                held.clone(),
                Arc::new(FileLockCoordinator::new(root.join("locks"))),
            );
            let storage = crate::RealmOptions {
                context_root: root.into(),
                state_root: Some(root.join("state")),
                user_config_root: Some(root.join("home")),
            }
            .resolve("owner")
            .unwrap();
            let mut startup = SessionStartup {
                storage,
                config,
                factory: meerkat::AgentFactory::new(root.join("state/owner"))
                    .with_provider_auth_persistence(persistence.clone()),
                persistence: None,
                connector_auth: None,
                standalone: None,
            };
            let (cancel, receive) = oneshot::channel();
            let interrupt = async {
                receive
                    .await
                    .map_err(|_| io::Error::other("test signal closed"))
            };
            tokio::pin!(interrupt);
            let value = "  synthetic-private-native-key-☃ /quit  ";
            let mut output = Output::default();
            let observed = output.clone();
            if before_entry {
                cancel.send(()).unwrap();
                assert!(
                    !save_api_key(
                        &mut startup,
                        &target,
                        persistence,
                        PersistedTokens::api_key(value),
                        &mut output,
                        interrupt.as_mut()
                    )
                    .await
                    .unwrap()
                );
                assert_eq!(held.saves.load(Ordering::SeqCst), 0);
                assert!(inner.load(&key).await.unwrap().is_none());
            } else {
                let owner = async {
                    held.entered.acquire().await.unwrap().forget();
                    cancel.send(()).unwrap();
                    shown(&observed, "Waiting for native credential storage to settle").await;
                    assert!(inner.load(&key).await.unwrap().is_none());
                    assert_eq!(held.saves.load(Ordering::SeqCst), 1);
                    held.release.add_permits(1);
                };
                let (saved, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                    tokio::join!(
                        save_api_key(
                            &mut startup,
                            &target,
                            persistence,
                            PersistedTokens::api_key(value),
                            &mut output,
                            interrupt.as_mut()
                        ),
                        owner
                    )
                })
                .await
                .unwrap();
                assert!(
                    !saved.unwrap(),
                    "joined cancellation cannot continue initialization"
                );
                assert_eq!(held.saves.load(Ordering::SeqCst), 1);
                assert_eq!(
                    inner
                        .load(&key)
                        .await
                        .unwrap()
                        .unwrap()
                        .primary_secret
                        .as_deref(),
                    Some(value)
                );
            }
            assert!(!observed.contains(value));
        }
    }
    // Exercise the consumer over a real stage and native file credentials.
    // This never starts the declared MCP command or a provider network request.
    struct ApproveStagedHelper {
        bytes: Vec<u8>,
        input: tokio::sync::mpsc::Sender<io::Result<String>>,
        decisions: usize,
    }

    impl Write for ApproveStagedHelper {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.decisions == 0 {
                let text = std::str::from_utf8(&self.bytes).unwrap();
                if let Some((_, decision)) = text.split_once("Type 'approve ") {
                    assert!(text.contains(
                        "Allow the supplied setup helper to prepare choices in this realm?"
                    ));
                    let (id, _) = decision.split_once('\'').unwrap();
                    self.input.try_send(Ok(format!("approve {id}"))).unwrap();
                    self.decisions += 1;
                }
            }
            Ok(())
        }
    }

    fn assert_ended_stage(error: ConnectError) {
        let ConnectError::Native { operation, source } = error else {
            panic!("the native stage refusal must remain the primary error");
        };
        assert_eq!(operation, "Read staged setup authentication authority");
        assert!(matches!(
            source.downcast_ref::<meerkat::governed_realm::staged::StagedRealmError>(),
            Some(meerkat::governed_realm::staged::StagedRealmError::Ended)
        ));
    }

    #[tokio::test]
    async fn staged_api_key_save_and_readiness_share_owner_through_cancellation_and_end() {
        for cancel_after_entry in [false, true] {
            tokio::time::timeout(
                Duration::from_secs(30),
                staged_api_key_scenario(cancel_after_entry),
            )
            .await
            .expect("the real staged consumer must settle without a second realm owner");
        }
    }

    async fn staged_api_key_scenario(cancel_after_entry: bool) {
        use meerkat::governed_realm::{
            staged::stage_stock_realm_persistence_as, stock::issuance::DEFAULT_STOCK_AGENT,
        };
        use meerkat_core::{
            ConfigStore, FileConfigStore,
            handles::{AuthLeasePhase, LeaseKey},
        };

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let storage = crate::RealmOptions {
            context_root: root.to_owned(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("owner")
        .unwrap();
        let config = config();
        let target = target(&config);
        let key = TokenKey::from_credential_identity(&target.credential_identity);
        let lease_key = LeaseKey::from_credential_identity(&target.credential_identity);
        let config_path = crate::realm::config_source(&storage)
            .unwrap()
            .config_doc_path(&storage.locator.realm);
        FileConfigStore::new(config_path.clone(), meerkat_models::canonical())
            .set(config)
            .await
            .unwrap();
        let artifacts = root.join("artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        let connection = crate::setup::PreparedSetupConnection::from_preparer_output(
            br#"{"mcpServers":{"toolkit-setup":{"command":"/bin/true","args":["staged-auth-consumer"]}}}"#,
        ).unwrap();
        let prepared = crate::resources::PreparedResources {
            connection,
            artifact_directory: artifacts.clone(),
        };
        let session = meerkat::Session::new();
        let (approvals, mut presenter) =
            crate::approval::LocalApprovals::create_paired_host(&artifacts).unwrap();
        approvals.bind_session(session.id()).unwrap();
        let (input, receive) = tokio::sync::mpsc::channel(1);
        let mut input = (
            ApproveStagedHelper {
                bytes: Vec::new(),
                input,
                decisions: 0,
            },
            TerminalInput::scripted(receive),
        );
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        let mut startup = SessionStartup::prepare_standalone(
            storage.clone(),
            session,
            prepared,
            &approvals,
            &mut presenter,
            &mut input.1,
            &mut input.0,
            interrupt.as_mut(),
        )
        .await
        .unwrap()
        .expect("the actual reviewed pre-stage path must enter one stage");
        approvals.retire();
        assert_eq!(input.0.decisions, 1);
        assert!(startup.persistence.is_none());
        assert!(startup.standalone.is_some());
        let config_before = std::fs::read(&config_path).unwrap();
        let owner_path = meerkat_store::grant_owner_store_path(
            &meerkat_store::realm_paths_in(&storage.locator.state_root, "owner").root,
        );
        let owner_before = std::fs::read(&owner_path).unwrap();

        // The fixture holds one actual FileTokenStore save. It does not replace
        // runtime authority, lifecycle publication, refresh coordination or stage.
        let inner = Arc::new(FileTokenStore::new(root.join("credentials")));
        let held = Arc::new(HeldStore {
            inner: inner.clone(),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            saves: AtomicUsize::new(0),
        });
        let persistence = ProviderAuthPersistence::new(
            held.clone(),
            Arc::new(FileLockCoordinator::new(root.join("locks"))),
        );
        startup.factory = startup
            .factory
            .clone()
            .with_provider_auth_persistence(persistence.clone());
        let pair = startup.provider_auth_runtime_authority().await.unwrap();
        let lease = pair.generated_auth_lease_handle();
        let native_pair = startup
            .standalone
            .as_ref()
            .unwrap()
            .stage
            .provider_auth_runtime_authority()
            .unwrap();
        assert!(lease.same_owner(&native_pair.generated_auth_lease_handle()));
        assert!(std::ptr::addr_eq(
            Arc::as_ptr(&pair.oauth_flow_authority()),
            Arc::as_ptr(&native_pair.oauth_flow_authority())
        ));
        drop(native_pair);
        assert!(inner.load(&key).await.unwrap().is_none());
        assert!(!lease.snapshot(&lease_key).credential_present);
        assert!(matches!(
            observe(&mut startup, &target).await.unwrap(),
            Observation::Credential(CredentialReadiness::Missing)
        ));

        let value = "synthetic-staged-native-key";
        let (cancel, receive) = oneshot::channel::<()>();
        let interrupt = async {
            receive
                .await
                .map_err(|_| io::Error::other("test signal closed"))
        };
        tokio::pin!(interrupt);
        let mut output = Output::default();
        let observed = output.clone();
        let continued = {
            let saving = save_api_key(
                &mut startup,
                &target,
                persistence.clone(),
                PersistedTokens::api_key(value),
                &mut output,
                interrupt.as_mut(),
            );
            tokio::pin!(saving);
            // The same existing native save must enter and remain pending. Do not
            // release it until cancellation has been observed when requested.
            tokio::select! { biased;
                result = saving.as_mut() => panic!("save settled before its held native store: {result:?}"),
                permit = held.entered.acquire() => permit.unwrap().forget(),
            }
            assert_eq!(held.saves.load(Ordering::SeqCst), 1);
            assert!(inner.load(&key).await.unwrap().is_none());
            // Native login publishes lifecycle before its guarded durable
            // save and compensates on failure. This is still a pending save.
            assert!(lease.snapshot(&lease_key).credential_present);
            if cancel_after_entry {
                cancel.send(()).unwrap();
                tokio::select! { biased;
                    result = saving.as_mut() => panic!("cancellation dropped an entered native save: {result:?}"),
                    () = shown(&observed, "Waiting for native credential storage to settle") => {},
                }
                assert!(inner.load(&key).await.unwrap().is_none());
            }
            held.release.add_permits(1);
            saving.await.unwrap()
        };
        assert_eq!(
            continued, !cancel_after_entry,
            "an entered cancelled save must settle without continuing startup"
        );
        assert_eq!(held.saves.load(Ordering::SeqCst), 1);
        assert_eq!(
            inner
                .load(&key)
                .await
                .unwrap()
                .unwrap()
                .primary_secret
                .as_deref(),
            Some(value)
        );
        assert!(
            lease.snapshot(&lease_key).credential_present,
            "native credential publication must reach the stage's actual auth owner"
        );
        assert_eq!(
            lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Valid)
        );
        assert!(matches!(
            observe(&mut startup, &target).await.unwrap(),
            Observation::Credential(CredentialReadiness::Ready)
        ));
        assert!(
            startup.persistence.is_none(),
            "no second persistence bundle was opened"
        );
        assert!(observed.contains("API key stored by the native credential owner."));
        assert!(!observed.contains(value));

        // A nonempty file alone cannot override the actual stage's lifecycle.
        // Removing the staged lease from observe() makes this assertion fail.
        lease.mark_reauth_required(&lease_key).unwrap();
        assert_eq!(
            inner
                .load(&key)
                .await
                .unwrap()
                .unwrap()
                .primary_secret
                .as_deref(),
            Some(value)
        );
        assert!(matches!(
            observe(&mut startup, &target).await.unwrap(),
            Observation::Credential(CredentialReadiness::NeedsReauth)
        ));
        let request = CreateSessionModelResolutionRequest {
            model: Some("gpt-6-astra".into()),
            auth_binding: Some(target.auth_binding.clone()),
            ..Default::default()
        };
        assert!(!ready_for_open(&mut startup, &request).await.unwrap());

        let mut abort_output = Vec::new();
        startup.abort_standalone(&mut abort_output).await.unwrap();
        assert!(
            String::from_utf8(abort_output)
                .unwrap()
                .contains("a native handle still retains this realm owner"),
            "the held auth pair must not be reported as released custody"
        );
        assert_ended_stage(
            startup
                .provider_auth_runtime_authority()
                .await
                .err()
                .expect("ended stage must refuse"),
        );
        assert_ended_stage(
            observe(&mut startup, &target)
                .await
                .err()
                .expect("ended readiness must refuse"),
        );
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        assert_ended_stage(
            save_api_key(
                &mut startup,
                &target,
                persistence,
                PersistedTokens::api_key("must-not-be-written"),
                &mut io::sink(),
                interrupt.as_mut(),
            )
            .await
            .unwrap_err(),
        );
        assert_eq!(
            held.saves.load(Ordering::SeqCst),
            1,
            "ended authority cannot start another save"
        );
        assert!(
            startup.persistence.is_none(),
            "Ended cannot choose an ordinary-owner fallback"
        );
        assert_eq!(std::fs::read(&config_path).unwrap(), config_before);
        assert_eq!(
            std::fs::read(&owner_path).unwrap(),
            owner_before,
            "provider setup and its abort must not issue another stock grant"
        );
        assert!(
            stage_stock_realm_persistence_as(
                &meerkat::DiskStorageProvider,
                &storage.locator.state_root,
                "owner",
                None,
                None,
                Some(storage.layout.clone()),
                DEFAULT_STOCK_AGENT
            )
            .await
            .is_err(),
            "the retained auth pair still holds this exact realm"
        );
        drop(pair);
        drop(lease);
        drop(startup);
        // Release may complete on an owned native task. Reopen only this same
        // realm, under the outer bound, and abort the resulting stage promptly.
        loop {
            match stage_stock_realm_persistence_as(
                &meerkat::DiskStorageProvider,
                &storage.locator.state_root,
                "owner",
                None,
                None,
                Some(storage.layout.clone()),
                DEFAULT_STOCK_AGENT,
            )
            .await
            {
                Ok(mut reopened) => {
                    assert_eq!(
                        reopened.abort().await.unwrap().custody,
                        meerkat_runtime::StagedCustodyRelease::Released
                    );
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }
}

fn validate_api_key_target(
    config: &meerkat_core::Config,
    target: &ResolvedConnectionTarget,
) -> Result<(), ConnectError> {
    if !managed_api_key(target) {
        return Err(ConnectError::InvalidInput(
            "The selected target does not accept managed API-key entry".into(),
        ));
    }
    meerkat_core::connection::resolve_write_owner(
        config,
        &target.auth_binding.realm,
        &target.auth_binding.binding,
    )
    .map_err(|e| ConnectError::provider_setup("Validate native credential write owner", e))?;
    let resolved = resolve_explicit_auth_binding_target(config, &target.auth_binding)
        .map_err(|e| ConnectError::provider_setup("Validate native credential identity", e))?;
    if resolved != *target {
        return Err(ConnectError::InvalidInput(
            "The selected credential identity no longer matches its native target".into(),
        ));
    }
    Ok(())
}

async fn api_key_login<I, W>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    input: &mut TerminalInput,
    output: &mut W,
    mut interrupt: Pin<&mut I>,
) -> Result<bool, ConnectError>
where
    I: Future<Output = io::Result<()>>,
    W: Write,
{
    api_key_login_reviewed(startup, target, None, input, output, interrupt.as_mut()).await
}

async fn api_key_login_reviewed<I, W>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    review: Option<&first_target::TargetReview>,
    input: &mut TerminalInput,
    output: &mut W,
    mut interrupt: Pin<&mut I>,
) -> Result<bool, ConnectError>
where
    I: Future<Output = io::Result<()>>,
    W: Write,
{
    if let Some(review) = review {
        review.refresh(startup).await?;
    }
    validate_api_key_target(&startup.config, target)?;
    let persistence = startup
        .factory
        .resolution_provider_auth_persistence()
        .map_err(|e| ConnectError::provider_setup("Open native provider credentials", e))?
        .ok_or_else(|| {
            ConnectError::InvalidInput("Native managed credential storage is unavailable".into())
        })?;
    input
        .require_terminal()
        .map_err(|e| ConnectError::native("Require protected API-key terminal", e))?;
    let read = tokio::select! {
        biased;
        signal = interrupt.as_mut() => signal
            .map(|()| None)
            .map_err(|e| ConnectError::native("Listen for API-key input cancellation", e)),
        value = input.read_api_key(output) => value
            .map_err(|e| ConnectError::native("Read hidden API key", e)),
    };
    let cleanup = input
        .check_cleanup()
        .map_err(|e| ConnectError::native("Restore API-key terminal", e));
    let tokens = match read {
        Err(error) => return Err(combined(error, cleanup)),
        Ok(tokens) => {
            cleanup?;
            tokens
        }
    };
    let Some(tokens) = tokens else {
        return Ok(false);
    };
    if let Some(review) = review {
        // Human input is an await boundary. Re-resolve the actual document
        // before native credential mutation, not the retained old Config.
        review.refresh(startup).await?;
    }
    let saved = save_api_key(
        startup,
        target,
        persistence,
        tokens,
        output,
        interrupt.as_mut(),
    )
    .await?;
    if saved && let Some(review) = review {
        // The native save can also await. Its settled saved effect remains
        // true if fresh configuration now refuses this continuation.
        review.refresh(startup).await?;
        if let Some(signal) = interrupt.as_mut().now_or_never() {
            signal.map_err(|error| {
                ConnectError::native("Listen for model setup cancellation", error)
            })?;
            return Ok(false);
        }
    }
    Ok(saved)
}

async fn save_api_key<I, W>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    persistence: meerkat_core::auth::token_store::ProviderAuthPersistence,
    tokens: meerkat_core::auth::token_store::PersistedTokens,
    output: &mut W,
    mut interrupt: Pin<&mut I>,
) -> Result<bool, ConnectError>
where
    I: Future<Output = io::Result<()>>,
    W: Write,
{
    validate_api_key_target(&startup.config, target)?;
    let authority = startup.provider_auth_runtime_authority().await?;
    // Once polled, this native transaction owns serialization and compensation.
    // Keep it alive through interruption and report its actual settled result.
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal.map_err(|e| ConnectError::native("Listen for API-key save cancellation", e))?;
        return Ok(false);
    }
    let save = meerkat_auth_core::save_tokens_and_publish_lifecycle(
        persistence,
        authority.generated_auth_lease_handle(),
        target.credential_identity.clone(),
        tokens,
    );
    tokio::pin!(save);
    let (saved, interrupted) = if let Some(saved) = save.as_mut().now_or_never() {
        (saved, None)
    } else {
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => {
                let shown = writeln!(output, "Cancellation requested. Waiting for native credential storage to settle; the key may be saved.")
                    .and_then(|()| output.flush())
                    .map_err(|e| ConnectError::native("Write API-key save cancellation", e));
                let signal = signal.map_err(|e| ConnectError::native("Listen for API-key save cancellation", e));
                (save.await, Some(match signal { Err(error) => Err(combined(error, shown)), Ok(()) => shown }))
            }
            saved = save.as_mut() => (saved, None),
        }
    };
    let result = saved.map(|_| ()).map_err(credential_save_error);
    let result = match result {
        Ok(()) => writeln!(output, "API key stored by the native credential owner.")
            .and_then(|()| output.flush())
            .map_err(|e| ConnectError::native("Write API-key storage result", e)),
        Err(error) => Err(error),
    };
    let interrupted = match interrupted {
        Some(outcome) => Some(outcome),
        None => interrupt.as_mut().now_or_never().map(|signal| {
            signal.map_err(|e| ConnectError::native("Listen for API-key save cancellation", e))
        }),
    };
    match (result, interrupted) {
        (Err(error), Some(cleanup)) => Err(combined(error, cleanup)),
        (Err(error), None) => Err(error),
        (Ok(()), Some(outcome)) => {
            outcome?;
            Ok(false)
        }
        (Ok(()), None) => Ok(true),
    }
}

fn credential_save_error(
    error: meerkat_core::auth::token_store::CredentialMutationError,
) -> ConnectError {
    // Store implementations may include private material in diagnostics.
    // Preserve native refusal categories while withholding free-form details.
    use meerkat_core::auth::token_store::CredentialMutationError as E;
    let error = match error {
        E::StalePreparation => E::StalePreparation,
        E::Operation(_) => E::Operation("details withheld".into()),
        E::TokenStore(_) => E::TokenStore("details withheld".into()),
        E::AuthLifecycle(_) => E::AuthLifecycle("details withheld".into()),
        E::Cancelled => E::Cancelled,
        E::LockFailed(_) => E::LockFailed("details withheld".into()),
        E::SlotRefused(reason) => E::SlotRefused(reason),
        E::CredentialAccess { refusal, .. } => E::CredentialAccess {
            refusal,
            detail: "details withheld".into(),
        },
        E::RollbackFailed(_) => E::RollbackFailed("details withheld".into()),
    };
    ConnectError::provider_setup("Store native API-key credential", error)
}

fn combined(primary: ConnectError, cleanup: Result<(), ConnectError>) -> ConnectError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

// The optional cancellation result remains separate until the caller reports
// the settled native outcome. A Ready poll can have saved credentials even if
// the interrupt listener or cancellation-status output subsequently failed.
async fn settle_device_request<T, F, I, W>(
    request: F,
    operation: &'static str,
    output: &mut W,
    mut interrupt: Pin<&mut I>,
) -> Result<Option<(T, Option<Result<(), ConnectError>>)>, ConnectError>
where
    F: Future<Output = Result<T, meerkat::HostAuthError>>,
    I: Future<Output = io::Result<()>>,
    W: Write,
{
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal.map_err(|e| ConnectError::native("Listen for device sign-in cancellation", e))?;
        return Ok(None);
    }
    tokio::pin!(request);
    // Poll once before selecting interruption. Once begun, this exact native
    // operation is retained and awaited, never canceled by dropping its future.
    let (result, interrupted) = if let Some(result) = request.as_mut().now_or_never() {
        (result, None)
    } else {
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => {
                let printed = writeln!(output, "Cancellation requested. Waiting for the native device request to settle; credentials may already have been saved.")
                    .and_then(|()| output.flush());
                let result = request.await;
                let interruption = signal
                    .map_err(|e| ConnectError::native("Listen for device sign-in cancellation", e))
                    .and_then(|()| printed.map_err(|e| ConnectError::native("Write device cancellation status", e)));
                (result, Some(interruption))
            }
            result = request.as_mut() => (result, None),
        }
    };
    match result {
        Ok(value) => Ok(Some((value, interrupted))),
        Err(error) => Err(combined(
            ConnectError::provider_setup(operation, error),
            interrupted.unwrap_or(Ok(())),
        )),
    }
}

fn device_stopped<W: Write>(output: &mut W) -> Result<bool, ConnectError> {
    writeln!(output, "Device sign-in polling stopped. This does not revoke provider authorization. Connect stopped before PairedHost pairing or setup session creation.")
        .and_then(|()| output.flush())
        .map_err(|e| ConnectError::native("Write device sign-in cancellation result", e))?;
    Ok(false)
}

async fn device_login<F: Future<Output = io::Result<()>>, W: Write>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    output: &mut W,
    interrupt: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    device_login_presented(startup, target, output, interrupt, None).await
}

async fn device_login_presented<F: Future<Output = io::Result<()>>, W: Write>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    output: &mut W,
    mut interrupt: Pin<&mut F>,
    mut presentation: Option<&mut SignInPresentation<'_>>,
) -> Result<bool, ConnectError> {
    let authority = startup.provider_auth_runtime_authority().await?;
    // Device POSTs contain credentials. Keep native endpoints and TLS policy,
    // refuse redirects, and bound HTTP itself instead of dropping native work.
    let http = reqwest::Client::builder()
        .connect_timeout(DEVICE_CONNECT_TIMEOUT)
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ConnectError::provider_setup("Build device sign-in transport", e))?;
    let service = HostAuthService::from_factory(&startup.factory, authority.clone())
        .map_err(|e| ConnectError::provider_setup("Open native device sign-in service", e))?
        .with_http_client(http);
    let target_ref = host_target(target, OAuthProviderIdentity::GitHubCopilot);
    let Some((device, interrupted)) = settle_device_request(
        service.device_start(&startup.config, &target_ref),
        "Start native device sign-in",
        output,
        interrupt.as_mut(),
    )
    .await?
    else {
        return device_stopped(output);
    };
    if let Some(interruption) = interrupted {
        interruption?;
        return device_stopped(output);
    }
    // This is the native admitted flow's deadline, not a second expiry owner.
    let expires_at = authority
        .oauth_flow_authority()
        .verify_device_code(
            &device.device_code,
            &target.credential_identity,
            OAuthProviderIdentity::GitHubCopilot,
        )
        .map_err(|e| ConnectError::provider_setup("Read native device sign-in window", e))?
        .expires_at;
    if let Some(present) = presentation.as_deref_mut() {
        (present.show)(SignInNotice::Device {
            verification_uri: device.verification_uri.clone(),
            user_code: device.user_code.clone(),
        })
    } else { writeln!(
        output,
        "Open this private device sign-in page:\n{}\nEnter this user code: {}\nWaiting for native device authorization. Ctrl-C stops polling.",
        review_text(&device.verification_uri),
        review_text(&device.user_code),
    )
    .and_then(|()| output.flush()) }
    .map_err(|e| ConnectError::native("Write private device sign-in instructions", e))?;
    let mut interval = device.interval.max(1);
    loop {
        let next_poll = std::time::Instant::now()
            .checked_add(Duration::from_secs(interval))
            .unwrap_or(expires_at)
            .min(expires_at);
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => {
                signal.map_err(|e| ConnectError::native("Listen for device sign-in cancellation", e))?;
                return device_stopped(output);
            }
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(next_poll)) => {}
        }
        if device_window_elapsed(expires_at, &mut presentation) {
            return Err(ConnectError::InvalidInput("The native device sign-in window elapsed. No new poll was started; provider authorization was not revoked.".into()));
        }
        let Some((outcome, interrupted)) = settle_device_request(
            service.device_poll(&startup.config, &target_ref, &device.device_code),
            "Poll native device sign-in",
            output,
            interrupt.as_mut(),
        )
        .await?
        else {
            return device_stopped(output);
        };
        if let Some(present) = presentation.as_deref_mut() {
            present.observe_device_poll(&outcome);
        }
        match outcome {
            HostAuthDevicePoll::Ready(_) => {
                writeln!(output, "Native device sign-in completed for the selected owner. Credentials remain in its native store.")
                    .and_then(|()| output.flush())
                    .map_err(|e| ConnectError::native("Write native device sign-in result", e))?;
                if let Some(interruption) = interrupted {
                    interruption?;
                    return device_stopped(output);
                }
                if let Some(signal) = interrupt.as_mut().now_or_never() {
                    signal.map_err(|e| ConnectError::native("Listen for device sign-in cancellation", e))?;
                    return device_stopped(output);
                }
                return Ok(true);
            }
            HostAuthDevicePoll::AccessDenied => return Err(combined(
                ConnectError::InvalidInput("Native device sign-in was denied. No retry, PairedHost pairing or setup session was started.".into()),
                interrupted.unwrap_or(Ok(())),
            )),
            HostAuthDevicePoll::Expired => return Err(combined(
                ConnectError::InvalidInput("Native device sign-in expired. No retry, PairedHost pairing or setup session was started.".into()),
                interrupted.unwrap_or(Ok(())),
            )),
            HostAuthDevicePoll::Pending | HostAuthDevicePoll::SlowDown => {
                if let Some(interruption) = interrupted {
                    interruption?;
                    return device_stopped(output);
                }
                if matches!(outcome, HostAuthDevicePoll::SlowDown) {
                    interval = interval.saturating_add(5);
                }
            }
        }
        writeln!(output, "Waiting before the next native device poll.")
            .and_then(|()| output.flush())
            .map_err(|e| ConnectError::native("Write native device sign-in progress", e))?;
    }
}

async fn browser_login<F: Future<Output = io::Result<()>>, W: Write>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    identity: OAuthProviderIdentity,
    output: &mut W,
    interrupt: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    browser_login_presented(startup, target, identity, output, interrupt, None).await
}

async fn browser_login_presented<F: Future<Output = io::Result<()>>, W: Write>(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    identity: OAuthProviderIdentity,
    output: &mut W,
    mut interrupt: Pin<&mut F>,
    mut presentation: Option<&mut SignInPresentation<'_>>,
) -> Result<bool, ConnectError> {
    let authority = startup.provider_auth_runtime_authority().await?;
    let service = HostAuthService::from_factory(&startup.factory, authority.clone())
        .map_err(|e| ConnectError::provider_setup("Open native sign-in service", e))?;
    let target_ref = host_target(target, identity);
    // Transport presentation matches the released native CLI's provider redirect
    // profiles. PKCE, state, provider endpoints and validation remain native.
    let (path, host, ports): (&str, &str, &[u16]) = match identity {
        OAuthProviderIdentity::OpenAiChatGpt => ("/auth/callback", "localhost", &[1455, 1457]),
        OAuthProviderIdentity::AnthropicClaudeAi => ("/callback", "localhost", &[0]),
        OAuthProviderIdentity::GoogleCodeAssist => ("/callback", "127.0.0.1", &[0]),
        _ => {
            return Err(ConnectError::InvalidInput(
                "This target has no Connect browser presentation".into(),
            ));
        }
    };
    let binding = bind_loopback_callback_with_redirect(path, host, ports)
        .await
        .map_err(|e| ConnectError::provider_setup("Bind native sign-in callback", e))?;
    let redirect = binding.redirect_url.clone();
    let start = tokio::select! {
        biased;
        signal = interrupt.as_mut() => {
            let cleanup = binding.cancel().await
                .map_err(|e| ConnectError::provider_setup("Retire native sign-in callback", e));
            return match signal {
                Ok(()) => { cleanup?; Ok(false) },
                Err(error) => Err(combined(ConnectError::native("Listen for sign-in cancellation", error), cleanup)),
            };
        }
        result = service.login_start(&startup.config, &target_ref, redirect.clone()) => result,
    };
    let start = match start {
        Ok(start) => start,
        Err(error) => {
            let cleanup = binding
                .cancel()
                .await
                .map_err(|e| ConnectError::provider_setup("Retire native sign-in callback", e));
            return Err(combined(
                ConnectError::provider_setup("Start native sign-in", error),
                cleanup,
            ));
        }
    };
    let flow = authority.oauth_flow_authority();
    let expire = || {
        flow.expire(
            &start.state,
            &target.credential_identity,
            identity.into(),
            &redirect,
        )
        .map_err(|e| ConnectError::provider_setup("Expire native sign-in attempt", e))
    };
    let handle = binding.expect_state(start.state.clone());
    let printed = if let Some(present) = presentation.as_deref_mut() {
        (present.show)(SignInNotice::Browser {
            authorization_url: start.authorize_url.clone(),
        })
    } else {
        writeln!(output,
        "Open this private sign-in URL in your own browser, outside agent control:\n{}\nWaiting for the native browser callback. Ctrl-C cancels sign-in; callback cleanup may take the remaining 120-second window.",
        review_text(&start.authorize_url)).and_then(|()| output.flush())
    };
    if let Err(error) = printed {
        let expired = expire();
        let cleanup = handle
            .cancel()
            .await
            .map_err(|e| ConnectError::provider_setup("Retire native sign-in callback", e));
        return Err(combined(
            combined(
                ConnectError::native("Write private sign-in URL", error),
                expired,
            ),
            cleanup,
        ));
    }
    let wait = handle.wait(CALLBACK_WINDOW);
    tokio::pin!(wait);
    let callback = tokio::select! {
        biased;
        signal = interrupt.as_mut() => {
            let expired = expire();
            let printed = writeln!(output, "Sign-in cancelled. Native callback cleanup is pending until its existing window ends; any callback will be discarded.")
                .and_then(|()| output.flush());
            // Released wait owns physical callback cleanup. Never drop it in
            // order to claim a joined cancellation receipt.
            let cleanup = match wait.await {
                Ok(_) | Err(OAuthError::Timeout | OAuthError::UserDenied) => Ok(()),
                Err(error) => Err(ConnectError::provider_setup("Retire native sign-in callback", error)),
            };
            cleanup?;
            expired?;
            printed.map_err(|e| ConnectError::native("Write sign-in cancellation status", e))?;
            signal.map_err(|e| ConnectError::native("Listen for sign-in cancellation", e))?;
            return Ok(false);
        }
        result = wait.as_mut() => result,
    };
    let callback = match callback {
        Ok(callback) => callback,
        Err(error) => {
            if let Some(present) = presentation {
                present.observe_callback_error(&error);
            }
            return Err(combined(
                ConnectError::provider_setup("Await native sign-in callback", error),
                expire(),
            ));
        }
    };
    // Poll cancellation before starting the credential mutation. After the
    // exchange starts, retain and await it even if cancellation is requested.
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        expire()?;
        signal.map_err(|e| ConnectError::native("Listen for sign-in cancellation", e))?;
        return Ok(false);
    }
    let exchange = service.login_complete(
        &startup.config,
        &target_ref,
        redirect.clone(),
        callback.state,
        callback.code,
    );
    tokio::pin!(exchange);
    let (result, cancelled) = if let Some(result) = exchange.as_mut().now_or_never() {
        (result, None)
    } else {
        tokio::select! {
            biased;
            signal = interrupt.as_mut() => {
                let printed = writeln!(output, "Cancellation requested. Waiting for native credential exchange to settle; credentials may already have been saved.")
                    .and_then(|()| output.flush());
                let result = exchange.await;
                (result, Some((signal, printed)))
            }
            result = exchange.as_mut() => (result, None),
        }
    };
    result.map_err(|e| ConnectError::provider_setup("Complete native sign-in", e))?;
    writeln!(
        output,
        "Native sign-in completed for the selected owner. Credentials remain in its native store."
    )
    .and_then(|()| output.flush())
    .map_err(|e| ConnectError::native("Write native sign-in result", e))?;
    let interrupted = match cancelled {
        Some((signal, printed)) => {
            printed.map_err(|e| ConnectError::native("Write exchange cancellation status", e))?;
            Some(signal)
        }
        // The exchange or final output may have completed after cancellation
        // became ready. Never poll an interrupt already consumed above.
        None => interrupt.as_mut().now_or_never(),
    };
    if let Some(signal) = interrupted {
        signal.map_err(|e| ConnectError::native("Listen for sign-in cancellation", e))?;
        writeln!(
            output,
            "Connect stopped before PairedHost pairing or setup session creation."
        )
        .and_then(|()| output.flush())
        .map_err(|e| ConnectError::native("Write sign-in cancellation result", e))?;
        Ok(false)
    } else {
        Ok(true)
    }
}

#[cfg(test)]
mod model_selection_tests;
