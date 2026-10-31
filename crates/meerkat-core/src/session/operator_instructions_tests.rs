//! Rows for operator-instruction entries: vocabulary, the byte-stable
//! entry render, re-render validation, identity exclusivity and the
//! inherited (fork child) projection.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::operator_instructions::{
    ActiveOperatorInstruction, INSTRUCTION_ENTRY_RENDER_VERSION_V1, InstructionEntryKind,
    InstructionRevision, InstructionSlot, OperatorInstructionChangeContent,
    OperatorInstructionEntryHeader, effective_operator_instructions,
    materialize_operator_instruction_messages, mint_operator_instruction_delta,
    mint_operator_instruction_snapshot, validate_operator_instruction_messages,
};
use super::{Session, TranscriptMessages};
use crate::types::{
    InstructionActivationId, InstructionActivationIdentity, InstructionContentDigest,
    InstructionKey, InstructionNamespace, InstructionRevisionId, InstructionRevisionRef, Message,
    MessageTimestamp, SessionId, SystemMessage, SystemMessageIdentity, SystemPromptKey,
    SystemPromptVersion, SystemPromptVersionIdentity, UserMessage,
};

fn at() -> MessageTimestamp {
    chrono::DateTime::<chrono::Utc>::UNIX_EPOCH
}

fn slot(namespace: &str, key: &str) -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new(namespace).unwrap(),
        key: InstructionKey::new(key).unwrap(),
    }
}

fn revision(value: u64) -> InstructionRevision {
    InstructionRevision::new(value).unwrap()
}

fn active(namespace: &str, key: &str, rev: u64, body: &str) -> ActiveOperatorInstruction {
    ActiveOperatorInstruction {
        slot: slot(namespace, key),
        revision: revision(rev),
        body: body.to_string(),
    }
}

fn activate(
    namespace: &str,
    key: &str,
    rev: u64,
    supersedes: Option<u64>,
    body: &str,
) -> OperatorInstructionChangeContent {
    OperatorInstructionChangeContent::Activate {
        slot: slot(namespace, key),
        revision: revision(rev),
        supersedes: supersedes.map(revision),
        body: body.to_string(),
    }
}

fn revoke(namespace: &str, key: &str, rev: u64, revokes: u64) -> OperatorInstructionChangeContent {
    OperatorInstructionChangeContent::Revoke {
        slot: slot(namespace, key),
        revision: revision(rev),
        revokes: revision(revokes),
    }
}

fn header(origin: &SessionId, entry_sequence: u64) -> OperatorInstructionEntryHeader {
    OperatorInstructionEntryHeader {
        origin_session_id: origin.clone(),
        entry_sequence,
        target_generation: entry_sequence + 10,
        run_id: None,
    }
}

/// A session whose head prompt is followed by one delta that activates two
/// slots, a user turn, and a second delta that supersedes one slot and
/// revokes the other.
fn session_with_two_deltas() -> Session {
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    let first = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![
            active("app.example", "tone", 1, "Answer in a formal tone."),
            active("app.example", "units", 1, "Use metric units."),
        ],
        vec![
            activate("app.example", "tone", 1, None, "Answer in a formal tone."),
            activate("app.example", "units", 1, None, "Use metric units."),
        ],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(first).unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    let second = mint_operator_instruction_delta(
        header(session.id(), 2),
        vec![active("app.example", "tone", 2, "Answer in a casual tone.")],
        vec![
            activate(
                "app.example",
                "tone",
                2,
                Some(1),
                "Answer in a casual tone.",
            ),
            revoke("app.example", "units", 2, 1),
        ],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(second).unwrap();
    session
}

fn entry_rows(messages: &[Message]) -> Vec<&SystemMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system) if system.instruction_entry.is_some() => Some(system),
            _ => None,
        })
        .collect()
}

