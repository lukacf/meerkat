pub fn normalize_executable_bit(path: &str, bytes: &[u8]) -> bool {
    let normalized_path = path.replace('\\', "/");
    if !normalized_path.starts_with("hooks/") {
        return false;
    }

    normalized_path.ends_with(".sh") || normalized_path.ends_with(".bash") || has_shebang(bytes)
}

fn has_shebang(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_bit_normalization() {
        assert!(normalize_executable_bit("hooks/run.sh", b"echo hi\n"));
        assert!(!normalize_executable_bit(
            "skills/review.md",
            b"#!/bin/sh\n"
        ));
        assert!(normalize_executable_bit(
            "hooks/setup",
            b"#!/usr/bin/env bash\nset -e\n"
        ));
    }
}
