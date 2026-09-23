#[test]
fn real_binary_stdio_contract() {
    let output = std::process::Command::new("python3")
        .arg(
            std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
                .join("tests/mcp_stdio.py"),
        )
        .arg(
            std::env::var_os("CARGO_BIN_EXE_codemob-mcp")
                .expect("CARGO_BIN_EXE_codemob-mcp is set by cargo test"),
        )
        .output()
        .expect("python3 is required for the bounded MCP subprocess harness");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
