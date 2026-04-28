use std::collections::VecDeque;
use std::path::PathBuf;

use meerkat_integration_tests::e2e_lanes::{
    E2eSelection, Lane, materialize_local_cargo_plan, plan_for_selection,
    smoke_test_filter_for_selection,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).collect::<VecDeque<_>>();
    let Some(command) = args.pop_front() else {
        usage();
        return Err("missing command".to_string());
    };

    match command.as_str() {
        "plan" => {
            let selection = parse_selection(args)?;
            let plan = plan_for_selection(&selection)?;
            let json = serde_json::to_string_pretty(&plan)
                .map_err(|error| format!("failed to serialize plan: {error}"))?;
            println!("{json}");
            Ok(())
        }
        "materialize" => {
            let mut manifest = None;
            let mut rest = VecDeque::new();
            while let Some(arg) = args.pop_front() {
                if arg == "--manifest" {
                    manifest = args.pop_front().map(PathBuf::from);
                } else {
                    rest.push_back(arg);
                }
            }
            let manifest =
                manifest.ok_or_else(|| "materialize requires --manifest <path>".to_string())?;
            let selection = parse_selection(rest)?;
            let plan = materialize_local_cargo_plan(&selection, &manifest)?;
            eprintln!(
                "e2e artifact manifest written: {} ({} specs, {} requirements)",
                manifest.display(),
                plan.specs.len(),
                plan.requirements.len()
            );
            Ok(())
        }
        "smoke-test-filter" => {
            let selection = parse_selection(args)?;
            if let Some(filter) = smoke_test_filter_for_selection(&selection)? {
                println!("{filter}");
            }
            Ok(())
        }
        "-h" | "--help" | "help" => {
            usage();
            Ok(())
        }
        _ => {
            usage();
            Err(format!("unknown command '{command}'"))
        }
    }
}

fn parse_selection(mut args: VecDeque<String>) -> Result<E2eSelection, String> {
    let mut selection = None;
    while let Some(arg) = args.pop_front() {
        let next = |args: &mut VecDeque<String>, flag: &str| {
            args.pop_front()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        let parsed = match arg.as_str() {
            "--lane" => match next(&mut args, "--lane")?.as_str() {
                "smoke" | "e2e-smoke" => E2eSelection::Lane(Lane::Smoke),
                other => return Err(format!("unsupported lane '{other}'")),
            },
            "--scenario" => {
                let value = next(&mut args, "--scenario")?;
                E2eSelection::Scenario(
                    value
                        .parse()
                        .map_err(|error| format!("invalid scenario id '{value}': {error}"))?,
                )
            }
            "--suite" => E2eSelection::Suite(next(&mut args, "--suite")?),
            "--test" => E2eSelection::SmokeTest(next(&mut args, "--test")?),
            "-h" | "--help" => {
                usage();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown selection argument '{arg}'")),
        };

        if selection.replace(parsed).is_some() {
            return Err("provide only one of --lane, --scenario, --suite, or --test".to_string());
        }
    }

    Ok(selection.unwrap_or(E2eSelection::Lane(Lane::Smoke)))
}

fn usage() {
    eprintln!(
        "usage: e2e_artifacts <plan|materialize|smoke-test-filter> [--lane smoke|--scenario N|--suite NAME|--test NAME] [--manifest PATH]"
    );
}
