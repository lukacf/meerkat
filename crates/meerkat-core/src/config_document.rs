//! Structure-preserving edits of persisted config documents.
//!
//! Realm composition reads which keys a realm's own document sets: an explicit
//! key overrides the parent realm even when its value equals the struct
//! default (`child-wins-scalar`). A config patch therefore edits the document
//! as written instead of re-serializing a typed [`Config`], which would write
//! every default as an explicit override and stop the realm from inheriting
//! it. Keys the patch does not name keep their presence, values, comments and
//! order.
//!
//! [`Config`]: crate::config::Config

use crate::config::ConfigError;
use serde_json::{Map, Number, Value as Json};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

/// Parse a persisted document for editing.
///
/// Callers load `content` as a config first, so a failure here is a
/// disagreement between the two TOML readers, not a bad document.
pub(crate) fn parse(content: &str) -> Result<DocumentMut, ConfigError> {
    content.parse::<DocumentMut>().map_err(|error| {
        ConfigError::InternalError(format!("config document is not editable: {error}"))
    })
}

/// Apply an RFC 7396 JSON merge patch to `document`.
///
/// A `null` member removes the key, so the field is unset (and inherits)
/// again; an object member merges into the table at that key, creating it or
/// replacing a non-table value; any other member replaces the value. New
/// tables are written as `[table]` sections and arrays of objects as
/// `[[table]]` arrays of tables with inline members.
///
/// TOML has no null. Inside a value the patch writes whole (a new table or an
/// array), a `null` object member is written as an absent key, as the typed
/// TOML serializer writes `None`; a `null` array element is refused.
///
/// Errors are [`ConfigError::Json`]: the patch, not the document, is at fault.
pub(crate) fn apply_merge_patch(
    document: &mut DocumentMut,
    patch: Json,
) -> Result<(), ConfigError> {
    let Json::Object(members) = patch else {
        return Err(patch_error(
            "a config patch must be a JSON object".to_string(),
        ));
    };
    merge_into_table(document.as_table_mut(), members, "")
}

/// The error for a patched document that no longer loads as a config: the
/// patch wrote a value the config schema does not accept.
pub(crate) fn invalid_patched_document(error: &toml::de::Error) -> ConfigError {
    patch_error(format!("the patched config document is invalid: {error}"))
}

fn patch_error(message: String) -> ConfigError {
    ConfigError::Json(<serde_json::Error as serde::de::Error>::custom(message))
}

fn merge_into_table(
    table: &mut Table,
    members: Map<String, Json>,
    path: &str,
) -> Result<(), ConfigError> {
    for (key, patch) in members {
        let key_path = member_path(path, &key);
        match patch {
            Json::Null => {
                table.remove(&key);
            }
            Json::Object(members) => match table.get_mut(&key) {
                Some(Item::Table(child)) => merge_into_table(child, members, &key_path)?,
                Some(Item::Value(Value::InlineTable(child))) => {
                    merge_into_inline_table(child, members, &key_path)?;
                }
                // Absent, or not a table: the member merges into an empty
                // object, as RFC 7396 specifies.
                _ => {
                    table.insert(&key, Item::Table(new_table(members, &key_path)?));
                }
            },
            patch => {
                let item = new_item(patch, &key_path)?;
                set_item(table, &key, item);
            }
        }
    }
    Ok(())
}

/// Write `item` at `key`. A value replacing a value is assigned in place:
/// the key keeps its decor (a comment line above it) and the new value takes
/// the old one's (an end-of-line comment). Any other write formats the key
/// afresh, since a key is spelled differently in `key = value` and in a
/// `[table]` header.
fn set_item(table: &mut Table, key: &str, item: Item) {
    let item = match item {
        Item::Value(value) => match table.get_mut(key) {
            Some(Item::Value(existing)) => {
                replace_value(existing, value);
                return;
            }
            _ => Item::Value(value),
        },
        item => item,
    };
    table.insert(key, item);
}

