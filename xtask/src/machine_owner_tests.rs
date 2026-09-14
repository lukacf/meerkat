use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerTestSpec {
    pub package: &'static str,
    pub target: &'static str,
    pub filters: &'static [&'static str],
    pub features: &'static [&'static str],
}

pub(crate) fn owner_test_specs_for_machine(slug: &str) -> &'static [OwnerTestSpec] {
    const MEERKAT: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_turn_admission_kernel",
            filters: &["session_turn_admission_kernel_attached_state_reached"],
            features: &[],
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_turn_admission_kernel",
            filters: &["session_turn_admission_kernel_interrupt_allowed_while_attached"],
            features: &[],
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_tool_visibility_kernel",
            filters: &["session_tool_visibility_kernel_publishes_committed_set_from_attached"],
            features: &[],
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_tool_visibility_kernel",
            filters: &[
                "session_tool_visibility_kernel_stages_deferred_requests_without_touching_active_state",
            ],
            features: &[],
        },
    ];
    const MOB: &[OwnerTestSpec] = &[OwnerTestSpec {
        package: "meerkat-mob",
        target: "lib",
        filters: &[
            "runtime::tests::test_cancel_fallback_uses_direct_pending_to_terminal_cas_attempts",
        ],
        features: &[],
    }];
    const LIVE_REQUEST: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-runtime",
            target: "lib",
            filters: &[
                "live_ledger::write::tests::trusted_native_issuer_commits_complete_permission_to_current_lifecycle",
                "live_ledger::write::tests::activation_commit_compares_lifecycle_inside_each_store_transaction",
                "live_ledger::write::tests::generated_request_transition_is_returned_only_after_actual_store_commit",
                "live_ledger::write::tests::generated_store_claim_reads_revocation_after_an_actual_policy_await",
                "live_ledger::write::tests::generated_request_scope_and_spent_claim_survive_closed_store_reopen",
                "live_ledger::write::tests::generated_request_close_before_admission_does_not_create_a_run",
                "live_ledger::write::tests::generated_request_snapshot_codec_rejects_unknown_missing_and_future_fields",
                "live_ledger::write::tests::private_recovery_import_cannot_supply_public_request_scope_authority",
                "live_ledger::write::tests::generated_executor_binding_aba_stays_revoked_after_reopen",
            ],
            features: &["live", "sqlite-store"],
        },
        OwnerTestSpec {
            package: "meerkat-runtime",
            target: "public_live_scoped_authority",
            filters: &[
                "scoped_execution_requires_the_canonical_live_request_owner",
                "generated_claim_enforces_stored_permission_even_when_ordinary_policy_permits",
                "generated_grant_limits_survive_completion_and_recovery",
                "generated_recovery_rejects_lost_permission_and_occupancy_fields",
                "generated_scope_restore_preserves_admission_won_close_and_exact_bindings",
                "generated_restore_rejects_missing_scope_fields_and_current_fences",
                "generated_recovery_rejects_same_cardinality_identity_corruption",
                "executor_binding_aba_cannot_reactivate_an_old_admission",
                "generated_claim_rechecks_revocation_after_policy_await",
                "claimed_effect_survives_revoke_and_recovery_without_resend_permission",
            ],
            features: &["live", "sqlite-store"],
        },
    ];
    const LIVE_TRANSCRIPT: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-runtime",
            target: "public_live_transcript_authority",
            filters: &[
                "generated_transcript_append_requires_contiguous_receive_identity",
                "generated_prefix_reservation_spends_frontier_and_preserves_gap_facts",
                "generated_transcript_control_credit_always_preserves_final_fence",
                "generated_crash_fence_never_advances_received_ordinal",
            ],
            features: &[],
        },
        OwnerTestSpec {
            package: "meerkat-runtime",
            target: "lib",
            filters: &[
                "live_ledger::write::tests::transcript_tests::native_transcript_ingress_keeps_actor_and_request_independent",
                "live_ledger::write::tests::transcript_tests::native_transcript_loss_closes_known_tail_with_last_credit",
                "live_ledger::write::tests::transcript_tests::native_transcript_activation_requires_current_lifecycle",
                "live_ledger::write::tests::transcript_tests::native_transcript_cold_recovery_cannot_guess_uncommitted_receive_count",
            ],
            features: &["sqlite-store"],
        },
    ];

    match slug {
        "meerkat_machine" => MEERKAT,
        "mob_machine" => MOB,
        "live_request" => LIVE_REQUEST,
        "live_transcript" => LIVE_TRANSCRIPT,
        _ => &[],
    }
}

fn owner_test_command(root: &Path, spec: &OwnerTestSpec) -> Command {
    let mut cmd = Command::new(root.join("scripts/repo-cargo"));
    cmd.arg("test").arg("-p").arg(spec.package);
    if spec.target == "lib" {
        cmd.arg("--lib");
    } else {
        cmd.arg("--test").arg(spec.target);
    }
    if !spec.features.is_empty() {
        cmd.arg("--features").arg(spec.features.join(","));
    }
    cmd.arg("--")
        .arg("--exact")
        .arg("--test-threads=1")
        .arg("--color")
        .arg("never")
        .args(spec.filters)
        .current_dir(root);
    cmd
}

