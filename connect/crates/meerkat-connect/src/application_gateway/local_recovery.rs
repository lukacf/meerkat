//! Review the existing installed owner's recovery offer on its sole terminal.
//! This is an idle operator command, not ordinary --realm management.
use super::*;
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalStatus, SessionId,
};
use std::{
    io::Write,
    path::Path,
    time::{Duration, SystemTime},
};
use toolkit_mobkit::host_gateway::{LocalRecoveryPageRead, LocalServiceRecoveryPage};

fn output(terminal: &mut InheritedOperatorTerminal, message: &str) -> Result<(), ConnectError> {
    writeln!(terminal.output, "{}", crate::terminal::review_text(message))
        .and_then(|()| terminal.output.flush())
        .map_err(|error| ConnectError::native("display installed operator recovery", error))
}
async fn line(
    terminal: &mut InheritedOperatorTerminal,
    closed: &PrivateHostClosed,
) -> Result<Option<String>, ConnectError> {
    let value = tokio::select! { biased;
        _ = closed.wait_closed() => return Ok(None),
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| ConnectError::native("listen for recovery cancellation", error))?;
            return Ok(None);
        },
        value = terminal.input.recv() => value,
    };
    value
        .transpose()
        .map_err(|error| ConnectError::native("read recovery choice", error))
}

fn row(page: &LocalServiceRecoveryPage, choice: &str) -> Option<serde_json::Value> {
    let index = choice.parse::<usize>().ok()?.checked_sub(1)?;
    page.projection()["rows"].as_array()?.get(index).cloned()
}

