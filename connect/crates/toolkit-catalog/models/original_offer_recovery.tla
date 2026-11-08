---- MODULE original_offer_recovery ----
EXTENDS model

\* Recovery composition slice over the unchanged generated offer module.
\* Core is an explicit environment: admission survives process loss, exact
\* session/key replay deduplicates, and reads may be unavailable. This slice
\* assumes those Core contracts; it does not prove their Rust implementation.
\* One logical offer, two bounded dispatch epochs and one optional old keyed
\* admission exercise lost answers and canonical-id deduplication. Each dispatch
\* is recorded in the owner before its call (RecordDispatchStart); the call ends
\* without native submission (its bind held or refused: the journal drops the
\* dispatch) or its answer is lost (the call never returns here); the read by key
\* decides. An Unavailable or non-final Absent read changes no owner state and is
\* not an owner input.
\* Source, protected callback, publication and multi-offer composition remain
\* separate.
CONSTANT Fault
VARIABLES alive, epoch, outbox, in_flight, dispatch_key, native_keys,
          canonical_input, raw_read, published_read, dispatched_epochs
recovery_vars == <<alive, epoch, outbox, in_flight, dispatch_key, native_keys,
                   canonical_input, raw_read, published_read, dispatched_epochs>>
all_vars == <<vars, recovery_vars>>
OriginalNativeKey == "session-one:raw-key"

RecoveryInit ==
    /\ Init
    /\ alive = TRUE
    /\ epoch = 1
    /\ outbox = FALSE
    /\ in_flight = FALSE
    /\ dispatch_key = ""
    /\ native_keys \in {{}, {OriginalNativeKey}}
    /\ canonical_input = IF native_keys = {} THEN "" ELSE "earlier-A"
    /\ raw_read = "Unread"
    /\ published_read = "Unread"
    /\ dispatched_epochs = {}

Prepare ==
    /\ alive
    /\ PrepareOriginalOffer("offer-one", "source-one", 1, "session-one",
                            "exact-input-B", "input-digest", "binding-digest", "prepare-one", epoch)
    /\ UNCHANGED recovery_vars
Seal ==
    /\ alive
    /\ SealOriginalPreparation(offer_key, revision, preparation_id, input_digest, "prepared")
    /\ UNCHANGED recovery_vars
CommitOwnerAndOutbox ==
    /\ alive
    /\ RecordOriginalSendIntent(offer_key, revision, source_key, source_revision,
                                incarnation, TRUE, TRUE, TRUE, TRUE)
    /\ outbox' = TRUE
    /\ UNCHANGED <<alive, epoch, in_flight, dispatch_key, native_keys,
                   canonical_input, raw_read, published_read, dispatched_epochs>>
Dispatch ==
    /\ alive /\ outbox /\ phase = "Offered" /\ ~in_flight
    /\ epoch \notin dispatched_epochs
    /\ RecordDispatchStart(offer_key, revision, epoch, dispatch_count + 1)
    /\ dispatch_key' = IF Fault = "fresh-key-replay" /\ epoch = 2
                       THEN "session-one:replacement-key" ELSE OriginalNativeKey
    /\ in_flight' = TRUE
    /\ dispatched_epochs' = dispatched_epochs \cup {epoch}
    /\ UNCHANGED <<alive, epoch, outbox, native_keys, canonical_input,
                   raw_read, published_read>>
CoreCommit ==
    /\ alive /\ in_flight
    /\ native_keys' = native_keys \cup {dispatch_key}
    /\ canonical_input' = IF dispatch_key \in native_keys THEN canonical_input ELSE "candidate-B"
    /\ in_flight' = FALSE
    /\ UNCHANGED <<vars, alive, epoch, outbox, dispatch_key,
                   raw_read, published_read, dispatched_epochs>>
\* The call ends without native submission (a held or refused bind).
Unsubmitted ==
    /\ alive /\ in_flight
    /\ RecordDispatchAnswer(offer_key, revision, input_digest, incarnation, dispatch_count,
                            "NotSubmitted", "")
    /\ in_flight' = FALSE
    /\ UNCHANGED <<alive, epoch, outbox, dispatch_key, native_keys, canonical_input,
                   raw_read, published_read, dispatched_epochs>>
\* A process can die before or after native commit, and before any response.
\* There is no inferred native rollback and no receipt materialized by crash.
Crash ==
    /\ alive /\ epoch = 1 /\ phase = "Offered"
    /\ alive' = FALSE /\ in_flight' = FALSE
    /\ raw_read' = "Unread" /\ published_read' = "Unread"
    /\ UNCHANGED <<vars, epoch, outbox, dispatch_key, native_keys,
                   canonical_input, dispatched_epochs>>
Restart ==
    /\ ~alive /\ epoch = 1
    /\ alive' = TRUE /\ epoch' = 2
    /\ UNCHANGED <<vars, outbox, in_flight, dispatch_key, native_keys,
                   canonical_input, raw_read, published_read, dispatched_epochs>>
ReadCore(available) ==
    /\ alive /\ phase = "Offered"
    /\ raw_read' = IF ~available THEN "Unavailable"
                   ELSE IF OriginalNativeKey \in native_keys THEN "Found" ELSE "Absent"
    /\ published_read' = IF Fault = "unavailable-as-absent" /\ ~available
                         THEN "Absent" ELSE raw_read'
    /\ UNCHANGED <<vars, alive, epoch, outbox, in_flight, dispatch_key,
                   native_keys, canonical_input, dispatched_epochs>>
ObserveAdmission ==
    /\ alive /\ phase = "Offered"
    /\ published_read = "Found" \/ (Fault = "forge-admission" /\ published_read = "Unavailable")
    /\ RetainFoundReceipt(offer_key, revision, input_digest, incarnation, epoch,
                          "Admitted", "receipt-session-one:raw-key")
    /\ outbox' = FALSE
    /\ UNCHANGED <<alive, epoch, in_flight, dispatch_key, native_keys,
                   canonical_input, raw_read, published_read, dispatched_epochs>>
\* Only a final Absent changes the owner (its evidence and the dispatches it covers).
ObserveFinalAbsence ==
    /\ alive /\ phase = "Offered"
    /\ published_read = "Absent"
    /\ RecordFinalAbsence(offer_key, revision, input_digest, incarnation, epoch, "Absent", "")
    /\ UNCHANGED recovery_vars
Finished ==
    /\ phase = "CoreObserved" /\ ~in_flight
    /\ UNCHANGED all_vars
RecoveryNext ==
    \/ Prepare \/ Seal \/ CommitOwnerAndOutbox \/ Dispatch \/ CoreCommit \/ Unsubmitted
    \/ Crash \/ Restart \/ (\E available \in BOOLEAN : ReadCore(available))
    \/ ObserveAdmission \/ ObserveFinalAbsence \/ Finished
RecoverySpec == RecoveryInit /\ [][RecoveryNext]_all_vars

I3_ExactKeyAtMostOnce == Cardinality(native_keys) <= 1
I7_CoreReceiptProvenance ==
    core_receipt_ref = "" \/
        (core_receipt_ref = "receipt-session-one:raw-key" /\ OriginalNativeKey \in native_keys)
I13_ReadClassification == published_read = raw_read
OwnerBeforeDispatch == dispatched_epochs = {} \/ phase \in {"Offered", "CoreObserved"}
OwnerObligationSurvivesCrash == phase # "Offered" \/ outbox
CanonicalDedupIdentity == (canonical_input = "earlier-A") => OriginalNativeKey \in native_keys
RecoveryBound == model_step_count <= 8
=============================================================================
