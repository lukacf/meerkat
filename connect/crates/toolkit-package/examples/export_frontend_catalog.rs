//! Maintainer export of canonical frontend observations for Python consumers.
//! No runtime, installation, network request or source interpretation occurs.

use std::{error::Error, fs, path::Path};

use toolkit_package::{
    ContentDigest, FRONTEND_CATALOG_MAX_BYTES, bundled_discovery_catalog,
    frontend_catalog_observation,
};

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args_os().len() != 1 {
        return Err("this maintainer export takes no arguments".into());
    }
    let catalog = bundled_discovery_catalog()?;
    let bytes = serde_json::to_vec(&frontend_catalog_observation(&catalog))?;
    if bytes.len() > FRONTEND_CATALOG_MAX_BYTES {
        return Err("the complete frontend catalog exceeds the resource limit".into());
    }
    let digest = ContentDigest::of_bytes(&bytes);
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/python/src/meerkat_toolkit/data");
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("frontend_catalog.json"), &bytes)?;
    fs::write(
        directory.join("frontend_catalog.sha256"),
        format!("{}\n", digest.as_str()),
    )?;
    println!(
        "frontend_catalog.json: {} bytes, sha256:{}",
        bytes.len(),
        digest.as_str()
    );
    Ok(())
}