fn merge_into_inline_table(
    table: &mut InlineTable,
    members: Map<String, Json>,
    path: &str,
) -> Result<(), ConfigError> {
    let mut members_changed = false;
    for (key, patch) in members {
        let key_path = member_path(path, &key);
        match patch {
            Json::Null => members_changed |= table.remove(&key).is_some(),
            Json::Object(members) => match table.get_mut(&key) {
                Some(Value::InlineTable(child)) => {
                    merge_into_inline_table(child, members, &key_path)?;
                }
                Some(existing) => replace_value(
                    existing,
                    Value::InlineTable(new_inline_table(members, &key_path)?),
                ),
                None => {
                    table.insert(
                        key,
                        Value::InlineTable(new_inline_table(members, &key_path)?),
                    );
                    members_changed = true;
                }
            },
            patch => {
                let value = new_value(patch, &key_path)?;
                match table.get_mut(&key) {
                    Some(existing) => replace_value(existing, value),
                    None => {
                        table.insert(key, value);
                        members_changed = true;
                    }
                }
            }
        }
    }
    // The separators around a member belong to its neighbours' decor, so an
    // added or removed member leaves them misplaced: respace the table.
    if members_changed {
        table.fmt();
    }
    Ok(())
}

fn replace_value(existing: &mut Value, mut replacement: Value) {
    *replacement.decor_mut() = existing.decor().clone();
    *existing = replacement;
}

/// A value written under a standard table: an object becomes a `[table]`
/// section and a nonempty array of objects a `[[table]]` array of tables.
fn new_item(value: Json, path: &str) -> Result<Item, ConfigError> {
    match value {
        Json::Object(members) => Ok(Item::Table(new_table(members, path)?)),
        Json::Array(elements) if !elements.is_empty() && elements.iter().all(Json::is_object) => {
            let mut tables = ArrayOfTables::new();
            for (index, element) in elements.into_iter().enumerate() {
                if let Json::Object(members) = element {
                    tables.push(array_element_table(members, &format!("{path}[{index}]"))?);
                }
            }
            Ok(Item::ArrayOfTables(tables))
        }
        value => Ok(Item::Value(new_value(value, path)?)),
    }
}

/// A new standard table. One holding only sub-tables writes no header of its
/// own; an empty one keeps its header, since a present section can carry
/// meaning (a `[realm.<id>]` section declares the realm).
fn new_table(members: Map<String, Json>, path: &str) -> Result<Table, ConfigError> {
    let mut table = Table::new();
    for (key, value) in members {
        if value.is_null() {
            continue;
        }
        let item = new_item(value, &member_path(path, &key))?;
        table.insert(&key, item);
    }
    let only_sub_tables = !table.is_empty()
        && table
            .iter()
            .all(|(_, item)| item.is_table() || item.is_array_of_tables());
    table.set_implicit(only_sub_tables);
    Ok(table)
}

/// An array-of-tables element: its members are written inline.
fn array_element_table(members: Map<String, Json>, path: &str) -> Result<Table, ConfigError> {
    let mut table = Table::new();
    for (key, value) in members {
        if value.is_null() {
            continue;
        }
        let value = new_value(value, &member_path(path, &key))?;
        table.insert(&key, Item::Value(value));
    }
    Ok(table)
}

/// An inline value: an object becomes an inline table, an array an inline
/// array.
fn new_value(value: Json, path: &str) -> Result<Value, ConfigError> {
    match value {
        // Object members that are null never get here (they are removals or
        // absent keys); only array elements can.
        Json::Null => Err(patch_error(format!(
            "`{path}` is a null array element, which TOML cannot store"
        ))),
        Json::Bool(flag) => Ok(Value::from(flag)),
        Json::Number(number) => number_value(&number, path),
        Json::String(text) => Ok(Value::from(text)),
        Json::Array(elements) => {
            let mut array = Array::new();
            for (index, element) in elements.into_iter().enumerate() {
                array.push(new_value(element, &format!("{path}[{index}]"))?);
            }
            Ok(Value::Array(array))
        }
        Json::Object(members) => Ok(Value::InlineTable(new_inline_table(members, path)?)),
    }
}

fn new_inline_table(members: Map<String, Json>, path: &str) -> Result<InlineTable, ConfigError> {
    let mut table = InlineTable::new();
    for (key, value) in members {
        if value.is_null() {
            continue;
        }
        let value = new_value(value, &member_path(path, &key))?;
        table.insert(key, value);
    }
    Ok(table)
}

/// TOML integers are `i64` and floats `f64`.
fn number_value(number: &Number, path: &str) -> Result<Value, ConfigError> {
    if let Some(integer) = number.as_i64() {
        return Ok(Value::from(integer));
    }
    if number.is_f64()
        && let Some(float) = number.as_f64()
    {
        return Ok(Value::from(float));
    }
    Err(patch_error(format!(
        "`{path}` = {number} is outside the TOML integer range"
    )))
}

