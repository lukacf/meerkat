//! `machine-alphabet`: emit the canonical machine alphabet as JSON.
//!
//! The machine-poster CONTENT-drift gate
//! (`scripts/machine-posters/generate-machine-posters.mjs --check
//! --alphabet <path>`) consumes this output to verify that the hand-authored
//! poster specs only advertise states (phase enum variants) and triggers
//! (input/signal enum variants) that exist in the canonical machine
//! authority, `meerkat_machine_schema::canonical_machine_schemas()`.
//!
//! The JSON is regenerated from the compiled catalog at check time (no
//! committed artifact), so it cannot go stale relative to the schemas: any
//! catalog change is reflected the next time the gate runs.

use anyhow::{Context, Result};
use clap::Args;
use meerkat_machine_schema::{EnumSchema, MachineSchema, canonical_machine_schemas};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct MachineAlphabetArgs {
    /// Write the alphabet JSON to this path (stdout when omitted).
    #[arg(long)]
    pub emit: Option<PathBuf>,
}

/// One canonical machine's poster-relevant alphabet.
#[derive(Debug, Serialize)]
struct MachineAlphabetEntry {
    /// Declared machine identity (e.g. `"MeerkatMachine"`).
    machine: String,
    /// Phase enum variant names — the only legal poster phase/group anchors.
    states: Vec<String>,
    /// Input enum variant names (includes surface-only and runtime-internal
    /// inputs) — legal poster trigger names of kind `input`.
    triggers: Vec<String>,
    /// Signal enum variant names — legal poster trigger names of kind
    /// `signal` (transitions may be signal-triggered).
    signals: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MachineAlphabetDocument {
    /// Entries in `canonical_machine_schemas()` catalog order. Consumers that
    /// need the `dsl_<fn>` binding zip this array against the call order in
    /// `meerkat-machine-schema/src/catalog/mod.rs::canonical_machine_schemas`
    /// (the same source this array is built from) and must verify the paired
    /// `machine` identity independently.
    machines: Vec<MachineAlphabetEntry>,
}

pub fn run_machine_alphabet(args: MachineAlphabetArgs) -> Result<()> {
    let machines = canonical_machine_schemas()
        .into_iter()
        .map(alphabet_entry)
        .collect();
    let document = MachineAlphabetDocument { machines };
    let mut json = serde_json::to_string_pretty(&document).context("serialize machine alphabet")?;
    json.push('\n');
    match args.emit {
        Some(path) => std::fs::write(&path, json.as_bytes())
            .with_context(|| format!("write machine alphabet to {}", path.display()))?,
        None => print!("{json}"),
    }
    Ok(())
}

fn alphabet_entry(schema: MachineSchema) -> MachineAlphabetEntry {
    MachineAlphabetEntry {
        machine: schema.machine.as_str().to_owned(),
        states: variant_names(&schema.state.phase),
        triggers: variant_names(&schema.inputs),
        signals: variant_names(&schema.signals),
    }
}

fn variant_names(schema: &EnumSchema) -> Vec<String> {
    schema
        .variants
        .iter()
        .map(|variant| variant.name.as_str().to_owned())
        .collect()
}