fn ensure_owner_tests_completed(stdout: &str, spec: &OwnerTestSpec) -> Result<()> {
    if spec.filters.is_empty() {
        bail!("owner test selection is empty");
    }
    let expected = format!(
        "test result: ok. {} passed; 0 failed; 0 ignored; 0 measured; ",
        spec.filters.len()
    );
    let mut summaries = stdout
        .lines()
        .filter(|line| line.starts_with("test result:"));
    if !summaries
        .next()
        .is_some_and(|line| line.starts_with(&expected))
        || summaries.next().is_some()
    {
        bail!("owner test summary did not confirm every selected test passed: {expected}");
    }
    for filter in spec.filters {
        let expected = format!("test {filter} ... ok");
        if stdout.lines().filter(|line| *line == expected).count() != 1 {
            bail!("owner test did not execute exactly once: {filter}");
        }
    }
    Ok(())
}

pub(crate) fn run_machine_owner_tests(root: &Path, machine: &str, slug: &str) -> Result<()> {
    for spec in owner_test_specs_for_machine(slug) {
        println!(
            "owner-tests: {machine} -> {}::{} {:?}",
            spec.package, spec.target, spec.filters
        );
        let output = owner_test_command(root, spec)
            .output()
            .with_context(|| format!("run owner tests {}::{}", spec.package, spec.target))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        print!("{stdout}");
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            bail!(
                "owner tests failed for {machine}: {}::{}",
                spec.package,
                spec.target
            );
        }
        ensure_owner_tests_completed(&stdout, spec)
            .with_context(|| format!("qualify owner tests {}::{}", spec.package, spec.target))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: OwnerTestSpec = OwnerTestSpec {
        package: "meerkat-runtime",
        target: "lib",
        filters: &["first", "second"],
        features: &["live", "sqlite-store"],
    };
    const COMPLETE: &str = "\
running 2 tests
test first ... ok
test second ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 81 filtered out; finished in 0.01s
";

    #[test]
    fn actual_owner_summary_requires_every_selected_test() {
        assert!(ensure_owner_tests_completed(COMPLETE, &SPEC).is_ok());
        for output in [
            "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 83 filtered out;",
            "test first ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 82 filtered out;",
            "test first ... ok\ntest second ... ignored\ntest result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 81 filtered out;",
            "test first ... ok\ntest wrong_name ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 81 filtered out;",
            "",
        ] {
            assert!(
                ensure_owner_tests_completed(output, &SPEC).is_err(),
                "{output}"
            );
        }
        assert!(ensure_owner_tests_completed(&format!("{COMPLETE}{COMPLETE}"), &SPEC).is_err());
        assert!(
            ensure_owner_tests_completed(
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 83 filtered out;",
                &OwnerTestSpec {
                    filters: &[],
                    ..SPEC
                },
            )
            .is_err()
        );
    }

    #[test]
    fn owner_command_selects_exact_named_tests_and_required_features() {
        let command = owner_test_command(Path::new("/checkout"), &SPEC);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "test",
                "-p",
                "meerkat-runtime",
                "--lib",
                "--features",
                "live,sqlite-store",
                "--",
                "--exact",
                "--test-threads=1",
                "--color",
                "never",
                "first",
                "second",
            ]
        );
        let integration = owner_test_command(
            Path::new("/checkout"),
            &OwnerTestSpec {
                target: "integration",
                features: &[],
                ..SPEC
            },
        );
        assert_eq!(
            integration.get_args().collect::<Vec<_>>(),
            [
                "test",
                "-p",
                "meerkat-runtime",
                "--test",
                "integration",
                "--",
                "--exact",
                "--test-threads=1",
                "--color",
                "never",
                "first",
                "second",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn successful_process_with_zero_tests_is_not_owner_qualification() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir()?;
        let scripts = root.path().join("scripts");
        std::fs::create_dir(&scripts)?;
        let wrapper = scripts.join("repo-cargo");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\nprintf 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out;\\n'\n",
        )?;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
        assert!(
            run_machine_owner_tests(root.path(), "LiveRequestMachine", "live_request").is_err()
        );
        std::fs::write(&wrapper, "#!/bin/sh\nexit 1\n")?;
        assert!(
            run_machine_owner_tests(root.path(), "LiveRequestMachine", "live_request").is_err()
        );
        Ok(())
    }

    #[test]
    fn missing_runner_is_not_owner_qualification() -> Result<()> {
        let root = tempfile::tempdir()?;
        assert!(
            run_machine_owner_tests(root.path(), "LiveRequestMachine", "live_request").is_err()
        );
        Ok(())
    }
}
