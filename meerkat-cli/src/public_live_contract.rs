use super::*;

#[test]
fn member_live_profile_flag_preserves_the_owner_selector_and_continuous_mode()
-> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::try_parse_from([
        "rkat",
        "mob",
        "live",
        "open",
        "mob-id",
        "worker",
        "--profile",
        "voice",
        "--turning-mode",
        "continuous",
    ])?;
    let Some(Commands::Mob {
        command:
            MobCommands::Live {
                command:
                    MobLiveCommands::Open {
                        profile,
                        turning_mode,
                        ..
                    },
            },
    }) = cli.command
    else {
        return Err("wrong CLI branch".into());
    };
    assert_eq!(
        profile
            .as_ref()
            .map(meerkat_core::live_execution::profile::LiveProfileId::as_str),
        Some("voice")
    );
    assert_eq!(turning_mode, Some(CliRealtimeTurningMode::Continuous));
    assert!(
        Cli::try_parse_from([
            "rkat",
            "mob",
            "live",
            "open",
            "mob-id",
            "worker",
            "--profile",
            "bad/profile",
        ])
        .is_err()
    );
    Ok(())
}

#[test]
fn omitted_profile_keeps_legacy_cli_selection() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::try_parse_from(["rkat", "mob", "live", "open", "mob-id", "worker"])?;
    let Some(Commands::Mob {
        command:
            MobCommands::Live {
                command:
                    MobLiveCommands::Open {
                        profile,
                        turning_mode,
                        ..
                    },
            },
    }) = cli.command
    else {
        return Err("wrong CLI branch".into());
    };
    assert!(profile.is_none());
    assert!(turning_mode.is_none());
    Ok(())
}
