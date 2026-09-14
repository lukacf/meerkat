use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// Serde ignores extra fields on internally tagged unit variants even when
/// the enum denies unknown fields. Empty struct variants close that hole.
#[test]
fn public_live_tagged_carriers_cannot_silently_discard_permission_fields() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow::anyhow!("workspace root missing"))?;
    let mut files = Vec::new();
    collect_rust_files(&root.join("meerkat-core/src/live_execution"), &mut files)?;
    collect_rust_files(&root.join("meerkat-openai/src/public_live"), &mut files)?;
    collect_rust_files(&root.join("meerkat-runtime/src/live_ledger"), &mut files)?;
    files.extend([
        root.join("meerkat-core/src/execution_scope.rs"),
        root.join("meerkat-runtime/src/live_request.rs"),
        root.join("meerkat-runtime/src/live_grant.rs"),
        root.join("meerkat-runtime/src/live_source.rs"),
        root.join("meerkat-core/src/live_observation.rs"),
        root.join("meerkat-contracts/src/wire/live_observation.rs"),
        root.join("meerkat-runtime/src/live_delivery.rs"),
        root.join("meerkat-runtime/src/live_resources.rs"),
    ]);
    let mut checked = 0;
    for path in files {
        let source = std::fs::read_to_string(&path)?;
        let file = syn::parse_file(&source)?;
        let mut visitor = TaggedVisitor {
            path: &path,
            violations: Vec::new(),
            checked: 0,
        };
        visitor.visit_file(&file);
        anyhow::ensure!(
            visitor.violations.is_empty(),
            "{}",
            visitor.violations.join("\n")
        );
        checked += visitor.checked;
    }
    anyhow::ensure!(
        checked >= 9,
        "public Live tagged inventory was not traversed"
    );
    let bridge = root.join("meerkat-contracts/src/wire/supervisor_bridge.rs");
    check_selected_tagged_carrier(
        &bridge,
        &std::fs::read_to_string(&bridge)?,
        "BridgeLiveProfileSelection",
    )?;
    Ok(())
}

fn check_selected_tagged_carrier(path: &Path, source: &str, name: &str) -> anyhow::Result<()> {
    let file = syn::parse_file(source)?;
    let mut selected = file.items.iter().filter_map(|item| match item {
        syn::Item::Enum(item) if item.ident == name => Some(item),
        _ => None,
    });
    let item = selected
        .next()
        .ok_or_else(|| anyhow::anyhow!("{}: missing selected carrier {name}", path.display()))?;
    anyhow::ensure!(
        selected.next().is_none(),
        "{}: duplicate selected carrier {name}",
        path.display()
    );
    let mut visitor = TaggedVisitor {
        path,
        violations: Vec::new(),
        checked: 0,
    };
    visitor.visit_item_enum(item);
    anyhow::ensure!(
        visitor.checked == 1,
        "{}: selected carrier {name} lost its tagged representation",
        path.display()
    );
    anyhow::ensure!(
        visitor.violations.is_empty(),
        "{}",
        visitor.violations.join("\n")
    );
    Ok(())
}

#[test]
fn mixed_legacy_files_still_check_the_exact_selected_public_carrier() {
    let legacy = r#"#[serde(tag = "kind")] enum Legacy { Absent }"#;
    for (selected, valid) in [
        (
            r#"#[serde(tag = "version", deny_unknown_fields)] enum Selected { V1 {} }"#,
            true,
        ),
        (
            r#"#[serde(tag = "version", deny_unknown_fields)] enum Selected { V1 }"#,
            false,
        ),
        ("enum Selected { V1 {} }", false),
        ("enum Renamed { V1 {} }", false),
        (
            r#"#[cfg_attr(all(), serde(tag = "version", deny_unknown_fields))] enum Selected { V1 {} }"#,
            false,
        ),
        (
            r#"#[serde(tag = "version", deny_unknown_fields)] enum Selected { V1 {} }
               #[serde(tag = "version", deny_unknown_fields)] enum Selected { V1 {} }"#,
            false,
        ),
    ] {
        assert_eq!(
            check_selected_tagged_carrier(
                Path::new("mixed.rs"),
                &format!("{legacy}\n{selected}"),
                "Selected",
            )
            .is_ok(),
            valid,
            "{selected}",
        );
    }
}

fn collect_rust_files(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_rust_files(&entry.path(), files)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|value| value == "rs") {
            files.push(entry.path());
        }
    }
    Ok(())
}

struct TaggedVisitor<'a> {
    path: &'a Path,
    violations: Vec<String>,
    checked: usize,
}