#[test]
fn entry_render_is_independent_of_insertion_order() {
    let origin = SessionId::new();
    let tone = active("app.example", "tone", 2, "Answer in a casual tone.");
    let units = active("app.example", "units", 1, "Use metric units.");
    let other = active("other.example", "tone", 5, "Keep replies short.");
    let changes = vec![
        activate("app.example", "units", 1, None, "Use metric units."),
        revoke("app.example", "zone", 4, 3),
        activate("other.example", "tone", 5, Some(4), "Keep replies short."),
    ];
    let forward = mint_operator_instruction_delta(
        header(&origin, 3),
        vec![tone.clone(), units.clone(), other.clone()],
        changes.clone(),
        at(),
    )
    .unwrap();
    let mut reversed = changes;
    reversed.reverse();
    let backward = mint_operator_instruction_delta(
        header(&origin, 3),
        vec![other.clone(), units.clone(), tone.clone()],
        reversed,
        at(),
    )
    .unwrap();
    assert_eq!(forward, backward, "the same changes render one entry");

    let content = &forward.content;
    assert!(content.starts_with("[meerkat-operator-instructions-v1]\n"));
    assert!(content.contains("\nkind=\"delta\"\n"));
    let active_lines = [
        "\nactive=\"app.example\" \"tone\" 2\n",
        "\nactive=\"app.example\" \"units\" 1\n",
        "\nactive=\"other.example\" \"tone\" 5\n",
    ]
    .map(|line| {
        content
            .find(line)
            .unwrap_or_else(|| panic!("{line:?} in {content}"))
    });
    assert!(
        active_lines.windows(2).all(|pair| pair[0] < pair[1]),
        "active lines are sorted by namespace, then key: {content}"
    );
    let sections = [
        "change=\"activate\"\nnamespace=\"app.example\"\nkey=\"units\"\n",
        "change=\"revoke\"\nnamespace=\"app.example\"\nkey=\"zone\"\n",
        "change=\"activate\"\nnamespace=\"other.example\"\nkey=\"tone\"\n",
    ]
    .map(|section| {
        content
            .find(section)
            .unwrap_or_else(|| panic!("{section:?} in {content}"))
    });
    assert!(
        sections.windows(2).all(|pair| pair[0] < pair[1]),
        "change sections are sorted by slot: {content}"
    );
    for forbidden in [origin.to_string(), "1970".to_string()] {
        assert!(
            !content.contains(&forbidden),
            "the render names no session id and no timestamp: {content}"
        );
    }

    let snapshot_forward = mint_operator_instruction_snapshot(
        header(&origin, 4),
        vec![tone.clone(), other.clone(), units.clone()],
        at(),
    )
    .unwrap()
    .expect("a non-empty set folds to one row");
    let snapshot_backward =
        mint_operator_instruction_snapshot(header(&origin, 4), vec![units, other, tone], at())
            .unwrap()
            .expect("a non-empty set folds to one row");
    assert_eq!(snapshot_forward, snapshot_backward);
    assert!(
        snapshot_forward
            .content
            .contains("\nkind=\"snapshot\"\nactive_count=3\n")
    );
}

#[test]
fn entry_render_survives_serde_and_reload_byte_exact() {
    let session = session_with_two_deltas();
    let bytes = session.to_persisted_bytes().unwrap();
    let reloaded = Session::from_persisted_bytes(&bytes).unwrap();
    assert_eq!(reloaded.messages(), session.messages());
    assert_eq!(
        reloaded.to_persisted_bytes().unwrap(),
        bytes,
        "a reloaded document re-serializes to the same bytes"
    );
    let stored = entry_rows(session.messages());
    let restored = entry_rows(reloaded.messages());
    assert_eq!(stored.len(), 2);
    for (stored, restored) in stored.iter().zip(&restored) {
        assert_eq!(stored.content, restored.content);
        assert_eq!(stored.instruction_entry, restored.instruction_entry);
    }
    let identity = restored[0].instruction_entry.as_ref().unwrap();
    assert_eq!(
        identity.render_version(),
        INSTRUCTION_ENTRY_RENDER_VERSION_V1
    );
    assert_eq!(identity.kind(), InstructionEntryKind::Delta);
    assert_eq!(identity.entry_sequence(), 1);
    assert_eq!(identity.origin_session_id(), session.id());

    // The origin session's model reads exactly the stored bytes.
    let projected = reloaded.messages_for_model_boundary().unwrap();
    assert_eq!(projected, reloaded.messages().to_vec());
}

