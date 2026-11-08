//! Installed/private gateway process entry. The stock executable selects the
//! ordinary defaults; a trusted Rust embedding can retain an explicit issuer
//! and real human route before any native factory or workflow wrapper exists.
use crate::application_gateway::{
    InheritedOperatorTerminal, serve_local_operator, serve_local_operator_with_setup,
};
use crate::application_remote::serve_remote_operator_with_service_parent;
use meerkat_mobkit::{
    rpc_gateway::run_persistent_with_host, runtime::cross_mob_control::ControlListenAddr,
};
use meerkat_toolkit::host_lifetime::control::PrivateHostLifetimeControl;
use std::sync::Arc;
use toolkit_mobkit::private_host::{
    PrivateHostConnection, PrivateHostStartup, PrivateHostStartupIntent,
};

#[cfg(test)]
pub(crate) mod canonical_g_capture;
#[cfg(test)]
pub(crate) mod canonical_g_recovery;
#[cfg(test)]
mod consent_transfer_tests;
#[cfg(test)]
pub(crate) mod current_g_receiptful_capture;
#[cfg(test)]
pub(crate) mod current_g_receiptful_recovery;
#[cfg(all(test, unix, feature = "console"))]
pub(crate) mod historical_g_recovery;

pub struct InstalledGatewayOptions {
    /// Trusted application review requirements. This does not choose a Human,
    /// an issuer or permission and cannot be supplied by wire/Console metadata.
    pub operation_review_policy:
        Option<Arc<dyn toolkit_mobkit::installed_host::InstalledOperationReviewPolicy>>,
    pub startup: toolkit_mobkit::installed_host::NativeOwnerStartup,
    pub private_workflows: bool,
    pub hosted_mcp_redirect_base: Option<meerkat::McpRegisteredRedirectBase>,
    /// Explicit trusted process selection. No plan, CLI, RPC field or installed
    /// artifact auto-discovers or creates an issuer or Human consent route.
    pub operation_review: Option<crate::application_consent::ApplicationOperationReview>,
    /// Explicit actual Console route and issuer, paired by the installed owner.
    pub console_consent:
        Option<crate::application_gateway::console_consent::ConsoleConsentSelection>,
    pub abandoned_workflows: Option<crate::application_consent::AbandonedWorkflowRecovery>,
}
impl Default for InstalledGatewayOptions {
    fn default() -> Self {
        Self {
            operation_review_policy: None,
            startup: toolkit_mobkit::installed_host::NativeOwnerStartup::Restore,
            private_workflows: false,
            hosted_mcp_redirect_base: None,
            operation_review: None,
            console_consent: None,
            abandoned_workflows: None,
        }
    }
}

#[cfg(test)]
pub(crate) fn compose_operation_review(
    host: toolkit_mobkit::host_gateway::ToolkitGatewayHost,
    review: Option<crate::application_consent::ApplicationOperationReview>,
) -> Result<
    toolkit_mobkit::host_gateway::ToolkitGatewayHost,
    meerkat_toolkit::host_lifetime::control::HostLifetimeError,
> {
    compose_operation_review_with_owner(host, review).map(|(host, _)| host)
}

pub(crate) fn compose_operation_review_with_owner(
    host: toolkit_mobkit::host_gateway::ToolkitGatewayHost,
    review: Option<crate::application_consent::ApplicationOperationReview>,
) -> Result<
    (
        toolkit_mobkit::host_gateway::ToolkitGatewayHost,
        Option<Arc<crate::application_consent::ApplicationConsentOwner>>,
    ),
    meerkat_toolkit::host_lifetime::control::HostLifetimeError,
> {
    match review {
        Some(review) => review
            .compose_installed_with_owner(host)
            .map(|(host, owner)| (host, Some(owner))),
        None => Ok((host, None)),
    }
}

