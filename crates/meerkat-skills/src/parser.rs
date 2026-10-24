//! SKILL.md frontmatter parser.
//!
//! Uses `serde-saphyr` (pure Rust, no `unsafe`) for YAML parsing of the
//! frontmatter block, under the rules in `yaml_options`.

use std::fmt;

use indexmap::IndexMap;
use meerkat_core::skills::{
    CapabilityId, SkillDescriptor, SkillDocument, SkillError, SkillExtensionKey, SkillKey,
    SkillName, SkillScope,
};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

/// Parsed frontmatter from a SKILL.md file.
#[derive(Debug, serde::Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    metadata: IndexMap<String, String>,
    #[serde(default)]
    requires_capabilities: Vec<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(flatten)]
    extensions: IndexMap<String, ExtensionValue>,
}

/// The YAML value of one author extension field. Mapping entries keep their
/// source order; keys may be any YAML value, as in the source.
#[derive(Debug)]
enum ExtensionValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Sequence(Vec<ExtensionValue>),
    Mapping(Vec<(ExtensionValue, ExtensionValue)>),
}

impl<'de> Deserialize<'de> for ExtensionValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExtensionValueVisitor;

        impl<'de> Visitor<'de> for ExtensionValueVisitor {
            type Value = ExtensionValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a YAML value")
            }

            fn visit_unit<E>(self) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::Null)
            }

            fn visit_none<E>(self) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::Null)
            }

            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> Result<ExtensionValue, D2::Error> {
                ExtensionValue::deserialize(deserializer)
            }

            fn visit_bool<E>(self, value: bool) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::Int(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::UInt(value))
            }

            fn visit_f64<E>(self, value: f64) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::Float(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<ExtensionValue, E> {
                Ok(ExtensionValue::String(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ExtensionValue, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(ExtensionValue::Sequence(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ExtensionValue, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(ExtensionValue::Mapping(entries))
            }
        }

        deserializer.deserialize_any(ExtensionValueVisitor)
    }
}

impl Serialize for ExtensionValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::UInt(value) => serializer.serialize_u64(*value),
            Self::Float(value) => serializer.serialize_f64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Sequence(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Mapping(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

/// The YAML rules for frontmatter.
///
/// - Booleans are YAML 1.2's: `yes`, `on`, `off`, `y` and `n` stay strings.
/// - `<<` is an ordinary key, never a merge.
/// - A duplicated key, at any level, and an unrecognized tag are refused.
/// - `.inf`, `-.inf` and `.nan` stay floats.
/// - An untyped number with leading zeros, such as `0755`, resolves as the
///   decimal `755.0`: upstream issue #225
///   (<https://github.com/bourumir-wyngs/serde-saphyr/issues/225>), open. The
///   skills guide tells authors to quote such values.
/// - The parser's default budget bounds nesting depth (64), nodes, events and
///   alias expansion.
fn yaml_options() -> serde_saphyr::Options {
    let mut options = serde_saphyr::Options::default();
    options.strict_booleans = true;
    options.merge_keys = serde_saphyr::MergeKeyPolicy::AsOrdinary;
    options.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::Error;
    options.reject_unsupported_tags = true;
    options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::PassThrough;
    options.with_snippet = false;
    options
}

/// Block YAML for a structured extension value: YAML 1.2 quoting, no
/// directive, and no folding of long lines.
fn yaml_serializer_options() -> serde_saphyr::SerializerOptions {
    let mut options = serde_saphyr::SerializerOptions::default();
    options.yaml_12 = true;
    options.no_lang_directive = true;
    options.folded_wrap_chars = usize::MAX;
    options
}

/// Refuse a `...` document end marker. Frontmatter is one YAML document, and
/// serde-saphyr 2.0.0 stops at the marker and ignores what follows it, even
/// malformed text: upstream issue #228, fixed after 2.0.0 by
/// <https://github.com/bourumir-wyngs/serde-saphyr/pull/230>. Remove this
/// guard at the bump to a release with that fix, and decide then whether a
/// marker with nothing after it stays refused (`test_rejects_document_end_marker`
/// pins both). Lines break where YAML breaks them: at LF, CRLF and a lone CR.
fn reject_document_end_marker(frontmatter: &str) -> Result<(), SkillError> {
    let marker_line = frontmatter
        .replace("\r\n", "\n")
        .split(['\n', '\r'])
        .position(|line| {
            line.strip_prefix("...")
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
        });
    match marker_line {
        Some(index) => Err(SkillError::Parse(
            format!(
                "frontmatter parse error: a `...` document end marker is not allowed (frontmatter line {})",
                index + 1
            )
            .into(),
        )),
        None => Ok(()),
    }
}

/// Parse a SKILL.md file into a `SkillDocument`.
pub fn parse_skill_md(
    key: SkillKey,
    scope: SkillScope,
    content: &str,
    expected_skill_name: Option<&str>,
) -> Result<SkillDocument, SkillError> {
    let (frontmatter_str, body) = split_frontmatter(content)?;
    reject_document_end_marker(&frontmatter_str)?;
    let fm: Frontmatter = serde_saphyr::from_str_with_options(&frontmatter_str, yaml_options())
        .map_err(|e| SkillError::Parse(format!("frontmatter parse error: {e}").into()))?;
    validate_frontmatter(&fm, expected_skill_name)?;

    let mut metadata = fm.metadata;
    if let Some(version) = fm.version {
        metadata.insert("version".to_string(), version);
    }

    let mut capability_requirements: Vec<CapabilityId> = Vec::new();
    for raw in fm.requires_capabilities {
        capability_requirements.push(CapabilityId::parse(&raw)?);
    }

    let extensions = parse_extensions(fm.extensions)?;

    Ok(SkillDocument {
        descriptor: SkillDescriptor {
            key,
            name: fm.name,
            description: fm.description,
            scope,
            metadata,
            capability_requirements,
            source_name: String::new(),
        },
        body: body.to_string(),
        extensions,
    })
}

fn validate_frontmatter(
    fm: &Frontmatter,
    expected_skill_name: Option<&str>,
) -> Result<(), SkillError> {
    let skill_name = SkillName::parse(&fm.name)?;
    if let Some(expected) = expected_skill_name
        && skill_name.as_str() != expected
    {
        return Err(SkillError::Parse(
            format!(
                "frontmatter name '{}' must match directory slug '{}'",
                fm.name, expected
            )
            .into(),
        ));
    }

    Ok(())
}

/// Parse the raw flattened frontmatter extensions into a typed
/// `namespace.key` → value map.
///
/// Each key is parsed into a [`SkillExtensionKey`] once, fail-closed: any
/// non-namespaced or malformed key (the only frontmatter fields that reach the
/// `#[serde(flatten)]` catch-all) errors here rather than silently surviving as
/// a re-split string. Values stay open-ended author metadata, stored as a
/// projection of the YAML data: a string as written, any other value as the
/// YAML text of its resolved value (`1.10` is the float `1.1`, a structured
/// value block YAML). A structured (non-scalar) value is policy-bearing and
/// must declare a companion `<namespace>.version`, enforced over the typed key
/// set.
fn parse_extensions(
    raw: IndexMap<String, ExtensionValue>,
) -> Result<IndexMap<SkillExtensionKey, String>, SkillError> {
    let mut typed: IndexMap<SkillExtensionKey, (ExtensionValue, String)> =
        IndexMap::with_capacity(raw.len());
    for (k, value) in raw {
        let key = SkillExtensionKey::parse(&k)?;
        let serialized = if let ExtensionValue::String(s) = &value {
            s.clone()
        } else {
            serde_saphyr::to_string_with_options(&value, yaml_serializer_options())
                .map_err(|e| SkillError::Parse(format!("extension serialize error: {e}").into()))?
                .trim()
                .to_string()
        };
        typed.insert(key, (value, serialized));
    }

    for (key, (value, _)) in &typed {
        if key.is_version() {
            continue;
        }
        let nontrivial = matches!(
            value,
            ExtensionValue::Mapping(_) | ExtensionValue::Sequence(_)
        );
        if nontrivial && !typed.contains_key(&key.version_companion()) {
            return Err(SkillError::Parse(
                format!(
                    "extension '{key}' requires companion '{}' for nontrivial values",
                    key.version_companion()
                )
                .into(),
            ));
        }
    }

    Ok(typed
        .into_iter()
        .map(|(key, (_, serialized))| (key, serialized))
        .collect())
}

/// Split content into frontmatter and body, separated by `---` delimiters.
///
/// The frontmatter keeps its final line break, so a block scalar that ends it
/// keeps its own as YAML specifies (`|` clips to one, `|+` keeps all).
fn split_frontmatter(content: &str) -> Result<(String, &str), SkillError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let trimmed = content.trim_start();
    let Some(first_line_end) = trimmed.find('\n') else {
        return Err(SkillError::Parse(
            "missing closing frontmatter delimiter".into(),
        ));
    };
    let first_line = trimmed[..first_line_end].trim_end_matches('\r').trim();
    if first_line != "---" {
        return Err(SkillError::Parse("missing frontmatter delimiter".into()));
    }

    let after_first = &trimmed[first_line_end + 1..];
    let mut offset = 0;
    for line in after_first.split_inclusive('\n') {
        let delimiter_candidate = line.trim_end_matches(['\r', '\n']).trim_end();
        if delimiter_candidate == "---" {
            let frontmatter = &after_first[..offset];
            let body = &after_first[offset + line.len()..];
            return Ok((
                frontmatter.to_string(),
                body.trim_start_matches(['\r', '\n']),
            ));
        }
        offset += line.len();
    }

    Err(SkillError::Parse(
        "missing closing frontmatter delimiter".into(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::SourceUuid;

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::builtin(),
            skill_name: SkillName::parse(skill).unwrap(),
        }
    }

    #[test]
    fn test_parse_simple_skill() {
        let content = r"---
name: shell-patterns
description: Background job workflows
requires_capabilities: [builtins, shell]
---

# shell-patterns

When running background jobs...";

        let doc = parse_skill_md(
            test_key("shell-patterns"),
            SkillScope::Builtin,
            content,
            None,
        )
        .unwrap();

        assert_eq!(doc.descriptor.name, "shell-patterns");
        assert_eq!(doc.descriptor.description, "Background job workflows");
        let caps: Vec<&str> = doc
            .descriptor
            .capability_requirements
            .iter()
            .map(meerkat_core::skills::CapabilityId::as_str)
            .collect();
        assert_eq!(caps, vec!["builtins", "shell"]);
        assert!(doc.body.contains("# shell-patterns"));
    }

    #[test]
    fn test_parse_no_capabilities() {
        let content =
            "---\nname: mcp-setup\ndescription: Configure MCP servers\n---\n\nContent here";
        let doc =
            parse_skill_md(test_key("mcp-setup"), SkillScope::Project, content, None).unwrap();
        assert!(doc.descriptor.capability_requirements.is_empty());
    }

    #[test]
    fn test_parse_description_with_colon() {
        let content = "---\nname: test\ndescription: \"Use: this tool carefully\"\n---\n\nBody";
        let doc = parse_skill_md(test_key("test"), SkillScope::Builtin, content, None).unwrap();
        assert_eq!(doc.descriptor.description, "Use: this tool carefully");
    }

    #[test]
    fn test_parse_accepts_bom_and_consumes_full_closing_delimiter_line() {
        let content = "\u{feff}---\r\nname: test-skill\r\ndescription: d\r\n---   \r\nBody";
        let doc =
            parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None).unwrap();
        assert_eq!(doc.body, "Body");
    }

    #[test]
    fn test_parse_keeps_indented_delimiter_in_yaml_block_scalar() {
        let content = r"---
name: test-skill
description: |
  first line
  ---
  still description
---
Body";

        let doc =
            parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None).unwrap();

        assert_eq!(
            doc.descriptor.description,
            "first line\n---\nstill description\n"
        );
        assert_eq!(doc.body, "Body");
    }

    #[test]
    fn test_missing_frontmatter() {
        let content = "# No frontmatter";
        let result = parse_skill_md(test_key("test"), SkillScope::Builtin, content, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_invalid_capability_slug() {
        let content = r"---
name: test-skill
description: d
requires_capabilities: [Invalid]
---
body";
        let result = parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_unnamespaced_unknown_field() {
        let content = r"---
name: test-skill
description: d
custom_field: true
---
body";
        let result = parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_name_directory_mismatch() {
        let content = r"---
name: wrong-name
description: d
---
body";
        let result = parse_skill_md(
            test_key("correct-name"),
            SkillScope::Builtin,
            content,
            Some("correct-name"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_structured_extension_with_version_parses_into_typed_key() {
        let content = r"---
name: test-skill
description: d
vendor.version: '1'
vendor.policy:
  allow: [a, b]
  deny: [c]
---
body";
        let doc =
            parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None).unwrap();

        // Keys are typed `SkillExtensionKey` owners with namespace/key splits
        // parsed once, not re-split strings.
        let policy = doc
            .extensions
            .keys()
            .find(|k| k.key() == "policy")
            .expect("policy extension present");
        assert_eq!(policy.namespace(), "vendor");
        assert_eq!(policy.as_str(), "vendor.policy");

        // The structured value round-trips into its serialized form rather than
        // being silently dropped.
        let serialized = &doc.extensions[policy];
        assert!(serialized.contains("allow"));
        assert!(serialized.contains("deny"));
    }

    #[test]
    fn test_structured_extension_without_version_fails_closed() {
        let content = r"---
name: test-skill
description: d
vendor.policy:
  allow: [a]
---
body";
        let result = parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None);
        // Policy-bearing (structured) extension without a companion
        // `vendor.version` fails at parse rather than silently stringifying.
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_extension_key_with_empty_segment() {
        let content = r"---
name: test-skill
description: d
vendor.: value
---
body";
        let result = parse_skill_md(test_key("test-skill"), SkillScope::Builtin, content, None);
        assert!(result.is_err());
    }

    fn parse_with_frontmatter(extra: &str) -> Result<SkillDocument, SkillError> {
        let content = format!("---\nname: test-skill\ndescription: d\n{extra}---\nbody");
        parse_skill_md(test_key("test-skill"), SkillScope::Builtin, &content, None)
    }

    fn extension_value<'a>(doc: &'a SkillDocument, key: &str) -> Option<&'a str> {
        doc.extensions
            .iter()
            .find(|(candidate, _)| candidate.as_str() == key)
            .map(|(_, value)| value.as_str())
    }

    /// Unquoted scalars in string fields keep their source text: no YAML type
    /// resolution reaches them.
    #[test]
    fn test_string_fields_keep_unquoted_scalar_source_text() {
        let doc = parse_with_frontmatter(
            "version: 1.10\nmetadata:\n  float: 1.10\n  hex: 0x1F\n  word: yes\n  flag: true\n  exp: 1e3\n",
        )
        .unwrap();
        let metadata = &doc.descriptor.metadata;
        for (key, expected) in [
            ("version", "1.10"),
            ("float", "1.10"),
            ("hex", "0x1F"),
            ("word", "yes"),
            ("flag", "true"),
            ("exp", "1e3"),
        ] {
            assert_eq!(
                metadata.get(key).map(String::as_str),
                Some(expected),
                "{key}"
            );
        }
    }

    /// Extension scalars resolve booleans as YAML 1.2 does: only
    /// true/True/TRUE and false/False/FALSE, so YAML 1.1 words stay strings.
    #[test]
    fn test_extension_scalars_resolve_booleans_as_yaml_1_2() {
        let doc = parse_with_frontmatter(
            "vendor.yes: yes\nvendor.on: on\nvendor.off: off\nvendor.y: y\nvendor.n: n\nvendor.no: No\nvendor.lower: true\nvendor.title: True\nvendor.upper: TRUE\nvendor.false: false\n",
        )
        .unwrap();
        for (key, expected) in [
            ("vendor.yes", "yes"),
            ("vendor.on", "on"),
            ("vendor.off", "off"),
            ("vendor.y", "y"),
            ("vendor.n", "n"),
            ("vendor.no", "No"),
            ("vendor.lower", "true"),
            ("vendor.title", "true"),
            ("vendor.upper", "true"),
            ("vendor.false", "false"),
        ] {
            assert_eq!(extension_value(&doc, key), Some(expected), "{key}");
        }
    }

    /// A non-string extension scalar is stored as the YAML text of its
    /// resolved value, not its source text: `1.10` is the float 1.1.
    #[test]
    fn test_extension_non_string_scalars_store_resolved_value_text() {
        let doc = parse_with_frontmatter(
            "vendor.float: 1.10\nvendor.hex: 0x1F\nvendor.octal: 0o17\nvendor.exp: 1e3\nvendor.frac: .5\nvendor.plus: +12\nvendor.minus: -12\nvendor.whole: 1.0\nvendor.max: 18446744073709551615\nvendor.inf: .inf\nvendor.neginf: -.inf\nvendor.nan: .nan\nvendor.tilde: ~\nvendor.empty:\nvendor.null: null\n",
        )
        .unwrap();
        for (key, expected) in [
            ("vendor.float", "1.1"),
            ("vendor.hex", "31"),
            ("vendor.octal", "15"),
            ("vendor.exp", "1000.0"),
            ("vendor.frac", "0.5"),
            ("vendor.plus", "12"),
            ("vendor.minus", "-12"),
            ("vendor.whole", "1.0"),
            ("vendor.max", "18446744073709551615"),
            ("vendor.inf", ".inf"),
            ("vendor.neginf", "-.inf"),
            ("vendor.nan", ".nan"),
            ("vendor.tilde", "null"),
            ("vendor.empty", "null"),
            ("vendor.null", "null"),
        ] {
            assert_eq!(extension_value(&doc, key), Some(expected), "{key}");
        }
    }

    /// A structured extension value is stored as block YAML, keys in source
    /// order, aliases expanded.
    #[test]
    fn test_structured_extension_stores_block_yaml_in_source_order() {
        let doc = parse_with_frontmatter(concat!(
            "vendor.version: '1'\n",
            "vendor.policy:\n",
            "  rules:\n",
            "    - name: r1\n",
            "      paths: [a, b]\n",
            "    - name: r2\n",
            "      when: {x: 1}\n",
            "  empty_map: {}\n",
            "  empty_seq: []\n",
            "  nothing: ~\n",
            "  enabled: true\n",
            "  ratio: 1.5\n",
            "  note: |\n",
            "    line one\n",
            "    line two\n",
            "vendor.list: &shared [a, 1, true]\n",
            "vendor.alias: *shared\n",
        ))
        .unwrap();
        assert_eq!(
            extension_value(&doc, "vendor.policy"),
            Some(concat!(
                "rules:\n",
                "- name: r1\n",
                "  paths:\n",
                "  - a\n",
                "  - b\n",
                "- name: r2\n",
                "  when:\n",
                "    x: 1\n",
                "empty_map: {}\n",
                "empty_seq: []\n",
                "nothing: null\n",
                "enabled: true\n",
                "ratio: 1.5\n",
                "note: |\n",
                "  line one\n",
                "  line two",
            ))
        );
        assert_eq!(
            extension_value(&doc, "vendor.list"),
            Some("- a\n- 1\n- true")
        );
        assert_eq!(
            extension_value(&doc, "vendor.alias"),
            Some("- a\n- 1\n- true")
        );
    }

    /// Frontmatter the parser refuses outright: a custom tag, a duplicated
    /// known field or nested key, a second YAML document, and an alias
    /// expansion bomb.
    #[test]
    fn test_rejects_tags_duplicates_documents_and_alias_bombs() {
        let bomb = concat!(
            "vendor.version: '1'\n",
            "vendor.a: &a [x, x, x, x, x, x, x, x, x]\n",
            "vendor.b: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a]\n",
            "vendor.c: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b]\n",
            "vendor.d: &d [*c, *c, *c, *c, *c, *c, *c, *c, *c]\n",
            "vendor.e: &e [*d, *d, *d, *d, *d, *d, *d, *d, *d]\n",
            "vendor.f: &f [*e, *e, *e, *e, *e, *e, *e, *e, *e]\n",
            "vendor.g: &g [*f, *f, *f, *f, *f, *f, *f, *f, *f]\n",
            "vendor.h: &h [*g, *g, *g, *g, *g, *g, *g, *g, *g]\n",
            "vendor.i: &i [*h, *h, *h, *h, *h, *h, *h, *h, *h]\n",
        );
        for (case, extra) in [
            ("custom tag", "vendor.tagged: !custom value\n"),
            (
                "custom tag on a mapping",
                "vendor.version: '1'\nvendor.tagged: !custom {x: 1}\n",
            ),
            ("duplicate known field", "description: again\n"),
            (
                "duplicate nested key",
                "vendor.version: '1'\nvendor.policy:\n  a: 1\n  a: 2\n",
            ),
            ("second document", "--- extra: 1\n"),
            ("alias bomb", bomb),
        ] {
            assert!(parse_with_frontmatter(extra).is_err(), "{case}");
        }
    }

    /// `<<` is an ordinary key, never a merge: at the top level it reaches the
    /// extension-key parse and fails there.
    #[test]
    fn test_merge_key_is_an_ordinary_key() {
        let error = parse_with_frontmatter("<<: {vendor.merged: 1}\n").unwrap_err();
        assert!(
            error.to_string().contains("invalid extension key '<<'"),
            "{error}"
        );
    }

    /// A key given twice is refused at every level, because which value was
    /// meant is ambiguous: a top-level extension key (however it is spelled)
    /// and a metadata key. A duplicated known field or key inside a
    /// structured value was refused already.
    #[test]
    fn test_rejects_duplicate_keys_at_every_level() {
        let accepted: Vec<&str> = [
            ("extension key", "vendor.flag: one\nvendor.flag: two\n"),
            (
                "extension key spelled two ways",
                "vendor.flag: one\n\"vendor.flag\": two\n",
            ),
            ("metadata key", "metadata:\n  owner: a\n  owner: b\n"),
        ]
        .into_iter()
        .filter(|(_, extra)| parse_with_frontmatter(extra).is_ok())
        .map(|(case, _)| case)
        .collect();
        assert!(accepted.is_empty(), "accepted: {accepted:?}");
    }

    /// A null where a string field expects text is an authoring error, and is
    /// refused: `~` or an empty value for the description or a metadata
    /// value. (A null capability was refused already, as an invalid slug.)
    #[test]
    fn test_rejects_null_in_string_fields() {
        let accepted: Vec<&str> = [
            ("description tilde", "metadata: {}\n", "description: ~\n"),
            ("metadata tilde", "metadata:\n  owner: ~\n", ""),
            ("metadata empty", "metadata:\n  owner:\n", ""),
        ]
        .into_iter()
        .filter(|(_, extra, description)| {
            let content = if description.is_empty() {
                format!("---\nname: test-skill\ndescription: d\n{extra}---\nbody")
            } else {
                format!("---\nname: test-skill\n{description}{extra}---\nbody")
            };
            parse_skill_md(test_key("test-skill"), SkillScope::Builtin, &content, None).is_ok()
        })
        .map(|(case, _, _)| case)
        .collect();
        assert!(accepted.is_empty(), "accepted: {accepted:?}");
        assert!(parse_with_frontmatter("requires_capabilities: [~]\n").is_err());
    }

    /// A structured extension value renders as YAML 1.2 block text: double
    /// quotes where a string needs quoting, a signed exponent, and a quoted
    /// `<<` key. The data is what the author wrote; only the rendering
    /// changed from serde_yaml 0.9's (`'1.10'`, `6.02e23`, `<<:`).
    #[test]
    fn test_structured_extension_rendering() {
        let doc = parse_with_frontmatter(concat!(
            "vendor.version: '1'\n",
            "vendor.policy:\n",
            "  quoted: '1.10'\n",
            "  empty: ''\n",
            "  colon: 'a: b'\n",
            "  big: 6.02e23\n",
            "  huge: 1e16\n",
            "  <<: {x: 1}\n",
        ))
        .unwrap();
        assert_eq!(
            extension_value(&doc, "vendor.policy"),
            Some(concat!(
                "quoted: \"1.10\"\n",
                "empty: \"\"\n",
                "colon: \"a: b\"\n",
                "big: 6.02e+23\n",
                "huge: 1.0e+16\n",
                "\"<<\":\n",
                "  x: 1",
            ))
        );
    }

    /// Untyped scalars that serde_yaml 0.9 kept as text, or refused, resolve
    /// as YAML values now: `1_000` and `017` are numbers, an integer past u64
    /// is a float, `1e999` is infinity, `tRUE` is a boolean, and `!!binary`
    /// is decoded.
    #[test]
    fn test_extension_scalar_edge_cases() {
        let doc = parse_with_frontmatter(concat!(
            "vendor.underscore: 1_000\n",
            "vendor.leading_zero: 017\n",
            "vendor.overflow: 1e999\n",
            "vendor.mixed_case: tRUE\n",
            "vendor.binary: !!binary aGVsbG8=\n",
        ))
        .unwrap();
        let mismatched: Vec<(&str, Option<&str>)> = [
            ("vendor.underscore", "1000"),
            ("vendor.leading_zero", "17.0"),
            ("vendor.overflow", ".inf"),
            ("vendor.mixed_case", "true"),
            ("vendor.binary", "hello"),
        ]
        .into_iter()
        .filter(|(key, expected)| extension_value(&doc, key) != Some(*expected))
        .map(|(key, _)| (key, extension_value(&doc, key)))
        .collect();
        assert!(mismatched.is_empty(), "mismatched: {mismatched:?}");

        let past_u64 =
            parse_with_frontmatter("vendor.past_u64: 123456789012345678901234\n").unwrap();
        assert_eq!(
            extension_value(&past_u64, "vendor.past_u64"),
            Some("1.2345678901234569e+23")
        );
    }

    /// An unrecognized tag is refused on a typed field too, where serde_yaml
    /// 0.9 dropped the tag and kept the value. A standard tag still applies.
    #[test]
    fn test_rejects_custom_tags_on_typed_fields() {
        let parse = |frontmatter: &str| {
            let content = format!("---\n{frontmatter}---\nbody");
            parse_skill_md(test_key("test-skill"), SkillScope::Builtin, &content, None)
        };
        let accepted: Vec<&str> = [
            ("name", "name: !custom test-skill\ndescription: d\n"),
            (
                "description",
                "name: test-skill\ndescription: !custom text\n",
            ),
            (
                "version",
                "name: test-skill\ndescription: d\nversion: !custom 1.0\n",
            ),
            (
                "metadata value",
                "name: test-skill\ndescription: d\nmetadata:\n  owner: !custom team\n",
            ),
            (
                "capability",
                "name: test-skill\ndescription: d\nrequires_capabilities: [!custom shell]\n",
            ),
        ]
        .into_iter()
        .filter(|(_, frontmatter)| parse(frontmatter).is_ok())
        .map(|(case, _)| case)
        .collect();
        assert!(accepted.is_empty(), "accepted: {accepted:?}");

        let standard = parse("name: test-skill\ndescription: !!str 123\n").unwrap();
        assert_eq!(standard.descriptor.description, "123");
    }

    /// Nesting deeper than the parser budget's 64 levels (the frontmatter
    /// mapping included) is refused.
    #[test]
    fn test_rejects_nesting_deeper_than_the_budget() {
        let nested = |depth: usize| {
            format!(
                "vendor.version: '1'\nvendor.deep: {}{}\n",
                "[".repeat(depth),
                "]".repeat(depth)
            )
        };
        assert!(parse_with_frontmatter(&nested(63)).is_ok());
        assert!(parse_with_frontmatter(&nested(64)).is_err());
    }

    /// The parser budget's alias heuristic: 100 or more aliases that outnumber
    /// their anchors more than ten to one are refused, even when the expansion
    /// is small.
    #[test]
    fn test_rejects_aliases_past_the_budget_ratio() {
        let aliases = |count: usize| {
            format!(
                "vendor.version: '1'\nvendor.anchor: &a x\nvendor.uses: [{}]\n",
                vec!["*a"; count].join(", ")
            )
        };
        assert!(parse_with_frontmatter(&aliases(99)).is_ok());
        assert!(parse_with_frontmatter(&aliases(101)).is_err());
    }

    /// A block scalar that ends the frontmatter keeps its final line break, as
    /// YAML specifies and as it does anywhere else in the frontmatter: `|` and
    /// `>` clip to one line break, `|+` keeps every trailing one, `|-` strips.
    /// The frontmatter used to be cut before that line break.
    #[test]
    fn test_block_scalar_ending_the_frontmatter_keeps_its_line_break() {
        let ending = |key: &str, block: &str| {
            let content = format!(
                "---\nname: test-skill\ndescription: d\nvendor.version: '1'\n{key}: {block}\n  first\n  second\n\n---\nbody"
            );
            let doc = parse_skill_md(test_key("test-skill"), SkillScope::Builtin, &content, None)
                .unwrap();
            extension_value(&doc, key).map(str::to_owned)
        };
        let mismatched: Vec<(&str, Option<String>)> = [
            ("|", "first\nsecond\n"),
            (">", "first second\n"),
            ("|+", "first\nsecond\n\n"),
            ("|-", "first\nsecond"),
        ]
        .into_iter()
        .filter(|(block, expected)| ending("vendor.note", block).as_deref() != Some(*expected))
        .map(|(block, _)| (block, ending("vendor.note", block)))
        .collect();
        assert!(mismatched.is_empty(), "mismatched: {mismatched:?}");

        let last = parse_skill_md(
            test_key("test-skill"),
            SkillScope::Builtin,
            "---\nname: test-skill\ndescription: |\n  first\n  second\n---\nbody",
            None,
        )
        .unwrap();
        let not_last = parse_skill_md(
            test_key("test-skill"),
            SkillScope::Builtin,
            "---\nname: test-skill\ndescription: |\n  first\n  second\nvendor.flag: x\n---\nbody",
            None,
        )
        .unwrap();
        assert_eq!(last.descriptor.description, "first\nsecond\n");
        assert_eq!(not_last.descriptor.description, last.descriptor.description);
    }

    /// A `...` document end marker is refused: serde-saphyr 2.0.0 ignores
    /// whatever follows one, malformed text included. YAML breaks lines at a
    /// lone CR too, and the reported line counts CRLF once. In a block scalar,
    /// `...` is text.
    #[test]
    fn test_rejects_document_end_marker() {
        for (case, extra, line) in [
            ("malformed text after the marker", "...\n]]] not yaml\n", 3),
            ("a comment after the marker", "... # end\n", 3),
            ("the marker alone", "...\n", 3),
            (
                "a lone CR around the marker",
                "vendor.note: x\r...\r]]] not yaml\n",
                4,
            ),
            ("CRLF line breaks", "vendor.note: x\r\n...\r\n", 4),
        ] {
            let error = parse_with_frontmatter(extra).unwrap_err().to_string();
            assert!(
                error.contains(&format!(
                    "document end marker is not allowed (frontmatter line {line})"
                )),
                "{case}: {error}"
            );
        }
        let doc = parse_with_frontmatter("vendor.note: |\n  first\n  ...\n  last\n").unwrap();
        assert_eq!(
            extension_value(&doc, "vendor.note"),
            Some("first\n...\nlast\n")
        );
    }
}