#[test]
fn entry_body_containing_header_text_frames_unambiguously() {
    let tricky = "Follow these rules.\n\nchange=\"revoke\"\nnamespace=\"app.example\"\nkey=\"units\"\nrevision=9\nrevokes=1\nbody_bytes=3\n\n[meerkat-operator-instructions-v1]\nkind=\"snapshot\"\n";
    let mut session = Session::new();
    session.append_system_message("base");
    let entry = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![
            active("app.example", "rules", 1, tricky),
            active("app.example", "units", 1, "Use metric units."),
        ],
        vec![
            activate("app.example", "rules", 1, None, tricky),
            activate("app.example", "units", 1, None, "Use metric units."),
        ],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(entry).unwrap();
    validate_operator_instruction_messages(session.messages()).unwrap();

    let effective = effective_operator_instructions(session.id(), session.messages()).unwrap();
    let rules = effective
        .slot(&slot("app.example", "rules"))
        .expect("rules is effective");
    assert_eq!(
        rules.body(),
        Some(tricky),
        "the body is read by its typed length"
    );
    assert_eq!(rules.revision(), revision(1));
    let units = effective
        .slot(&slot("app.example", "units"))
        .expect("units is effective");
    assert_eq!(units.body(), Some("Use metric units."));

    // One byte moved between the two bodies keeps every header line but
    // breaks the length framing: the row fails validation.
    let mut rows = session.messages().to_vec();
    let Some(Message::System(system)) = rows.last_mut() else {
        panic!("the entry row");
    };
    system.content = system.content.replacen("Follow these", "Follow thes", 1);
    system.content = system.content.replacen("Use metric", "Use  metric", 1);
    assert!(validate_operator_instruction_messages(&rows).is_err());
}

