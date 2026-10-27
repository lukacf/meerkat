//! Trusted local projection of an explicit package and activation selection.
//! No network, process execution, environment lookup or credential resolution.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use toolkit_package::{
    ActivationManifest, LocalSoftwareDeclaration, Id, IntegrationPackage, NodeInstallPlan, canonical_json,
};

const MAX_DOCUMENT: usize = 1024 * 1024;
const HELP: &str =
    "toolkit-node-plan --package ABSOLUTE_PATH (--selection ABSOLUTE_PATH | --declaration ABSOLUTE_PATH) --component ID

Validates explicit local inputs and emits a Node installation projection.
It does not install, launch, authorize, authenticate or establish readiness.
An installer must invoke this trusted executable against its own frozen inputs;
serialized plan data is neither authorization nor a reusable validation receipt.
Input directories must remain controlled by the local operator. Final-component
symlinks and nonregular files are refused; this is not a filesystem sandbox.
";

#[derive(Debug)]
struct Failure(&'static str);

fn absolute(value: String) -> Result<PathBuf, Failure> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(Failure("absolute_path_required"));
    }
    Ok(path)
}

fn document(path: &Path) -> Result<Vec<u8>, Failure> {
    // The trusted operator owns parent directories. O_NOFOLLOW protects the
    // final component during the actual open, not just the metadata preflight.
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    return Err(Failure("unsupported_platform"));
    if !fs::symlink_metadata(path)
        .map_err(|_| Failure("input_unavailable"))?
        .file_type()
        .is_file()
    {
        return Err(Failure("regular_file_required"));
    }
    let file = options
        .open(path)
        .map_err(|_| Failure("input_unavailable"))?;
    let metadata = file.metadata().map_err(|_| Failure("input_unavailable"))?;
    if !metadata.is_file() {
        return Err(Failure("regular_file_required"));
    }
    if metadata.len() > MAX_DOCUMENT as u64 {
        return Err(Failure("input_too_large"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_DOCUMENT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure("input_unavailable"))?;
    if bytes.len() > MAX_DOCUMENT {
        return Err(Failure("input_too_large"));
    }
    Ok(bytes)
}

fn run(arguments: Vec<String>) -> Result<Vec<u8>, Failure> {
    if arguments.len() != 6 {
        return Err(Failure("invalid_arguments"));
    }
    let mut values = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        if !["--package", "--selection", "--declaration", "--component"].contains(&pair[0].as_str())
            || values.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err(Failure("invalid_arguments"));
        }
    }
    let declaration = values.contains_key("--declaration");
    let mut take = |name: &str| values.remove(name).ok_or(Failure("invalid_arguments"));
    let package_path = absolute(take("--package")?)?;
    let selection_path = absolute(take(if declaration { "--declaration" } else { "--selection" })?)?;
    let component = Id::new(take("--component")?).map_err(|_| Failure("invalid_component"))?;
    let package: IntegrationPackage = serde_json::from_slice(&document(&package_path)?)
        .map_err(|_| Failure("invalid_package"))?;
    let selected_bytes = document(&selection_path)?;
    let plan = if declaration {
        let selection: LocalSoftwareDeclaration = serde_json::from_slice(&selected_bytes)
            .map_err(|_| Failure("invalid_selection"))?;
        let validated = selection.validate(&package).map_err(|_| Failure("invalid_selection"))?;
        NodeInstallPlan::from_declaration(&package, &validated, &component)
            .map_err(|_| Failure("unsupported_node_selection"))?
    } else {
        let selection: ActivationManifest = serde_json::from_slice(&selected_bytes)
            .map_err(|_| Failure("invalid_selection"))?;
        let validated = selection.validate(&package).map_err(|_| Failure("invalid_selection"))?;
        NodeInstallPlan::new(&package, &validated, &component)
            .map_err(|_| Failure("unsupported_node_selection"))?
    };
    canonical_json(&plan).map_err(|_| Failure("serialization_failed"))
}

fn main() -> ExitCode {
    let arguments = match std::env::args_os()
        .skip(1)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| Failure("invalid_arguments"))
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("{}", error.0);
            return ExitCode::FAILURE;
        }
    };
    if arguments == ["--help"] {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    match run(arguments) {
        Ok(bytes) => {
            let mut output = std::io::stdout().lock();
            if output
                .write_all(&bytes)
                .and_then(|()| output.write_all(b"\n"))
                .is_err()
            {
                eprintln!("output_unavailable");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.0);
            ExitCode::FAILURE
        }
    }
}
