//! The SDKs' closed vocabularies are this module's.
//!
//! The Python SDK (`meerkat-meta`) and the TypeScript SDK (`@rkat/meta-sdk`)
//! are written by hand, and each lists the family's closed vocabularies for
//! its callers: the refusal kinds, the outcome tokens, an instruction
//! outcome's kinds and reasons, and a seat. A kind added here and not there
//! still reaches an SDK host (the SDKs keep an unknown token), but no host
//! can match on it, and adding one is a breaking change of the SDKs. This
//! row fails the Rust change that makes the lists differ; the SDK suites
//! hold the same comparison from their side.
//!
//! The Rust lists are each type's `ALL_TOKENS`, which an exhaustive match
//! keeps complete. The SDK files live outside this crate, so they are read
//! at run time from the repository, not compiled in.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::{
    AgentSeat, IdentityOutcome, IdentityRefusalKind, InstructionNextSessionReason,
    InstructionOutcome,
};

const PYTHON_ERRORS: &str = "sdk/python/meerkat_meta/errors.py";
const PYTHON_IDENTITIES: &str = "sdk/python/meerkat_meta/identities.py";
const TYPESCRIPT_ERRORS: &str = "sdk/typescript/src/errors.ts";
const TYPESCRIPT_IDENTITIES: &str = "sdk/typescript/src/identities.ts";

fn meta_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("meerkat-meta sits under crates/ at Meta's root")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = meta_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {} failed: {error}", path.display()))
}

/// Every double-quoted token of `source` between the one occurrence of
/// `header` and the next `footer`.
fn declared(file: &str, source: &str, header: &str, footer: &str) -> BTreeSet<String> {
    let mut parts = source.split(header);
    let (Some(_), Some(after), None) = (parts.next(), parts.next(), parts.next()) else {
        panic!("{file} declares `{header}` exactly once");
    };
    let body = after
        .split(footer)
        .next()
        .expect("a split yields its first piece");
    let tokens: BTreeSet<String> = body
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    assert!(!tokens.is_empty(), "{file}: `{header}` lists no token");
    tokens
}

fn python(file: &str, name: &str) -> BTreeSet<String> {
    declared(
        file,
        &read(file),
        &format!("\n{name}: frozenset[str] = frozenset("),
        ")\n",
    )
}

fn typescript(file: &str, name: &str) -> BTreeSet<String> {
    declared(
        file,
        &read(file),
        &format!("\nexport const {name} = ["),
        "] as const;",
    )
}

fn rust(tokens: &[&str]) -> BTreeSet<String> {
    let listed: BTreeSet<String> = tokens.iter().map(|token| (*token).to_owned()).collect();
    assert_eq!(listed.len(), tokens.len(), "a token is listed once");
    listed
}

/// One closed vocabulary: the family's tokens, and where each SDK lists
/// them.
struct Vocabulary {
    what: &'static str,
    family: &'static [&'static str],
    python: (&'static str, &'static str),
    typescript: (&'static str, &'static str),
}

#[test]
fn the_sdks_closed_vocabularies_are_the_familys() {
    let vocabularies = [
        Vocabulary {
            what: "refusal kinds",
            family: IdentityRefusalKind::ALL_TOKENS,
            python: (PYTHON_ERRORS, "IDENTITY_REFUSAL_KINDS"),
            typescript: (TYPESCRIPT_ERRORS, "IDENTITY_REFUSAL_KIND_TOKENS"),
        },
        Vocabulary {
            what: "outcome tokens",
            family: IdentityOutcome::ALL_TOKENS,
            python: (PYTHON_IDENTITIES, "IDENTITY_OUTCOMES"),
            typescript: (TYPESCRIPT_IDENTITIES, "IDENTITY_OUTCOMES"),
        },
        Vocabulary {
            what: "instruction outcome kinds",
            family: InstructionOutcome::ALL_TOKENS,
            python: (PYTHON_IDENTITIES, "INSTRUCTION_OUTCOME_KINDS"),
            typescript: (TYPESCRIPT_IDENTITIES, "INSTRUCTION_OUTCOME_KINDS"),
        },
        Vocabulary {
            what: "instruction next-session reasons",
            family: InstructionNextSessionReason::ALL_TOKENS,
            python: (PYTHON_IDENTITIES, "INSTRUCTION_NEXT_SESSION_REASONS"),
            typescript: (TYPESCRIPT_IDENTITIES, "INSTRUCTION_NEXT_SESSION_REASONS"),
        },
        Vocabulary {
            what: "seats",
            family: AgentSeat::ALL_TOKENS,
            python: (PYTHON_IDENTITIES, "AGENT_SEATS"),
            typescript: (TYPESCRIPT_IDENTITIES, "AGENT_SEATS"),
        },
    ];
    for vocabulary in vocabularies {
        let what = vocabulary.what;
        let family = rust(vocabulary.family);
        assert_eq!(
            python(vocabulary.python.0, vocabulary.python.1),
            family,
            "the Python SDK's {what} differ from the family's: change both, and the change is \
             Breaking for the SDK"
        );
        assert_eq!(
            typescript(vocabulary.typescript.0, vocabulary.typescript.1),
            family,
            "the TypeScript SDK's {what} differ from the family's: change both, and the change \
             is Breaking for the SDK"
        );
    }
    assert_eq!(IdentityRefusalKind::ALL_TOKENS.len(), 57);
}
