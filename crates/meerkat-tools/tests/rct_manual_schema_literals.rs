//! Dogma gate: no production code in the workspace writes a tool's
//! `input_schema` as a manual `json!` literal.
//!
//! Workspace-only: it scans every crate of the repository from this crate's
//! grandparent, which the published package does not have (from the package
//! cache it would scan every other crate there), so the package leaves this
//! file out (ci/standalone-workspace-only.toml).

#![allow(clippy::panic, clippy::unwrap_used)]

use std::path::Path;

#[test]
fn test_rct_contracts_no_manual_tool_schema_literals() -> Result<(), Box<dyn std::error::Error>> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_root
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("missing workspace root")?
        .to_path_buf();

    let mut offenders = Vec::new();
    scan_for_manual_input_schema_literals(&workspace_root, &workspace_root, &mut offenders)?;

    assert!(
        offenders.is_empty(),
        "Manual tool schema literals found:\n{}",
        offenders.join("\n")
    );
    Ok(())
}

/// Whether `path` is a whole-file module the PARENT declares under
/// `#[cfg(test)]`, or under a compound `#[cfg(..)]` that still holds only in
/// test builds (see [`carries_cfg_test_only_attribute`]).
///
/// The scanner needs this because such a file carries no in-file
/// `#[cfg(test)]` for the line tracking below to observe. Answering it from
/// the `_tests.rs` filename alone would be a permanent hole rather than an
/// exemption: any production file that later adopts the name would leave this
/// dogma gate silently, and nothing would say so. The declaration is cheap to
/// verify, so it is verified.
fn declared_as_cfg_test_module(path: &Path) -> std::io::Result<bool> {
    let (Some(stem), Some(dir)) = (path.file_stem().and_then(|s| s.to_str()), path.parent()) else {
        return Ok(false);
    };

    let mut parents: Vec<std::path::PathBuf> = ["mod.rs", "lib.rs", "main.rs"]
        .iter()
        .map(|name| dir.join(name))
        .collect();
    // The 2018-edition sibling form: `foo.rs` beside the `foo/` directory.
    if let (Some(dir_name), Some(grandparent)) =
        (dir.file_name().and_then(|name| name.to_str()), dir.parent())
    {
        parents.push(grandparent.join(format!("{dir_name}.rs")));
    }

    let declaration = format!("mod {stem};");
    for parent in parents {
        let Ok(contents) = std::fs::read_to_string(&parent) else {
            continue;
        };
        // The test-only cfg must still be in force at the declaration: it may
        // be followed by further attributes or comments, but any other item
        // between the two means the attribute belonged to that item instead.
        let mut cfg_test_in_force = false;
        for line in contents.lines() {
            let line = line.trim();
            if carries_cfg_test_only_attribute(line) {
                cfg_test_in_force = true;
                continue;
            }
            if line.ends_with(&declaration) {
                return Ok(cfg_test_in_force);
            }
            if !line.is_empty() && !line.starts_with("//") && !line.starts_with("#[") {
                cfg_test_in_force = false;
            }
        }
    }

    Ok(false)
}

/// Whether `line` carries a `#[cfg(..)]` attribute whose predicate holds only
/// when `test` is set, so the item it gates is compiled into test builds and
/// nowhere else.
///
/// Plain `#[cfg(test)]` is the common case, but a test module that also needs
/// a feature or target is declared `#[cfg(all(test, feature = ".."))]`, and
/// that is just as test-only. `#[cfg(any(test, feature = "test-support"))]`
/// is not: the feature alone compiles the item into a non-test build. The
/// predicate is decided structurally, so it has to close on this line; an
/// attribute that spans lines is not recognised and its module stays scanned.
fn carries_cfg_test_only_attribute(line: &str) -> bool {
    line.match_indices("#[cfg(").any(|(start, opener)| {
        let rest = &line[start + opener.len()..];
        let Some(close) = closing_paren(rest) else {
            return false;
        };
        rest[close + 1..].starts_with(']') && cfg_predicate_requires_test(&rest[..close])
    })
}

