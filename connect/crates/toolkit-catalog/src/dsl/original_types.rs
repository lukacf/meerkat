//! Bounded vocabulary for the source and offer catalogs.
//!
//! Outcome values are observations supplied by Core, never Connect decisions.
//! The bridge must verify the observation and its exact key before applying it.

macro_rules! bounded_enum {
    ($name:ident { $first:ident $(, $rest:ident)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
                 serde::Serialize, serde::Deserialize)]
        pub enum $name {
            #[default]
            $first,
            $($rest,)*
        }

        impl $name {
            pub(crate) const MODEL_VARIANTS: &'static [&'static str] =
                &[stringify!($first), $(stringify!($rest)),*];
        }
    };
}

bounded_enum!(ToolkitSourceKind { Ordinary, Helper });
bounded_enum!(ToolkitSourceReadiness {
    ClosedToOffers,
    Preparing,
    Ready
});
bounded_enum!(ToolkitCoreObservation {
    Unresolved,
    Admitted,
    NotAdmitted,
    Absent,
    Unavailable,
});
// The answer to one dispatch of an offer's intent, as its call returned: admitted with
// Core's receipt; refused by Core or by the entry fence (a typed refusal before native
// submission); lost (AdmissionUnresolved: admitted or not); or ended without any native
// submission (a held or refused bind, a volatile drop, a supervisor NotReady).
bounded_enum!(ToolkitDispatchAnswer {
    NotSubmitted,
    Admitted,
    CoreRefused,
    EntryRefused,
    Unresolved,
});
// What a terminal receipt settles: a dispatch's admission, a read by key that found the
// admission, Core's refusal or the entry fence's typed refusal of one dispatch.
bounded_enum!(ToolkitReceiptKind {
    Unsettled,
    Admitted,
    Found,
    CoreRefused,
    EntryRefused,
});
// Why an offer was withdrawn before any dispatch: a verdict on the delivery (its authority
// denied, its source closed or changed), or not (the component's admission closed, its session
// replaced during the preparation, its preparing process ended), when the same delivery may be
// prepared again.
bounded_enum!(ToolkitWithdrawalCause {
    NotWithdrawn,
    PolicyDenied,
    SourceClosed,
    AdmissionClosed,
    PreparationStale,
    ProcessEnded,
});
// Which of Core's reads by key answered: the durable read, which takes no gate (an admission
// whose write is still in flight after its call returned may land after it answers), or the
// live read, which takes the session's mutation gate and then its driver lock and so is ordered
// after any admission write in flight on the session.
bounded_enum!(ToolkitCoreRead { Durable, Live });