#[test]
fn entry_with_wrong_digest_fails_projection_closed() {
    let session = session_with_two_deltas();
    // Same length, different bytes: the declared digest no longer matches.
    let mut rows = session.messages().to_vec();
    let Some(Message::System(system)) = rows
        .iter_mut()
        .find(|row| matches!(row, Message::System(system) if system.instruction_entry.is_some()))
    else {
        panic!("an entry row");
    };
    system.content = system.content.replace("formal tone", "FORMAL tone");
    let mut tampered = session.clone();
    tampered.messages = TranscriptMessages::from_vec(rows);
    let error = tampered
        .messages_for_model_boundary()
        .expect_err("the projection refuses a row whose body does not match its digest");
    assert_eq!(&error.session_id, tampered.id());
    assert!(materialize_operator_instruction_messages(tampered.id(), tampered.messages()).is_err());

    // A document carrying the tampered row is refused at decode.
    let mut value: serde_json::Value =
        serde_json::from_slice(&session.to_persisted_bytes().unwrap()).unwrap();
    let messages = value["messages"].as_array_mut().unwrap();
    let entry = messages
        .iter_mut()
        .find(|message| message.get("instruction_entry").is_some())
        .unwrap();
    let content = entry["content"]
        .as_str()
        .unwrap()
        .replace("formal tone", "FORMAL tone");
    entry["content"] = serde_json::Value::String(content);
    assert!(Session::from_persisted_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn instruction_entry_identity_is_exclusive_with_other_system_identities() {
    let session = session_with_two_deltas();
    let row = entry_rows(session.messages())[0].clone();
    let activation = InstructionActivationIdentity {
        activation_id: InstructionActivationId::new("act-1").unwrap(),
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new("app.example").unwrap(),
            key: InstructionKey::new("tone").unwrap(),
            revision_id: InstructionRevisionId::new("rev-1").unwrap(),
            content_sha256: InstructionContentDigest::for_body("x"),
        },
        supersedes: None,
        origin_session_id: session.id().clone(),
        render_version: 1,
    };
    let mut with_append_identity = row.clone();
    with_append_identity.identity = Some(SystemMessageIdentity {
        source: Some("host".to_string()),
        idempotency_key: None,
    });
    let mut with_prompt_version = row.clone();
    with_prompt_version.prompt_version = Some(SystemPromptVersionIdentity {
        key: SystemPromptKey::new("primary").unwrap(),
        version: SystemPromptVersion::INITIAL,
    });
    let mut with_activation = row;
    with_activation.instruction_activation = Some(activation);
    for (what, forged) in [
        ("append identity", with_append_identity),
        ("prompt version", with_prompt_version),
        ("instruction activation", with_activation),
    ] {
        let messages = vec![Message::System(forged)];
        let error = super::validate_instruction_activation_messages(&messages).expect_err(what);
        assert!(error.contains("mutually exclusive"), "{what}: {error}");

        let mut document = session.clone();
        document.messages = TranscriptMessages::from_vec(messages);
        let value: serde_json::Value =
            serde_json::to_value(&document).expect("serialize the forged document");
        assert!(
            serde_json::from_value::<Session>(value).is_err(),
            "{what}: a document carrying both identities is refused at decode"
        );
    }
}

#[test]
fn snapshot_of_equal_active_sets_is_byte_identical_across_sessions() {
    let first_session = SessionId::new();
    let second_session = SessionId::new();
    let set = [
        active("app.example", "tone", 3, "Answer in a casual tone."),
        active("app.example", "units", 1, "Use metric units."),
        active(
            "ops.example",
            "escalation",
            7,
            "Escalate outages to the on-call host.",
        ),
    ];
    let first = mint_operator_instruction_snapshot(
        OperatorInstructionEntryHeader {
            origin_session_id: first_session,
            entry_sequence: 4,
            target_generation: 9,
            run_id: Some(crate::lifecycle::RunId::new()),
        },
        set.to_vec(),
        at(),
    )
    .unwrap()
    .unwrap();
    let mut reordered = set.to_vec();
    reordered.rotate_left(1);
    let second = mint_operator_instruction_snapshot(
        OperatorInstructionEntryHeader {
            origin_session_id: second_session,
            entry_sequence: 11,
            target_generation: 2,
            run_id: None,
        },
        reordered,
        chrono::Utc::now(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        first.content, second.content,
        "a snapshot's bytes are a pure function of the active set"
    );
    assert_ne!(first.instruction_entry, second.instruction_entry);
    assert!(
        mint_operator_instruction_snapshot(header(&SessionId::new(), 1), Vec::new(), at())
            .unwrap()
            .is_none(),
        "an empty active set renders no row"
    );
}

#[test]
fn inherited_entry_rows_project_without_bodies() {
    let parent = session_with_two_deltas();
    let child = parent.fork();
    let projected =
        materialize_operator_instruction_messages(child.id(), child.messages()).unwrap();
    let inherited = entry_rows(&projected);
    assert_eq!(inherited.len(), 2);
    for row in &inherited {
        assert!(
            row.content
                .starts_with("[meerkat-inherited-operator-instructions-v1]\n"),
            "{}",
            row.content
        );
        assert!(row.content.ends_with("status=not_active_in_child"));
        for body in ["formal tone", "metric units", "casual tone"] {
            assert!(
                !row.content.contains(body),
                "an inherited row carries no body: {}",
                row.content
            );
        }
    }
    let child_view = child.messages_for_model_boundary().unwrap();
    assert!(
        entry_rows(&child_view).iter().all(|row| row
            .content
            .starts_with("[meerkat-inherited-operator-instructions-v1]")),
        "the child's model reads the inherited form"
    );
    // The parent still reads its own rows, bodies included.
    let parent_view = parent.messages_for_model_boundary().unwrap();
    assert!(
        entry_rows(&parent_view)
            .iter()
            .any(|row| row.content.contains("casual tone"))
    );
    // Inherited rows are not effective in the child.
    let effective = effective_operator_instructions(child.id(), child.messages()).unwrap();
    assert!(effective.is_empty());
    let parent_effective = effective_operator_instructions(parent.id(), parent.messages()).unwrap();
    assert_eq!(parent_effective.active_count(), 1);
    assert_eq!(parent_effective.last_entry_sequence(), 2);
}

#[test]
fn entry_chronology_refuses_a_broken_supersedes_chain() {
    let mut session = Session::new();
    let first = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![active("app.example", "tone", 1, "Answer in a formal tone.")],
        vec![activate(
            "app.example",
            "tone",
            1,
            None,
            "Answer in a formal tone.",
        )],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(first).unwrap();
    // A second activation that names no predecessor although revision 1 is
    // active, and one that repeats the entry sequence, are both refused.
    let unchained = mint_operator_instruction_delta(
        header(session.id(), 2),
        vec![active("app.example", "tone", 2, "Answer in a casual tone.")],
        vec![activate(
            "app.example",
            "tone",
            2,
            None,
            "Answer in a casual tone.",
        )],
        at(),
    )
    .unwrap();
    assert!(
        session
            .append_operator_instruction_entry(unchained)
            .is_err()
    );
    let repeated = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![active("app.example", "tone", 2, "Answer in a casual tone.")],
        vec![activate(
            "app.example",
            "tone",
            2,
            Some(1),
            "Answer in a casual tone.",
        )],
        at(),
    )
    .unwrap();
    assert!(session.append_operator_instruction_entry(repeated).is_err());
    assert_eq!(
        entry_rows(session.messages()).len(),
        1,
        "refusals change nothing"
    );
}

/// A generic rewrite of `start..end` with `replacement` is refused as a
/// change of an operator-instruction entry, and the refusal changes nothing.
fn assert_refused(session: &mut Session, start: usize, end: usize, replacement: Vec<Message>) {
    use crate::{TranscriptEditError, TranscriptRewriteReason, TranscriptRewriteSelection};

    let before = session.messages().to_vec();
    let error = session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start, end },
            replacement,
            TranscriptRewriteReason::new("attempt an entry rewrite"),
            Some("unit-test".to_string()),
            None,
        )
        .expect_err("the rewrite is refused");
    assert!(
        matches!(
            &error,
            TranscriptEditError::InvalidTranscriptShape(message)
                if message.contains("operator instruction entr")
        ),
        "{error:?}"
    );
    assert_eq!(
        session.messages(),
        before.as_slice(),
        "a refusal changes nothing"
    );
}

