//! Binary to emit JSON schema artifacts.
//!
//! Usage: `cargo run -p meerkat-contracts --features schema --bin emit-schemas [-- OUTPUT_DIR]`

#[allow(clippy::print_stdout)] // binary, not library
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let output_dir = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("artifacts/schemas"));
    if args.next().is_some() {
        return Err("emit-schemas accepts at most one output directory".into());
    }
    meerkat_contracts::emit::emit_all_schemas(&output_dir)?;
    println!("Schemas written to {}", output_dir.display());
    Ok(())
}
