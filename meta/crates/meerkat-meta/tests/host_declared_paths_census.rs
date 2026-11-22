//! Census of the factories that build members' file tools.
//!
//! Meerkat's in-process file tools (`apply_patch`, `blob_save_file`) do not
//! write a path their host declares as its own, and Meta's compositions
//! declare theirs through one helper,
//! `gateway_composition::declare_host_paths`, called on every agent factory
//! a composition builds its members from. This guard reads the crate's
//! production sources and states where those factories are constructed: each
//! construction is followed, in its own function, by a call of the helper.
//! A construction added somewhere else, or one whose call is removed, fails
//! it; the failure names the place.
//!
//! Test code is out of scope: the scan skips `#[cfg(test)]` items and test
//! source files, and it reads code only, never string, char or comment
//! contents.

#![allow(clippy::panic, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// The constructors of an agent factory.
const CONSTRUCTORS: &[&str] = &["AgentFactory::new(", "AgentFactory::minimal("];
/// The helper every member factory is declared through.
const DECLARATION: &str = "declare_host_paths(";

/// Where a member factory is constructed and declared: the source file under
/// `src/` and the function, once per construction.
const DECLARED: &[(&str, &str)] = &[
    ("bin/meta_gateway.rs", "build_persistent_session_service"),
    ("bin/meta_gateway.rs", "run"),
    ("mob_handle_runtime.rs", "ephemeral_inner"),
    (
        "mob_handle_runtime.rs",
        "ephemeral_runtime_backed_with_provider_stores",
    ),
    (
        "mob_handle_runtime.rs",
        "persistent_inner_with_provider_stores",
    ),
    ("rpc_gateway.rs", "run_persistent_inner"),
    ("rpc_gateway.rs", "run_persistent_inner"),
];

/// Factories that build no agent, so no file tool: the source file, the
/// function, and what the factory is for.
const WITHOUT_FILE_TOOLS: &[(&str, &str, &str)] = &[(
    "memory/factory_handle.rs",
    "for_model",
    "resolves the model clients of the memory stages; it builds no agent",
)];