/// Only a run start and a compaction fold write entry rows. A generic
/// rewrite (and a restore, which commits through the same guard) cannot
/// erase one, move one, or mint one, even a row that would continue the
/// entry chronology.
#[test]
fn generic_rewrite_cannot_mint_move_or_erase_entry_rows() {
    // [head prompt, delta 1, user turn, delta 2]
    let mut session = session_with_two_deltas();
    let rows = session.messages().to_vec();
    assert_eq!(entry_rows(&rows).len(), 2);

    // Erase the newest entry.
    assert_refused(&mut session, 3, 4, Vec::new());
    // Move it before the user turn it follows.
    assert_refused(&mut session, 2, 4, vec![rows[3].clone(), rows[2].clone()]);
    // Mint a snapshot of what is in effect, with the next entry sequence.
    let snapshot = mint_operator_instruction_snapshot(
        header(session.id(), 3),
        vec![active("app.example", "tone", 2, "Answer in a casual tone.")],
        at(),
    )
    .unwrap()
    .expect("a non-empty set renders a snapshot");
    assert_refused(&mut session, 4, 4, vec![Message::System(snapshot)]);
}

/// An entry keeps its order among the System rows of its gap (the rows
/// between two conversation rows) that the model reads, as well as its
/// place among the conversation rows. A generic rewrite cannot put the
/// first-run entry before the head prompt, an ordinary System row, the first
/// version of a keyed prompt (which stands where the row it adopted stood)
/// or the newest version of a keyed prompt (the only one the model reads,
/// even while an older version still stands before the entry), nor move a
/// snapshot off the end of the leading System prefix.
#[test]
fn generic_rewrite_cannot_move_an_entry_among_the_system_rows_of_its_gap() {
    // [head prompt, delta 1, user turn, delta 2]: delta 1 before the prompt.
    let mut session = session_with_two_deltas();
    let rows = session.messages().to_vec();
    assert_refused(&mut session, 0, 2, vec![rows[1].clone(), rows[0].clone()]);

    // [keyed head prompt, delta 1, user turn]: delta 1 before the prompt's
    // first version.
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session
        .update_system_prompt(prompt_update(None, Some(0), "Prompt version one."))
        .expect("the head prompt is adopted in place");
    let entry = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![active("app.example", "tone", 1, "Answer in a formal tone.")],
        vec![activate(
            "app.example",
            "tone",
            1,
            None,
            "Answer in a formal tone.",
        )],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(entry).unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    let rows = session.messages().to_vec();
    assert_refused(&mut session, 0, 2, vec![rows[1].clone(), rows[0].clone()]);

    // [prompt version one, prompt version two, delta 1, user turn]: delta 1
    // before version two, the version the model reads. Version one still
    // stands before the entry, but the model would read the entry before the
    // head prompt.
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session
        .update_system_prompt(prompt_update(None, Some(0), "Prompt version one."))
        .expect("the head prompt is adopted in place");
    session
        .update_system_prompt(prompt_update(Some(1), None, "Prompt version two."))
        .expect("a later version is inserted right after the current one");
    let entry = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![active("app.example", "tone", 1, "Answer in a formal tone.")],
        vec![activate(
            "app.example",
            "tone",
            1,
            None,
            "Answer in a formal tone.",
        )],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(entry).unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    let rows = session.messages().to_vec();
    assert_refused(&mut session, 1, 3, vec![rows[2].clone(), rows[1].clone()]);

    // [head prompt, context, snapshot, user turn]: the snapshot before the
    // context row, off the end of the leading System prefix.
    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    session.append_system_message("The user works in a laboratory.");
    let snapshot = mint_operator_instruction_snapshot(
        header(session.id(), 1),
        vec![active("app.example", "tone", 1, "Answer in a formal tone.")],
        at(),
    )
    .unwrap()
    .expect("a non-empty set renders a snapshot");
    session.append_operator_instruction_entry(snapshot).unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    let rows = session.messages().to_vec();
    assert_refused(&mut session, 1, 3, vec![rows[2].clone(), rows[1].clone()]);
}

