//! The fork seam of a member rewind, on the mob side. A member rewind
//! rotates a member onto a durable fork of its own session; these rows pin
//! what the rotation asks of the mob's session service and of the member's
//! comms key before any of it runs:
//!
//! - a service without durable sessions refuses the seam typed, and the
//!   durable owner and the test double both answer it;
//! - the test double forks a planned member child at a boundary, so rows of
//!   the rotation itself can run on it;
//! - the member's key pair is carried to a verified child only, once, and
//!   the file the carry writes is the file `meerkat-comms` loads.

use super::*;

use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::AtomicU32;

use meerkat_core::service::{
    MemberRewindChildObservation, MemberRewindEvidenceField, MemberRewindForkEvidence,
};

use crate::runtime::dormant_seat::{MemberSeatEndpointSource, MemberSeatEndpointsUnavailable};
use crate::runtime::member_key_carry::{
    MemberKeyCarryOutcome, MemberKeyCarryPlan, MemberKeyCarryRefusal, VerifiedRewindChild,
    carry_member_key, carry_member_key_linking, hard_link_key, plan_member_key_carry,
    verify_member_rewind_child,
};
use crate::runtime::session_service::{MemberRewindFork, MemberRewindLineageWitness};

/// The member every row rewinds.
fn rewound_member() -> meerkat_core::MobMemberBinding {
    meerkat_core::MobMemberBinding {
        mob_id: "mob-1".to_string(),
        role: "worker".to_string(),
        member: "main".to_string(),
    }
}

/// Another member of the same mob.
fn another_member() -> meerkat_core::MobMemberBinding {
    meerkat_core::MobMemberBinding {
        mob_id: "mob-1".to_string(),
        role: "worker".to_string(),
        member: "second".to_string(),
    }
}

/// Two answered turns of `binding`'s member, with the session metadata a
/// mob build writes: four messages, every even count a complete boundary.
fn member_session(binding: &meerkat_core::MobMemberBinding) -> Session {
    let mut session = Session::new();
    for (question, answer) in [
        ("first question", "first answer"),
        ("second question", "second answer"),
    ] {
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text(question),
        ));
        session.push(meerkat_core::Message::BlockAssistant(
            meerkat_core::BlockAssistantMessage::new(
                vec![meerkat_core::AssistantBlock::Text {
                    text: answer.to_string(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            ),
        ));
    }
    let mut session = factory_policy_session(session, "test-model".to_string(), 1_024);
    let mut metadata = session.session_metadata().expect("session metadata");
    metadata.comms_name = Some(binding.comms_name().expect("comms name").to_string());
    metadata.mob_member_binding = Some(binding.clone());
    metadata.tooling.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::AllowList(
        ["read_file".to_string()].into_iter().collect(),
    ));
    session
        .set_session_metadata(metadata)
        .expect("member metadata");
    session
}

/// The evidence a rewind recorded for a child, and the observation of
/// `child_session_id` as a child that shows it.
fn recorded_and_matching(
    child_session_id: &SessionId,
) -> (MemberRewindForkEvidence, MemberRewindChildObservation) {
    let recorded = MemberRewindForkEvidence {
        prefix_message_count: 2,
        prefix_digest: "digest-of-the-prefix".to_string(),
        tool_access_policy: None,
        auth_binding: None,
        realm_id: None,
    };
    let matching = MemberRewindChildObservation {
        session_id: child_session_id.clone(),
        evidence: recorded.clone(),
        member_binding: Some(rewound_member()),
        archived: false,
    };
    (recorded, matching)
}

/// A child that was verified against its record.
fn verified_child(child_session_id: &SessionId) -> VerifiedRewindChild {
    let (recorded, matching) = recorded_and_matching(child_session_id);
    verify_member_rewind_child(&recorded, &rewound_member(), &matching)
        .expect("a matching child verifies")
}

/// The names of the files in `dir`, sorted; empty when it does not exist.
fn files_in(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names = entries
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// One member key root with the old session's key pair stored in it, as the
/// member's builds keep it.
struct KeyRoot {
    root: tempfile::TempDir,
    old_session_id: SessionId,
    old_key: meerkat_comms::Keypair,
}

impl KeyRoot {
    async fn with_old_key() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let old_session_id = SessionId::new();
        let old_key =
            meerkat_comms::Keypair::load_or_generate(&root.path().join(old_session_id.to_string()))
                .await
                .expect("the old session's key pair");
        Self {
            root,
            old_session_id,
            old_key,
        }
    }

    fn dir_of(&self, session_id: &SessionId) -> PathBuf {
        self.root.path().join(session_id.to_string())
    }

    fn source(&self) -> Result<MemberSeatEndpointSource, MemberSeatEndpointsUnavailable> {
        Ok(MemberSeatEndpointSource::SessionScopedInproc {
            identity_root: self.root.path().to_path_buf(),
        })
    }

    fn recorded_key(&self) -> Option<[u8; 32]> {
        Some(*self.old_key.public_key().as_bytes())
    }

    /// The carry plan of the old session's recorded key under this root.
    async fn plan(&self) -> MemberKeyCarryPlan {
        let plan = plan_member_key_carry(self.source(), &self.old_session_id, self.recorded_key())
            .await
            .expect("a stored key under a resolved root plans a carry");
        assert_eq!(
            plan,
            MemberKeyCarryPlan::Carry {
                identity_root: self.root.path().to_path_buf(),
            }
        );
        plan
    }
}

fn unsupported_fork(result: Result<(), SessionError>, door: &str) {
    assert!(
        matches!(
            &result,
            Err(SessionError::Unsupported(reason))
                if reason == "session service does not expose durable transcript fork authority"
        ),
        "{door} refuses typed, as a durable fork does: {result:?}"
    );
}

