//! No filesystem, Git, registry, credential or cluster access is performed.
use std::io::{self, Read, Write};
use std::process::ExitCode;
use toolkit_package::{GITOPS_MAX_INPUT, canonical_json, prepare_kubernetes_gitops_json};

const HELP: &str = "toolkit-gitops-prepare\n\nRead one bounded KubernetesGitOpsRequest JSON object from stdin and write a\nprepared-only proposal with exact Deployment/ConfigMap replacement bytes.\nThe host must pin profile/base/source inputs and sanitize all nonsecret data.\nNo approval, image verification, Git mutation, deployment or readiness occurs.\n";

fn run() -> Result<Vec<u8>, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == ["--help"] {
        return Ok(HELP.as_bytes().to_vec());
    }
    if !args.is_empty() {
        return Err("invalid_arguments".into());
    }
    let mut bytes = Vec::new();
    io::stdin()
        .take((GITOPS_MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "input_unavailable".to_string())?;
    let result = prepare_kubernetes_gitops_json(&bytes).map_err(|e| e.to_string())?;
    let mut output = canonical_json(&result).map_err(|_| "output_unavailable".to_string())?;
    output.push(b'\n');
    Ok(output)
}

fn main() -> ExitCode {
    match run() {
        Ok(bytes) => match io::stdout().lock().write_all(&bytes) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                eprintln!("output_unavailable");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
