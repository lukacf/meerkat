//! Binary to emit JSON schema artifacts.
//!
//! Usage: `cargo run -p meerkat-contracts --features schema --bin emit-schemas`

#[allow(clippy::print_stdout)] // binary, not library
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::path::PathBuf::from("artifacts/schemas");
    meerkat_contracts::emit::emit_all_schemas(&output_dir)?;
    println!("Schemas written to {}", output_dir.display());
    Ok(())
}
