//! Render source and offer models for separately recorded TLC verification.

use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env::args_os().nth(1).ok_or("output directory required")?);
    for (name, schema, artifacts) in [
        (
            "source_record",
            toolkit_catalog::source_record_schema(),
            toolkit_catalog::source_record_model_artifacts()?,
        ),
        (
            "original_offer",
            toolkit_catalog::original_offer_schema(),
            toolkit_catalog::original_offer_model_artifacts()?,
        ),
        (
            "component_lifetime",
            toolkit_catalog::component_lifetime_schema(),
            toolkit_catalog::component_lifetime_model_artifacts()?,
        ),
    ] {
        let directory = root.join(name);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("model.tla"), artifacts.owner_model)?;
        fs::write(directory.join("entrypoint.txt"), "model.tla")?;
        fs::write(directory.join("ci.cfg"), artifacts.ci_config)?;
        fs::write(directory.join("deep.cfg"), artifacts.deep_config)?;
        let transitions = schema
            .transitions
            .iter()
            .map(|transition| transition.name.as_str())
            .collect::<Vec<_>>();
        fs::write(
            directory.join("transitions.json"),
            serde_json::to_vec_pretty(&transitions)?,
        )?;
    }
    let recovery = toolkit_catalog::original_offer_recovery_artifacts()?;
    let directory = root.join("original_offer_recovery");
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("model.tla"), recovery.owner_model)?;
    fs::write(
        directory.join("original_offer_recovery.tla"),
        recovery.composition_model,
    )?;
    fs::write(
        directory.join("entrypoint.txt"),
        "original_offer_recovery.tla",
    )?;
    let mut profiles = Vec::new();
    for profile in recovery.profiles {
        fs::write(
            directory.join(format!("{}.cfg", profile.name)),
            profile.config,
        )?;
        profiles.push(serde_json::json!({
            "name": profile.name,
            "expected_invariant": profile.expected_invariant,
        }));
    }
    fs::write(
        directory.join("profiles.json"),
        serde_json::to_vec_pretty(&profiles)?,
    )?;
    // The one rank table (I10), for the model generators.
    let directory = root.join("lock_order");
    fs::create_dir_all(&directory)?;
    let ranks = toolkit_catalog::lock_order::SOURCE_OWNER_LOCK_ORDER
        .iter()
        .map(|entry| {
            serde_json::json!({
                "lock": entry.lock,
                "rank": entry.rank,
                "owner": entry.owner,
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        directory.join("ranks.json"),
        serde_json::to_vec_pretty(&ranks)?,
    )?;
    Ok(())
}