fn is_ident(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The source with every comment and every string or char literal blanked
/// to spaces, newlines kept, so braces and calls are read from code only.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let blank = |character: char| if character == '\n' { '\n' } else { ' ' };
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                out.push(' ');
                index += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0usize;
            while index < chars.len() {
                let pair = (chars[index], chars.get(index + 1).copied());
                if pair == ('/', Some('*')) {
                    depth += 1;
                    out.push_str("  ");
                    index += 2;
                } else if pair == ('*', Some('/')) {
                    depth -= 1;
                    out.push_str("  ");
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[index]));
                    index += 1;
                }
            }
            continue;
        }
        let starts_raw = character == 'r'
            && (index == 0
                || !is_ident(chars[index - 1])
                || (chars[index - 1] == 'b' && (index < 2 || !is_ident(chars[index - 2]))));
        if starts_raw {
            let mut hashes = 0;
            while chars.get(index + 1 + hashes) == Some(&'#') {
                hashes += 1;
            }
            if chars.get(index + 1 + hashes) == Some(&'"') {
                let open = 2 + hashes;
                out.push_str(&" ".repeat(open));
                index += open;
                while index < chars.len() {
                    if chars[index] == '"'
                        && (1..=hashes).all(|offset| chars.get(index + offset) == Some(&'#'))
                    {
                        out.push_str(&" ".repeat(1 + hashes));
                        index += 1 + hashes;
                        break;
                    }
                    out.push(blank(chars[index]));
                    index += 1;
                }
                continue;
            }
        }
        if character == '"' {
            out.push(' ');
            index += 1;
            while index < chars.len() {
                match chars[index] {
                    '\\' => {
                        out.push(' ');
                        if let Some(escaped) = chars.get(index + 1) {
                            out.push(blank(*escaped));
                        }
                        index += 2;
                    }
                    '"' => {
                        out.push(' ');
                        index += 1;
                        break;
                    }
                    other => {
                        out.push(blank(other));
                        index += 1;
                    }
                }
            }
            continue;
        }
        if character == '\'' {
            // A char literal, not a lifetime: `'x'`, or an escape such as
            // `'\''`, `'\n'` or `'\u{7b}'`.
            let end = if next == Some('\\') {
                (index + 2..chars.len().min(index + 12))
                    .find(|&at| chars[at] == '\'' && at > index + 2)
            } else if chars.get(index + 2) == Some(&'\'') {
                Some(index + 2)
            } else {
                None
            };
            if let Some(end) = end {
                out.push_str(&" ".repeat(end + 1 - index));
                index = end + 1;
                continue;
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// The code with every `#[cfg(test)]` item removed: the attribute, any
/// attributes after it, and the item through its closing brace (or its `;`).
fn production_code(source: &str) -> String {
    let code = code_only(source);
    let lines: Vec<&str> = code.lines().collect();
    let mut kept = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test") {
            let mut item = index + 1;
            while item < lines.len() && lines[item].trim_start().starts_with("#[") {
                item += 1;
            }
            // A one-line item: a `use`, a statement, a field or a match arm.
            if item < lines.len() {
                let first = lines[item].trim_end();
                if first.ends_with(';') || (first.ends_with(',') && !first.contains('{')) {
                    index = item + 1;
                    continue;
                }
            }
            let start = item;
            let mut depth: i64 = 0;
            let mut opened = false;
            while item < lines.len() {
                for character in lines[item].chars() {
                    match character {
                        '{' => {
                            depth += 1;
                            opened = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if opened && depth <= 0 {
                    break;
                }
                if !opened && depth < 0 {
                    // The enclosing block closed before the item opened one:
                    // the item was its first line only.
                    item = start;
                    break;
                }
                item += 1;
            }
            index = item + 1;
            continue;
        }
        kept.push(lines[index]);
        index += 1;
    }
    kept.join("\n")
}

/// The name of the function a line declares, when the line opens with a
/// function's header (`fn name`, after its visibility and qualifiers).
fn function_name(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    loop {
        let qualifier = [
            "pub(crate) ",
            "pub(super) ",
            "pub ",
            "async ",
            "const ",
            "unsafe ",
        ]
        .into_iter()
        .find(|qualifier| rest.starts_with(*qualifier));
        match qualifier {
            Some(qualifier) => rest = &rest[qualifier.len()..],
            None => break,
        }
    }
    let name: String = rest
        .strip_prefix("fn ")?
        .chars()
        .take_while(|character| is_ident(*character))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// One construction of an agent factory in production code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Construction {
    /// The source file, relative to `src/`.
    file: String,
    /// The function the construction stands in.
    function: String,
    /// Whether a call of the helper follows it in that function, before
    /// the function's next construction.
    declared: bool,
}

/// Every construction of an agent factory in the production code of
/// `source`, with the function it stands in.
fn constructions_in(file: &str, source: &str) -> Vec<Construction> {
    let code = production_code(source);
    let mut found: Vec<Construction> = Vec::new();
    // The functions whose bodies are open, innermost last, each with the
    // brace depth its body opened at and its latest construction.
    let mut open: Vec<(String, i64, Option<usize>)> = Vec::new();
    let mut pending: Option<String> = None;
    let mut depth: i64 = 0;
    let mut parens: i64 = 0;
    for line in code.lines() {
        if let Some(name) = function_name(line) {
            pending = Some(name);
            parens = 0;
        }
        let constructs = CONSTRUCTORS
            .iter()
            .any(|constructor| line.contains(constructor));
        let declares = line.contains(DECLARATION) && function_name(line).is_none();
        for character in line.chars() {
            match character {
                '(' => parens += 1,
                ')' => parens -= 1,
                '{' => {
                    depth += 1;
                    if parens == 0
                        && let Some(name) = pending.take()
                    {
                        open.push((name, depth, None));
                    }
                }
                '}' => {
                    if open.last().is_some_and(|(_, opened, _)| *opened == depth) {
                        open.pop();
                    }
                    depth -= 1;
                }
                ';' if parens == 0 => pending = None,
                _ => {}
            }
        }
        let Some((function, _, latest)) = open.last_mut() else {
            continue;
        };
        if constructs {
            *latest = Some(found.len());
            found.push(Construction {
                file: file.to_owned(),
                function: function.clone(),
                declared: false,
            });
        }
        if declares && let Some(at) = latest.take() {
            found[at].declared = true;
        }
    }
    found
}

/// Whether `path`, relative to `src/`, is a test source file: a module the
/// crate compiles for its own tests only.
fn is_test_source(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        let stem = name.strip_suffix(".rs").unwrap_or(&name);
        stem == "tests" || stem.ends_with("_tests") || stem.starts_with("test_")
    })
}

/// Every `.rs` file under `directory`.
fn rust_sources(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// Every construction of an agent factory in the crate's production code.
fn crate_constructions() -> Vec<Construction> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in files {
        let relative = path.strip_prefix(&src).expect("a file under src");
        if is_test_source(relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        found.extend(constructions_in(
            &relative.to_string_lossy().replace('\\', "/"),
            &source,
        ));
    }
    found.sort();
    found
}

/// Every factory a composition builds its members from is constructed in
/// one of the listed functions and declared there; the only other factory
/// in production code builds no agent.
#[test]
fn every_member_factory_declares_its_host_paths() {
    let mut stated: Vec<Construction> = DECLARED
        .iter()
        .map(|(file, function)| Construction {
            file: (*file).to_owned(),
            function: (*function).to_owned(),
            declared: true,
        })
        .chain(
            WITHOUT_FILE_TOOLS
                .iter()
                .map(|(file, function, _)| Construction {
                    file: (*file).to_owned(),
                    function: (*function).to_owned(),
                    declared: false,
                }),
        )
        .collect();
    stated.sort();
    let found = crate_constructions();
    assert_eq!(
        found, stated,
        "the constructions of an agent factory in production code (found, then stated): a \
         factory that builds members calls gateway_composition::declare_host_paths in the \
         function that constructs it and is listed in DECLARED; one that builds no agent is \
         listed in WITHOUT_FILE_TOOLS with what it is for"
    );
}

/// The scan reports a construction without the declaration, takes one with
/// it, tells two constructions of one function apart, and reads neither
/// test code nor the contents of strings and comments.
#[test]
fn the_census_tells_a_declared_construction_from_an_undeclared_one() {
    let source = concat!(
        "#[cfg(test)]\n",
        "mod tests {\n",
        "    const OPEN: &str = \"{\";\n",
        "    fn fixture() { let _ = AgentFactory::new(dir()); }\n",
        "}\n",
        "// AgentFactory::new( in a comment\n",
        "const TEXT: &str = \"AgentFactory::new( in a string\";\n",
        "pub(crate) async fn declared(\n",
        "    path: PathBuf,\n",
        ") -> Result<(), Error> {\n",
        "    let mut factory = AgentFactory::new(&path).builtins(true);\n",
        "    if let Some(root) = root {\n",
        "        factory = factory.user_config_root(root);\n",
        "    }\n",
        "    factory = declare_host_paths(factory, [path], []);\n",
        "    Ok(())\n",
        "}\n",
        "fn undeclared() {\n",
        "    let factory = meerkat::AgentFactory::minimal();\n",
        "}\n",
        "fn twice() {\n",
        "    let first = AgentFactory::new(a());\n",
        "    let first = declare_host_paths(first, [], []);\n",
        "    let second = AgentFactory::new(b());\n",
        "}\n",
    );
    let found: Vec<(String, bool)> = constructions_in("sample.rs", source)
        .into_iter()
        .map(|construction| (construction.function, construction.declared))
        .collect();
    assert_eq!(
        found,
        vec![
            ("declared".to_owned(), true),
            ("undeclared".to_owned(), false),
            ("twice".to_owned(), true),
            ("twice".to_owned(), false),
        ]
    );
}
