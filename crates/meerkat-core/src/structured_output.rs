//! Request-time visibility of a configured structured-output schema.
//!
//! When a run has an [`OutputSchema`](crate::types::OutputSchema), the model is told up front, in the
//! request's system instructions, that its final reply must be JSON matching
//! that schema. The agent loop applies this projection to every request it
//! composes for the run (main turns and extraction turns alike), so the
//! rendered instruction prefix is byte-identical across turns and provider
//! prompt caching keeps working.
//!
//! The projection is request-only: it is never written into the session
//! transcript. It is derived from the build-time schema on every request, so
//! every request the loop composes carries exactly the schema the session is
//! currently configured with, and nothing at all when no schema is configured.
//! A provider-side conversation chain that replays earlier requests (OpenAI
//! Responses `store=true` continuation) keeps whatever section those requests
//! carried, exactly as it keeps an earlier system prompt.
//!
//! This module owns the rendering and the placement. It is public so provider
//! adapter tests can compose exactly the request the agent loop sends.

use crate::types::{Message, SystemMessage};
use serde_json::Value;

/// Opening delimiter of the structured-output instruction section.
pub const OUTPUT_SCHEMA_INSTRUCTIONS_OPEN: &str = "<structured_output>";

/// Closing delimiter of the structured-output instruction section.
pub const OUTPUT_SCHEMA_INSTRUCTIONS_CLOSE: &str = "</structured_output>";

/// Separator placed between an existing leading system prompt and the
/// structured-output instruction section.
pub const OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR: &str = "\n\n";

const OUTPUT_SCHEMA_INSTRUCTIONS_BODY: &str = "Your final reply (the reply in which you call no \
tools) is parsed as structured output. It must be a single JSON value that validates against the \
JSON Schema below, with no other text and no markdown code fences. Replies that call tools are \
not affected.";

/// Render the structured-output instruction section for one output schema.
///
/// Callers holding an [`OutputSchema`](crate::types::OutputSchema) pass either its raw schema value or the
/// value the active provider compiles from it.
///
/// `display_schema` is the JSON Schema shown to the model. The agent loop
/// passes the schema exactly as the active provider compiles it for
/// validation, so the model is asked for the same shape the validator accepts.
///
/// The rendering is a pure function of `display_schema`: the same schema
/// always renders to the same bytes, which is what keeps the request prefix
/// stable across turns. The schema's optional `name` is deliberately not
/// rendered, because naming it invites the model to wrap its answer in an
/// object keyed by that name.
pub fn render_output_schema_instructions(display_schema: &Value) -> String {
    // `Value`'s `Display` is compact JSON and cannot fail, unlike
    // `serde_json::to_string`, which has to report non-string map keys that a
    // `Value` can never contain.
    let schema = display_schema.to_string();
    let mut section = String::with_capacity(
        OUTPUT_SCHEMA_INSTRUCTIONS_OPEN.len()
            + OUTPUT_SCHEMA_INSTRUCTIONS_BODY.len()
            + schema.len()
            + OUTPUT_SCHEMA_INSTRUCTIONS_CLOSE.len()
            + 32,
    );
    section.push_str(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN);
    section.push('\n');
    section.push_str(OUTPUT_SCHEMA_INSTRUCTIONS_BODY);
    section.push_str("\nJSON Schema:\n");
    section.push_str(&schema);
    section.push('\n');
    section.push_str(OUTPUT_SCHEMA_INSTRUCTIONS_CLOSE);
    section
}

/// Place a rendered instruction section into the request messages.
///
/// The section is appended to the content of the first message when that
/// message is a system prompt, so every provider lowers it through the same
/// path as the system prompt itself (Anthropic `system`, Gemini
/// `systemInstruction`, OpenAI Responses system input or ChatGPT
/// `instructions`, Chat Completions `system`). When the request has no leading
/// system message, one is inserted at the front.
///
/// Only the first message is ever touched, so the section sits at the same
/// place in every request of a run and the prompt-cache prefix stays stable.
pub fn project_output_schema_instructions(messages: &mut Vec<Message>, instructions: &str) {
    if let Some(Message::System(system)) = messages.first_mut() {
        if system.content.is_empty() {
            system.content = instructions.to_string();
        } else {
            let mut content = String::with_capacity(
                system.content.len()
                    + OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR.len()
                    + instructions.len(),
            );
            content.push_str(&system.content);
            content.push_str(OUTPUT_SCHEMA_INSTRUCTIONS_SEPARATOR);
            content.push_str(instructions);
            system.content = content;
        }
        return;
    }
    messages.insert(0, Message::System(SystemMessage::new(instructions)));
}