#[test]
fn tagged_carrier_ratchet_detects_the_original_unit_variant_hole() -> anyhow::Result<()> {
    for (source, expected) in [
        (
            r#"#[serde(tag = "kind", deny_unknown_fields)] enum Evidence { Absent }"#,
            1,
        ),
        (r#"#[serde(tag = "kind")] enum Evidence { Absent {} }"#, 1),
        (
            r#"#[serde(tag = "kind", deny_unknown_fields)] enum Evidence { Absent {} }"#,
            0,
        ),
        (
            r#"#[serde(tag = "kind", content = "value", deny_unknown_fields)] enum Evidence { Absent }"#,
            0,
        ),
    ] {
        let mut visitor = TaggedVisitor {
            path: Path::new("fixture.rs"),
            violations: Vec::new(),
            checked: 0,
        };
        visitor.visit_file(&syn::parse_file(source)?);
        assert_eq!(visitor.checked, 1);
        assert_eq!(visitor.violations.len(), expected, "{source}");
    }
    Ok(())
}

fn conditional_serde(meta: &syn::Meta) -> syn::Result<bool> {
    if !meta.path().is_ident("cfg_attr") {
        return Ok(false);
    }
    let list = meta.require_list()?;
    let arguments = list.parse_args_with(
        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
    )?;
    for attribute in arguments.iter().skip(1) {
        if attribute.path().is_ident("serde") || conditional_serde(attribute)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[test]
fn conditional_serde_cannot_disappear_behind_a_satisfied_inventory_count() -> anyhow::Result<()> {
    let mut visible = String::new();
    for index in 0..9 {
        visible.push_str(&format!(
            "#[serde(tag = \"kind\", deny_unknown_fields)] enum Visible{index} {{ Absent {{}} }}\n"
        ));
    }
    for hidden in [
        r#"#[cfg_attr(all(), serde(tag = "kind", deny_unknown_fields))] enum Hidden { Absent }"#,
        r#"#[cfg_attr(all(), cfg_attr(all(), serde(tag = "kind", deny_unknown_fields)))] enum Hidden { Absent }"#,
        r#"#[serde(tag = "kind", deny_unknown_fields)] #[cfg_attr(any(), serde(content = "value"))] enum Hidden { Absent }"#,
    ] {
        let mut visitor = TaggedVisitor {
            path: Path::new("fixture.rs"),
            violations: Vec::new(),
            checked: 0,
        };
        visitor.visit_file(&syn::parse_file(&format!("{visible}{hidden}"))?);
        assert!(visitor.checked >= 9);
        assert!(
            visitor
                .violations
                .iter()
                .any(|error| error.contains("conditional serde")),
            "conditional representation silently escaped: {hidden}"
        );
    }
    Ok(())
}

impl<'ast> Visit<'ast> for TaggedVisitor<'_> {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        match conditional_serde(&attribute.meta) {
            Ok(false) => {}
            Ok(true) => self.violations.push(format!(
                "{}: conditional serde representation is unsupported by this audit; \
                 use unconditional serde attributes or extend configuration-aware validation",
                self.path.display()
            )),
            Err(error) => self.violations.push(format!(
                "{}: conditional serde inventory could not be parsed: {error}",
                self.path.display()
            )),
        }
        syn::visit::visit_attribute(self, attribute);
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        let mut tag = false;
        let mut content = false;
        let mut strict = false;
        for attribute in &item.attrs {
            if !attribute.path().is_ident("serde") {
                continue;
            }
            let parsed = attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("tag") {
                    tag = true;
                }
                if meta.path.is_ident("content") {
                    content = true;
                }
                if meta.path.is_ident("deny_unknown_fields") {
                    strict = true;
                }
                if meta.input.peek(syn::Token![=]) {
                    let _: syn::Expr = meta.value()?.parse()?;
                }
                Ok(())
            });
            if let Err(error) = parsed {
                self.violations.push(format!(
                    "{}: {} serde inventory could not be parsed: {error}",
                    self.path.display(),
                    item.ident
                ));
            }
        }
        if tag {
            self.checked += 1;
            if !strict {
                self.violations.push(format!(
                    "{}: {} must deny unknown fields",
                    self.path.display(),
                    item.ident
                ));
            }
            if !content {
                for variant in &item.variants {
                    if matches!(variant.fields, syn::Fields::Unit) {
                        self.violations.push(format!(
                            "{}: {}::{} must use an empty struct variant to reject extra fields",
                            self.path.display(),
                            item.ident,
                            variant.ident
                        ));
                    }
                }
            }
        }
        syn::visit::visit_item_enum(self, item);
    }
}