async fn confirm(
    terminal: &mut InheritedOperatorTerminal,
    parent: &Path,
    closed: &PrivateHostClosed,
    selected: serde_json::Value,
) -> Result<bool, ConnectError> {
    terminal
        .input
        .require_terminal()
        .map_err(|error| ConnectError::native("require protected recovery terminal", error))?;
    let (approvals, presenter) = LocalApprovals::create(parent)?;
    let mut presenter = Some(presenter);
    // Attribution for this existing approval presenter only. No native runtime
    // session or Original is created for administrative cleanup.
    let session = SessionId::new();
    approvals.bind_session(&session)?;
    let request = ApprovalRequest {
        requester: ApprovalPrincipalId::new("connect-local-operator")
            .map_err(|error| ConnectError::native("name local recovery presenter", error))?,
        owner: ApprovalOwnerRef::Session {
            session_id: session,
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("unpublished-service-approval"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Attempt to revoke this exact unresolved service approval?".into(),
            body: Some(selected),
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: SystemTime::now()
            .checked_add(Duration::from_secs(120))
            .map(Into::into),
        metadata: Default::default(),
        request_provenance: None,
    };
    let review = approvals.review_access(request);
    tokio::pin!(review);
    let result = loop {
        let prompt = tokio::select! { biased;
            result = review.as_mut() => break result,
            _ = closed.wait_closed() => {
                break retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
            },
            signal = tokio::signal::ctrl_c() => {
                let result = retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
                break match signal {
                    Ok(()) => result,
                    Err(error) => Err(ConnectError::native("listen before recovery approval", error)),
                };
            },
            prompt = presenter.as_mut().expect("retained recovery presenter").next() => prompt,
        };
        let Some(prompt) = prompt else {
            break retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
        };
        match terminal.input.fresh_prompt() {
            Ok(true) => {}
            Ok(false) => {
                drop(prompt);
                break retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
            }
            Err(error) => {
                drop(prompt);
                let joined =
                    retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
                let primary = ConnectError::native("start exact recovery prompt", error);
                break Err(match joined {
                    Ok(_) => primary,
                    Err(cleanup) => ConnectError::Cleanup {
                        primary: Box::new(primary),
                        cleanup: Box::new(cleanup),
                    },
                });
            }
        }
        let interrupted = async {
            tokio::select! { biased;
                _ = closed.wait_closed() => Ok(()), signal = tokio::signal::ctrl_c() => signal,
            }
        };
        match present_approval_with_review(
            prompt,
            &mut terminal.input,
            &mut terminal.output,
            interrupted,
            review.as_mut(),
        )
        .await
        {
            ReviewPresentation::Reviewed(result) => break result,
            ReviewPresentation::Presented(Ok(true)) => {}
            ReviewPresentation::Presented(Ok(false)) => {
                break retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
            }
            ReviewPresentation::Presented(Err(error)) => {
                let joined =
                    retire_presenter_and_join(&approvals, &mut presenter, review.as_mut()).await;
                break Err(match joined {
                    Ok(_) => error,
                    Err(cleanup) => ConnectError::InvalidInput(format!(
                        "{error}; review retirement also failed: {cleanup}"
                    )),
                });
            }
        }
    };
    let retired = approvals.retire_checked();
    drop(presenter.take());
    let restored = terminal
        .input
        .check_cleanup()
        .map_err(|error| ConnectError::native("restore recovery terminal", error));
    // Attempt both owner retirement and terminal cleanup even when either
    // fails; neither changes the native recovery result (no mutation here).
    retired?;
    restored?;
    Ok(result?.status == ApprovalStatus::Approved)
}

pub(super) async fn present(
    host: &ToolkitGatewayHost,
    terminal: &mut InheritedOperatorTerminal,
    artifact_parent: &Path,
    closed: &PrivateHostClosed,
) -> Result<bool, ConnectError> {
    terminal
        .input
        .require_terminal()
        .map_err(|error| ConnectError::native("check local operator terminal", error))?;
    let view = match host.local_service_recovery() {
        Ok(Some(view)) => view,
        _ => {
            output(
                terminal,
                "Recovery is unavailable for this installed host and local operator. No cleanup was entered.",
            )?;
            return Ok(true);
        }
    };
    let mut page = match view.page(None).await {
        Ok(page) => page,
        Err(_) => {
            output(
                terminal,
                "The current approval inventory is unavailable. No cleanup was entered.",
            )?;
            return Ok(true);
        }
    };
    loop {
        let projection = page.projection();
        let rows = projection["rows"]
            .as_array()
            .expect("owner projection rows");
        output(
            terminal,
            "Unresolved operator approvals. Missing task publication does not establish abandonment. Cleanup attempts native grant revocation; policy, credentials, software and running work are separate.",
        )?;
        for (index, selected) in rows.iter().enumerate() {
            output(
                terminal,
                &format!(
                    "{}. {}",
                    index + 1,
                    serde_json::to_string(selected).map_err(|error| ConnectError::native(
                        "project exact recovery row",
                        error
                    ))?
                ),
            )?;
        }
        if rows.is_empty() {
            output(terminal, "No visible approvals on this page.")?;
        }
        if !terminal
            .input
            .fresh_prompt()
            .map_err(|error| ConnectError::native("start recovery selection", error))?
        {
            return Ok(false);
        }
        output(
            terminal,
            if page.has_next() {
                "Choose a row number, next, refresh, or back."
            } else {
                "Choose a row number, refresh, or back."
            },
        )?;
        let Some(choice) = line(terminal, closed).await? else {
            return Ok(false);
        };
        match choice.trim() {
            "back" | "/quit" => return Ok(true),
            "next" | "refresh" => {
                let direction = if choice.trim() == "next" {
                    LocalRecoveryPageRead::Next
                } else {
                    LocalRecoveryPageRead::Refresh
                };
                page = match view.page(Some((&page, direction))).await {
                    Ok(page) => page,
                    Err(_) => {
                        output(
                            terminal,
                            "The inventory changed or is unavailable. No cleanup was entered; use /recover for a new observation.",
                        )?;
                        return Ok(true);
                    }
                };
            }
            value => {
                let Some(selected) = row(&page, value).filter(|row| row["can_recover"] == true)
                else {
                    output(
                        terminal,
                        "Choose a displayed actionable row. A missing parent or published source cannot be cleaned up here.",
                    )?;
                    continue;
                };
                let id = selected["id"]
                    .as_str()
                    .ok_or_else(|| {
                        ConnectError::InvalidInput(
                            "The retained recovery row is unavailable".into(),
                        )
                    })?
                    .to_owned();
                if !confirm(terminal, artifact_parent, closed, selected).await? {
                    return Ok(true);
                }
                if closed.is_closed_for_presenter() {
                    return Ok(false);
                }
                // Cancellation can end only awaited preparation. The same
                // poll enters and returns both synchronous native effects;
                // there is no child worker or post-mutation await to abandon.
                let result = tokio::select! { biased;
                    _ = closed.wait_closed() => return Ok(false),
                    signal = tokio::signal::ctrl_c() => {
                        signal.map_err(|error| ConnectError::native("listen before recovery entry", error))?;
                        return Ok(false);
                    },
                    result = view.revoke(&page, &id) => result,
                };
                match result {
                    Ok(result) => {
                        output(
                            terminal,
                            &serde_json::to_string(&result.projection()).map_err(|error| {
                                ConnectError::native("project native recovery result", error)
                            })?,
                        )?;
                        if result.uncertain() {
                            output(
                                terminal,
                                "Native cleanup is uncertain. Do not retry automatically; a later deliberate review must obtain a new inventory.",
                            )?;
                        }
                    }
                    Err(_) => output(
                        terminal,
                        "This attempt refused before native mutation. Earlier uncertainty remains unresolved; use a new inventory for any later deliberate action.",
                    )?,
                }
                return Ok(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use nix::{pty::openpty, sys::termios::tcgetattr, unistd::write};

    fn native_record(parent: &Path, id: &str) -> meerkat_core::ApprovalRecord {
        let rows = std::fs::read_dir(parent)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        let store =
            meerkat_store::FileApprovalStore::open(rows[0].path().join("native-decisions.json"))
                .unwrap();
        let service = meerkat_core::ApprovalService::with_store(Arc::new(store)).unwrap();
        let id = serde_json::from_value(serde_json::json!(id)).unwrap();
        service.get(&id).unwrap()
    }
    fn shown_id(path: &Path) -> String {
        let shown = std::fs::read_to_string(path).unwrap();
        shown
            .split("Type 'approve ")
            .nth(1)
            .unwrap()
            .split('\'')
            .next()
            .unwrap()
            .to_owned()
    }
    fn selected() -> serde_json::Value {
        serde_json::json!({"id":"b".repeat(64),"service":"mail","target":"worker",
            "operations":["read"],"resource":"inbox","state":"publication_unresolved",
            "parent":"issued","child":"issued","policy":"pending","can_recover":true})
    }

    #[tokio::test]
    async fn recovery_terminal_requires_fresh_exact_decision_and_releases_the_same_input() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (connection, commands, responses) =
                super::super::setup_presenter::tests::presentation_pipe().await;
            let closed = connection.closed_observer();
            for approve in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let output = tempfile::NamedTempFile::new().unwrap();
                let pair = openpty(None, None).unwrap();
                let observer = pair.slave.try_clone().unwrap();
                let original = tcgetattr(&observer).unwrap();
                let mut terminal = InheritedOperatorTerminal {
                    input: TerminalInput::from_owned_fd(pair.slave),
                    output: output.reopen().unwrap(),
                };
                // A queued ordinary line is not a decision for the exact offer.
                write(&pair.master, b"approve stale-id\n").unwrap();
                let choice = selected();
                let mut review = Box::pin(confirm(
                    &mut terminal,
                    directory.path(),
                    &closed,
                    choice.clone(),
                ));
                assert!(review.as_mut().now_or_never().is_none());
                let id = shown_id(output.path());
                assert_eq!(
                    native_record(directory.path(), &id).status,
                    ApprovalStatus::Pending
                );
                let command = format!("{} {id}\n", if approve { "approve" } else { "deny" });
                write(&pair.master, command.as_bytes()).unwrap();
                assert_eq!(review.await.unwrap(), approve);
                let record = native_record(directory.path(), &id);
                assert_eq!(
                    record.status,
                    if approve {
                        ApprovalStatus::Approved
                    } else {
                        ApprovalStatus::Denied
                    }
                );
                assert_eq!(record.proposed_action.body, Some(choice));
                terminal.input.check_cleanup().unwrap();
                assert_eq!(tcgetattr(&observer).unwrap(), original);
                write(&pair.master, b"next ordinary command\n").unwrap();
                assert_eq!(
                    terminal.input.recv().await.unwrap().unwrap(),
                    "next ordinary command"
                );
            }
            connection.finish().await.unwrap();
            closed.wait_closed().await;
            drop((commands, responses));
        })
        .await
        .expect("exact terminal review or input restoration did not settle");
    }

    #[tokio::test]
    async fn recovery_terminal_close_joins_pending_review_without_fabricating_a_decision() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (connection, commands, responses) =
                super::super::setup_presenter::tests::presentation_pipe().await;
            let closed = connection.closed_observer();
            let directory = tempfile::tempdir().unwrap();
            let output = tempfile::NamedTempFile::new().unwrap();
            let pair = openpty(None, None).unwrap();
            let observer = pair.slave.try_clone().unwrap();
            let original = tcgetattr(&observer).unwrap();
            let mut terminal = InheritedOperatorTerminal {
                input: TerminalInput::from_owned_fd(pair.slave),
                output: output.reopen().unwrap(),
            };
            let mut review = Box::pin(confirm(
                &mut terminal,
                directory.path(),
                &closed,
                selected(),
            ));
            assert!(review.as_mut().now_or_never().is_none());
            let id = shown_id(output.path());
            connection.finish().await.unwrap();
            closed.wait_closed().await;
            assert!(review.await.is_err());
            assert_eq!(
                native_record(directory.path(), &id).status,
                ApprovalStatus::Pending
            );
            terminal.input.check_cleanup().unwrap();
            assert_eq!(tcgetattr(&observer).unwrap(), original);
            write(&pair.master, b"ordinary input after close\n").unwrap();
            assert_eq!(
                terminal.input.recv().await.unwrap().unwrap(),
                "ordinary input after close"
            );
            drop((commands, responses));
        })
        .await
        .expect("private close stranded recovery review until its human deadline");
    }
}