fn prompt_update(
    expected_version: Option<u64>,
    target_message_index: Option<usize>,
    content: &str,
) -> super::SystemPromptUpdateRequest {
    super::SystemPromptUpdateRequest {
        key: SystemPromptKey::new("primary").unwrap(),
        expected_version: expected_version
            .map(|version| SystemPromptVersion::new(version).unwrap()),
        target_message_index,
        content: content.to_string(),
        actor: Some("host-test".to_string()),
        expected_parent_revision: None,
    }
}

/// A keyed system-prompt update inserts each later version right after the
/// current one, so before the entry rows that follow the head prompt. Entry
/// rows are told apart by their identity and their place among the
/// conversation rows, not by their offset: two later versions both commit,
/// the entry row is unchanged and still reaches the model after the newest
/// prompt, and every rewrite record of the reloaded graph validates.
#[test]
fn update_system_prompt_inserts_a_later_version_before_entry_rows() {
    use super::SystemPromptUpdateStatus;

    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    let adopted = session
        .update_system_prompt(prompt_update(None, Some(0), "Prompt version one."))
        .expect("the head prompt is adopted in place");
    assert_eq!(adopted.version, SystemPromptVersion::INITIAL);
    let entry = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![active("app.example", "tone", 1, "Answer in a formal tone.")],
        vec![activate(
            "app.example",
            "tone",
            1,
            None,
            "Answer in a formal tone.",
        )],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(entry).unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    let entry_row = session.messages()[1].clone();
    assert_eq!(entry_rows(std::slice::from_ref(&entry_row)).len(), 1);

    for (expected, content) in [(1, "Prompt version two."), (2, "Prompt version three.")] {
        let result = session
            .update_system_prompt(prompt_update(Some(expected), None, content))
            .unwrap_or_else(|error| {
                panic!("a later prompt version commits before the entry row: {error}")
            });
        assert_eq!(result.status, SystemPromptUpdateStatus::Applied);
        assert_eq!(result.version.get(), expected + 1);
    }
    let rows = session.messages().to_vec();
    assert_eq!(
        rows.len(),
        5,
        "three prompt versions, the entry row and the user turn"
    );
    assert_eq!(rows[3], entry_row, "the entry row is unchanged");
    assert_eq!(entry_rows(&rows).len(), 1);

    let projected = session.messages_for_model_boundary().unwrap();
    assert!(
        matches!(
            &projected[0],
            Message::System(system) if system.content == "Prompt version three."
        ),
        "the model reads the newest prompt: {projected:?}"
    );
    assert_eq!(projected[1], entry_row, "and the entry row after it");

    let reloaded = Session::from_persisted_bytes(&session.to_persisted_bytes().unwrap())
        .expect("the reloaded rewrite graph validates");
    assert_eq!(reloaded.messages(), session.messages());
    let history = reloaded
        .validated_transcript_history_state()
        .unwrap()
        .expect("the session holds a rewrite graph");
    assert_eq!(
        history.commit_count(),
        3,
        "the adoption and the two later versions"
    );
    for commit in history.commits() {
        let parent = history.materialize_rewrite_parent(commit).unwrap();
        let child = history.materialize_rewrite_child(commit).unwrap();
        crate::TranscriptRewriteRecord::new(commit.clone(), parent, child)
            .expect("the rewrite record validates with both of its bodies");
    }
}

