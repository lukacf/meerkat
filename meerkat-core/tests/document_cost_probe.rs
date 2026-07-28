//! Measurement probe: where does the cost of materializing a real production
//! document actually go?
//!
//! Not a gate — an instrument. Ignored by default; point it at a real
//! durable document and run it explicitly:
//!
//! ```text
//! MEERKAT_PROBE_DOC=/path/to/document.json \
//!   cargo test -p meerkat-core --test document_cost_probe -- --ignored --nocapture
//! ```
//!
//! It reports wall time and the content-digest byte attribution table for each
//! phase a resume performs, so "materialization is slow" can be attributed to a
//! pass instead of guessed at.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::Instant;

use meerkat_core::Session;

fn digest_table(label: &str, before: [u64; 8]) {
    let now = meerkat_core::checkpoint::digest_site_bytes();
    let mut printed = false;
    for (i, name) in meerkat_core::checkpoint::DIGEST_SITE_LABELS
        .iter()
        .enumerate()
    {
        let delta = now[i].saturating_sub(before[i]);
        if delta > 0 {
            if !printed {
                println!("    digest bytes by pass ({label}):");
                printed = true;
            }
            println!("      {name:<20} {delta:>12} B");
        }
    }
    if !printed {
        println!("    digest bytes by pass ({label}): none");
    }
}

#[test]
#[ignore = "measurement probe; needs MEERKAT_PROBE_DOC"]
fn document_cost_probe() {
    let path = match std::env::var("MEERKAT_PROBE_DOC") {
        Ok(path) => path,
        Err(_) => {
            println!("set MEERKAT_PROBE_DOC to a durable session document");
            return;
        }
    };
    let bytes = std::fs::read(&path).expect("read probe document");
    println!("\ndocument: {} ({:.1} MB)", path, bytes.len() as f64 / 1e6);

    // 1. Decode — the durable-document ingress every resume pays.
    let mark = meerkat_core::checkpoint::digest_site_bytes();
    let t = Instant::now();
    let session = Session::from_persisted_bytes(&bytes).expect("decode durable document");
    let decode = t.elapsed();
    println!("\n  decode                {decode:>8.2?}");
    digest_table("decode", mark);
    println!("    live messages: {}", session.messages().len());

    // 2. Validate the retained transcript graph (content-addressing).
    let mark = meerkat_core::checkpoint::digest_site_bytes();
    let t = Instant::now();
    let validated = session.validate_transcript_history_state();
    let validate = t.elapsed();
    println!(
        "\n  validate history      {:>8.2?}  ok={}",
        validate,
        validated.is_ok()
    );
    digest_table("validate", mark);

    // 3. Whole-document checkpoint digest — the verification every load runs.
    let mark = meerkat_core::checkpoint::digest_site_bytes();
    let t = Instant::now();
    let state = session.try_checkpoint_state();
    let digest = t.elapsed();
    println!(
        "\n  checkpoint verify     {:>8.2?}  ok={}",
        digest,
        state.is_ok()
    );
    digest_table("checkpoint", mark);

    // 4. Re-encode — what a boundary save costs.
    let mark = meerkat_core::checkpoint::digest_site_bytes();
    let t = Instant::now();
    let encoded = session.to_persisted_bytes().expect("encode");
    let encode = t.elapsed();
    println!(
        "\n  encode                {:>8.2?}  ({:.1} MB)",
        encode,
        encoded.len() as f64 / 1e6
    );
    digest_table("encode", mark);

    let total = decode + validate + digest + encode;
    println!("\n  ---- one materialize round trip: {total:.2?}");
    println!(
        "  encode bytes counter: {} B",
        meerkat_core::checkpoint::global_session_encode_bytes()
    );
}
