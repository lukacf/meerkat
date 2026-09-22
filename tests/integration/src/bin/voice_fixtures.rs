//! Mint and verify the browser-peer voice fixtures declared in
//! `tests/live_smoke/browser/fixtures/gpt_live_client/manifest.json`.
//!
//! ```text
//! voice_fixtures verify [--manifest <path>]
//! voice_fixtures mint <name> [--force] [--manifest <path>]
//! voice_fixtures mint --missing [--manifest <path>]
//! ```
//!
//! `verify` is offline. `mint` calls the OpenAI speech API with the entry's
//! voice and text (`OPENAI_API_KEY` or `RKAT_OPENAI_API_KEY`), applies the
//! live-adapter VAD normalisation plus the trailing-silence floor, writes
//! the WAV, and updates the entry's sha256/duration/silence fields.

use std::collections::VecDeque;
use std::path::PathBuf;

use meerkat_integration_tests::voice_fixtures::{
    FIXTURE_DIR, FixtureManifest, MANIFEST_FILE, VoiceFixtureError, mint_fixture, workspace_root,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn usage() {
    eprintln!(
        "usage:\n  voice_fixtures verify [--manifest <path>]\n  voice_fixtures mint <name> [--force] [--manifest <path>]\n  voice_fixtures mint --missing [--manifest <path>]"
    );
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).collect::<VecDeque<_>>();
    let Some(command) = args.pop_front() else {
        usage();
        return Err("missing command".into());
    };
    let mut manifest_path = None;
    let mut force = false;
    let mut missing = false;
    let mut names = Vec::new();
    while let Some(arg) = args.pop_front() {
        match arg.as_str() {
            "--manifest" => {
                manifest_path = Some(PathBuf::from(
                    args.pop_front().ok_or("--manifest requires a path")?,
                ));
            }
            "--force" => force = true,
            "--missing" => missing = true,
            "-h" | "--help" => {
                usage();
                return Ok(());
            }
            other if other.starts_with("--") => return Err(format!("unknown flag {other}").into()),
            other => names.push(other.to_owned()),
        }
    }
    let manifest_path = match manifest_path {
        Some(path) => path,
        None => workspace_root()
            .ok_or("cannot locate the workspace root; set MEERKAT_WORKSPACE_ROOT")?
            .join(FIXTURE_DIR)
            .join(MANIFEST_FILE),
    };
    let fixture_dir = manifest_path
        .parent()
        .ok_or("manifest path has no parent directory")?
        .to_path_buf();
    let mut manifest = FixtureManifest::load(&manifest_path)?;

    match command.as_str() {
        "verify" => {
            let verified = manifest.verify(&fixture_dir)?;
            for fixture in &verified {
                println!(
                    "ok {:<20} duration_ms={:<6} trailing_silence_ms={}",
                    fixture.name, fixture.duration_ms, fixture.trailing_silence_ms
                );
            }
            println!(
                "verified {} fixture(s) in {}",
                verified.len(),
                fixture_dir.display()
            );
            Ok(())
        }
        "mint" => {
            let api_key = ["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
                .ok_or("OPENAI_API_KEY (or RKAT_OPENAI_API_KEY) is required to mint")?;
            let selected: Vec<String> = if missing {
                manifest
                    .fixtures
                    .iter()
                    .filter(|entry| entry.is_mintable() && !fixture_dir.join(&entry.file).is_file())
                    .map(|entry| entry.name.clone())
                    .collect()
            } else {
                if names.is_empty() {
                    usage();
                    return Err("mint requires a fixture name or --missing".into());
                }
                names
            };
            if selected.is_empty() {
                println!("nothing to mint: every mintable fixture exists");
                return Ok(());
            }
            let sample_rate_hz = manifest.sample_rate_hz;
            let floor = manifest.mint_trailing_silence_ms;
            for name in selected {
                let entry = manifest.entry_mut(&name)?;
                if !entry.is_mintable() {
                    return Err(format!(
                        "{name}: voice {:?} is a historical fixture and cannot be re-minted",
                        entry.voice
                    )
                    .into());
                }
                if fixture_dir.join(&entry.file).is_file() && !force && !missing {
                    return Err(format!(
                        "{name}: {} exists; pass --force to overwrite",
                        entry.file
                    )
                    .into());
                }
                let wav =
                    mint_fixture(&api_key, &fixture_dir, sample_rate_hz, floor, entry).await?;
                println!(
                    "minted {:<20} file={} voice={} duration_ms={} sha256={}",
                    entry.name,
                    entry.file,
                    entry.voice,
                    wav.duration_ms(),
                    entry.sha256
                );
            }
            manifest.save(&manifest_path)?;
            // Never leave a manifest that fails its own verification.
            manifest.verify(&fixture_dir)?;
            println!("manifest updated: {}", manifest_path.display());
            Ok(())
        }
        other => {
            usage();
            Err(Box::new(VoiceFixtureError::UnknownFixture(format!(
                "command {other}"
            ))))
        }
    }
}