/// A keyed system-prompt update adopts the ordinary head prompt an entry
/// follows: the prompt's first version stands where the row it adopted
/// stood, so the entry keeps its order among the System rows of its gap,
/// and a later version is inserted before the entry. The entry rows are
/// unchanged, and every rewrite record of the reloaded graph validates.
#[test]
fn update_system_prompt_adopts_the_head_prompt_an_entry_follows() {
    // [head prompt, delta 1, user turn, delta 2]
    let mut session = session_with_two_deltas();
    let rows_before = session.messages().to_vec();
    let adopted = session
        .update_system_prompt(prompt_update(None, Some(0), "Prompt version one."))
        .expect("the head prompt an entry follows is adopted in place");
    assert_eq!(adopted.version, SystemPromptVersion::INITIAL);
    let later = session
        .update_system_prompt(prompt_update(Some(1), None, "Prompt version two."))
        .expect("a later version commits before the entry row");
    assert_eq!(later.version.get(), 2);
    assert_eq!(
        entry_rows(session.messages()),
        entry_rows(&rows_before),
        "the entry rows are unchanged"
    );

    let reloaded = Session::from_persisted_bytes(&session.to_persisted_bytes().unwrap())
        .expect("the reloaded rewrite graph validates");
    assert_eq!(reloaded.messages(), session.messages());
    let history = reloaded
        .validated_transcript_history_state()
        .unwrap()
        .expect("the session holds a rewrite graph");
    assert_eq!(
        history.commit_count(),
        2,
        "the adoption and the later version"
    );
    for commit in history.commits() {
        let parent = history.materialize_rewrite_parent(commit).unwrap();
        let child = history.materialize_rewrite_child(commit).unwrap();
        crate::TranscriptRewriteRecord::new(commit.clone(), parent, child)
            .expect("the rewrite record validates with both of its bodies");
    }
}

