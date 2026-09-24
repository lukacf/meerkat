#![allow(clippy::expect_used)]

//! B-7 V4: builtin skill tools receive a typed `SkillKey` directly at the
//! ingress boundary; there is no slash-string path parsing. Malformed input
//! raises `BuiltinToolError::InvalidArgs`.

use std::fs;
use std::path::Path;

fn read(path: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    let full = Path::new(root).join(path);
    fs::read_to_string(full).expect("read source")
}

#[test]
fn builtin_skill_tools_have_no_slash_string_parsing() {
    for file in [
        "src/builtin/skills/browse.rs",
        "src/builtin/skills/load.rs",
        "src/builtin/skills/resources.rs",
        "src/builtin/skills/functions.rs",
    ] {
        let body = read(file);
        assert!(
            !body.contains("SkillId("),
            "{file} must not construct SkillId anywhere",
        );
        assert!(
            !body.contains("split('/')") && !body.contains("splitn(2, '/')"),
            "{file} must not split-parse skill identifiers on '/'",
        );
    }
}

#[test]
fn builtin_skill_tools_accept_typed_source_uuid_and_skill_name() {
    // Every skill tool must accept the `source_uuid` + `skill_name` tuple as
    // its ingress shape — not a single `id` string.
    for file in [
        "src/builtin/skills/load.rs",
        "src/builtin/skills/resources.rs",
        "src/builtin/skills/functions.rs",
    ] {
        let body = read(file);
        assert!(
            body.contains("SourceUuid::parse"),
            "{file} must parse source_uuid via SourceUuid::parse at ingress",
        );
        assert!(
            body.contains("SkillName::parse"),
            "{file} must parse skill_name via SkillName::parse at ingress",
        );
    }
}