/// A member rewind fork of the member's current session `source`, into a
/// fresh planned child.
fn fork_of_current(source: &SessionId, message_count: usize) -> MemberRewindFork {
    MemberRewindFork::of_current_session(
        rewound_member(),
        source.clone(),
        message_count,
        SessionId::new(),
    )
}

/// The witness the mob runtime builds for `session`, an earlier session of
/// the rewound member's lineage.
fn lineage_witness(session: &SessionId) -> MemberRewindLineageWitness {
    MemberRewindLineageWitness::proven_by_machine(rewound_member(), session.clone())
}

/// A member rewind fork of `source`, an earlier session of the member's
/// rewind lineage, into a fresh planned child.
fn fork_of_lineage(source: &SessionId, message_count: usize) -> MemberRewindFork {
    MemberRewindFork::of_lineage_session(lineage_witness(source), message_count, SessionId::new())
}

#[tokio::test]
async fn a_service_without_durable_sessions_refuses_the_planned_member_fork_typed() {
    let service = meerkat_session::EphemeralSessionService::new(PersistentMockBuilder, 16);
    let source = SessionId::new();
    let fork = fork_of_current(&source, 2);

    unsupported_fork(
        MobSessionService::preflight_member_rewind_fork(&service, &fork)
            .await
            .map(|_| ()),
        "the preflight",
    );
    unsupported_fork(
        MobSessionService::fork_member_rewind_child(&service, &fork)
            .await
            .map(|_| ()),
        "the planned member fork",
    );
    unsupported_fork(
        MobSessionService::observe_member_rewind_child(
            &service,
            &fork.request().planned_child_session_id,
        )
        .await
        .map(|_| ()),
        "the child observation",
    );
    unsupported_fork(
        MobSessionService::read_member_rewind_lineage_session(
            &service,
            lineage_witness(&source),
            2,
        )
        .await
        .map(|_| ()),
        "the lineage session read",
    );
}

#[tokio::test]
async fn the_test_double_forks_a_planned_member_child_at_a_boundary() {
    let service = MockSessionService::new();
    let source = member_session(&rewound_member());
    let source_id = source.id().clone();
    service.replace_persisted_session(source.clone()).await;
    let fork = fork_of_current(&source_id, 2);
    let planned = fork.request().planned_child_session_id.clone();

    // The preflight returns the evidence and writes no session.
    let evidence = service
        .preflight_member_rewind_fork(&fork)
        .await
        .expect("the double preflights a member rewind fork");
    assert_eq!(evidence.prefix_message_count, 2);
    assert_eq!(
        evidence.prefix_digest,
        source
            .transcript_prefix_digest(2)
            .expect("digest the prefix")
    );
    assert_eq!(
        evidence.tool_access_policy,
        source
            .session_metadata()
            .expect("source metadata")
            .tooling
            .tool_access_policy
    );
    assert!(
        service.persisted_session_clone(&planned).await.is_none(),
        "the preflight writes no session"
    );
    assert_eq!(
        service
            .observe_member_rewind_child(&planned)
            .await
            .expect("observe before the fork"),
        None,
        "no such child before the fork"
    );

    // The fork commits the planned child, bound to the same member.
    let observed = service
        .fork_member_rewind_child(&fork)
        .await
        .expect("the double forks the planned member child");
    assert_eq!(observed.session_id, planned);
    assert_eq!(evidence.verify_child(&rewound_member(), &observed), Ok(()));
    let child = service
        .persisted_session_clone(&planned)
        .await
        .expect("the planned child is durable");
    assert_eq!(child.id(), &planned);
    assert_eq!(
        child.messages(),
        &source.messages()[..2],
        "the child holds the exact prefix"
    );
    let child_metadata = child.session_metadata().expect("child metadata");
    assert_eq!(child_metadata.mob_member_binding, Some(rewound_member()));
    assert_eq!(
        child_metadata.comms_name,
        Some(
            rewound_member()
                .comms_name()
                .expect("comms name")
                .to_string()
        )
    );
    assert_eq!(
        service
            .observe_member_rewind_child(&planned)
            .await
            .expect("observe the child"),
        Some(observed.clone())
    );

    // A retried fork reaches the same child.
    assert_eq!(
        service
            .fork_member_rewind_child(&fork)
            .await
            .expect("the retried fork"),
        observed
    );

    // A session bound to another member is not the member's source: it is
    // answered not found.
    let others = member_session(&another_member());
    let others_id = others.id().clone();
    service.replace_persisted_session(others).await;
    let refused = service
        .preflight_member_rewind_fork(&fork_of_current(&others_id, 2))
        .await
        .expect_err("a session of another member is not the member's source");
    assert!(
        matches!(&refused, SessionError::NotFound { id } if id == &others_id),
        "{refused:?}"
    );

    // An archived source is read only through a lineage witness.
    service
        .archived_session_ids
        .write()
        .await
        .insert(source_id.clone());
    let refused = service
        .preflight_member_rewind_fork(&fork_of_current(&source_id, 2))
        .await
        .expect_err("an archived session is no current source");
    assert!(
        matches!(&refused, SessionError::NotFound { id } if id == &source_id),
        "{refused:?}"
    );
    let redo = service
        .fork_member_rewind_child(&fork_of_lineage(&source_id, 4))
        .await
        .expect("the double forks from an archived lineage session");
    assert_eq!(redo.evidence.prefix_message_count, 4);
    let read = service
        .read_member_rewind_lineage_session(lineage_witness(&source_id), 2)
        .await
        .expect("the double reads a lineage session");
    assert_eq!(read.message_count, 4);
    assert_eq!(
        read.prefix_digest,
        Some(
            source
                .transcript_prefix_digest(2)
                .expect("digest the prefix")
        )
    );
}

