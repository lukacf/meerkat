//! Files in a pack that deserve a second look.
//!
//! A pack holds every regular file under its mob directory. This module
//! points out the packed files that look like secrets, repository metadata,
//! local state or an earlier pack: by their names, and for one form by their
//! content (a file that is exactly a 64-digit hex key, the form of a mobpack
//! signing key). A warning names a path; it never carries a byte of a file.
//! The list is a heuristic: a pack without a warning is not thereby clean.
//! The full list of a pack's files is what
//! [`inspect_archive_bytes`](crate::pack::inspect_archive_bytes) returns.

use std::collections::BTreeMap;
use std::fmt;

/// Why a packed path is pointed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PackedPathConcern {
    /// A `.git` directory, or a `.git` file.
    RepositoryMetadata,
    /// A `.rkat` directory.
    LocalState,
    /// A `.ssh`, `.aws` or `.gnupg` directory.
    CredentialDirectory,
    /// A file whose whole content is 64 hex digits: the form of a mobpack
    /// signing key, whatever the file is called.
    SigningKeyForm,
    /// `.env`, `.env.<name>` other than the template forms, or `<name>.env`.
    EnvFile,
    /// `.npmrc`, `.netrc`, `.pypirc`, `.git-credentials` or `.htpasswd`.
    CredentialFile,
    /// A private key by its name.
    KeyFile,
    /// A `.mobpack` file or `mobpack.bin`.
    Mobpack,
    /// Another directory whose name starts with `.`.
    HiddenDirectory,
    /// Another file whose name starts with `.`.
    HiddenFile,
}

impl PackedPathConcern {
    /// What a warning says about the path.
    pub fn description(self) -> &'static str {
        match self {
            PackedPathConcern::RepositoryMetadata => "repository metadata",
            PackedPathConcern::LocalState => "local Meerkat state and configuration",
            PackedPathConcern::CredentialDirectory => "a directory that usually holds credentials",
            PackedPathConcern::SigningKeyForm => "has the form of a signing key",
            PackedPathConcern::EnvFile => "an environment file, which usually holds secrets",
            PackedPathConcern::CredentialFile => "a file that usually holds credentials",
            PackedPathConcern::KeyFile => "looks like a private key",
            PackedPathConcern::Mobpack => "looks like an earlier pack",
            PackedPathConcern::HiddenDirectory => "a hidden directory",
            PackedPathConcern::HiddenFile => "a hidden file",
        }
    }
}

/// One thing to look at in a pack: a packed file, or a directory with the
/// packed files under it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PackContentWarning {
    /// The path in the pack. A directory ends in `/`.
    pub path: String,
    /// How many packed files the warning stands for: 1 for a file, and for a
    /// directory every packed file under it.
    pub files: usize,
    /// Why the path is pointed out.
    pub concern: PackedPathConcern,
}

impl fmt::Display for PackContentWarning {
    /// `<path> is in the pack: <why>`, with the file count after a directory.
    /// The path is the only thing taken from the pack. Its control characters
    /// and the characters that change how a line is laid out on screen are
    /// printed as escapes, so a file name can neither start a line of its own
    /// nor reorder the one it is on.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for ch in self.path.chars() {
            if ch.is_control() {
                write!(f, "{}", ch.escape_default())?;
            } else if changes_line_layout(ch) {
                write!(f, "{}", ch.escape_unicode())?;
            } else {
                write!(f, "{ch}")?;
            }
        }
        if self.path.ends_with('/') {
            let noun = if self.files == 1 { "file" } else { "files" };
            write!(f, " ({} {noun})", self.files)?;
        }
        write!(f, " is in the pack: {}", self.concern.description())
    }
}

