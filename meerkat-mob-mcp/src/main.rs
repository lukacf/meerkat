use std::io::{self, BufRead, Write};

use meerkat_mob_mcp::handle_rpc_line;
use meerkat_mob_mcp::MobMcpState;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let state = Arc::new(MobMcpState::new(std::env::temp_dir()));

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