/// What the crate-private constructor of a fork of the current session
/// puts into it: the source is read as the member's current session.
#[test]
fn a_fork_of_the_current_session_reads_its_source_as_current() {
    let source = SessionId::new();
    let planned = SessionId::new();
    let current =
        MemberRewindFork::of_current_session(rewound_member(), source.clone(), 2, planned.clone());
    assert_eq!(
        current.request(),
        &meerkat_core::service::MemberRewindForkRequest {
            source_session_id: source,
            message_count: 2,
            planned_child_session_id: planned,
            member_binding: rewound_member(),
            source: meerkat_core::service::MemberRewindSource::Current,
        }
    );
}

/// A lineage fork takes its member and its session from the witness it
/// consumes, and nothing else can name them: it cannot ask for another
/// member's session than the one the machine proved. Its source is read
/// as an archived lineage session.
#[test]
fn a_lineage_fork_names_the_member_and_session_of_its_witness() {
    let source = SessionId::new();
    let planned = SessionId::new();
    let witness = MemberRewindLineageWitness::proven_by_machine(another_member(), source.clone());
    assert_eq!(witness.member_binding(), &another_member());
    assert_eq!(witness.session_id(), &source);
    let lineage = MemberRewindFork::of_lineage_session(witness, 4, planned.clone());
    assert_eq!(
        lineage.request(),
        &meerkat_core::service::MemberRewindForkRequest {
            source_session_id: source,
            message_count: 4,
            planned_child_session_id: planned,
            member_binding: another_member(),
            source: meerkat_core::service::MemberRewindSource::ArchivedLineage,
        },
        "a lineage fork names the witness's member and session"
    );
}

/// A refusal as it is compared: the whole typed error.
fn refusal(error: SessionError) -> String {
    format!("{error:?}")
}

/// What one service answers to one member rewind fork, asked as a rotation
/// asks: the preflight, the fork, the observation of the planned child, and
/// the child the fork committed.
#[derive(Debug, PartialEq)]
struct SeamAnswers {
    preflight: Result<MemberRewindForkEvidence, String>,
    fork: Result<MemberRewindChildObservation, String>,
    observed: Result<Option<MemberRewindChildObservation>, String>,
    child: Option<serde_json::Value>,
}

impl SeamAnswers {
    async fn of(
        service: &dyn MobSessionService,
        fork: &MemberRewindFork,
        child: impl std::future::Future<Output = Option<Session>>,
    ) -> Self {
        let preflight = service
            .preflight_member_rewind_fork(fork)
            .await
            .map_err(refusal);
        let forked = service
            .fork_member_rewind_child(fork)
            .await
            .map_err(refusal);
        let observed = service
            .observe_member_rewind_child(&fork.request().planned_child_session_id)
            .await
            .map_err(refusal);
        let child = child.await.map(|child| {
            serde_json::json!({
                "messages": child.messages(),
                "metadata": child.try_session_metadata().expect("the child's metadata decodes"),
                "build_state": child.try_build_state().expect("the child's build state decodes"),
                "fork_seed": child
                    .metadata()
                    .get(meerkat_core::fork_notice::SESSION_FORK_SEED_KEY),
            })
        });
        Self {
            preflight,
            fork: forked,
            observed,
            child,
        }
    }

    /// Whether the preflight and the fork both answered `refused`, and no
    /// child was committed.
    fn both_refused(&self, refused: &str) -> bool {
        self.preflight.as_ref().is_err_and(|error| error == refused)
            && self.fork.as_ref().is_err_and(|error| error == refused)
            && self.observed == Ok(None)
            && self.child.is_none()
    }
}

/// The durable owner and the test double, holding the same committed
/// sessions.
struct OwnerAndDouble {
    owner: meerkat_session::PersistentSessionService<PersistentMockBuilder>,
    double: MockSessionService,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    machine: meerkat_runtime::MeerkatMachine,
    /// Where the two answered differently, and where the owner answered
    /// something else than the case is about.
    findings: Vec<String>,
}

impl OwnerAndDouble {
    fn new() -> Self {
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let owner = meerkat_session::PersistentSessionService::new(
            PersistentMockBuilder,
            4,
            Arc::new(MemoryStore::new()),
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        );
        let machine =
            meerkat_runtime::MeerkatMachine::persistent(Arc::clone(&runtime_store), blob_store)
                .expect("construct runtime authority");
        Self {
            owner,
            double: MockSessionService::new(),
            runtime_store,
            machine,
            findings: Vec::new(),
        }
    }

