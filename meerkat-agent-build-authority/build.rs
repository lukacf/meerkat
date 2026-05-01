fn fnv1a64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let material = format!("{manifest_dir}|{out_dir}|{profile}");
    let bytes = material.as_bytes();

    let words = [
        fnv1a64(0xcbf2_9ce4_8422_2325, bytes),
        fnv1a64(0x6c62_272e_07bb_0142, bytes),
        fnv1a64(0x9e37_79b9_7f4a_7c15, bytes),
        fnv1a64(0x94d0_49bb_1331_11eb, bytes),
    ];

    for (index, word) in words.into_iter().enumerate() {
        println!("cargo:rustc-env=MEERKAT_AGENT_BUILD_AUTHORITY_WORD_{index}={word:016x}");
        println!("cargo:word_{index}={word:016x}");
        println!("cargo:metadata=word_{index}={word:016x}");
    }

    println!("cargo:rerun-if-env-changed=OUT_DIR");
    println!("cargo:rerun-if-env-changed=PROFILE");
}
