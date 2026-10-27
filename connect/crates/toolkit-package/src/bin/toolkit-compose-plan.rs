//! Pure Compose preparation; Docker execution belongs to the selected backend.
use std::io::{self, Read, Write};
use std::process::ExitCode;
use toolkit_package::{
    COMPOSE_MAX_INPUT, canonical_json, prepare_compose_json, prepare_compose_successor_json,
};

const HELP: &str = "toolkit-compose-plan [--successor]\n\nRead one bounded ComposeRequest JSON object from stdin and emit a prepared-only\nplan with exact Compose/file bytes, dependencies and lifecycle order. Explicit\n--successor mode reads a ComposeSuccessorRequest with exact origin, predecessor\nand successor inputs. Ordinary mode never accepts a successor custody marker.\nThe host must admit the exact payload and verify selected tools, engine and images.\nNo Docker operation, authorization, installation or role readiness is implied.\n";

fn run() -> Result<Vec<u8>, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == ["--help"] {
        return Ok(HELP.as_bytes().to_vec());
    }
    let successor = args == ["--successor"];
    if !args.is_empty() && !successor {
        return Err("invalid_arguments".into());
    }
    let mut bytes = Vec::new();
    io::stdin()
        .take((COMPOSE_MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "input_unavailable")?;
    let mut output = if successor {
        canonical_json(&prepare_compose_successor_json(&bytes).map_err(|e| e.to_string())?)
    } else {
        canonical_json(&prepare_compose_json(&bytes).map_err(|e| e.to_string())?)
    }
    .map_err(|_| "output_unavailable")?;
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