    /// Commit `session` as the durable body both services read.
    async fn store(&self, session: &Session) -> SessionId {
        self.runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(session.id()),
                meerkat_runtime::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(session)
                        .expect("serialize the session")
                        .into(),
                },
            )
            .await
            .expect("commit the session to the owner's store");
        self.double.replace_persisted_session(session.clone()).await;
        session.id().clone()
    }

    /// Archive `id` in both services.
    async fn archive(&self, id: &SessionId) {
        self.machine
            .register_session(id.clone())
            .await
            .expect("register the session with its runtime");
        self.owner
            .archive_with_machine_protocol(
                id,
                meerkat_session::MachineSessionArchiveProtocol::from_machine(&self.machine),
            )
            .await
            .expect("archive the owner's session");
        self.double
            .archived_session_ids
            .write()
            .await
            .insert(id.clone());
    }

    /// Ask both services for `fork` and note where they differ. Returns
    /// the owner's answers.
    async fn ask(&mut self, case: &str, fork: &MemberRewindFork) -> SeamAnswers {
        let planned = &fork.request().planned_child_session_id;
        let owner = SeamAnswers::of(&self.owner, fork, async {
            self.owner
                .load_durable_session_body(planned)
                .await
                .expect("read the owner's child")
        })
        .await;
        let double = SeamAnswers::of(
            &self.double,
            fork,
            self.double.persisted_session_clone(planned),
        )
        .await;
        if owner != double {
            self.findings.push(format!(
                "{case}: the answers differ\n  owner:  {owner:?}\n  double: {double:?}"
            ));
        }
        owner
    }

    /// Ask both services for the lineage read of `session` and note where
    /// they differ. Returns the owner's answer.
    async fn ask_lineage_read(
        &mut self,
        case: &str,
        session: &SessionId,
        prefix_message_count: usize,
    ) -> Result<meerkat_core::service::MemberRewindLineageRead, String> {
        let owner = MobSessionService::read_member_rewind_lineage_session(
            &self.owner,
            lineage_witness(session),
            prefix_message_count,
        )
        .await
        .map_err(refusal);
        let double = MobSessionService::read_member_rewind_lineage_session(
            &self.double,
            lineage_witness(session),
            prefix_message_count,
        )
        .await
        .map_err(refusal);
        if owner != double {
            self.findings.push(format!(
                "{case}: the lineage reads differ\n  owner:  {owner:?}\n  double: {double:?}"
            ));
        }
        owner
    }

    /// Note that the owner's answer is not what `case` is about.
    fn expect(&mut self, case: &str, holds: bool, answered: &dyn std::fmt::Debug) {
        if !holds {
            self.findings.push(format!(
                "{case}: the owner answered something else: {answered:?}"
            ));
        }
    }
}

/// A session of `binding` between an instruction delivery and its next
/// turn: its transcript ends with an instruction activation of its own.
fn ending_with_its_own_activation(binding: &meerkat_core::MobMemberBinding) -> Session {
    let mut session = member_session(binding);
    session
        .activate_instruction(meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::InstructionRevisionRef {
                namespace: meerkat_core::InstructionNamespace::new("app.example")
                    .expect("namespace"),
                key: meerkat_core::InstructionKey::new("primary").expect("key"),
                revision_id: meerkat_core::InstructionRevisionId::new("rev-act-primary")
                    .expect("revision"),
                content_sha256: meerkat_core::InstructionContentDigest::for_body("policy primary"),
            },
            activation_id: meerkat_core::InstructionActivationId::new("act-primary")
                .expect("activation id"),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: "policy primary".to_string(),
        })
        .expect("activate the key");
    session
}

/// The messages and metadata of `session` under a new session id, without
/// its build state.
fn without_build_state(session: &Session) -> Session {
    let mut bare = Session::new();
    for message in session.messages() {
        bare.push(message.clone());
    }
    bare.set_session_metadata(session.session_metadata().expect("session metadata"))
        .expect("member metadata");
    bare
}