/// The bidirectional and other format characters: invisible themselves, they
/// change the order or the joining of the characters around them.
fn changes_line_layout(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// Directories pointed out as a whole, in one warning.
const FLAGGED_DIRECTORIES: [(&str, PackedPathConcern); 5] = [
    (".git", PackedPathConcern::RepositoryMetadata),
    (".rkat", PackedPathConcern::LocalState),
    (".ssh", PackedPathConcern::CredentialDirectory),
    (".aws", PackedPathConcern::CredentialDirectory),
    (".gnupg", PackedPathConcern::CredentialDirectory),
];

/// `.env.<name>` forms that are templates, not environments.
const ENV_TEMPLATE_NAMES: [&str; 3] = [".env.example", ".env.sample", ".env.template"];

const CREDENTIAL_FILE_NAMES: [&str; 5] = [
    ".npmrc",
    ".netrc",
    ".pypirc",
    ".git-credentials",
    ".htpasswd",
];

const PRIVATE_KEY_FILE_NAMES: [&str; 4] = ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"];

const PRIVATE_KEY_SUFFIXES: [&str; 4] = [".pem", ".key", ".p12", ".pfx"];

/// Classifies one packed path by its name.
///
/// Returns the path the warning is about (the file itself, or the directory
/// that is pointed out as a whole, ending in `/`) and why. `None` for a name
/// the list does not know, which says nothing about the file's content.
/// Names are compared without regard to ASCII case.
pub fn classify_packed_path(path: &str) -> Option<(String, PackedPathConcern)> {
    classify(path, false)
}

/// Classifies one packed file by its name and, for the one content form, by
/// its content: a file that [has the form of a signing
/// key](has_signing_key_form) is pointed out whatever it is called, unless a
/// flagged directory already stands for it.
pub fn classify_packed_file(path: &str, content: &[u8]) -> Option<(String, PackedPathConcern)> {
    classify(path, has_signing_key_form(content))
}

/// Whether `content` is exactly a 64-digit hex key: 64 ASCII hex digits and
/// at most one line end (`\n` or `\r\n`) after them, nothing else. That is
/// the form of the key file `rkat mob pack --sign` takes.
pub fn has_signing_key_form(content: &[u8]) -> bool {
    let digits = content
        .strip_suffix(b"\r\n")
        .or_else(|| content.strip_suffix(b"\n"))
        .unwrap_or(content);
    digits.len() == 64 && digits.iter().all(u8::is_ascii_hexdigit)
}

fn classify(path: &str, signing_key_form: bool) -> Option<(String, PackedPathConcern)> {
    let components: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let (file_name, directories) = components.split_last()?;

    for (depth, directory) in directories.iter().enumerate() {
        let lowered = directory.to_ascii_lowercase();
        if let Some((_, concern)) = FLAGGED_DIRECTORIES
            .iter()
            .find(|(name, _)| *name == lowered)
        {
            return Some((directory_path(directories, depth), *concern));
        }
    }

    if signing_key_form {
        return Some((path.to_string(), PackedPathConcern::SigningKeyForm));
    }

    let name = file_name.to_ascii_lowercase();
    if let Some(concern) = classify_file_name(&name) {
        return Some((path.to_string(), concern));
    }

    if let Some(depth) = directories.iter().position(|part| part.starts_with('.')) {
        return Some((
            directory_path(directories, depth),
            PackedPathConcern::HiddenDirectory,
        ));
    }
    if name.starts_with('.') {
        return Some((path.to_string(), PackedPathConcern::HiddenFile));
    }
    None
}

/// The path of the directory at `depth`, ending in `/`.
fn directory_path(directories: &[&str], depth: usize) -> String {
    let mut path = String::new();
    for directory in directories.iter().take(depth + 1) {
        path.push_str(directory);
        path.push('/');
    }
    path
}

/// `name` is a file name in ASCII lower case.
fn classify_file_name(name: &str) -> Option<PackedPathConcern> {
    if name == ".git" {
        return Some(PackedPathConcern::RepositoryMetadata);
    }
    // `.env` itself, `prod.env`, and `.env.<name>` other than the templates.
    if name.ends_with(".env") || (name.starts_with(".env.") && !ENV_TEMPLATE_NAMES.contains(&name))
    {
        return Some(PackedPathConcern::EnvFile);
    }
    if CREDENTIAL_FILE_NAMES.contains(&name) {
        return Some(PackedPathConcern::CredentialFile);
    }
    if name.ends_with(".mobpack") || name == "mobpack.bin" {
        return Some(PackedPathConcern::Mobpack);
    }
    if is_private_key_name(name) {
        return Some(PackedPathConcern::KeyFile);
    }
    None
}

/// A private key by name: the usual SSH key names, the usual key container
/// suffixes, and a name that is `key`, or ends in `-key`, `_key` or `.key`,
/// before its extension (`key.hex`, `signing-key.txt`). A `.pub` file is a
/// public key and is not pointed out.
fn is_private_key_name(name: &str) -> bool {
    if name.ends_with(".pub") {
        return false;
    }
    if PRIVATE_KEY_FILE_NAMES.contains(&name)
        || PRIVATE_KEY_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
    {
        return true;
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem == "key"
        || ["-key", "_key", ".key"]
            .iter()
            .any(|end| stem.ends_with(end))
}

/// The warnings for a pack's files, given as path and content: one per
/// pointed-out file or directory, in path order. `signature.toml` and every
/// other file the list does not know give no warning. A hidden directory's
/// count is the number of packed files under it, also when some of them have
/// a warning of their own.
pub fn pack_content_warnings<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Vec<PackContentWarning> {
    let mut paths = Vec::new();
    let mut found: BTreeMap<String, (usize, PackedPathConcern)> = BTreeMap::new();
    for (path, content) in files {
        paths.push(path);
        if let Some((warned, concern)) = classify_packed_file(path, content) {
            found.entry(warned).or_insert((0, concern)).0 += 1;
        }
    }
    found
        .into_iter()
        .map(|(path, (files, concern))| {
            let files = if concern == PackedPathConcern::HiddenDirectory {
                paths
                    .iter()
                    .filter(|packed| packed.starts_with(&path))
                    .count()
            } else {
                files
            };
            PackContentWarning {
                path,
                files,
                concern,
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn concern_of(path: &str) -> Option<PackedPathConcern> {
        classify_packed_path(path).map(|(_, concern)| concern)
    }

    #[test]
    fn test_pack_sources_and_ordinary_files_give_no_warning() {
        for path in [
            "manifest.toml",
            "definition.json",
            "signature.toml",
            "skills/notes.md",
            "hooks/run.sh",
            "config/defaults.toml",
            "adaptive/policies.toml",
            "schemas/registry.json",
            "docs/key-concepts.md",
            "skills/monkey.md",
            "keys.md",
            "environment.md",
            "id_rsa.pub",
            "signing-key.pub",
        ] {
            assert_eq!(concern_of(path), None, "{path}");
        }
    }

    #[test]
    fn test_env_files_are_pointed_out_and_templates_are_hidden_files() {
        for path in [".env", ".env.local", ".env.production", "config/.ENV"] {
            assert_eq!(concern_of(path), Some(PackedPathConcern::EnvFile), "{path}");
        }
        for path in [".env.example", ".env.sample", ".env.template"] {
            assert_eq!(
                concern_of(path),
                Some(PackedPathConcern::HiddenFile),
                "{path}"
            );
        }
    }

    #[test]
    fn test_key_like_names_are_pointed_out() {
        for path in [
            "id_rsa",
            "id_dsa",
            "id_ecdsa",
            "keys/id_ed25519",
            "release.key",
            "signing.key",
            "tls/server.pem",
            "cert.P12",
            "cert.pfx",
            "key.hex",
            "signing-key.txt",
            "private_key",
            "skills/api_key.json",
        ] {
            assert_eq!(concern_of(path), Some(PackedPathConcern::KeyFile), "{path}");
        }
    }

    #[test]
    fn test_credential_files_and_earlier_packs_are_pointed_out() {
        for path in [
            ".npmrc",
            ".netrc",
            ".pypirc",
            ".git-credentials",
            "web/.htpasswd",
        ] {
            assert_eq!(
                concern_of(path),
                Some(PackedPathConcern::CredentialFile),
                "{path}"
            );
        }
        for path in ["out.mobpack", "dist/release.MOBPACK", "web/mobpack.bin"] {
            assert_eq!(concern_of(path), Some(PackedPathConcern::Mobpack), "{path}");
        }
    }

    #[test]
    fn test_flagged_directories_are_pointed_out_as_a_whole() {
        assert_eq!(
            classify_packed_path(".git/objects/ab/cdef"),
            Some((".git/".to_string(), PackedPathConcern::RepositoryMetadata))
        );
        assert_eq!(
            classify_packed_path("vendor/tool/.git/HEAD"),
            Some((
                "vendor/tool/.git/".to_string(),
                PackedPathConcern::RepositoryMetadata
            ))
        );
        assert_eq!(
            classify_packed_path(".rkat/sessions/chat.jsonl"),
            Some((".rkat/".to_string(), PackedPathConcern::LocalState))
        );
        for path in [".ssh/id_rsa", ".aws/credentials", ".gnupg/pubring.kbx"] {
            assert_eq!(
                concern_of(path),
                Some(PackedPathConcern::CredentialDirectory),
                "{path}"
            );
        }
        // A `.git` file (a linked worktree's pointer) is metadata too.
        assert_eq!(
            classify_packed_path(".git"),
            Some((".git".to_string(), PackedPathConcern::RepositoryMetadata))
        );
    }

    #[test]
    fn test_other_hidden_entries_are_pointed_out_once_per_directory_or_file() {
        assert_eq!(
            classify_packed_path(".vscode/settings.json"),
            Some((".vscode/".to_string(), PackedPathConcern::HiddenDirectory))
        );
        assert_eq!(
            classify_packed_path("skills/.cache/a/b.bin"),
            Some((
                "skills/.cache/".to_string(),
                PackedPathConcern::HiddenDirectory
            ))
        );
        assert_eq!(
            classify_packed_path("skills/.DS_Store"),
            Some((
                "skills/.DS_Store".to_string(),
                PackedPathConcern::HiddenFile
            ))
        );
        // A known name inside a hidden directory keeps its own reason.
        assert_eq!(
            classify_packed_path(".secrets/id_rsa"),
            Some((".secrets/id_rsa".to_string(), PackedPathConcern::KeyFile))
        );
    }

    #[test]
    fn test_flagged_directory_names_are_matched_without_regard_to_case() {
        assert_eq!(
            classify_packed_path(".GIT/HEAD"),
            Some((".GIT/".to_string(), PackedPathConcern::RepositoryMetadata))
        );
        assert_eq!(
            classify_packed_path("sub/.Rkat/sessions/chat.jsonl"),
            Some(("sub/.Rkat/".to_string(), PackedPathConcern::LocalState))
        );
        assert_eq!(
            concern_of(".SSH/config"),
            Some(PackedPathConcern::CredentialDirectory)
        );
    }

    #[test]
    fn test_the_top_most_hidden_directory_is_the_one_pointed_out() {
        assert_eq!(
            classify_packed_path(".a/.b/x"),
            Some((".a/".to_string(), PackedPathConcern::HiddenDirectory))
        );
        assert_eq!(
            classify_packed_path("s/.a/t/.b/x"),
            Some(("s/.a/".to_string(), PackedPathConcern::HiddenDirectory))
        );
    }

    /// Files by name alone: each with a content that has no form.
    fn by_name<'a>(paths: &'a [&'a str]) -> impl Iterator<Item = (&'a str, &'a [u8])> {
        paths.iter().map(|path| (*path, &b"text\n"[..]))
    }

    fn lines(warnings: &[PackContentWarning]) -> Vec<String> {
        warnings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn test_warnings_are_one_per_path_in_path_order_with_file_counts() {
        let warnings = pack_content_warnings(by_name(&[
            "manifest.toml",
            "release.key",
            ".git/HEAD",
            ".git/config",
            ".env",
            "definition.json",
            "signature.toml",
        ]));
        assert_eq!(
            lines(&warnings),
            [
                ".env is in the pack: an environment file, which usually holds secrets",
                ".git/ (2 files) is in the pack: repository metadata",
                "release.key is in the pack: looks like a private key",
            ]
        );
        assert_eq!(warnings.iter().map(|w| w.files).sum::<usize>(), 4);
        assert!(pack_content_warnings(by_name(&["manifest.toml", "definition.json"])).is_empty());
    }

    #[test]
    fn test_warning_text_escapes_control_characters_of_a_path() {
        let warnings = pack_content_warnings(by_name(&[".env.a\nwarning\tforged"]));
        let line = warnings[0].to_string();
        assert!(!line.contains('\n') && !line.contains('\t'), "{line}");
        assert!(line.starts_with(".env.a\\nwarning\\tforged is in the pack"));
    }

    #[test]
    fn test_warning_text_prints_format_characters_as_code_points() {
        let warnings = pack_content_warnings(by_name(&[
            "a\u{202e}b.key",
            "c\u{200b}d\u{feff}.key",
            "e\u{2066}f\u{2069}.key",
            "plain-\u{e9}-\u{4e2d}.key",
        ]));
        assert_eq!(
            lines(&warnings),
            [
                "a\\u{202e}b.key is in the pack: looks like a private key",
                "c\\u{200b}d\\u{feff}.key is in the pack: looks like a private key",
                "e\\u{2066}f\\u{2069}.key is in the pack: looks like a private key",
                "plain-\u{e9}-\u{4e2d}.key is in the pack: looks like a private key",
            ]
        );
    }

    #[test]
    fn test_env_files_are_pointed_out_by_suffix_too() {
        for path in ["prod.env", "settings/production.env", "LOCAL.ENV"] {
            assert_eq!(concern_of(path), Some(PackedPathConcern::EnvFile), "{path}");
        }
        for path in ["environment.md", "env", "prod.envrc", "notes.env.md"] {
            assert_eq!(concern_of(path), None, "{path}");
        }
    }

    #[test]
    fn test_a_hidden_directory_counts_every_file_under_it() {
        let warnings = pack_content_warnings(by_name(&[
            ".config/a.txt",
            ".config/token.key",
            ".config/deep/b.txt",
            ".configuration/c.txt",
            "manifest.toml",
        ]));
        assert_eq!(
            lines(&warnings),
            [
                ".config/ (3 files) is in the pack: a hidden directory",
                ".config/token.key is in the pack: looks like a private key",
                ".configuration/ (1 file) is in the pack: a hidden directory",
            ]
        );
        // A hidden directory whose every file has a line of its own gets none.
        let warnings = pack_content_warnings(by_name(&[".secrets/id_rsa"]));
        assert_eq!(
            lines(&warnings),
            [".secrets/id_rsa is in the pack: looks like a private key"]
        );
    }

    /// 64 hex digits made up here.
    fn hex_key() -> String {
        (0u8..32)
            .map(|index| index.wrapping_mul(11).wrapping_add(7))
            .flat_map(|byte| [byte >> 4, byte & 0x0f])
            .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
            .collect()
    }

    #[test]
    fn test_signing_key_form_is_exactly_64_hex_digits_and_at_most_one_line_end() {
        let key = hex_key();
        for content in [
            key.clone(),
            format!("{key}\n"),
            format!("{key}\r\n"),
            key.to_ascii_uppercase(),
        ] {
            assert!(has_signing_key_form(content.as_bytes()), "{content:?}");
        }
        for content in [
            String::new(),
            key[1..].to_string(),
            format!("{key}0"),
            format!("{key}\n\n"),
            format!("{key}\r"),
            format!(" {key}"),
            format!("{key} \n"),
            format!("{}g", &key[1..]),
            format!("{key}\n{key}\n"),
        ] {
            assert!(!has_signing_key_form(content.as_bytes()), "{content:?}");
        }
    }

    #[test]
    fn test_a_file_with_the_form_of_a_signing_key_is_pointed_out_whatever_its_name() {
        let key = hex_key();
        let key_line = format!("{key}\n");
        let files: [(&str, &[u8]); 7] = [
            ("release.hex", key.as_bytes()),
            ("signer.txt", key_line.as_bytes()),
            ("keys/prod.hex", key.as_bytes()),
            // The content form says more than the name does.
            ("key.hex", key.as_bytes()),
            // A flagged directory stands for every file under it.
            (".git/objects/aa", key.as_bytes()),
            // A hidden directory does not hide the form.
            (".config/token", key.as_bytes()),
            ("skills/notes.md", b"# Notes\n"),
        ];
        let warnings = pack_content_warnings(files);
        assert_eq!(
            lines(&warnings),
            [
                ".config/token is in the pack: has the form of a signing key",
                ".git/ (1 file) is in the pack: repository metadata",
                "key.hex is in the pack: has the form of a signing key",
                "keys/prod.hex is in the pack: has the form of a signing key",
                "release.hex is in the pack: has the form of a signing key",
                "signer.txt is in the pack: has the form of a signing key",
            ]
        );
        // No line carries a digit run of the content.
        assert!(
            lines(&warnings)
                .iter()
                .all(|line| !line.contains(&key[..16]))
        );
        // By name alone the same file is a key by its name, or nothing.
        assert_eq!(concern_of("key.hex"), Some(PackedPathConcern::KeyFile));
        assert_eq!(concern_of("release.hex"), None);
    }
}