fn member_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn patched(content: &str, patch: Json) -> String {
        let mut document = parse(content).expect("test document parses");
        apply_merge_patch(&mut document, patch).expect("patch applies");
        document.to_string()
    }

    fn refusal(content: &str, patch: Json) -> String {
        let mut document = parse(content).expect("test document parses");
        match apply_merge_patch(&mut document, patch) {
            Err(error @ ConfigError::Json(_)) => error.to_string(),
            other => panic!("expected a typed patch refusal, got {other:?}"),
        }
    }

    #[test]
    fn replaces_named_values_keeping_comments_and_order() {
        let content = "# notes\nmax_tokens = 100 # cap\n\n[tools]\n# keep low\nmax_concurrent = 4   # host\nshell_enabled = true\n";
        assert_eq!(
            patched(
                content,
                serde_json::json!({ "max_tokens": 200, "tools": { "max_concurrent": 2 } })
            ),
            "# notes\nmax_tokens = 200 # cap\n\n[tools]\n# keep low\nmax_concurrent = 2   # host\nshell_enabled = true\n"
        );
    }

    #[test]
    fn null_removes_keys_from_standard_inline_and_dotted_tables() {
        let content = "tools.shell_enabled = true\ntools.max_concurrent = 3\n\
            provider_tools = { openai = { web_search = false }, gemini = { google_search = false } }\n\n\
            [retry]\nmax_retries = 7\nmultiplier = 3.0\n";
        assert_eq!(
            patched(
                content,
                serde_json::json!({
                    "tools": { "max_concurrent": null },
                    "provider_tools": { "gemini": null },
                    "retry": { "max_retries": null },
                    "absent": null,
                })
            ),
            "tools.shell_enabled = true\n\
            provider_tools = { openai = { web_search = false } }\n\n\
            [retry]\nmultiplier = 3.0\n"
        );
    }

    #[test]
    fn objects_merge_into_existing_tables_of_every_style() {
        let content = "tools.shell_enabled = true\n\
            provider_tools = { openai = { web_search = false } }\n\n\
            [skills]\nenabled = false\n";
        assert_eq!(
            patched(
                content,
                serde_json::json!({
                    "tools": { "max_concurrent": 3 },
                    "provider_tools": { "anthropic": { "web_search": false } },
                    "skills": { "inventory_threshold": 5 },
                })
            ),
            "tools.shell_enabled = true\ntools.max_concurrent = 3\n\
            provider_tools = { openai = { web_search = false }, anthropic = { web_search = false } }\n\n\
            [skills]\nenabled = false\ninventory_threshold = 5\n"
        );
    }

    #[test]
    fn new_sections_are_tables_and_arrays_of_tables_with_inline_members() {
        let content = "[realm.child]\nparent = \"parent\"\n";
        assert_eq!(
            patched(
                content,
                serde_json::json!({
                    // Sorted keys: new members are written in the patch
                    // object's iteration order.
                    "realm": { "global": {} },
                    "tools": { "mcp_servers": [{
                        "args": ["--read-only"],
                        "command": "sentinel-mcp",
                        "connect_timeout_secs": null,
                        "env": { "MODE": "ro", "UNSET": null },
                        "name": "sentinel",
                    }]},
                })
            ),
            "[realm.child]\nparent = \"parent\"\n\n\
            [realm.global]\n\n\
            [[tools.mcp_servers]]\nargs = [\"--read-only\"]\ncommand = \"sentinel-mcp\"\n\
            env = { MODE = \"ro\" }\nname = \"sentinel\"\n"
        );
    }

    #[test]
    fn values_and_tables_replace_each_other() {
        let content = "[tools]\ntool_timeouts = 5\n\n[shell]\nprogram = \"nu\"\n";
        assert_eq!(
            patched(
                content,
                serde_json::json!({
                    "tools": { "tool_timeouts": { "search": "30s" } },
                    "shell": "bash",
                })
            ),
            "shell = \"bash\"\n[tools]\n\n[tools.tool_timeouts]\nsearch = \"30s\"\n"
        );
    }

    #[test]
    fn refuses_patches_toml_cannot_store() {
        assert!(refusal("", serde_json::json!(5)).contains("must be a JSON object"));
        assert!(
            refusal("", serde_json::json!({ "tools": { "x": [1, null] } }))
                .contains("`tools.x[1]` is a null array element")
        );
        assert!(
            refusal("", serde_json::json!({ "limits": { "budget": u64::MAX } })).contains(
                "`limits.budget` = 18446744073709551615 is outside the TOML integer range"
            )
        );
    }
}