/// The durable owner and the test double are asked the same member rewind
/// forks over the same committed sessions, and answer the same: the same
/// evidence, the same child, the same typed refusal. Rows of the rotation
/// run on the double, so every answer of it they stand on is the owner's.
#[tokio::test]
async fn the_durable_owner_and_the_test_double_answer_the_member_rewind_fork_seam() {
    let mut both = OwnerAndDouble::new();

    // A complete boundary of the member's session: evidence, then a child
    // that shows it; the same request again reaches the same child.
    let case = "a complete boundary";
    let member = member_session(&rewound_member());
    let source = both.store(&member).await;
    let request = fork_of_current(&source, 2);
    let answers = both.ask(case, &request).await;
    let verified = match (&answers.preflight, &answers.fork) {
        (Ok(evidence), Ok(observed)) => {
            evidence.verify_child(&rewound_member(), observed) == Ok(())
                && answers.observed.as_ref() == Ok(&Some(observed.clone()))
                && answers.child.is_some()
        }
        _ => false,
    };
    both.expect(case, verified, &answers);
    let again = both.ask("the same request again", &request).await;
    both.expect("the same request again", again == answers, &again);
    let case = "a lineage read of a current session";
    let read = both.ask_lineage_read(case, &source, 2).await;
    both.expect(
        case,
        read.as_ref()
            .is_ok_and(|read| read.message_count == 4 && read.prefix_digest.is_some()),
        &read,
    );
    let case = "a lineage read past the session's end";
    let read = both.ask_lineage_read(case, &source, 9).await;
    both.expect(
        case,
        read.as_ref()
            .is_ok_and(|read| read.message_count == 4 && read.prefix_digest.is_none()),
        &read,
    );

    // A boundary past the source's end.
    let case = "a boundary past the end";
    let answers = both.ask(case, &fork_of_current(&source, 9)).await;
    both.expect(
        case,
        answers.preflight.is_err() && answers.fork.is_err() && answers.child.is_none(),
        &answers,
    );

    // Another member's session, and a session that is nobody's.
    let case = "another member's session";
    let others = both.store(&member_session(&another_member())).await;
    let answers = both.ask(case, &fork_of_current(&others, 2)).await;
    let not_found = refusal(SessionError::NotFound { id: others.clone() });
    both.expect(case, answers.both_refused(&not_found), &answers);
    let case = "a lineage read of another member's session";
    let read = both.ask_lineage_read(case, &others, 2).await;
    both.expect(case, read.as_ref() == Err(&not_found), &read);

    let case = "a session that does not exist";
    let absent = SessionId::new();
    let answers = both.ask(case, &fork_of_current(&absent, 2)).await;
    let not_found = refusal(SessionError::NotFound { id: absent.clone() });
    both.expect(case, answers.both_refused(&not_found), &answers);
    let case = "a lineage read of a session that does not exist";
    let read = both.ask_lineage_read(case, &absent, 2).await;
    both.expect(case, read.as_ref() == Err(&not_found), &read);

    let case = "a session without canonical metadata";
    let mut plain = Session::new();
    for message in member.messages() {
        plain.push(message.clone());
    }
    let plain = both.store(&plain).await;
    let answers = both.ask(case, &fork_of_current(&plain, 2)).await;
    // Nobody's member session: its existence does not show.
    let not_found = refusal(SessionError::NotFound { id: plain.clone() });
    both.expect(case, answers.both_refused(&not_found), &answers);
    let case = "a lineage read of a session without canonical metadata";
    let read = both.ask_lineage_read(case, &plain, 2).await;
    both.expect(case, read.as_ref() == Err(&not_found), &read);

    // A planned child id that names a session of another member: the fork
    // answers it as a session that does not exist and leaves it as it is.
    let case = "a planned child id that names another member's session";
    let taken = both.store(&member_session(&another_member())).await;
    let fork =
        MemberRewindFork::of_current_session(rewound_member(), source.clone(), 2, taken.clone());
    let taken_before = both
        .owner
        .load_durable_session_body(&taken)
        .await
        .expect("read the other member's session")
        .map(|session| serde_json::to_value(session.messages()).expect("messages"));
    let answers = both.ask(case, &fork).await;
    let taken_after = both
        .owner
        .load_durable_session_body(&taken)
        .await
        .expect("read the other member's session")
        .map(|session| serde_json::to_value(session.messages()).expect("messages"));
    both.expect(
        case,
        answers.fork.as_ref() == Err(&refusal(SessionError::NotFound { id: taken.clone() }))
            && taken_before.is_some()
            && taken_before == taken_after,
        &answers,
    );
    // The same for a session between an instruction delivery and its next
    // turn: whose it is is decided before anything of it is read.
    let case =
        "a planned child id that names another member's session that ends with its own activation";
    let taken = both
        .store(&ending_with_its_own_activation(&another_member()))
        .await;
    let fork =
        MemberRewindFork::of_current_session(rewound_member(), source.clone(), 2, taken.clone());
    let answers = both.ask(case, &fork).await;
    both.expect(
        case,
        answers.fork.as_ref() == Err(&refusal(SessionError::NotFound { id: taken.clone() })),
        &answers,
    );

    // The source's unanswered end: the busy answer, naming the source.
    let case = "a pending input boundary";
    let mut pending = member_session(&rewound_member());
    pending.push(meerkat_core::Message::User(
        meerkat_core::UserMessage::text("a question the member has not answered"),
    ));
    let pending = both.store(&pending).await;
    let answers = both.ask(case, &fork_of_current(&pending, 5)).await;
    let busy = refusal(SessionError::Busy {
        id: pending.clone(),
    });
    both.expect(case, answers.both_refused(&busy), &answers);

    // A source whose comms name is not its member binding's: the preflight
    // refuses it.
    let case = "a comms name that is not the member binding's";
    let mut misnamed = member_session(&rewound_member());
    let mut metadata = misnamed.session_metadata().expect("session metadata");
    metadata.comms_name = Some(
        another_member()
            .comms_name()
            .expect("comms name")
            .to_string(),
    );
    misnamed
        .set_session_metadata(metadata)
        .expect("member metadata");
    let misnamed = both.store(&misnamed).await;
    let answers = both.ask(case, &fork_of_current(&misnamed, 2)).await;
    both.expect(
        case,
        answers.preflight.as_ref().is_err_and(|error| {
            error.contains("carries a comms name that is not its member binding's")
        }),
        &answers,
    );

    // A source with metadata and no build state: a child of it could not
    // run a turn.
    let case = "a source without build state";
    let unbuilt = both
        .store(&without_build_state(&member_session(&rewound_member())))
        .await;
    let answers = both.ask(case, &fork_of_current(&unbuilt, 2)).await;
    let no_build_state = refusal(SessionError::fork_source_build_state_missing(&unbuilt));
    both.expect(case, answers.both_refused(&no_build_state), &answers);

    // A source that still holds an undelivered fork seed: the member
    // rewind child holds none.
    let case = "a source with an undelivered fork seed";
    let mut seeded = member_session(&rewound_member());
    seeded
        .try_set_metadata(
            meerkat_core::fork_notice::SESSION_FORK_SEED_KEY,
            serde_json::to_value(meerkat_core::fork_notice::ForkSeed {
                fork: meerkat_core::fork_notice::ForkNoticeKind::Transcript,
                source: meerkat_core::fork_notice::ForkNoticeSource {
                    member: Some(another_member()),
                    session_id: Some(SessionId::new()),
                },
                planned_context: None,
                source_instructions: Vec::new(),
                instructions_carried: Vec::new(),
                skills_removed: Vec::new(),
                skills_carried: Vec::new(),
                injected_rows_removed: 0,
                injected_rows_carried: 0,
                earlier_rows_omitted: 0,
                source_tools: meerkat_core::fork_notice::ForkSeedSourceTools::Read {
                    names: Vec::new(),
                },
            })
            .expect("serialize the fork seed"),
        )
        .expect("record the fork seed");
    let seeded = both.store(&seeded).await;
    let answers = both.ask(case, &fork_of_current(&seeded, 2)).await;
    both.expect(
        case,
        answers
            .child
            .as_ref()
            .is_some_and(|child| child["fork_seed"].is_null()),
        &answers,
    );

    // An archived session: no current source, and a lineage source.
    both.archive(&source).await;
    let case = "an archived session as the current source";
    let answers = both.ask(case, &fork_of_current(&source, 2)).await;
    let not_found = refusal(SessionError::NotFound { id: source.clone() });
    both.expect(case, answers.both_refused(&not_found), &answers);
    let case = "an archived session as a lineage source";
    let answers = both.ask(case, &fork_of_lineage(&source, 4)).await;
    both.expect(
        case,
        answers.fork.as_ref().is_ok_and(|observed| {
            observed.evidence.prefix_message_count == 4 && !observed.archived
        }),
        &answers,
    );
    let case = "a lineage read of an archived session";
    let read = both.ask_lineage_read(case, &source, 2).await;
    both.expect(
        case,
        read.as_ref()
            .is_ok_and(|read| read.message_count == 4 && read.prefix_digest.is_some()),
        &read,
    );

    assert!(
        both.findings.is_empty(),
        "the owner and the double answer the seam alike:\n{}",
        both.findings.join("\n")
    );
}