/// Whether a cfg predicate can only hold in a test build: `test` itself, an
/// `all(..)` with such a member, or a non-empty `any(..)` whose every member
/// is one. `not(..)` and every other predicate never qualify.
fn cfg_predicate_requires_test(predicate: &str) -> bool {
    let predicate = predicate.trim();
    if predicate == "test" {
        return true;
    }
    if let Some(members) = combinator_members(predicate, "all") {
        return members.into_iter().any(cfg_predicate_requires_test);
    }
    if let Some(members) = combinator_members(predicate, "any") {
        return !members.is_empty() && members.into_iter().all(cfg_predicate_requires_test);
    }
    false
}

/// The comma-separated members of `name(..)`, or `None` when `predicate` is
/// not that combinator.
fn combinator_members<'a>(predicate: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let inner = predicate
        .strip_prefix(name)?
        .trim_start()
        .strip_prefix('(')?;
    let close = closing_paren(inner)?;
    if !inner[close + 1..].trim().is_empty() {
        return None;
    }

    let inner = &inner[..close];
    let mut members = Vec::new();
    let (mut depth, mut in_string, mut member_start) = (0usize, false, 0);
    for (idx, ch) in inner.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth = depth.saturating_sub(1),
            ',' if !in_string && depth == 0 => {
                members.push(inner[member_start..idx].trim());
                member_start = idx + 1;
            }
            _ => {}
        }
    }
    members.push(inner[member_start..].trim());
    // A trailing comma leaves an empty final member; it is not a predicate.
    members.retain(|member| !member.is_empty());
    Some(members)
}

/// Byte index of the `)` that closes a group whose `(` was already consumed,
/// skipping parentheses inside string literals.
fn closing_paren(text: &str) -> Option<usize> {
    let (mut depth, mut in_string) = (0usize, false);
    for (idx, ch) in text.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => match depth.checked_sub(1) {
                Some(outer) => depth = outer,
                None => return Some(idx),
            },
            _ => {}
        }
    }
    None
}

#[test]
fn cfg_test_only_attribute_is_decided_by_the_predicate() {
    for (line, test_only) in [
        ("#[cfg(test)]", true),
        ("#[cfg(test)] // trailing comment", true),
        (
            r#"#[cfg(all(test, feature = "copilot", not(target_arch = "wasm32")))]"#,
            true,
        ),
        ("#[cfg(all(unix, test,))]", true),
        ("#[cfg(any(test, all(test, unix)))]", true),
        (r#"#[cfg(any(test, feature = "test-support"))]"#, false),
        (
            r#"#[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]"#,
            false,
        ),
        ("#[cfg(not(test))]", false),
        ("#[cfg(all(not(test), unix))]", false),
        ("#[cfg(any())]", false),
        ("#[cfg(testing)]", false),
        ("#[cfg_attr(test, derive(Debug))]", false),
        ("#[cfg(any(", false),
    ] {
        assert_eq!(
            carries_cfg_test_only_attribute(line),
            test_only,
            "{line} should{} be test-only",
            if test_only { "" } else { " not" }
        );
    }
}

#[test]
fn cfg_test_module_exemption_is_verified_not_inferred_from_the_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("declared_tests.rs"), "// test module\n").unwrap();
    std::fs::write(root.join("undeclared_tests.rs"), "// not a test module\n").unwrap();
    std::fs::write(root.join("adjacent_tests.rs"), "// not a test module\n").unwrap();
    std::fs::write(root.join("compound_tests.rs"), "// test module\n").unwrap();
    std::fs::write(root.join("support_tests.rs"), "// not a test module\n").unwrap();
    std::fs::write(
        root.join("lib.rs"),
        "#[cfg(test)]\nmod declared_tests;\n\n\
         #[cfg(test)]\nstruct Fixture;\nmod adjacent_tests;\n\n\
         mod undeclared_tests;\n\n\
         #[cfg(all(test, feature = \"copilot\", not(target_arch = \"wasm32\")))]\n\
         mod compound_tests;\n\n\
         #[cfg(any(test, feature = \"test-support\"))]\nmod support_tests;\n",
    )
    .unwrap();

    assert!(
        declared_as_cfg_test_module(&root.join("declared_tests.rs")).unwrap(),
        "a module the parent declares under #[cfg(test)] is exempt"
    );
    assert!(
        !declared_as_cfg_test_module(&root.join("undeclared_tests.rs")).unwrap(),
        "the `_tests.rs` name alone must not buy an exemption"
    );
    assert!(
        !declared_as_cfg_test_module(&root.join("adjacent_tests.rs")).unwrap(),
        "a #[cfg(test)] consumed by an intervening item does not reach the declaration"
    );
    assert!(
        declared_as_cfg_test_module(&root.join("compound_tests.rs")).unwrap(),
        "a compound cfg that holds only in test builds is exempt"
    );
    assert!(
        !declared_as_cfg_test_module(&root.join("support_tests.rs")).unwrap(),
        "a cfg that a feature alone satisfies also compiles outside tests"
    );
    assert!(
        !declared_as_cfg_test_module(&root.join("declared.rs")).unwrap(),
        "an undeclared file is scanned even when the parent exists"
    );
}

