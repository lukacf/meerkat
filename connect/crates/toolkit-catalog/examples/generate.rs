//! Render model artifacts for a separate recorded verification run.

use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(env::args_os().nth(1).ok_or("output directory required")?);
    fs::create_dir_all(&directory)?;
    let (model, ci, deep) = toolkit_catalog::acquisition_model_artifacts()?;
    fs::write(directory.join("model.tla"), model)?;
    fs::write(directory.join("ci.cfg"), ci)?;
    fs::write(directory.join("deep.cfg"), deep)?;
    let schema = toolkit_catalog::acquisition_schema();
    let transitions = schema
        .transitions
        .iter()
        .map(|transition| transition.name.as_str())
        .collect::<Vec<_>>();
    fs::write(
        directory.join("transitions.json"),
        serde_json::to_vec_pretty(&transitions)?,
    )?;
    Ok(())
}