/// The double answers busy for a source that is running a turn, as the
/// owner does (the session rows hold a turn open on the owner).
#[tokio::test]
async fn the_test_double_answers_busy_for_a_source_that_is_running_a_turn() {
    let service = MockSessionService::new();
    let source = member_session(&rewound_member());
    let source_id = source.id().clone();
    service.replace_persisted_session(source).await;
    service.set_session_active(&source_id, true).await;
    let request = fork_of_current(&source_id, 2);
    for (door, refused) in [
        (
            "the preflight",
            service
                .preflight_member_rewind_fork(&request)
                .await
                .map(|_| ()),
        ),
        (
            "the fork",
            service.fork_member_rewind_child(&request).await.map(|_| ()),
        ),
    ] {
        assert!(
            matches!(&refused, Err(SessionError::Busy { id }) if id == &source_id),
            "{door} answers the busy refusal for a running source: {refused:?}"
        );
    }
    assert!(
        service
            .persisted_session_clone(&request.request().planned_child_session_id)
            .await
            .is_none(),
        "a refused fork leaves no child"
    );

    // Another member's running session is not found, never busy.
    let others = member_session(&another_member());
    let others_id = others.id().clone();
    service.replace_persisted_session(others).await;
    service.set_session_active(&others_id, true).await;
    let refused = service
        .preflight_member_rewind_fork(&fork_of_current(&others_id, 2))
        .await
        .expect_err("another member's session is not the member's source");
    assert!(
        matches!(&refused, SessionError::NotFound { id } if id == &others_id),
        "{refused:?}"
    );
}

/// The four methods of the seam on `MobSessionService`. Their defaults
/// refuse, so an implementation that does not answer them compiles and
/// refuses every member rewind.
const SEAM_METHODS: [&str; 4] = [
    "preflight_member_rewind_fork",
    "fork_member_rewind_child",
    "observe_member_rewind_child",
    "read_member_rewind_lineage_session",
];

/// The `MobSessionService` impl block that `header` opens in `source`, up
/// to its closing brace.
fn session_service_impl<'a>(source: &'a str, header: &str) -> Option<&'a str> {
    let at = source.find(header)?;
    let line = source[..at].rfind('\n').map_or(0, |newline| newline + 1);
    if !source[line..at].trim_start().starts_with("impl") {
        return None;
    }
    let body = &source[at..];
    Some(&body[..body.find("\n}\n")?])
}

/// The guard a wrapper of a durable owner copies: its `MobSessionService`
/// impl names each seam method and forwards it to the service it wraps.
/// This reads source text only; what the owner and the double answer is
/// compared by
/// `the_durable_owner_and_the_test_double_answer_the_member_rewind_fork_seam`.
#[test]
fn the_forward_guard_finds_each_seam_method_in_the_owner_and_the_double() {
    let owner = session_service_impl(
        include_str!("../session_service.rs"),
        "MobSessionService for meerkat_session::PersistentSessionService<B>",
    )
    .expect("the durable owner's impl is in the source");
    let double = session_service_impl(
        include_str!("../tests.rs"),
        "MobSessionService for MockSessionService {",
    )
    .expect("the test double's impl is in the source");
    for method in SEAM_METHODS {
        assert!(
            owner.contains(&format!("async fn {method}(")),
            "the durable owner answers {method}"
        );
        assert!(
            owner.contains(&format!("PersistentSessionService::<B>::{method}(")),
            "the durable owner forwards {method} to the persistent service"
        );
        assert!(
            double.contains(&format!("async fn {method}(")),
            "the test double answers {method}"
        );
    }
}