/// Run the existing installed initialization/restore and all-exit cleanup.
/// The caller transfers the real inherited private startup and terminal.
/// Preparation retains its separate `application_driver::run_with_options`.
/// Passing an issuer does not authenticate a person, grant work or admit G.
pub async fn run_with_options(
    private_startup: PrivateHostStartup,
    operator_terminal: Option<InheritedOperatorTerminal>,
    control_listen: Option<ControlListenAddr>,
    options: InstalledGatewayOptions,
) -> Result<(), String> {
    let InstalledGatewayOptions {
        operation_review_policy,
        startup,
        private_workflows,
        hosted_mcp_redirect_base,
        operation_review,
        console_consent,
        abandoned_workflows,
    } = options;
    if operation_review.is_some() && console_consent.is_some() {
        return Err("select either generic operation review or Console consent".into());
    }
    if matches!(
        private_startup.intent(),
        PrivateHostStartupIntent::PrepareApplication(_)
    ) {
        return Err("installed entry cannot consume a prepared application startup".into());
    }
    if private_workflows
        && (startup != toolkit_mobkit::installed_host::NativeOwnerStartup::Restore
            || matches!(
                private_startup.intent(),
                PrivateHostStartupIntent::ReplaceApplication(_)
            ))
    {
        return Err("installed private workflows require ordinary original-owner Restore".into());
    }
    // The existing transaction's trusted launcher selects replacement input.
    // Validate before restoring an owner; no no-key fallback or fresh setup.
    let replacement = match private_startup.intent() {
        PrivateHostStartupIntent::ReplaceApplication(selected) => {
            if startup != toolkit_mobkit::installed_host::NativeOwnerStartup::Restore {
                return Err(
                    "replacement application startup requires original-owner Restore".into(),
                );
            }
            match crate::application_update::ValidatedReplacementStartup::validate(selected) {
                Ok(selected) => Some(selected),
                Err(error) => {
                    private_startup
                        .reject(error)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Err("replacement application inputs were refused".into());
                }
            }
        }
        _ => None,
    };
    let private_startup = match &replacement {
        Some(selected) => private_startup
            .replace_with_original(selected.predecessor().clone())
            .map_err(|error| error.to_string())?,
        None => private_startup,
    };
    let mut composition = None;
    let mut acquisition_composition = None;
    let mut operator_artifacts = None;
    let (owner, _, private, operator_reviews, remote_reviews) =
        PrivateHostConnection::bootstrap_from_startup_with_controller_reviews(private_startup, |selection| {
            let composition = &mut composition;
            let acquisition_composition = &mut acquisition_composition;
            let operator_artifacts = &mut operator_artifacts;
            async move {
                *operator_artifacts = Some(selection.deployment_root.clone());
                let (owner, installed, acquisition) = match replacement {
                    Some(selected) => selected.open().await?,
                    None if private_workflows => {
                        crate::application_acquisition::open_gateway_acquisition_with_workflows(
                            selection.clone(), Arc::new(crate::application_acquisition::workflow::PrivateApplicationWorkflowLauncher::default())).await?
                    }
                    None => {
                        crate::application_acquisition::open_gateway_acquisition(
                            selection.clone(), startup).await?
                    }
                };
                *acquisition_composition = acquisition;
                let originals = installed.originals();
                *composition = Some(installed);
                Ok((owner, originals))
            }
        })
            .await
            .map_err(|error| error.to_string())?;
    let service_parent = match private.service_parent_endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
            });
        }
    };
    // The FnOnce factory above either returns both objects or bootstrap
    // fails. It cannot select the legacy ownerless constructor.
    let installed = match composition {
        Some(installed) => installed,
        None => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => "installed Toolkit composition is unavailable".into(),
                Err(error) => format!(
                    "installed Toolkit composition is unavailable; private host cleanup also failed: {error}"
                ),
            });
        }
    };
    // The existing transferred terminal owns this enrollment receiver.
    // Create it before native init so every offered factory can enroll its
    // actual selected adapter. No terminal means no local setup presenter.
    let (setup_approvals, setup_reviews) = match (&operator_terminal, &operator_artifacts) {
        (Some(terminal), Some(parent)) if acquisition_composition.is_some() => {
            let (approvals, reviews) = terminal.setup_reviews(parent.clone());
            (Some(approvals), Some(reviews))
        }
        _ => (None, None),
    };
    let class_policy = installed.prepared_tool_policy();
    let (host, consent_owner) = match installed
        .with_operation_review_policy(operation_review_policy)
        .compose(owner.clone(), private.closed_observer())
        .with_console_logins(private.console_logins())
        .and_then(|host| host.with_member_class_policy(class_policy))
        .and_then(|host| match hosted_mcp_redirect_base {
            Some(base) => host.with_hosted_mcp_auth(private.hosted_mcp_auth(), base),
            None => Ok(host),
        })
        .and_then(|host| compose_operation_review_with_owner(host, operation_review))
    {
        Ok(host) => host,
        Err(error) => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
            });
        }
    };
    #[cfg(test)]
    let console_consent = match current_g_receiptful_capture::select(&host, console_consent) {
        Ok(selection) => selection,
        Err(error) => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
            });
        }
    };
    let (host, consent_route, consent_owner) = match console_consent {
        Some(selection) => match selection.compose(host) {
            Ok((host, route, owner)) => (host, Some(route), Some(owner)),
            Err(error) => {
                let cleanup = private.finish().await;
                return Err(match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
                });
            }
        },
        None => (host, None, consent_owner),
    };
    let mut console_setup_reviews = None;
    let restored_local_round = acquisition_composition
        .as_ref()
        .and_then(|acquisition| acquisition.restored_local_round());
    let installed_workflow = acquisition_composition
        .as_ref()
        .and_then(|acquisition| acquisition.installed_workflow());
    let host = match acquisition_composition {
        Some(acquisition) => {
            let composed = operator_artifacts
                .clone()
                .ok_or(meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection)
                .and_then(|artifacts| {
                    acquisition.compose_selected_with_service_parent(
                        host,
                        owner.clone(),
                        setup_approvals,
                        artifacts,
                        Some(service_parent.clone()),
                    )
                });
            match composed {
                Ok((host, reviews)) => {
                    console_setup_reviews = reviews;
                    host
                }
                Err(error) => {
                    let cleanup = private.finish().await;
                    return Err(match cleanup {
                        Ok(()) => error.to_string(),
                        Err(cleanup) => {
                            format!("{error}; private host cleanup also failed: {cleanup}")
                        }
                    });
                }
            }
        }
        None => host,
    };
    let host = match consent_route {
        Some(route) => match host
            .augment_console_routes(Arc::new(move |resolver| route.routes(resolver)))
        {
            Ok(host) => host,
            Err(error) => {
                let cleanup = private.finish().await;
                return Err(match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
                });
            }
        },
        None => host,
    };
    #[cfg(test)]
    let host = match current_g_receiptful_capture::observe_runtime(host) {
        Ok(host) => host,
        Err(error) => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
            });
        }
    };
    let host = Arc::new(host);
    let native_host: Result<Arc<dyn meerkat_mobkit::rpc_gateway::GatewayHostComposition>, _> =
        private
            .member_classes()
            .bind(&host)
            .and_then(|()| match installed_workflow {
                Some(workflow) => {
                    workflow.bind_private(&private.workflow_children())?;
                    workflow.compose_with_consent_owner(
                        host.clone(),
                        consent_owner,
                        abandoned_workflows,
                    )
                }
                None if private_workflows || abandoned_workflows.is_some() => Err(
                    meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection,
                ),
                None => Ok(
                    host.clone() as Arc<dyn meerkat_mobkit::rpc_gateway::GatewayHostComposition>
                ),
            });
    let native_host = match native_host {
        Ok(host) => host,
        Err(error) => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
            });
        }
    };
    let console_setup_task = console_setup_reviews.map(|reviews| {
        tokio::spawn(crate::application_gateway::serve_console_setup(
            reviews,
            private.closed_observer(),
        ))
    });
    let remote_task = match operator_artifacts.clone() {
        Some(parent) => Some(tokio::spawn(serve_remote_operator_with_service_parent(
            host.clone(),
            remote_reviews,
            parent,
            private.closed_observer(),
            service_parent,
        ))),
        None => {
            drop(remote_reviews);
            None
        }
    };
    let operator_task = match (operator_terminal, operator_artifacts) {
        (Some(terminal), Some(parent)) => {
            let operator_host = host.clone();
            let closed = private.closed_observer();
            Some(tokio::spawn(async move {
                match setup_reviews {
                    Some(setup) => {
                        serve_local_operator_with_setup(
                            operator_host,
                            operator_reviews,
                            terminal,
                            parent,
                            closed,
                            setup,
                        )
                        .await
                    }
                    None => {
                        serve_local_operator(
                            operator_host,
                            operator_reviews,
                            terminal,
                            parent,
                            closed,
                        )
                        .await
                    }
                }
            }))
        }
        _ => {
            // Without the transferred terminal this process has no local
            // operator presenter. Requests refuse, never select a fallback.
            drop(operator_reviews);
            None
        }
    };
    let gateway_result = run_persistent_with_host(control_listen, native_host).await;
    let round_result = match restored_local_round {
        Some(round) => round.join_after_native_close().await,
        None => Ok(()),
    };
    let gateway_result = match (gateway_result, round_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error.to_string()),
        (Ok(()), Err(error)) => Err(error.to_string()),
        (Err(gateway), Err(round)) => Err(format!(
            "{gateway}; local round cleanup also failed: {round}"
        )),
    };
    // Even a failed gateway initialization closes and joins the exact
    // private owner. No ordinary error path exits the process first.
    // The private JoinSet and these presenter joins preserve issuance
    // custody even after RPC or parent loss. Neither observer aborts
    // native work. The close's final release of the native custody
    // follows them, within the host close's join bound.
    let joins = async {
        (
            private.finish().await.map_err(|error| error.to_string()),
            join_presenter(operator_task).await,
            join_presenter(console_setup_task).await,
            join_presenter(remote_task).await,
        )
    };
    let (joined, custody_result) = crate::application_driver::finish_then_release(
        toolkit_mobkit::host_gateway::CLOSE_JOIN_BOUND,
        joins,
        owner.release_native_custody(),
    )
    .await;
    let custody_result = custody_result.map_err(|error| error.to_string());
    let (private_result, operator_result, console_setup_result, remote_result) = joined
        .unwrap_or_else(|| {
            (
                Err(
                    "private host cleanup and its presenters did not finish within the close bound"
                        .into(),
                ),
                Ok(()),
                Ok(()),
                Ok(()),
            )
        });
    let operator_result = match (operator_result, remote_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(local), Err(remote)) => Err(format!(
            "{local}; remote presenter cleanup also failed: {remote}"
        )),
    };
    let operator_result = match (operator_result, console_setup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(console)) => Err(format!(
            "{error}; Console setup cleanup also failed: {console}"
        )),
    };
    let result = match (gateway_result, private_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error.to_string()),
        (Ok(()), Err(error)) => Err(error),
        (Err(gateway), Err(cleanup)) => Err(format!(
            "{gateway}; private host cleanup also failed: {cleanup}"
        )),
    };
    let result = match (result, operator_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(operator)) => Err(format!(
            "{error}; operator presenter cleanup also failed: {operator}"
        )),
    };
    match (result, custody_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(custody)) => Err(format!(
            "{error}; native custody release also failed: {custody}"
        )),
    }
}

/// Join a presenter task, if one runs.
async fn join_presenter<E: std::fmt::Display>(
    task: Option<tokio::task::JoinHandle<Result<(), E>>>,
) -> Result<(), String> {
    match task {
        Some(task) => task
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string())),
        None => Ok(()),
    }
}