/// The graph validator holds a rewrite record to the rule of the rewrite
/// guard: a record whose revision moves an entry among the System rows of
/// its gap (here the first-run entry before the head prompt) does not
/// validate, so a graph that holds one is refused.
#[test]
fn rewrite_record_validation_refuses_an_entry_moved_among_its_gap() {
    use crate::{TranscriptEditError, TranscriptRewriteReason, TranscriptRewriteSelection};

    let mut session = Session::new();
    session.append_system_message("You are a careful assistant.");
    let entry = mint_operator_instruction_delta(
        header(session.id(), 1),
        vec![active("app.example", "tone", 1, "Answer in a formal tone.")],
        vec![activate(
            "app.example",
            "tone",
            1,
            None,
            "Answer in a formal tone.",
        )],
        at(),
    )
    .unwrap();
    session.append_operator_instruction_entry(entry).unwrap();
    session.push(Message::User(UserMessage::text(
        "What is the boiling point?",
    )));
    // [head prompt, delta 1, user turn]: an edit of the head prompt keeps the
    // entry where it stands, so the guard commits it.
    let rows = session.messages().to_vec();
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![
                Message::System(SystemMessage::new("You are a terse assistant.")),
                rows[1].clone(),
            ],
            TranscriptRewriteReason::new("edit the head prompt"),
            Some("unit-test".to_string()),
            None,
        )
        .expect("an edit that keeps the entry in place commits");
    let history = session
        .validated_transcript_history_state()
        .unwrap()
        .expect("the session holds a rewrite graph");
    let commit = history.commits().next().expect("the edit").clone();
    let parent = history.materialize_rewrite_parent(&commit).unwrap();
    let mut child = history.materialize_rewrite_child(&commit).unwrap();
    crate::TranscriptRewriteRecord::new(commit.clone(), parent.clone(), child.clone())
        .expect("the committed record validates");

    // The same record, its revision holding the entry before the head prompt.
    let digest = |messages: &[Message]| super::transcript_messages_digest(messages).unwrap();
    child.messages.swap(0, 1);
    child.revision = digest(&child.messages);
    let mut moved = commit;
    moved.revision = child.revision.clone();
    moved.replacement_digest = digest(&child.messages[..2]);
    let error = crate::TranscriptRewriteRecord::new(moved, parent, child)
        .expect_err("a record that moves the entry is refused");
    assert!(
        matches!(
            &error,
            TranscriptEditError::HistoryStateMalformed(detail)
                if detail.contains("operator instruction entry")
        ),
        "{error:?}"
    );
}

/// A keyed system-prompt update cannot adopt an operator-instruction entry
/// row as its prompt: only a run start or a compaction fold writes one. The
/// refusal says so, instead of claiming that the row is not a System row,
/// and changes nothing.
#[test]
fn update_system_prompt_refuses_an_entry_target_as_an_operator_instruction_entry() {
    use super::SystemPromptUpdateError;
    use crate::TranscriptEditError;

    // [head prompt, delta 1, user turn, delta 2]
    let mut session = session_with_two_deltas();
    let before = session.messages().to_vec();
    let error = session
        .update_system_prompt(prompt_update(None, Some(1), "Replacement prompt."))
        .expect_err("an entry row cannot be adopted as a prompt");
    assert!(
        matches!(
            &error,
            SystemPromptUpdateError::Transcript(TranscriptEditError::InvalidTranscriptShape(detail))
                if detail.contains("operator instruction entry")
        ),
        "{error:?}"
    );
    assert_eq!(
        session.messages(),
        before.as_slice(),
        "a refusal changes nothing"
    );
}