/// Swap one projected instruction section for another in request messages
/// that [`project_output_schema_instructions`] already composed.
///
/// The projection always leaves the section at the very end of the leading
/// system prompt (either after the separator or as the whole prompt), so the
/// swap replaces exactly those trailing bytes. Returns `true` when the leading
/// system prompt ended with `previous` and now ends with `next`; otherwise the
/// messages are left untouched and `false` is returned.
pub fn replace_output_schema_instructions(
    messages: &mut [Message],
    previous: &str,
    next: &str,
) -> bool {
    let Some(Message::System(system)) = messages.first_mut() else {
        return false;
    };
    let Some(head) = system.content.strip_suffix(previous) else {
        return false;
    };
    let mut content = String::with_capacity(head.len() + next.len());
    content.push_str(head);
    content.push_str(next);
    system.content = content;
    true
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::{OutputSchema, UserMessage};
    use serde_json::json;

    fn schema() -> OutputSchema {
        OutputSchema::new(json!({
            "type": "object",
            "properties": {
                "verdict": {"type": "string", "enum": ["approve", "reject"]},
                "comments": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["verdict"]
        }))
        .expect("valid schema")
    }

    #[test]
    fn rendering_is_delimited_and_embeds_the_compact_schema() {
        let schema = schema();
        let section = render_output_schema_instructions(schema.schema.as_value());
        assert!(section.starts_with(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN));
        assert!(section.ends_with(OUTPUT_SCHEMA_INSTRUCTIONS_CLOSE));
        let compact = schema.schema.as_value().to_string();
        assert!(
            section.contains(&format!("\nJSON Schema:\n{compact}\n")),
            "the section must carry the schema as compact JSON: {section}"
        );
        assert!(!compact.contains('\n'), "compact JSON has no newlines");
    }

    #[test]
    fn rendering_is_byte_stable_for_the_same_schema() {
        let first = schema();
        let second =
            OutputSchema::from_json_str(&first.schema.as_value().to_string()).expect("round-trips");
        assert_eq!(
            render_output_schema_instructions(first.schema.as_value()),
            render_output_schema_instructions(second.schema.as_value()),
            "a schema that survives serialization renders identically"
        );
    }

    #[test]
    fn rendering_omits_the_schema_name() {
        let named = schema().with_name("advisor");
        let section = render_output_schema_instructions(named.schema.as_value());
        assert!(
            !section.contains("advisor"),
            "the schema name must not be rendered: {section}"
        );
    }

    #[test]
    fn rendering_shows_the_display_schema_not_the_raw_one() {
        let schema = schema();
        let mut compiled = schema.schema.as_value().clone();
        compiled["additionalProperties"] = json!(false);
        let section = render_output_schema_instructions(&compiled);
        assert!(section.contains("\"additionalProperties\":false"));
    }

    #[test]
    fn projection_appends_to_the_leading_system_prompt() {
        let mut messages = vec![
            Message::System(SystemMessage::new("You are a reviewer.")),
            Message::User(UserMessage::text("review this")),
        ];
        project_output_schema_instructions(&mut messages, "SECTION");
        assert_eq!(messages.len(), 2);
        match &messages[0] {
            Message::System(system) => {
                assert_eq!(system.content, "You are a reviewer.\n\nSECTION");
            }
            other => panic!("expected system prompt, got {other:?}"),
        }
        assert!(matches!(&messages[1], Message::User(_)));
    }

    #[test]
    fn projection_touches_only_the_first_of_several_leading_system_messages() {
        let mut messages = vec![
            Message::System(SystemMessage::new("first")),
            Message::System(SystemMessage::new("second")),
            Message::User(UserMessage::text("go")),
        ];
        project_output_schema_instructions(&mut messages, "SECTION");
        assert_eq!(messages.len(), 3);
        assert!(matches!(&messages[0], Message::System(s) if s.content == "first\n\nSECTION"));
        assert!(matches!(&messages[1], Message::System(s) if s.content == "second"));
    }

    #[test]
    fn projection_fills_an_empty_leading_system_prompt_without_a_separator() {
        let mut messages = vec![
            Message::System(SystemMessage::new("")),
            Message::User(UserMessage::text("go")),
        ];
        project_output_schema_instructions(&mut messages, "SECTION");
        assert!(matches!(&messages[0], Message::System(s) if s.content == "SECTION"));
    }

    #[test]
    fn projection_inserts_a_system_prompt_when_none_leads() {
        let mut messages = vec![
            Message::User(UserMessage::text("go")),
            Message::System(SystemMessage::new("late system row")),
        ];
        project_output_schema_instructions(&mut messages, "SECTION");
        assert_eq!(messages.len(), 3);
        assert!(matches!(&messages[0], Message::System(s) if s.content == "SECTION"));
        assert!(matches!(&messages[1], Message::User(_)));
        assert!(matches!(&messages[2], Message::System(s) if s.content == "late system row"));
    }

    #[test]
    fn replacement_swaps_an_appended_section() {
        let mut messages = vec![
            Message::System(SystemMessage::new("You are a reviewer.")),
            Message::User(UserMessage::text("review this")),
        ];
        project_output_schema_instructions(&mut messages, "OLD");
        assert!(replace_output_schema_instructions(
            &mut messages,
            "OLD",
            "NEW"
        ));
        assert!(
            matches!(&messages[0], Message::System(s) if s.content == "You are a reviewer.\n\nNEW")
        );
        assert!(matches!(&messages[1], Message::User(_)));
    }

    #[test]
    fn replacement_swaps_an_inserted_section() {
        let mut messages = vec![Message::User(UserMessage::text("go"))];
        project_output_schema_instructions(&mut messages, "OLD");
        assert!(replace_output_schema_instructions(
            &mut messages,
            "OLD",
            "NEW"
        ));
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], Message::System(s) if s.content == "NEW"));
    }

    #[test]
    fn replacement_leaves_messages_without_the_section_untouched() {
        let mut messages = vec![
            Message::System(SystemMessage::new("no section here")),
            Message::User(UserMessage::text("go")),
        ];
        assert!(!replace_output_schema_instructions(
            &mut messages,
            "OLD",
            "NEW"
        ));
        assert!(matches!(&messages[0], Message::System(s) if s.content == "no section here"));

        let mut messages = vec![Message::User(UserMessage::text("OLD"))];
        assert!(!replace_output_schema_instructions(
            &mut messages,
            "OLD",
            "NEW"
        ));
        assert!(matches!(&messages[0], Message::User(_)));
    }

    #[test]
    fn projection_into_an_empty_request_creates_the_system_prompt() {
        let mut messages = Vec::new();
        project_output_schema_instructions(&mut messages, "SECTION");
        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0], Message::System(s) if s.content == "SECTION"));
    }
}