/// The top-level directories of the repository's products other than Core.
fn product_roots(root: &Path) -> Vec<String> {
    let registry: serde_json::Value = std::fs::read_to_string(root.join("ci/products.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    registry["products"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|product| product["root"].as_str())
        .filter(|product_root| !product_root.is_empty())
        .map(str::to_owned)
        .collect()
}

fn scan_for_manual_input_schema_literals(
    root: &Path,
    dir: &Path,
    offenders: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if path.is_dir() {
            match file_name.as_ref() {
                ".git" | "target" | "tests" => continue,
                _ => {}
            }
            // The roots of the repository's other products
            // (ci/products.json) keep their own tool conventions.
            if dir == root
                && product_roots(root)
                    .iter()
                    .any(|product_root| *product_root == *file_name)
            {
                continue;
            }

            scan_for_manual_input_schema_literals(root, &path, offenders)?;
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        // Whole-file test modules are declared `#[cfg(test)] mod foo_tests;`
        // in the PARENT, so the in-file `#[cfg(test)]` tracking below cannot
        // observe that the file is test-only.
        if declared_as_cfg_test_module(&path)? {
            continue;
        }

        let contents = std::fs::read_to_string(&path)?;
        let mut in_test_module = false;
        let mut test_brace_depth = 0;

        for (idx, line) in contents.lines().enumerate() {
            // Track entry into #[cfg(test)] modules
            if line.contains("#[cfg(test)]") {
                in_test_module = true;
                test_brace_depth = 0;
            }

            // Track brace depth when inside test module
            if in_test_module {
                test_brace_depth += line.chars().filter(|&c| c == '{').count();
                test_brace_depth =
                    test_brace_depth.saturating_sub(line.chars().filter(|&c| c == '}').count());
                if test_brace_depth == 0 && line.contains('}') {
                    in_test_module = false;
                }
                continue; // Skip test code
            }

            if contains_manual_input_schema_literal(line) {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }

    Ok(())
}

fn contains_manual_input_schema_literal(line: &str) -> bool {
    for needle in [
        "input_schema: serde_json::json!(",
        "input_schema: json!(",
        "input_schema:serde_json::json!(",
        "input_schema:json!(",
    ] {
        let Some(pos) = line.find(needle) else {
            continue;
        };

        // Don't trip over our own pattern strings.
        if line[..pos].ends_with('"') {
            continue;
        }

        return true;
    }

    false
}