#[tokio::test]
async fn a_verified_childs_key_is_the_old_sessions_key_and_a_repeated_carry_changes_nothing() {
    let keys = KeyRoot::with_old_key().await;
    let plan = keys.plan().await;
    let child_id = SessionId::new();
    let verified = verified_child(&child_id);
    assert_eq!(verified.session_id(), &child_id);
    let old_dir = keys.dir_of(&keys.old_session_id);
    let child_dir = keys.dir_of(&child_id);
    assert!(
        !child_dir.exists(),
        "the child holds no key before the carry"
    );

    assert_eq!(
        carry_member_key(&plan, &keys.old_session_id, &verified).await,
        Ok(MemberKeyCarryOutcome::Carried)
    );
    let old_secret = std::fs::read(old_dir.join("identity.key")).expect("the old key");
    let old_public = std::fs::read(old_dir.join("identity.pub")).expect("the old public key");
    assert_eq!(
        std::fs::read(child_dir.join("identity.key")).expect("the child's key"),
        old_secret,
        "the child's key is the old session's key"
    );
    assert_eq!(
        std::fs::read(child_dir.join("identity.pub")).expect("the child's public key"),
        old_public
    );
    assert_eq!(
        files_in(&child_dir),
        vec!["identity.key".to_string(), "identity.pub".to_string()],
        "the carry leaves the key pair and nothing else"
    );
    let mode = |path: &std::path::Path| {
        std::fs::metadata(path)
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(
        mode(&child_dir.join("identity.key")),
        0o600,
        "the carried key is readable by its owner only"
    );

    // A carry already done is observed: same bytes, nothing rewritten.
    let modified = |path: &std::path::Path| {
        std::fs::metadata(path)
            .expect("key metadata")
            .modified()
            .expect("modified time")
    };
    let key_written = modified(&child_dir.join("identity.key"));
    let public_written = modified(&child_dir.join("identity.pub"));
    assert_eq!(
        carry_member_key(&plan, &keys.old_session_id, &verified).await,
        Ok(MemberKeyCarryOutcome::AlreadyCarried)
    );
    assert_eq!(
        std::fs::read(child_dir.join("identity.key")).expect("the child's key"),
        old_secret
    );
    assert_eq!(modified(&child_dir.join("identity.key")), key_written);
    assert_eq!(modified(&child_dir.join("identity.pub")), public_written);
    assert_eq!(
        files_in(&child_dir),
        vec!["identity.key".to_string(), "identity.pub".to_string()]
    );
    // The old session keeps its key pair.
    assert_eq!(
        std::fs::read(old_dir.join("identity.key")).expect("the old key"),
        old_secret
    );
}

#[tokio::test]
async fn a_carry_interrupted_before_its_public_half_is_completed_by_the_next() {
    let keys = KeyRoot::with_old_key().await;
    let plan = keys.plan().await;
    let child_id = SessionId::new();
    let verified = verified_child(&child_id);
    let child_dir = keys.dir_of(&child_id);
    assert_eq!(
        carry_member_key(&plan, &keys.old_session_id, &verified).await,
        Ok(MemberKeyCarryOutcome::Carried)
    );
    // The carry writes the secret key first: a crash after it leaves the
    // child's directory without the public half.
    std::fs::remove_file(child_dir.join("identity.pub")).expect("remove the public half");

    assert_eq!(
        carry_member_key(&plan, &keys.old_session_id, &verified).await,
        Ok(MemberKeyCarryOutcome::AlreadyCarried)
    );
    assert_eq!(
        std::fs::read(child_dir.join("identity.pub")).expect("the child's public half"),
        keys.old_key.public_key().as_bytes().to_vec(),
        "the next carry writes the public half of the key it observed"
    );
    assert_eq!(
        files_in(&child_dir),
        vec!["identity.key".to_string(), "identity.pub".to_string()]
    );
}

#[tokio::test]
async fn the_carried_key_file_loads_as_the_same_key_pair() {
    let keys = KeyRoot::with_old_key().await;
    let plan = keys.plan().await;
    let child_id = SessionId::new();
    assert_eq!(
        carry_member_key(&plan, &keys.old_session_id, &verified_child(&child_id)).await,
        Ok(MemberKeyCarryOutcome::Carried)
    );
    let child_dir = keys.dir_of(&child_id);

    // The file the carry wrote is the file a member's build loads: a change
    // of the key file layout in meerkat-comms turns this row red.
    let loaded = meerkat_comms::Keypair::load(&child_dir)
        .await
        .expect("the carried key loads");
    assert_eq!(
        loaded.public_key().as_bytes(),
        keys.old_key.public_key().as_bytes(),
        "the carried key is the member's public key"
    );
    assert_eq!(loaded.secret_bytes(), keys.old_key.secret_bytes());
    // A build of the child mints nothing: it finds the carried key.
    let built = meerkat_comms::Keypair::load_or_generate(&child_dir)
        .await
        .expect("the child's build loads its key");
    assert_eq!(
        built.public_key().as_bytes(),
        keys.old_key.public_key().as_bytes()
    );
}

#[tokio::test]
async fn an_unverified_child_never_holds_the_key() {
    let keys = KeyRoot::with_old_key().await;
    let child_id = SessionId::new();
    let (recorded, matching) = recorded_and_matching(&child_id);

    // A child whose prefix is not the recorded one is not verified.
    let mut another_prefix = matching.clone();
    another_prefix.evidence.prefix_digest = "another-digest".to_string();
    let refused = verify_member_rewind_child(&recorded, &rewound_member(), &another_prefix)
        .expect_err("a child with another prefix digest is not verified");
    assert_eq!(refused.field, MemberRewindEvidenceField::PrefixDigest);
    // Nor is a child bound to another member.
    let mut another_binding = matching;
    another_binding.member_binding = Some(another_member());
    let refused = verify_member_rewind_child(&recorded, &rewound_member(), &another_binding)
        .expect_err("a child bound to another member is not verified");
    assert_eq!(refused.field, MemberRewindEvidenceField::MemberBinding);

    // The carry takes a verified child and nothing else, and no verified
    // child was made: the child's key directory was never written.
    assert_eq!(
        files_in(&keys.dir_of(&child_id)),
        Vec::<String>::new(),
        "an unverified child holds no key"
    );
    assert!(!keys.dir_of(&child_id).exists());
}

#[tokio::test]
async fn a_key_with_no_resolvable_root_refuses_the_carry() {
    let keys = KeyRoot::with_old_key().await;
    for unavailable in [
        MemberSeatEndpointsUnavailable::NotKept,
        MemberSeatEndpointsUnavailable::Unresolved("no home".to_string()),
    ] {
        // The member's peers trust a key, and there is no root to read it
        // from: the carry is refused before anything starts.
        assert_eq!(
            plan_member_key_carry(
                Err(unavailable.clone()),
                &keys.old_session_id,
                keys.recorded_key(),
            )
            .await,
            Err(MemberKeyCarryRefusal::NoIdentityRoot(unavailable.clone()))
        );
        // No key: nothing to carry.
        assert_eq!(
            plan_member_key_carry(Err(unavailable), &keys.old_session_id, None).await,
            Ok(MemberKeyCarryPlan::NothingToCarry)
        );
    }
    // Network comms keep the key with the realm, not with the session.
    assert_eq!(
        plan_member_key_carry(
            Ok(MemberSeatEndpointSource::Network),
            &keys.old_session_id,
            None,
        )
        .await,
        Ok(MemberKeyCarryPlan::NothingToCarry)
    );

    // A resolved root that does not hold the recorded key refuses too.
    let keyless = SessionId::new();
    assert_eq!(
        plan_member_key_carry(keys.source(), &keyless, keys.recorded_key()).await,
        Err(MemberKeyCarryRefusal::SourceKeyMissing)
    );
    let another_key = meerkat_comms::Keypair::generate();
    assert_eq!(
        plan_member_key_carry(
            keys.source(),
            &keys.old_session_id,
            Some(*another_key.public_key().as_bytes()),
        )
        .await,
        Err(MemberKeyCarryRefusal::SourceKeyMismatch)
    );
    // A session without a key pair and without a recorded key carries none.
    assert_eq!(
        plan_member_key_carry(keys.source(), &keyless, None).await,
        Ok(MemberKeyCarryPlan::NothingToCarry)
    );
    let child_id = SessionId::new();
    assert_eq!(
        carry_member_key(
            &MemberKeyCarryPlan::NothingToCarry,
            &keyless,
            &verified_child(&child_id),
        )
        .await,
        Ok(MemberKeyCarryOutcome::NothingToCarry)
    );
    assert!(!keys.dir_of(&child_id).exists());
}

/// The mode of the temporary key file as the carry's link step found it.
static TEMPORARY_KEY_MODE: AtomicU32 = AtomicU32::new(u32::MAX);

/// A link step that records the temporary file's mode, then links.
fn link_recording_the_temporary_mode(
    temporary: &std::path::Path,
    key: &std::path::Path,
) -> std::io::Result<()> {
    let mode = std::fs::metadata(temporary)?.permissions().mode() & 0o777;
    TEMPORARY_KEY_MODE.store(mode, Ordering::SeqCst);
    hard_link_key(temporary, key)
}

/// A link step of a file system that cannot hard-link.
fn link_refused(_temporary: &std::path::Path, _key: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this file system cannot hard-link",
    ))
}

