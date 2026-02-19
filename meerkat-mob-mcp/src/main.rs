use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use meerkat_mob_mcp::MobMcpState;
use meerkat_mob_mcp::handle_rpc_line;
use std::sync::Arc;

fn default_storage_root() -> PathBuf {
    if let Ok(path) = std::env::var("RKAT_MOB_STATE_ROOT") {
        return PathBuf::from(path);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(".rkat").join("mob")
}

#[tokio::main]
async fn main() {
    let state = Arc::new(MobMcpState::new(default_storage_root()).await);

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let reader = io::BufReader::new(stdin);
    for line_result in reader.lines() {
        match line_result {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                let response = handle_rpc_line(&state, &line).await;
                let _ = writeln!(out, "{}", response);
            }
            Err(_) => break,
        }
    }
}