#[tokio::test]
async fn a_failed_key_link_leaves_no_temporary_file() {
    let keys = KeyRoot::with_old_key().await;
    let plan = keys.plan().await;
    let key_pair = vec!["identity.key".to_string(), "identity.pub".to_string()];
    let mode = |path: &std::path::Path| {
        std::fs::metadata(path)
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777
    };

    // The temporary file holds the key from its creation, so it is created
    // readable by its owner only: the link step never sees a wider mode.
    let linked = SessionId::new();
    assert_eq!(
        carry_member_key_linking(
            &plan,
            &keys.old_session_id,
            &verified_child(&linked),
            link_recording_the_temporary_mode,
        )
        .await,
        Ok(MemberKeyCarryOutcome::Carried)
    );
    assert_eq!(
        TEMPORARY_KEY_MODE.load(Ordering::SeqCst),
        0o600,
        "the temporary key file is created readable by its owner only"
    );
    assert_eq!(files_in(&keys.dir_of(&linked)), key_pair);

    // Where the link fails, the key is created exclusively instead, and the
    // temporary file is gone.
    let unlinked = SessionId::new();
    assert_eq!(
        carry_member_key_linking(
            &plan,
            &keys.old_session_id,
            &verified_child(&unlinked),
            link_refused,
        )
        .await,
        Ok(MemberKeyCarryOutcome::Carried)
    );
    let unlinked_dir = keys.dir_of(&unlinked);
    assert_eq!(
        files_in(&unlinked_dir),
        key_pair,
        "a failed link leaves no temporary file behind"
    );
    assert_eq!(mode(&unlinked_dir.join("identity.key")), 0o600);
    assert_eq!(
        meerkat_comms::Keypair::load(&unlinked_dir)
            .await
            .expect("the carried key loads")
            .secret_bytes(),
        keys.old_key.secret_bytes()
    );

    // A child that already holds another key keeps it: the carry is
    // refused, the stored key is not replaced, and no temporary file stays.
    let occupied = SessionId::new();
    let occupied_dir = keys.dir_of(&occupied);
    let other_key = meerkat_comms::Keypair::load_or_generate(&occupied_dir)
        .await
        .expect("another key pair in the child's directory");
    let links: [fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>; 2] =
        [hard_link_key, link_refused];
    for link in links {
        assert_eq!(
            carry_member_key_linking(
                &plan,
                &keys.old_session_id,
                &verified_child(&occupied),
                link,
            )
            .await,
            Err(MemberKeyCarryRefusal::ChildHoldsAnotherKey)
        );
        assert_eq!(files_in(&occupied_dir), key_pair);
        assert_eq!(
            meerkat_comms::Keypair::load(&occupied_dir)
                .await
                .expect("the stored key loads")
                .secret_bytes(),
            other_key.secret_bytes(),
            "a stored key is never replaced"
        );
    }
}
